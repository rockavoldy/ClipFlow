//! Adaptive capture (ECO) loop.
//!
//! Polls the battery/RAM snapshot every 5 s, always — so a future telemetry row
//! stays live even while ECO is off. Once the ECO state has been stable for a
//! few seconds, the rolling buffer shrinks to 30 s on the **live** engine
//! (never touching the persisted setting) and restores itself when conditions
//! clear.
//!
//! Deferred with the STATS view: the threshold *controls* are not exposed in
//! phase 1, but the setting values are still read, so enabling adaptive ECO
//! from `settings.json` works.

use std::time::Duration;

use gpui_kit::{AppContext as _, WeakEntity};

use crate::policy::eco::{DEFAULT_WINDOW_MS, EcoHysteresis, EcoInput, decide_eco};
use crate::power::PowerState;
use crate::state::AppState;
use crate::ui::deck::Deck;

/// 5 s, matching the old `useEffect` poll.
pub const POLL: Duration = Duration::from_secs(5);

/// Loop state, split from the view so the policy stays testable.
#[derive(Debug)]
pub struct EcoRun {
    hysteresis: EcoHysteresis,
    /// Buffer that was applied by ECO, or `None` when not shrunk.
    applied_buffer: Option<u32>,
}

impl EcoRun {
    pub fn new() -> Self {
        Self {
            hysteresis: EcoHysteresis::new(DEFAULT_WINDOW_MS),
            applied_buffer: None,
        }
    }

    /// One poll. Returns the buffer length to push to the live engine, if it
    /// needs changing.
    ///
    /// Turning ECO off restores the configured buffer immediately, exactly as
    /// the old loop did.
    pub fn observe(
        &mut self,
        power: &PowerState,
        settings: &crate::settings::Settings,
        now_ms: u64,
    ) -> Option<u32> {
        let input = EcoInput::from_settings(settings);

        if !input.adaptive_eco {
            // ECO off: restore any shrunk buffer right away, then idle.
            if self.applied_buffer.take().is_some() {
                return Some(settings.buffer_seconds);
            }
            return None;
        }

        let decision = decide_eco(power, &input);
        let flip = self.hysteresis.tick(decision.active, now_ms)?;
        self.hysteresis.commit(flip);

        // The buffer that *should* be live: ECO's length while active, the
        // user's configured one otherwise.
        let target = if flip {
            decision.eco_buffer_seconds
        } else {
            settings.buffer_seconds
        };
        // `applied_buffer` records what ECO changed; `None` means "the live
        // buffer is the configured one".
        let currently_live = self.applied_buffer.unwrap_or(settings.buffer_seconds);
        if target == currently_live {
            // Nothing to do. This also covers the case where ECO is active but
            // the configured buffer is already <= 30 s: re-applying would
            // re-allocate the ring every 5 s for no benefit.
            return None;
        }
        self.applied_buffer = (target != settings.buffer_seconds).then_some(target);
        Some(target)
    }
}

impl Default for EcoRun {
    fn default() -> Self {
        Self::new()
    }
}

/// Starts the loop. `reading_power` is a function pointer rather than a call so
/// the UI never depends on the Windows power module directly.
pub fn start(
    deck: WeakEntity<Deck>,
    state: std::sync::Arc<AppState>,
    cx: &mut gpui_kit::App,
) {
    cx.spawn(async move |cx| {
        let mut run = EcoRun::new();
        let started = std::time::Instant::now();

        loop {
            let state_bg = std::sync::Arc::clone(&state);
            let sample = cx
                .background_spawn(async move {
                    let power = crate::power::read_power_state().ok();
                    let settings = state_bg.settings_snapshot();
                    (power, settings)
                })
                .await;

            let (power, settings) = sample;
            // Telemetry is unavailable this tick — try again next poll.
            if let Some(power) = power {
                let now_ms = started.elapsed().as_millis() as u64;
                if let Some(buffer) = run.observe(&power, &settings, now_ms) {
                    state.set_live_buffer_seconds(buffer);
                    deck.update(cx, |deck, cx| {
                        deck.on_power_snapshot(power);
                        cx.notify();
                    })
                    .ok();
                } else {
                    // Still show the telemetry even when nothing changed.
                    deck.update(cx, |deck, cx| {
                        deck.on_power_snapshot(power);
                        cx.notify();
                    })
                    .ok();
                }
            }

            cx.background_executor().timer(POLL).await;
        }
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::eco::ECO_BUFFER_SECONDS;

    const S: u64 = 1_000;
    const GIB: u64 = 1024 * 1024 * 1024;

    fn settings(adaptive_eco: bool) -> crate::settings::Settings {
        crate::settings::Settings {
            adaptive_eco,
            buffer_seconds: 60,
            target_fps: 60,
            eco_battery_threshold_pct: 30,
            eco_ram_free_gbs: 4,
            ..crate::settings::Settings::default()
        }
    }

    fn power(on_battery: bool, pct: u8, free_gibs: u64) -> PowerState {
        PowerState {
            on_battery,
            battery_percent: pct,
            available_ram_bytes: free_gibs * GIB,
            total_ram_bytes: 16 * GIB,
        }
    }

    #[test]
    fn eco_off_never_touches_the_buffer() {
        let mut run = EcoRun::new();
        let s = settings(false);
        for t in 0..10 {
            assert_eq!(run.observe(&power(true, 5, 1), &s, t * S), None);
        }
    }

    #[test]
    fn shrinking_waits_for_the_hysteresis_window() {
        let mut run = EcoRun::new();
        let s = settings(true);
        let low = power(true, 10, 16);
        assert_eq!(run.observe(&low, &s, 0), None);
        assert_eq!(run.observe(&low, &s, DEFAULT_WINDOW_MS - 1), None);
        assert_eq!(
            run.observe(&low, &s, DEFAULT_WINDOW_MS),
            Some(ECO_BUFFER_SECONDS)
        );
    }

    #[test]
    fn a_cleared_condition_restores_the_configured_buffer() {
        let mut run = EcoRun::new();
        let s = settings(true);
        let low = power(true, 10, 16);
        run.observe(&low, &s, 0);
        assert_eq!(
            run.observe(&low, &s, DEFAULT_WINDOW_MS),
            Some(ECO_BUFFER_SECONDS)
        );

        let healthy = power(false, 100, 16);
        let t = DEFAULT_WINDOW_MS + 1;
        assert_eq!(run.observe(&healthy, &s, t), None);
        assert_eq!(
            run.observe(&healthy, &s, t + DEFAULT_WINDOW_MS),
            Some(60)
        );
    }

    #[test]
    fn disabling_eco_restores_the_buffer_immediately() {
        let mut run = EcoRun::new();
        let s = settings(true);
        let low = power(true, 10, 16);
        run.observe(&low, &s, 0);
        run.observe(&low, &s, DEFAULT_WINDOW_MS);

        let off = settings(false);
        // No hysteresis on the way out: the user's value comes back at once.
        assert_eq!(
            run.observe(&power(true, 10, 16), &off, DEFAULT_WINDOW_MS + S),
            Some(60)
        );
        // ...and only once.
        assert_eq!(
            run.observe(&power(true, 10, 16), &off, DEFAULT_WINDOW_MS + 2 * S),
            None
        );
    }

    #[test]
    fn an_already_short_buffer_is_not_changed() {
        let mut run = EcoRun::new();
        let s = settings(true);
        let mut s = s;
        s.buffer_seconds = ECO_BUFFER_SECONDS; // already 30
        let low = power(true, 10, 16);
        run.observe(&low, &s, 0);
        // The decision says 30, which is already live -> no redundant engine
        // call (that would re-allocate the ring every 5 s).
        assert_eq!(run.observe(&low, &s, DEFAULT_WINDOW_MS), None);
    }
}

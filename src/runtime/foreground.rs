//! The 2 s foreground poll: privacy gate + auto-save-on-game-exit.
//!
//! Port of the `useEffect` in `App.tsx:520`, minus profile auto-switching
//! (deferred with the profiles UI) and auto-save-on-exit kept intact because it
//! is a shipping feature.
//!
//! Ordering inside one tick is load-bearing and preserved:
//!
//! 1. **Auto-save first**, while `last_focused_exe` still names the game that
//!    just lost focus — the flush carries that exe so the clip lands in the
//!    right per-game folder.
//! 2. **Privacy gate second**, so the last 30 s of a closed game are flushed
//!    *before* the gate drops the ring history.
//!
//! Two thresholds are user-visible contracts from the original: a game must
//! have held focus for at least [`FOCUS_MIN_MS`] before its exit auto-saves,
//! and two auto-saves are at least [`AUTOSAVE_COOLDOWN_MS`] apart.

use std::time::Duration;

use gpui_kit::{AppContext as _, WeakEntity};

use crate::policy::privacy::{PrivacyGateHysteresis, DEFAULT_WINDOW_MS};
use crate::state::{AppState, SaveTrigger};
use crate::ui::deck::Deck;

/// How often the foreground is sampled.
pub const POLL: Duration = Duration::from_millis(2_000);
/// A game must have held focus this long before its exit triggers an autosave.
pub const FOCUS_MIN_MS: u64 = 10_000;
/// Minimum gap between two auto-saves.
pub const AUTOSAVE_COOLDOWN_MS: u64 = 60_000;
/// Auto-saves flush this much of the ring.
pub const AUTOSAVE_SECONDS: f32 = 30.0;

/// Mutable state owned by the loop. Split out from the view so the whole policy
/// is testable without a window.
#[derive(Debug)]
pub struct ForegroundWatcher {
    pub gate: PrivacyGateHysteresis,
    last_focused_exe: Option<String>,
    focused_since_ms: u64,
    /// `None` until the first autosave: the cooldown must not suppress the
    /// very first one just because the clock started at zero.
    last_autosave_ms: Option<u64>,
}

impl ForegroundWatcher {
    pub fn new() -> Self {
        Self {
            gate: PrivacyGateHysteresis::new(DEFAULT_WINDOW_MS),
            last_focused_exe: None,
            focused_since_ms: 0,
            last_autosave_ms: None,
        }
    }

    /// What one poll decided to do. Returned instead of performed so the
    /// decision can be asserted in tests.
    pub fn observe(
        &mut self,
        exe: Option<String>,
        now_ms: u64,
        privacy_enabled: bool,
        autosave_enabled: bool,
    ) -> Decision {
        let mut decision = Decision::default();

        // --- 1. auto-save on game exit -------------------------------------
        // Compare against the *previous* foreground, which is the game that
        // just lost focus — that is why the flush carries `last_focused_exe`.
        if let (Some(previous), Some(current)) = (self.last_focused_exe.clone(), exe.as_ref()) {
            let cooldown_clear = self
                .last_autosave_ms
                .is_none_or(|last| now_ms.saturating_sub(last) >= AUTOSAVE_COOLDOWN_MS);
            if &previous != current
                && autosave_enabled
                && now_ms.saturating_sub(self.focused_since_ms) >= FOCUS_MIN_MS
                && cooldown_clear
            {
                self.last_autosave_ms = Some(now_ms);
                decision.autosave = Some(previous);
            }
        }
        if exe != self.last_focused_exe {
            self.focused_since_ms = now_ms;
            self.last_focused_exe = exe.clone();
        }

        // --- 2. privacy gate ----------------------------------------------
        // Only meaningful while privacy mode is on; `tick` un-gates instantly
        // and one-shot when it is off.
        let desired = crate::policy::privacy::should_gate_for_foreground(exe.as_deref());
        decision.gate = self.gate.tick(privacy_enabled, desired, now_ms);

        decision
    }

    /// Commits a gate value the engine accepted. Must not be called on failure:
    /// the next tick then retries the apply.
    pub fn commit_gate(&mut self, gate: bool) {
        self.gate.commit(gate);
    }
}

impl Default for ForegroundWatcher {
    fn default() -> Self {
        Self::new()
    }
}

/// What one poll wants done. Empty means "nothing changed".
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Decision {
    /// Gate value to push to the engine, if it changed.
    pub gate: Option<bool>,
    /// The exe that just lost focus, if an autosave is due.
    pub autosave: Option<String>,
}

/// Starts the poll. Blocking foreground queries happen on the background
/// executor.
pub fn start(deck: WeakEntity<Deck>, state: std::sync::Arc<AppState>, cx: &mut gpui_kit::App) {
    cx.spawn(async move |cx| {
        let mut watcher = ForegroundWatcher::new();
        let started = std::time::Instant::now();

        loop {
            let state_bg = std::sync::Arc::clone(&state);
            let sample = cx
                .background_spawn(async move {
                    let exe = crate::state::get_foreground_game().map(|g| g.exe.to_ascii_lowercase());
                    let settings = state_bg.settings_snapshot();
                    (exe, settings)
                })
                .await;

            let (exe, settings) = sample;
            let now_ms = started.elapsed().as_millis() as u64;
            let decision = watcher.observe(
                exe,
                now_ms,
                settings.privacy_pause_when_unfocused,
                settings.autosave_on_game_exit,
            );

            // --- autosave (blocking flush, so off the UI thread) -----------
            if let Some(previous_exe) = decision.autosave {
                let state_bg = std::sync::Arc::clone(&state);
                let saved = cx
                    .background_spawn(async move {
                        state_bg
                            .save_instant_replay(
                                Some(AUTOSAVE_SECONDS),
                                SaveTrigger::Autosave,
                                Some(previous_exe.clone()),
                            )
                            .map(|outcome| (outcome, previous_exe))
                    })
                    .await;

                if let Ok((outcome, exe)) = saved {
                        deck.update(cx, |deck, cx| {
                            // The deck is usually hidden while a game exits, so
                            // the native toast is the only feedback the user
                            // gets.
                            crate::platform::notify::clip_saved(
                                cx,
                                &outcome.clip.file_name,
                                outcome.clip.duration_seconds,
                                SaveTrigger::Autosave.as_str(),
                            );
                            deck.on_clip_saved(outcome, format!("{exe} — last 30 s saved"));
                            cx.notify();
                        })
                        .ok();
                }
                // `Err`: buffer not armed (or privacy refused) — nothing to save.
            }

            // --- privacy gate ---------------------------------------------
            if let Some(gate) = decision.gate {
                let state_bg = std::sync::Arc::clone(&state);
                cx.background_spawn(async move { state_bg.set_privacy_gate(gate) })
                    .await;
                watcher.commit_gate(gate);

                // Only announce transitions that the user caused by toggling
                // privacy mode or by alt-tabbing away. The one-shot un-gate that
                // runs when privacy is switched *off* is silent: the settings
                // row already says it.
                if settings.privacy_pause_when_unfocused {
                    let (title, body) = if gate {
                        (
                            "Privacy mode paused",
                            "No game is focused — nothing is being recorded right now.",
                        )
                    } else {
                        ("Privacy mode resumed", "A game is focused — capture is live.")
                    };
                    deck.update(cx, |deck, cx| {
                        deck.status_info(title, body);
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

    /// The gate needs to have been stable for its 5 s window before it fires,
    /// which makes these tests talk in seconds.
    const S: u64 = 1_000;

    fn watcher() -> ForegroundWatcher {
        ForegroundWatcher::new()
    }

    #[test]
    fn a_game_that_held_focus_for_long_enough_autosaves_on_exit() {
        let mut w = watcher();
        // Game focused at t=0.
        let d = w.observe(Some("cs2.exe".into()), 0, false, true);
        assert_eq!(d.autosave, None);
        // Still focused 20 s later (past FOCUS_MIN_MS).
        let d = w.observe(Some("cs2.exe".into()), 20 * S, false, true);
        assert_eq!(d.autosave, None);
        // Desktop takes focus: the *game* is what gets tagged.
        let d = w.observe(Some("explorer.exe".into()), 22 * S, false, true);
        assert_eq!(d.autosave.as_deref(), Some("cs2.exe"));
    }

    #[test]
    fn a_quick_alt_tab_does_not_autosave() {
        let mut w = watcher();
        w.observe(Some("cs2.exe".into()), 0, false, true);
        // Only 4 s of focus: below FOCUS_MIN_MS.
        let d = w.observe(Some("explorer.exe".into()), 4 * S, false, true);
        assert_eq!(d.autosave, None);
    }

    #[test]
    fn the_cooldown_prevents_back_to_back_autosaves() {
        let mut w = watcher();
        w.observe(Some("cs2.exe".into()), 0, false, true);
        // First autosave at 20 s.
        let d = w.observe(Some("explorer.exe".into()), 20 * S, false, true);
        assert_eq!(d.autosave.as_deref(), Some("cs2.exe"));

        // Back into a game, then out again 10 s later: still inside the 60 s
        // cooldown, so nothing fires.
        w.observe(Some("elden.exe".into()), 22 * S, false, true);
        let d = w.observe(Some("explorer.exe".into()), 40 * S, false, true);
        assert_eq!(d.autosave, None);

        // Past the cooldown, the next exit does fire.
        w.observe(Some("elden.exe".into()), 50 * S, false, true);
        let d = w.observe(Some("explorer.exe".into()), 90 * S, false, true);
        assert_eq!(d.autosave.as_deref(), Some("elden.exe"));
    }

    #[test]
    fn autosave_respects_the_setting() {
        let mut w = watcher();
        w.observe(Some("cs2.exe".into()), 0, false, false);
        let d = w.observe(Some("explorer.exe".into()), 30 * S, false, false);
        assert_eq!(d.autosave, None);
    }

    #[test]
    fn the_privacy_gate_and_autosave_are_evaluated_in_the_right_order() {
        let mut w = watcher();
        // Game focused long enough to be autosavable.
        w.observe(Some("cs2.exe".into()), 0, true, true);
        w.observe(Some("cs2.exe".into()), 20 * S, true, true);
        // Exit to desktop: the autosave must be reported in the SAME decision
        // that also starts the privacy gate countdown, so the caller flushes
        // before dropping the ring.
        let d = w.observe(Some("explorer.exe".into()), 22 * S, true, true);
        assert_eq!(d.autosave.as_deref(), Some("cs2.exe"));
        // Gate is not applied yet (5 s hysteresis), which is what protects the
        // flush above.
        assert_eq!(d.gate, None);
    }

    #[test]
    fn the_gate_only_fires_after_its_window() {
        let mut w = watcher();
        w.observe(Some("explorer.exe".into()), 0, true, false);
        assert_eq!(w.observe(Some("explorer.exe".into()), 4 * S, true, false).gate, None);
        assert_eq!(
            w.observe(Some("explorer.exe".into()), 5 * S, true, false).gate,
            Some(true)
        );
    }

    #[test]
    fn disabling_privacy_ungates_immediately() {
        let mut w = watcher();
        w.observe(Some("explorer.exe".into()), 0, true, false);
        let d = w.observe(Some("explorer.exe".into()), 5 * S, true, false);
        assert_eq!(d.gate, Some(true));
        w.commit_gate(true);
        // Off: one-shot un-gate, no window.
        let d = w.observe(Some("explorer.exe".into()), 6 * S, false, false);
        assert_eq!(d.gate, Some(false));
    }

    #[test]
    fn a_failed_foreground_query_keeps_recording() {
        let mut w = watcher();
        // `None` = elevated game / protected process / lock screen.
        w.observe(None, 0, true, false);
        let d = w.observe(None, 10 * S, true, false);
        assert_eq!(d.gate, Some(false));
    }

    #[test]
    fn the_very_first_poll_never_autosaves() {
        // There is no "previous" game to tag, and flushing on launch would
        // save whatever the desktop was doing.
        let mut w = watcher();
        let d = w.observe(Some("explorer.exe".into()), 100 * S, false, true);
        assert_eq!(d.autosave, None);
    }
}

//! Adaptive capture (ECO) policy — pure and unit-testable.
//!
//! The deck polls the power snapshot every few seconds and runs [`decide_eco`]
//! over it. When the machine is on battery at/below the configured threshold,
//! or free RAM drops under the threshold, the rolling buffer shrinks to
//! [`ECO_BUFFER_SECONDS`] (live, engine-only) and fps is capped at
//! [`ECO_FPS`] (applies on the next engine start, same contract as profiles).
//! When conditions clear, both restore to the user's configured values.
//!
//! [`EcoHysteresis`] mirrors [`super::privacy::PrivacyGateHysteresis`]: the
//! desired state must hold for the window before it flips, so a momentary RAM
//! blip never churns the buffer or spams toasts.

use crate::power::PowerState;

/// Ring length used while ECO is active.
pub const ECO_BUFFER_SECONDS: u32 = 30;
/// FPS cap applied on the next engine start while ECO is active.
pub const ECO_FPS: u32 = 30;
/// Default stability window: 6 s, matching `ECO_HYSTERESIS_MS` in `App.tsx`.
pub const DEFAULT_WINDOW_MS: u64 = 6_000;

const BYTES_PER_GIB: u64 = 1024 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EcoReason {
    Battery,
    Ram,
    Off,
}

impl EcoReason {
    pub fn as_str(self) -> &'static str {
        match self {
            EcoReason::Battery => "battery",
            EcoReason::Ram => "ram",
            EcoReason::Off => "off",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EcoDecision {
    pub active: bool,
    pub reason: EcoReason,
    /// The buffer window that should be live right now.
    pub eco_buffer_seconds: u32,
    /// The fps cap that should apply (takes effect on the next engine start).
    pub eco_fps: u32,
}

/// The subset of settings the ECO decision reads.
#[derive(Debug, Clone, Copy)]
pub struct EcoInput {
    pub adaptive_eco: bool,
    pub eco_battery_threshold_pct: u32,
    pub eco_ram_free_gbs: u32,
    pub buffer_seconds: u32,
    pub target_fps: u32,
}

impl EcoInput {
    pub fn from_settings(s: &crate::settings::Settings) -> Self {
        Self {
            adaptive_eco: s.adaptive_eco,
            eco_battery_threshold_pct: s.eco_battery_threshold_pct,
            eco_ram_free_gbs: s.eco_ram_free_gbs,
            buffer_seconds: s.buffer_seconds,
            target_fps: s.target_fps,
        }
    }
}

pub fn decide_eco(power: &PowerState, settings: &EcoInput) -> EcoDecision {
    if !settings.adaptive_eco {
        return EcoDecision {
            active: false,
            reason: EcoReason::Off,
            eco_buffer_seconds: settings.buffer_seconds,
            eco_fps: settings.target_fps,
        };
    }

    let low_battery =
        power.on_battery && u32::from(power.battery_percent) <= settings.eco_battery_threshold_pct;
    let low_ram = power.available_ram_bytes < settings.eco_ram_free_gbs as u64 * BYTES_PER_GIB;
    let active = low_battery || low_ram;

    EcoDecision {
        active,
        // Battery wins the label when both fire: it is the more actionable
        // signal for the user.
        reason: if low_battery {
            EcoReason::Battery
        } else if low_ram {
            EcoReason::Ram
        } else {
            EcoReason::Off
        },
        // Never *raise* the buffer: if the user configured 15 s, ECO keeps 15.
        eco_buffer_seconds: if active {
            settings.buffer_seconds.min(ECO_BUFFER_SECONDS)
        } else {
            settings.buffer_seconds
        },
        eco_fps: if active {
            settings.target_fps.min(ECO_FPS)
        } else {
            settings.target_fps
        },
    }
}

/// Stability gate for the ECO *active* boolean. The buffer only changes once
/// the desired state has held for the whole window — in either direction, so a
/// RAM blip cannot shrink the buffer for a few seconds and restore it again.
#[derive(Debug, Clone)]
pub struct EcoHysteresis {
    window_ms: u64,
    last_desired: Option<bool>,
    applied: Option<bool>,
    stable_since: u64,
}

impl EcoHysteresis {
    pub fn new(window_ms: u64) -> Self {
        Self {
            window_ms,
            last_desired: None,
            applied: None,
            stable_since: 0,
        }
    }

    /// Feed one poll. Returns the desired state to apply, or `None` for no
    /// change.
    pub fn tick(&mut self, desired: bool, now_ms: u64) -> Option<bool> {
        if self.last_desired != Some(desired) {
            self.last_desired = Some(desired);
            self.stable_since = now_ms;
        }
        if now_ms.saturating_sub(self.stable_since) >= self.window_ms
            && self.applied != Some(desired)
        {
            return Some(desired);
        }
        None
    }

    /// Call after the engine apply for the value `tick()` returned succeeded.
    pub fn commit(&mut self, value: bool) {
        self.applied = Some(value);
    }

    pub fn is_active(&self) -> bool {
        self.applied == Some(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn power(on_battery: bool, battery_percent: u8, available_gib: u64) -> PowerState {
        PowerState {
            on_battery,
            battery_percent,
            available_ram_bytes: available_gib * BYTES_PER_GIB,
            total_ram_bytes: 16 * BYTES_PER_GIB,
        }
    }

    fn input(adaptive_eco: bool) -> EcoInput {
        EcoInput {
            adaptive_eco,
            eco_battery_threshold_pct: 30,
            eco_ram_free_gbs: 4,
            buffer_seconds: 60,
            target_fps: 60,
        }
    }

    #[test]
    fn eco_off_keeps_the_configured_values() {
        let d = decide_eco(&power(true, 5, 1), &input(false));
        assert!(!d.active);
        assert_eq!(d.reason, EcoReason::Off);
        assert_eq!(d.eco_buffer_seconds, 60);
        assert_eq!(d.eco_fps, 60);
    }

    #[test]
    fn on_ac_with_plenty_of_ram_is_not_active() {
        let d = decide_eco(&power(false, 100, 16), &input(true));
        assert!(!d.active);
        assert_eq!(d.reason, EcoReason::Off);
        assert_eq!(d.eco_buffer_seconds, 60);
        assert_eq!(d.eco_fps, 60);
    }

    #[test]
    fn low_battery_activates_eco() {
        let d = decide_eco(&power(true, 20, 16), &input(true));
        assert!(d.active);
        assert_eq!(d.reason, EcoReason::Battery);
        assert_eq!(d.eco_buffer_seconds, 30);
        assert_eq!(d.eco_fps, 30);
    }

    #[test]
    fn battery_exactly_at_the_threshold_activates() {
        // `<=`, not `<`: this is the documented behaviour and easy to flip by
        // accident.
        let d = decide_eco(&power(true, 30, 16), &input(true));
        assert!(d.active);
        assert_eq!(d.reason, EcoReason::Battery);
    }

    #[test]
    fn battery_on_ac_never_activates_from_the_battery_rule() {
        // A low percentage while charging (or an unknown value reported as 0
        // on AC) must not shrink the buffer.
        let d = decide_eco(&power(false, 5, 16), &input(true));
        assert!(!d.active);
    }

    #[test]
    fn low_ram_activates_eco_without_battery_pressure() {
        let d = decide_eco(&power(false, 100, 3), &input(true));
        assert!(d.active);
        assert_eq!(d.reason, EcoReason::Ram);
        assert_eq!(d.eco_buffer_seconds, 30);
    }

    #[test]
    fn ram_exactly_at_the_threshold_is_enough() {
        // `available < threshold` is false at equality.
        let d = decide_eco(&power(false, 100, 4), &input(true));
        assert!(!d.active);
    }

    #[test]
    fn battery_reason_wins_when_both_rules_fire() {
        let d = decide_eco(&power(true, 10, 1), &input(true));
        assert!(d.active);
        assert_eq!(d.reason, EcoReason::Battery);
    }

    #[test]
    fn eco_never_raises_a_shorter_configured_buffer_or_fps() {
        let mut i = input(true);
        i.buffer_seconds = 15;
        i.target_fps = 24;
        let d = decide_eco(&power(true, 5, 1), &i);
        assert!(d.active);
        assert_eq!(d.eco_buffer_seconds, 15);
        assert_eq!(d.eco_fps, 24);
    }

    #[test]
    fn hysteresis_waits_for_the_window_in_both_directions() {
        let mut h = EcoHysteresis::new(6_000);
        assert_eq!(h.tick(true, 0), None);
        assert_eq!(h.tick(true, 5_999), None);
        assert_eq!(h.tick(true, 6_000), Some(true));
        h.commit(true);
        assert!(h.is_active());

        // Clearing also waits.
        assert_eq!(h.tick(false, 7_000), None);
        assert_eq!(h.tick(false, 13_000), Some(false));
        h.commit(false);
        assert!(!h.is_active());
    }

    #[test]
    fn a_ram_blip_does_not_shrink_the_buffer() {
        let mut h = EcoHysteresis::new(6_000);
        assert_eq!(h.tick(false, 0), None);
        assert_eq!(h.tick(false, 6_000), Some(false));
        h.commit(false);
        // 2 s of "active", then back: never applied.
        assert_eq!(h.tick(true, 7_000), None);
        assert_eq!(h.tick(false, 9_000), None);
        assert_eq!(h.tick(false, 15_000), None);
        assert!(!h.is_active());
    }
}

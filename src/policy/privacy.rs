//! Privacy-mode gate hysteresis.
//!
//! The 2 s foreground poll computes the *desired* gate state every tick, but
//! the engine must only learn about it once the state has been stable for the
//! window — a quick alt-tab or a transient detection blip must never wipe the
//! ring buffer, re-force an encoder key frame or spam toasts. Turning privacy
//! off un-gates immediately.
//!
//! The clock is injected so the policy is testable without sleeping.

/// Default stability window: 5 s, matching `PRIVACY_HYSTERESIS_MS` in
/// `App.tsx`.
pub const DEFAULT_WINDOW_MS: u64 = 5_000;

/// Pure hysteresis state machine. One instance lives for the app's lifetime.
#[derive(Debug, Clone)]
pub struct PrivacyGateHysteresis {
    window_ms: u64,
    /// Last desired gate state (None = never computed yet).
    desired: Option<bool>,
    /// Last gate state that was successfully applied to the engine.
    applied: Option<bool>,
    /// When `desired` last changed (clock units).
    changed_at: u64,
}

impl PrivacyGateHysteresis {
    pub fn new(window_ms: u64) -> Self {
        Self {
            window_ms,
            desired: None,
            applied: None,
            changed_at: 0,
        }
    }

    /// Feed one foreground poll.
    ///
    /// * `privacy_enabled` — the Privacy mode setting.
    /// * `desired_gate` — what the current foreground implies: `true` pauses
    ///   (desktop / ClipFlow focused), `false` records (game focused).
    /// * `now_ms` — monotonic clock in milliseconds.
    ///
    /// Returns the gate value to apply to the engine, or `None` when nothing
    /// should change.
    pub fn tick(&mut self, privacy_enabled: bool, desired_gate: bool, now_ms: u64) -> Option<bool> {
        // Privacy off: un-gate right away (one-shot), regardless of stability.
        // The engine gate follows the setting instantly, mirroring the old
        // reset-on-toggle behaviour.
        if !privacy_enabled {
            let was_active = self.desired.is_some() || self.applied.is_some();
            self.desired = None;
            self.applied = None;
            self.changed_at = 0;
            return if was_active { Some(false) } else { None };
        }

        if self.desired != Some(desired_gate) {
            self.desired = Some(desired_gate);
            self.changed_at = now_ms;
        }

        // Only hand over a gate change once the state has been stable for the
        // whole window, and never re-apply what the engine already has.
        if now_ms.saturating_sub(self.changed_at) >= self.window_ms
            && self.applied != Some(desired_gate)
        {
            return Some(desired_gate);
        }
        None
    }

    /// Call after the engine apply for the value `tick()` returned succeeded.
    ///
    /// Must NOT be called on failure: the next tick then returns the same value
    /// again, which retries the apply.
    pub fn commit(&mut self, gate: bool) {
        self.applied = Some(gate);
    }

    /// Whether the engine currently holds the paused gate (for badges).
    pub fn is_gated(&self) -> bool {
        self.applied == Some(true)
    }
}

/// Mirrors the Rust `privacy_should_gate()`: pause only when the foreground is
/// *positively* ClipFlow itself or the desktop (`explorer.exe`). A failed query
/// (`None`/empty) means "keep recording" — breaking `Alt+C` for an undetected
/// game is worse than a rare ambiguous frame.
pub fn should_gate_for_foreground(exe: Option<&str>) -> bool {
    match exe {
        Some(exe) if !exe.is_empty() => {
            let lower = exe.to_ascii_lowercase();
            lower.starts_with("clipflow") || lower.starts_with("explorer")
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn does_not_flip_before_the_window_elapses() {
        let mut h = PrivacyGateHysteresis::new(5_000);
        // Desktop focused at t=0 -> desired gate, but not applied yet.
        assert_eq!(h.tick(true, true, 0), None);
        assert_eq!(h.tick(true, true, 4_999), None);
        assert_eq!(h.tick(true, true, 5_000), Some(true));
    }

    #[test]
    fn never_reapplies_what_the_engine_already_has() {
        let mut h = PrivacyGateHysteresis::new(1_000);
        assert_eq!(h.tick(true, true, 0), None);
        assert_eq!(h.tick(true, true, 1_000), Some(true));
        h.commit(true);
        // Same state, window elapsed -> still nothing.
        assert_eq!(h.tick(true, true, 3_000), None);
    }

    #[test]
    fn a_transient_blip_does_not_wipe_the_ring() {
        let mut h = PrivacyGateHysteresis::new(5_000);
        // Stable "record" for 10 s.
        assert_eq!(h.tick(true, false, 0), None);
        assert_eq!(h.tick(true, false, 5_000), Some(false));
        h.commit(false);
        // One tick says "gate" (an alt-tab), then back to "record".
        assert_eq!(h.tick(true, true, 5_500), None);
        assert_eq!(h.tick(true, false, 6_000), None);
        // The gate never fired, so the engine was never told to pause.
        assert_eq!(h.tick(true, false, 12_000), None);
        assert!(!h.is_gated());
    }

    #[test]
    fn a_stable_flip_after_the_window_does_apply() {
        let mut h = PrivacyGateHysteresis::new(5_000);
        assert_eq!(h.tick(true, false, 0), None);
        assert_eq!(h.tick(true, false, 5_000), Some(false));
        h.commit(false);
        // Now stable "gate" from 6 s.
        assert_eq!(h.tick(true, true, 6_000), None);
        assert_eq!(h.tick(true, true, 11_000), Some(true));
        h.commit(true);
        assert!(h.is_gated());
    }

    #[test]
    fn disabling_privacy_ungates_immediately() {
        let mut h = PrivacyGateHysteresis::new(5_000);
        h.tick(true, true, 0);
        h.tick(true, true, 5_000);
        h.commit(true);
        assert!(h.is_gated());

        // One-shot: the very next tick un-gates, no window.
        assert_eq!(h.tick(false, true, 5_100), Some(false));
        // And only once.
        assert_eq!(h.tick(false, true, 5_200), None);
        assert!(!h.is_gated());
    }

    #[test]
    fn disabling_privacy_while_clean_reports_nothing() {
        let mut h = PrivacyGateHysteresis::new(5_000);
        // Never applied a gate (or even computed one) -> no engine call.
        assert_eq!(h.tick(false, false, 0), None);
        assert_eq!(h.tick(false, false, 10_000), None);
    }

    #[test]
    fn a_failed_apply_is_retried_on_the_next_tick() {
        let mut h = PrivacyGateHysteresis::new(1_000);
        assert_eq!(h.tick(true, true, 0), None);
        // tick() returns true, caller's invoke fails, commit() not called.
        assert_eq!(h.tick(true, true, 1_000), Some(true));
        assert_eq!(h.tick(true, true, 1_100), Some(true));
    }

    #[test]
    fn gate_mirrors_the_rust_positive_detection_rule() {
        assert!(should_gate_for_foreground(Some("explorer.exe")));
        assert!(should_gate_for_foreground(Some("clipflow.exe")));
        assert!(should_gate_for_foreground(Some("EXPLORER.EXE")));
        // Games, unknown processes, an empty string and a failed query all
        // keep recording.
        assert!(!should_gate_for_foreground(Some("cs2.exe")));
        assert!(!should_gate_for_foreground(Some("")));
        assert!(!should_gate_for_foreground(None));
    }
}

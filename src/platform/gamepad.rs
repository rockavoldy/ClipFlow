//! Gamepad / controller buttons as global-hotkey triggers.
//!
//! A binding is stored as `Pad+<n>`, where `n` is a stable, device-independent
//! button number (0 = the primary/south face button, 1 = east, 2 = west,
//! 3 = north, then the shoulder/trigger/menu/stick/d-pad buttons). The UI shows
//! it as "Button 0", "Button 1", ... rather than assuming an Xbox layout,
//! because the same input may come from a fight stick, a foot pedal or a
//! non-XInput controller.
//!
//! **Backend:** `gilrs` handles both XInput and the Windows Gaming Input raw
//! controller path, so pads Windows does not expose through XInput still work.
//! It lives on a small dedicated thread and pushes presses onto a channel; the
//! GPUI runtime drains that channel, exactly like `global-hotkey` events. This
//! keeps the blocking/polling code off the render path and out of the executor.

use std::fmt;

/// A numbered controller button. The numbering is fixed by [`from_gilrs`] so a
/// binding written today still identifies the same physical button after a
/// restart, even if the controller is XInput on one run and raw HID on the
/// next.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct GamepadButton(u8);

impl GamepadButton {
    /// Face buttons, in the usual clockwise order starting at the south button
    /// (Xbox A / PlayStation Cross).
    pub const SOUTH: Self = Self(0);
    pub const EAST: Self = Self(1);
    pub const WEST: Self = Self(2);
    pub const NORTH: Self = Self(3);

    /// Shoulders and analog triggers.
    pub const LEFT_SHOULDER: Self = Self(4);
    pub const RIGHT_SHOULDER: Self = Self(5);
    pub const LEFT_TRIGGER: Self = Self(6);
    pub const RIGHT_TRIGGER: Self = Self(7);

    /// Menu buttons and stick clicks.
    pub const SELECT: Self = Self(8);
    pub const START: Self = Self(9);
    pub const MODE: Self = Self(10);
    pub const LEFT_STICK: Self = Self(11);
    pub const RIGHT_STICK: Self = Self(12);

    /// D-pad directions.
    pub const DPAD_UP: Self = Self(13);
    pub const DPAD_DOWN: Self = Self(14);
    pub const DPAD_LEFT: Self = Self(15);
    pub const DPAD_RIGHT: Self = Self(16);

    /// Extra face buttons used by some pads (GameCube layout).
    pub const C: Self = Self(17);
    pub const Z: Self = Self(18);

    /// The highest number [`from_gilrs`] can currently produce. Parsing accepts
    /// a little more headroom so a future backend can add buttons without a
    /// settings migration.
    pub const MAX_INDEX: u8 = 31;

    pub fn index(self) -> u8 {
        self.0
    }

    /// Human-readable label for the settings badge.
    pub fn label(self) -> String {
        format!("Button {}", self.0)
    }

    /// Canonical value stored in `settings.json`.
    pub fn accelerator(self) -> String {
        format!("Pad+{}", self.0)
    }

    /// Parses `Pad+0`, `Pad+Button 0`, `Gamepad+0`, etc. The explicit prefix
    /// keeps a gamepad button from ever being mistaken for a keyboard key in
    /// the same settings field.
    pub fn parse(accelerator: &str) -> Option<Self> {
        let (prefix, rest) = accelerator.split_once('+')?;
        if !prefix.eq_ignore_ascii_case("pad") && !prefix.eq_ignore_ascii_case("gamepad") {
            return None;
        }
        let token = rest.trim();
        let digits = token
            .strip_prefix("Button")
            .or_else(|| token.strip_prefix("button"))
            .unwrap_or(token)
            .trim()
            .trim_start_matches([' ', '_']);
        let index: u8 = digits.parse().ok()?;
        if index > Self::MAX_INDEX {
            return None;
        }
        Some(Self(index))
    }

    /// Maps gilrs' semantic layout onto our stable numbering.
    #[cfg(all(windows, not(feature = "headless-sim")))]
    pub fn from_gilrs(button: gilrs::Button) -> Option<Self> {
        use gilrs::Button as G;
        let index = match button {
            G::South => Self::SOUTH,
            G::East => Self::EAST,
            G::West => Self::WEST,
            G::North => Self::NORTH,
            // gilrs names the *shoulder* `LeftTrigger` and the analog trigger
            // `LeftTrigger2` (and likewise on the right).
            G::LeftTrigger => Self::LEFT_SHOULDER,
            G::RightTrigger => Self::RIGHT_SHOULDER,
            G::LeftTrigger2 => Self::LEFT_TRIGGER,
            G::RightTrigger2 => Self::RIGHT_TRIGGER,
            G::Select => Self::SELECT,
            G::Start => Self::START,
            G::Mode => Self::MODE,
            G::LeftThumb => Self::LEFT_STICK,
            G::RightThumb => Self::RIGHT_STICK,
            G::DPadUp => Self::DPAD_UP,
            G::DPadDown => Self::DPAD_DOWN,
            G::DPadLeft => Self::DPAD_LEFT,
            G::DPadRight => Self::DPAD_RIGHT,
            G::C => Self::C,
            G::Z => Self::Z,
            _ => return None,
        };
        Some(index)
    }
}

impl fmt::Display for GamepadButton {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.label())
    }
}

/// Starts the backend thread and returns the receiving end of its button
/// stream. The thread owns `Gilrs` for the lifetime of the process; when the
/// receiver is dropped (app exit) the next send fails and the thread returns.
#[cfg(all(windows, not(feature = "headless-sim")))]
pub fn listen() -> std::sync::mpsc::Receiver<GamepadButton> {
    use std::time::Duration;

    let (tx, rx) = std::sync::mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("yascreenclipper-gamepad".to_string())
        .spawn(move || {
            let mut gilrs = match gilrs::Gilrs::new() {
                Ok(gilrs) => gilrs,
                Err(e) => {
                    log::warn!("[yascreenclipper] gamepad hotkeys unavailable: {e}");
                    return;
                }
            };

            loop {
                // `next_event` never blocks, so the thread pumps at a fixed
                // cadence. 8 ms is well below one frame and costs nothing.
                while let Some(event) = gilrs.next_event() {
                    let gilrs::EventType::ButtonPressed(button, _) = event.event else {
                        continue;
                    };
                    let Some(button) = GamepadButton::from_gilrs(button) else {
                        continue;
                    };
                    if tx.send(button).is_err() {
                        return;
                    }
                }
                std::thread::sleep(Duration::from_millis(8));
            }
        });

    if let Err(e) = spawned {
        log::warn!("[yascreenclipper] could not start the gamepad listener: {e}");
    }
    rx
}

/// Off-Windows / headless there is no gamepad backend. The receiver is
/// disconnected, which the drain loop treats as "nothing to do".
#[cfg(any(not(windows), feature = "headless-sim"))]
pub fn listen() -> std::sync::mpsc::Receiver<GamepadButton> {
    let (_tx, rx) = std::sync::mpsc::channel();
    rx
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_canonical_numeric_form() {
        assert_eq!(GamepadButton::parse("Pad+0"), Some(GamepadButton::SOUTH));
        assert_eq!(GamepadButton::parse("pad+3"), Some(GamepadButton::NORTH));
        assert_eq!(GamepadButton::parse("Gamepad+18"), Some(GamepadButton(18)));
    }

    #[test]
    fn accepts_the_human_readable_button_form() {
        assert_eq!(
            GamepadButton::parse("Pad+Button 7"),
            Some(GamepadButton::RIGHT_TRIGGER)
        );
        assert_eq!(
            GamepadButton::parse("Pad+button_12"),
            Some(GamepadButton::RIGHT_STICK)
        );
    }

    #[test]
    fn a_keyboard_chord_is_not_a_gamepad_button() {
        assert_eq!(GamepadButton::parse("Alt+C"), None);
        assert_eq!(GamepadButton::parse("C"), None);
        assert_eq!(GamepadButton::parse("Pad+Ctrl+C"), None);
    }

    #[test]
    fn rejects_out_of_range_buttons() {
        assert_eq!(GamepadButton::parse("Pad+32"), None);
        assert_eq!(GamepadButton::parse("Pad+999"), None);
        assert_eq!(GamepadButton::parse("Pad+-1"), None);
    }

    #[test]
    fn round_trips_through_the_accelerator() {
        for button in [
            GamepadButton::SOUTH,
            GamepadButton::LEFT_TRIGGER,
            GamepadButton::DPAD_RIGHT,
            GamepadButton::Z,
        ] {
            assert_eq!(GamepadButton::parse(&button.accelerator()), Some(button));
        }
    }

    #[test]
    fn labels_are_generic_button_numbers() {
        assert_eq!(GamepadButton::SOUTH.label(), "Button 0");
        assert_eq!(GamepadButton::SOUTH.accelerator(), "Pad+0");
        assert_eq!(GamepadButton(9).to_string(), "Button 9");
    }

    #[cfg(all(windows, not(feature = "headless-sim")))]
    #[test]
    fn gilrs_layout_maps_to_the_stable_numbers() {
        use gilrs::Button as G;
        assert_eq!(
            GamepadButton::from_gilrs(G::South),
            Some(GamepadButton::SOUTH)
        );
        assert_eq!(
            GamepadButton::from_gilrs(G::East),
            Some(GamepadButton::EAST)
        );
        // gilrs calls the shoulder `LeftTrigger` and the analog trigger
        // `LeftTrigger2`; our numbering keeps the physical order.
        assert_eq!(
            GamepadButton::from_gilrs(G::LeftTrigger),
            Some(GamepadButton::LEFT_SHOULDER)
        );
        assert_eq!(
            GamepadButton::from_gilrs(G::LeftTrigger2),
            Some(GamepadButton::LEFT_TRIGGER)
        );
        assert_eq!(GamepadButton::from_gilrs(G::Unknown), None);
    }
}

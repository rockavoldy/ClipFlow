//! Hotkey recording.
//!
//! Click a badge, press a chord, done. The badge is an ordinary element with a
//! click handler; the *capture* is driven from the deck's `on_key_down`, which
//! means a recorded chord can never also trigger a widget shortcut while the
//! recorder is armed.
//!
//! The formatting rules live in [`crate::platform::hotkeys::accelerator_from_chord`]
//! and produce exactly the `"Ctrl+Alt+Shift+F9"` shape the old
//! `acceleratorFromEvent` produced, so strings already in `settings.json` stay
//! valid.
//!
//! Ordering contract, inherited from the Tauri build and **not** to be
//! "simplified": the new chord is registered with the OS *before* the settings
//! file is written. A conflict therefore fails the whole patch and leaves the
//! old binding live, instead of persisting a hotkey that does not work.

use gpui_kit::{Context, KeyDownEvent, Window};

use crate::state::SettingsPatch;
use crate::ui::deck::{Deck, HotkeyAction};

impl Deck {
    /// Arms the recorder for `action` and moves keyboard focus to the deck root
    /// so the next chord arrives at [`Deck::record_hotkey_key`].
    pub fn begin_hotkey_capture(
        &mut self,
        action: HotkeyAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.recording_hotkey = Some(action);
        self.focus.focus(window, cx);
        cx.notify();
    }

    /// Cancels an in-progress recording.
    pub fn cancel_hotkey_capture(&mut self, cx: &mut Context<Self>) {
        if self.recording_hotkey.take().is_some() {
            cx.notify();
        }
    }

    /// Feeds one key press into the recorder.
    ///
    /// Always consumes the event while recording, so the chord cannot leak to
    /// another handler. Escape cancels; modifier-only presses and unsupported
    /// keys keep waiting; a modifier-less chord is rejected with the same
    /// message the old UI used (Windows reserves bare keys for typing).
    pub fn record_hotkey_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let Some(action) = self.recording_hotkey else {
            return;
        };
        let keystroke = &event.keystroke;

        if keystroke.key == "escape" {
            self.recording_hotkey = None;
            cx.notify();
            return;
        }

        let Some(chord) = crate::platform::hotkeys::accelerator_from_chord(
            &keystroke.key,
            keystroke.modifiers.control,
            keystroke.modifiers.alt,
            keystroke.modifiers.shift,
            keystroke.modifiers.platform,
        ) else {
            // Modifier alone, or a key that cannot be part of a global shortcut.
            return;
        };

        // Validate *before* leaving record mode so an invalid combo keeps the
        // badge armed instead of silently doing nothing.
        if let Err(e) = crate::platform::hotkeys::validate(&chord) {
            self.recording_hotkey = None;
            self.toast_error("Hotkey not accepted", &e);
            cx.notify();
            return;
        }

        self.recording_hotkey = None;
        let patch = match action {
            HotkeyAction::Save => SettingsPatch {
                hotkey_save: Some(chord),
                ..Default::default()
            },
            HotkeyAction::Toggle => SettingsPatch {
                hotkey_toggle: Some(chord),
                ..Default::default()
            },
        };
        // `apply_patch` registers first, then persists.
        self.apply_patch(patch, cx);
    }

    /// Badge label: the chord, or a prompt while recording.
    pub fn hotkey_badge_label(&self, action: HotkeyAction, accelerator: &str) -> String {
        if self.recording_hotkey == Some(action) {
            "PRESS KEYS… (Esc cancels)".to_string()
        } else {
            accelerator.to_string()
        }
    }
}

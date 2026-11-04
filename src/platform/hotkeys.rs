//! Global shortcuts, without a plugin layer.
//!
//! `Alt + C`       ➜ flush the rolling buffer to disk (the money shortcut)
//! `Alt + Shift+C` ➜ arm / disarm the buffer
//!
//! The OS callback must return in microseconds: it does nothing but push the
//! action onto a channel, otherwise Windows considers the hook unresponsive and
//! silently unregisters it mid-game.
//!
//! **Threading:** `GlobalHotKeyManager` creates a hidden window and needs a
//! message loop on the thread that created it. It is constructed inside
//! `Application::run`, whose Windows loop is `GetMessageW(NULL)` +
//! `DispatchMessageW`, so `WM_HOTKEY` is dispatched for it there.

use global_hotkey::hotkey::HotKey;

/// Parses a stored accelerator. Kept separate from [`validate`] because
/// `rebind` needs to inspect the *previous* value without re-validating it: a
/// settings file written by an older build may contain something we would
/// reject today, and that must not wedge rebinding.
pub fn parse(accelerator: &str) -> Result<HotKey, String> {
    accelerator
        .parse::<HotKey>()
        .map_err(|e| format!("invalid shortcut '{accelerator}': {e}"))
}

/// Parses + rejects combos that Windows would treat as dangerous (a bare key
/// with no modifier can hijack typing in every app).
pub fn validate(accelerator: &str) -> Result<HotKey, String> {
    let hotkey = parse(accelerator)?;
    let has_modifier = !hotkey.mods.is_empty();
    if has_modifier {
        Ok(hotkey)
    } else {
        Err(format!(
            "'{accelerator}' needs a modifier (Alt, Ctrl, Shift or Win) — Windows reserves bare keys for typing"
        ))
    }
}

/// Formats a chord the way `acceleratorFromEvent` in the old settings panel
/// did: `Ctrl+Alt+Shift+Win+Key`, resulting in strings the existing
/// `settings.json` already contains.
///
/// Returns `None` for modifier-only presses and for keys that cannot be part
/// of a global shortcut (arrows, media keys, Escape = cancel), so the recorder
/// badge simply keeps waiting.
pub fn accelerator_from_chord(
    key: &str,
    ctrl: bool,
    alt: bool,
    shift: bool,
    win: bool,
) -> Option<String> {
    if key.is_empty() {
        return None;
    }
    // A modifier on its own is not a combo yet.
    if matches!(
        key,
        "control"
            | "ctrl"
            | "alt"
            | "option"
            | "shift"
            | "platform"
            | "super"
            | "meta"
            | "cmd"
            | "cap"
            | "capslock"
            | "tab"
            | "escape"
            | "fn"
    ) {
        return None;
    }

    let token = single_char_token(key).or_else(|| function_key_token(key))?;

    let mut parts: Vec<&str> = Vec::new();
    if ctrl {
        parts.push("Ctrl");
    }
    if alt {
        parts.push("Alt");
    }
    if shift {
        parts.push("Shift");
    }
    if win {
        parts.push("Win");
    }
    // Windows reserves bare keys for typing, so a modifier-less chord is not a
    // valid global shortcut even though it is a valid combination.
    if parts.is_empty() {
        return None;
    }
    parts.push(&token);
    Some(parts.join("+"))
}

fn single_char_token(key: &str) -> Option<String> {
    if key == " " || key == "space" {
        return Some("Space".to_string());
    }
    let mut chars = key.chars();
    let (first, rest) = (chars.next()?, chars.next());
    if rest.is_some() {
        return None;
    }
    if first.is_ascii_alphanumeric() {
        return Some(first.to_ascii_uppercase().to_string());
    }
    None
}

fn function_key_token(key: &str) -> Option<String> {
    let lower = key.to_ascii_lowercase();
    let digits = lower.strip_prefix('f')?;
    let n: u32 = digits.parse().ok()?;
    if (1..=24).contains(&n) {
        Some(format!("F{n}"))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_shipped_defaults() {
        assert!(parse("Alt+C").is_ok());
        assert!(parse("Alt+Shift+C").is_ok());
        // The parser is case-insensitive, so a hand-edited settings file is fine.
        assert!(parse("alt+c").is_ok());
    }

    #[test]
    fn rejects_a_bare_key_with_a_specific_message() {
        let err = validate("C").unwrap_err();
        assert!(
            err.contains("needs a modifier (Alt, Ctrl, Shift or Win)"),
            "unexpected message: {err}"
        );
    }

    #[test]
    fn keeps_a_modifier_chord() {
        assert!(validate("Ctrl+Alt+Shift+F9").is_ok());
    }

    #[test]
    fn formats_the_old_ui_order() {
        assert_eq!(
            accelerator_from_chord("c", true, true, false, false).unwrap(),
            "Ctrl+Alt+C"
        );
        assert_eq!(
            accelerator_from_chord("c", false, true, true, false).unwrap(),
            "Alt+Shift+C"
        );
        // The order is fixed, not press order: Ctrl, Alt, Shift, Win.
        assert_eq!(
            accelerator_from_chord("f9", true, true, true, true).unwrap(),
            "Ctrl+Alt+Shift+Win+F9"
        );
        assert_eq!(
            accelerator_from_chord("space", true, false, false, false).unwrap(),
            "Ctrl+Space"
        );
    }

    #[test]
    fn rejects_modifier_only_and_bare_chords() {
        for key in ["control", "alt", "shift", "platform", "capslock", "tab"] {
            assert_eq!(
                accelerator_from_chord(key, true, true, true, true),
                None,
                "{key} is not a valid key token"
            );
        }
        // A real key with no modifier is reserved for typing.
        assert_eq!(accelerator_from_chord("c", false, false, false, false), None);
        // Arrows / media keys are not global-shortcut tokens in this UI.
        assert_eq!(accelerator_from_chord("left", true, false, false, false), None);
        assert_eq!(
            accelerator_from_chord("mediatracknext", true, false, false, false),
            None
        );
    }

    #[test]
    fn function_keys_are_bounded() {
        assert!(accelerator_from_chord("f24", true, false, false, false).is_some());
        assert!(accelerator_from_chord("f25", true, false, false, false).is_none());
        assert!(accelerator_from_chord("f0", true, false, false, false).is_none());
    }

    #[test]
    fn round_trips_through_the_parser() {
        let formatted = accelerator_from_chord("c", false, true, true, false).unwrap();
        assert_eq!(formatted, "Alt+Shift+C");
        assert!(validate(&formatted).is_ok());
    }
}

// ---------------------------------------------------------------------------
// Runtime registration
// ---------------------------------------------------------------------------

/// The platform half of a registration.
///
/// Registering a global hotkey is a process-wide, exclusive operation: two
/// live `Alt+C` registrations cannot coexist. That is why the trait exists -
/// it lets the binding *logic* (which combo wins, in which order the old one is
/// released) be exercised without an OS. Tests register against a recorder, not
/// against the real manager.
pub trait Registrar {
    fn register(&self, key: HotKey) -> Result<(), String>;
    fn unregister(&self, key: HotKey) -> Result<(), String>;
}

#[cfg(all(windows, not(feature = "headless-sim")))]
impl Registrar for global_hotkey::GlobalHotKeyManager {
    fn register(&self, key: HotKey) -> Result<(), String> {
        global_hotkey::GlobalHotKeyManager::register(self, key).map_err(|e| e.to_string())
    }
    fn unregister(&self, key: HotKey) -> Result<(), String> {
        global_hotkey::GlobalHotKeyManager::unregister(self, key).map_err(|e| e.to_string())
    }
}

/// Resolves a stored accelerator, falling back to a default when the stored
/// value is unusable.
///
/// A corrupt `settings.json` must not stop the app from opening: the combo is
/// logged and replaced, and the deck buttons still work.
pub fn resolve_or_default(stored: &str, fallback: &str) -> Result<HotKey, String> {
    match validate(stored) {
        Ok(key) => Ok(key),
        Err(e) => {
            log::warn!("[clipflow] hotkey '{stored}' unusable: {e}");
            validate(fallback).map_err(|e| e.to_string())
        }
    }
}

/// Swaps one binding, **registering the new combo before unregistering the old
/// one** (the ordering inherited from the Tauri build).
///
/// That ordering is what makes a conflict safe: if the new combo is already
/// owned by another app, `register` fails and the old binding is still live, so
/// the user is never left with no hotkey at all.
pub fn swap_binding(registrar: &dyn Registrar, old: HotKey, new: HotKey) -> Result<(), String> {
    if old == new {
        return Ok(()); // no-op; re-registering would fail
    }
    registrar.register(new)?;
    // Only now is it safe to drop the old one.
    if let Err(e) = registrar.unregister(old) {
        log::warn!("[clipflow] could not unregister the old hotkey: {e}");
    }
    Ok(())
}

#[cfg(all(windows, not(feature = "headless-sim")))]
/// The live bindings.
///
/// Owns the OS registrations for exactly as long as the app runs and keeps the
/// id → intent map the drain task uses.
///
/// The manager is optional so the binding logic can be exercised without the
/// OS. See [`Registered::unbound`].
pub struct Registered {
    manager: Option<global_hotkey::GlobalHotKeyManager>,
    save: HotKey,
    toggle: HotKey,
}

#[cfg(all(windows, not(feature = "headless-sim")))]
impl Registered {
    /// Registers both stored accelerators with the OS.
    ///
    /// Fails when the OS refuses a combo (another app already owns it). The
    /// caller decides whether that is fatal.
    pub fn install(save: &str, toggle: &str) -> Result<Self, String> {
        let manager = global_hotkey::GlobalHotKeyManager::new()
            .map_err(|e| format!("could not create the hotkey manager: {e}"))?;

        let save_key = resolve_or_default(save, "Alt+C")?;
        let toggle_key = resolve_or_default(toggle, "Alt+Shift+C")?;

        Registrar::register(&manager, save_key)
            .map_err(|e| format!("could not register '{save}': {e}"))?;
        if let Err(e) = Registrar::register(&manager, toggle_key) {
            // Roll back the first so we never leave a half-bound state.
            let _ = Registrar::unregister(&manager, save_key);
            return Err(format!("could not register '{toggle}': {e}"));
        }

        Ok(Self {
            manager: Some(manager),
            save: save_key,
            toggle: toggle_key,
        })
    }

    /// A binding set that is **not** registered with the OS.
    ///
    /// Used when the OS refused the combos: the app still runs and the deck
    /// buttons still work, the keyboard shortcuts simply do not fire. Without
    /// this the app would have to crash, because the only combos it knows how
    /// to register are the ones that were just refused.
    pub fn unbound(save: &str, toggle: &str) -> Result<Self, String> {
        Ok(Self {
            manager: None,
            save: resolve_or_default(save, "Alt+C")?,
            toggle: resolve_or_default(toggle, "Alt+Shift+C")?,
        })
    }

    /// Swaps one binding in place.
    pub fn rebind(
        &mut self,
        action: crate::ui::deck::HotkeyAction,
        new_accelerator: &str,
    ) -> Result<(), String> {
        let new_key = validate(new_accelerator)?;
        let (old_key, slot) = match action {
            crate::ui::deck::HotkeyAction::Save => (&mut self.save, "save"),
            crate::ui::deck::HotkeyAction::Toggle => (&mut self.toggle, "toggle"),
        };

        if let Some(manager) = self.manager.as_ref() {
            swap_binding(manager, *old_key, new_key)
                .map_err(|e| format!("'{new_accelerator}' is already in use ({e})"))?;
            let _ = slot;
        }
        *old_key = new_key;
        Ok(())
    }

    /// Which action a hotkey id maps to, if any.
    pub fn intent_for(&self, id: u32) -> Option<crate::ui::deck::Intent> {
        use crate::ui::deck::{HotkeyAction, Intent};
        if id == self.save.id() {
            Some(Intent::Hotkey(HotkeyAction::Save))
        } else if id == self.toggle.id() {
            Some(Intent::Hotkey(HotkeyAction::Toggle))
        } else {
            None
        }
    }
}

/// Off-Windows / headless: no OS registration, but the same type exists so the
/// app's plumbing compiles unchanged. `rebind` still validates, which keeps the
/// "invalid shortcut" messages testable everywhere.
#[cfg(any(not(windows), feature = "headless-sim"))]
pub struct Registered {
    save: HotKey,
    toggle: HotKey,
}

#[cfg(any(not(windows), feature = "headless-sim"))]
impl Registered {
    pub fn install(save: &str, toggle: &str) -> Result<Self, String> {
        Ok(Self {
            save: resolve_or_default(save, "Alt+C")?,
            toggle: resolve_or_default(toggle, "Alt+Shift+C")?,
        })
    }

    /// Off-Windows there is no OS to refuse a combo, so this is the same as
    /// [`Registered::install`]. It exists so the call sites read the same on
    /// every platform.
    pub fn unbound(save: &str, toggle: &str) -> Result<Self, String> {
        Self::install(save, toggle)
    }

    pub fn rebind(
        &mut self,
        action: crate::ui::deck::HotkeyAction,
        new_accelerator: &str,
    ) -> Result<(), String> {
        let new_key = validate(new_accelerator)?;
        match action {
            crate::ui::deck::HotkeyAction::Save => self.save = new_key,
            crate::ui::deck::HotkeyAction::Toggle => self.toggle = new_key,
        }
        Ok(())
    }

    pub fn intent_for(&self, id: u32) -> Option<crate::ui::deck::Intent> {
        use crate::ui::deck::{HotkeyAction, Intent};
        if id == self.save.id() {
            Some(Intent::Hotkey(HotkeyAction::Save))
        } else if id == self.toggle.id() {
            Some(Intent::Hotkey(HotkeyAction::Toggle))
        } else {
            None
        }
    }
}

#[cfg(test)]
mod registration_tests {
    use super::*;
    use crate::ui::deck::{HotkeyAction, Intent};
    use std::cell::RefCell;

    /// Records what the registration layer was asked to do, so the ordering
    /// contract can be asserted without touching the OS.
    #[derive(Default)]
    struct Recorder {
        calls: RefCell<Vec<&'static str>>,
        fail_register: bool,
    }

    impl Recorder {
        fn calls(&self) -> Vec<&'static str> {
            self.calls.borrow().clone()
        }
    }

    impl Registrar for Recorder {
        fn register(&self, _key: HotKey) -> Result<(), String> {
            self.calls.borrow_mut().push("register");
            if self.fail_register {
                Err("combo is taken".to_string())
            } else {
                Ok(())
            }
        }
        fn unregister(&self, _key: HotKey) -> Result<(), String> {
            self.calls.borrow_mut().push("unregister");
            Ok(())
        }
    }

    #[test]
    fn a_corrupt_stored_combo_falls_back_to_the_default() {
        // A corrupt settings.json must not stop the app from opening.
        assert_eq!(
            resolve_or_default("not+a+hotkey", "Alt+C").unwrap().id(),
            parse("Alt+C").unwrap().id()
        );
        assert_eq!(
            resolve_or_default("", "Alt+Shift+C").unwrap().id(),
            parse("Alt+Shift+C").unwrap().id()
        );
    }

    #[test]
    fn the_two_actions_never_alias_each_other() {
        let reg = Registered::unbound("Alt+C", "Alt+Shift+C").unwrap();
        let save = reg.intent_for(parse("Alt+C").unwrap().id());
        let toggle = reg.intent_for(parse("Alt+Shift+C").unwrap().id());
        assert_ne!(save, toggle);
        assert_eq!(save, Some(Intent::Hotkey(HotkeyAction::Save)));
        assert_eq!(toggle, Some(Intent::Hotkey(HotkeyAction::Toggle)));
    }

    #[test]
    fn rebind_rejects_a_modifierless_combo_and_keeps_the_old_binding() {
        let mut reg = Registered::unbound("Alt+C", "Alt+Shift+C").unwrap();
        let err = reg.rebind(HotkeyAction::Save, "C").unwrap_err();
        assert!(err.contains("needs a modifier"), "{err}");
        assert_eq!(
            reg.intent_for(parse("Alt+C").unwrap().id()),
            Some(Intent::Hotkey(HotkeyAction::Save))
        );
    }

    #[test]
    fn rebind_to_the_same_combo_is_a_noop() {
        let mut reg = Registered::unbound("Alt+C", "Alt+Shift+C").unwrap();
        assert!(reg.rebind(HotkeyAction::Save, "Alt+C").is_ok());
        assert_eq!(
            reg.intent_for(parse("Alt+C").unwrap().id()),
            Some(Intent::Hotkey(HotkeyAction::Save))
        );
    }

    #[test]
    fn rebind_moves_the_binding() {
        let mut reg = Registered::unbound("Alt+C", "Alt+Shift+C").unwrap();
        reg.rebind(HotkeyAction::Save, "Ctrl+Alt+F9").unwrap();
        assert_eq!(
            reg.intent_for(parse("Ctrl+Alt+F9").unwrap().id()),
            Some(Intent::Hotkey(HotkeyAction::Save))
        );
        // The old combo no longer maps to anything.
        assert_eq!(reg.intent_for(parse("Alt+C").unwrap().id()), None);
    }

    /// The ordering contract: the new combo is registered *before* the old one
    /// is released, so a conflict leaves the user with a working hotkey.
    #[test]
    fn the_new_combo_is_registered_before_the_old_one_is_released() {
        let recorder = Recorder::default();
        let old = parse("Alt+C").unwrap();
        let new = parse("Ctrl+Alt+F9").unwrap();

        swap_binding(&recorder, old, new).unwrap();
        assert_eq!(recorder.calls(), vec!["register", "unregister"]);
    }

    /// ...and when the new combo is refused, the old one is never released.
    #[test]
    fn a_refused_combo_leaves_the_old_binding_live() {
        let recorder = Recorder {
            fail_register: true,
            ..Recorder::default()
        };
        let old = parse("Alt+C").unwrap();
        let new = parse("Ctrl+Alt+F9").unwrap();

        let err = swap_binding(&recorder, old, new).unwrap_err();
        assert!(err.contains("combo is taken"), "{err}");
        assert_eq!(recorder.calls(), vec!["register"]);
    }
}

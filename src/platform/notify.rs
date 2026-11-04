//! Native notifications.
//!
//! Phase 1 has no HUD and no in-app toast stack, so a Windows toast is the
//! *only* mid-game confirmation that `Alt+C` worked. The rule that matters is
//! inherited from the HUD: never steal focus from a game. A toast does not
//! activate the app, so it is safe to fire unconditionally; raising the window
//! is gated separately on [`crate::state::safe_to_raise`].
//!
//! Implementation note: GPUI's Windows platform already ships
//! `IInspectable`-based WinRT toasts **and** registers the
//! `HKCU\Software\Classes\AppUserModelId\<id>` entry an unpackaged exe needs
//! (`gpui-pre-windows/src/system_notifications.rs`). Reusing it means no
//! `tauri-winrt-notification` dependency and no second AUMID registration path
//! that could disagree with the one the shell already knows about.

use gpui_kit::App;

/// Stable AUMID. Must match `Packager.toml`'s identifier so the installed
/// build groups its toasts under the same app entry.
pub const APP_ID: &str = "dev.clipflow.app";

/// Registers the AUMID and app name once, at startup. GPUI performs the
/// registry write lazily too, but doing it eagerly means the very first toast
/// (which may fire 200 ms after launch from a hotkey) is attributed correctly.
pub fn init(cx: &App) {
    cx.set_app_identity(APP_ID, "ClipFlow");
}

/// Shows a toast. `tag` collapses repeated notifications of the same kind
/// (e.g. two quick saves) instead of stacking them.
pub fn show(cx: &App, tag: &str, title: &str, body: &str) {
    use gpui_kit::{SystemNotification, SystemNotificationAction};
    cx.show_system_notification(SystemNotification {
        tag: tag.to_string().into(),
        title: title.to_string().into(),
        body: body.to_string().into(),
        actions: Vec::<SystemNotificationAction>::new(),
    });
}

/// Convenience for the save path, so the copy stays identical everywhere a
/// clip lands: hotkey, tray, "save last 30 s" and auto-save-on-exit.
pub fn clip_saved(cx: &App, file_name: &str, duration_seconds: f32, trigger: &str) {
    let body = format!("{file_name} · {duration_seconds:.0}s · saved from {trigger}");
    show(cx, "clip-saved", "Clip saved", &body);
}

/// The one user-facing failure worth a toast: a save that was refused because
/// privacy mode had gated the ring. Reported so the hotkey never appears to
/// have done nothing.
pub fn save_blocked(cx: &App, message: &str) {
    show(cx, "save-blocked", "ClipFlow", message);
}

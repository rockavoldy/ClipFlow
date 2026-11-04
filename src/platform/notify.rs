//! Native notifications.
//!
//! Phase 1 has no HUD and no in-app toast stack, so a Windows toast is the
//! *only* mid-game confirmation that `Alt+C` worked. The rule that matters is
//! inherited from the HUD: never steal focus from a game. A toast does not
//! activate the app, so it is safe to fire unconditionally; raising the window
//! is gated separately on [`crate::state::safe_to_raise`].
//!
//! **Why this does not use `App::show_system_notification`:** WinRT toast
//! calls (`CreateToastNotifierWithId`, `Show`) are out-of-process COM calls and
//! pump the Win32 message loop while waiting. When they run under an entity
//! update — the save path raises the toast from inside `deck.update(...)` —
//! the pumped `WM_PAINT` re-enters GPUI's `request_frame`, which tries to
//! borrow the `AppCell` that the update already holds and logs
//! `RefCell already borrowed: attempted mutable App borrow`. gpui's own
//! Windows code defers `Command::spawn` for exactly this reason.
//!
//! The toast therefore runs on a short-lived worker thread, after COM has been
//! initialised there. That thread never touches GPUI, so a message pump is
//! harmless. The AUMID is registered once at startup so unpackaged builds are
//! attributed correctly.

use gpui_kit::App;

/// Stable AUMID. Must match `Packager.toml`'s identifier so the installed
/// build groups its toasts under the same app entry.
pub const APP_ID: &str = "dev.clipflow.app";

/// Registers the AUMID and app name once, at startup. This runs before the
/// window exists and only touches the registry, so it cannot pump messages.
pub fn init(cx: &App) {
    // Kept so any gpui-owned notification path stays correctly attributed.
    cx.set_app_identity(APP_ID, "ClipFlow");
    #[cfg(all(windows, not(feature = "headless-sim")))]
    register_aumid();
}

/// Shows a toast. `tag` collapses repeated notifications of the same kind
/// (e.g. two quick saves) instead of stacking them.
///
/// Returns immediately: the WinRT work happens on a detached thread.
pub fn show(tag: &str, title: &str, body: &str) {
    let tag = tag.to_string();
    let title = title.to_string();
    let body = body.to_string();

    #[cfg(all(windows, not(feature = "headless-sim")))]
    {
        let spawned = std::thread::Builder::new()
            .name("clipflow-toast".into())
            .spawn(move || {
                if let Err(e) = show_blocking(&tag, &title, &body) {
                    log::warn!("[clipflow] toast failed: {e}");
                }
            });
        if let Err(e) = spawned {
            log::warn!("[clipflow] could not spawn toast thread: {e}");
        }
    }
    #[cfg(any(not(windows), feature = "headless-sim"))]
    {
        log::info!("[clipflow] (stub) toast {title}: {body}");
        let _ = tag;
    }
}

/// The toast body shared by every caller. Kept separate so the XML can be
/// exercised on non-Windows CI.
#[cfg_attr(not(all(windows, not(feature = "headless-sim"))), allow(dead_code))]
fn toast_xml(title: &str, body: &str) -> String {
    // The text is user/OS-derived (clip file names, error strings), so escape
    // anything that would break the XML document.
    fn escape(s: &str) -> String {
        s.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
            .replace('\'', "&apos;")
    }
    format!(
        "<toast><visual><binding template=\"ToastGeneric\">\
         <text>{}</text><text>{}</text></binding></visual>\
         <audio silent=\"true\"/></toast>",
        escape(title),
        escape(body)
    )
}

#[cfg(all(windows, not(feature = "headless-sim")))]
fn show_blocking(tag: &str, title: &str, body: &str) -> windows::core::Result<()> {
    use windows::Data::Xml::Dom::XmlDocument;
    use windows::UI::Notifications::{ToastNotification, ToastNotificationManager};
    use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};

    unsafe {
        let initialized = CoInitializeEx(None, COINIT_MULTITHREADED).is_ok();
        let result = (|| {
            let document = XmlDocument::new()?;
            document.LoadXml(&toast_xml(title, body).into())?;

            let toast = ToastNotification::CreateToastNotification(&document)?;
            toast.SetTag(&tag.into())?;

            let notifier = ToastNotificationManager::CreateToastNotifierWithId(&APP_ID.into())?;
            notifier.Show(&toast)?;
            Ok(())
        })();
        if initialized {
            CoUninitialize();
        }
        result
    }
}

/// Registers `HKCU\Software\Classes\AppUserModelId\<APP_ID>` so an unpackaged
/// `clipflow.exe` can raise toasts. gpui used to do this lazily on its first
/// toast; since this module owns the notification path now, it owns the
/// registration too.
#[cfg(all(windows, not(feature = "headless-sim")))]
fn register_aumid() {
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::System::Registry::{
        HKEY_CURRENT_USER, KEY_WRITE, REG_SZ, RegCloseKey, RegCreateKeyExW, RegSetValueExW,
    };

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    let path = wide(&format!("Software\\Classes\\AppUserModelId\\{APP_ID}"));
    let name = wide("DisplayName");
    let value = wide("ClipFlow");

    unsafe {
        let mut key = std::ptr::null_mut();
        if RegCreateKeyExW(
            HKEY_CURRENT_USER,
            path.as_ptr(),
            0,
            std::ptr::null(),
            0,
            KEY_WRITE,
            std::ptr::null(),
            &mut key,
            std::ptr::null_mut(),
        ) != ERROR_SUCCESS
        {
            log::warn!("[clipflow] could not register toast AUMID");
            return;
        }
        let bytes = std::slice::from_raw_parts(
            value.as_ptr() as *const u8,
            value.len() * std::mem::size_of::<u16>(),
        );
        if RegSetValueExW(key, name.as_ptr(), 0, REG_SZ, bytes.as_ptr(), bytes.len() as u32)
            != ERROR_SUCCESS
        {
            log::warn!("[clipflow] could not write toast display name");
        }
        RegCloseKey(key);
    }
}

/// Convenience for the save path, so the copy stays identical everywhere a
/// clip lands: hotkey, tray, "save last 30 s" and auto-save-on-exit.
pub fn clip_saved(file_name: &str, duration_seconds: f32, trigger: &str) {
    let body = format!("{file_name} · {duration_seconds:.0}s · saved from {trigger}");
    show("clip-saved", "Clip saved", &body);
}

/// The one user-facing failure worth a toast: a save that was refused because
/// privacy mode had gated the ring. Reported so the hotkey never appears to
/// have done nothing.
pub fn save_blocked(message: &str) {
    show("save-blocked", "ClipFlow", message);
}

#[cfg(test)]
mod tests {
    use super::toast_xml;

    #[test]
    fn toast_xml_escapes_user_text() {
        let xml = toast_xml("A & B", "<clip> \"one\"");
        assert!(xml.contains("A &amp; B"));
        assert!(xml.contains("&lt;clip&gt; &quot;one&quot;"));
        assert!(!xml.contains("<clip>"));
    }
}

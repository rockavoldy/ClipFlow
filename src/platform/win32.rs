//! Thin, Tauri-free Win32 layer for the window controls GPUI does not expose.
//!
//! Why this module exists at all: `gpui_kw::Window` has no
//! `set_always_on_top`, no `hide`/`show` and no `set_position`, and the
//! components' `TitleBar` owns minimise/maximise/drag itself. Everything the
//! deck needs beyond that is a handful of `user32`/`shell32` calls.
//!
//! **Handle discipline:** the capture engine is pinned to `windows 0.58`,
//! while this module and `tray-icon`/`global-hotkey` use `windows-sys 0.61`.
//! A typed `HWND` must never cross that boundary, so every function here takes
//! and returns a bare `isize`. Callers obtain the value once from
//! `Window::window_handle()` and store it; they never hold a typed handle.
//!
//! Everything here is best-effort: a failed `SetWindowDisplayAffinity` on
//! Windows 10 pre-2004 must not abort an otherwise healthy save.

use gpui_kit::App;

/// Runs an HWND operation once the current gpui update has finished.
///
/// `ShowWindow`, `SetForegroundWindow`, `SetWindowPos` and `ShellExecuteW`
/// dispatch window messages synchronously — `WM_NCHITTEST` among them. Called
/// from inside an entity update they re-enter GPUI's window procedure while the
/// `App` is mutably borrowed, which logs
/// `RefCell already borrowed: attempted mutable App borrow` and breaks titlebar
/// hit-testing for that message. A spawned task runs between message
/// dispatches, where no borrow is held, so handle work goes through here.
///
/// `App::defer` is *not* a substitute: deferred callbacks run inside
/// `App::flush_effects`, still under the borrow.
pub fn defer(cx: &App, f: impl FnOnce() + 'static) {
    cx.spawn(async move |_cx| f()).detach();
}

/// Applies (or clears) `HWND_TOPMOST`. `SetWindowPos` is used instead of
/// `SetWindowLong` so the change also restacks the window immediately.
#[cfg(all(windows, not(feature = "headless-sim")))]
pub fn set_topmost(hwnd: isize, topmost: bool) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        HWND_NOTOPMOST, HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SetWindowPos,
    };
    if hwnd == 0 {
        return;
    }
    let insert_after = if topmost { HWND_TOPMOST } else { HWND_NOTOPMOST };
    unsafe {
        SetWindowPos(
            hwnd as _,
            insert_after,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        );
    }
}

#[cfg(any(not(windows), feature = "headless-sim"))]
pub fn set_topmost(_hwnd: isize, _topmost: bool) {}

/// `ShowWindow(SW_HIDE)` / `ShowWindow(SW_SHOW)`. Hiding rather than destroying
/// is what keeps the capture engine and the tray icon alive when the user
/// "closes" the deck with `minimize_to_tray` on.
#[cfg(all(windows, not(feature = "headless-sim")))]
pub fn set_visible(hwnd: isize, visible: bool) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{SW_HIDE, SW_SHOW, ShowWindow};
    if hwnd == 0 {
        return;
    }
    unsafe {
        ShowWindow(hwnd as _, if visible { SW_SHOW } else { SW_HIDE });
    }
}

#[cfg(any(not(windows), feature = "headless-sim"))]
pub fn set_visible(_hwnd: isize, _visible: bool) {}

/// Brings the deck to the front after a tray click or a "safe to raise" save.
/// Callers must gate this on [`crate::state::safe_to_raise`]: activating the
/// window while an exclusive-fullscreen game has focus minimises the game.
#[cfg(all(windows, not(feature = "headless-sim")))]
pub fn activate(hwnd: isize) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{SW_RESTORE, SetForegroundWindow, ShowWindow};
    if hwnd == 0 {
        return;
    }
    unsafe {
        // Restore first: a minimised window cannot be focused.
        ShowWindow(hwnd as _, SW_RESTORE);
        SetForegroundWindow(hwnd as _);
    }
}

#[cfg(any(not(windows), feature = "headless-sim"))]
pub fn activate(_hwnd: isize) {}

/// Marks a window as excluded from *every* capture path: DXGI Desktop
/// Duplication (what the engine records with), Windows Graphics Capture,
/// `PrintWindow`, `BitBlt`. Phase 1 has no HUD, so nothing calls this yet — it
/// stays because the deferred HUD is its only caller and the engine would
/// otherwise record its own overlay.
#[cfg(all(windows, not(feature = "headless-sim")))]
pub fn set_capture_excluded(hwnd: isize, excluded: bool) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SetWindowDisplayAffinity, WDA_EXCLUDEFROMCAPTURE, WDA_NONE,
    };
    if hwnd == 0 {
        return;
    }
    unsafe {
        SetWindowDisplayAffinity(
            hwnd as _,
            if excluded {
                WDA_EXCLUDEFROMCAPTURE
            } else {
                WDA_NONE
            },
        );
    }
}

#[cfg(any(not(windows), feature = "headless-sim"))]
pub fn set_capture_excluded(_hwnd: isize, _excluded: bool) {}

/// Opens a path in its default application, or reveals it in Explorer when
/// `reveal` is set. Shelling out through `ShellExecuteW` keeps this
/// dependency-free; `rfd`/`opener` would add a whole tree for one verb.
#[cfg(all(windows, not(feature = "headless-sim")))]
pub fn shell_open(path: &std::path::Path, reveal: bool) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    // `explorer /select,"path"` is the documented way to reveal a file; there
    // is no ShellExecute verb that does it.
    let mut file: Vec<u16>;
    let params: Vec<u16>;
    let verb = wide("open");

    if reveal {
        let target = if path.is_dir() {
            path.parent().unwrap_or(path).to_path_buf()
        } else {
            path.to_path_buf()
        };
        file = "explorer.exe".encode_utf16().chain(std::iter::once(0)).collect();
        params = format!("/select,\"{}\"", target.display())
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
    } else {
        file = path.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
        params = Vec::new();
    }

    let params_ptr = if params.is_empty() {
        std::ptr::null()
    } else {
        params.as_ptr()
    };

    let code = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            file.as_mut_ptr(),
            params_ptr,
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    };
    // ShellExecuteW returns a value > 32 on success.
    if code as isize > 32 {
        Ok(())
    } else {
        Err(format!("ShellExecuteW failed (code {})", code as isize))
    }
}

#[cfg(any(not(windows), feature = "headless-sim"))]
pub fn shell_open(path: &std::path::Path, reveal: bool) -> Result<(), String> {
    log::info!(
        "[yascreenclipper] (stub) {} {}",
        if reveal { "reveal" } else { "open" },
        path.display()
    );
    Ok(())
}

/// Puts one or more files on the clipboard as a `CF_HDROP` file drop — the
/// shape Discord / Explorer / Slack / Teams paste handlers expect for a video.
/// Multi-file is the same payload with more NUL-terminated entries and one
/// extra NUL at the end.
#[cfg(all(windows, not(feature = "headless-sim")))]
pub fn copy_files_to_clipboard(paths: &[std::path::PathBuf]) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
    };
    use windows_sys::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
    use windows_sys::Win32::System::Ole::CF_HDROP;
    use windows_sys::Win32::UI::Shell::DROPFILES;

    if paths.is_empty() {
        return Err("nothing to copy".into());
    }
    for p in paths {
        if !p.exists() {
            return Err(format!("clip not found: {}", p.display()));
        }
    }

    let mut wide: Vec<u16> = Vec::new();
    for p in paths {
        wide.extend(p.as_os_str().encode_wide());
        wide.push(0);
    }
    wide.push(0); // second NUL terminates the whole list

    let header = std::mem::size_of::<DROPFILES>();
    let total = header + wide.len() * 2;

    unsafe {
        if OpenClipboard(std::ptr::null_mut()) == 0 {
            return Err("OpenClipboard failed (another app is holding it)".into());
        }
        let result = (|| -> Result<(), String> {
            if EmptyClipboard() == 0 {
                return Err("EmptyClipboard failed".into());
            }
            let hmem = GlobalAlloc(GMEM_MOVEABLE, total);
            if hmem.is_null() {
                return Err("GlobalAlloc failed".into());
            }
            let ptr = GlobalLock(hmem);
            if ptr.is_null() {
                return Err("GlobalLock returned null".into());
            }
            std::ptr::write_bytes(ptr as *mut u8, 0, total);
            let df = ptr as *mut DROPFILES;
            (*df).pFiles = header as u32;
            (*df).fWide = 1;
            std::ptr::copy_nonoverlapping(
                wide.as_ptr(),
                (ptr as *mut u8).add(header) as *mut u16,
                wide.len(),
            );
            GlobalUnlock(hmem);
            // On success the clipboard owns the HGLOBAL - do not free it.
            if SetClipboardData(CF_HDROP as u32, hmem as HANDLE).is_null() {
                return Err("SetClipboardData failed".into());
            }
            Ok(())
        })();
        CloseClipboard();
        result
    }
}

#[cfg(any(not(windows), feature = "headless-sim"))]
pub fn copy_files_to_clipboard(paths: &[std::path::PathBuf]) -> Result<(), String> {
    if paths.is_empty() {
        return Err("nothing to copy".into());
    }
    for p in paths {
        if !p.exists() {
            return Err(format!("clip not found: {}", p.display()));
        }
    }
    let names: Vec<String> = paths.iter().map(|p| p.display().to_string()).collect();
    log::info!("[yascreenclipper] (stub) copied to clipboard: {}", names.join(", "));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clipboard_rejects_an_empty_selection() {
        // The Windows path returns early too; a 0-file drop is always a bug.
        assert!(copy_files_to_clipboard(&[]).is_err());
    }

    #[test]
    fn clipboard_reports_a_missing_file() {
        let missing = std::path::PathBuf::from("/definitely/not/here/clip.mp4");
        let err = copy_files_to_clipboard(&[missing]).unwrap_err();
        assert!(err.contains("clip not found"), "unexpected error: {err}");
    }

    #[test]
    fn zero_handles_are_ignored_by_every_window_control() {
        // Guards the "HWND not captured yet" path: the first frame may render
        // before `window_handle()` was read, and none of these may panic.
        set_topmost(0, true);
        set_visible(0, false);
        activate(0);
        set_capture_excluded(0, true);
    }
}

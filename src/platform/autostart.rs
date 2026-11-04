//! Launch-at-startup through the per-user Run key.
//!
//! Two-way reconciliation, deliberately: the `launch_at_startup` *setting* and
//! the *registry state* are compared at startup and the setting wins, so a
//! corrupt `settings.json` can never leave an unwanted autostart behind, and a
//! user who removed the entry by hand gets it back if the setting says so.
//!
//! The value name and payload match what `tauri-plugin-autostart` wrote, so an
//! existing install keeps its Run entry across the migration instead of
//! accumulating a second one.

/// `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`
#[cfg(all(windows, not(feature = "headless-sim")))]
const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

/// Registry value name. This is the *value*, not the key path, and it is
/// what the old Tauri plugin wrote.
pub const VALUE_NAME: &str = "ClipFlow";

/// The exact command line the Run entry should hold. Quoted because the
/// install path may contain spaces; no CLI args, since the app always starts
/// hidden-in-tray behaviour itself.
pub fn launch_command(exe: &std::path::Path) -> String {
    format!("\"{}\"", exe.display())
}

/// Reads the current Run entry, if any.
#[cfg(all(windows, not(feature = "headless-sim")))]
pub fn current() -> Option<String> {
    use windows_sys::Win32::System::Registry::{
        HKEY_CURRENT_USER, KEY_READ, REG_SZ, RegCloseKey, RegOpenKeyExW, RegQueryValueExW,
    };

    let subkey = wide(RUN_KEY);
    let value = wide(VALUE_NAME);
    let mut hkey = std::ptr::null_mut();
    unsafe {
        let status = RegOpenKeyExW(
            HKEY_CURRENT_USER,
            subkey.as_ptr(),
            0,
            KEY_READ,
            &mut hkey,
        );
        if status != 0 {
            return None;
        }
        let mut kind = 0u32;
        let mut len = 0u32;
        // First call sizes the buffer.
        let status = RegQueryValueExW(
            hkey,
            value.as_ptr(),
            std::ptr::null_mut(),
            &mut kind,
            std::ptr::null_mut(),
            &mut len,
        );
        if status != 0 || kind != REG_SZ || len == 0 {
            RegCloseKey(hkey);
            return None;
        }
        let mut buf = vec![0u8; len as usize];
        let status = RegQueryValueExW(
            hkey,
            value.as_ptr(),
            std::ptr::null_mut(),
            &mut kind,
            buf.as_mut_ptr(),
            &mut len,
        );
        RegCloseKey(hkey);
        if status != 0 {
            return None;
        }
        // REG_SZ is UTF-16LE with a trailing NUL.
        let units: Vec<u16> = buf
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .take_while(|&c| c != 0)
            .collect();
        Some(String::from_utf16_lossy(&units))
    }
}

#[cfg(any(not(windows), feature = "headless-sim"))]
pub fn current() -> Option<String> {
    None
}

/// Writes or removes the Run entry. `Some(path)` enables, `None` disables.
#[cfg(all(windows, not(feature = "headless-sim")))]
pub fn set(exe: Option<&std::path::Path>) -> Result<(), String> {
    use windows_sys::Win32::System::Registry::{
        HKEY_CURRENT_USER, KEY_SET_VALUE, REG_SZ, RegCloseKey, RegCreateKeyExW, RegDeleteValueW,
        RegSetValueExW,
    };

    let subkey = wide(RUN_KEY);
    let value = wide(VALUE_NAME);
    let mut hkey = std::ptr::null_mut();
    unsafe {
        let status = RegCreateKeyExW(
            HKEY_CURRENT_USER,
            subkey.as_ptr(),
            0,
            std::ptr::null(),
            0,
            KEY_SET_VALUE,
            std::ptr::null(),
            &mut hkey,
            std::ptr::null_mut(),
        );
        if status != 0 {
            return Err(format!("could not open the Run key (code {status})"));
        }
        let result = match exe {
            Some(path) => {
                let data = wide(&launch_command(path));
                let bytes = std::slice::from_raw_parts(
                    data.as_ptr() as *const u8,
                    data.len() * std::mem::size_of::<u16>(),
                );
                let status = RegSetValueExW(
                    hkey,
                    value.as_ptr(),
                    0,
                    REG_SZ,
                    bytes.as_ptr(),
                    bytes.len() as u32,
                );
                if status == 0 {
                    Ok(())
                } else {
                    Err(format!("could not write the Run entry (code {status})"))
                }
            }
            None => {
                let status = RegDeleteValueW(hkey, value.as_ptr());
                // 2 = ERROR_FILE_NOT_FOUND: nothing to delete is success.
                if status == 0 || status == 2 {
                    Ok(())
                } else {
                    Err(format!("could not remove the Run entry (code {status})"))
                }
            }
        };
        RegCloseKey(hkey);
        result
    }
}

#[cfg(any(not(windows), feature = "headless-sim"))]
pub fn set(exe: Option<&std::path::Path>) -> Result<(), String> {
    log::info!(
        "[clipflow] (stub) autostart {}",
        match exe {
            Some(p) => format!("enabled -> {}", p.display()),
            None => "disabled".to_string(),
        }
    );
    Ok(())
}

/// Brings the Run key in line with the persisted setting. Called once at
/// startup (and after every change to `launch_at_startup`). Errors are logged,
/// never fatal: a locked registry must not stop the app from running.
pub fn reconcile(enabled: bool) {
    let exe = std::env::current_exe().ok();
    let desired = if enabled { exe.as_deref() } else { None };

    // Avoid a pointless write on every launch: compare the stored command line.
    if let (Some(path), Some(existing)) = (desired, current()) {
        if existing == launch_command(path) {
            return;
        }
    } else if desired.is_none() && current().is_none() {
        return;
    }

    if let Err(e) = set(desired) {
        log::warn!("[clipflow] autostart reconcile failed: {e}");
    }
}

#[cfg(all(windows, not(feature = "headless-sim")))]
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_command_is_quoted_for_spaces() {
        let exe = std::path::Path::new(r"C:\Program Files\ClipFlow\clipflow.exe");
        assert_eq!(
            launch_command(exe),
            "\"C:\\Program Files\\ClipFlow\\clipflow.exe\""
        );
    }

    #[test]
    fn stub_reconcile_is_a_no_op() {
        // Must never panic off-Windows; the app calls it unconditionally.
        reconcile(true);
        reconcile(false);
    }
}

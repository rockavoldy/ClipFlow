//! Update probe.
//!
//! The old build used `tauri-plugin-updater`, which needs the Tauri runtime.
//! The GPUI build only needs the *check*: fetch `latest.json` from the Releases
//! feed, compare the version, and hand the user the download page. Installing a
//! signed update is deferred (see CHANGELOG), so there is no signing key, no
//! installer invocation and no background service.
//!
//! **No new dependency:** WinHTTP is part of Windows. A `ureq`/`reqwest` stack
//! would add 1-2 MB to a 6 MB exe for one JSON GET. Everything here runs on a
//! background task; nothing blocks the UI.

/// Where the manifest lives. Matches the old updater endpoint, so an existing
/// release pipeline keeps working unchanged.
pub const LATEST_JSON: &str =
    "https://github.com/jaimitus/ClipFlow/releases/latest/download/latest.json";

/// The public releases page, opened when the user wants the download.
pub const RELEASES_PAGE: &str = "https://github.com/jaimitus/ClipFlow/releases";

/// Compares dotted versions numerically. `0.10.0 > 0.9.9`, which a string
/// comparison gets wrong.
pub fn is_newer(remote: &str, local: &str) -> bool {
    let parse = |v: &str| -> Vec<u64> {
        v.trim()
            .trim_start_matches('v')
            .split('.')
            .map(|p| {
                p.chars()
                    .take_while(|c| c.is_ascii_digit())
                    .collect::<String>()
                    .parse()
                    .unwrap_or(0)
            })
            .collect()
    };
    let (mut r, mut l) = (parse(remote), parse(local));
    // Pad so `1.2` and `1.2.0` compare equal.
    let len = r.len().max(l.len());
    r.resize(len, 0);
    l.resize(len, 0);
    r > l
}

/// Pulls `"version"` out of the manifest.
///
/// Deliberately lenient: the manifest is also consumed by the Tauri updater
/// format, and a full struct would break if a field we do not care about
/// changes.
pub fn version_from_manifest(json: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(json).ok()?;
    let version = value.get("version")?.as_str()?.trim().to_string();
    if version.is_empty() { None } else { Some(version) }
}

/// Result of a check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateCheck {
    UpToDate,
    Available { version: String },
}

/// Fetches and compares. Blocking: call from `background_spawn`.
#[cfg(all(windows, not(feature = "headless-sim")))]
pub fn check(current: &str) -> Result<UpdateCheck, String> {
    let body = http_get(LATEST_JSON)?;
    let remote =
        version_from_manifest(&body).ok_or_else(|| "release feed has no version".to_string())?;
    Ok(if is_newer(&remote, current) {
        UpdateCheck::Available { version: remote }
    } else {
        UpdateCheck::UpToDate
    })
}

/// Off-Windows / headless: there is no WinHTTP stack to lean on, and the check
/// is cosmetic. Report it as unsupported rather than pretending to be current.
#[cfg(any(not(windows), feature = "headless-sim"))]
pub fn check(_current: &str) -> Result<UpdateCheck, String> {
    Err("update checks require Windows".to_string())
}

/// Opens the Releases page in the default browser.
pub fn open_releases_page() -> Result<(), String> {
    crate::platform::win32::shell_open(std::path::Path::new(RELEASES_PAGE), false)
}

/// Minimal HTTPS GET via WinHTTP. Follows redirects (GitHub's download URL
/// redirects to a CDN), caps the body at 64 KiB — the manifest is a few hundred
/// bytes and anything larger is not the manifest.
#[cfg(all(windows, not(feature = "headless-sim")))]
fn http_get(url: &str) -> Result<String, String> {
    use windows_sys::Win32::Networking::WinHttp::{
        WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, WINHTTP_FLAG_SECURE, WINHTTP_QUERY_STATUS_CODE,
        WINHTTP_QUERY_FLAG_NUMBER, WinHttpCloseHandle, WinHttpConnect, WinHttpOpen,
        WinHttpOpenRequest, WinHttpQueryHeaders, WinHttpReadData, WinHttpReceiveResponse,
        WinHttpSendRequest,
    };

    const MAX_BODY: usize = 64 * 1024;
    let wide = |s: &str| -> Vec<u16> { s.encode_utf16().chain(std::iter::once(0)).collect() };

    // Split "https://host/path" into the pieces WinHTTP wants.
    let rest = url
        .strip_prefix("https://")
        .ok_or_else(|| "only https is supported".to_string())?;
    let (host, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };

    let agent = wide("ClipFlow");
    let host_w = wide(host);
    let path_w = wide(path);
    let verb = wide("GET");

    unsafe {
        let session = WinHttpOpen(
            agent.as_ptr(),
            WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
            std::ptr::null(),
            std::ptr::null(),
            0,
        );
        if session.is_null() {
            return Err("WinHttpOpen failed".into());
        }
        let result = (|| -> Result<String, String> {
            let connection = WinHttpConnect(session, host_w.as_ptr(), 443, 0);
            if connection.is_null() {
                return Err("WinHttpConnect failed".into());
            }
            let request = WinHttpOpenRequest(
                connection,
                verb.as_ptr(),
                path_w.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null_mut(),
                WINHTTP_FLAG_SECURE,
            );
            if request.is_null() {
                WinHttpCloseHandle(connection);
                return Err("WinHttpOpenRequest failed".into());
            }
            let send = WinHttpSendRequest(
                request,
                std::ptr::null(),
                0,
                std::ptr::null_mut(),
                0,
                0,
                0,
            );
            let mut out = Err("WinHttpSendRequest failed".to_string());
            if send != 0 && WinHttpReceiveResponse(request, std::ptr::null_mut()) != 0 {
                // Only a 200 is the manifest; a 302 to the login page or a 404
                // must not be parsed as JSON.
                let mut status: u32 = 0;
                let mut len = std::mem::size_of::<u32>() as u32;
                let ok_status = WinHttpQueryHeaders(
                    request,
                    WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
                    std::ptr::null(),
                    &mut status as *mut u32 as *mut _,
                    &mut len,
                    std::ptr::null_mut(),
                ) != 0
                    && status == 200;

                if ok_status {
                    let mut body: Vec<u8> = Vec::new();
                    let mut buf = [0u8; 4096];
                    loop {
                        let mut read = 0u32;
                        let ok = WinHttpReadData(
                            request,
                            buf.as_mut_ptr() as *mut _,
                            buf.len() as u32,
                            &mut read,
                        );
                        if ok == 0 || read == 0 {
                            break;
                        }
                        body.extend_from_slice(&buf[..read as usize]);
                        if body.len() > MAX_BODY {
                            return Err("release manifest is unexpectedly large".into());
                        }
                    }
                    out = String::from_utf8(body)
                        .map_err(|e| format!("release manifest is not UTF-8: {e}"));
                } else {
                    out = Err(format!("release feed returned HTTP {status}"));
                }
            }
            WinHttpCloseHandle(request);
            WinHttpCloseHandle(connection);
            out
        })();
        WinHttpCloseHandle(session);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_comparison_is_numeric_not_lexical() {
        assert!(is_newer("0.10.0", "0.9.9"));
        assert!(is_newer("1.4.3", "1.4.2"));
        assert!(is_newer("2.0.0", "1.99.99"));
        assert!(!is_newer("1.4.2", "1.4.2"));
        assert!(!is_newer("1.4.1", "1.4.2"));
    }

    #[test]
    fn versions_with_different_component_counts_compare_sanely() {
        assert!(!is_newer("1.2", "1.2.0"));
        assert!(!is_newer("1.2.0", "1.2"));
        assert!(is_newer("1.2.1", "1.2"));
        assert!(is_newer("1.3", "1.2.9"));
    }

    #[test]
    fn a_leading_v_and_prerelease_suffix_are_tolerated() {
        // The feed may publish `v1.5.0` or `1.5.0-beta.1`.
        assert!(is_newer("v1.5.0", "1.4.2"));
        assert!(is_newer("1.5.0-beta.1", "1.4.2"));
        assert!(!is_newer("v1.4.2", "1.4.2"));
    }

    #[test]
    fn the_manifest_version_is_extracted() {
        let json = r#"{"version":"1.5.0","notes":"x","platforms":{}}"#;
        assert_eq!(version_from_manifest(json).as_deref(), Some("1.5.0"));
    }

    #[test]
    fn a_manifest_without_a_version_is_rejected() {
        assert_eq!(version_from_manifest(r#"{"notes":"x"}"#), None);
        assert_eq!(version_from_manifest(r#"{"version":""}"#), None);
        assert_eq!(version_from_manifest(r#"{"version":42}"#), None);
        assert_eq!(version_from_manifest("not json"), None);
    }

    #[test]
    fn the_manifest_endpoint_matches_the_old_updater() {
        // An existing release pipeline publishes here; changing it silently
        // would break update checks for every user.
        assert!(LATEST_JSON.contains("github.com/jaimitus/ClipFlow/releases/latest/download/latest.json"));
        assert_eq!(
            RELEASES_PAGE,
            "https://github.com/jaimitus/ClipFlow/releases"
        );
    }
}

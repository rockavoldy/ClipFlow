//! Number/time formatting shared by every view.
//!
//! Ported from `src/lib/format.ts` so the deck reads the same as the Tauri
//! build. The rounding rules are user-visible and are asserted in the tests
//! below (a "1.0 GB" that should read "1.00 GB" looks broken).

/// `1_234_567` -> `"1.2 MB"`. Binary units, one decimal past bytes.
pub fn bytes(n: u64) -> String {
    bytes_with_digits(n, 1)
}

/// Same as [`bytes`] with an explicit decimal count, mirroring
/// `formatBytes(bytes, digits)`.
pub fn bytes_with_digits(n: u64, digits: usize) -> String {
    if n == 0 {
        return "0 B".to_string();
    }
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut i = 0usize;
    let mut value = n as f64;
    while value >= 1024.0 && i < UNITS.len() - 1 {
        value /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{n} B")
    } else {
        format!("{value:.digits$} {}", UNITS[i])
    }
}

/// `83.4` -> `"1:23"`. Floors, like the original, so a clip never displays a
/// duration longer than it actually holds.
pub fn duration(seconds: f32) -> String {
    let seconds = if seconds.is_finite() && seconds > 0.0 {
        seconds
    } else {
        0.0
    };
    let total = seconds.floor() as u64;
    let (h, m, s) = (total / 3600, (total % 3600) / 60, total % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// `12_000` -> `"12.0 Mbps"`. The deck shows bitrate in Mbps while settings
/// store kbps.
pub fn bitrate_mbps(kbps: u32) -> String {
    format!("{:.1} Mbps", kbps as f64 / 1000.0)
}

/// Relative timestamp for clip cards, matching `formatRelativeTime`:
/// under 45 s is "just now", then min/h/d, then a `Mon D` date.
pub fn ago(created_unix_ms: i64, now_unix_ms: i64) -> String {
    let delta = now_unix_ms.saturating_sub(created_unix_ms);
    if delta < 45_000 {
        return "just now".to_string();
    }
    let minutes = (delta as f64 / 60_000.0).round() as i64;
    if minutes < 60 {
        return format!("{minutes} min ago");
    }
    let hours = (minutes as f64 / 60.0).round() as i64;
    if hours < 24 {
        return format!("{hours} h ago");
    }
    let days = (hours as f64 / 24.0).round() as i64;
    if days < 7 {
        return format!("{days} d ago");
    }
    // Beyond a week the deck showed a calendar date; a relative one becomes
    // meaningless. Format from the epoch day without pulling in a locale.
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let (month, day) = civil_from_unix_ms(created_unix_ms);
    format!("{} {}", MONTHS[month], day)
}

/// Converts a Unix millisecond timestamp into `(month_index, day_of_month)`.
///
/// Howard Hinnant's civil-from-days algorithm; avoids a date crate in the
/// render path.
fn civil_from_unix_ms(ms: i64) -> (usize, u32) {
    let days = ms.div_euclid(86_400_000);
    // days since 1970-01-01 -> civil date
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    ((month - 1) as usize, day)
}

/// Memory pressure label for the ECO row.
pub fn gib(n: u64) -> String {
    format!("{:.1} GB", n as f64 / (1024.0 * 1024.0 * 1024.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_uses_binary_units_with_one_decimal() {
        assert_eq!(bytes(0), "0 B");
        assert_eq!(bytes(512), "512 B");
        assert_eq!(bytes(1024), "1.0 KB");
        assert_eq!(bytes(1_234_567), "1.2 MB");
        assert_eq!(bytes(1024 * 1024 * 1024), "1.0 GB");
        assert_eq!(bytes_with_digits(1024, 2), "1.00 KB");
    }

    #[test]
    fn bytes_caps_at_terabytes() {
        // u64::MAX / 1024^4 is ~16 TB; the unit list must not overflow.
        assert!(bytes(u64::MAX).ends_with("TB"), "{}", bytes(u64::MAX));
    }

    #[test]
    fn durations_floor_and_grow_an_hour_field() {
        assert_eq!(duration(0.0), "0:00");
        assert_eq!(duration(5.9), "0:05");
        assert_eq!(duration(83.4), "1:23");
        assert_eq!(duration(59.99), "0:59");
        assert_eq!(duration(3600.0), "1:00:00");
        assert_eq!(duration(3725.0), "1:02:05");
    }

    #[test]
    fn durations_survive_nonsense() {
        // NaN/negative can come from a partially-probed clip; a panic in the
        // render path would take the whole window down.
        assert_eq!(duration(f32::NAN), "0:00");
        assert_eq!(duration(-10.0), "0:00");
    }

    #[test]
    fn bitrate_is_shown_in_mbps() {
        assert_eq!(bitrate_mbps(12_000), "12.0 Mbps");
        assert_eq!(bitrate_mbps(60_000), "60.0 Mbps");
    }

    #[test]
    fn ago_matches_the_old_thresholds() {
        assert_eq!(ago(1_000, 1_000), "just now");
        assert_eq!(ago(0, 44_000), "just now");
        assert_eq!(ago(0, 45_000), "1 min ago");
        assert_eq!(ago(0, 5 * 60_000), "5 min ago");
        assert_eq!(ago(0, 2 * 3_600_000), "2 h ago");
        assert_eq!(ago(0, 3 * 86_400_000), "3 d ago");
        // A clock that went backwards must not render a negative age.
        assert_eq!(ago(10_000, 5_000), "just now");
    }

    #[test]
    fn ago_falls_back_to_a_calendar_date_past_a_week() {
        // 2024-03-05T00:00:00Z
        let created = 1_709_596_800_000i64;
        let now = created + 10 * 86_400_000;
        assert_eq!(ago(created, now), "Mar 5");
    }

    #[test]
    fn civil_dates_are_correct_across_leap_years() {
        // 2000-02-29 (a leap day) and 2024-02-29.
        assert_eq!(civil_from_unix_ms(951_782_400_000), (1, 29));
        assert_eq!(civil_from_unix_ms(1_709_164_800_000), (1, 29));
        // Unix epoch itself.
        assert_eq!(civil_from_unix_ms(0), (0, 1));
    }

    #[test]
    fn gib_is_one_decimal() {
        assert_eq!(gib(0), "0.0 GB");
        assert_eq!(gib(4 * 1024 * 1024 * 1024), "4.0 GB");
    }
}

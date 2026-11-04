//! Trimmer policy, retained with its tests.
//!
//! The trimmer itself is deferred to a follow-up plan (see the README), but
//! `split_at_seconds` is a pure clamp with a user-visible contract: neither half
//! of a split may be empty. Keeping it here — and keeping its tests — means the
//! follow-up does not have to rediscover the rule, and the engine's
//! `trim_stream_copy` already enforces the same minimum at the stream level.

/// Validates a split point for a clip of `duration_seconds`.
///
/// Rejects clips at or below 0.4 s (two 0.2 s halves is the floor) and clamps
/// the playhead into `[0.2, duration - 0.2]`, so a split at the very start or
/// past the end still produces two non-empty parts.
pub fn split_at_seconds(duration_seconds: f32, at_seconds: f32) -> Result<f32, String> {
    if duration_seconds <= 0.4 {
        return Err("clip is too short to split".to_string());
    }
    Ok(at_seconds.clamp(0.2, duration_seconds - 0.2))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_clips_too_short_to_split() {
        // 0.4 s is the minimum *total* length: it cannot produce two real
        // halves, and everything shorter is rejected too.
        assert!(split_at_seconds(0.4, 0.2).is_err());
        assert!(split_at_seconds(0.3, 0.1).is_err());
        assert!(split_at_seconds(0.0, 0.0).is_err());
    }

    #[test]
    fn passes_through_playheads_inside_the_media() {
        assert_eq!(split_at_seconds(10.0, 5.0).unwrap(), 5.0);
        assert_eq!(split_at_seconds(60.0, 30.0).unwrap(), 30.0);
        assert_eq!(split_at_seconds(10.0, 3.25).unwrap(), 3.25);
    }

    #[test]
    fn clamps_playhead_to_a_minimum_half_length() {
        // At the very start -> pushed to 0.2 s so part A is non-empty.
        assert_eq!(split_at_seconds(10.0, 0.0).unwrap(), 0.2);
        assert_eq!(split_at_seconds(10.0, -5.0).unwrap(), 0.2);
        // At/after the end -> pushed to duration - 0.2 s so part B is non-empty.
        assert_eq!(split_at_seconds(10.0, 9.9).unwrap(), 9.8);
        assert_eq!(split_at_seconds(10.0, 999.0).unwrap(), 9.8);
    }

    #[test]
    fn clamp_keeps_both_halves_inside_the_media() {
        // Whatever the playhead, both halves must be within [0, duration].
        for at in [-100.0, -0.1, 0.0, 0.2, 1.0, 5.0, 9.8, 10.0, 100.0] {
            let cut = split_at_seconds(10.0, at).unwrap();
            assert!(cut >= 0.2, "cut {cut} below 0.2");
            assert!(cut <= 9.8, "cut {cut} above duration - 0.2");
        }
    }
}

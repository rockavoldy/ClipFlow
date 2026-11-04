//! Library filter / sort / persisted view state.
//!
//! Ported from `src/lib/galleryState.ts` plus the `filtered` memo in
//! `App.tsx:1041`. Two behaviours here are easy to lose and are the reason this
//! is a module and not inline code in the view:
//!
//! 1. **Stale filters fall back.** A persisted `game_filter` or `tag_filter`
//!    may point at a game or tag that no longer exists (renamed exe, clips
//!    deleted). Without the fallback the library looks mysteriously empty after
//!    a restart.
//! 2. **Selection ranges span the whole filtered list**, so shift-click works
//!    across pages that are not rendered yet.

use serde::{Deserialize, Serialize};

use crate::media::library::ClipMetadata;

/// Longest query worth keeping — guards against junk data left in storage.
pub const MAX_QUERY_LENGTH: usize = 200;
pub const ALL_GAMES: &str = "all";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SortKey {
    #[default]
    Newest,
    Oldest,
    Largest,
    Smallest,
    Longest,
    Shortest,
}

impl SortKey {
    pub fn as_str(self) -> &'static str {
        match self {
            SortKey::Newest => "newest",
            SortKey::Oldest => "oldest",
            SortKey::Largest => "largest",
            SortKey::Smallest => "smallest",
            SortKey::Longest => "longest",
            SortKey::Shortest => "shortest",
        }
    }

    /// Every key, in the order the sort dropdown shows them.
    pub const ALL: [SortKey; 6] = [
        SortKey::Newest,
        SortKey::Oldest,
        SortKey::Largest,
        SortKey::Smallest,
        SortKey::Longest,
        SortKey::Shortest,
    ];

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.as_str() == s)
    }
}

/// The persisted view state. Field names match the old localStorage blob so an
/// existing `clipflow.gallery.state.v1` value loads unchanged.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct GalleryState {
    pub query: String,
    pub game_filter: String,
    pub sort_key: SortKey,
    pub audio_only: bool,
    pub fav_only: bool,
    pub tag_filter: Option<String>,
    pub compact: bool,
}

impl Default for GalleryState {
    fn default() -> Self {
        Self {
            query: String::new(),
            game_filter: ALL_GAMES.to_string(),
            sort_key: SortKey::Newest,
            audio_only: false,
            fav_only: false,
            tag_filter: None,
            compact: false,
        }
    }
}

impl GalleryState {
    /// Coerces whatever came out of storage into a valid state. Mirrors
    /// `sanitizeGalleryState`: a wrong-typed field falls back, it never fails
    /// the whole load.
    pub fn sanitize(raw: &serde_json::Value) -> Self {
        let d = Self::default();
        let Some(obj) = raw.as_object() else {
            return d;
        };
        Self {
            query: obj
                .get("query")
                .and_then(|v| v.as_str())
                .map(|s| s.chars().take(MAX_QUERY_LENGTH).collect())
                .unwrap_or(d.query),
            game_filter: obj
                .get("gameFilter")
                .and_then(|v| v.as_str())
                .map(str::to_string)
                .unwrap_or(d.game_filter),
            // A key we no longer know about (or a number) falls back to newest.
            sort_key: obj
                .get("sortKey")
                .and_then(|v| v.as_str())
                .and_then(SortKey::parse)
                .unwrap_or(d.sort_key),
            audio_only: obj.get("audioOnly").and_then(|v| v.as_bool()).unwrap_or(false),
            fav_only: obj.get("favOnly").and_then(|v| v.as_bool()).unwrap_or(false),
            tag_filter: obj
                .get("tagFilter")
                .and_then(|v| v.as_str())
                .map(str::to_string),
            compact: obj.get("compact").and_then(|v| v.as_bool()).unwrap_or(false),
        }
    }
}

/// Applies the filters and the sort, returning a fresh list in view order.
///
/// Falls back the game/tag filter when nothing matches it, exactly as
/// `App.tsx` did.
pub fn filter_and_sort(clips: &[ClipMetadata], state: &GalleryState) -> Vec<ClipMetadata> {
    let q = state.query.trim().to_ascii_lowercase();

    // A persisted game/tag filter may point at a game or tag that no longer
    // exists (renamed exe, clips deleted) — fall back so the library never
    // looks mysteriously empty after a restart.
    // `None` here means "no game constraint": either the filter is "all", or it
    // names a game that no longer exists in the library.
    let active_game = if state.game_filter == ALL_GAMES {
        None
    } else if clips
        .iter()
        .any(|c| c.game.as_deref() == Some(state.game_filter.as_str()))
    {
        Some(state.game_filter.as_str())
    } else {
        None
    };
    let active_tag = state.tag_filter.as_ref().filter(|tag| {
        clips.iter().any(|c| c.tags.iter().any(|t| t == *tag))
    });

    let mut out: Vec<ClipMetadata> = clips
        .iter()
        .filter(|c| {
            let matches_query = q.is_empty()
                || c.title.to_ascii_lowercase().contains(&q)
                || c.file_name.to_ascii_lowercase().contains(&q);
            let matches_audio = !state.audio_only || c.has_audio;
            let matches_game = match active_game {
                Some(game) => c.game.as_deref() == Some(game),
                None => true, // "all" or a stale filter
            };
            let matches_fav = !state.fav_only || c.favorite;
            let matches_tag = match active_tag {
                Some(tag) => c.tags.iter().any(|t| t == tag),
                None => true,
            };
            matches_query && matches_audio && matches_game && matches_fav && matches_tag
        })
        .cloned()
        .collect();

    sort_clips(&mut out, state.sort_key);
    out
}

pub fn sort_clips(clips: &mut [ClipMetadata], key: SortKey) {
    match key {
        SortKey::Newest => clips.sort_by_key(|c| std::cmp::Reverse(c.created_unix_ms)),
        SortKey::Oldest => clips.sort_by_key(|c| c.created_unix_ms),
        SortKey::Largest => clips.sort_by_key(|c| std::cmp::Reverse(c.size_bytes)),
        SortKey::Smallest => clips.sort_by_key(|c| c.size_bytes),
        SortKey::Longest => clips.sort_by(|a, b| {
            b.duration_seconds
                .partial_cmp(&a.duration_seconds)
                .unwrap_or(std::cmp::Ordering::Equal)
        }),
        SortKey::Shortest => clips.sort_by(|a, b| {
            a.duration_seconds
                .partial_cmp(&b.duration_seconds)
                .unwrap_or(std::cmp::Ordering::Equal)
        }),
    }
}

/// Distinct game names present in the library, sorted, for the filter dropdown.
/// `None` (clips in the output root) is shown as "Unorganised".
pub fn available_games(clips: &[ClipMetadata]) -> Vec<String> {
    let mut games: Vec<String> = clips.iter().filter_map(|c| c.game.clone()).collect();
    games.sort();
    games.dedup();
    games
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clip(name: &str, game: Option<&str>, created: i64, size: u64, dur: f32) -> ClipMetadata {
        ClipMetadata {
            id: name.to_string(),
            path: format!("/clips/{name}"),
            file_name: format!("{name}.mp4"),
            title: name.to_string(),
            duration_seconds: dur,
            size_bytes: size,
            created_unix_ms: created,
            width: 1920,
            height: 1080,
            fps: 60,
            has_audio: true,
            game: game.map(str::to_string),
            thumbnail: None,
            favorite: false,
            tags: Vec::new(),
        }
    }

    fn library() -> Vec<ClipMetadata> {
        let mut a = clip("alpha", Some("cs2"), 1_000, 500, 30.0);
        a.favorite = true;
        a.tags = vec!["clutch".into()];
        let b = clip("bravo", Some("cs2"), 3_000, 100, 10.0);
        let mut c = clip("charlie", Some("elden"), 2_000, 900, 60.0);
        c.has_audio = false;
        c.tags = vec!["boss".into()];
        let d = clip("delta", None, 4_000, 200, 5.0);
        vec![a, b, c, d]
    }

    #[test]
    fn defaults_match_the_old_frontend() {
        let d = GalleryState::default();
        assert_eq!(d.query, "");
        assert_eq!(d.game_filter, "all");
        assert_eq!(d.sort_key, SortKey::Newest);
        assert!(!d.audio_only);
        assert!(!d.fav_only);
        assert_eq!(d.tag_filter, None);
        assert!(!d.compact);
    }

    #[test]
    fn newest_is_the_default_order() {
        let out = filter_and_sort(&library(), &GalleryState::default());
        let names: Vec<&str> = out.iter().map(|c| c.title.as_str()).collect();
        assert_eq!(names, vec!["delta", "bravo", "charlie", "alpha"]);
    }

    #[test]
    fn every_sort_key_orders_correctly() {
        let lib = library();
        let titles = |key: SortKey| -> Vec<String> {
            let state = GalleryState {
                sort_key: key,
                ..GalleryState::default()
            };
            filter_and_sort(&lib, &state)
                .into_iter()
                .map(|c| c.title)
                .collect()
        };
        assert_eq!(titles(SortKey::Oldest)[0], "alpha");
        assert_eq!(titles(SortKey::Largest)[0], "charlie"); // 900
        assert_eq!(titles(SortKey::Smallest)[0], "bravo"); // 100
        assert_eq!(titles(SortKey::Longest)[0], "charlie"); // 60 s
        assert_eq!(titles(SortKey::Shortest)[0], "delta"); // 5 s
    }

    #[test]
    fn query_matches_title_or_file_name_case_insensitively() {
        let lib = library();
        let state = GalleryState {
            query: "CHAR".into(),
            ..GalleryState::default()
        };
        let out = filter_and_sort(&lib, &state);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].title, "charlie");
    }

    #[test]
    fn a_stale_game_filter_falls_back_instead_of_emptying_the_library() {
        let lib = library();
        let state = GalleryState {
            game_filter: "a-game-that-was-uninstalled".into(),
            ..GalleryState::default()
        };
        let out = filter_and_sort(&lib, &state);
        // Falls back to "all", so every clip survives.
        assert_eq!(out.len(), lib.len());
    }

    #[test]
    fn a_live_game_filter_only_shows_that_game() {
        let lib = library();
        let state = GalleryState {
            game_filter: "cs2".into(),
            ..GalleryState::default()
        };
        let out = filter_and_sort(&lib, &state);
        assert_eq!(out.len(), 2);
        assert!(out.iter().all(|c| c.game.as_deref() == Some("cs2")));
    }

    #[test]
    fn a_stale_tag_filter_falls_back_too() {
        let lib = library();
        let state = GalleryState {
            tag_filter: Some("deleted-tag".into()),
            ..GalleryState::default()
        };
        assert_eq!(filter_and_sort(&lib, &state).len(), lib.len());
    }

    #[test]
    fn fav_and_audio_filters_compose() {
        let lib = library();
        let state = GalleryState {
            fav_only: true,
            ..GalleryState::default()
        };
        let out = filter_and_sort(&lib, &state);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].title, "alpha");

        let state = GalleryState {
            audio_only: true,
            ..GalleryState::default()
        };
        let out = filter_and_sort(&lib, &state);
        // charlie is the only silent clip.
        assert!(out.iter().all(|c| c.title != "charlie"));
        assert_eq!(out.len(), 3);
    }

    #[test]
    fn sanitize_tolerates_every_kind_of_junk() {
        // Wrong types fall back field by field.
        let v = serde_json::json!({
            "query": 42,
            "gameFilter": null,
            "sortKey": "sideways",
            "audioOnly": "yes",
            "favOnly": 1,
            "tagFilter": 7,
            "compact": "true"
        });
        let s = GalleryState::sanitize(&v);
        assert_eq!(s, GalleryState::default());

        // Not an object at all.
        assert_eq!(
            GalleryState::sanitize(&serde_json::json!("nonsense")),
            GalleryState::default()
        );
    }

    #[test]
    fn sanitize_keeps_valid_values_and_caps_the_query() {
        let long = "x".repeat(MAX_QUERY_LENGTH + 50);
        let v = serde_json::json!({
            "query": long,
            "gameFilter": "cs2",
            "sortKey": "oldest",
            "audioOnly": true,
            "favOnly": true,
            "tagFilter": "clutch",
            "compact": true
        });
        let s = GalleryState::sanitize(&v);
        assert_eq!(s.query.chars().count(), MAX_QUERY_LENGTH);
        assert_eq!(s.game_filter, "cs2");
        assert_eq!(s.sort_key, SortKey::Oldest);
        assert!(s.audio_only && s.fav_only && s.compact);
        assert_eq!(s.tag_filter.as_deref(), Some("clutch"));
    }

    #[test]
    fn the_storage_blob_is_camel_case() {
        // An existing `clipflow.gallery.state.v1` value must load unchanged.
        let parsed: GalleryState = serde_json::from_str(
            r#"{"query":"","gameFilter":"all","sortKey":"newest","audioOnly":false,
                "favOnly":false,"tagFilter":null,"compact":false}"#,
        )
        .unwrap();
        assert_eq!(parsed, GalleryState::default());
    }

    #[test]
    fn available_games_are_sorted_and_deduplicated() {
        let games = available_games(&library());
        assert_eq!(games, vec!["cs2".to_string(), "elden".to_string()]);
    }

    #[test]
    fn empty_library_filters_to_empty() {
        let state = GalleryState {
            game_filter: "cs2".into(),
            ..GalleryState::default()
        };
        assert!(filter_and_sort(&[], &state).is_empty());
    }
}

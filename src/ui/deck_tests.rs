//! Tests for the deck's non-visual behaviour.
//!
//! Kept out of `deck.rs` on purpose: the render module imports `gpui_kit::*`
//! and compiles a very large element tree, and a `#[test]` inside it makes the
//! `#[test]` macro expand inside that same deeply nested module (the compiler
//! hits its macro recursion budget). A separate module with explicit imports
//! keeps the tests cheap to compile and the failure output readable.

use crate::policy::gallery::SortKey;
use crate::ui::deck::{load_gallery_state_from, released_value, save_gallery_state_to};
use crate::policy::gallery::GalleryState;


    #[test]
    fn released_value_ignores_drag_updates() {
        use gpui_kit::component::slider::{SliderEvent, SliderValue};
        // `Change` fires continuously while dragging and must never persist.
        assert_eq!(
            released_value(&SliderEvent::Change(SliderValue::Single(42.0))),
            None
        );
        assert_eq!(
            released_value(&SliderEvent::Release(SliderValue::Single(42.4))),
            Some(42)
        );
        assert_eq!(
            released_value(&SliderEvent::Release(SliderValue::Range(7.6, 9.0))),
            Some(8)
        );
        // A slider dragged below zero must not wrap to a huge u32.
        assert_eq!(
            released_value(&SliderEvent::Release(SliderValue::Single(-3.0))),
            Some(0)
        );
    }

    fn temp_state_path(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir()
            .join(format!("clipflow-gallery-{}-{tag}", std::process::id()))
            .join("gallery_state.json")
    }

    #[test]
    fn gallery_state_round_trips_through_the_saved_file() {
        let state = GalleryState {
            game_filter: "cs2".into(),
            sort_key: SortKey::Oldest,
            audio_only: true,
            tag_filter: Some("clutch".into()),
            ..GalleryState::default()
        };

        let path = temp_state_path("roundtrip");
        save_gallery_state_to(&path, &state);
        assert_eq!(load_gallery_state_from(&path), state);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn a_missing_blob_loads_defaults() {
        // First launch: no file at all.
        assert_eq!(
            load_gallery_state_from(std::path::Path::new("/no/such/gallery_state.json")),
            GalleryState::default()
        );
    }

    #[test]
    fn a_corrupt_blob_falls_back_to_defaults_instead_of_failing() {
        let path = temp_state_path("corrupt");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "this is not json").unwrap();
        assert_eq!(load_gallery_state_from(&path), GalleryState::default());
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn unknown_sort_keys_fall_back_to_newest() {
        // `sanitize` never fails on bad input; it coerces field by field.
        let junk: serde_json::Value = serde_json::from_str(r#"{"sortKey":"nope"}"#).unwrap();
        assert_eq!(GalleryState::sanitize(&junk).sort_key, SortKey::Newest);
    }

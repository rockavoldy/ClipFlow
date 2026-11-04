//! Tests for the deck's non-visual behaviour.
//!
//! Kept out of `deck.rs` on purpose: the render module imports `gpui_kit::*`
//! and compiles a very large element tree, and a `#[test]` inside it makes the
//! `#[test]` macro expand inside that same deeply nested module (the compiler
//! hits its macro recursion budget). A separate module with explicit imports
//! keeps the tests cheap to compile and the failure output readable.

use crate::ui::deck::released_value;

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

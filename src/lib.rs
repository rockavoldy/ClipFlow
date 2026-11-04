//! ClipFlow — instant replay and game clipping for Windows.
//!
//! # Architecture
//!
//! ```text
//! main.rs  ──> app::run()            bootstrap: globals, tray, window, loops
//!                │
//!                ├── ui::deck        the single window (deck)
//!                │     ├── capture_card   ARM/SAVE + telemetry
//!                │     ├── library        scan/pick/open/delete
//!                │     └── settings       one row per SettingsPatch field
//!                │
//!                ├── runtime::stats       4 Hz telemetry snapshot
//!                ├── runtime::foreground  2 s privacy gate + autosave
//!                ├── runtime::events      tray + hotkey + power drain
//!                │
//!                ├── state::AppState      engine + settings + clip metadata
//!                └── platform::*          win32 / tray / hotkeys / toast / autostart
//! ```
//!
//! The engine (`media::*`), `settings`, `clipmeta`, `foreground` and `power`
//! are unchanged from the Tauri build. Everything Tauri-specific lives in
//! `state`, `ui`, `runtime` and `platform`.
//!
//! # Threading
//!
//! The engine is synchronous and blocking by design. It is never called from
//! the render path: `runtime` spawns everything through `cx.background_spawn`
//! and pushes results back into the views. The two hot spots are the 4 Hz stats
//! poll and the 2 s foreground poll.

// gpui's element builders nest deeply enough that a plain `#[test]` in a view
// module overflows the default macro recursion budget.
#![recursion_limit = "512"]
//
// Two gotchas worth knowing before adding tests to a UI module:
//
// 1. **Never `use gpui_kit::*;` in a module that contains `#[test]`.** gpui
//    re-exports an attribute macro named `test`, so the glob shadows the
//    built-in attribute and rustc dies inside the macro with SIGBUS. Import
//    what you need by name instead.
// 2. Tests that need a `Context` live in `ui/deck_tests.rs`, not inline in
//    `deck.rs`: a `#[test]` inside that render module also overflows the macro
//    recursion budget.

pub mod app;
pub mod clipmeta;
pub mod foreground;
pub mod media;
pub mod platform;
pub mod policy;
pub mod power;
pub mod runtime;
pub mod settings;
pub mod state;
pub mod ui;

/// Entry point used by `main.rs`. Kept as a function (rather than inlined into
/// `main`) so the `windows_subsystem` attribute stays the only thing in the
/// binary.
pub fn run() {
    app::run();
}

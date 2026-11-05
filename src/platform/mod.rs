//! Host-OS integration that has no business being in the UI or the engine.
//!
//! Every submodule follows the crate-wide cfg convention:
//! `#[cfg(all(windows, not(feature = "headless-sim")))]` real implementation,
//! `#[cfg(any(not(windows), feature = "headless-sim"))]` stub. The stubs are
//! not dead weight: `cargo check --features headless-sim` is how the app is
//! developed on macOS, and the policy unit tests run on the stub path.

pub mod autostart;
pub mod dialog;
pub mod gamepad;
pub mod hotkeys;
pub mod notify;
pub mod tray;
pub mod update;
pub mod win32;

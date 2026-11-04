//! Background loops. Nothing in here touches the render path.
//!
//! They all follow the same shape: spawn with [`gpui_kit::Context::spawn`],
//! sleep on the **background** executor, do the blocking work there, then land
//! back on the UI thread via `this.update(...)` to store the snapshot and
//! `cx.notify()`.
//!
//! Replaces the Tauri event bus. `emit` is gone, so each loop writes into a
//! shared snapshot the view reads; the visible consequence is that a save now
//! mutates the clip list directly instead of being caught by a listener.

pub mod eco;
pub mod foreground;
pub mod stats;
pub mod tray;

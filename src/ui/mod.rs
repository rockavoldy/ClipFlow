//! The GPUI front end.
//!
//! One window, one root view ([`deck::Deck`]), and small modules for the things
//! that would otherwise bloat it:
//!
//! * [`theme`] — the palette and the component-theme override
//! * [`format`] — number/time strings, shared with tests
//! * [`capture`] — the pure SAVE-gate and telemetry logic
//! * [`chrome`] — titlebar, capture section, toasts, footer
//! * [`library`] — clip list, filters, selection wiring
//! * [`settings`] — one row per `SettingsPatch` field
//! * [`thumbs`] — PNG bytes → `RenderImage` cache

pub mod capture_card;
pub mod deck;
#[cfg(test)]
mod deck_tests;
pub mod format;
pub mod hotkey_input;
pub mod library;
pub mod settings;
pub mod theme;
pub mod thumbs;

//! The GPUI front end.
//!
//! One window, one root view ([`deck::Deck`]), and small modules for the things
//! that would otherwise bloat it:
//!
//! * [`theme`] — the palette and the component-theme override
//! * [`format`] — number/time strings, shared with tests
//! * [`capture_card`] — the pure SAVE-gate, telemetry and capture card
//! * [`settings`] — one row per `SettingsPatch` field
//! * [`hotkey_input`] — the click-then-press hotkey recorder

pub mod capture_card;
pub mod deck;
#[cfg(test)]
mod deck_tests;
pub mod format;
pub mod hotkey_input;
pub mod settings;
pub mod theme;

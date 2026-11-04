//! Pure view/session policy ported from the TypeScript frontend.
//!
//! These were Vitest suites in the Tauri build (`privacyGate.test.ts`,
//! `powerPolicy.test.ts`, `selection.ts`, `galleryState.ts`). They have no UI
//! or engine dependencies, so they port one-for-one and stay unit-tested. The
//! only thing that changed is the language.

pub mod eco;
pub mod gallery;
pub mod privacy;
pub mod selection;
pub mod trim;

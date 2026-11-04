//! Folder picking.
//!
//! GPUI ships a native picker (`App::prompt_for_paths`), and on Windows it is
//! `IFileOpenDialog` with `FOS_PICKFOLDERS` — exactly the dialog the Tauri app
//! showed. Using it means no `rfd`, no GTK, and the same modal parenting: the
//! dialog is owned by the active window, so it cannot fall behind the deck.
//!
//! The receiver is awaited from a GPUI task, never on the render path: the
//! dialog is modal and stays open until the user answers.

use std::path::PathBuf;

use gpui_kit::{AsyncApp, PathPromptOptions};

/// The prompt the deck shows. Kept here so the copy lives in one place.
pub fn options() -> PathPromptOptions {
    PathPromptOptions {
        files: false,
        directories: true,
        multiple: false,
        prompt: Some("Choose where clips are saved".into()),
    }
}

/// Opens the picker and awaits the answer.
///
/// A cancel (`Ok(Ok(None))`), a picker failure and a dropped oneshot all mean
/// "no new folder", so the caller reports a no-op instead of an error. The
/// multi-path arm is defensive: `multiple: false` should never produce more
/// than one entry.
pub async fn pick_folder(cx: &mut AsyncApp) -> Option<PathBuf> {
    let receiver = cx.update(|cx| cx.prompt_for_paths(options()));
    match receiver.await {
        Ok(Ok(Some(mut paths))) if paths.len() == 1 => paths.pop(),
        _ => None,
    }
}

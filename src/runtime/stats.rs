//! 4 Hz telemetry.
//!
//! The old build emitted `yascreenclipper://stats` every 250 ms and `App.tsx` stored
//! the payload. Here the loop writes the same snapshot into the deck's state
//! and calls `cx.notify()`.
//!
//! **Why `engine.stats()` is also read on demand:** the emit came *after* a
//! `spawn_blocking` in the old code, so the snapshot could lag a start/stop by
//! up to 250 ms. That made ARM feel unresponsive. The deck therefore reads the
//! engine directly right after an arm/disarm/save instead of waiting for the
//! next tick.

use std::time::Duration;

use gpui_kit::{AppContext as _, WeakEntity};

use crate::state::AppState;
use crate::ui::deck::Deck;

/// 4 Hz, matching the old emitter.
pub const TICK: Duration = Duration::from_millis(250);

/// Reads a stats snapshot. Cheap (all atomics) but still done off the UI thread
/// so a slow `process_rss_bytes` syscall can never hitch a frame.
pub fn read_stats(state: &AppState) -> crate::media::recorder::EngineStats {
    state.engine_stats()
}

/// Starts the loop. Returns immediately; the task lives until the view is
/// dropped.
pub fn start(deck: WeakEntity<Deck>, state: std::sync::Arc<AppState>, cx: &mut gpui_kit::App) {
    cx.spawn(async move |cx| {
        loop {
            // Snapshot on the background pool and hand it back to the UI.
            let state_bg = std::sync::Arc::clone(&state);
            let stats = cx
                .background_spawn(async move { state_bg.engine_stats() })
                .await;

            let alive = deck.update(cx, |deck, cx| {
                deck.on_stats(stats);
                cx.notify();
            });

            // The view was dropped (app quitting) — stop looping.
            if alive.is_err() {
                return;
            }

            cx.background_executor().timer(TICK).await;
        }
    })
    .detach();
}

//! Gamepad button drain.
//!
//! `platform::gamepad` runs the `gilrs` backend on its own thread and pushes
//! every button press onto a channel. This task, like the tray and global-hotkey
//! drains, polls that channel briefly and hands the press to the deck. Nothing
//! blocks and nothing here touches the render path.

use std::time::Duration;

use gpui_kit::WeakEntity;

use crate::ui::deck::Deck;

/// A little under one frame at 60 Hz: imperceptible for a hotkey, and cheap
/// enough that polling is cheaper than waking a blocking receiver.
pub const POLL: Duration = Duration::from_millis(16);

/// Drains gamepad presses into the deck.
///
/// While the settings recorder is armed the press binds the button; otherwise
/// it is looked up in the registry and dispatched exactly like a keyboard
/// hotkey.
#[cfg(all(windows, not(feature = "headless-sim")))]
pub fn start(deck: WeakEntity<Deck>, cx: &mut gpui_kit::App) {
    let buttons = crate::platform::gamepad::listen();
    cx.spawn(async move |cx| loop {
        loop {
            let button = match buttons.try_recv() {
                Ok(button) => button,
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                // The backend thread is gone (no controller runtime, app
                // shutting down). There is nothing left to drain.
                Err(std::sync::mpsc::TryRecvError::Disconnected) => return,
            };
            let updated = deck.update(cx, |deck, cx| {
                deck.on_gamepad_button(button, cx);
                cx.notify();
            });
            if updated.is_err() {
                return;
            }
        }
        cx.background_executor().timer(POLL).await;
    })
    .detach();
}

/// Off-Windows / headless there is no backend, so there is nothing to drain.
#[cfg(any(not(windows), feature = "headless-sim"))]
pub fn start(_deck: WeakEntity<Deck>, _cx: &mut gpui_kit::App) {}

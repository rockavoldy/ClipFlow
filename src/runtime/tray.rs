//! Tray + hotkey drains.
//!
//! Both producers push onto `std::sync::mpsc` channels from the Win32 thread
//! (`tray-icon`'s window proc and `global-hotkey`'s `WM_HOTKEY` handler), and
//! these tasks poll them. A blocking `recv()` would be simpler but would pin a
//! GPUI executor slot forever; a short sleep is the least surprising trade.
//!
//! Everything is dispatched by handing an intent to the deck, which owns the
//! engine and the window handle. Nothing here blocks.

use std::sync::mpsc::Receiver;
use std::time::Duration;

use gpui_kit::WeakEntity;

use crate::platform::tray::TrayAction;
use crate::ui::deck::{Deck, Intent};

/// Poll interval for both channels. 50 ms is imperceptible for a hotkey and
/// costs nothing measurable.
pub const POLL: Duration = Duration::from_millis(50);

/// Drains [`TrayAction`]s into the deck.
pub fn spawn_tray(deck: WeakEntity<Deck>, tray: Receiver<TrayAction>, cx: &mut gpui_kit::App) {
    cx.spawn(async move |cx| {
        loop {
            while let Ok(action) = tray.try_recv() {
                let intent = match action {
                    TrayAction::SaveReplay => Intent::TraySaveReplay,
                    TrayAction::SaveLast30 => Intent::TraySaveLast30,
                    TrayAction::ToggleArm => Intent::ToggleArm,
                    TrayAction::ShowWindow => Intent::ShowWindow,
                    TrayAction::OpenClipsFolder => Intent::OpenClipsFolder,
                    TrayAction::Quit => Intent::Quit,
                };
                if deck
                    .update(cx, |deck, cx| {
                        deck.handle_intent(intent, cx);
                        cx.notify();
                    })
                    .is_err()
                {
                    return;
                }
            }
            cx.background_executor().timer(POLL).await;
        }
    })
    .detach();
}

/// Drains global hotkey presses into the deck.
///
/// Only `Pressed` is acted on: `Released` also fires, and acting on both would
/// flush twice per keypress.
#[cfg(all(windows, not(feature = "headless-sim")))]
pub fn spawn_hotkeys(
    deck: WeakEntity<Deck>,
    hotkeys: std::sync::Arc<parking_lot::Mutex<crate::platform::hotkeys::Registered>>,
    cx: &mut gpui_kit::App,
) {
    cx.spawn(async move |cx| {
        loop {
            while let Ok(event) = global_hotkey::GlobalHotKeyEvent::receiver().try_recv() {
                if event.state != global_hotkey::HotKeyState::Pressed {
                    continue;
                }
                let Some(intent) = hotkeys.lock().intent_for(event.id) else {
                    continue;
                };
                if deck
                    .update(cx, |deck, cx| {
                        deck.handle_intent(intent, cx);
                        cx.notify();
                    })
                    .is_err()
                {
                    return;
                }
            }
            cx.background_executor().timer(POLL).await;
        }
    })
    .detach();
}

/// Off-Windows / headless there is no global hotkey manager; the menu and the
/// deck buttons are the whole surface.
#[cfg(any(not(windows), feature = "headless-sim"))]
pub fn spawn_hotkeys(
    _deck: WeakEntity<Deck>,
    _hotkeys: std::sync::Arc<parking_lot::Mutex<crate::platform::hotkeys::Registered>>,
    _cx: &mut gpui_kit::App,
) {
}

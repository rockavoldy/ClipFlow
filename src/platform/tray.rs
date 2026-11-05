//! System tray icon + menu.
//!
//! The menu is the only way to reach YAScreenClipper once the deck is hidden to the
//! tray, so it carries the full action set, not just "show window":
//!
//! ```text
//! Save replay        (Alt+C)
//! Save last 30 s
//! Arm / disarm       (Alt+Shift+C)
//! ─────────────
//! Open YAScreenClipper
//! Open clips folder
//! ─────────────
//! Quit
//! ```
//!
//! **Threading:** `tray-icon` creates a hidden window and requires a message
//! loop on the creating thread (`WM_USER`-posted icon callbacks are dispatched
//! there). It is created inside `Application::run`, so GPUI's
//! `GetMessageW(NULL)` loop pumps it. The `TrayIcon` value must outlive the
//! loop — dropping it removes the icon — which is why it is parked in a GPUI
//! global rather than a local.
//!
//! Menu clicks arrive on that same thread, so they are forwarded over a
//! `std::sync::mpsc` channel that a GPUI task drains. Nothing from `muda` is
//! touched from the UI thread.

use std::sync::mpsc::{Receiver, channel};

/// Everything the tray can ask the app to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayAction {
    SaveReplay,
    SaveLast30,
    ToggleArm,
    ShowWindow,
    OpenClipsFolder,
    Quit,
}

/// Item ids are strings because `muda` matches them back as `MenuId`.
#[cfg(all(windows, not(feature = "headless-sim")))]
const ID_SAVE: &str = "tray-save";
#[cfg(all(windows, not(feature = "headless-sim")))]
const ID_SAVE_30: &str = "tray-save-30";
#[cfg(all(windows, not(feature = "headless-sim")))]
const ID_TOGGLE: &str = "tray-toggle";
#[cfg(all(windows, not(feature = "headless-sim")))]
const ID_SHOW: &str = "tray-show";
#[cfg(all(windows, not(feature = "headless-sim")))]
const ID_FOLDER: &str = "tray-folder";
#[cfg(all(windows, not(feature = "headless-sim")))]
const ID_QUIT: &str = "tray-quit";

/// Owns the tray icon and the receiver side of the action channel.
#[cfg(all(windows, not(feature = "headless-sim")))]
pub struct Tray {
    /// Never read. Its only job is to stay alive: dropping `TrayIcon` removes
    /// the icon from the notification area.
    _icon: tray_icon::TrayIcon,
}

/// The icon is embedded at compile time from the same PNG set the installer
/// uses. Decoding is cheap (~2 KB) and happens once at startup.
#[cfg(all(windows, not(feature = "headless-sim")))]
fn icon() -> Result<tray_icon::Icon, String> {
    const ICON_PNG: &[u8] = include_bytes!("../../icons/32x32.png");
    let img = image::load_from_memory_with_format(ICON_PNG, image::ImageFormat::Png)
        .map_err(|e| format!("tray icon decode: {e}"))?
        .to_rgba8();
    let (w, h) = img.dimensions();
    tray_icon::Icon::from_rgba(img.into_raw(), w, h).map_err(|e| format!("tray icon: {e}"))
}

/// Builds the menu, the icon, and the click plumbing.
///
/// Returns `None` when the tray could not be created (missing shell, locked
/// session). That is not fatal: the deck still works and the user can quit from
/// the header, so the caller only logs it.
///
/// The action receiver is returned separately because it is *moved into* the
/// drain task, while the icon must stay owned by the caller for the process
/// lifetime.
#[cfg(all(windows, not(feature = "headless-sim")))]
pub fn install() -> (Option<Tray>, Receiver<TrayAction>) {
    use tray_icon::menu::{Menu, MenuItem, PredefinedMenuItem};
    use tray_icon::{TrayIconBuilder, TrayIconEvent};

    let (tx, actions) = channel::<TrayAction>();

    let menu = Menu::new();
    let save = MenuItem::with_id(ID_SAVE, "Save replay", true, None);
    let save30 = MenuItem::with_id(ID_SAVE_30, "Save last 30 s", true, None);
    let toggle = MenuItem::with_id(ID_TOGGLE, "Arm / disarm", true, None);
    let show = MenuItem::with_id(ID_SHOW, "Open YAScreenClipper", true, None);
    let folder = MenuItem::with_id(ID_FOLDER, "Open clips folder", true, None);
    let quit = MenuItem::with_id(ID_QUIT, "Quit", true, None);
    let sep1 = PredefinedMenuItem::separator();
    let sep2 = PredefinedMenuItem::separator();
    if let Err(e) = menu.append_items(&[
        &save, &save30, &toggle, &sep1, &show, &folder, &sep2, &quit,
    ]) {
        log::warn!("[yascreenclipper] tray menu build failed: {e}");
        return (None, actions);
    }

    // Menu events: forward the action, never do work inline. `muda` delivers
    // these on the thread that owns the menu (ours).
    let menu_tx = tx.clone();
    tray_icon::menu::MenuEvent::set_event_handler(Some(move |event: tray_icon::menu::MenuEvent| {
        let action = match event.id().as_ref() {
            ID_SAVE => TrayAction::SaveReplay,
            ID_SAVE_30 => TrayAction::SaveLast30,
            ID_TOGGLE => TrayAction::ToggleArm,
            ID_SHOW => TrayAction::ShowWindow,
            ID_FOLDER => TrayAction::OpenClipsFolder,
            ID_QUIT => TrayAction::Quit,
            _ => return,
        };
        let _ = menu_tx.send(action);
    }));

    // Left click restores the deck; that is the muscle memory for every tray
    // app, so it must not be reserved for the menu only.
    let click_tx = tx;
    TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| {
        if let TrayIconEvent::Click { button, .. } = event {
            if button == tray_icon::MouseButton::Left {
                let _ = click_tx.send(TrayAction::ShowWindow);
            }
        }
    }));

    let icon = match icon() {
        Ok(icon) => icon,
        Err(e) => {
            log::warn!("[yascreenclipper] {e}");
            return (None, actions);
        }
    };

    match TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("YAScreenClipper — instant replay")
        .with_icon(icon)
        .build()
    {
        Ok(icon) => (Some(Tray { _icon: icon }), actions),
        Err(e) => {
            log::warn!("[yascreenclipper] tray icon creation failed: {e}");
            (None, actions)
        }
    }
}

/// Off-Windows / headless there is no tray at all, so `Tray` does not exist.
/// The channel is still created so the app's drain task can be spawned
/// unconditionally and simply never receives anything.
#[cfg(any(not(windows), feature = "headless-sim"))]
pub struct Tray;

#[cfg(any(not(windows), feature = "headless-sim"))]
pub fn install() -> (Option<Tray>, Receiver<TrayAction>) {
    let (_tx, rx) = channel();
    (None, rx)
}

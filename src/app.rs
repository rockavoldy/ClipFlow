//! Application bootstrap.
//!
//! Order matters and mirrors the Tauri `setup` hook:
//!
//! 1. logging, settings, shared state
//! 2. `gpui_kit::init` + theme
//! 3. **autostart reconciliation** (the setting wins over a stale registry key)
//! 4. tray icon and global hotkeys — both must be created on this thread
//! 5. the window
//! 6. the runtime loops
//! 7. launch-time behaviour: arm the buffer, prune old clips
//!
//! Everything that can fail is logged and the app keeps going. A missing tray
//! icon or a hotkey owned by another app must not stop the deck from opening.

use std::sync::Arc;

use gpui_kit::*;

use crate::platform::hotkeys::Registered;
use crate::platform::tray::{self, Tray};
use crate::state::AppState;
use crate::ui::deck::Deck;

/// A GPUI global holding everything the runtime loops need.
///
/// Globals are the only way to reach app-wide state from `cx` inside a task
/// without threading an `Arc` through every call site.
pub struct Globals {
    pub state: Arc<AppState>,
    pub hotkeys: Arc<parking_lot::Mutex<Registered>>,
    /// Kept alive here: dropping `TrayIcon` removes the icon.
    pub tray: Option<Tray>,
}

impl Global for Globals {}

pub fn run() {
    init_logging();

    let state = Arc::new(AppState::new());

    // Reconcile autostart before the window opens: the *setting* is the source
    // of truth, so a corrupt settings.json can never leave an unwanted Run key,
    // and a user who deleted the key by hand gets it back.
    crate::platform::autostart::reconcile(state.settings_snapshot().launch_at_startup);

    let state_for_ui = Arc::clone(&state);
    gpui_kit::application().run(move |cx| {
        gpui_kit::init(cx);
        crate::ui::theme::init(cx);
        crate::platform::notify::init(cx);

        // Tray + hotkeys are created here, on the thread whose Win32 loop
        // `Application::run` owns. Both need a message loop on their creating
        // thread; GPUI's `GetMessageW(NULL)` pump services them.
        let (tray, tray_actions) = tray::install();
        let settings = state.settings_snapshot();
        let hotkeys = match Registered::install(&settings.hotkey_save, &settings.hotkey_toggle) {
            Ok(registered) => Arc::new(parking_lot::Mutex::new(registered)),
            Err(e) => {
                // A hotkey conflict must not stop the app: the deck buttons and
                // the tray menu still work, and the user can rebind. The
                // fallback bindings are *not* registered with the OS - if the
                // stored combos were refused, retrying the same default combos
                // would be refused too, and the app must still open.
                log::warn!("[clipflow] global hotkeys unavailable: {e}");
                match Registered::unbound(&settings.hotkey_save, &settings.hotkey_toggle) {
                    Ok(registered) => Arc::new(parking_lot::Mutex::new(registered)),
                    Err(e) => {
                        log::error!("[clipflow] no usable hotkey bindings: {e}");
                        // Even this cannot fail: `unbound` only parses, and the
                        // defaults are known-good.
                        Arc::new(parking_lot::Mutex::new(
                            Registered::unbound("Alt+C", "Alt+Shift+C")
                                .unwrap_or_else(|_| unreachable!("the defaults always parse")),
                        ))
                    }
                }
            }
        };

        cx.set_global(Globals {
            state: Arc::clone(&state_for_ui),
            hotkeys: Arc::clone(&hotkeys),
            tray: None,
        });

        // ------------------------------------------------------------------
        // Window
        // ------------------------------------------------------------------
        let options = WindowOptions {
            // Native Win32 decorations: the OS draws the titlebar, close /
            // minimize / maximize and the window menu, so there is no custom
            // chrome to keep in sync with the platform.
            titlebar: Some(TitlebarOptions {
                title: Some("ClipFlow".into()),
                ..Default::default()
            }),
            // The deck is a compact single column now that the clip library is
            // gone; settings scrolls within this same window.
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                None,
                size(px(760.0), px(400.0)),
                cx,
            ))),
            window_min_size: Some(size(px(640.0), px(320.0))),
            ..Default::default()
        };

        let (window_handle, deck) = match gpui_kit::open_window(options, cx, |window, cx| {
            cx.new(|cx| Deck::new(window, cx, Arc::clone(&state_for_ui), Arc::clone(&hotkeys)))
        }) {
            Ok(opened) => opened,
            Err(e) => {
                log::error!("[clipflow] could not open the deck: {e}");
                return;
            }
        };

        // `always_on_top` is applied once the HWND exists (next frame). The
        // render method captures the handle; this task waits for it.
        if state.settings_snapshot().always_on_top {
            let weak = deck.downgrade();
            cx.spawn(async move |cx| {
                // One frame is enough for `render` to capture the HWND.
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(120))
                    .await;
                weak.update(cx, |deck, _cx| deck.hwnd)
                    .map(|hwnd| {
                        cx.update(|cx| {
                            crate::platform::win32::defer(cx, move || {
                                crate::platform::win32::set_topmost(hwnd, true)
                            })
                        });
                    })
                    .ok();
            })
            .detach();
        }

        // Close means hide when `minimize_to_tray` is on; the deck's titlebar
        // button does the same, and `on_window_should_close` covers the OS path
        // (Alt+F4, taskbar close).
        let settings_for_close = Arc::clone(&state_for_ui);
        window_handle
            .update(cx, |_, window, cx| {
                let state = Arc::clone(&settings_for_close);
                window.on_window_should_close(cx, move |window, cx| {
                    if state.settings_snapshot().minimize_to_tray {
                        // Returning `false` vetoes the close; the window is
                        // hidden instead and the engine keeps running.
                        if let Ok(handle) = raw_window_handle::HasWindowHandle::window_handle(window)
                        {
                            if let raw_window_handle::RawWindowHandle::Win32(win32) =
                                handle.as_raw()
                            {
                                // `ShowWindow` dispatches messages, so it must
                                // not run while the App is borrowed inside this
                                // close handler.
                                let hwnd = win32.hwnd.get();
                                crate::platform::win32::defer(cx, move || {
                                    crate::platform::win32::set_visible(hwnd, false)
                                });
                            }
                        }
                        false
                    } else {
                        true
                    }
                });
            })
            .ok();

        // ------------------------------------------------------------------
        // Runtime loops
        // ------------------------------------------------------------------
        let weak = deck.downgrade();
        crate::runtime::stats::start(weak.clone(), Arc::clone(&state_for_ui), cx);
        crate::runtime::foreground::start(weak.clone(), Arc::clone(&state_for_ui), cx);
        crate::runtime::eco::start(weak.clone(), Arc::clone(&state_for_ui), cx);
        crate::runtime::tray::spawn_hotkeys(weak.clone(), Arc::clone(&hotkeys), cx);
        crate::runtime::tray::spawn_tray(weak.clone(), tray_actions, cx);

        // Park the icon (when one was created) in the global so it is not
        // dropped: dropping `TrayIcon` removes it from the notification area.
        cx.global_mut::<Globals>().tray = tray;

        // Shut the encoder down on *every* exit path, not just the Quit button:
        // the OS close button, Alt+F4 and the tray's Quit all end here. This
        // replaces Tauri's `RunEvent::ExitRequested` handler.
        let state_for_quit = Arc::clone(&state_for_ui);
        cx.on_app_quit(move |_cx| {
            state_for_quit.quit();
            async {}
        })
        .detach();

        // ------------------------------------------------------------------
        // Launch behaviour
        // ------------------------------------------------------------------
        launch_tasks(Arc::clone(&state_for_ui), deck.downgrade(), cx);
    });
}

/// Arm the buffer at launch and prune old clips, both off the UI thread.
fn launch_tasks(state: Arc<AppState>, deck: WeakEntity<Deck>, cx: &mut App) {
    let settings = state.settings_snapshot();

    if settings.autostart_buffer {
        let state_bg = Arc::clone(&state);
        let buffer = settings.buffer_seconds;
        let fps = settings.target_fps;
        cx.spawn(async move |cx| {
            let result = cx
                .background_spawn(async move { state_bg.start_replay_buffer(buffer, fps) })
                .await;
            match result {
                Ok(_) => log::info!("[clipflow] buffer armed at launch"),
                Err(e) => log::warn!("[clipflow] could not arm at launch: {e}"),
            }
            // Force a stats refresh so the pill flips to ARMED immediately.
            deck.update(cx, |deck, cx| {
                deck.on_stats(deck.state.engine_stats());
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    if settings.auto_cleanup_days > 0 {
        let state_bg = Arc::clone(&state);
        let days = settings.auto_cleanup_days;
        cx.spawn(async move |cx| {
            let deleted = cx
                .background_spawn(async move { state_bg.cleanup_old_clips(days) })
                .await;
            if let Ok(n) = deleted {
                if n > 0 {
                    log::info!("[clipflow] pruned {n} clips older than {days} days");
                }
            }
            let _ = cx;
        })
        .detach();
    }
}

fn init_logging() {
    // `env_logger` writes to stderr; in a release Windows build there is no
    // console, so the output is discarded harmlessly.
    let _ = env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("clipflow=info,warn"),
    )
    .try_init();
}

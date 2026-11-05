//! The control deck: one window, two views.
//!
//! ```text
//! [native Windows titlebar]
//! ┌─────────────────────────────────────────────────────────┐
//! │  CAPTURE                                                │
//! │   [ Arm ] [ Save ] [ Last 30 s ]   NVENC · H.264        │
//! │   telemetry rows                                        │
//! └─────────────────────────────────────────────────────────┘
//! footer: ARMED · 12.0 Mbps · 60 fps · 17.0 MB     Settings v1.4.2
//! ```
//!
//! Settings is a second full-window view rather than another section, so a long
//! settings page never buries the capture controls.
//!
//! # Why there is no event bus
//!
//! The Tauri build emitted `yascreenclipper://clip-saved`, and `App.tsx` had one
//! listener that drove the toast. There is no listener layer here:
//! [`Deck::on_clip_saved`] updates the status line directly.
//!
//! # Blocking discipline
//!
//! Every engine call goes through `cx.background_spawn` and returns to the view
//! through `WeakEntity::update`. Nothing in `render` touches the engine, disk or
//! the registry. The three exceptions are deliberate and cheap: `hwnd` capture
//! (a single `raw_window_handle` read), hotkey rebinding (an OS call that must
//! fail *before* anything is persisted), and `settings.save()`.

use std::sync::Arc;

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::InputEvent;
use gpui_kit::AppContext as _;
use gpui_kit::*;

use crate::media::recorder::EngineStats;
use crate::power::PowerState;
use crate::state::{AppState, SaveTrigger, SettingsPatch};
use crate::ui::capture_card as capture;
use crate::ui::format;
use crate::ui::theme::{Palette, Tone};

/// What a click, a tray item or a hotkey asks the deck to do. Routing all three
/// inputs through one enum keeps them from drifting apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Intent {
    Arm,
    Disarm,
    ToggleArm,
    /// SAVE from the deck.
    SaveReplay,
    /// "Save replay" from the tray menu.
    TraySaveReplay,
    SaveLast30,
    /// "Save last 30 s" from the tray menu (the deck button uses `SaveLast30`).
    TraySaveLast30,
    ShowWindow,
    OpenClipsFolder,
    Quit,
    Hotkey(HotkeyAction),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyAction {
    Save,
    Toggle,
}

/// Which full-window view is showing. Settings replaces the deck rather than
/// scrolling below it, so a long settings page never buries the clip library.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Deck,
    Settings,
}

/// The latest status message, shown as a single non-blocking line above the
/// footer. Native Windows toasts are raised separately by
/// `platform::toast` for the save path, which must be visible while the deck is
/// hidden.
#[derive(Debug, Clone)]
pub struct Status {
    pub tone: Tone,
    pub title: String,
    pub body: Option<String>,
}

/// The main window's root view.
pub struct Deck {
    pub state: Arc<AppState>,
    /// Hotkey registry, shared with the drain task so a rebind takes effect
    /// without restarting.
    pub hotkeys: Arc<parking_lot::Mutex<crate::platform::hotkeys::Registered>>,
    /// Native window handle, captured on the first frame. `0` until then.
    pub hwnd: isize,
    /// GPUI window handle, captured on the first frame. Needed to echo text
    /// back into the bitrate field (which requires a `&mut Window`).
    pub window_handle: Option<AnyWindowHandle>,

    pub view: View,
    pub stats: EngineStats,
    pub settings: crate::settings::Settings,
    pub power: Option<PowerState>,
    /// Last event, rendered as a one-line status strip. `None` renders as
    /// "Ready" so the strip keeps a stable height instead of shifting layout.
    pub status: Option<Status>,

    /// `None` = not recording a hotkey. `Some` = waiting for the next chord.
    pub recording_hotkey: Option<HotkeyAction>,
    pub busy: bool,
    /// Monotonic id for encoder restarts. A completion only clears `busy`
    /// and writes the status line when it belongs to the newest request.
    pub restart_seq: u64,

    /// Root focus handle. The deck owns focus so `on_key_down` can capture the
    /// next chord while recording a hotkey (a chord must never reach a widget
    /// shortcut mid-recording).
    pub focus: FocusHandle,

    /// Monitors enumerated at startup, for the Display row.
    pub monitors: Vec<crate::media::recorder::MonitorInfo>,

    /// About row: last update-check outcome, shown next to the version.
    pub update_status: String,
    pub update_error: bool,

    // --- settings controls ------------------------------------------------
    // Slider and input state must outlive a frame, so the entities are owned
    // by the deck and created once. Slider `Release` events persist, which
    // keeps `settings.save()` off the drag path; the bitrate field persists on
    // `Change` events that parse to an in-range value (see `parse_bitrate`).
    pub bitrate_input: Entity<gpui_kit::component::input::InputState>,
    pub game_volume_slider: Entity<gpui_kit::component::slider::SliderState>,
    pub mic_volume_slider: Entity<gpui_kit::component::slider::SliderState>,
}

impl Deck {
    pub fn new(
        window: &mut Window,
        cx: &mut Context<Self>,
        state: Arc<AppState>,
        hotkeys: Arc<parking_lot::Mutex<crate::platform::hotkeys::Registered>>,
    ) -> Self {
        let settings = state.settings_snapshot();
        let stats = state.engine_stats();

        let focus = cx.focus_handle();

        let bitrate_input = cx.new(|cx| {
            crate::ui::settings::bitrate_input_state(window, cx, settings.bitrate_kbps)
        });
        let game_volume_slider = cx.new(|_| {
            crate::ui::settings::slider_state(
                crate::ui::settings::VOLUME_MIN,
                crate::ui::settings::VOLUME_MAX,
                crate::ui::settings::VOLUME_STEP,
                settings.game_volume,
            )
        });
        let mic_volume_slider = cx.new(|_| {
            crate::ui::settings::slider_state(
                crate::ui::settings::VOLUME_MIN,
                crate::ui::settings::VOLUME_MAX,
                crate::ui::settings::VOLUME_STEP,
                settings.mic_volume,
            )
        });

        let deck = Self {
            state: Arc::clone(&state),
            hotkeys,
            hwnd: 0,
            window_handle: None,
            view: View::Deck,
            stats,
            settings,
            power: None,
            status: None,
            recording_hotkey: None,
            busy: false,
            restart_seq: 0,
            focus,
            monitors: state.monitors(),
            update_status: "Not checked yet".to_string(),
            update_error: false,
            bitrate_input,
            game_volume_slider,
            mic_volume_slider,
        };
        // Bitrate commits: `Change` persists in-range numbers as they are
        // typed (intermediate keystrokes never touch `settings.json`), and
        // `Blur` reverts rejected text to the applied value.
        cx.subscribe(
            &deck.bitrate_input.clone(),
            |deck, state, event: &InputEvent, cx| {
                match event {
                    InputEvent::Change => {
                        let text = state.read(cx).value();
                        let Some(value) = crate::ui::settings::parse_bitrate(&text) else {
                            return;
                        };
                        if value == deck.settings.bitrate_kbps {
                            return;
                        }
                        deck.apply_patch(
                            SettingsPatch {
                                bitrate_kbps: Some(value),
                                ..Default::default()
                            },
                            cx,
                        );
                    }
                    InputEvent::Blur => {
                        let text = state.read(cx).value();
                        if text.trim() != deck.settings.bitrate_kbps.to_string().as_str() {
                            deck.revert_bitrate_text(cx);
                            deck.status_warn(
                                "Bitrate unchanged",
                                "Enter a value from 3000 to 30000 kbps.",
                            );
                            cx.notify();
                        }
                    }
                    _ => {}
                }
            },
        )
        .detach();
        // Slider releases persist; `Change` only repaints.
        cx.subscribe(
            &deck.game_volume_slider.clone(),
            |deck, _state, event: &gpui_kit::component::slider::SliderEvent, cx| {
                if let Some(value) = released_value(event) {
                    deck.apply_patch(
                        SettingsPatch {
                            game_volume: Some(value),
                            ..Default::default()
                        },
                        cx,
                    );
                }
            },
        )
        .detach();
        cx.subscribe(
            &deck.mic_volume_slider.clone(),
            |deck, _state, event: &gpui_kit::component::slider::SliderEvent, cx| {
                if let Some(value) = released_value(event) {
                    deck.apply_patch(
                        SettingsPatch {
                            mic_volume: Some(value),
                            ..Default::default()
                        },
                        cx,
                    );
                }
            },
        )
        .detach();

        deck
    }

    // -----------------------------------------------------------------------
    // Snapshots pushed in by runtime loops
    // -----------------------------------------------------------------------

    pub fn on_stats(&mut self, stats: EngineStats) {
        self.stats = stats;
    }

    pub fn on_power_snapshot(&mut self, power: PowerState) {
        self.power = Some(power);
    }

    /// A save finished. Records it on the status line.
    pub fn on_clip_saved(&mut self, outcome: crate::state::SaveOutcome, body: String) {
        self.set_status(Status {
            tone: Tone::Ok,
            title: format!("Saved · {:.0} ms", outcome.flush_ms),
            body: Some(body),
        });
        // The engine just flushed, so its own state moved without waiting for
        // the 4 Hz tick.
        self.stats = self.state.engine_stats();
    }

    pub fn status_info(&mut self, title: &str, body: &str) {
        self.set_status(Status {
            tone: Tone::Info,
            title: title.to_string(),
            body: Some(body.to_string()),
        });
    }

    pub fn status_warn(&mut self, title: &str, body: &str) {
        self.set_status(Status {
            tone: Tone::Warn,
            title: title.to_string(),
            body: Some(body.to_string()),
        });
    }

    pub fn status_error(&mut self, title: &str, body: &str) {
        self.set_status(Status {
            tone: Tone::Err,
            title: title.to_string(),
            body: Some(body.to_string()),
        });
    }

    /// Replaces the status line. It persists until the next event — nothing
    /// auto-dismisses or overlays the view.
    fn set_status(&mut self, status: Status) {
        self.status = Some(status);
    }

    // -----------------------------------------------------------------------
    // Intents
    // -----------------------------------------------------------------------

    /// Single entry point for buttons, tray items and hotkeys.
    pub fn handle_intent(&mut self, intent: Intent, cx: &mut Context<Self>) {
        // A flush or an engine start is in flight: rejecting here is what keeps
        // a mashed Alt+C from queueing four flushes. `ShowWindow` and `Quit`
        // are exempt because they must always work.
        let always = matches!(intent, Intent::ShowWindow | Intent::Quit);
        if self.busy && !always {
            self.status_warn("One moment", "A save or engine change is already running.");
            cx.notify();
            return;
        }

        match intent {
            Intent::Arm => self.arm(cx),
            Intent::Disarm => self.disarm(cx),
            Intent::ToggleArm | Intent::Hotkey(HotkeyAction::Toggle) => {
                if self.state.is_armed() {
                    self.disarm(cx);
                } else {
                    self.arm(cx);
                }
            }
            Intent::SaveReplay => self.save(None, SaveTrigger::Ui, cx, false),
            Intent::Hotkey(HotkeyAction::Save) => self.save(None, SaveTrigger::Hotkey, cx, true),
            Intent::TraySaveReplay => self.save(None, SaveTrigger::Tray, cx, true),
            Intent::SaveLast30 => self.save(Some(30.0), SaveTrigger::Ui, cx, false),
            Intent::TraySaveLast30 => self.save(Some(30.0), SaveTrigger::Tray, cx, false),
            Intent::ShowWindow => {
                // A tray click is an explicit "show me", so raising is always
                // safe: the user just clicked our icon.
                let hwnd = self.hwnd;
                crate::platform::win32::defer(cx, move || {
                    crate::platform::win32::set_visible(hwnd, true);
                    crate::platform::win32::activate(hwnd);
                });
            }
            Intent::OpenClipsFolder => self.open_clips_folder(cx),
            Intent::Quit => self.quit(cx),
        }
    }

    pub fn arm(&mut self, cx: &mut Context<Self>) {
        self.busy = true;
        cx.notify();

        let state = Arc::clone(&self.state);
        let buffer = self.settings.buffer_seconds;
        let fps = self.settings.target_fps;
        cx.spawn(async move |deck, cx| {
            let result = cx
                .background_spawn(async move { state.start_replay_buffer(buffer, fps) })
                .await;
            deck.update(cx, |deck, cx| {
                deck.busy = false;
                match result {
                    Ok(stats) => {
                        // Read the engine directly for the ARM/DISARM
                        // transition: waiting for the next 250 ms telemetry tick
                        // makes the button feel dead.
                        deck.stats = stats;
                        deck.status_info("Buffer armed", "Recording the rolling window.");
                    }
                    Err(e) => deck.status_error("Could not arm the buffer", &e),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub fn disarm(&mut self, cx: &mut Context<Self>) {
        self.busy = true;
        cx.notify();

        let state = Arc::clone(&self.state);
        cx.spawn(async move |deck, cx| {
            let stats = cx
                .background_spawn(async move { state.stop_replay_buffer() })
                .await;
            deck.update(cx, |deck, cx| {
                deck.busy = false;
                deck.stats = stats;
                deck.status_info("Buffer disarmed", "Nothing is being recorded.");
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Flushes the ring.
    ///
    /// `raise_if_safe` is the focus contract: the window is only activated when
    /// the foreground is positively our own deck or the desktop, because raising
    /// it mid-match minimises exclusive-fullscreen games. The native toast always
    /// fires — see [`crate::state::safe_to_raise`] for why this differs from the
    /// Tauri build.
    fn save(
        &mut self,
        max_seconds: Option<f32>,
        trigger: SaveTrigger,
        cx: &mut Context<Self>,
        raise_if_safe: bool,
    ) {
        self.busy = true;
        cx.notify();

        let state = Arc::clone(&self.state);
        cx.spawn(async move |deck, cx| {
            // Foreground sampling happens off-thread: `GetWindowTextW` can hang
            // on a stuck foreground app.
            let result = cx
                .background_spawn(async move {
                    let outcome = state.save_instant_replay(max_seconds, trigger, None);
                    (outcome, crate::state::safe_to_raise())
                })
                .await;

            deck.update(cx, |deck, cx| {
                deck.busy = false;
                match result {
                    (Ok(outcome), safe_to_raise) => {
                        // Native notification: it is the only feedback while
                        // the deck is hidden, and it never steals focus. The
                        // in-app status line records the same result.
                        crate::platform::notify::clip_saved(
                            &outcome.clip.file_name,
                            outcome.clip.duration_seconds,
                            trigger.as_str(),
                        );
                        let body = format!(
                            "{} · {} · {}",
                            outcome.clip.file_name,
                            format::duration(outcome.clip.duration_seconds),
                            format::bytes(outcome.clip.size_bytes)
                        );
                        deck.on_clip_saved(outcome, body);
                        if raise_if_safe && safe_to_raise {
                            let hwnd = deck.hwnd;
                            crate::platform::win32::defer(cx, move || {
                                crate::platform::win32::activate(hwnd)
                            });
                        }
                    }
                    (Err(e), _) => {
                        if e.contains("already in progress") {
                            deck.status_warn("Save already running", &e);
                        } else if e.starts_with("Privacy mode:") {
                            // The exact refusal is actionable, so surface it
                            // verbatim instead of a generic failure.
                            crate::platform::notify::save_blocked(&e);
                            deck.status_error("Nothing saved", &e);
                        } else {
                            deck.status_error("Save failed", &e);
                        }
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Opens the clips folder in Explorer. Reached from the tray menu.
    pub fn open_clips_folder(&mut self, cx: &mut Context<Self>) {
        let dir = self.state.output_dir();
        let _ = std::fs::create_dir_all(&dir);
        // `ShellExecuteW` pumps messages, so it must not run under a gpui
        // borrow — see `platform::win32::defer`.
        cx.spawn(async move |deck, cx| {
            if let Err(e) = crate::platform::win32::shell_open(&dir, false) {
                deck.update(cx, |deck, cx| {
                    deck.status_error("Could not open the folder", &e);
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    // -----------------------------------------------------------------------
    // Settings
    // -----------------------------------------------------------------------

    /// Applies a patch. Hotkeys are registered with the OS first, so a conflict
    /// aborts the whole patch and leaves the old binding live and the file
    /// untouched.
    pub fn apply_patch(&mut self, patch: SettingsPatch, cx: &mut Context<Self>) {
        let hotkeys = Arc::clone(&self.hotkeys);
        let current_save = self.settings.hotkey_save.clone();
        let mut rebind = |old: &str, new: &str| {
            // `update_settings` does not say which action it is rebinding, so the
            // two stored values identify it.
            let action = if old == current_save {
                HotkeyAction::Save
            } else {
                HotkeyAction::Toggle
            };
            hotkeys.lock().rebind(action, new)
        };

        let hwnd = self.hwnd;
        // `SetWindowPos` must not run while the App is borrowed, so the value is
        // captured here and applied after the update.
        let mut topmost_request: Option<bool> = None;
        let mut topmost = |on: bool| topmost_request = Some(on);

        // Capture before `patch` is moved into `update_settings`.
        let needs_encoder_restart = patch.affects_encoder();

        match self.state.update_settings(patch, &mut rebind, &mut topmost) {
            Ok(settings) => {
                self.settings = settings;
                if needs_encoder_restart {
                    if self.state.is_armed() {
                        // fps/bitrate/codec/monitor/audio are baked into the
                        // engine at start, so rebuild it rather than letting the
                        // UI claim they were applied.
                        self.restart_encoder(cx);
                    } else {
                        self.status_info(
                            "Settings saved",
                            "Applies the next time the buffer is armed.",
                        );
                    }
                } else {
                    self.status_info("Settings saved", "Applied immediately.");
                }
            }
            Err(e) => {
                // A rejected patch leaves the file and the live hotkey alone;
                // the view must not pretend otherwise.
                self.status_error("Settings not saved", &e);
            }
        }

        if let Some(on) = topmost_request {
            crate::platform::win32::defer(cx, move || {
                crate::platform::win32::set_topmost(hwnd, on)
            });
        }
        cx.notify();
    }

    /// Stops and restarts the capture engine so encoder settings (fps, bitrate,
    /// codec, rate control, GOP, monitor, audio source) become live. Blocking,
    /// so it runs on the background executor.
    fn restart_encoder(&mut self, cx: &mut Context<Self>) {
        self.busy = true;
        self.restart_seq = self.restart_seq.wrapping_add(1);
        let seq = self.restart_seq;
        self.status_info("Settings saved", "Restarting the encoder…");
        cx.notify();

        let state = Arc::clone(&self.state);
        cx.spawn(async move |deck, cx| {
            let result = cx
                .background_spawn(async move { state.restart_replay_buffer() })
                .await;
            deck.update(cx, |deck, cx| {
                // A newer restart queued behind this one owns the UI state.
                if deck.restart_seq != seq {
                    return;
                }
                deck.busy = false;
                match result {
                    Ok(stats) => {
                        deck.stats = stats;
                        deck.status_info(
                            "Encoder restarted",
                            "FPS, bitrate, codec and GOP are live now.",
                        );
                    }
                    Err(e) => deck.status_error("Could not restart the encoder", &e),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Steps the bitrate by `delta` kbps, clamping to the numeric range. The
    /// field text is echoed back because the value may have been clamped.
    pub fn step_bitrate(&mut self, delta: i32, window: &mut Window, cx: &mut Context<Self>) {
        use crate::ui::settings::{BITRATE_MAX, BITRATE_MIN};
        let next = (self.settings.bitrate_kbps as i32 + delta)
            .clamp(BITRATE_MIN as i32, BITRATE_MAX as i32) as u32;
        self.apply_patch(
            SettingsPatch {
                bitrate_kbps: Some(next),
                ..Default::default()
            },
            cx,
        );
        let text = self.settings.bitrate_kbps.to_string();
        self.bitrate_input.update(cx, |state, cx| {
            state.set_value(text, window, cx);
        });
    }

    /// Reverts the bitrate field to the applied setting, used when blur leaves
    /// rejected text behind. Goes through the stored window handle because
    /// `set_value` needs a `&mut Window`; silently skips if it is gone.
    pub fn revert_bitrate_text(&mut self, cx: &mut Context<Self>) {
        let Some(handle) = self.window_handle else {
            return;
        };
        let canonical = self.settings.bitrate_kbps.to_string();
        let input = self.bitrate_input.clone();
        let _ = handle.update(cx, |_, window, cx| {
            input.update(cx, |state, cx| {
                state.set_value(canonical, window, cx);
            });
        });
    }

    /// Opens the folder picker and applies the choice.
    pub fn choose_output_folder(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |deck, cx| {
            let Some(path) = crate::platform::dialog::pick_folder(cx).await else {
                return; // cancelled or unavailable
            };
            deck.update(cx, |deck, cx| {
                deck.apply_patch(
                    SettingsPatch {
                        output_dir: Some(path.to_string_lossy().to_string()),
                        ..Default::default()
                    },
                    cx,
                );
            })
            .ok();
        })
        .detach();
    }

    /// Probes the Releases feed off-thread and reports the outcome in the About
    /// row. Cosmetic: a failed check never blocks anything else.
    pub fn check_for_updates(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        self.update_status = "Checking…".to_string();
        self.update_error = false;
        cx.notify();

        let current = env!("CARGO_PKG_VERSION").to_string();
        cx.spawn(async move |deck, cx| {
            let result = cx
                .background_spawn(async move { crate::platform::update::check(&current) })
                .await;
            deck.update(cx, |deck, cx| {
                match result {
                    Ok(crate::platform::update::UpdateCheck::UpToDate) => {
                        deck.update_status = "Up to date".to_string();
                        deck.update_error = false;
                    }
                    Ok(crate::platform::update::UpdateCheck::Available { version }) => {
                        deck.update_status = format!("{version} available");
                        deck.update_error = false;
                        deck.status_info(
                            "Update available",
                            &format!("YAScreenClipper {version} is out — see the Releases page."),
                        );
                    }
                    Err(e) => {
                        deck.update_status = format!("Check failed: {e}");
                        deck.update_error = true;
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub fn simulate_device_loss(&mut self, cx: &mut Context<Self>) {
        self.state.engine.simulate_device_loss();
        self.status_warn("Device loss simulated", "The engine should recover on its own.");
        cx.notify();
    }

    pub fn quit(&mut self, cx: &mut Context<Self>) {
        // Stop the encoder before the process dies: the engine holds GPU
        // resources that must be released in order.
        self.state.quit();
        cx.quit();
    }

    // -----------------------------------------------------------------------
    // Render
    // -----------------------------------------------------------------------

    fn armed(&self) -> bool {
        capture::is_armed(self.stats.state)
    }
}

/// The integer a slider released at. `Release` only (not `Change`), so dragging
/// does not write `settings.json` on every frame.
pub(crate) fn released_value(event: &gpui_kit::component::slider::SliderEvent) -> Option<u32> {
    use gpui_kit::component::slider::{SliderEvent, SliderValue};
    let SliderEvent::Release(value) = event else {
        return None;
    };
    let raw = match value {
        SliderValue::Single(v) => *v,
        SliderValue::Range(start, _) => *start,
    };
    Some(raw.round().max(0.0) as u32)
}

impl Focusable for Deck {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for Deck {
    // `window` is only read on the Windows path (HWND capture); the attribute
    // keeps the stub build warning-free without a second `cfg` fork of render.
    #[cfg_attr(any(not(windows), feature = "headless-sim"), allow(unused_variables))]
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Capture the HWND on the first frame. GPUI has no `set_always_on_top`,
        // no hide/show and no `set_position`, so `platform::win32` needs the raw
        // handle; it is read once and stored as an `isize` so no typed handle
        // ever crosses into the `windows 0.58` engine.
        if self.window_handle.is_none() {
            self.window_handle = Some(window.window_handle());
        }
        if self.hwnd == 0 {
            #[cfg(all(windows, not(feature = "headless-sim")))]
            {
                use raw_window_handle::HasWindowHandle as _;
                if let Ok(handle) = window.window_handle() {
                    if let raw_window_handle::RawWindowHandle::Win32(win32) = handle.as_raw() {
                        self.hwnd = win32.hwnd.get();
                    }
                }
            }
            #[cfg(any(not(windows), feature = "headless-sim"))]
            {
                // Sentinel: there is no HWND to find, and retrying every frame
                // would just burn cycles.
                self.hwnd = 1;
            }
        }

        let palette = Palette::new();
        let armed = self.armed();
        // Stock widgets resolve their colors through the component theme that
        // `ui::theme::init` installed; nothing to do here.

        let mut deck_body = div()
            .id("deck-body")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .gap_4()
            .p_4()
            .overflow_y_scroll();
        deck_body = match self.view {
            View::Deck => deck_body.child(self.render_capture_section(palette, cx)),
            View::Settings => deck_body.child(self.render_settings(palette, cx)),
        };

        div()
            .id("deck-root")
            .flex()
            .flex_col()
            .size_full()
            .bg(palette.background)
            .text_color(palette.text)
            .font_family(crate::ui::theme::SANS)
            .track_focus(&self.focus)
            .focusable()
            .on_key_down(cx.listener(|deck, event: &KeyDownEvent, _window, cx| {
                deck.record_hotkey_key(event, cx);
            }))
            .child(deck_body)
            .child(self.render_status(palette))
            .child(self.render_footer(palette, armed, cx))
    }
}

// ---------------------------------------------------------------------------
// Status line and footer
// ---------------------------------------------------------------------------

impl Deck {
    /// One-line status strip above the footer. It never overlays content and
    /// never auto-dismisses: the latest event stays visible until the next one
    /// replaces it, so settings confirmations and save results read as panel
    /// state rather than popups.
    pub(crate) fn render_status(&self, palette: Palette) -> impl IntoElement {
        let (accent, title, body) = match &self.status {
            Some(s) => (palette.tone(s.tone), s.title.clone(), s.body.clone()),
            None => (
                palette.muted,
                "Ready".to_string(),
                Option::<String>::None,
            ),
        };
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .px_4()
            .py_1()
            .child(div().size(px(6.0)).rounded_full().bg(accent))
            .child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(accent)
                    .child(title),
            )
            .children(body.map(|text| {
                div()
                    .flex_1()
                    .min_w_0()
                    .font_family(crate::ui::theme::MONO)
                    .text_xs()
                    .text_color(palette.muted)
                    .truncate()
                    .child(text)
            }))
    }

    pub(crate) fn render_footer(
        &self,
        palette: Palette,
        armed: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let stats = &self.stats;
        let dot = if armed { palette.ok } else { palette.muted };
        let summary = format!(
            "{} · {} · {} fps · {}",
            if armed { "ARMED" } else { "IDLE" },
            crate::ui::format::bitrate_mbps(stats.bitrate_kbps),
            stats.target_fps,
            crate::ui::format::bytes(stats.process_rss_bytes),
        );

        div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .px_4()
            .py_1()
            .bg(palette.chrome)
            .border_t_1()
            .border_color(palette.line)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(div().size(px(6.0)).rounded_full().bg(dot))
                    .child(
                        div()
                            .font_family(crate::ui::theme::MONO)
                            .text_xs()
                            .text_color(palette.muted)
                            .child(summary),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_3()
                    .child(
                        Button::new("toggle-settings")
                            .ghost()
                            .compact()
                            .label(if self.view == View::Settings {
                                "Back to deck"
                            } else {
                                "Settings"
                            })
                            .on_click(cx.listener(|deck, _ev, _window, cx| {
                                deck.view = match deck.view {
                                    View::Deck => View::Settings,
                                    View::Settings => View::Deck,
                                };
                                cx.notify();
                            })),
                    )
                    .child(
                        div()
                            .font_family(crate::ui::theme::MONO)
                            .text_xs()
                            .text_color(palette.muted)
                            .child(format!("v{}", env!("CARGO_PKG_VERSION"))),
                    ),
            )
    }
}



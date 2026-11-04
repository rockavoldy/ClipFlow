//! The control deck: one window, two views.
//!
//! ```text
//! ┌ titlebar ─────────────────── [ARMED] Settings Hide Quit ┐
//! │ DECK                                                    │
//! │  CAPTURE                                                │
//! │   [ ARM ] [ SAVE ] [ LAST 30 s ]   NVENC · H.264        │
//! │   telemetry rows                                        │
//! │  LIBRARY                                                │
//! │   search   All cs2 elden  Favourites  With audio        │
//! │   Newest Oldest Largest Smallest Longest Shortest       │
//! │   [ clip rows… ]                                        │
//! │ footer: ARMED · 12.0 Mbps · 60 fps · 17.0 MB · v1.4.2   │
//! └─────────────────────────────────────────────────────────┘
//!
//! Settings is a second full-window view (gear button) rather than another
//! section: a long settings page would otherwise bury the clip library.
//! ```
//!
//! # Why there is no event bus
//!
//! The Tauri build emitted `clipflow://clip-saved`, and `App.tsx` had one
//! listener that drove *both* the gallery insert and the toast. There is no
//! listener layer here: [`Deck::on_clip_saved`] mutates the clip list and raises
//! the toast in one call, so the two can never disagree.
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
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::component::TitleBar;
use gpui_kit::AppContext as _;
use gpui_kit::*;

use crate::media::library::ClipMetadata;
use crate::media::recorder::EngineStats;
use crate::policy::gallery::{self, GalleryState, SortKey};
use crate::policy::selection::Selection;
use crate::power::PowerState;
use crate::state::{AppState, SaveTrigger, SettingsPatch};
use crate::ui::capture_card as capture;
use crate::ui::format;
use crate::ui::thumbs::ThumbnailCache;
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

/// An in-app toast. Native Windows toasts are raised separately by
/// `platform::toast` for the save path, which must be visible while the deck is
/// hidden.
#[derive(Debug, Clone)]
pub struct Toast {
    pub tone: Tone,
    pub title: String,
    pub body: Option<String>,
    /// When it was raised. Used to expire the stack without a timer per toast.
    pub raised_at: std::time::Instant,
}

/// How many toasts are kept. Three fills the visible stack.
const MAX_TOASTS: usize = 3;
/// How long a toast stays up. Matches the old deck's 4 s auto-dismiss.
const TOAST_TTL: std::time::Duration = std::time::Duration::from_secs(4);

/// The main window's root view.
pub struct Deck {
    pub state: Arc<AppState>,
    /// Hotkey registry, shared with the drain task so a rebind takes effect
    /// without restarting.
    pub hotkeys: Arc<parking_lot::Mutex<crate::platform::hotkeys::Registered>>,
    /// Native window handle, captured on the first frame. `0` until then.
    pub hwnd: isize,

    pub view: View,
    pub stats: EngineStats,
    pub settings: crate::settings::Settings,
    pub power: Option<PowerState>,
    pub toasts: Vec<Toast>,

    pub gallery_state: GalleryState,
    pub selection: Selection,
    pub clips: Vec<ClipMetadata>,
    pub loading_clips: bool,
    /// Set when the search query changes; flushed by the 1 s tick so typing
    /// does not write `gallery_state.json` on every keystroke.
    pub gallery_dirty: bool,
    /// `None` = not recording a hotkey. `Some` = waiting for the next chord.
    pub recording_hotkey: Option<HotkeyAction>,
    pub busy: bool,

    pub search: Entity<InputState>,

    /// Root focus handle. The deck owns focus so `on_key_down` can capture the
    /// next chord while recording a hotkey (a chord must never reach a widget
    /// shortcut mid-recording).
    pub focus: FocusHandle,

    /// Monitors enumerated at startup, for the Display row.
    pub monitors: Vec<crate::media::recorder::MonitorInfo>,

    /// About row: last update-check outcome, shown next to the version.
    pub update_status: String,
    pub update_error: bool,

    // --- settings sliders ---------------------------------------------------
    // Slider state must outlive a frame, so the entities are owned by the deck
    // and created once. Only `Release` events persist, which keeps
    // `settings.save()` off the drag path.
    pub bitrate_slider: Entity<gpui_kit::component::slider::SliderState>,
    pub game_volume_slider: Entity<gpui_kit::component::slider::SliderState>,
    pub mic_volume_slider: Entity<gpui_kit::component::slider::SliderState>,

    // --- library view state -------------------------------------------------
    /// The filtered + sorted clips the grid paints. Kept separate from `clips`
    /// so a filter change does not disturb the source list.
    pub visible: Vec<ClipMetadata>,
    pub library_scroll: gpui_kit::UniformListScrollHandle,
    pub thumbs: ThumbnailCache,
    /// Decoding is started lazily per visible row; this records which paths are
    /// already in flight so a repaint cannot queue the same decode twice.
    pub thumbs_pending: std::collections::HashSet<String>,
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
        let gallery_state = load_gallery_state();

        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search clips"));
        let focus = cx.focus_handle();

        let bitrate_slider = cx.new(|_| {
            crate::ui::settings::slider_state(
                crate::ui::settings::BITRATE_MIN,
                crate::ui::settings::BITRATE_MAX,
                crate::ui::settings::BITRATE_STEP,
                settings.bitrate_kbps,
            )
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

        // Typing re-filters the library live. This replaces the controlled
        // `<input>` in `App.tsx`; the state entity owns the text, the deck owns
        // the filter.
        cx.subscribe(&search, |deck, input, event, cx| {
            if matches!(event, InputEvent::Change) {
                deck.gallery_state.query = input.read(cx).value().to_string();
                deck.refresh_visible_dirty();
                // Flushed by the 1 s tick instead of writing per keystroke.
                deck.gallery_dirty = true;
                cx.notify();
            }
        })
        .detach();

        let mut deck = Self {
            state: Arc::clone(&state),
            hotkeys,
            hwnd: 0,
            view: View::Deck,
            stats,
            settings,
            power: None,
            toasts: Vec::new(),
            gallery_state,
            selection: Selection::default(),
            clips: Vec::new(),
            loading_clips: true,
            gallery_dirty: false,
            recording_hotkey: None,
            busy: false,
            search,
            focus,
            monitors: state.monitors(),
            update_status: "Not checked yet".to_string(),
            update_error: false,
            bitrate_slider,
            game_volume_slider,
            mic_volume_slider,
            visible: Vec::new(),
            library_scroll: gpui_kit::UniformListScrollHandle::new(),
            thumbs: ThumbnailCache::new(),
            thumbs_pending: std::collections::HashSet::new(),
        };
        // Slider releases persist; `Change` only repaints.
        cx.subscribe(
            &deck.bitrate_slider.clone(),
            |deck, _state, event: &gpui_kit::component::slider::SliderEvent, cx| {
                if let Some(value) = released_value(event) {
                    deck.apply_patch(
                        SettingsPatch {
                            bitrate_kbps: Some(value),
                            ..Default::default()
                        },
                        cx,
                    );
                }
            },
        )
        .detach();
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

        deck.load_clips(cx);
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

    /// A save finished. Replaces the `EVT_CLIP_SAVED` listener: it inserts the
    /// clip *and* raises the confirmation in one place, so the library and the
    /// toast can never disagree.
    pub fn on_clip_saved(&mut self, outcome: crate::state::SaveOutcome, body: String) {
        // Newest first, so the card appears at the top of the default sort.
        self.clips.insert(0, outcome.clip);
        self.refresh_visible_dirty();

        self.push_toast(Toast {
            tone: Tone::Ok,
            title: format!("Saved · {:.0} ms", outcome.flush_ms),
            body: Some(body),
            raised_at: std::time::Instant::now(),
        });
        // The engine just flushed, so its own state moved without waiting for
        // the 4 Hz tick.
        self.stats = self.state.engine_stats();
    }

    pub fn toast_info(&mut self, title: &str, body: &str) {
        self.push_toast(Toast {
            tone: Tone::Info,
            title: title.to_string(),
            body: Some(body.to_string()),
            raised_at: std::time::Instant::now(),
        });
    }

    pub fn toast_warn(&mut self, title: &str, body: &str) {
        self.push_toast(Toast {
            tone: Tone::Warn,
            title: title.to_string(),
            body: Some(body.to_string()),
            raised_at: std::time::Instant::now(),
        });
    }

    pub fn toast_error(&mut self, title: &str, body: &str) {
        self.push_toast(Toast {
            tone: Tone::Err,
            title: title.to_string(),
            body: Some(body.to_string()),
            raised_at: std::time::Instant::now(),
        });
    }

    fn push_toast(&mut self, toast: Toast) {
        self.toasts.push(toast);
        if self.toasts.len() > MAX_TOASTS {
            self.toasts.remove(0);
        }
    }

    /// Drops toasts older than [`TOAST_TTL`]. Driven by a 1 s task in `app`,
    /// which is far simpler than a timer per toast.
    /// One-second housekeeping: expire toasts and flush a dirty gallery state.
    pub fn tick(&mut self, cx: &mut Context<Self>) {
        self.expire_toasts(cx);
        if self.gallery_dirty {
            self.persist_gallery_state();
        }
    }

    pub fn expire_toasts(&mut self, cx: &mut Context<Self>) {
        let before = self.toasts.len();
        self.toasts
            .retain(|t| t.raised_at.elapsed() < TOAST_TTL);
        if self.toasts.len() != before {
            cx.notify();
        }
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
            self.toast_warn("One moment", "A save or engine change is already running.");
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
                crate::platform::win32::set_visible(self.hwnd, true);
                crate::platform::win32::activate(self.hwnd);
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
                        deck.toast_info("Buffer armed", "Recording the rolling window.");
                    }
                    Err(e) => deck.toast_error("Could not arm the buffer", &e),
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
                deck.toast_info("Buffer disarmed", "Nothing is being recorded.");
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
                        // Always toast: it is the only feedback while the deck
                        // is hidden, and it never steals focus.
                        crate::platform::notify::clip_saved(
                            cx,
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
                            crate::platform::win32::activate(deck.hwnd);
                        }
                    }
                    (Err(e), _) => {
                        if e.contains("already in progress") {
                            deck.toast_warn("Save already running", &e);
                        } else if e.starts_with("Privacy mode:") {
                            // The exact refusal is actionable, so surface it
                            // verbatim instead of a generic failure.
                            crate::platform::notify::save_blocked(cx, &e);
                            deck.toast_error("Nothing saved", &e);
                        } else {
                            deck.toast_error("Save failed", &e);
                        }
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    // -----------------------------------------------------------------------
    // Library
    // -----------------------------------------------------------------------

    /// Scans the clips folder and folds in favourites/tags.
    ///
    /// Deliberately scanned with `with_thumbnails = false`: the Tauri build
    /// asked `scan_directory` for a base64 data URL per clip so the webview
    /// could inline it, which meant decoding *every* clip's first frame on
    /// every scan. Here thumbnails are decoded lazily per visible row and
    /// cached (see `ui/thumbs.rs`), which keeps a scan of thousands of clips to
    /// a directory walk plus the probe cache.
    pub fn load_clips(&mut self, cx: &mut Context<Self>) {
        self.loading_clips = true;
        let state = Arc::clone(&self.state);
        cx.spawn(async move |deck, cx| {
            let clips = cx
                .background_spawn(async move { state.recorded_clips(false) })
                .await;
            deck.update(cx, |deck, cx| {
                deck.loading_clips = false;
                match clips {
                    Ok(clips) => {
                        deck.clips = clips;
                        deck.refresh_visible_dirty();
                    }
                    Err(e) => deck.toast_error("Could not read the library", &e),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Recomputes the filtered/sorted view. The visible list is what the grid
    /// paints and what shift-selection ranges over, so both stay consistent.
    pub fn refresh_visible(&mut self, cx: &mut Context<Self>) {
        self.refresh_visible_dirty();
        cx.notify();
    }

    /// Persists the gallery view state. Called on filter/sort changes and from
    /// the 1 s tick for the search query (which would otherwise write a file per
    /// keystroke).
    pub fn persist_gallery_state(&mut self) {
        save_gallery_state(&self.gallery_state);
        self.gallery_dirty = false;
    }

    fn refresh_visible_dirty(&mut self) {
        self.visible = gallery::filter_and_sort(&self.clips, &self.gallery_state);
        // Prune selection entries that the new filter hid, so Delete can never
        // remove something the user cannot see.
        let live: Vec<&str> = self.visible.iter().map(|c| c.path.as_str()).collect();
        self.selection
            .selected
            .retain(|p| live.contains(&p.as_str()));
    }

    pub fn set_game_filter(&mut self, game: String, cx: &mut Context<Self>) {
        self.gallery_state.game_filter = game;
        self.selection.clear();
        self.refresh_visible_dirty();
        self.persist_gallery_state();
        cx.notify();
    }

    pub fn set_sort(&mut self, sort: SortKey, cx: &mut Context<Self>) {
        self.gallery_state.sort_key = sort;
        self.refresh_visible_dirty();
        self.persist_gallery_state();
        cx.notify();
    }

    pub fn toggle_fav_filter(&mut self, cx: &mut Context<Self>) {
        self.gallery_state.fav_only = !self.gallery_state.fav_only;
        self.selection.clear();
        self.refresh_visible_dirty();
        self.persist_gallery_state();
        cx.notify();
    }

    pub fn toggle_audio_filter(&mut self, cx: &mut Context<Self>) {
        self.gallery_state.audio_only = !self.gallery_state.audio_only;
        self.selection.clear();
        self.refresh_visible_dirty();
        self.persist_gallery_state();
        cx.notify();
    }

    pub fn select_all_visible(&mut self, cx: &mut Context<Self>) {
        let order = self.visible_order();
        self.selection.select_all(&order);
        cx.notify();
    }

    pub fn click_clip(&mut self, path: &str, ctrl: bool, shift: bool, cx: &mut Context<Self>) {
        let order = self.visible_order();
        self.selection.click(path, &order, ctrl, shift);
        cx.notify();
    }

    /// The visible order, as strings. This is what shift-ranges span, including
    /// rows that are not painted yet.
    pub fn visible_order(&self) -> Vec<String> {
        self.visible.iter().map(|c| c.path.clone()).collect()
    }

    pub fn delete_selected(&mut self, cx: &mut Context<Self>) {
        let paths = self.selection.selected.clone();
        if paths.is_empty() {
            return;
        }
        let state = Arc::clone(&self.state);
        let for_task = paths.clone();
        cx.spawn(async move |deck, cx| {
            let deleted = cx
                .background_spawn(async move { state.delete_clips(&for_task) })
                .await;
            deck.update(cx, |deck, cx| {
                deck.selection.remove(&paths);
                deck.clips.retain(|c| !paths.iter().any(|p| p == &c.path));
                deck.refresh_visible_dirty();
                deck.toast_info("Clips deleted", &format!("{deleted} removed"));
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Deletes every clip in the output folder. Destructive and irreversible, so
    /// the button that calls it is styled as danger and says so.
    pub fn delete_all_clips(&mut self, cx: &mut Context<Self>) {
        let state = Arc::clone(&self.state);
        cx.spawn(async move |deck, cx| {
            let deleted = cx
                .background_spawn(async move { state.delete_all_clips() })
                .await;
            deck.update(cx, |deck, cx| {
                match deleted {
                    Ok(count) => {
                        deck.clips.clear();
                        deck.selection.clear();
                        deck.thumbs.clear();
                        deck.refresh_visible_dirty();
                        deck.toast_info("Library cleared", &format!("{count} clips deleted"));
                    }
                    Err(e) => deck.toast_error("Could not delete the clips", &e),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub fn open_clip(&mut self, path: &str, cx: &mut Context<Self>) {
        // A single click opens the OS player: phase 1 has no `<video>` surface.
        let target = std::path::PathBuf::from(path);
        if let Err(e) = crate::platform::win32::shell_open(&target, false) {
            self.toast_error("Could not open the clip", &e);
            cx.notify();
        }
    }

    pub fn reveal_clip(&mut self, path: &str, cx: &mut Context<Self>) {
        let target = std::path::PathBuf::from(path);
        if let Err(e) = crate::platform::win32::shell_open(&target, true) {
            self.toast_error("Could not reveal the clip", &e);
            cx.notify();
        }
    }

    pub fn copy_paths(&mut self, paths: Vec<String>, cx: &mut Context<Self>) {
        let paths: Vec<std::path::PathBuf> = paths.into_iter().map(Into::into).collect();
        match crate::platform::win32::copy_files_to_clipboard(&paths) {
            Ok(()) => self.toast_info("Copied", "Clip file(s) are on the clipboard."),
            Err(e) => self.toast_error("Could not copy", &e),
        }
        cx.notify();
    }

    pub fn open_clips_folder(&mut self, cx: &mut Context<Self>) {
        let dir = self.state.output_dir();
        let _ = std::fs::create_dir_all(&dir);
        if let Err(e) = crate::platform::win32::shell_open(&dir, false) {
            self.toast_error("Could not open the folder", &e);
            cx.notify();
        }
    }

    pub fn toggle_favorite(&mut self, path: &str, cx: &mut Context<Self>) {
        let Some(clip) = self.clips.iter().find(|c| c.path == path) else {
            return;
        };
        let favorite = !clip.favorite;
        let state = Arc::clone(&self.state);
        let path_owned = path.to_string();
        let for_task = path_owned.clone();
        cx.spawn(async move |deck, cx| {
            let result = cx
                .background_spawn(async move { state.set_clip_favorite(&for_task, favorite) })
                .await;
            deck.update(cx, |deck, cx| {
                match result {
                    Ok(entry) => {
                        if let Some(clip) =
                            deck.clips.iter_mut().find(|c| c.path == path_owned)
                        {
                            clip.favorite = entry.favorite;
                        }
                        deck.refresh_visible_dirty();
                    }
                    Err(e) => deck.toast_error("Could not update the clip", &e),
                }
                cx.notify();
            })
            .ok();
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
        let mut topmost = |on: bool| crate::platform::win32::set_topmost(hwnd, on);

        match self.state.update_settings(patch, &mut rebind, &mut topmost) {
            Ok(settings) => {
                let previous_dir = self.settings.output_dir.clone();
                self.settings = settings;
                // A changed output folder invalidates the whole library view.
                if self.settings.output_dir != previous_dir {
                    // Every cached texture belongs to the old folder.
                    self.selection.clear();
                    self.clips.clear();
                    self.thumbs.clear();
                    self.refresh_visible_dirty();
                    self.load_clips(cx);
                }
                self.toast_info("Settings saved", "Applied immediately.");
            }
            Err(e) => {
                // A rejected patch leaves the file and the live hotkey alone;
                // the view must not pretend otherwise.
                self.toast_error("Settings not saved", &e);
            }
        }
        cx.notify();
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
                        deck.toast_info(
                            "Update available",
                            &format!("ClipFlow {version} is out — see the Releases page."),
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
        self.toast_warn("Device loss simulated", "The engine should recover on its own.");
        cx.notify();
    }

    pub fn quit(&mut self, cx: &mut Context<Self>) {
        // Persist the view state and stop the encoder before the process dies:
        // the engine holds GPU resources that must be released in order.
        self.persist_gallery_state();
        self.state.quit();
        cx.quit();
    }

    // -----------------------------------------------------------------------
    // Hotkey recording
    // -----------------------------------------------------------------------

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
            .gap_5()
            .p_5()
            .overflow_y_scroll();
        deck_body = match self.view {
            View::Deck => deck_body
                .child(self.render_capture_section(palette, cx))
                .child(self.render_library(palette, cx)),
            View::Settings => deck_body.child(self.render_settings(palette, cx)),
        };

        div()
            .id("deck-root")
            .flex()
            .flex_col()
            .size_full()
            .bg(palette.deck)
            .text_color(palette.text)
            .font_family(crate::ui::theme::SANS)
            .track_focus(&self.focus)
            .focusable()
            .on_key_down(cx.listener(|deck, event: &KeyDownEvent, _window, cx| {
                deck.record_hotkey_key(event, cx);
            }))
            .child(self.render_titlebar(palette, armed, cx))
            .child(deck_body)
            .child(self.render_toasts(palette))
            .child(self.render_footer(palette, armed))
    }
}

/// Persisted gallery view state.
///
/// The Tauri build kept this in `localStorage` under
/// `clipflow.gallery.state.v1`. The GPUI build keeps the same JSON blob in a
/// small file next to `settings.json`, so the key and shape stay identical and
/// the module's sanitizer still guards against junk.
const GALLERY_FILE: &str = "gallery_state.json";

pub fn gallery_state_path() -> std::path::PathBuf {
    crate::settings::Settings::config_path()
        .parent()
        .map(|dir| dir.join(GALLERY_FILE))
        .unwrap_or_else(|| std::env::temp_dir().join("ClipFlow").join(GALLERY_FILE))
}

pub fn load_gallery_state() -> GalleryState {
    load_gallery_state_from(&gallery_state_path())
}

/// Reads a view-state blob from an explicit path.
///
/// Split out so the round-trip can be tested without touching the user's real
/// `gallery_state.json`.
pub fn load_gallery_state_from(path: &std::path::Path) -> GalleryState {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return GalleryState::default();
    };
    // Unreadable / corrupt blob: start fresh rather than failing the load.
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return GalleryState::default();
    };
    GalleryState::sanitize(&value)
}

pub fn save_gallery_state(state: &GalleryState) {
    save_gallery_state_to(&gallery_state_path(), state);
}

/// Writes a view-state blob to an explicit path. Failures are swallowed: a
/// read-only AppData must never block the UI.
pub fn save_gallery_state_to(path: &std::path::Path, state: &GalleryState) {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(json) = serde_json::to_string(state) {
        let _ = std::fs::write(path, json);
    }
}

// ---------------------------------------------------------------------------
// Window chrome: titlebar, toasts, footer
// ---------------------------------------------------------------------------

impl Deck {
    pub(crate) fn render_titlebar(
        &self,
        palette: Palette,
        armed: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        // The window is frameless (see `TitleBar::window_options`), so the app
        // owns drag handling and draws its own controls. `TitleBar` supplies the
        // drag region on Windows via WM_NCHITTEST for anything placed directly
        // in it; the buttons are ordinary elements on top.
        let state = self.stats.state;
        let pill_color = if armed {
            palette.lime
        } else {
            palette.muted
        };

        TitleBar::new()
            .on_close_window(cx.listener(|deck, _ev: &ClickEvent, _window, cx| {
                // "Close" means hide-to-tray when that setting is on, exactly
                // like the Tauri `CloseRequested` handler. A real quit is the
                // footer button or the tray's Quit.
                if deck.settings.minimize_to_tray {
                    crate::platform::win32::set_visible(deck.hwnd, false);
                } else {
                    deck.quit(cx);
                }
            }))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .w_full()
                    .h(px(44.0))
                    .px_4()
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_3()
                            .child(
                                // The logotype: a cyan block glyph plus the word
                                // mark, matching the old brand row.
                                div()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .size(px(22.0))
                                    .rounded_md()
                                    .bg(palette.cyan)
                                    .text_color(palette.void)
                                    .font_weight(FontWeight::BOLD)
                                    .child("C"),
                            )
                            .child(
                                div()
                                    .font_weight(FontWeight::BOLD)
                                    .text_lg()
                                    .child("CLIPFLOW"),
                            )
                            .child(
                                div()
                                    .font_family(crate::ui::theme::MONO)
                                    .text_xs()
                                    .text_color(palette.muted)
                                    .child("INSTANT REPLAY"),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_3()
                            // Armed pill: colour + label, never a bare dot, so
                            // the state is readable without colour vision.
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap_2()
                                    .px_3()
                                    .py_1()
                                    .rounded_full()
                                    .border_1()
                                    .border_color(pill_color)
                                    .child(
                                        div()
                                            .size(px(7.0))
                                            .rounded_full()
                                            .bg(pill_color),
                                    )
                                    .child(
                                        div()
                                            .font_family(crate::ui::theme::MONO)
                                            .text_xs()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(pill_color)
                                            .child(capture::state_label(state)),
                                    ),
                            )
                            // Settings is a full-window view behind this gear,
                            // matching the old deck's navigation.
                            .child(
                                Button::new("gear")
                                    .ghost()
                                    .compact()
                                    .label(if self.view == View::Settings {
                                        "Deck"
                                    } else {
                                        "Settings"
                                    })
                                    .tooltip("Capture settings, hotkeys and folders")
                                    .on_click(cx.listener(|deck, _ev, _window, cx| {
                                        deck.view = match deck.view {
                                            View::Deck => View::Settings,
                                            View::Settings => View::Deck,
                                        };
                                        cx.notify();
                                    })),
                            )
                            // Hide to the tray without stopping the engine.
                            // The header X does the same when
                            // `minimize_to_tray` is on; this button makes it
                            // reachable either way.
                            .child(
                                Button::new("hide")
                                    .ghost()
                                    .compact()
                                    .label("Hide")
                                    .tooltip("Keep recording and hide the deck to the tray")
                                    .on_click(cx.listener(|deck, _ev, _window, _cx| {
                                        crate::platform::win32::set_visible(deck.hwnd, false);
                                    })),
                            )
                            // The tray-agnostic quit: available even when
                            // `minimize_to_tray` is on and the header X hides.
                            .child(
                                Button::new("quit")
                                    .ghost()
                                    .compact()
                                    .label("Quit")
                                    .tooltip("Stop the engine and exit ClipFlow")
                                    .on_click(cx.listener(|deck, _ev, _window, cx| {
                                        deck.handle_intent(Intent::Quit, cx);
                                    })),
                            ),
                    ),
            )
    }

    pub(crate) fn render_toasts(&self, palette: Palette) -> impl IntoElement {
        let toasts: Vec<_> = self.toasts.clone();
        div()
            .flex()
            .flex_col()
            .gap_2()
            .px_5()
            .pb_2()
            .children(toasts.into_iter().map(move |t| {
                let accent = palette.tone(t.tone);
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .bg(palette.panel_2)
                    .border_1()
                    .border_color(accent)
                    .child(
                        div()
                            .font_family(crate::ui::theme::MONO)
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(accent)
                            .child(t.title),
                    )
                    .children(t.body.map(|body| {
                        div()
                            .font_family(crate::ui::theme::MONO)
                            .text_xs()
                            .text_color(palette.muted)
                            .child(body)
                    }))
            }))
    }

    pub(crate) fn render_footer(&self, palette: Palette, armed: bool) -> impl IntoElement {
        let stats = &self.stats;
        let dot = if armed { palette.lime } else { palette.muted };
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
            .px_5()
            .py_2()
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
                        div()
                            .font_family(crate::ui::theme::MONO)
                            .text_xs()
                            .text_color(palette.muted)
                            .child(format!("v{}", env!("CARGO_PKG_VERSION"))),
                    ),
            )
    }
}



//! Everything the old `commands.rs` did, without IPC.
//!
//! Each `#[tauri::command]` became a plain method on [`AppState`]. Nothing here
//! talks to a UI: these are synchronous, potentially blocking calls, and the
//! view is responsible for running them through
//! `cx.background_spawn(...)` so a flush (which can take a second) never
//! stalls a frame.
//!
//! Kept deliberately close to the original bodies: the clamps, error strings
//! and ordering are user-visible contracts (a rebind that fails must leave the
//! old hotkey live, a settings save must be all-or-nothing, and privacy mode
//! must refuse a flush of desktop footage). Do not "simplify" them.
//!
//! The one structural change is the removal of the event bus. `EVT_CLIP_SAVED`
//! used to drive *both* the gallery insert and the toast in `App.tsx`; with no
//! listener indirection left, [`SaveOutcome`] is returned to the caller, which
//! updates the clip list and raises the toast itself.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};

use crate::clipmeta::{ClipMetaEntry, ClipMetaStore};
use crate::foreground::ForegroundGame;
use crate::media::library::{self, ClipMetadata};
use crate::media::recorder::{
    CaptureEngine, ClipWriteResult, Codec, EngineStats, MonitorInfo, enumerate_monitors,
};
use crate::settings::{CaptureProfile, ProfileMapEntry, Settings};

/// Shared application state.
///
/// `engine` is `Arc` because a flush must own a handle to run off-thread while
/// the view keeps its own reference. `settings` is an `RwLock` rather than
/// plain state because the 4 Hz stats loop, the foreground watcher and the UI
/// all read it concurrently.
pub struct AppState {
    pub engine: Arc<CaptureEngine>,
    pub settings: RwLock<Settings>,
    /// Favourites + custom tags, persisted to `clip_meta.json` in AppData.
    pub clip_meta: Mutex<ClipMetaStore>,
    /// Guards against two `Alt+C` flushes racing each other. `try_lock` is the
    /// point: if a flush is already running the second press is rejected with
    /// "Save replay already in progress" instead of doubling encoder work.
    pub flush_lock: Arc<parking_lot::Mutex<()>>,
}

impl AppState {
    pub fn new() -> Self {
        let mut settings = Settings::load();
        settings.seed_default_profiles();
        let _ = settings.save(); // persist the stock profiles once
        Self {
            engine: Arc::new(CaptureEngine::new()),
            settings: RwLock::new(settings),
            clip_meta: Mutex::new(ClipMetaStore::load()),
            flush_lock: Arc::new(parking_lot::Mutex::new(())),
        }
    }

    pub fn output_dir(&self) -> PathBuf {
        PathBuf::from(self.settings.read().output_dir.clone())
    }

    pub fn settings_snapshot(&self) -> Settings {
        self.settings.read().clone()
    }

    // -----------------------------------------------------------------------
    // Buffer lifecycle
    // -----------------------------------------------------------------------

    /// Initialises DXGI Desktop Duplication + the hardware encoder and starts
    /// filling the in-memory ring buffer.
    ///
    /// Blocking (encoder init takes 100-400 ms). Call via `background_spawn`.
    /// Returns the fresh stats so the caller can flip ARM/SAVE state without
    /// waiting for the next 250 ms telemetry tick.
    pub fn start_replay_buffer(&self, buffer_seconds: u32, target_fps: u32) -> Result<EngineStats, String> {
        let cfg = {
            let mut s = self.settings.write();
            s.buffer_seconds = buffer_seconds.clamp(5, 600);
            s.target_fps = target_fps.clamp(24, 240);
            let _ = s.save();
            s.to_recorder_config()
        };
        std::fs::create_dir_all(&cfg.output_dir).map_err(|e| e.to_string())?;
        self.engine.start(cfg).map_err(|e| e.to_string())?;
        Ok(self.engine.stats())
    }

    pub fn stop_replay_buffer(&self) -> EngineStats {
        self.engine.stop();
        self.engine.stats()
    }

    pub fn engine_stats(&self) -> EngineStats {
        self.engine.stats()
    }

    pub fn is_armed(&self) -> bool {
        self.engine.is_running()
    }

    /// Live-only apply: the engine's rolling window changes without touching
    /// the persisted setting — persisting is [`Self::update_settings`]' job.
    /// Adaptive ECO relies on this: it shrinks the ring under battery/RAM
    /// pressure and restores it afterwards without clobbering the user's value.
    pub fn set_live_buffer_seconds(&self, seconds: u32) {
        self.engine.set_buffer_seconds(seconds.clamp(5, 600));
    }

    pub fn set_privacy_gate(&self, gate: bool) {
        self.engine.set_privacy_gate(gate);
    }

    // -----------------------------------------------------------------------
    // Save path
    // -----------------------------------------------------------------------

    /// Flushes the ring buffer to a finalised `.mp4`.
    ///
    /// Blocking (this is the expensive one — hundreds of ms for 60 s of 1440p)
    /// and must run on `background_spawn`.
    ///
    /// `max_seconds` lets the tray ask for "last 30 s" out of a 120 s buffer
    /// without reconfiguring the engine. `game_override` is used by the
    /// auto-save-on-game-exit flow: the game that just lost focus is tagged
    /// even though it is no longer the foreground process at flush time.
    pub fn save_instant_replay(
        &self,
        max_seconds: Option<f32>,
        trigger: SaveTrigger,
        game_override: Option<String>,
    ) -> Result<SaveOutcome, String> {
        // Privacy mode: never save desktop content. If no game has focus the
        // ring is empty anyway — fail with an actionable message instead of a
        // bare "empty buffer".
        if let Some(msg) = privacy_blocked_message(self, game_override.as_deref()) {
            return Err(msg);
        }

        // The guard is real, not decoration: two Alt+C presses must not race
        // the encoder.
        let _guard = self
            .flush_lock
            .try_lock()
            .ok_or_else(|| "Save replay already in progress".to_string())?;

        // Per-game organisation: route the flush into output_dir/<game>/. The
        // subfolder is recomputed on every save so it can never go stale.
        let game_folder = resolve_game_folder(self, game_override.as_deref());
        self.engine.set_output_subfolder(game_folder.clone());

        let result: ClipWriteResult = self
            .engine
            .flush_to_disk(max_seconds)
            .map_err(|e| e.to_string())?;

        let path = PathBuf::from(&result.path);
        let thumbnail = library::extract_thumbnail(
            &path,
            (result.duration_seconds * 0.35).clamp(0.2, 6.0),
        )
        .ok();

        let now = chrono::Utc::now().timestamp_millis();
        let clip = ClipMetadata {
            id: format!("{}-{now}", result.file_name),
            path: result.path.clone(),
            file_name: result.file_name.clone(),
            title: result.file_name.trim_end_matches(".mp4").replace('_', " "),
            duration_seconds: result.duration_seconds,
            size_bytes: result.size_bytes,
            created_unix_ms: now,
            width: result.width,
            height: result.height,
            fps: result.fps,
            has_audio: result.has_audio,
            game: game_folder,
            thumbnail,
            favorite: false,
            tags: Vec::new(),
        };

        log::info!(
            "[clipflow] saved {} ({:.1}s, {:.1} MB) in {:.1} ms",
            clip.file_name,
            clip.duration_seconds,
            clip.size_bytes as f32 / 1_048_576.0,
            result.flush_ms
        );

        Ok(SaveOutcome {
            clip,
            flush_ms: result.flush_ms,
            trigger,
        })
    }

    // -----------------------------------------------------------------------
    // Library
    // -----------------------------------------------------------------------

    /// Scans the output folder, folding in favourites + tags from the sidecar.
    /// Blocking: probes uncached files through Media Foundation.
    pub fn recorded_clips(&self, with_thumbnails: bool) -> Result<Vec<ClipMetadata>, String> {
        let dir = self.output_dir();
        let mut clips = library::scan_directory(&dir, with_thumbnails)
            .map_err(|e| e.to_string())?;
        self.clip_meta.lock().merge_into(&mut clips);
        Ok(clips)
    }

    pub fn delete_clip(&self, path: &str) -> Result<(), String> {
        library::delete_clip(Path::new(path)).map_err(|e| e.to_string())
    }

    /// Deletes several clips at once. Returns how many were actually removed.
    pub fn delete_clips(&self, paths: &[String]) -> u32 {
        let mut deleted = 0u32;
        for p in paths {
            if library::delete_clip(Path::new(p)).is_ok() {
                deleted += 1;
            }
        }
        deleted
    }

    /// Removes every recorded clip from the output folder. Blocking.
    pub fn delete_all_clips(&self) -> Result<u32, String> {
        let dir = self.output_dir();
        let clips = library::scan_directory(&dir, false).map_err(|e| e.to_string())?;
        let mut deleted = 0u32;
        for c in &clips {
            if library::delete_clip(Path::new(&c.path)).is_ok() {
                deleted += 1;
            }
        }
        Ok(deleted)
    }

    /// Deletes clips older than `days`. Blocking.
    pub fn cleanup_old_clips(&self, days: u32) -> Result<u32, String> {
        let dir = self.output_dir();
        let days = days.max(1);
        let cutoff = chrono::Utc::now() - chrono::Duration::days(days as i64);
        let clips = library::scan_directory(&dir, false).map_err(|e| e.to_string())?;
        let mut deleted = 0u32;
        for c in &clips {
            let created = chrono::DateTime::from_timestamp_millis(c.created_unix_ms);
            if let Some(created) = created {
                if created < cutoff && library::delete_clip(Path::new(&c.path)).is_ok() {
                    deleted += 1;
                }
            }
        }
        Ok(deleted)
    }

    pub fn rename_clip(&self, path: &str, new_name: &str) -> Result<String, String> {
        let src = PathBuf::from(path);
        let sanitized: String = new_name
            .chars()
            .filter(|c| !matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|'))
            .collect();
        let name = if sanitized.to_ascii_lowercase().ends_with(".mp4") {
            sanitized
        } else {
            format!("{sanitized}.mp4")
        };
        let dest = src
            .parent()
            .ok_or_else(|| "clip has no parent directory".to_string())?
            .join(name);
        std::fs::rename(&src, &dest).map_err(|e| e.to_string())?;
        // Keep favourites/tags attached across the rename: the sidecar is keyed
        // by path, so the entry moves to the new path.
        self.clip_meta
            .lock()
            .migrate(&src.to_string_lossy(), &dest.to_string_lossy());
        Ok(dest.to_string_lossy().to_string())
    }

    // -----------------------------------------------------------------------
    // Clip metadata (favourites + tags)
    // -----------------------------------------------------------------------

    pub fn set_clip_favorite(&self, path: &str, favorite: bool) -> Result<ClipMetaEntry, String> {
        if !Path::new(path).exists() {
            return Err(format!("clip not found: {path}"));
        }
        self.clip_meta.lock().set_favorite(path, favorite)
    }

    pub fn set_clip_tags(&self, path: &str, tags: Vec<String>) -> Result<ClipMetaEntry, String> {
        if !Path::new(path).exists() {
            return Err(format!("clip not found: {path}"));
        }
        self.clip_meta.lock().set_tags(path, tags)
    }

    pub fn set_clips_favorite(&self, paths: &[String], favorite: bool) -> u32 {
        self.clip_meta.lock().set_favorite_many(paths, favorite)
    }

    pub fn add_clips_tag(&self, paths: &[String], tag: &str) -> u32 {
        self.clip_meta.lock().add_tag_many(paths, tag)
    }

    pub fn remove_clips_tag(&self, paths: &[String], tag: &str) -> u32 {
        self.clip_meta.lock().remove_tag_many(paths, tag)
    }

    pub fn monitors(&self) -> Vec<MonitorInfo> {
        enumerate_monitors()
    }

    // -----------------------------------------------------------------------
    // Settings
    // -----------------------------------------------------------------------

    pub fn get_settings(&self) -> Settings {
        self.settings.read().clone()
    }

    /// Applies a patch, all-or-nothing.
    ///
    /// Ordering matters and is inherited from the Tauri implementation:
    /// hotkeys are re-registered with the OS *before* anything is persisted, so
    /// a combo owned by another app fails the whole patch and leaves the old
    /// binding registered and the file untouched.
    ///
    /// `rebind` is injected rather than called directly because the hotkey
    /// manager only exists on Windows and only inside the running app; tests
    /// exercise the pure halves ([`hotkey_conflict`], [`apply_patch_values`])
    /// instead.
    pub fn update_settings(
        &self,
        patch: SettingsPatch,
        rebind: &mut dyn FnMut(&str, &str) -> Result<(), String>,
        apply_topmost: &mut dyn FnMut(bool),
    ) -> Result<Settings, String> {
        let mut s = self.settings.write();

        // Conflicts are rejected up front so one combo can never drive both
        // actions.
        hotkey_conflict(&patch, &s)?;

        // Register first: if the OS refuses the combo nothing below runs.
        if let Some(v) = patch.hotkey_save.clone() {
            rebind(&s.hotkey_save, &v)?;
            s.hotkey_save = v;
        }
        if let Some(v) = patch.hotkey_toggle.clone() {
            rebind(&s.hotkey_toggle, &v)?;
            s.hotkey_toggle = v;
        }

        let effects = apply_patch_values(&patch, &mut s)?;
        // Side effects run before the file write, so a bad output folder fails
        // the patch instead of persisting a path the engine cannot use.
        effects.apply(&self.engine, apply_topmost)?;

        s.save()?;
        Ok(s.clone())
    }

    /// Re-registers just the save hotkey (kept for API symmetry; the settings
    /// view goes through `update_settings` so both hotkeys rebind the same way).
    pub fn set_save_hotkey(
        &self,
        accelerator: String,
        rebind: &mut dyn FnMut(&str, &str) -> Result<(), String>,
    ) -> Result<String, String> {
        let previous = self.settings.read().hotkey_save.clone();
        rebind(&previous, &accelerator)?;
        let mut s = self.settings.write();
        s.hotkey_save = accelerator.clone();
        s.save()?;
        Ok(accelerator)
    }

    pub fn set_toggle_hotkey(
        &self,
        accelerator: String,
        rebind: &mut dyn FnMut(&str, &str) -> Result<(), String>,
    ) -> Result<String, String> {
        let previous = self.settings.read().hotkey_toggle.clone();
        rebind(&previous, &accelerator)?;
        let mut s = self.settings.write();
        s.hotkey_toggle = accelerator.clone();
        s.save()?;
        Ok(accelerator)
    }

    // -----------------------------------------------------------------------
    // Profiles (persisted; the editor UI is deferred but apply/seed stay live)
    // -----------------------------------------------------------------------

    pub fn get_profiles(&self) -> Vec<CaptureProfile> {
        self.settings.read().profiles.clone()
    }

    pub fn save_profile(&self, profile: CaptureProfile) -> Result<Settings, String> {
        let mut s = self.settings.write();
        let id = profile.id.trim().to_ascii_lowercase();
        if id.is_empty() {
            return Err("profile id cannot be empty".into());
        }
        let mut profile = profile;
        profile.id = id.clone();
        if profile.name.trim().is_empty() {
            profile.name = id.clone();
        } else {
            profile.name = profile.name.trim().to_string();
        }
        profile.buffer_seconds = profile.buffer_seconds.clamp(5, 600);
        profile.target_fps = profile.target_fps.clamp(24, 240);
        profile.bitrate_kbps = profile.bitrate_kbps.clamp(1_000, 150_000);

        if let Some(existing) = s.profiles.iter_mut().find(|p| p.id == id) {
            *existing = profile;
        } else {
            s.profiles.push(profile);
        }
        s.save()?;
        Ok(s.clone())
    }

    pub fn delete_profile(&self, profile_id: &str) -> Result<Settings, String> {
        let mut s = self.settings.write();
        let before = s.profiles.len();
        s.profiles.retain(|p| p.id != profile_id);
        if s.profiles.len() == before {
            return Err(format!("profile '{profile_id}' not found"));
        }
        s.profile_map.retain(|m| m.profile_id != profile_id);
        s.save()?;
        Ok(s.clone())
    }

    /// Replaces the whole exe → profile map. Rows with empty exes or unknown
    /// profile ids are dropped; duplicate exes keep the first entry.
    pub fn set_profile_map(&self, map: Vec<ProfileMapEntry>) -> Result<Settings, String> {
        let mut s = self.settings.write();
        let mut seen = std::collections::HashSet::new();
        let cleaned: Vec<ProfileMapEntry> = map
            .into_iter()
            .filter_map(|m| {
                let exe = m.exe_name.trim().to_ascii_lowercase();
                if exe.is_empty() || s.profile_by_id(&m.profile_id).is_none() {
                    return None;
                }
                if !seen.insert(exe.clone()) {
                    return None;
                }
                Some(ProfileMapEntry {
                    profile_id: m.profile_id,
                    exe_name: exe,
                })
            })
            .collect();
        s.profile_map = cleaned;
        s.save()?;
        Ok(s.clone())
    }

    /// Applies a profile right now: the buffer window changes live on the
    /// running engine, fps/bitrate/codec are persisted and take effect on the
    /// next engine start.
    pub fn apply_profile(&self, profile_id: &str) -> Result<Settings, String> {
        let mut s = self.settings.write();
        let profile = s
            .profile_by_id(profile_id)
            .cloned()
            .ok_or_else(|| format!("profile '{profile_id}' not found"))?;
        s.apply_profile_values(&profile);
        self.engine.set_buffer_seconds(s.buffer_seconds);
        s.save()?;
        Ok(s.clone())
    }

    // -----------------------------------------------------------------------
    // System
    // -----------------------------------------------------------------------

    pub fn system_report(&self) -> SystemReport {
        SystemReport {
            os: std::env::consts::OS.to_string(),
            app_version: env!("CARGO_PKG_VERSION").to_string(),
            process_rss_bytes: crate::media::recorder::process_rss_bytes(),
            output_dir: self.settings.read().output_dir.clone(),
            monitors: enumerate_monitors(),
        }
    }

    pub fn quit(&self) {
        self.engine.stop();
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Save result
// ---------------------------------------------------------------------------

/// Who asked for the save. Replaces the `triggered_by` string + `open_trimmer`
/// bool pair from the event payload.
///
/// `OpenTrimmer` was decided in Rust because the hotkey path must never raise
/// the deck mid-game. With the trimmer deferred there is nothing to open, so
/// that field is gone on purpose rather than half-ported: the view always
/// reports the result on the status line (plus a native notification), and
/// raises the window only when [`safe_to_raise`] allows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveTrigger {
    Ui,
    Hotkey,
    Tray,
    Autosave,
}

impl SaveTrigger {
    pub fn as_str(self) -> &'static str {
        match self {
            SaveTrigger::Ui => "ui",
            SaveTrigger::Hotkey => "hotkey",
            SaveTrigger::Tray => "tray",
            SaveTrigger::Autosave => "autosave",
        }
    }
}

/// A finished save, returned to the caller instead of emitted as an event.
#[derive(Debug, Clone)]
pub struct SaveOutcome {
    pub clip: ClipMetadata,
    pub flush_ms: f32,
    pub trigger: SaveTrigger,
}

// ---------------------------------------------------------------------------
// Settings patch
// ---------------------------------------------------------------------------

/// Wire-compatible with the old IPC struct: `camelCase` keys, every field
/// optional, so a patch from any source only touches what it names.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsPatch {
    pub buffer_seconds: Option<u32>,
    pub target_fps: Option<u32>,
    pub bitrate_kbps: Option<u32>,
    pub codec: Option<Codec>,
    pub monitor_index: Option<u32>,
    pub capture_system_audio: Option<bool>,
    pub capture_microphone: Option<bool>,
    pub output_dir: Option<String>,
    pub open_trimmer_after_save: Option<bool>,
    pub minimize_to_tray: Option<bool>,
    pub autostart_buffer: Option<bool>,
    pub play_save_sound: Option<bool>,
    pub hotkey_save: Option<String>,
    pub hotkey_toggle: Option<String>,
    pub always_on_top: Option<bool>,
    pub auto_switch_profiles: Option<bool>,
    pub launch_at_startup: Option<bool>,
    pub auto_cleanup_days: Option<u32>,
    pub organize_by_game: Option<bool>,
    pub autosave_on_game_exit: Option<bool>,
    pub hud_enabled: Option<bool>,
    pub privacy_pause_when_unfocused: Option<bool>,
    pub game_volume: Option<u32>,
    pub mic_volume: Option<u32>,
    pub adaptive_eco: Option<bool>,
    pub eco_battery_threshold_pct: Option<u32>,
    pub eco_ram_free_gbs: Option<u32>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemReport {
    pub os: String,
    pub app_version: String,
    pub process_rss_bytes: u64,
    pub output_dir: String,
    pub monitors: Vec<MonitorInfo>,
}

// ---------------------------------------------------------------------------
// Settings patch application
// ---------------------------------------------------------------------------

/// Rejects a patch whose hotkeys would collide. Pure so the messages — which
/// are user-facing — can be asserted without an engine or a registry.
pub fn hotkey_conflict(patch: &SettingsPatch, current: &Settings) -> Result<(), String> {
    if let (Some(a), Some(b)) = (patch.hotkey_save.as_ref(), patch.hotkey_toggle.as_ref()) {
        if a == b {
            return Err(format!(
                "save and arm/disarm hotkeys cannot share the same combo '{a}'"
            ));
        }
    }
    if let Some(v) = patch.hotkey_save.as_ref() {
        if v == &current.hotkey_toggle {
            return Err(format!("'{v}' is already used by the arm/disarm hotkey"));
        }
    }
    if let Some(v) = patch.hotkey_toggle.as_ref() {
        if v == &current.hotkey_save {
            return Err(format!("'{v}' is already used by the save hotkey"));
        }
    }
    Ok(())
}

/// Side effects a patch implies. Collected so the pure value-update step stays
/// testable and the engine/OS calls happen in one obvious place.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct PatchEffects {
    /// New live ring length (already clamped), if the patch changed it.
    pub live_buffer_seconds: Option<u32>,
    /// New audio mix (already clamped), if either volume changed.
    pub volumes: Option<(u32, u32)>,
    /// Desired topmost flag, if the patch changed it.
    pub topmost: Option<bool>,
    /// Desired privacy gate, if privacy mode was toggled on.
    pub privacy_gate_on: Option<bool>,
}

impl PatchEffects {
    pub fn apply(
        &self,
        engine: &CaptureEngine,
        apply_topmost: &mut dyn FnMut(bool),
    ) -> Result<(), String> {
        if let Some(seconds) = self.live_buffer_seconds {
            engine.set_buffer_seconds(seconds);
        }
        if let Some((game, mic)) = self.volumes {
            engine.set_audio_volumes(game, mic);
        }
        if let Some(topmost) = self.topmost {
            apply_topmost(topmost);
        }
        if let Some(on) = self.privacy_gate_on {
            // If privacy just turned on while no game is focused, gate the
            // engine immediately instead of waiting for the next 2 s poll.
            // Turning it off un-gates right away too.
            engine.set_privacy_gate(if on { privacy_should_gate() } else { false });
        }
        Ok(())
    }
}

/// Applies every field of `patch` to `settings` with the original clamps, and
/// reports the side effects the caller must perform.
///
/// The one failure mode is the output folder: it is created here before the
/// path is stored, so a patch that changes it fails instead of persisting a
/// folder the engine cannot write to.
pub fn apply_patch_values(
    patch: &SettingsPatch,
    settings: &mut Settings,
) -> Result<PatchEffects, String> {
    let mut effects = PatchEffects::default();

    if let Some(v) = patch.buffer_seconds {
        settings.buffer_seconds = v.clamp(5, 600);
        effects.live_buffer_seconds = Some(settings.buffer_seconds);
    }
    if let Some(v) = patch.target_fps {
        settings.target_fps = v.clamp(24, 240);
    }
    if let Some(v) = patch.bitrate_kbps {
        // Matches the settings panel's numeric field (`BITRATE_MIN..=MAX` in
        // `ui::settings`); the engine itself accepts a wider range, but the UI
        // never offers it.
        settings.bitrate_kbps = v.clamp(3_000, 30_000);
    }
    if let Some(v) = patch.codec {
        settings.codec = v;
    }
    if let Some(v) = patch.monitor_index {
        settings.monitor_index = v;
    }
    if let Some(v) = patch.capture_system_audio {
        settings.capture_system_audio = v;
    }
    if let Some(v) = patch.capture_microphone {
        settings.capture_microphone = v;
    }
    if let Some(v) = patch.output_dir.as_ref() {
        std::fs::create_dir_all(v).map_err(|e| e.to_string())?;
        settings.output_dir = v.clone();
    }
    if let Some(v) = patch.open_trimmer_after_save {
        settings.open_trimmer_after_save = v;
    }
    if let Some(v) = patch.minimize_to_tray {
        settings.minimize_to_tray = v;
    }
    if let Some(v) = patch.autostart_buffer {
        settings.autostart_buffer = v;
    }
    if let Some(v) = patch.play_save_sound {
        settings.play_save_sound = v;
    }
    if let Some(v) = patch.always_on_top {
        settings.always_on_top = v;
        effects.topmost = Some(v);
    }
    if let Some(v) = patch.auto_switch_profiles {
        settings.auto_switch_profiles = v;
    }
    if let Some(v) = patch.launch_at_startup {
        // Registry Run key lives with the OS; keep it in sync with the setting
        // so a corrupt settings.json can never leave an unwanted autostart.
        settings.launch_at_startup = v;
        if v {
            crate::platform::autostart::set(std::env::current_exe().ok().as_deref())
                .map_err(|e| format!("could not enable launch at startup: {e}"))?;
        } else {
            crate::platform::autostart::set(None)
                .map_err(|e| format!("could not disable launch at startup: {e}"))?;
        }
    }
    if let Some(v) = patch.auto_cleanup_days {
        settings.auto_cleanup_days = v;
    }
    if let Some(v) = patch.organize_by_game {
        settings.organize_by_game = v;
    }
    if let Some(v) = patch.autosave_on_game_exit {
        settings.autosave_on_game_exit = v;
    }
    if let Some(v) = patch.hud_enabled {
        // No HUD in phase 1. The field is persisted so a later build picks it
        // up unchanged; nothing else happens.
        settings.hud_enabled = v;
    }
    if let Some(v) = patch.privacy_pause_when_unfocused {
        settings.privacy_pause_when_unfocused = v;
        effects.privacy_gate_on = Some(v);
    }
    if let Some(v) = patch.game_volume {
        settings.game_volume = v.clamp(0, 100);
        effects.volumes = Some((settings.game_volume, settings.mic_volume));
    }
    if let Some(v) = patch.mic_volume {
        settings.mic_volume = v.clamp(0, 100);
        effects.volumes = Some((settings.game_volume, settings.mic_volume));
    }
    if let Some(v) = patch.adaptive_eco {
        settings.adaptive_eco = v;
        // Re-evaluate immediately so enabling ECO reacts now, not at the next
        // poll. The caller applies the resulting buffer change.
    }
    if let Some(v) = patch.eco_battery_threshold_pct {
        settings.eco_battery_threshold_pct = v.clamp(5, 100);
    }
    if let Some(v) = patch.eco_ram_free_gbs {
        settings.eco_ram_free_gbs = v.clamp(1, 32);
    }

    Ok(effects)
}

// ---------------------------------------------------------------------------
// Policy helpers (ported verbatim, including the comments' intent)
// ---------------------------------------------------------------------------

/// Sanitises a foreground exe into a safe per-game folder name. Returns `None`
/// for our own process so pressing `Alt+C` on the deck never creates a
/// "clipflow" folder.
pub fn game_folder_name(exe: &str) -> Option<String> {
    let exe = exe.to_ascii_lowercase();
    if exe.starts_with("clipflow") {
        return None;
    }
    let stem = match exe.rfind('.') {
        Some(i) if i > 0 => &exe[..i],
        _ => exe.as_str(),
    };
    let sanitized: String = stem
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_') {
                c
            } else {
                '-'
            }
        })
        .collect();
    let name = sanitized.trim_matches('-').to_string();
    if name.is_empty() { None } else { Some(name) }
}

/// Resolves the per-game subfolder for the next flush: the game in the
/// foreground, or `game_override` when the caller already knows the game (the
/// auto-save-on-game-exit flow). Returns `None` when per-game organisation is
/// off or no game applies — the clip then lands in the output root.
pub fn resolve_game_folder(state: &AppState, game_override: Option<&str>) -> Option<String> {
    let s = state.settings.read();
    if !s.organize_by_game {
        return None;
    }
    if let Some(g) = game_override {
        return game_folder_name(g);
    }
    crate::foreground::get_foreground_game().and_then(|g| game_folder_name(&g.exe))
}

/// The single source of truth for the privacy gate: pause only when the
/// foreground is *positively* our own deck or the desktop. When the query
/// returns `None` (elevated game, protected process, lock screen) we keep
/// recording — breaking `Alt+C` for an undetected game is worse than a rare
/// ambiguous frame.
///
/// Uses the title-less [`crate::foreground::foreground_exe`] so it is safe on
/// the main thread (no blocking `GetWindowTextW`).
pub fn privacy_should_gate() -> bool {
    match crate::foreground::foreground_exe() {
        Some(e) => e.starts_with("clipflow") || e.starts_with("explorer"),
        None => false,
    }
}

/// When privacy mode is on and the foreground is positively the desktop or our
/// own deck, saving would flush desktop footage (or an empty buffer). Returns
/// the message to surface, or `None` when saving is fine.
pub fn privacy_blocked_message(state: &AppState, game_override: Option<&str>) -> Option<String> {
    let s = state.settings.read();
    if !s.privacy_pause_when_unfocused {
        return None;
    }
    if game_override.is_some() {
        return None; // auto-save tags a game that just lost focus: gameplay
    }
    if privacy_should_gate() {
        Some(
            "Privacy mode: nothing recorded — no game is focused. Focus a game to arm the buffer again."
                .to_string(),
        )
    } else {
        None
    }
}

/// Whether raising the deck is safe. Raising the window mid-match
/// minimises/pauses exclusive-fullscreen games, so this is only true when the
/// foreground is *explicitly* our own window or the desktop; an undetectable
/// foreground (`None`) is treated as unsafe.
///
/// This is the one deliberate behaviour change from the Tauri build: there the
/// toast fired only when `open_trimmer && !safe_to_raise`, i.e. a toast was
/// mutually exclusive with raising. Now that the native notification plus the
/// in-app status line are the confirmation and the trimmer is gone, the save
/// path *always* notifies and *additionally* raises when safe. Commented here
/// so it does not read as a bug.
pub fn safe_to_raise() -> bool {
    let focused = crate::foreground::foreground_exe();
    safe_to_raise_for(focused.as_deref())
}

/// Pure helper so the rule can be unit-tested without a live foreground.
pub fn safe_to_raise_for(focused_exe: Option<&str>) -> bool {
    match focused_exe {
        Some(exe) => {
            let exe = exe.to_ascii_lowercase();
            exe.starts_with("clipflow") || exe.starts_with("explorer")
        }
        None => false,
    }
}

/// The game currently in the foreground, if any. Blocking on Windows
/// (`GetWindowTextW` can hang on a hung foreground app), so call it through
/// `background_spawn`.
pub fn get_foreground_game() -> Option<ForegroundGame> {
    crate::foreground::get_foreground_game()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn defaults() -> Settings {
        Settings::default()
    }

    /// `ClipMetaStore` is read-only for this test: the flush fails before any
    /// metadata is written, so loading the real sidecar is harmless and keeps
    /// the engine module untouched.
    fn empty_clip_meta() -> ClipMetaStore {
        ClipMetaStore::load()
    }

    #[test]
    fn save_trigger_strings_match_the_old_payload() {
        // The event payload used exactly these four values; log lines and
        // notifications still key off them.
        assert_eq!(SaveTrigger::Ui.as_str(), "ui");
        assert_eq!(SaveTrigger::Hotkey.as_str(), "hotkey");
        assert_eq!(SaveTrigger::Tray.as_str(), "tray");
        assert_eq!(SaveTrigger::Autosave.as_str(), "autosave");
    }

    #[test]
    fn game_folder_name_strips_extensions_and_sanitizes() {
        assert_eq!(game_folder_name("cs2.exe").as_deref(), Some("cs2"));
        assert_eq!(game_folder_name("Elden Ring.exe").as_deref(), Some("elden-ring"));
        // Our own process never creates a folder.
        assert_eq!(game_folder_name("clipflow.exe"), None);
        assert_eq!(game_folder_name("clipflow_lib.exe"), None);
        // A name made only of separators sanitises to nothing.
        assert_eq!(game_folder_name("   .exe"), None);
    }

    #[test]
    fn safe_to_raise_only_for_our_own_windows() {
        assert!(safe_to_raise_for(Some("clipflow.exe")));
        assert!(safe_to_raise_for(Some("explorer.exe")));
        assert!(safe_to_raise_for(Some("CLIPFLOW.EXE")));
        // A game, an unknown process, and an undetectable foreground are all
        // "do not steal focus".
        assert!(!safe_to_raise_for(Some("cs2.exe")));
        assert!(!safe_to_raise_for(None));
    }

    #[test]
    fn settings_patch_uses_camel_case_keys() {
        // The GPUI view builds patches in Rust, but keeping the wire shape
        // means a JSON patch from any source (tests, a future CLI) still works.
        let patch: SettingsPatch =
            serde_json::from_str(r#"{"bufferSeconds":30,"targetFps":120}"#).unwrap();
        assert_eq!(patch.buffer_seconds, Some(30));
        assert_eq!(patch.target_fps, Some(120));
        assert!(patch.codec.is_none());
    }

    #[test]
    fn rejects_the_same_combo_for_both_hotkeys() {
        let current = defaults();
        let patch = SettingsPatch {
            hotkey_save: Some("Alt+C".into()),
            hotkey_toggle: Some("Alt+C".into()),
            ..Default::default()
        };
        let err = hotkey_conflict(&patch, &current).unwrap_err();
        assert!(err.contains("cannot share the same combo"), "{err}");
    }

    #[test]
    fn rejects_a_combo_owned_by_the_other_action() {
        let current = defaults(); // Alt+C save, Alt+Shift+C toggle
        let patch = SettingsPatch {
            hotkey_save: Some("Alt+Shift+C".into()),
            ..Default::default()
        };
        let err = hotkey_conflict(&patch, &current).unwrap_err();
        assert!(err.contains("already used by the arm/disarm hotkey"), "{err}");

        let patch = SettingsPatch {
            hotkey_toggle: Some("Alt+C".into()),
            ..Default::default()
        };
        let err = hotkey_conflict(&patch, &current).unwrap_err();
        assert!(err.contains("already used by the save hotkey"), "{err}");
    }

    #[test]
    fn a_patch_naming_only_one_hotkey_does_not_conflict_with_itself() {
        let current = defaults();
        let patch = SettingsPatch {
            hotkey_save: Some("Ctrl+Alt+Z".into()),
            ..Default::default()
        };
        assert!(hotkey_conflict(&patch, &current).is_ok());
    }

    #[test]
    fn clamps_are_applied_to_every_numeric_field() {
        let mut s = defaults();
        let patch = SettingsPatch {
            buffer_seconds: Some(1),
            target_fps: Some(999),
            bitrate_kbps: Some(1),
            game_volume: Some(500),
            mic_volume: Some(500),
            eco_battery_threshold_pct: Some(0),
            eco_ram_free_gbs: Some(99),
            ..Default::default()
        };
        let effects = apply_patch_values(&patch, &mut s).unwrap();

        assert_eq!(s.buffer_seconds, 5);
        assert_eq!(s.target_fps, 240);
        assert_eq!(s.bitrate_kbps, 3_000);
        assert_eq!(s.game_volume, 100);
        assert_eq!(s.mic_volume, 100);
        assert_eq!(s.eco_battery_threshold_pct, 5);
        assert_eq!(s.eco_ram_free_gbs, 32);

        // ...and the engine-facing side effects carry the clamped values.
        assert_eq!(effects.live_buffer_seconds, Some(5));
        assert_eq!(effects.volumes, Some((100, 100)));
    }

    #[test]
    fn a_patch_without_numeric_fields_produces_no_side_effects() {
        let mut s = defaults();
        let patch = SettingsPatch {
            codec: Some(Codec::Hevc),
            ..Default::default()
        };
        let effects = apply_patch_values(&patch, &mut s).unwrap();
        assert_eq!(effects, PatchEffects::default());
        assert_eq!(s.codec, Codec::Hevc);
    }

    #[test]
    fn topmost_and_privacy_are_reported_as_effects() {
        let mut s = defaults();
        let patch = SettingsPatch {
            always_on_top: Some(true),
            privacy_pause_when_unfocused: Some(false),
            ..Default::default()
        };
        let effects = apply_patch_values(&patch, &mut s).unwrap();
        assert_eq!(effects.topmost, Some(true));
        assert_eq!(effects.privacy_gate_on, Some(false));
    }

    #[test]
    fn hud_fields_are_persisted_but_inert() {
        // The HUD is deferred; toggling it must still round-trip through
        // settings.json so a later build can honour it.
        let mut s = defaults();
        let patch = SettingsPatch {
            hud_enabled: Some(true),
            ..Default::default()
        };
        let effects = apply_patch_values(&patch, &mut s).unwrap();
        assert!(s.hud_enabled);
        assert_eq!(effects, PatchEffects::default());
    }

    #[test]
    fn an_older_settings_file_still_loads_and_re_saves() {
        // Validation item: the GPUI build must accept a `settings.json` written
        // by the Tauri build. This blob is the 1.4.2 shape *minus* the fields
        // added later, so it also proves the `#[serde(default)]` attributes do
        // their job.
        let old = r#"{
            "bufferSeconds": 45,
            "targetFps": 120,
            "bitrateKbps": 24000,
            "codec": "h264",
            "monitorIndex": 1,
            "captureSystemAudio": true,
            "captureMicrophone": false,
            "outputDir": "C:\\Users\\me\\Videos\\ClipFlow",
            "hotkeySave": "Alt+C",
            "hotkeyToggle": "Alt+Shift+C",
            "openTrimmerAfterSave": true,
            "minimizeToTray": true,
            "autostartBuffer": false,
            "playSaveSound": true
        }"#;
        let parsed: Settings = serde_json::from_str(old).expect("old settings load");
        assert_eq!(parsed.buffer_seconds, 45);
        assert_eq!(parsed.hotkey_save, "Alt+C");
        assert_eq!(parsed.codec, Codec::H264);
        // Defaults fill in everything the old file did not have.
        assert_eq!(parsed.game_volume, 100);
        assert_eq!(parsed.eco_battery_threshold_pct, 30);
        assert!(parsed.organize_by_game, "defaults to true, as before");

        // And the round trip keeps the camelCase shape an older build reads.
        let resaved = serde_json::to_string(&parsed).unwrap();
        assert!(resaved.contains("\"bufferSeconds\":45"));
        assert!(resaved.contains("\"hotkeyToggle\":\"Alt+Shift+C\""));
        let reparsed: Settings = serde_json::from_str(&resaved).unwrap();
        assert_eq!(reparsed.buffer_seconds, parsed.buffer_seconds);
    }

    #[test]
    fn saving_without_an_armed_buffer_is_an_error_not_a_panic() {
        // Built by hand rather than with `AppState::new`, which persists the
        // seeded settings to the real `settings.json`. A test must never touch
        // the user's config.
        let state = AppState {
            engine: Arc::new(CaptureEngine::new()),
            settings: RwLock::new(defaults()),
            clip_meta: Mutex::new(empty_clip_meta()),
            flush_lock: Arc::new(parking_lot::Mutex::new(())),
        };
        // `game_override` bypasses the privacy check, so this reaches the
        // engine itself: an idle engine has no ring and must fail cleanly.
        let err = state
            .save_instant_replay(None, SaveTrigger::Ui, Some("cs2.exe".into()))
            .expect_err("idle engine cannot flush");
        assert!(!err.is_empty());
    }
}

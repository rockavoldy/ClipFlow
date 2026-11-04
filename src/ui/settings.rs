//! Settings panel: one control per `SettingsPatch` field.
//!
//! The panel used to be `SettingsPanel.tsx`. Its structure is preserved —
//! sections, presets, slider ranges — but the slider/stepper widgets are GPUI
//! component primitives and the hotkey recorder is the same click-then-press
//! badge, now driven by real `KeyDownEvent`s instead of a DOM `keydown`.
//!
//! **Deferred rows are hidden, not removed from the model.** `open_trimmer_after_save`,
//! `hud_enabled` and the ECO threshold controls stay persisted in
//! `settings.json` so a later build picks them up; the panel simply does not
//! show them, and the README documents the omission. Showing a switch that does
//! nothing would be worse than hiding it.
//!
//! Slider ranges are copied verbatim from the old panel:
//! `bitrate 3000..60000 step 1000`, `volumes 0..100 step 5`.

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::slider::{Slider, SliderState};
use gpui_kit::component::switch::Switch;
use gpui_kit::{
    Context, Entity, InteractiveElement, IntoElement, ParentElement,
    StatefulInteractiveElement, Styled, div,
};

use crate::power::PowerState;
use crate::state::SettingsPatch;
use crate::ui::deck::{Deck, HotkeyAction};
use crate::ui::format;
use crate::ui::theme::Palette;

/// Buffer presets from the old settings panel.
pub const BUFFER_PRESETS: [u32; 5] = [15, 30, 60, 120, 300];
/// FPS presets from the old settings panel.
pub const FPS_PRESETS: [u32; 4] = [30, 60, 120, 144];
/// Bitrate slider bounds, `step 1000`.
pub const BITRATE_MIN: u32 = 3_000;
pub const BITRATE_MAX: u32 = 60_000;
pub const BITRATE_STEP: u32 = 1_000;
/// Volume slider bounds, `step 5`.
pub const VOLUME_MIN: u32 = 0;
pub const VOLUME_MAX: u32 = 100;
pub const VOLUME_STEP: u32 = 5;

impl Deck {
    pub(crate) fn render_settings(
        &self,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let settings = &self.settings;

        div()
            .flex()
            .flex_col()
            .gap_4()
            .p_4()
            .rounded_lg()
            .bg(palette.panel)
            .border_1()
            .border_color(palette.line)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .font_family(crate::ui::theme::MONO)
                            .text_xs()
                            .text_color(palette.cyan)
                            .child("SETTINGS"),
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
                                    .child(format!(
                                        "{} · {} Mbps",
                                        settings.codec.as_str(),
                                        settings.bitrate_kbps / 1000
                                    )),
                            )
                            .child(
                                Button::new("back-to-deck")
                                    .secondary()
                                    .compact()
                                    .label("Back to deck")
                                    .tooltip("Return to the capture card and library")
                                    .on_click(cx.listener(|deck, _ev, _window, cx| {
                                        deck.view = crate::ui::deck::View::Deck;
                                        cx.notify();
                                    })),
                            ),
                    ),
            )
            // --- capture ---------------------------------------------------
            .child(self.section(
                palette,
                "BUFFER",
                div()
                    .flex()
                    .flex_row()
                    .flex_wrap()
                    .gap_2()
                    .children(BUFFER_PRESETS.into_iter().map(|preset| {
                        self.preset_button(
                            palette,
                            format!("buffer-{preset}"),
                            format!("{preset}s"),
                            settings.buffer_seconds == preset,
                            cx,
                            move |deck, cx| {
                                deck.apply_patch(
                                    SettingsPatch {
                                        buffer_seconds: Some(preset),
                                        ..Default::default()
                                    },
                                    cx,
                                );
                            },
                        )
                    })),
            ))
            .child(self.section(
                palette,
                "CAPTURE FPS",
                div()
                    .flex()
                    .flex_row()
                    .flex_wrap()
                    .gap_2()
                    .children(FPS_PRESETS.into_iter().map(|preset| {
                        self.preset_button(
                            palette,
                            format!("fps-{preset}"),
                            format!("{preset}"),
                            settings.target_fps == preset,
                            cx,
                            move |deck, cx| {
                                deck.apply_patch(
                                    SettingsPatch {
                                        target_fps: Some(preset),
                                        ..Default::default()
                                    },
                                    cx,
                                );
                            },
                        )
                    })),
            ))
            .child(self.section(
                palette,
                "CODEC",
                div()
                    .flex()
                    .flex_row()
                    .gap_2()
                    .children([crate::media::recorder::Codec::H264, crate::media::recorder::Codec::Hevc]
                        .into_iter()
                        .map(|codec| {
                            let active = settings.codec == codec;
                            let label = match codec {
                                crate::media::recorder::Codec::H264 => "H.264",
                                crate::media::recorder::Codec::Hevc => "HEVC",
                            };
                            self.preset_button(
                                palette,
                                format!("codec-{}", codec.as_str()),
                                label.to_string(),
                                active,
                                cx,
                                move |deck, cx| {
                                    deck.apply_patch(
                                        SettingsPatch {
                                            codec: Some(codec),
                                            ..Default::default()
                                        },
                                        cx,
                                    );
                                },
                            )
                        })),
            ))
            .child(self.section(
                palette,
                "DISPLAY",
                div()
                    .flex()
                    .flex_row()
                    .flex_wrap()
                    .gap_2()
                    .children(self.monitors.iter().enumerate().map(|(index, monitor)| {
                        let index = index as u32;
                        let active = settings.monitor_index == index;
                        self.preset_button(
                            palette,
                            format!("monitor-{index}"),
                            monitor_label(monitor, index),
                            active,
                            cx,
                            move |deck, cx| {
                                deck.apply_patch(
                                    SettingsPatch {
                                        monitor_index: Some(index),
                                        ..Default::default()
                                    },
                                    cx,
                                );
                            },
                        )
                    })),
            ))
            .child(self.slider_row(
                palette,
                "BITRATE",
                format!("{} kbps", settings.bitrate_kbps),
                self.bitrate_slider.clone(),
            ))
            .child(self.slider_row(
                palette,
                "GAME VOLUME",
                format!("{}%", settings.game_volume),
                self.game_volume_slider.clone(),
            ))
            .child(self.slider_row(
                palette,
                "MIC VOLUME",
                format!("{}%", settings.mic_volume),
                self.mic_volume_slider.clone(),
            ))
            .child(self.toggle_row(
                palette,
                "Capture system audio",
                "Record the game's own audio alongside the video.",
                settings.capture_system_audio,
                cx,
                |deck, value, cx| {
                    deck.apply_patch(
                        SettingsPatch {
                            capture_system_audio: Some(value),
                            ..Default::default()
                        },
                        cx,
                    )
                },
            ))
            .child(self.toggle_row(
                palette,
                "Capture microphone",
                "Mix your mic into the clip.",
                settings.capture_microphone,
                cx,
                |deck, value, cx| {
                    deck.apply_patch(
                        SettingsPatch {
                            capture_microphone: Some(value),
                            ..Default::default()
                        },
                        cx,
                    )
                },
            ))
            // --- hotkeys ---------------------------------------------------
            .child(self.section(
                palette,
                "HOTKEYS",
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(self.hotkey_row(
                        palette,
                        "Save replay",
                        &settings.hotkey_save,
                        HotkeyAction::Save,
                        cx,
                    ))
                    .child(self.hotkey_row(
                        palette,
                        "Arm / disarm",
                        &settings.hotkey_toggle,
                        HotkeyAction::Toggle,
                        cx,
                    )),
            ))
            // --- storage ---------------------------------------------------
            .child(self.section(
                palette,
                "CLIPS FOLDER",
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .font_family(crate::ui::theme::MONO)
                            .text_xs()
                            .text_color(palette.muted)
                            .truncate()
                            .child(settings.output_dir.clone()),
                    )
                    .child(
                        Button::new("choose-folder")
                            .secondary()
                            .compact()
                            .label("Change…")
                            .tooltip("Pick where clips are saved")
                            .on_click(cx.listener(|deck, _ev, _window, cx| {
                                deck.choose_output_folder(cx);
                            })),
                    ),
            ))
            .child(self.toggle_row(
                palette,
                "Organise by game",
                "Save each clip into a subfolder named after the game.",
                settings.organize_by_game,
                cx,
                |deck, value, cx| {
                    deck.apply_patch(
                        SettingsPatch {
                            organize_by_game: Some(value),
                            ..Default::default()
                        },
                        cx,
                    )
                },
            ))
            // --- behaviour ------------------------------------------------
            .child(self.toggle_row(
                palette,
                "Minimise to tray",
                "Closing the deck keeps recording in the background.",
                settings.minimize_to_tray,
                cx,
                |deck, value, cx| {
                    deck.apply_patch(
                        SettingsPatch {
                            minimize_to_tray: Some(value),
                            ..Default::default()
                        },
                        cx,
                    )
                },
            ))
            .child(self.toggle_row(
                palette,
                "Arm on launch",
                "Start the rolling buffer as soon as ClipFlow opens.",
                settings.autostart_buffer,
                cx,
                |deck, value, cx| {
                    deck.apply_patch(
                        SettingsPatch {
                            autostart_buffer: Some(value),
                            ..Default::default()
                        },
                        cx,
                    )
                },
            ))
            .child(self.toggle_row(
                palette,
                "Launch at startup",
                "Start ClipFlow hidden in the tray when Windows signs in.",
                settings.launch_at_startup,
                cx,
                |deck, value, cx| {
                    deck.apply_patch(
                        SettingsPatch {
                            launch_at_startup: Some(value),
                            ..Default::default()
                        },
                        cx,
                    )
                },
            ))
            .child(self.toggle_row(
                palette,
                "Always on top",
                "Keep the deck above other windows.",
                settings.always_on_top,
                cx,
                |deck, value, cx| {
                    deck.apply_patch(
                        SettingsPatch {
                            always_on_top: Some(value),
                            ..Default::default()
                        },
                        cx,
                    )
                },
            ))
            .child(self.toggle_row(
                palette,
                "Privacy mode",
                "Stop recording whenever no game has focus, so clips never contain your desktop.",
                settings.privacy_pause_when_unfocused,
                cx,
                |deck, value, cx| {
                    deck.apply_patch(
                        SettingsPatch {
                            privacy_pause_when_unfocused: Some(value),
                            ..Default::default()
                        },
                        cx,
                    )
                },
            ))
            .child(self.toggle_row(
                palette,
                "Play a sound on save",
                "A short chime confirms the clip landed while you are still in-game.",
                settings.play_save_sound,
                cx,
                |deck, value, cx| {
                    deck.apply_patch(
                        SettingsPatch {
                            play_save_sound: Some(value),
                            ..Default::default()
                        },
                        cx,
                    )
                },
            ))
            .child(self.toggle_row(
                palette,
                "Auto-save on game exit",
                "Flush the last 30 seconds when a game loses focus.",
                settings.autosave_on_game_exit,
                cx,
                |deck, value, cx| {
                    deck.apply_patch(
                        SettingsPatch {
                            autosave_on_game_exit: Some(value),
                            ..Default::default()
                        },
                        cx,
                    )
                },
            ))
            .child(self.toggle_row(
                palette,
                "Adaptive capture (ECO)",
                "Shrink the buffer on battery or under memory pressure.",
                settings.adaptive_eco,
                cx,
                |deck, value, cx| {
                    deck.apply_patch(
                        SettingsPatch {
                            adaptive_eco: Some(value),
                            ..Default::default()
                        },
                        cx,
                    )
                },
            ))
            // --- maintenance ---------------------------------------------
            .child(self.section(
                palette,
                "MAINTENANCE",
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(
                        Button::new("retention")
                            .secondary()
                            .compact()
                            .label(retention_label(settings.auto_cleanup_days))
                            .tooltip("Delete clips older than this at launch")
                            .on_click(cx.listener(|deck, _ev, _window, cx| {
                                let next = next_retention(deck.settings.auto_cleanup_days);
                                deck.apply_patch(
                                    SettingsPatch {
                                        auto_cleanup_days: Some(next),
                                        ..Default::default()
                                    },
                                    cx,
                                );
                            })),
                    )
                    .child(
                        Button::new("delete-all")
                            .danger()
                            .compact()
                            .label("Delete all clips")
                            .tooltip("Remove every clip in the folder. Cannot be undone.")
                            .on_click(cx.listener(|deck, _ev, _window, cx| {
                                deck.delete_all_clips(cx);
                            })),
                    )
                    .child(
                        Button::new("simulate-loss")
                            .ghost()
                            .compact()
                            .label("Simulate device loss")
                            .tooltip("Exercise the encoder's recovery path")
                            .on_click(cx.listener(|deck, _ev, _window, cx| {
                                deck.simulate_device_loss(cx);
                            })),
                    ),
            ))
            .children(self.power_telemetry(palette, self.power.as_ref()))
            // --- about ------------------------------------------------------
            .child(self.section(
                palette,
                "ABOUT",
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .font_family(crate::ui::theme::MONO)
                                    .text_xs()
                                    .text_color(palette.muted)
                                    .child(format!("ClipFlow v{}", env!("CARGO_PKG_VERSION"))),
                            )
                            .child(
                                div()
                                    .font_family(crate::ui::theme::MONO)
                                    .text_xs()
                                    .text_color(if self.update_error {
                                        palette.danger
                                    } else {
                                        palette.muted
                                    })
                                    .child(self.update_status.clone()),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .child(
                                Button::new("check-updates")
                                    .secondary()
                                    .compact()
                                    .label("Check for updates")
                                    .tooltip("Compare with the newest release on GitHub")
                                    .on_click(cx.listener(|deck, _ev, _window, cx| {
                                        deck.check_for_updates(cx);
                                    })),
                            )
                            .child(
                                Button::new("open-releases")
                                    .ghost()
                                    .compact()
                                    .label("Releases page")
                                    .tooltip("Open the download page in your browser")
                                    .on_click(cx.listener(|_deck, _ev, _window, _cx| {
                                        if let Err(e) =
                                            crate::platform::update::open_releases_page()
                                        {
                                            log::warn!("[clipflow] {e}");
                                        }
                                    })),
                            ),
                    ),
            ))
    }

    // -----------------------------------------------------------------------
    // Row builders
    // -----------------------------------------------------------------------

    fn section(
        &self,
        palette: Palette,
        title: &str,
        body: impl IntoElement,
    ) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .font_family(crate::ui::theme::MONO)
                    .text_xs()
                    .text_color(palette.muted)
                    .child(title.to_string()),
            )
            .child(body)
    }

    #[allow(clippy::too_many_arguments)]
    fn preset_button(
        &self,
        palette: Palette,
        id: String,
        label: String,
        active: bool,
        cx: &mut Context<Self>,
        on_pick: impl Fn(&mut Self, &mut Context<Self>) + 'static,
    ) -> impl IntoElement {
        let _ = palette;
        let mut button = Button::new(id).compact().label(label);
        button = if active {
            button.primary()
        } else {
            button.secondary()
        };
        button.on_click(cx.listener(move |deck, _ev, _window, cx| on_pick(deck, cx)))
    }

    fn toggle_row(
        &self,
        palette: Palette,
        title: &str,
        description: &str,
        value: bool,
        cx: &mut Context<Self>,
        on_change: impl Fn(&mut Self, bool, &mut Context<Self>) + 'static,
    ) -> impl IntoElement {
        let id = format!("switch-{}", slug(title));
        div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap_3()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap_1()
                    .child(
                        div()
                            .text_sm()
                            .text_color(palette.text)
                            .child(title.to_string()),
                    )
                    .child(
                        div()
                            .font_family(crate::ui::theme::MONO)
                            .text_xs()
                            .text_color(palette.muted)
                            .child(description.to_string()),
                    ),
            )
            .child(
                Switch::new(id)
                    .checked(value)
                    .on_change(cx.listener(move |deck, checked: &bool, _window, cx| {
                        on_change(deck, *checked, cx);
                    })),
            )
    }

    /// A slider plus its live value.
    ///
    /// The entity lives on the deck (created once in `Deck::new`) because a
    /// `SliderState` must outlive a frame, and the *release* event is what
    /// persists — the old panel used `onChange` on a range input, which fired
    /// continuously and made `settings.save()` chatty.
    fn slider_row(
        &self,
        palette: Palette,
        title: &str,
        value_label: String,
        state: Entity<SliderState>,
    ) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .font_family(crate::ui::theme::MONO)
                            .text_xs()
                            .text_color(palette.muted)
                            .child(title.to_string()),
                    )
                    .child(
                        div()
                            .font_family(crate::ui::theme::MONO)
                            .text_xs()
                            .text_color(palette.cyan)
                            .child(value_label),
                    ),
            )
            .child(Slider::new(&state).horizontal())
    }

    fn hotkey_row(
        &self,
        palette: Palette,
        title: &str,
        accelerator: &str,
        action: HotkeyAction,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let recording = self.recording_hotkey == Some(action);
        let label = self.hotkey_badge_label(action, accelerator);

        div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap_3()
            .child(
                div()
                    .text_sm()
                    .text_color(palette.text)
                    .child(title.to_string()),
            )
            .child(
                div()
                    .id(format!("hotkey-{}", slug(title)))
                    .px_3()
                    .py_1()
                    .rounded_md()
                    .border_1()
                    .border_color(if recording { palette.lime } else { palette.magenta })
                    .bg(if recording {
                        palette.panel_2
                    } else {
                        palette.panel
                    })
                    .cursor_pointer()
                    .font_family(crate::ui::theme::MONO)
                    .text_xs()
                    .text_color(if recording { palette.lime } else { palette.magenta })
                    .child(label)
                    .on_click(cx.listener(move |deck, _ev, window, cx| {
                        deck.begin_hotkey_capture(action, window, cx);
                    })),
            )
    }

    fn power_telemetry(
        &self,
        palette: Palette,
        power: Option<&PowerState>,
    ) -> Option<impl IntoElement> {
        let power = power?;
        let text = format!(
            "{} · {}% · {} free",
            if power.on_battery { "Battery" } else { "AC" },
            power.battery_percent,
            format::gib(power.available_ram_bytes)
        );
        let eco = if self.settings.adaptive_eco {
            let input = crate::policy::eco::EcoInput::from_settings(&self.settings);
            let decision = crate::policy::eco::decide_eco(power, &input);
            format!("ECO {}", if decision.active { decision.reason.as_str() } else { "idle" })
        } else {
            "ECO off".to_string()
        };
        Some(
            div()
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .px_2()
                .py_1()
                .rounded_sm()
                .bg(palette.panel_2)
                .child(
                    div()
                        .font_family(crate::ui::theme::MONO)
                        .text_xs()
                        .text_color(palette.muted)
                        .child(text),
                )
                .child(
                    div()
                        .font_family(crate::ui::theme::MONO)
                        .text_xs()
                        .text_color(palette.amber)
                        .child(eco),
                ),
        )
    }
}

/// Builds the `SliderState` builder for one numeric row. The caller wraps it in
/// an entity: `cx.new(|_| slider_state(min, max, step, value))`.
pub fn slider_state(
    min: u32,
    max: u32,
    step: u32,
    value: u32,
) -> SliderState {
    SliderState::new()
        .min(min as f32)
        .max(max as f32)
        .step(step as f32)
        .default_value(value as f32)
}

/// Click-to-cycle retention values, matching `CLEANUP_PRESETS` plus OFF.
const RETENTION_DAYS: [u32; 5] = [0, 7, 14, 30, 60];

/// "Display 1 · 2560×1440" so two same-size monitors are still distinguishable.
fn monitor_label(monitor: &crate::media::recorder::MonitorInfo, index: u32) -> String {
    format!(
        "{} · {}×{}",
        index + 1,
        monitor.width,
        monitor.height
    )
}

fn retention_label(days: u32) -> String {
    if days == 0 {
        "Keep all".to_string()
    } else {
        format!("{days} days")
    }
}

fn next_retention(current: u32) -> u32 {
    let pos = RETENTION_DAYS.iter().position(|d| *d == current).unwrap_or(0);
    RETENTION_DAYS[(pos + 1) % RETENTION_DAYS.len()]
}

fn slug(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_match_the_old_settings_panel() {
        // Copied from `SettingsPanel.tsx`; if these drift the deck stops
        // offering the values existing users expect to find.
        assert_eq!(BUFFER_PRESETS, [15, 30, 60, 120, 300]);
        assert_eq!(FPS_PRESETS, [30, 60, 120, 144]);
        assert_eq!((BITRATE_MIN, BITRATE_MAX, BITRATE_STEP), (3_000, 60_000, 1_000));
        assert_eq!((VOLUME_MIN, VOLUME_MAX, VOLUME_STEP), (0, 100, 5));
    }

    #[test]
    fn retention_cycles_through_the_presets_and_wraps() {
        let mut days = 0;
        let mut seen = vec![days];
        for _ in 0..RETENTION_DAYS.len() - 1 {
            days = next_retention(days);
            seen.push(days);
        }
        assert_eq!(seen, RETENTION_DAYS.to_vec());
        // Wraps back to "keep all".
        assert_eq!(next_retention(*RETENTION_DAYS.last().unwrap()), 0);
    }

    #[test]
    fn an_unknown_retention_value_resets_to_keep_all() {
        // A hand-edited settings.json could hold anything.
        assert_eq!(next_retention(999), RETENTION_DAYS[1]);
    }

    #[test]
    fn retention_labels_are_human_readable() {
        assert_eq!(retention_label(0), "Keep all");
        assert_eq!(retention_label(7), "7 days");
        assert_eq!(retention_label(60), "60 days");
    }

    #[test]
    fn slugs_are_stable_widget_ids() {
        assert_eq!(slug("Arm / disarm"), "arm---disarm");
        assert_eq!(slug("Capture system audio"), "capture-system-audio");
    }
}

//! Capture card: the pure half.
//!
//! Everything the card *decides* lives here so it is testable without a
//! window: whether SAVE is enabled, why it is not, and how a stats snapshot is
//! turned into the telemetry rows.
//!
//! The `canSave` rule and its tooltips are lifted from `HudOverlay.tsx`, which
//! established them: SAVE is disabled while the buffer is not armed, while
//! privacy mode has gated capture, and while a flush is already running — and
//! each case gets a *specific* tooltip, because a greyed-out button with no
//! explanation reads as a bug.

use gpui_kit::base::Disableable;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::{Context, IntoElement, ParentElement, Styled, div, px};

use crate::media::recorder::{Codec, EngineState, EngineStats};
use crate::ui::deck::{Deck, Intent};
use crate::ui::theme::Palette;

/// Why SAVE is disabled. `None` in the `Result` means it is enabled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveBlock {
    /// A flush is already in flight.
    Busy,
    /// The engine is not buffering.
    NotArmed,
    /// Privacy mode is gating capture right now.
    PrivacyGated,
}

impl SaveBlock {
    /// The tooltip text. Exactly one sentence, actionable.
    pub fn reason(self) -> &'static str {
        match self {
            SaveBlock::Busy => "A save is already running.",
            SaveBlock::NotArmed => "Arm the buffer first.",
            SaveBlock::PrivacyGated => "Privacy mode paused recording — focus a game.",
        }
    }
}

/// The `canSave` decision.
///
/// Order matters: `Busy` is reported first because it is the state the user
/// just caused and can act on, and `NotArmed` before `PrivacyGated` because an
/// idle engine has no ring to gate in the first place.
pub fn save_block(stats: &EngineStats, busy: bool) -> Option<SaveBlock> {
    if busy {
        return Some(SaveBlock::Busy);
    }
    if !is_armed(stats.state) {
        return Some(SaveBlock::NotArmed);
    }
    if stats.privacy_active {
        return Some(SaveBlock::PrivacyGated);
    }
    None
}

/// Buffering and flushing are both "armed": a flush still holds the ring.
pub fn is_armed(state: EngineState) -> bool {
    matches!(state, EngineState::Buffering | EngineState::Flushing)
}

/// Short label for the armed pill in the titlebar.
pub fn state_label(state: EngineState) -> &'static str {
    match state {
        EngineState::Idle => "IDLE",
        EngineState::Starting => "STARTING",
        EngineState::Buffering => "ARMED",
        EngineState::Flushing => "SAVING",
        EngineState::Recovering => "RECOVERING",
        EngineState::Error => "ERROR",
    }
}

/// One telemetry row: label, value, and whether the value is healthy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub label: &'static str,
    pub value: String,
    pub warn: bool,
}

/// Builds the telemetry list the deck shows under the capture buttons.
///
/// The field set comes from `IDLE_STATS` / `PipelinePanel` in `App.tsx`; the
/// values are formatted through [`crate::ui::format`] so they match the old
/// deck's strings.
pub fn telemetry(stats: &EngineStats) -> Vec<Row> {
    let mut rows = Vec::with_capacity(9);

    rows.push(Row {
        label: "ENCODER",
        // `EncoderInfo::friendly_name` already reads "Not initialised" when
        // there is no encoder, so no special case is needed.
        value: format!("{} · {}", stats.encoder.friendly_name, stats.encoder.codec),
        warn: !stats.encoder.hardware,
    });

    rows.push(Row {
        label: "RESOLUTION",
        value: if stats.width == 0 {
            "—".to_string()
        } else {
            format!("{}×{}", stats.width, stats.height)
        },
        warn: false,
    });

    rows.push(Row {
        label: "CAPTURE",
        value: format!("{:.0} / {} fps", stats.capture_fps, stats.target_fps),
        warn: stats.capture_fps + 1.0 < stats.target_fps as f32,
    });

    rows.push(Row {
        label: "BITRATE",
        value: if stats.bitrate_kbps == 0 {
            "—".to_string()
        } else {
            crate::ui::format::bitrate_mbps(stats.bitrate_kbps)
        },
        warn: false,
    });

    rows.push(Row {
        label: "RING",
        value: format!(
            "{} / {}s",
            crate::ui::format::duration(stats.buffered_seconds),
            stats.buffer_seconds
        ),
        warn: false,
    });

    rows.push(Row {
        label: "MEMORY",
        value: crate::ui::format::bytes(stats.process_rss_bytes),
        warn: false,
    });

    rows.push(Row {
        label: "ENCODE",
        value: format!("{:.1} ms", stats.encode_ms_avg),
        warn: stats.encode_ms_avg > 16.0,
    });

    rows.push(Row {
        label: "DROPPED",
        value: stats.dropped_frames.to_string(),
        warn: stats.dropped_frames > 0,
    });

    // Audio gets one row that names the failure rather than a raw flag, because
    // "audio_error" is the only thing a user can act on (close the exclusive
    // device).
    rows.push(Row {
        label: "AUDIO",
        value: match &stats.audio_error {
            Some(e) => e.clone(),
            None => match (stats.audio_system, stats.audio_mic) {
                (true, true) => "Game + Mic".to_string(),
                (true, false) => "Game".to_string(),
                (false, true) => "Mic".to_string(),
                (false, false) => "Muted".to_string(),
            },
        },
        warn: stats.audio_error.is_some(),
    });

    rows
}

/// Encoder badge text, matching `VENDOR_LABEL` from `src/lib/types.ts`.
pub fn vendor_label(stats: &EngineStats) -> &'static str {
    use crate::media::recorder::EncoderVendor::*;
    match stats.encoder.vendor {
        Nvenc => "NVENC",
        AmdAmf => "AMF",
        IntelQsv => "QSV",
        MediaFoundationSoftware => "MF",
        Unavailable => "—",
    }
}

/// Codec toggle label.
pub fn codec_label(codec: Codec) -> &'static str {
    codec.as_str()
}

/// "save last 30 s" is only offered when the ring is longer than 30 s: on a
/// 15 s buffer the button would be a lie.
pub fn can_save_last_30(stats: &EngineStats) -> bool {
    is_armed(stats.state) && stats.buffer_seconds > 30
}

// ---------------------------------------------------------------------------
// Render
// ---------------------------------------------------------------------------

impl Deck {
    pub(crate) fn render_capture_section(
        &self,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let armed = is_armed(self.stats.state);
        let block = save_block(&self.stats, self.busy);

        // A real `disabled` button cannot show a tooltip in GPUI (the element
        // never receives hover), so the reason is rendered as a visible line
        // under the row instead of relying on hover — the same lesson the
        // WebView2 HUD learned.
        let save_tooltip = block.map(|b| b.reason()).unwrap_or("Save the whole buffer");

        let primary = if armed {
            Button::new("disarm")
                .secondary()
                .label("DISARM")
                .tooltip("Stop recording the rolling buffer")
                .on_click(cx.listener(|deck, _ev, _window, cx| {
                    deck.handle_intent(Intent::Disarm, cx);
                }))
        } else {
            Button::new("arm")
                .primary()
                .label("ARM")
                .tooltip("Start recording the rolling buffer")
                .on_click(cx.listener(|deck, _ev, _window, cx| {
                    deck.handle_intent(Intent::Arm, cx);
                }))
        };

        let mut save = Button::new("save")
            .label("SAVE")
            .tooltip(save_tooltip)
            .on_click(cx.listener(|deck, _ev, _window, cx| {
                deck.handle_intent(Intent::SaveReplay, cx);
            }));
        if block.is_some() {
            save = save.disabled(true);
        } else {
            save = save.danger();
        }

        let mut last30 = Button::new("save-30")
            .secondary()
            .label("LAST 30 s")
            .tooltip("Save only the most recent 30 seconds")
            .on_click(cx.listener(|deck, _ev, _window, cx| {
                deck.handle_intent(Intent::SaveLast30, cx);
            }));
        if !can_save_last_30(&self.stats) || block.is_some() {
            last30 = last30.disabled(true);
        }

        let rows = telemetry(&self.stats);

        div()
            .flex()
            .flex_col()
            .gap_3()
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
                            .child("CAPTURE"),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_3()
                            // Privacy badge: the deck is deliberately not
                            // recording desktop content right now. Same
                            // "PRIV" chip the old HUD showed.
                            .children(self.stats.privacy_active.then(|| {
                                div()
                                    .px_2()
                                    .rounded_sm()
                                    .bg(palette.panel_2)
                                    .font_family(crate::ui::theme::MONO)
                                    .text_xs()
                                    .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                    .text_color(palette.amber)
                                    .child("🔒 PRIV")
                            }))
                            .child(
                                div()
                                    .font_family(crate::ui::theme::MONO)
                                    .text_xs()
                                    .text_color(palette.muted)
                                    .child(format!(
                                        "{} · {}",
                                        vendor_label(&self.stats),
                                        self.stats.encoder.codec
                                    )),
                            ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .h(px(38.0))
                    .child(primary)
                    .child(save)
                    .child(last30),
            )
            // The disabled reason, when there is one. This replaces the tooltip
            // that a disabled button cannot show.
            .children(block.map(|b| {
                div()
                    .font_family(crate::ui::theme::MONO)
                    .text_xs()
                    .text_color(palette.amber)
                    .child(b.reason())
            }))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_wrap()
                    .gap_x(px(6.0))
                    .gap_y(px(2.0))
                    .children(rows.into_iter().map(|row| {
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .px_2()
                            .py_1()
                            .child(
                                div()
                                    .font_family(crate::ui::theme::MONO)
                                    .text_xs()
                                    .text_color(palette.muted)
                                    .child(row.label),
                            )
                            .child(
                                div()
                                    .font_family(crate::ui::theme::MONO)
                                    .text_xs()
                                    .text_color(if row.warn {
                                        palette.amber
                                    } else {
                                        palette.text
                                    })
                                    .child(row.value),
                            )
                    })),
            )
    }

}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::recorder::EncoderInfo;

    fn idle() -> EngineStats {
        EngineStats {
            state: EngineState::Idle,
            encoder: EncoderInfo::default(),
            width: 0,
            height: 0,
            target_fps: 60,
            bitrate_kbps: 0,
            capture_fps: 0.0,
            buffer_seconds: 60,
            buffered_seconds: 0.0,
            ring_bytes: 0,
            ring_frames: 0,
            process_rss_bytes: 17 * 1024 * 1024,
            encode_ms_avg: 0.0,
            gpu_submit_ms_avg: 0.0,
            dropped_frames: 0,
            device_resets: 0,
            audio_system: true,
            audio_mic: false,
            audio_drift_ms: 0.0,
            audio_error: None,
            privacy_active: false,
            uptime_seconds: 0,
            last_error: None,
        }
    }

    fn armed() -> EngineStats {
        let mut s = idle();
        s.state = EngineState::Buffering;
        s.width = 2560;
        s.height = 1440;
        s.bitrate_kbps = 12_000;
        s.capture_fps = 59.8;
        s.buffered_seconds = 30.0;
        s
    }

    #[test]
    fn save_is_disabled_until_armed() {
        assert_eq!(save_block(&idle(), false), Some(SaveBlock::NotArmed));
        assert_eq!(save_block(&armed(), false), None);
    }

    #[test]
    fn flushing_still_counts_as_armed() {
        let mut s = armed();
        s.state = EngineState::Flushing;
        assert!(is_armed(s.state));
        assert_eq!(save_block(&s, false), None);
    }

    #[test]
    fn privacy_gate_disables_save_with_its_own_reason() {
        let mut s = armed();
        s.privacy_active = true;
        assert_eq!(save_block(&s, false), Some(SaveBlock::PrivacyGated));
        assert!(SaveBlock::PrivacyGated.reason().contains("focus a game"));
    }

    #[test]
    fn busy_wins_over_every_other_reason() {
        let mut s = armed();
        s.privacy_active = true;
        assert_eq!(save_block(&s, true), Some(SaveBlock::Busy));
    }

    #[test]
    fn every_block_has_a_specific_actionable_reason() {
        // A greyed button with no explanation reads as a bug, so no two reasons
        // may collapse to the same string.
        let all = [
            SaveBlock::Busy.reason(),
            SaveBlock::NotArmed.reason(),
            SaveBlock::PrivacyGated.reason(),
        ];
        assert_eq!(all.len(), 3);
        assert_ne!(all[0], all[1]);
        assert_ne!(all[1], all[2]);
        assert_ne!(all[0], all[2]);
        assert!(all.iter().all(|r| !r.is_empty()));
    }

    #[test]
    fn telemetry_reports_idle_as_em_dashes_not_zeros() {
        let rows = telemetry(&idle());
        let get = |label: &str| rows.iter().find(|r| r.label == label).unwrap();

        assert_eq!(get("RESOLUTION").value, "—");
        assert_eq!(get("BITRATE").value, "—");
        // "Not initialised" comes from `EncoderInfo::default`.
        assert!(get("ENCODER").value.contains("Not initialised"));
        assert!(get("ENCODER").warn, "no hardware encoder is a warning");
    }

    #[test]
    fn telemetry_formats_a_live_engine_like_the_old_deck() {
        let rows = telemetry(&armed());
        let get = |label: &str| rows.iter().find(|r| r.label == label).unwrap();

        assert_eq!(get("RESOLUTION").value, "2560×1440");
        assert_eq!(get("BITRATE").value, "12.0 Mbps");
        assert_eq!(get("RING").value, "0:30 / 60s");
        assert_eq!(get("MEMORY").value, "17.0 MB");
        assert!(!get("CAPTURE").warn, "59.8 fps is close enough to 60");
    }

    #[test]
    fn a_dropped_frame_is_flagged() {
        let mut s = armed();
        s.dropped_frames = 3;
        let dropped = telemetry(&s)
            .into_iter()
            .find(|r| r.label == "DROPPED")
            .unwrap();
        assert!(dropped.warn);
        assert_eq!(dropped.value, "3");
    }

    #[test]
    fn a_slow_encode_is_flagged() {
        let mut s = armed();
        s.encode_ms_avg = 22.5;
        let encode = telemetry(&s)
            .into_iter()
            .find(|r| r.label == "ENCODE")
            .unwrap();
        assert!(encode.warn);
        assert_eq!(encode.value, "22.5 ms");
    }

    #[test]
    fn audio_failure_names_the_problem_instead_of_a_flag() {
        let mut s = armed();
        s.audio_error = Some("device held in exclusive mode".into());
        let audio = telemetry(&s)
            .into_iter()
            .find(|r| r.label == "AUDIO")
            .unwrap();
        assert!(audio.warn);
        assert_eq!(audio.value, "device held in exclusive mode");
    }

    #[test]
    fn audio_sources_are_summarised() {
        let mut s = armed();
        s.audio_system = true;
        s.audio_mic = true;
        let audio = |s: &EngineStats| {
            telemetry(s)
                .into_iter()
                .find(|r| r.label == "AUDIO")
                .unwrap()
                .value
        };
        assert_eq!(audio(&s), "Game + Mic");
        s.audio_mic = false;
        assert_eq!(audio(&s), "Game");
        s.audio_system = false;
        s.audio_mic = true;
        assert_eq!(audio(&s), "Mic");
        s.audio_mic = false;
        assert_eq!(audio(&s), "Muted");
    }

    #[test]
    fn save_last_30_needs_a_buffer_longer_than_30s() {
        let mut s = armed();
        s.buffer_seconds = 60;
        assert!(can_save_last_30(&s));
        // Exactly 30 s would be a no-op, not a "last 30 s".
        s.buffer_seconds = 30;
        assert!(!can_save_last_30(&s));
        s.buffer_seconds = 120;
        assert!(can_save_last_30(&s));
        // Never offered while idle, whatever the configured length.
        s.state = EngineState::Idle;
        assert!(!can_save_last_30(&s));
    }

    #[test]
    fn state_labels_are_uppercase_and_short() {
        assert_eq!(state_label(EngineState::Buffering), "ARMED");
        assert_eq!(state_label(EngineState::Flushing), "SAVING");
        assert_eq!(state_label(EngineState::Idle), "IDLE");
        for state in [
            EngineState::Idle,
            EngineState::Starting,
            EngineState::Buffering,
            EngineState::Flushing,
            EngineState::Recovering,
            EngineState::Error,
        ] {
            let label = state_label(state);
            assert!(label.len() <= 10, "{label} is too long for the pill");
            assert_eq!(label, label.to_ascii_uppercase());
        }
    }

    #[test]
    fn vendor_badges_match_the_old_vendor_map() {
        use crate::media::recorder::EncoderVendor;
        let mut s = idle();
        let label = |s: &EngineStats| vendor_label(s);
        s.encoder.vendor = EncoderVendor::Nvenc;
        assert_eq!(label(&s), "NVENC");
        s.encoder.vendor = EncoderVendor::AmdAmf;
        assert_eq!(label(&s), "AMF");
        s.encoder.vendor = EncoderVendor::IntelQsv;
        assert_eq!(label(&s), "QSV");
        s.encoder.vendor = EncoderVendor::MediaFoundationSoftware;
        assert_eq!(label(&s), "MF");
        s.encoder.vendor = EncoderVendor::Unavailable;
        assert_eq!(label(&s), "—");
    }
}

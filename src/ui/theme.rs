//! Neutral, near-monochrome visual language.
//!
//! The deck is a plain Windows utility: white content, light-gray chrome,
//! one-pixel borders and dark-gray text. There is no brand colour and no
//! accent tint — active controls are near-black, inactive ones are gray.
//! Hue is reserved for things a user has to notice: the armed indicator and
//! errors (`ok` / `warn` / `danger`), and even those are the restrained
//! Windows system shades rather than neon.
//!
//! Fonts are the ones every Windows install already has: Segoe UI and
//! Consolas. Nothing is bundled and nothing is downloaded.

use gpui_kit::component::Theme;
use gpui_kit::{App, FontWeight, Hsla, px, rgb};

pub const BACKGROUND: u32 = 0xffffff;
pub const CHROME: u32 = 0xf3f3f3;
pub const SURFACE: u32 = 0xffffff;
pub const SUBTLE: u32 = 0xf2f2f2;
pub const SELECTED: u32 = 0xe5e5e5;
pub const LINE: u32 = 0xdedede;
pub const TEXT: u32 = 0x1a1a1a;
pub const MUTED: u32 = 0x666666;
pub const DISABLED: u32 = 0xa0a0a0;
pub const ACCENT: u32 = 0x202020;
pub const OK: u32 = 0x107c10;
pub const WARN: u32 = 0x8a5a00;
pub const DANGER: u32 = 0xc42b1c;

pub const SANS: &str = "Segoe UI";
pub const MONO: &str = "Consolas";

/// Semantic colors resolved once per frame so the render path does no hex math.
#[derive(Debug, Clone, Copy)]
pub struct Palette {
    /// Main window / content background.
    pub background: Hsla,
    /// Titlebar and footer.
    pub chrome: Hsla,
    /// Cards and panels.
    pub surface: Hsla,
    /// Hover fills and inactive chips.
    pub subtle: Hsla,
    /// Selected rows and active chips.
    pub selected: Hsla,
    pub line: Hsla,
    pub text: Hsla,
    pub muted: Hsla,
    pub disabled: Hsla,
    /// Near-black emphasis for filled buttons and active labels.
    pub accent: Hsla,
    pub ok: Hsla,
    pub warn: Hsla,
    pub danger: Hsla,
}

/// `rgb()` yields `Rgba`; theme slots want `Hsla`. One conversion helper keeps
/// every constant above readable as a hex literal.
fn hsl(hex: u32) -> Hsla {
    Hsla::from(rgb(hex))
}

impl Palette {
    pub fn new() -> Self {
        Self {
            background: hsl(BACKGROUND),
            chrome: hsl(CHROME),
            surface: hsl(SURFACE),
            subtle: hsl(SUBTLE),
            selected: hsl(SELECTED),
            line: hsl(LINE),
            text: hsl(TEXT),
            muted: hsl(MUTED),
            disabled: hsl(DISABLED),
            accent: hsl(ACCENT),
            ok: hsl(OK),
            warn: hsl(WARN),
            danger: hsl(DANGER),
        }
    }

    /// Color for a status tone: only a failure or a warning gets hue.
    pub fn tone(&self, tone: Tone) -> Hsla {
        match tone {
            Tone::Ok => self.ok,
            Tone::Warn => self.warn,
            Tone::Err => self.danger,
            Tone::Info => self.text,
        }
    }

    /// Color for an engine state, used by the armed pill and the status line.
    pub fn state(&self, state: ArmedState) -> Hsla {
        match state {
            ArmedState::Armed => self.ok,
            ArmedState::Starting => self.warn,
            ArmedState::Busy => self.text,
            ArmedState::Idle => self.muted,
            ArmedState::Error => self.danger,
        }
    }
}

impl Default for Palette {
    fn default() -> Self {
        Self::new()
    }
}

/// Status tones for the one-line status strip. Kept because the copy/tone pairing is a user-visible contract
/// ("saved" is positive, "privacy blocked" is an error).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Ok,
    Warn,
    Err,
    Info,
}

/// Coarse armed state for pill/status coloring. Derived from `EngineStats`,
/// never stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArmedState {
    Armed,
    Starting,
    Busy,
    Idle,
    Error,
}

/// Applies the neutral palette to gpui-component's theme so any stock widget
/// (buttons, inputs, menus) inherits the same restrained look.
///
/// The light mode is forced: the deck is a light-only design, and loading the
/// system's dark palette first would leave half the stock widgets dark.
pub fn init(cx: &mut App) {
    // `change` loads a registered theme and replaces the colors, so the edits
    // must happen in a second pass.
    Theme::change(gpui_kit::component::ThemeMode::Light, None, cx);

    Theme::update(cx, |theme| {
        theme.font_family = SANS.into();
        theme.mono_font_family = MONO.into();
        theme.font_size = px(13.0);
        theme.mono_font_size = px(12.0);
        theme.radius = px(4.0);
        theme.radius_lg = px(6.0);
        theme.shadow = false;

        let p = Palette::new();
        theme.background = p.background;
        theme.foreground = p.text;
        theme.border = p.line;
        theme.muted = p.subtle;
        theme.muted_foreground = p.muted;
        theme.primary = p.accent;
        theme.primary_foreground = p.background;
        theme.primary_hover = hsl(0x333333);
        theme.primary_active = hsl(0x000000);
        theme.secondary = p.subtle;
        theme.secondary_foreground = p.text;
        theme.secondary_hover = hsl(0xe8e8e8);
        theme.secondary_active = p.selected;
        theme.accent = p.subtle;
        theme.accent_foreground = p.text;
        theme.danger = p.danger;
        theme.danger_foreground = p.background;
        theme.danger_hover = hsl(0xa82316);
        theme.success = p.ok;
        theme.success_foreground = p.background;
        theme.warning = p.warn;
        theme.warning_foreground = p.background;
        theme.info = p.text;
        theme.info_foreground = p.background;
        theme.ring = p.muted;
        theme.selection = p.selected;
        theme.caret = p.text;
        theme.input = p.background;
        theme.link = p.text;
        theme.link_hover = p.muted;
        theme.link_active = p.text;
        theme.title_bar = p.chrome;
        theme.title_bar_border = p.line;
        theme.status_bar = p.chrome;
        theme.status_bar_border = p.line;
        theme.list_hover = p.subtle;
        theme.list_active = p.selected;
        theme.list_active_border = p.line;
        theme.popover = p.background;
        theme.popover_foreground = p.text;
        theme.group_box = p.chrome;
        theme.group_box_foreground = p.text;
        theme.skeleton = p.subtle;
        theme.slider_bar = p.accent;
        theme.slider_thumb = p.background;
        theme.progress_bar = p.accent;
        theme.switch = hsl(0xc8c8c8);
        theme.switch_thumb = p.background;
        theme.scrollbar_thumb = hsl(0xc0c0c0);
        theme.scrollbar_thumb_hover = hsl(0xa0a0a0);
        theme.window_border = p.line;
        theme.tab_bar = p.chrome;
        theme.tab = p.background;
        theme.tab_active = p.surface;
        theme.tab_active_foreground = p.text;
        theme.tab_foreground = p.muted;

        // Stock buttons read the component tokens, not the semantic colors, so
        // they are set explicitly rather than relying on a fallback.
        theme.button = p.subtle;
        theme.button_hover = hsl(0xe8e8e8);
        theme.button_active = p.selected;
        theme.button_foreground = p.text;
        theme.button_primary = p.accent;
        theme.button_primary_hover = hsl(0x333333);
        theme.button_primary_active = hsl(0x000000);
        theme.button_primary_foreground = p.background;
        theme.button_secondary = p.subtle;
        theme.button_secondary_hover = hsl(0xe8e8e8);
        theme.button_secondary_active = p.selected;
        theme.button_secondary_foreground = p.text;
        theme.button_danger = p.danger;
        theme.button_danger_hover = hsl(0xa82316);
        theme.button_danger_active = hsl(0x8f1d12);
        theme.button_danger_foreground = p.background;
    });
}

/// Weight used for the short uppercase section labels.
pub fn label_weight() -> FontWeight {
    FontWeight::SEMIBOLD
}

/// Reads the active palette. It is fixed and cheap to rebuild; keeping it out
/// of a global avoids a second source of truth next to gpui-component's Theme.
pub fn palette(_cx: &App) -> Palette {
    Palette::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `Hsla` keeps hue/saturation/lightness, so the round-trip goes back
    /// through `Rgba` to compare byte channels.
    fn channels(c: Hsla) -> (u8, u8, u8) {
        let rgba = gpui_kit::Rgba::from(c);
        (
            (rgba.r * 255.0).round() as u8,
            (rgba.g * 255.0).round() as u8,
            (rgba.b * 255.0).round() as u8,
        )
    }

    fn expected(hex: u32) -> (u8, u8, u8) {
        (
            ((hex >> 16) & 0xff) as u8,
            ((hex >> 8) & 0xff) as u8,
            (hex & 0xff) as u8,
        )
    }

    #[test]
    fn palette_is_neutral_apart_from_status_colors() {
        let p = Palette::new();
        assert_eq!(channels(p.background), expected(BACKGROUND));
        assert_eq!(channels(p.chrome), expected(CHROME));
        assert_eq!(channels(p.line), expected(LINE));
        assert_eq!(channels(p.text), expected(TEXT));
        assert_eq!(channels(p.accent), expected(ACCENT));
        assert_eq!(channels(p.ok), expected(OK));
        assert_eq!(channels(p.warn), expected(WARN));
        assert_eq!(channels(p.danger), expected(DANGER));
    }

    #[test]
    fn only_failures_and_warnings_are_colored() {
        let p = Palette::new();
        assert_eq!(p.tone(Tone::Ok), p.ok);
        assert_eq!(p.tone(Tone::Err), p.danger);
        assert_eq!(p.tone(Tone::Warn), p.warn);
        assert_eq!(p.tone(Tone::Info), p.text);
    }

    #[test]
    fn the_theme_uses_standard_windows_fonts() {
        assert_eq!(SANS, "Segoe UI");
        assert_eq!(MONO, "Consolas");
    }
}

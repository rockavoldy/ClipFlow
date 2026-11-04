//! Visual language: the same palette as `src/index.css`, expressed as Rust.
//!
//! The old deck was Tailwind with `@theme` tokens; this is the direct port of
//! those tokens. Values are copied verbatim so the GPUI build reads as the same
//! product, not a redesign:
//!
//! | token   | hex       |
//! |---------|-----------|
//! | void    | `#05060d` |
//! | deck    | `#0a0d1a` |
//! | panel   | `#0e1224` |
//! | panel2  | `#131832` |
//! | line    | `#1e2545` |
//! | cyan    | `#5eeaff` |
//! | magenta | `#ff4fd8` |
//! | lime    | `#76ff9c` |
//! | amber   | `#ffc94f` |
//! | danger  | `#ff5f6d` |
//!
//! Font note: the web build loaded Inter and JetBrains Mono from the network.
//! Neither is bundled (that would add ~500 KB to a ~6 MB exe and the licences
//! differ), so the deck asks for them by family name and falls back to the
//! platform UI font and its monospace. On a machine with those fonts installed
//! it is pixel-identical; otherwise it degrades to Segoe UI / Consolas.

use gpui_kit::component::{ActiveTheme, Theme};
use gpui_kit::{App, FontWeight, Hsla, px, rgb};

pub const VOID: u32 = 0x05060d;
pub const DECK: u32 = 0x0a0d1a;
pub const PANEL: u32 = 0x0e1224;
pub const PANEL_2: u32 = 0x131832;
pub const LINE: u32 = 0x1e2545;
pub const TEXT: u32 = 0xe6ecff;
pub const MUTED: u32 = 0x8a93b8;
pub const CYAN: u32 = 0x5eeaff;
pub const MAGENTA: u32 = 0xff4fd8;
pub const LIME: u32 = 0x76ff9c;
pub const AMBER: u32 = 0xffc94f;
pub const DANGER: u32 = 0xff5f6d;

pub const SANS: &str = "Inter";
pub const MONO: &str = "JetBrains Mono";

/// Semantic colors resolved once per frame so the render path does no hex math.
#[derive(Debug, Clone, Copy)]
pub struct Palette {
    pub void: Hsla,
    pub deck: Hsla,
    pub panel: Hsla,
    pub panel_2: Hsla,
    pub line: Hsla,
    pub text: Hsla,
    pub muted: Hsla,
    pub cyan: Hsla,
    pub magenta: Hsla,
    pub lime: Hsla,
    pub amber: Hsla,
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
            void: hsl(VOID),
            deck: hsl(DECK),
            panel: hsl(PANEL),
            panel_2: hsl(PANEL_2),
            line: hsl(LINE),
            text: hsl(TEXT),
            muted: hsl(MUTED),
            cyan: hsl(CYAN),
            magenta: hsl(MAGENTA),
            lime: hsl(LIME),
            amber: hsl(AMBER),
            danger: hsl(DANGER),
        }
    }

    /// Color for a `ToastTone`, matching `TONE_STYLES` from `App.tsx`.
    pub fn tone(&self, tone: Tone) -> Hsla {
        match tone {
            Tone::Ok => self.lime,
            Tone::Warn => self.amber,
            Tone::Err => self.danger,
            Tone::Info => self.cyan,
        }
    }

    /// Color for an engine state, used by the armed pill and the status line.
    pub fn state(&self, state: ArmedState) -> Hsla {
        match state {
            ArmedState::Armed => self.lime,
            ArmedState::Starting => self.amber,
            ArmedState::Busy => self.cyan,
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

/// The four toast tones from `App.tsx`. Kept because the copy/tone pairing is a
/// user-visible contract ("saved" is green, "privacy blocked" is red).
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

/// Applies the palette to gpui-component's theme so any stock widget (buttons,
/// inputs, menus) inherits the deck's colors instead of the library default
/// light/blue look.
pub fn init(cx: &mut App) {
    Theme::update(cx, |theme| {
        theme.font_family = SANS.into();
        theme.mono_font_family = MONO.into();
        theme.font_size = px(13.0);
        theme.mono_font_size = px(12.0);
        theme.radius = px(6.0);
        theme.radius_lg = px(10.0);

        let p = Palette::new();
        theme.background = p.deck;
        theme.foreground = p.text;
        theme.border = p.line;
        theme.muted_foreground = p.muted;
        theme.primary = p.cyan;
        theme.primary_foreground = p.void;
        theme.secondary = p.panel_2;
        theme.danger = p.danger;
        theme.success = p.lime;
        theme.warning = p.amber;
        theme.info = p.cyan;
    });
}

/// Uppercase, wide-tracked label style used throughout the old deck
/// (`tracking-[0.18em] text-[10px] uppercase`). Returned as helpers because
/// gpui has no letter-spacing, so we lean on caps + weight instead.
pub fn label_weight() -> FontWeight {
    FontWeight::SEMIBOLD
}

pub fn mono() -> &'static str {
    MONO
}

/// Reads the active palette from the app theme's global, so views never build
/// colors inline.
pub fn palette(cx: &App) -> Palette {
    // The palette is fixed (dark-only) and cheap to rebuild; keeping it out of
    // a global avoids a second source of truth next to gpui-component's Theme.
    let _ = cx.theme();
    Palette::new()
}

/// Color for an armed/idle pill. Free function so render helpers can tint
/// without threading the whole palette.
pub fn state_color(armed: bool) -> Hsla {
    if armed { hsl(LIME) } else { hsl(MUTED) }
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
    fn palette_values_match_the_web_theme_tokens() {
        // These hex values are copied from `src/index.css` `@theme`; if someone
        // "tidies" them the deck stops matching the shipped screenshots.
        let p = Palette::new();
        assert_eq!(channels(p.void), expected(VOID));
        assert_eq!(channels(p.deck), expected(DECK));
        assert_eq!(channels(p.cyan), expected(CYAN));
        assert_eq!(channels(p.lime), expected(LIME));
        assert_eq!(channels(p.danger), expected(DANGER));
        assert_eq!(channels(p.magenta), expected(MAGENTA));
        assert_eq!(channels(p.amber), expected(AMBER));
    }

    #[test]
    fn tone_colors_pair_with_the_old_tailwind_styles() {
        let p = Palette::new();
        assert_eq!(p.tone(Tone::Ok), p.lime);
        assert_eq!(p.tone(Tone::Err), p.danger);
        assert_eq!(p.tone(Tone::Warn), p.amber);
        assert_eq!(p.tone(Tone::Info), p.cyan);
    }
}

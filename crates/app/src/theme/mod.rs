//! IRCord's colours as runtime tokens, so a colour scheme can change them
//! while the app runs. Views read the active tokens with [`get`] at the top
//! of `render` and never name a colour themselves; [`hex`] is for colours
//! that come from data (IRC colour codes, a member's name colour).
//!
//! The default is a dark palette close to the web client's default theme.

use gpui_kit::{App, Global, Hsla, Rgba, rgb};

/// The colour tokens every view draws with.
#[derive(Clone, Debug, PartialEq)]
pub struct Theme {
    /// The network rail and the sign-in backdrop.
    pub rail: Hsla,
    /// Sidebars, cards and panels.
    pub sidebar: Hsla,
    /// The account strip under the conversation list.
    pub footer: Hsla,
    /// The main pane.
    pub main: Hsla,
    pub input: Hsla,
    pub hover: Hsla,
    /// A message row under the pointer.
    pub row_hover: Hsla,
    pub selected: Hsla,
    pub text: Hsla,
    pub muted: Hsla,
    pub faint: Hsla,
    pub accent: Hsla,
    /// Text drawn on `accent`, `danger` and other strong fills.
    pub on_accent: Hsla,
    pub link: Hsla,
    pub danger: Hsla,
    pub warning: Hsla,
    pub online: Hsla,
    pub away: Hsla,
    pub offline: Hsla,
    pub mention: Hsla,
    /// Inline code and monospace runs.
    pub code: Hsla,
}

impl Theme {
    pub fn dark() -> Self {
        Self {
            rail: hex(0x1e1f22),
            sidebar: hex(0x2b2d31),
            footer: hex(0x232428),
            main: hex(0x313338),
            input: hex(0x383a40),
            hover: hex(0x35373c),
            row_hover: hex(0x2e3035),
            selected: hex(0x404249),
            text: hex(0xdbdee1),
            muted: hex(0x949ba4),
            faint: hex(0x6d6f78),
            accent: hex(0x5865f2),
            on_accent: hex(0xffffff),
            link: hex(0x00a8fc),
            danger: hex(0xf23f43),
            warning: hex(0xf0b232),
            online: hex(0x23a55a),
            away: hex(0xf0b232),
            offline: hex(0x80848e),
            mention: hex(0xf23f43),
            code: hex(0x1e1f22),
        }
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self::dark()
    }
}

impl Global for Theme {}

/// Install the default theme. Called once at start.
pub fn init(cx: &mut App) {
    if !cx.has_global::<Theme>() {
        cx.set_global(Theme::default());
    }
}

/// The active tokens. Clone them at the top of `render` when a listener
/// needs `cx` mutably later.
pub fn get(cx: &App) -> &Theme {
    cx.try_global::<Theme>().unwrap_or_else(|| default_theme())
}

fn default_theme() -> &'static Theme {
    static DEFAULT: std::sync::OnceLock<Theme> = std::sync::OnceLock::new();
    DEFAULT.get_or_init(Theme::default)
}

/// Replace the active tokens and redraw every window.
pub fn set(theme: Theme, cx: &mut App) {
    cx.set_global(theme);
    cx.refresh_windows();
}

/// A colour from data, `0xRRGGBB`.
pub fn hex(value: u32) -> Hsla {
    Hsla::from(rgb(value))
}

/// A colour from data with transparency.
pub fn hex_alpha(value: u32, alpha: f32) -> Hsla {
    let mut color: Rgba = rgb(value);
    color.a = alpha;
    Hsla::from(color)
}

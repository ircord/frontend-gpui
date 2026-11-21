//! IRCord's colours, fonts and shapes as runtime tokens, so the appearance
//! settings can change them while the app runs. Views read the active tokens
//! with [`get`] at the top of `render` and never name a colour themselves;
//! [`hex`] is for colours that come from data (IRC colour codes, a member's
//! name colour).
//!
//! The tokens are compiled from the account's appearance (scheme, accent,
//! font, scale, corners, density, motion, personal theme) by
//! `ircord_client::store::appearance`; [`appearance`] keeps them applied and
//! is what settings edit.

pub mod appearance;
pub mod fonts;

use gpui_kit::{App, Global, Hsla, Pixels, Rems, Rgba, SharedString, px, rems, rgb};
use ircord_client::store::appearance::{self as look, Compiled, Look, Mode};

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
    /// Side panels and the settings navigation.
    pub panel: Hsla,
    /// Cards and raised controls.
    pub raised: Hsla,
    /// Quiet dividers.
    pub border: Hsla,
    /// Edges of fields and checkboxes, at 3:1 on what they sit on.
    pub control_border: Hsla,
    /// Message text, a little softer than `text`.
    pub body: Hsla,
    /// The drawn scheme is a dark one.
    pub dark: bool,
    /// The interface font family.
    pub font: SharedString,
    /// The monospace family for code.
    pub mono_font: SharedString,
    /// Vertical padding of a sidebar row (the sidebar density).
    pub row_padding: Rems,
    /// Corner radius of rows, controls and cards.
    pub radius: Pixels,
    /// Corner radius of dialogs and large cards.
    pub radius_large: Pixels,
    /// The sidebar is compact.
    pub compact: bool,
    /// Interface scale, 1.0 = 100 %.
    pub scale: f32,
    /// Leave out non-essential motion.
    pub reduced_motion: bool,
}

impl Theme {
    /// The tokens for one compiled appearance.
    pub fn from_compiled(c: &Compiled) -> Self {
        let t = &c.tokens;
        Self {
            rail: hex(t.rail),
            sidebar: hex(t.sidebar),
            footer: hex(t.rail),
            main: hex(t.main),
            input: hex(t.input),
            hover: hex(t.hover),
            row_hover: hex(t.raised),
            selected: hex(t.selection),
            text: hex(t.text),
            muted: hex(t.muted),
            faint: hex(t.faint),
            accent: hex(t.accent),
            on_accent: hex(t.accent_ink),
            link: hex(t.accent),
            danger: hex(t.danger),
            warning: hex(t.yellow),
            online: hex(t.mint),
            away: hex(t.yellow),
            offline: hex(t.faint),
            // Amber, as the web marks a mention: red reads as an error.
            mention: hex(t.yellow),
            code: hex(t.rail),
            panel: hex(t.panel),
            raised: hex(t.raised),
            border: hex(t.border),
            control_border: hex(t.control_border),
            body: hex(t.body),
            dark: c.mode == Mode::Dark,
            font: c.font.unwrap_or(".SystemUIFont").into(),
            mono_font: look::MONO_FAMILY.into(),
            row_padding: if c.compact { rems(0.0625) } else { rems(0.25) },
            radius: px(c.radius),
            radius_large: px(c.radius_large),
            compact: c.compact,
            scale: c.scale,
            reduced_motion: c.reduced_motion,
        }
    }

    /// IRCord's own dark scheme with the default settings.
    pub fn dark() -> Self {
        Self::from_compiled(&look::compile(&Look::default(), Mode::Dark, &[], false))
    }

    /// IRCord's own light scheme with the default settings.
    pub fn light() -> Self {
        Self::from_compiled(&look::compile(&Look::default(), Mode::Light, &[], false))
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self::dark()
    }
}

impl Global for Theme {}

/// Install the default theme and the bundled fonts, and keep the account's
/// appearance applied. Called once at start.
pub fn init(cx: &mut App) {
    if !cx.has_global::<Theme>() {
        cx.set_global(Theme::default());
    }
    fonts::register(cx);
    appearance::init(cx);
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

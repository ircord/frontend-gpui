//! Appearance: the colour schemes, accents, fonts, density, corners and
//! motion, and personal themes, the way the web client keeps them
//! (`shared/appearance.ts`, `shared/schemes.ts`, `shared/themes.ts`).
//!
//! The account's copies are the `appearance` and `themes` documents in
//! `Workspace::documents`. This module is the pure part: the scheme data, the
//! colour arithmetic that turns a scheme and an accent into every interface
//! token, cleaning a stored appearance, this device's own copy when sync is
//! off ([`Device`]), and personal themes' import and export. The app applies
//! the result (`crates/app/src/theme`).
//!
//! Owned by package S1. The message settings in the same document (layout,
//! message font, size, spacing, clock, coloured names, animated media) belong
//! to S2; this module keeps them as they are when it edits the document.

use ircord_contract::event::ServerEvent;
use ircord_contract::{
    Appearance, AppearanceAccent, AppearanceCorners, AppearanceDensity, AppearanceFont,
    AppearanceMessageFont, AppearanceMode, AppearanceMotion, Snapshot, StoredAppearance, Theme,
    ThemeBackground, ThemeBase, ThemeColors, ThemePalette, ThemeSeeds,
};
use serde::{Deserialize, Serialize};

use super::theme_css::{self, CssVariables};
use super::{Changes, Workspace};

pub mod background;
pub mod drafts;

/// The topic this sub-state reports in [`Changes`].
pub const TOPIC: &str = "appearance";

#[derive(Debug, Clone, Default)]
pub struct State {}

/// Called for every fresh event after the workspace's own handling.
pub(super) fn apply(_w: &mut Workspace, _event: &ServerEvent, _now_ms: i64) -> Changes {
    Changes::default()
}

/// Called when a bootstrap replaces the workspace, before it is replaced.
pub(super) fn reset(_w: &mut Workspace, _snapshot: &Snapshot) -> Changes {
    Changes::default()
}

// ---- Schemes -------------------------------------------------------------------------------

/// Light or dark.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Dark,
    Light,
}

/// The eight palette hues every scheme provides, `0xRRGGBB`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    pub red: u32,
    pub orange: u32,
    pub yellow: u32,
    pub green: u32,
    pub cyan: u32,
    pub blue: u32,
    pub purple: u32,
    pub pink: u32,
}

/// The handful of colours every token is derived from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Seeds {
    /// Darkest chrome: the network rail and the account strip.
    pub canvas: u32,
    /// The conversation background.
    pub surface: u32,
    /// Raised elements: hover, borders and controls are mixed from this.
    pub overlay: u32,
    pub ink: u32,
    pub muted: u32,
    pub accent: u32,
    pub palette: Palette,
}

/// A published palette reduced to its seeds.
#[derive(Debug)]
pub struct Scheme {
    pub id: &'static str,
    pub family: &'static str,
    pub name: &'static str,
    pub mode: Mode,
    /// Where the palette comes from, empty for IRCord's own.
    pub source: &'static str,
    pub seeds: Seeds,
    /// Exact token values that replace the derived ones (IRCord's own tuned
    /// palette), by the web client's token name.
    pub tokens: &'static [(&'static str, u32)],
}

/// A scheme family and its light and dark variants.
#[derive(Debug)]
pub struct Family {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub variants: &'static [Scheme],
}

/// The schemes used when nothing else is chosen.
pub const DEFAULT_DARK: &str = "ircord-dark";
pub const DEFAULT_LIGHT: &str = "ircord-light";

/// Every scheme, family by family.
pub fn schemes() -> impl Iterator<Item = &'static Scheme> {
    FAMILIES.iter().flat_map(|f| f.variants.iter())
}

pub fn find_scheme(id: &str) -> Option<&'static Scheme> {
    schemes().find(|s| s.id == id)
}

pub fn family(id: &str) -> Option<&'static Family> {
    FAMILIES.iter().find(|f| f.id == id)
}

/// A scheme for `mode`: `id` if it is one, else its family's variant in that
/// mode, else IRCord's.
pub fn scheme_for(id: &str, mode: Mode) -> &'static Scheme {
    let found = find_scheme(id);
    if let Some(scheme) = found.filter(|s| s.mode == mode) {
        return scheme;
    }
    if let Some(sibling) =
        found.and_then(|f| schemes().find(|s| s.family == f.family && s.mode == mode))
    {
        return sibling;
    }
    find_scheme(match mode {
        Mode::Dark => DEFAULT_DARK,
        Mode::Light => DEFAULT_LIGHT,
    })
    .expect("the default schemes exist")
}

// ---- Colour arithmetic (shared/utils/color.ts) ---------------------------------------------

fn channels(color: u32) -> [f64; 3] {
    [
        f64::from((color >> 16) & 0xff),
        f64::from((color >> 8) & 0xff),
        f64::from(color & 0xff),
    ]
}

fn pack(values: [f64; 3]) -> u32 {
    values
        .iter()
        .fold(0, |acc, v| (acc << 8) | (v.clamp(0., 255.).round() as u32))
}

/// Mix `b` into `a`: `amount` 0 is `a`, 1 is `b`.
pub fn mix(a: u32, b: u32, amount: f64) -> u32 {
    let (x, y) = (channels(a), channels(b));
    pack([0, 1, 2].map(|i| x[i] + (y[i] - x[i]) * amount))
}

/// Relative luminance (WCAG).
pub fn luminance(color: u32) -> f64 {
    let [r, g, b] = channels(color).map(|c| {
        let v = c / 255.;
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    });
    r * 0.2126 + g * 0.7152 + b * 0.0722
}

/// WCAG contrast ratio, 1 to 21.
pub fn contrast(a: u32, b: u32) -> f64 {
    let (x, y) = (luminance(a), luminance(b));
    (x.max(y) + 0.05) / (x.min(y) + 0.05)
}

/// `color` moved towards white (dark backgrounds) or black (light ones) until
/// it reaches `ratio` on every background. The backgrounds must be all dark or
/// all light; the first decides the direction.
pub fn readable_on_all(color: u32, backgrounds: &[u32], ratio: f64) -> u32 {
    let target = if luminance(backgrounds[0]) < 0.4 {
        0xffffff
    } else {
        0x000000
    };
    for step in 0..=20 {
        let candidate = mix(color, target, f64::from(step) / 20.);
        if backgrounds.iter().all(|b| contrast(candidate, *b) >= ratio) {
            return candidate;
        }
    }
    target
}

pub fn readable_on(color: u32, background: u32, ratio: f64) -> u32 {
    readable_on_all(color, &[background], ratio)
}

/// Dark or light ink for text on a `fill`, at 4.5:1 where possible.
pub fn ink_for(fill: u32) -> u32 {
    if contrast(fill, 0x111114) >= contrast(fill, 0xffffff) {
        readable_on(mix(fill, 0x000000, 0.82), fill, 4.5)
    } else {
        0xffffff
    }
}

/// `#rrggbb` (any case) as `0xRRGGBB`.
pub fn parse_hex(text: &str) -> Option<u32> {
    let digits = text.strip_prefix('#')?;
    if digits.len() != 6 || !digits.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    u32::from_str_radix(digits, 16).ok()
}

/// `0xRRGGBB` as `#rrggbb`.
pub fn to_hex(color: u32) -> String {
    format!("#{:06x}", color & 0xffffff)
}

// ---- Tokens --------------------------------------------------------------------------------

/// Every interface colour, `0xRRGGBB`, as the web client names them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Tokens {
    pub rail: u32,
    pub sidebar: u32,
    pub main: u32,
    pub panel: u32,
    pub raised: u32,
    pub hover: u32,
    pub input: u32,
    pub border: u32,
    pub control_border: u32,
    pub text: u32,
    pub body: u32,
    pub muted: u32,
    pub faint: u32,
    pub accent: u32,
    pub accent_ink: u32,
    pub selection: u32,
    pub mint: u32,
    pub danger: u32,
    pub yellow: u32,
    pub code_keyword: u32,
    pub code_string: u32,
    pub code_number: u32,
    pub code_comment: u32,
    pub code_function: u32,
    pub code_type: u32,
    pub code_property: u32,
    pub code_meta: u32,
    pub code_deletion: u32,
}

impl Tokens {
    /// The current colour of the token `name`, for a theme CSS's `var()`.
    fn token(self, name: &str) -> Option<u32> {
        let mut copy = self;
        copy.slot(name).copied()
    }

    /// The token the web client calls `name`, for a scheme's exact overrides.
    fn slot(&mut self, name: &str) -> Option<&mut u32> {
        Some(match name {
            "rail" => &mut self.rail,
            "sidebar" => &mut self.sidebar,
            "main" => &mut self.main,
            "panel" => &mut self.panel,
            "raised" => &mut self.raised,
            "hover" => &mut self.hover,
            "input" => &mut self.input,
            "border" => &mut self.border,
            "control-border" => &mut self.control_border,
            "text" => &mut self.text,
            "body" => &mut self.body,
            "muted" => &mut self.muted,
            "faint" => &mut self.faint,
            "accent" => &mut self.accent,
            "accent-ink" => &mut self.accent_ink,
            "selection" => &mut self.selection,
            "mint" => &mut self.mint,
            "danger" => &mut self.danger,
            "yellow" => &mut self.yellow,
            "code-keyword" => &mut self.code_keyword,
            "code-string" => &mut self.code_string,
            "code-number" => &mut self.code_number,
            "code-comment" => &mut self.code_comment,
            "code-function" => &mut self.code_function,
            "code-type" => &mut self.code_type,
            "code-property" => &mut self.code_property,
            "code-meta" => &mut self.code_meta,
            "code-deletion" => &mut self.code_deletion,
            _ => return None,
        })
    }

    /// The surfaces text is drawn on: every text token reads at 4.5:1 on each.
    pub fn surfaces(&self) -> [u32; 8] {
        [
            self.rail,
            self.sidebar,
            self.main,
            self.panel,
            self.raised,
            self.hover,
            self.input,
            self.selection,
        ]
    }
}

/// Every interface token for `seeds` in `mode`, with a scheme's exact
/// `overrides`. `accent` replaces the scheme's accent; every text colour is
/// then nudged until it reads at 4.5:1 on every surface (`schemeTokens`).
pub fn tokens(seeds: &Seeds, mode: Mode, overrides: &[(&str, u32)], accent: Option<u32>) -> Tokens {
    let Seeds {
        canvas,
        surface,
        overlay,
        ink,
        muted,
        palette,
        ..
    } = *seeds;
    let rail_override = overrides
        .iter()
        .find(|(name, _)| *name == "rail")
        .map(|(_, v)| *v);
    let code_bg = rail_override.unwrap_or(canvas);
    let on = |color: u32| readable_on(color, code_bg, 4.5);
    let mut t = Tokens {
        rail: canvas,
        sidebar: mix(canvas, surface, 0.45),
        main: surface,
        panel: mix(canvas, surface, 0.7),
        raised: mix(surface, overlay, 0.4),
        hover: mix(surface, overlay, 0.6),
        input: mix(surface, overlay, 0.35),
        border: mix(surface, overlay, 0.75),
        control_border: 0,
        text: ink,
        body: mix(ink, muted, 0.3),
        muted,
        faint: mix(muted, surface, 0.18),
        accent: seeds.accent,
        accent_ink: 0,
        selection: 0,
        mint: palette.green,
        danger: palette.red,
        yellow: palette.yellow,
        code_keyword: on(palette.purple),
        code_string: on(palette.green),
        code_number: on(palette.orange),
        code_comment: on(muted),
        code_function: on(palette.blue),
        code_type: on(palette.yellow),
        code_property: on(palette.cyan),
        code_meta: on(palette.pink),
        code_deletion: on(palette.red),
    };
    for (name, value) in overrides {
        if let Some(slot) = t.slot(name) {
            *slot = *value;
        }
    }
    if let Some(accent) = accent {
        t.accent = accent;
    }
    let mut surfaces = vec![
        t.rail, t.sidebar, t.main, t.panel, t.raised, t.hover, t.input,
    ];
    let tint = match mode {
        Mode::Dark => 0.16,
        Mode::Light => 0.13,
    };
    t.accent = readable_on_all(t.accent, &surfaces, 4.5);
    for _ in 0..8 {
        t.selection = mix(t.main, t.accent, tint);
        let mut with_selection = surfaces.clone();
        with_selection.push(t.selection);
        let accent = readable_on_all(t.accent, &with_selection, 4.5);
        if accent == t.accent {
            break;
        }
        t.accent = accent;
    }
    surfaces.push(t.selection);
    for value in [
        &mut t.text,
        &mut t.body,
        &mut t.muted,
        &mut t.faint,
        &mut t.mint,
        &mut t.danger,
        &mut t.yellow,
    ] {
        *value = readable_on_all(*value, &surfaces, 4.5);
    }
    if contrast(t.faint, t.main) > contrast(t.muted, t.main) {
        t.faint = t.muted;
    }
    t.accent_ink = ink_for(t.accent);
    t.control_border = readable_on_all(
        t.border,
        &[t.input, t.main, t.panel, t.raised, t.sidebar],
        3.,
    );
    t
}

/// A scheme's tokens with an optional replacement accent.
pub fn scheme_tokens(scheme: &Scheme, accent: Option<u32>) -> Tokens {
    tokens(&scheme.seeds, scheme.mode, scheme.tokens, accent)
}

// Generated from the web client's `shared/schemes.ts`; regenerate rather than edit.
#[rustfmt::skip]
pub static FAMILIES: &[Family] = &[
    Family { id: "ircord", name: "IRCord", description: "Charcoal and lavender, the house style.", variants: &[
        Scheme { id: "ircord-dark", family: "ircord", name: "Charcoal", mode: Mode::Dark, source: "",
            seeds: Seeds { canvas: 0x141418, surface: 0x232329, overlay: 0x3a3a44, ink: 0xeeeef2, muted: 0xa2a2af, accent: 0xbfadf9,
                palette: Palette { red: 0xf2a4a6, orange: 0xe8ad89, yellow: 0xe5c38f, green: 0xa7d9bf, cyan: 0x91cbd1, blue: 0x98b8f0, purple: 0xbfadf9, pink: 0xe7a1bc } },
            tokens: &[("rail", 0x141418), ("sidebar", 0x1b1b21), ("main", 0x232329), ("panel", 0x1d1d23), ("raised", 0x2b2b33), ("hover", 0x303039), ("input", 0x2c2c34), ("border", 0x34343d), ("text", 0xeeeef2), ("body", 0xceced7), ("muted", 0xa2a2af), ("faint", 0x8f8c9c)] },
        Scheme { id: "ircord-light", family: "ircord", name: "Daylight", mode: Mode::Light, source: "",
            seeds: Seeds { canvas: 0xe8e6ef, surface: 0xffffff, overlay: 0xdedbe7, ink: 0x2c2935, muted: 0x6e667c, accent: 0x7350b9,
                palette: Palette { red: 0xa13b4a, orange: 0x945c36, yellow: 0x84692c, green: 0x397752, cyan: 0x307177, blue: 0x3e649d, purple: 0x7350b9, pink: 0x984263 } },
            tokens: &[("rail", 0xe8e6ef), ("sidebar", 0xf2f1f6), ("main", 0xffffff), ("panel", 0xf8f7fb), ("raised", 0xf0eef5), ("hover", 0xe8e5ef), ("input", 0xf6f5f9), ("border", 0xdedbe7), ("text", 0x2c2935), ("body", 0x484351), ("muted", 0x6e667c), ("faint", 0x766e83)] },
    ] },
    Family { id: "hibiki", name: "Hibiki", description: "Warm obsidian and coral with an ambient top glow.", variants: &[
        Scheme { id: "hibiki-dark", family: "hibiki", name: "Stage", mode: Mode::Dark, source: "",
            seeds: Seeds { canvas: 0x111013, surface: 0x18161a, overlay: 0x2d2730, ink: 0xf4f0f3, muted: 0xa89ea6, accent: 0xf43f5e,
                palette: Palette { red: 0xf87171, orange: 0xfb923c, yellow: 0xfbbf24, green: 0x4ade80, cyan: 0x38bdf8, blue: 0x818cf8, purple: 0xc084fc, pink: 0xfb7185 } },
            tokens: &[] },
        Scheme { id: "hibiki-light", family: "hibiki", name: "Paper", mode: Mode::Light, source: "",
            seeds: Seeds { canvas: 0xefe9ec, surface: 0xfcf9fa, overlay: 0xdfd6db, ink: 0x261f24, muted: 0x6b5e66, accent: 0xd92648,
                palette: Palette { red: 0xbe123c, orange: 0xc2410c, yellow: 0xa16207, green: 0x15803d, cyan: 0x0369a1, blue: 0x4338ca, purple: 0x7e22ce, pink: 0xbe185d } },
            tokens: &[] },
    ] },
    Family { id: "midnight", name: "Midnight", description: "True black for OLED screens, with crisp contrast.", variants: &[
        Scheme { id: "midnight", family: "midnight", name: "OLED", mode: Mode::Dark, source: "",
            seeds: Seeds { canvas: 0x000000, surface: 0x0b0b0c, overlay: 0x26262a, ink: 0xf5f5f7, muted: 0xa1a1a8, accent: 0x5eead4,
                palette: Palette { red: 0xff6b6b, orange: 0xffa94d, yellow: 0xffd43b, green: 0x69db7c, cyan: 0x66d9e8, blue: 0x74c0fc, purple: 0xb197fc, pink: 0xf783ac } },
            tokens: &[] },
    ] },
    Family { id: "catppuccin", name: "Catppuccin", description: "Soothing pastels with gentle, low-contrast surfaces.", variants: &[
        Scheme { id: "catppuccin-mocha", family: "catppuccin", name: "Mocha", mode: Mode::Dark, source: "https://catppuccin.com/palette/",
            seeds: Seeds { canvas: 0x181825, surface: 0x1e1e2e, overlay: 0x313244, ink: 0xcdd6f4, muted: 0xa6adc8, accent: 0xcba6f7,
                palette: Palette { red: 0xf38ba8, orange: 0xfab387, yellow: 0xf9e2af, green: 0xa6e3a1, cyan: 0x89dceb, blue: 0x89b4fa, purple: 0xcba6f7, pink: 0xf5c2e7 } },
            tokens: &[] },
        Scheme { id: "catppuccin-macchiato", family: "catppuccin", name: "Macchiato", mode: Mode::Dark, source: "https://catppuccin.com/palette/",
            seeds: Seeds { canvas: 0x1e2030, surface: 0x24273a, overlay: 0x363a4f, ink: 0xcad3f5, muted: 0xa5adcb, accent: 0xc6a0f6,
                palette: Palette { red: 0xed8796, orange: 0xf5a97f, yellow: 0xeed49f, green: 0xa6da95, cyan: 0x91d7e3, blue: 0x8aadf4, purple: 0xc6a0f6, pink: 0xf5bde6 } },
            tokens: &[] },
        Scheme { id: "catppuccin-frappe", family: "catppuccin", name: "Frappé", mode: Mode::Dark, source: "https://catppuccin.com/palette/",
            seeds: Seeds { canvas: 0x292c3c, surface: 0x303446, overlay: 0x414559, ink: 0xc6d0f5, muted: 0xa5adce, accent: 0xca9ee6,
                palette: Palette { red: 0xe78284, orange: 0xef9f76, yellow: 0xe5c890, green: 0xa6d189, cyan: 0x99d1db, blue: 0x8caaee, purple: 0xca9ee6, pink: 0xf4b8e4 } },
            tokens: &[] },
        Scheme { id: "catppuccin-latte", family: "catppuccin", name: "Latte", mode: Mode::Light, source: "https://catppuccin.com/palette/",
            seeds: Seeds { canvas: 0xe6e9ef, surface: 0xeff1f5, overlay: 0xccd0da, ink: 0x4c4f69, muted: 0x65687d, accent: 0x8839ef,
                palette: Palette { red: 0xd20f39, orange: 0xfe640b, yellow: 0xdf8e1d, green: 0x40a02b, cyan: 0x04a5e5, blue: 0x1e66f5, purple: 0x8839ef, pink: 0xea76cb } },
            tokens: &[] },
    ] },
    Family { id: "tokyo-night", name: "Tokyo Night", description: "Neon city lights over deep indigo.", variants: &[
        Scheme { id: "tokyo-night", family: "tokyo-night", name: "Night", mode: Mode::Dark, source: "https://github.com/folke/tokyonight.nvim",
            seeds: Seeds { canvas: 0x16161e, surface: 0x1a1b26, overlay: 0x292e42, ink: 0xc0caf5, muted: 0xa9b1d6, accent: 0x7aa2f7,
                palette: Palette { red: 0xf7768e, orange: 0xff9e64, yellow: 0xe0af68, green: 0x9ece6a, cyan: 0x7dcfff, blue: 0x7aa2f7, purple: 0xbb9af7, pink: 0xff7eb6 } },
            tokens: &[] },
        Scheme { id: "tokyo-storm", family: "tokyo-night", name: "Storm", mode: Mode::Dark, source: "https://github.com/folke/tokyonight.nvim",
            seeds: Seeds { canvas: 0x1f2335, surface: 0x24283b, overlay: 0x3b4261, ink: 0xc0caf5, muted: 0xa9b1d6, accent: 0x7aa2f7,
                palette: Palette { red: 0xf7768e, orange: 0xff9e64, yellow: 0xe0af68, green: 0x9ece6a, cyan: 0x7dcfff, blue: 0x7aa2f7, purple: 0xbb9af7, pink: 0xff7eb6 } },
            tokens: &[] },
        Scheme { id: "tokyo-moon", family: "tokyo-night", name: "Moon", mode: Mode::Dark, source: "https://github.com/folke/tokyonight.nvim",
            seeds: Seeds { canvas: 0x1e2030, surface: 0x222436, overlay: 0x2f334d, ink: 0xc8d3f5, muted: 0xa9b8e8, accent: 0x82aaff,
                palette: Palette { red: 0xff757f, orange: 0xff966c, yellow: 0xffc777, green: 0xc3e88d, cyan: 0x86e1fc, blue: 0x82aaff, purple: 0xc099ff, pink: 0xfca7ea } },
            tokens: &[] },
        Scheme { id: "tokyo-day", family: "tokyo-night", name: "Day", mode: Mode::Light, source: "https://github.com/folke/tokyonight.nvim",
            seeds: Seeds { canvas: 0xd0d5e3, surface: 0xe1e2e7, overlay: 0xc4c8da, ink: 0x3258af, muted: 0x4a5993, accent: 0x2e7de9,
                palette: Palette { red: 0xf52a65, orange: 0xb15c00, yellow: 0x8c6c3e, green: 0x587539, cyan: 0x007197, blue: 0x2e7de9, purple: 0x9854f1, pink: 0xd20065 } },
            tokens: &[] },
    ] },
    Family { id: "nord", name: "Nord", description: "An arctic, north-bluish palette.", variants: &[
        Scheme { id: "nord-dark", family: "nord", name: "Polar Night", mode: Mode::Dark, source: "https://www.nordtheme.com/docs/colors-and-palettes",
            seeds: Seeds { canvas: 0x2e3440, surface: 0x3b4252, overlay: 0x4c566a, ink: 0xeceff4, muted: 0xd8dee9, accent: 0x88c0d0,
                palette: Palette { red: 0xbf616a, orange: 0xd08770, yellow: 0xebcb8b, green: 0xa3be8c, cyan: 0x8fbcbb, blue: 0x81a1c1, purple: 0xb48ead, pink: 0xc895bf } },
            tokens: &[] },
        Scheme { id: "nord-light", family: "nord", name: "Snow Storm", mode: Mode::Light, source: "https://www.nordtheme.com/docs/colors-and-palettes",
            seeds: Seeds { canvas: 0xe5e9f0, surface: 0xeceff4, overlay: 0xd8dee9, ink: 0x2e3440, muted: 0x4c566a, accent: 0x5e81ac,
                palette: Palette { red: 0xa3434d, orange: 0xa85d43, yellow: 0x8a6d1f, green: 0x5d7a45, cyan: 0x3b7a79, blue: 0x5e81ac, purple: 0x85667f, pink: 0x9a5a8c } },
            tokens: &[] },
    ] },
    Family { id: "rose-pine", name: "Rosé Pine", description: "All natural pine, faux fur and a bit of soho vibes.", variants: &[
        Scheme { id: "rose-pine", family: "rose-pine", name: "Main", mode: Mode::Dark, source: "https://rosepinetheme.com/palette/",
            seeds: Seeds { canvas: 0x191724, surface: 0x1f1d2e, overlay: 0x26233a, ink: 0xe0def4, muted: 0x908caa, accent: 0xebbcba,
                palette: Palette { red: 0xeb6f92, orange: 0xea9a97, yellow: 0xf6c177, green: 0x9ccfd8, cyan: 0x9ccfd8, blue: 0x31748f, purple: 0xc4a7e7, pink: 0xebbcba } },
            tokens: &[] },
        Scheme { id: "rose-pine-moon", family: "rose-pine", name: "Moon", mode: Mode::Dark, source: "https://rosepinetheme.com/palette/",
            seeds: Seeds { canvas: 0x232136, surface: 0x2a273f, overlay: 0x393552, ink: 0xe0def4, muted: 0x918dab, accent: 0xea9a97,
                palette: Palette { red: 0xeb6f92, orange: 0xea9a97, yellow: 0xf6c177, green: 0x9ccfd8, cyan: 0x9ccfd8, blue: 0x3e8fb0, purple: 0xc4a7e7, pink: 0xea9a97 } },
            tokens: &[] },
        Scheme { id: "rose-pine-dawn", family: "rose-pine", name: "Dawn", mode: Mode::Light, source: "https://rosepinetheme.com/palette/",
            seeds: Seeds { canvas: 0xf2e9e1, surface: 0xfffaf3, overlay: 0xdfdad9, ink: 0x575279, muted: 0x6a6683, accent: 0xd47873,
                palette: Palette { red: 0xb4637a, orange: 0xd7827e, yellow: 0xea9d34, green: 0x56949f, cyan: 0x56949f, blue: 0x286983, purple: 0x907aa9, pink: 0xd7827e } },
            tokens: &[] },
    ] },
    Family { id: "gruvbox", name: "Gruvbox", description: "Retro groove with warm, earthy contrast.", variants: &[
        Scheme { id: "gruvbox-dark", family: "gruvbox", name: "Dark", mode: Mode::Dark, source: "https://github.com/morhetz/gruvbox",
            seeds: Seeds { canvas: 0x1d2021, surface: 0x282828, overlay: 0x3c3836, ink: 0xebdbb2, muted: 0xbdae93, accent: 0xfabd2f,
                palette: Palette { red: 0xfb4934, orange: 0xfe8019, yellow: 0xfabd2f, green: 0xb8bb26, cyan: 0x8ec07c, blue: 0x83a598, purple: 0xd3869b, pink: 0xd3869b } },
            tokens: &[] },
        Scheme { id: "gruvbox-light", family: "gruvbox", name: "Light", mode: Mode::Light, source: "https://github.com/morhetz/gruvbox",
            seeds: Seeds { canvas: 0xf2e5bc, surface: 0xfbf1c7, overlay: 0xd5c4a1, ink: 0x3c3836, muted: 0x665c54, accent: 0xaf3a03,
                palette: Palette { red: 0x9d0006, orange: 0xaf3a03, yellow: 0xb57614, green: 0x79740e, cyan: 0x427b58, blue: 0x076678, purple: 0x8f3f71, pink: 0x8f3f71 } },
            tokens: &[] },
    ] },
    Family { id: "dracula", name: "Dracula", description: "Dark, vivid and a little theatrical.", variants: &[
        Scheme { id: "dracula", family: "dracula", name: "Dracula", mode: Mode::Dark, source: "https://draculatheme.com/contribute",
            seeds: Seeds { canvas: 0x21222c, surface: 0x282a36, overlay: 0x44475a, ink: 0xf8f8f2, muted: 0xa3abd1, accent: 0xbd93f9,
                palette: Palette { red: 0xff5555, orange: 0xffb86c, yellow: 0xf1fa8c, green: 0x50fa7b, cyan: 0x8be9fd, blue: 0x8aa0f2, purple: 0xbd93f9, pink: 0xff79c6 } },
            tokens: &[] },
        Scheme { id: "alucard", family: "dracula", name: "Alucard", mode: Mode::Light, source: "https://draculatheme.com/contribute",
            seeds: Seeds { canvas: 0xf5f1de, surface: 0xfffbeb, overlay: 0xdedccf, ink: 0x1f1f1f, muted: 0x635d97, accent: 0x644ac9,
                palette: Palette { red: 0xcb3a2a, orange: 0xa34d14, yellow: 0x846e15, green: 0x14710a, cyan: 0x036a96, blue: 0x3a56b8, purple: 0x644ac9, pink: 0xa3144d } },
            tokens: &[] },
    ] },
    Family { id: "solarized", name: "Solarized", description: "Precision colours tuned for readability.", variants: &[
        Scheme { id: "solarized-dark", family: "solarized", name: "Dark", mode: Mode::Dark, source: "https://ethanschoonover.com/solarized/",
            seeds: Seeds { canvas: 0x002b36, surface: 0x073642, overlay: 0x0e4a59, ink: 0xeee8d5, muted: 0x93a1a1, accent: 0x268bd2,
                palette: Palette { red: 0xdc322f, orange: 0xcb4b16, yellow: 0xb58900, green: 0x859900, cyan: 0x2aa198, blue: 0x268bd2, purple: 0x6c71c4, pink: 0xd33682 } },
            tokens: &[] },
        Scheme { id: "solarized-light", family: "solarized", name: "Light", mode: Mode::Light, source: "https://ethanschoonover.com/solarized/",
            seeds: Seeds { canvas: 0xeee8d5, surface: 0xfdf6e3, overlay: 0xddd6c1, ink: 0x073642, muted: 0x566c73, accent: 0x268bd2,
                palette: Palette { red: 0xdc322f, orange: 0xcb4b16, yellow: 0xb58900, green: 0x859900, cyan: 0x2aa198, blue: 0x268bd2, purple: 0x6c71c4, pink: 0xd33682 } },
            tokens: &[] },
    ] },
    Family { id: "everforest", name: "Everforest", description: "Green-based, warm and easy on the eyes.", variants: &[
        Scheme { id: "everforest-dark", family: "everforest", name: "Dark", mode: Mode::Dark, source: "https://github.com/sainnhe/everforest/blob/master/palette.md",
            seeds: Seeds { canvas: 0x232a2e, surface: 0x2d353b, overlay: 0x3d484d, ink: 0xd3c6aa, muted: 0x9da9a0, accent: 0xa7c080,
                palette: Palette { red: 0xe67e80, orange: 0xe69875, yellow: 0xdbbc7f, green: 0xa7c080, cyan: 0x83c092, blue: 0x7fbbb3, purple: 0xd699b6, pink: 0xd699b6 } },
            tokens: &[] },
        Scheme { id: "everforest-light", family: "everforest", name: "Light", mode: Mode::Light, source: "https://github.com/sainnhe/everforest/blob/master/palette.md",
            seeds: Seeds { canvas: 0xefebd4, surface: 0xfdf6e3, overlay: 0xe0dcc7, ink: 0x5c6a72, muted: 0x606d5f, accent: 0x859801,
                palette: Palette { red: 0xf85552, orange: 0xf57d26, yellow: 0xdfa000, green: 0x8da101, cyan: 0x35a77c, blue: 0x3a94c5, purple: 0xdf69ba, pink: 0xdf69ba } },
            tokens: &[] },
    ] },
    Family { id: "kanagawa", name: "Kanagawa", description: "Ink and paper, after Hokusai’s Great Wave.", variants: &[
        Scheme { id: "kanagawa-wave", family: "kanagawa", name: "Wave", mode: Mode::Dark, source: "https://github.com/rebelot/kanagawa.nvim",
            seeds: Seeds { canvas: 0x16161d, surface: 0x1f1f28, overlay: 0x363646, ink: 0xdcd7ba, muted: 0xa6a69c, accent: 0x7e9cd8,
                palette: Palette { red: 0xff5d62, orange: 0xffa066, yellow: 0xe6c384, green: 0x98bb6c, cyan: 0x7aa89f, blue: 0x7e9cd8, purple: 0x957fb8, pink: 0xd27e99 } },
            tokens: &[] },
        Scheme { id: "kanagawa-lotus", family: "kanagawa", name: "Lotus", mode: Mode::Light, source: "https://github.com/rebelot/kanagawa.nvim",
            seeds: Seeds { canvas: 0xe7dba0, surface: 0xf2ecbc, overlay: 0xdcd5ac, ink: 0x545464, muted: 0x636055, accent: 0x4d699b,
                palette: Palette { red: 0xc84053, orange: 0xcc6d00, yellow: 0x836f4a, green: 0x6f894e, cyan: 0x597b75, blue: 0x4d699b, purple: 0x624c83, pink: 0xb35b79 } },
            tokens: &[] },
    ] },
    Family { id: "github", name: "GitHub", description: "Familiar, neutral and high contrast.", variants: &[
        Scheme { id: "github-dark", family: "github", name: "Dark", mode: Mode::Dark, source: "https://primer.style/foundations/color",
            seeds: Seeds { canvas: 0x010409, surface: 0x0d1117, overlay: 0x30363d, ink: 0xe6edf3, muted: 0x8d96a0, accent: 0x4493f8,
                palette: Palette { red: 0xf85149, orange: 0xdb6d28, yellow: 0xd29922, green: 0x3fb950, cyan: 0x39c5cf, blue: 0x4493f8, purple: 0xab7df8, pink: 0xdb61a2 } },
            tokens: &[] },
        Scheme { id: "github-light", family: "github", name: "Light", mode: Mode::Light, source: "https://primer.style/foundations/color",
            seeds: Seeds { canvas: 0xf6f8fa, surface: 0xffffff, overlay: 0xd1d9e0, ink: 0x1f2328, muted: 0x59636e, accent: 0x0969da,
                palette: Palette { red: 0xd1242f, orange: 0xbc4c00, yellow: 0x9a6700, green: 0x1a7f37, cyan: 0x1b7c83, blue: 0x0969da, purple: 0x8250df, pink: 0xbf3989 } },
            tokens: &[] },
    ] },
];

// ---- Settings ------------------------------------------------------------------------------

/// The accent presets; any of them, or a custom colour, is adjusted to read
/// on every scheme.
pub const ACCENTS: &[(AppearanceAccent, u32)] = &[
    (AppearanceAccent::Lavender, 0xa78bfa),
    (AppearanceAccent::Indigo, 0x6366f1),
    (AppearanceAccent::Blue, 0x3b82f6),
    (AppearanceAccent::Sky, 0x0ea5e9),
    (AppearanceAccent::Teal, 0x14b8a6),
    (AppearanceAccent::Emerald, 0x10b981),
    (AppearanceAccent::Lime, 0x84cc16),
    (AppearanceAccent::Amber, 0xf59e0b),
    (AppearanceAccent::Orange, 0xf97316),
    (AppearanceAccent::Rose, 0xf43f5e),
    (AppearanceAccent::Pink, 0xec4899),
    (AppearanceAccent::Fuchsia, 0xd946ef),
];

/// The interface fonts in the order they are offered.
pub const FONTS: &[AppearanceFont] = &[
    AppearanceFont::Geist,
    AppearanceFont::Inter,
    AppearanceFont::IbmMinusPlex,
    AppearanceFont::Manrope,
    AppearanceFont::Nunito,
    AppearanceFont::Atkinson,
    AppearanceFont::SourceMinusSans,
    AppearanceFont::System,
];

/// The font family an interface font draws with, as the bundled files name
/// it; `None` is the system's own font.
pub fn font_family(font: AppearanceFont) -> Option<&'static str> {
    Some(match font {
        AppearanceFont::Geist | AppearanceFont::Unknown => "Geist",
        AppearanceFont::Inter => "Inter",
        AppearanceFont::IbmMinusPlex => "IBM Plex Sans",
        AppearanceFont::Manrope => "Manrope",
        AppearanceFont::Nunito => "Nunito",
        AppearanceFont::Atkinson => "Atkinson Hyperlegible Next",
        AppearanceFont::SourceMinusSans => "Source Sans 3",
        AppearanceFont::System => return None,
    })
}

/// The font family a message font draws with (package S2 offers them; the
/// files are bundled with the interface fonts). `None` follows the interface.
pub fn message_font_family(font: AppearanceMessageFont) -> Option<&'static str> {
    Some(match font {
        AppearanceMessageFont::Interface | AppearanceMessageFont::Unknown => return None,
        AppearanceMessageFont::Atkinson => "Atkinson Hyperlegible Next",
        AppearanceMessageFont::Lexend => "Lexend",
        AppearanceMessageFont::Serif => "Source Serif 4",
        AppearanceMessageFont::Literata => "Literata",
        AppearanceMessageFont::JetbrainsMinusMono => "JetBrains Mono",
        AppearanceMessageFont::GeistMinusMono => "Geist Mono",
        AppearanceMessageFont::AtkinsonMinusMono => "Atkinson Hyperlegible Mono",
    })
}

/// A setting's stored value (`"ibm-plex"`, `"lime"`), for element IDs.
pub fn key<T: Serialize>(value: &T) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

/// The monospace family for code.
pub const MONO_FAMILY: &str = "Geist Mono";

/// Interface scale in percent.
pub const SCALE: std::ops::RangeInclusive<i64> = 85..=150;
pub const DEFAULT_SCALE: i64 = 100;

/// The interface settings this section shows, cleaned: every field valid.
#[derive(Debug, Clone, PartialEq)]
pub struct Look {
    pub mode: AppearanceMode,
    pub scheme_dark: String,
    pub scheme_light: String,
    pub accent: AppearanceAccent,
    pub accent_custom: Option<u32>,
    pub font: AppearanceFont,
    pub scale: i64,
    pub density: AppearanceDensity,
    pub corners: AppearanceCorners,
    pub motion: AppearanceMotion,
    /// The personal theme in use, by ID in the account's library.
    pub theme: Option<String>,
}

impl Default for Look {
    fn default() -> Self {
        Self {
            mode: AppearanceMode::System,
            scheme_dark: DEFAULT_DARK.into(),
            scheme_light: DEFAULT_LIGHT.into(),
            accent: AppearanceAccent::Scheme,
            accent_custom: None,
            font: AppearanceFont::Geist,
            scale: DEFAULT_SCALE,
            density: AppearanceDensity::Comfortable,
            corners: AppearanceCorners::Rounded,
            motion: AppearanceMotion::System,
            theme: None,
        }
    }
}

fn known<T: Copy + PartialEq>(value: Option<T>, unknown: T, fallback: T) -> T {
    value.filter(|v| *v != unknown).unwrap_or(fallback)
}

/// A theme ID as the backend accepts it.
pub fn is_theme_id(id: &str) -> bool {
    (1..=40).contains(&id.len())
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

impl Look {
    /// Clean a stored appearance (`sanitizeAppearance`): unknown or invalid
    /// values fall back to the defaults.
    pub fn of(a: &Appearance) -> Self {
        let d = Self::default();
        let scheme = |id: &Option<String>, mode: Mode, fallback: String| {
            id.as_deref()
                .and_then(find_scheme)
                .filter(|s| s.mode == mode)
                .map(|s| s.id.to_string())
                .unwrap_or(fallback)
        };
        Self {
            mode: known(a.mode, AppearanceMode::Unknown, d.mode),
            scheme_dark: scheme(&a.scheme_dark, Mode::Dark, d.scheme_dark),
            scheme_light: scheme(&a.scheme_light, Mode::Light, d.scheme_light),
            accent: known(a.accent, AppearanceAccent::Unknown, d.accent),
            accent_custom: a.accent_custom.as_deref().and_then(parse_hex),
            font: known(a.font, AppearanceFont::Unknown, d.font),
            scale: a
                .interface_scale
                .map(|s| s.clamp(*SCALE.start(), *SCALE.end()))
                .unwrap_or(d.scale),
            density: known(a.density, AppearanceDensity::Unknown, d.density),
            corners: known(a.corners, AppearanceCorners::Unknown, d.corners),
            motion: known(a.motion, AppearanceMotion::Unknown, d.motion),
            theme: a.theme.clone().filter(|id| is_theme_id(id)),
        }
    }

    /// Write these interface settings into `a`, keeping its other keys (the
    /// message settings S2 shows).
    pub fn write(&self, a: &mut Appearance) {
        a.mode = Some(self.mode);
        a.scheme_dark = Some(self.scheme_dark.clone());
        a.scheme_light = Some(self.scheme_light.clone());
        a.accent = Some(self.accent);
        a.accent_custom = self.accent_custom.map(to_hex);
        a.font = Some(self.font);
        a.interface_scale = Some(self.scale);
        a.density = Some(self.density);
        a.corners = Some(self.corners);
        a.motion = Some(self.motion);
        a.theme = self.theme.clone();
    }

    /// The colour the accent is derived from, or `None` for the scheme's own.
    pub fn accent_source(&self) -> Option<u32> {
        match self.accent {
            AppearanceAccent::Custom => self.accent_custom,
            preset => ACCENTS.iter().find(|(a, _)| *a == preset).map(|(_, c)| *c),
        }
    }

    /// Light or dark: the chosen mode, or the system's when following it.
    pub fn resolve_mode(&self, system: Mode) -> Mode {
        match self.mode {
            AppearanceMode::Dark => Mode::Dark,
            AppearanceMode::Light => Mode::Light,
            _ => system,
        }
    }

    /// The scheme chosen for `mode`.
    pub fn scheme(&self, mode: Mode) -> &'static Scheme {
        match mode {
            Mode::Dark => scheme_for(&self.scheme_dark, mode),
            Mode::Light => scheme_for(&self.scheme_light, mode),
        }
    }

    /// Whether motion is reduced; `system` follows `system_reduced`.
    pub fn reduced_motion(&self, system_reduced: bool) -> bool {
        match self.motion {
            AppearanceMotion::Reduced => true,
            AppearanceMotion::Full => false,
            _ => system_reduced,
        }
    }

    /// Corner radii in pixels: rows and controls, then cards and dialogs.
    pub fn radii(&self) -> (f32, f32) {
        match self.corners {
            AppearanceCorners::Soft => (5., 8.),
            AppearanceCorners::Square => (2., 3.),
            _ => (8., 12.),
        }
    }
}

/// Reset the interface settings to the defaults, keeping the message
/// settings and the personal theme library.
pub fn reset_look(a: &mut Appearance) {
    Look::default().write(a);
}

// ---- This device ---------------------------------------------------------------------------

/// This device's copy, kept in the app's state file. With `sync` on it
/// follows the account (and is shown at start before the account's copy
/// arrives); with it off this device keeps its own look.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Device {
    pub sync: bool,
    pub settings: Option<Appearance>,
}

impl Default for Device {
    fn default() -> Self {
        Self {
            sync: true,
            settings: None,
        }
    }
}

impl Device {
    /// What to show: with sync on, the account's copy once it is read, else
    /// this device's own.
    pub fn effective(&self, account: Option<&StoredAppearance>) -> Appearance {
        let account = account.and_then(|a| a.settings.as_ref());
        match (self.sync, account) {
            (true, Some(settings)) => settings.clone(),
            _ => self.settings.clone().unwrap_or_default(),
        }
    }

    /// The account's copy changed: with sync on, remember it so the next start
    /// shows it at once. True when this copy changed and should be written.
    pub fn follow(&mut self, account: Option<&StoredAppearance>) -> bool {
        let Some(settings) = account.and_then(|a| a.settings.as_ref()) else {
            return false;
        };
        if !self.sync || self.settings.as_ref() == Some(settings) {
            return false;
        }
        self.settings = Some(settings.clone());
        true
    }
}

// ---- Applying ------------------------------------------------------------------------------

/// Everything the app needs to draw one appearance in one mode.
#[derive(Debug, Clone, PartialEq)]
pub struct Compiled {
    pub mode: Mode,
    /// The scheme drawn: the chosen one, or a personal theme's base.
    pub scheme: &'static str,
    pub tokens: Tokens,
    /// `None` is the system font.
    pub font: Option<&'static str>,
    /// Interface scale, 1.0 = 100 %.
    pub scale: f32,
    pub radius: f32,
    pub radius_large: f32,
    pub compact: bool,
    pub reduced_motion: bool,
    /// The personal theme drawn, when one is in use and this client can draw it.
    pub theme: Option<String>,
    /// What the theme's CSS set on the root, when it was applied
    /// ([`compile_with_css`]); `None` when not applied.
    pub css: Option<CssVariables>,
    /// The picture the theme in use draws behind the app, when it has one.
    pub backdrop: Option<Backdrop>,
}

/// Where a theme's picture is drawn (`placement` in the theme document).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackdropPlacement {
    /// Behind the open conversation only.
    Conversation,
    /// Behind the whole app; the shell's own surfaces turn translucent.
    App,
}

/// How the picture fills its area (`fit`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackdropFit {
    Cover,
    Contain,
    Tile,
}

/// Where the picture sits when it does not fill its area (`position`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackdropPosition {
    Center,
    Top,
    Bottom,
    Left,
    Right,
}

/// A personal theme's background, as the web draws it
/// (`backgroundDeclarations` in `shared/themes.ts`): the dim and the panel
/// opacity are already raised to the floors that keep text readable on the
/// picture's measured tones.
#[derive(Debug, Clone, PartialEq)]
pub struct Backdrop {
    /// An upload on the server: `/files/…` or `/api/v1/uploads/…`.
    pub path: String,
    pub placement: BackdropPlacement,
    pub fit: BackdropFit,
    pub position: BackdropPosition,
    /// How much of the main surface's colour covers the picture, 0 to 1.
    pub dim: f32,
    /// Blur radius in pixels.
    pub blur: f32,
    /// How much the shell's surfaces cover an app-wide picture, 0 to 1.
    pub panel: f32,
}

/// Whether `path` is an upload on the server a theme may name
/// (`instanceFilePath` in `shared/themes.ts`): no query, no host.
pub fn is_upload_path(path: &str) -> bool {
    background::is_upload_path(path)
}

/// The lowest dim and panel opacity, in percent, that keep text readable
/// over a picture with these dark and light tones (`backgroundFloors`).
fn backdrop_floors(tokens: &Tokens, tones: Option<[u32; 2]>) -> (i64, i64) {
    let Some(tones) = tones else {
        return (60, 85);
    };
    let readable = |surface: u32| {
        contrast(tokens.text, surface) >= 4.5
            && contrast(tokens.body, surface) >= 4.5
            && contrast(tokens.muted, surface) >= 3.
    };
    let lowest = |min: u32, max: u32, cover: u32| {
        (min..=max)
            .step_by(5)
            .find(|percent| {
                tones
                    .iter()
                    .all(|tone| readable(mix(*tone, cover, f64::from(*percent) / 100.)))
            })
            .unwrap_or(max)
    };
    (
        i64::from(lowest(0, 90, tokens.main)),
        i64::from(lowest(40, 100, tokens.sidebar)),
    )
}

/// A background's two measured tones, dark then light, as colours.
fn background_tones(background: &ThemeBackground) -> Option<[u32; 2]> {
    match background.tones.as_deref() {
        Some([dark, light]) => parse_hex(dark).zip(parse_hex(light)).map(|(d, l)| [d, l]),
        _ => None,
    }
}

/// The lowest dim and panel opacity, in percent, the editor offers for
/// `background` (`backgroundFloors`).
pub fn background_floors(tokens: &Tokens, background: &ThemeBackground) -> (i64, i64) {
    backdrop_floors(tokens, background_tones(background))
}

/// A background with nothing chosen yet (`defaultBackground`).
pub fn default_background() -> ThemeBackground {
    use ircord_contract::{
        ThemeBackgroundFit as F, ThemeBackgroundPlacement as P, ThemeBackgroundPosition as Q,
    };
    ThemeBackground {
        url: None,
        placement: Some(P::Conversation),
        fit: Some(F::Cover),
        position: Some(Q::Center),
        dim: Some(55),
        blur: Some(0),
        panel_opacity: Some(85),
        tones: None,
    }
}

/// The path of an upload, read against `server`, from the address the upload endpoint
/// returned (`instanceFilePath`): its path alone, `None` when it has a query
/// or fragment or is not an upload's path.
pub fn instance_file_path(address: &str, server: &url::Url) -> Option<String> {
    background::instance_upload_path(address, server)
}

/// The dark and light tones of a picture shrunk to a few pixels, as RGBA
/// bytes (`measureTones`): the colours a tenth from either end of the
/// luminance order. `None` when nothing is opaque.
pub fn measure_tones(pixels: &[u8]) -> Option<[String; 2]> {
    let mut samples: Vec<(f64, [u8; 3])> = pixels
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|p| p[3] >= 16)
        .map(|p| {
            let (r, g, b) = (f64::from(p[0]), f64::from(p[1]), f64::from(p[2]));
            (0.2126 * r + 0.7152 * g + 0.0722 * b, [p[0], p[1], p[2]])
        })
        .collect();
    if samples.is_empty() {
        return None;
    }
    samples.sort_by(|a, b| a.0.total_cmp(&b.0));
    let at = |q: f64| {
        let [r, g, b] = samples[((q * samples.len() as f64) as usize).min(samples.len() - 1)].1;
        format!("#{r:02x}{g:02x}{b:02x}")
    };
    Some([at(0.1), at(0.9)])
}

/// The backdrop `theme` asks for, when it names a usable picture.
pub fn theme_backdrop(theme: &Theme, tokens: &Tokens) -> Option<Backdrop> {
    use ircord_contract::{
        ThemeBackgroundFit as F, ThemeBackgroundPlacement as P, ThemeBackgroundPosition as Q,
    };
    let background = theme.background.as_ref()?;
    let path = background.url.as_deref().filter(|u| is_upload_path(u))?;
    let (dim_floor, panel_floor) = background_floors(tokens, background);
    let percent = |value: Option<i64>, min: i64, max: i64, default: i64, floor: i64| {
        value.map_or(default, |v| v.clamp(min, max)).max(floor) as f32 / 100.
    };
    Some(Backdrop {
        path: path.to_string(),
        placement: match background.placement {
            Some(P::App) => BackdropPlacement::App,
            _ => BackdropPlacement::Conversation,
        },
        fit: match background.fit {
            Some(F::Contain) => BackdropFit::Contain,
            Some(F::Tile) => BackdropFit::Tile,
            _ => BackdropFit::Cover,
        },
        position: match background.position {
            Some(Q::Top) => BackdropPosition::Top,
            Some(Q::Bottom) => BackdropPosition::Bottom,
            Some(Q::Left) => BackdropPosition::Left,
            Some(Q::Right) => BackdropPosition::Right,
            _ => BackdropPosition::Center,
        },
        dim: percent(background.dim, 0, 90, 55, dim_floor),
        blur: background.blur.map_or(0, |b| b.clamp(0, 20)) as f32,
        panel: percent(background.panel_opacity, 40, 100, 85, panel_floor),
    })
}

/// Compile `look` for `mode` (`compileAppearance`). `themes` is the account's
/// library; a theme in use replaces the scheme's seeds. Its CSS is not
/// applied; see [`compile_with_css`].
pub fn compile(look: &Look, mode: Mode, themes: &[Theme], system_reduced: bool) -> Compiled {
    compile_with_css(look, mode, themes, system_reduced, false)
}

/// [`compile`], and when `css_allowed` (the account's switch is on and the
/// server allows CSS) the theme in use's CSS sets the variables it names on
/// top: colours through the tokens, corners and interface scale over the
/// look's. Fonts and message sizes are left to the app, which knows what
/// the system has (`theme_css` in this crate).
pub fn compile_with_css(
    look: &Look,
    mode: Mode,
    themes: &[Theme],
    system_reduced: bool,
    css_allowed: bool,
) -> Compiled {
    let theme = look
        .theme
        .as_deref()
        .and_then(|id| themes.iter().find(|t| t.id.as_deref() == Some(id)))
        .filter(|t| theme_supported(t));
    let accent = look.accent_source();
    let (scheme, mut tokens) = match theme {
        Some(theme) => {
            let base = theme_base(theme, mode);
            let tokens = match theme_seeds(theme, mode) {
                Some(seeds) => self::tokens(&seeds, mode, &[], accent),
                None => scheme_tokens(base, accent),
            };
            (base.id, tokens)
        }
        None => {
            let scheme = look.scheme(mode);
            (scheme.id, scheme_tokens(scheme, accent))
        }
    };
    let (mut radius, mut radius_large) = look.radii();
    let mut scale = look.scale as f32 / 100.;
    let mut css = None;
    if let Some(theme) = theme.filter(|t| css_allowed && has_css(t)) {
        let vars = theme_css::theme_variables(theme.css.as_deref().unwrap_or_default(), &|name| {
            tokens.token(name)
        });
        for (name, rgb) in &vars.colors {
            if let Some(slot) = tokens.slot(name) {
                *slot = *rgb;
            }
        }
        if let Some(px) = vars.radius {
            radius = px;
        }
        if let Some(px) = vars.radius_large {
            radius_large = px;
        }
        if let Some(factor) = vars.interface_scale {
            scale = factor;
        }
        css = Some(vars);
    }
    let backdrop = theme.and_then(|t| theme_backdrop(t, &tokens));
    Compiled {
        mode,
        scheme,
        tokens,
        font: font_family(look.font),
        scale,
        radius,
        radius_large,
        compact: look.density == AppearanceDensity::Compact,
        reduced_motion: look.reduced_motion(system_reduced),
        theme: theme.and_then(|t| t.id.clone()),
        css,
        backdrop,
    }
}

// ---- Personal themes (shared/themes.ts) ----------------------------------------------------

/// The theme format this client writes and draws; a newer one is not drawn.
pub const THEME_FORMAT: i64 = 1;
/// How many themes the library holds.
pub const MAX_THEMES: usize = 20;
pub const MAX_THEME_NAME: usize = 60;
/// Custom CSS: only the root variables it sets are applied ([`compile_with_css`]).
pub const MAX_THEME_CSS: usize = 64 * 1024;
/// A file or pasted text offered for import.
pub const MAX_IMPORT: usize = 256 * 1024;

/// Whether this client can draw `theme`.
pub fn theme_supported(theme: &Theme) -> bool {
    theme.format.unwrap_or(THEME_FORMAT) <= THEME_FORMAT
}

/// The scheme a theme is based on in `mode`.
pub fn theme_base(theme: &Theme, mode: Mode) -> &'static Scheme {
    let base = theme.base.as_ref();
    let id = match mode {
        Mode::Dark => base.and_then(|b| b.dark.as_deref()),
        Mode::Light => base.and_then(|b| b.light.as_deref()),
    };
    scheme_for(id.unwrap_or(""), mode)
}

/// A theme's own seeds in `mode`, when it has valid ones.
pub fn theme_seeds(theme: &Theme, mode: Mode) -> Option<Seeds> {
    let colors = theme.colors.as_ref()?;
    let seeds = match mode {
        Mode::Dark => colors.dark.as_ref(),
        Mode::Light => colors.light.as_ref(),
    }?;
    seeds_from(seeds)
}

fn seeds_from(s: &ThemeSeeds) -> Option<Seeds> {
    let p = &s.palette;
    Some(Seeds {
        canvas: parse_hex(&s.canvas)?,
        surface: parse_hex(&s.surface)?,
        overlay: parse_hex(&s.overlay)?,
        ink: parse_hex(&s.ink)?,
        muted: parse_hex(&s.muted)?,
        accent: parse_hex(&s.accent)?,
        palette: Palette {
            red: parse_hex(&p.red)?,
            orange: parse_hex(&p.orange)?,
            yellow: parse_hex(&p.yellow)?,
            green: parse_hex(&p.green)?,
            cyan: parse_hex(&p.cyan)?,
            blue: parse_hex(&p.blue)?,
            purple: parse_hex(&p.purple)?,
            pink: parse_hex(&p.pink)?,
        },
    })
}

/// Seeds as the theme document stores them.
pub fn seeds_to(s: &Seeds) -> ThemeSeeds {
    let p = &s.palette;
    ThemeSeeds {
        canvas: to_hex(s.canvas),
        surface: to_hex(s.surface),
        overlay: to_hex(s.overlay),
        ink: to_hex(s.ink),
        muted: to_hex(s.muted),
        accent: to_hex(s.accent),
        palette: ThemePalette {
            red: to_hex(p.red),
            orange: to_hex(p.orange),
            yellow: to_hex(p.yellow),
            green: to_hex(p.green),
            cyan: to_hex(p.cyan),
            blue: to_hex(p.blue),
            purple: to_hex(p.purple),
            pink: to_hex(p.pink),
        },
    }
}

/// The seed a theme editor changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Seed {
    Canvas,
    Surface,
    Overlay,
    Ink,
    Muted,
    Accent,
    Red,
    Orange,
    Yellow,
    Green,
    Cyan,
    Blue,
    Purple,
    Pink,
}

impl Seed {
    pub const INTERFACE: [Seed; 6] = [
        Seed::Canvas,
        Seed::Surface,
        Seed::Overlay,
        Seed::Ink,
        Seed::Muted,
        Seed::Accent,
    ];
    pub const PALETTE: [Seed; 8] = [
        Seed::Red,
        Seed::Orange,
        Seed::Yellow,
        Seed::Green,
        Seed::Cyan,
        Seed::Blue,
        Seed::Purple,
        Seed::Pink,
    ];

    pub fn get(self, s: &Seeds) -> u32 {
        match self {
            Seed::Canvas => s.canvas,
            Seed::Surface => s.surface,
            Seed::Overlay => s.overlay,
            Seed::Ink => s.ink,
            Seed::Muted => s.muted,
            Seed::Accent => s.accent,
            Seed::Red => s.palette.red,
            Seed::Orange => s.palette.orange,
            Seed::Yellow => s.palette.yellow,
            Seed::Green => s.palette.green,
            Seed::Cyan => s.palette.cyan,
            Seed::Blue => s.palette.blue,
            Seed::Purple => s.palette.purple,
            Seed::Pink => s.palette.pink,
        }
    }

    pub fn set(self, s: &mut Seeds, value: u32) {
        *match self {
            Seed::Canvas => &mut s.canvas,
            Seed::Surface => &mut s.surface,
            Seed::Overlay => &mut s.overlay,
            Seed::Ink => &mut s.ink,
            Seed::Muted => &mut s.muted,
            Seed::Accent => &mut s.accent,
            Seed::Red => &mut s.palette.red,
            Seed::Orange => &mut s.palette.orange,
            Seed::Yellow => &mut s.palette.yellow,
            Seed::Green => &mut s.palette.green,
            Seed::Cyan => &mut s.palette.cyan,
            Seed::Blue => &mut s.palette.blue,
            Seed::Purple => &mut s.palette.purple,
            Seed::Pink => &mut s.palette.pink,
        } = value;
    }
}

/// Change one seed of `theme` in `mode`, starting from its base's seeds when
/// the theme has none of its own for that mode yet.
pub fn set_seed(theme: &mut Theme, mode: Mode, seed: Seed, value: u32) {
    let mut seeds = drawn_seeds(theme, mode);
    seed.set(&mut seeds, value);
    set_theme_seeds(theme, mode, Some(seeds));
}

/// Store a mode's own colors, or return that mode to its base palette.
pub fn set_theme_seeds(theme: &mut Theme, mode: Mode, seeds: Option<Seeds>) {
    if seeds.is_none() && theme.colors.is_none() {
        return;
    }
    let colors = theme.colors.get_or_insert_default();
    let stored = seeds.as_ref().map(seeds_to);
    match mode {
        Mode::Dark => colors.dark = stored,
        Mode::Light => colors.light = stored,
    }
}

/// Change one mode's base scheme without changing either mode's custom colors.
pub fn set_theme_base(theme: &mut Theme, scheme: &'static Scheme) {
    let base = theme.base.get_or_insert_default();
    let id = Some(scheme.id.to_string());
    match scheme.mode {
        Mode::Dark => base.dark = id,
        Mode::Light => base.light = id,
    }
}

/// The seeds a theme draws with in `mode`: its own, or its base's.
pub fn drawn_seeds(theme: &Theme, mode: Mode) -> Seeds {
    theme_seeds(theme, mode).unwrap_or(theme_base(theme, mode).seeds)
}

/// A new theme ID.
pub fn new_theme_id() -> String {
    format!("t-{:018x}", rand::random::<u64>() >> 8)
}

/// A new theme based on the schemes `look` uses, named `name`.
pub fn new_theme(look: &Look, name: &str, made_with: &str, now: &str) -> Theme {
    Theme {
        format: Some(THEME_FORMAT),
        id: Some(new_theme_id()),
        name: Some(clean_name(name)),
        base: Some(ThemeBase {
            dark: Some(look.scheme(Mode::Dark).id.to_string()),
            light: Some(look.scheme(Mode::Light).id.to_string()),
        }),
        colors: Some(ThemeColors::default()),
        background: None,
        css: Some(String::new()),
        made_with: Some(made_with.to_string()),
        updated_at: Some(now.to_string()),
    }
}

/// Copy a theme within the account, retaining its background image and appearance.
pub fn duplicate_theme(theme: &Theme, name: &str, made_with: &str, now: &str) -> Theme {
    Theme {
        id: Some(new_theme_id()),
        name: Some(clean_name(name)),
        made_with: Some(made_with.to_string()),
        updated_at: Some(now.to_string()),
        ..theme.clone()
    }
}

fn clean_name(name: &str) -> String {
    let name: String = name.chars().take(MAX_THEME_NAME).collect();
    let name = name.trim();
    if name.is_empty() {
        "Untitled theme".into()
    } else {
        name.to_string()
    }
}

fn truncate_bytes(text: &str, max: usize) -> String {
    let mut end = text.len().min(max);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_string()
}

/// What an exported `.ircord-theme.json` holds. The image itself and its
/// address are never included.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ThemeFile {
    #[serde(rename = "ircordTheme")]
    pub ircord_theme: i64,
    pub name: String,
    pub base: ThemeBase,
    pub colors: ThemeColors,
    pub background: Option<serde_json::Value>,
    pub css: String,
    #[serde(rename = "madeWith")]
    pub made_with: String,
}

/// The JSON a theme exports as.
pub fn export_theme(theme: &Theme) -> String {
    let background = theme.background.as_ref().map(|b| {
        let mut value = serde_json::to_value(b).unwrap_or_default();
        if let Some(fields) = value.as_object_mut() {
            fields.remove("url");
            fields.remove("tones");
        }
        value
    });
    let file = ThemeFile {
        ircord_theme: theme.format.unwrap_or(THEME_FORMAT),
        name: theme.name.clone().unwrap_or_default(),
        base: theme.base.clone().unwrap_or_default(),
        colors: theme.colors.clone().unwrap_or_default(),
        background,
        css: theme.css.clone().unwrap_or_default(),
        made_with: theme.made_with.clone().unwrap_or_default(),
    };
    serde_json::to_string_pretty(&file).unwrap_or_default()
}

/// The file name a theme exports to.
pub fn theme_file_name(theme: &Theme) -> String {
    let mut slug = String::new();
    for c in theme.name.as_deref().unwrap_or("").to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c);
        } else if !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let slug = slug.trim_matches('-');
    format!(
        "{}.ircord-theme.json",
        if slug.is_empty() { "theme" } else { slug }
    )
}

/// Why a file or text could not be imported.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportError {
    TooLarge,
    NotJson,
    NotTheme,
    /// Made by a newer IRCord than this one.
    Newer,
    CssTooLong,
}

/// Read an exported theme into a new, unused library entry with its own ID.
/// An imported background never carries an image: one has to be chosen here.
pub fn import_theme(raw: &str, made_with: &str, now: &str) -> Result<Theme, ImportError> {
    if raw.len() > MAX_IMPORT {
        return Err(ImportError::TooLarge);
    }
    let value: serde_json::Value = serde_json::from_str(raw).map_err(|_| ImportError::NotJson)?;
    let Some(fields) = value.as_object() else {
        return Err(ImportError::NotTheme);
    };
    let format = match fields.get("ircordTheme").and_then(|v| v.as_i64()) {
        Some(f) if f >= 1 => f,
        _ => return Err(ImportError::NotTheme),
    };
    if format > THEME_FORMAT {
        return Err(ImportError::Newer);
    }
    let css = fields.get("css").and_then(|v| v.as_str()).unwrap_or("");
    if css.len() > MAX_THEME_CSS {
        return Err(ImportError::CssTooLong);
    }
    let text = |key: &str| fields.get(key).and_then(|v| v.as_str()).unwrap_or("");
    let base: ThemeBase = fields
        .get("base")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();
    let base = ThemeBase {
        dark: Some(
            scheme_for(base.dark.as_deref().unwrap_or(""), Mode::Dark)
                .id
                .into(),
        ),
        light: Some(
            scheme_for(base.light.as_deref().unwrap_or(""), Mode::Light)
                .id
                .into(),
        ),
    };
    let colors = fields.get("colors").and_then(|v| v.as_object());
    let seeds = |key: &str| {
        colors
            .and_then(|c| c.get(key))
            .and_then(|v| serde_json::from_value::<ThemeSeeds>(v.clone()).ok())
            .and_then(|s| seeds_from(&s))
            .map(|s| seeds_to(&s))
    };
    let background = fields.get("background").filter(|v| v.is_object()).map(|v| {
        let mut b: ThemeBackground = serde_json::from_value(v.clone()).unwrap_or_default();
        b.url = None;
        b.tones = None;
        b
    });
    let made = text("madeWith");
    Ok(Theme {
        format: Some(format),
        id: Some(new_theme_id()),
        name: Some(clean_name(text("name"))),
        base: Some(base),
        colors: Some(ThemeColors {
            dark: seeds("dark"),
            light: seeds("light"),
        }),
        background,
        css: Some(css.to_string()),
        made_with: Some(truncate_bytes(
            if made.is_empty() { made_with } else { made },
            40,
        )),
        updated_at: Some(now.to_string()),
    })
}

/// Whether a theme carries custom CSS (applied as far as [`compile_with_css`] reads it).
pub fn has_css(theme: &Theme) -> bool {
    theme.css.as_deref().is_some_and(|c| !c.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tokens the web client computes for every scheme with its own accent,
    /// lime and amber (`schemeTokens`), in its token order.
    const GOLDEN: &str = include_str!("appearance_golden.txt");

    fn row(t: &Tokens) -> [u32; 28] {
        [
            t.rail,
            t.sidebar,
            t.main,
            t.panel,
            t.raised,
            t.hover,
            t.input,
            t.border,
            t.control_border,
            t.text,
            t.body,
            t.muted,
            t.faint,
            t.accent,
            t.accent_ink,
            t.selection,
            t.mint,
            t.danger,
            t.yellow,
            t.code_keyword,
            t.code_string,
            t.code_number,
            t.code_comment,
            t.code_function,
            t.code_type,
            t.code_property,
            t.code_meta,
            t.code_deletion,
        ]
    }

    #[test]
    fn every_scheme_derives_the_web_clients_tokens() {
        let mut checked = 0;
        for line in GOLDEN.lines() {
            let mut parts = line.split(' ');
            let id = parts.next().unwrap();
            let accent = parts.next().unwrap();
            let expected: Vec<u32> = parts.map(|v| u32::from_str_radix(v, 16).unwrap()).collect();
            let scheme = find_scheme(id).unwrap_or_else(|| panic!("scheme {id}"));
            let accent = (accent != "-").then(|| parse_hex(accent).unwrap());
            let got = row(&scheme_tokens(scheme, accent));
            assert_eq!(got.to_vec(), expected, "{id} with accent {accent:?}");
            checked += 1;
        }
        assert_eq!(checked, schemes().count() * 3);
    }

    #[test]
    fn every_scheme_and_accent_reads_clearly() {
        let accents = std::iter::once(None).chain(ACCENTS.iter().map(|(_, c)| Some(*c)));
        let accents: Vec<_> = accents.chain([Some(0x000000), Some(0xffffff)]).collect();
        for scheme in schemes() {
            for accent in &accents {
                let t = scheme_tokens(scheme, *accent);
                for surface in t.surfaces() {
                    for (name, text) in [
                        ("text", t.text),
                        ("body", t.body),
                        ("muted", t.muted),
                        ("faint", t.faint),
                        ("accent", t.accent),
                        ("mint", t.mint),
                        ("danger", t.danger),
                        ("yellow", t.yellow),
                    ] {
                        let ratio = contrast(text, surface);
                        assert!(
                            ratio >= 4.5,
                            "{} {name} {} on {} is {ratio:.2} (accent {accent:?})",
                            scheme.id,
                            to_hex(text),
                            to_hex(surface)
                        );
                    }
                }
                for surface in [t.input, t.main, t.panel, t.raised, t.sidebar] {
                    assert!(contrast(t.control_border, surface) >= 3., "{}", scheme.id);
                }
                assert!(contrast(t.accent_ink, t.accent) >= 4.5, "{}", scheme.id);
            }
        }
    }

    #[test]
    fn every_family_has_a_dark_variant_and_ids_are_unique() {
        let ids: std::collections::HashSet<_> = schemes().map(|s| s.id).collect();
        assert_eq!(ids.len(), schemes().count());
        for family in FAMILIES {
            assert!(family.variants.iter().any(|s| s.mode == Mode::Dark));
            assert!(family.variants.iter().all(|s| s.family == family.id));
        }
        assert_eq!(
            scheme_for("catppuccin-mocha", Mode::Light).id,
            "catppuccin-latte"
        );
        assert_eq!(scheme_for("midnight", Mode::Light).id, DEFAULT_LIGHT);
        assert_eq!(scheme_for("nope", Mode::Dark).id, DEFAULT_DARK);
    }

    #[test]
    fn a_stored_appearance_is_cleaned_and_keeps_the_message_settings() {
        let stored: Appearance = serde_json::from_value(serde_json::json!({
            "mode": "sepia", "schemeDark": "catppuccin-latte", "schemeLight": "nord-light",
            "accent": "custom", "accentCustom": "#ABCDEF", "font": "inter",
            "interfaceScale": 400, "density": "compact", "corners": "square",
            "motion": "reduced", "theme": "bad id!", "messageSize": 17, "messageLayout": "irc"
        }))
        .unwrap();
        let look = Look::of(&stored);
        assert_eq!(look.mode, AppearanceMode::System);
        assert_eq!(look.scheme_dark, DEFAULT_DARK);
        assert_eq!(look.scheme_light, "nord-light");
        assert_eq!(look.accent_source(), Some(0xabcdef));
        assert_eq!(look.font, AppearanceFont::Inter);
        assert_eq!(look.scale, 150);
        assert_eq!(look.theme, None);
        assert_eq!(look.radii(), (2., 3.));

        let mut edited = stored.clone();
        let mut next = look.clone();
        next.font = AppearanceFont::Nunito;
        next.write(&mut edited);
        assert_eq!(edited.font, Some(AppearanceFont::Nunito));
        assert_eq!(edited.message_size, Some(17));
        reset_look(&mut edited);
        assert_eq!(Look::of(&edited), Look::default());
        assert_eq!(edited.message_size, Some(17));
    }

    #[test]
    fn the_device_copy_follows_the_account_only_with_sync_on() {
        let account = |font| StoredAppearance {
            settings: Some(Appearance {
                font: Some(font),
                ..Default::default()
            }),
            updated_at: Some("r1".into()),
        };
        let mut device = Device::default();
        assert_eq!(device.effective(None), Appearance::default());
        assert!(device.follow(Some(&account(AppearanceFont::Inter))));
        assert!(!device.follow(Some(&account(AppearanceFont::Inter))));
        assert_eq!(
            device
                .effective(Some(&account(AppearanceFont::Manrope)))
                .font,
            Some(AppearanceFont::Manrope)
        );
        device.sync = false;
        assert!(!device.follow(Some(&account(AppearanceFont::Manrope))));
        assert_eq!(
            device
                .effective(Some(&account(AppearanceFont::Manrope)))
                .font,
            Some(AppearanceFont::Inter)
        );
    }

    #[test]
    fn only_an_upload_on_the_server_can_be_a_background() {
        // The web's `instanceFilePath`.
        let file = format!("/files/{}/{}/castle.webp", "a".repeat(32), "b".repeat(48));
        for ok in [file.as_str(), "/api/v1/uploads/abc-123"] {
            assert!(is_upload_path(ok), "{ok}");
        }
        for other in [
            "https://tracker.example/files/a/b/c.png",
            "//tracker.example/files/a/b/c.png",
            "/files/a/b/c.png?x=1",
            "/files/a/b/c.png#x",
            "/files/a/b",
            "/files/a/b/c/d.png",
            "/api/v1/uploads/",
            "/api/v1/uploads/a/b",
            "/avatars/fox.webp",
            "",
        ] {
            assert!(!is_upload_path(other), "{other}");
        }
    }

    #[test]
    fn an_upload_address_becomes_its_path_and_a_picture_is_measured_like_the_web() {
        let server = url::Url::parse("https://chat.example").unwrap();
        let path = format!("/files/{}/{}/castle.webp", "a".repeat(32), "b".repeat(48));
        assert_eq!(instance_file_path(&path, &server), Some(path.clone()));
        assert_eq!(
            instance_file_path(&format!("https://chat.example{path}"), &server),
            Some(path.clone())
        );
        assert_eq!(instance_file_path(&format!("{path}?x=1"), &server), None);
        assert_eq!(instance_file_path("/avatars/fox.webp", &server), None);

        // Ten black and ten white pixels, and one clear one that is skipped.
        let mut pixels = Vec::new();
        for i in 0..21 {
            let v = if i < 10 { 0 } else { 255 };
            pixels.extend([v, v, v, if i == 20 { 0 } else { 255 }]);
        }
        assert_eq!(
            measure_tones(&pixels),
            Some(["#000000".to_string(), "#ffffff".to_string()])
        );
        assert_eq!(measure_tones(&[1, 2, 3, 0]), None);
        let background = default_background();
        assert_eq!(background.dim, Some(55));
        assert_eq!(background.panel_opacity, Some(85));
    }

    #[test]
    fn a_background_is_raised_to_the_floors_that_keep_text_readable() {
        let mut theme = new_theme(&Look::default(), "Pic", "test", "now");
        let tokens = scheme_tokens(Look::default().scheme(Mode::Dark), None);
        assert_eq!(theme_backdrop(&theme, &tokens), None, "no background");
        theme.background = Some(ThemeBackground {
            url: Some("/api/v1/uploads/abc".into()),
            dim: Some(5),
            panel_opacity: Some(40),
            blur: Some(99),
            ..Default::default()
        });
        // Without measured tones the floors are the web's safe guess.
        let b = theme_backdrop(&theme, &tokens).unwrap();
        assert_eq!((b.dim, b.panel, b.blur), (0.60, 0.85, 20.));
        assert_eq!(b.placement, BackdropPlacement::Conversation);
        assert_eq!(b.fit, BackdropFit::Cover);
        // A very light picture needs more cover than a dark one under light text.
        let tones = |dark: &str, light: &str| Some(vec![dark.to_string(), light.to_string()]);
        theme.background.as_mut().unwrap().tones = tones("#000000", "#000000");
        let dark = theme_backdrop(&theme, &tokens).unwrap();
        theme.background.as_mut().unwrap().tones = tones("#ffffff", "#ffffff");
        let light = theme_backdrop(&theme, &tokens).unwrap();
        assert!(dark.dim <= light.dim && dark.panel <= light.panel);
        assert!(light.dim > dark.dim);
        // The theme's own unusable address draws nothing.
        theme.background.as_mut().unwrap().url = Some("https://x.example/a.png".into());
        assert_eq!(theme_backdrop(&theme, &tokens), None);
    }

    #[test]
    fn compiling_uses_the_mode_scheme_accent_and_theme() {
        let mut look = Look {
            mode: AppearanceMode::Light,
            scheme_light: "nord-light".into(),
            ..Default::default()
        };
        let light = compile(&look, look.resolve_mode(Mode::Dark), &[], false);
        assert_eq!(light.mode, Mode::Light);
        assert_eq!(light.scheme, "nord-light");
        assert_eq!(light.font, Some("Geist"));
        look.mode = AppearanceMode::System;
        assert_eq!(look.resolve_mode(Mode::Dark), Mode::Dark);

        let mut theme = new_theme(&look, "  Mine  ", "test", "now");
        assert_eq!(theme.name.as_deref(), Some("Mine"));
        set_seed(&mut theme, Mode::Dark, Seed::Surface, 0x102030);
        look.theme = theme.id.clone();
        let drawn = compile(&look, Mode::Dark, std::slice::from_ref(&theme), false);
        assert_eq!(drawn.tokens.main, 0x102030);
        assert_eq!(drawn.theme, theme.id);
        // The other mode has no seeds of its own and draws its base.
        let base = compile(&look, Mode::Light, std::slice::from_ref(&theme), false);
        assert_eq!(base.scheme, "nord-light");
        assert_eq!(
            base.tokens,
            scheme_tokens(find_scheme("nord-light").unwrap(), None)
        );
        // A theme from a newer client is not drawn.
        theme.format = Some(THEME_FORMAT + 1);
        let newer = compile(&look, Mode::Dark, std::slice::from_ref(&theme), false);
        assert_eq!(newer.theme, None);
        assert_eq!(newer.scheme, DEFAULT_DARK);
    }

    #[test]
    fn custom_colors_follow_their_own_mode_and_can_return_to_base() {
        let look = Look::default();
        let mut theme = new_theme(&look, "Test", "Tests", "revision-1");
        let dark = scheme_for("github-dark", Mode::Dark);
        let light = scheme_for("hibiki-light", Mode::Light);
        set_theme_base(&mut theme, dark);
        set_theme_base(&mut theme, light);
        assert_eq!(drawn_seeds(&theme, Mode::Dark), dark.seeds);
        assert_eq!(drawn_seeds(&theme, Mode::Light), light.seeds);
        set_seed(&mut theme, Mode::Dark, Seed::Blue, 0x468def);
        set_seed(&mut theme, Mode::Light, Seed::Canvas, 0xd2e4fc);
        set_theme_seeds(&mut theme, Mode::Light, None);
        assert_eq!(drawn_seeds(&theme, Mode::Light), light.seeds);
        assert_eq!(drawn_seeds(&theme, Mode::Dark).palette.blue, 0x468def);
        assert_eq!(theme_base(&theme, Mode::Dark).id, dark.id);
    }

    #[test]
    fn a_theme_css_sets_the_variables_it_names_only_when_allowed() {
        let mut look = Look::default();
        let mut theme = new_theme(&look, "Css", "test", "now");
        theme.css = Some(
            ":root { --accent: #ff0000; --radius: 4px; --interface-scale: 1.25 } .x { color: red }"
                .into(),
        );
        look.theme = theme.id.clone();
        let themes = std::slice::from_ref(&theme);

        let applied = compile_with_css(&look, Mode::Dark, themes, false, true);
        assert_eq!(applied.tokens.accent, 0xff0000);
        assert_eq!(applied.radius, 4.0);
        assert_eq!(applied.scale, 1.25);
        let css = applied.css.as_ref().expect("the CSS is applied");
        assert_eq!(css.skipped_rules, 1);
        assert!(css.applied().contains(&"accent"));

        // Switched off, or not allowed by the server: the look as without CSS.
        let plain = compile(&look, Mode::Dark, themes, false);
        assert_eq!(
            compile_with_css(&look, Mode::Dark, themes, false, false),
            plain
        );
        assert_eq!(plain.css, None);
        assert_ne!(plain.tokens.accent, 0xff0000);
    }

    #[test]
    fn a_theme_exports_and_imports_without_its_image() {
        let look = Look::default();
        let mut theme = new_theme(&look, "Deep Sea!", "v1", "then");
        set_seed(&mut theme, Mode::Dark, Seed::Accent, 0x00ffaa);
        theme.background = Some(ThemeBackground {
            url: Some("/files/a/b/c.png".into()),
            tones: Some(vec!["#000000".into(), "#ffffff".into()]),
            dim: Some(40),
            ..Default::default()
        });
        theme.css = Some(".x { color: red }".into());
        assert_eq!(theme_file_name(&theme), "deep-sea.ircord-theme.json");
        let file = export_theme(&theme);
        assert!(!file.contains("/files/"));
        let imported = import_theme(&file, "v2", "now").unwrap();
        assert_ne!(imported.id, theme.id);
        assert!(is_theme_id(imported.id.as_deref().unwrap()));
        assert_eq!(imported.name.as_deref(), Some("Deep Sea!"));
        assert_eq!(imported.made_with.as_deref(), Some("v1"));
        assert_eq!(drawn_seeds(&imported, Mode::Dark).accent, 0x00ffaa);
        let background = imported.background.as_ref().unwrap();
        assert_eq!((background.url.as_ref(), background.dim), (None, Some(40)));
        assert!(has_css(&imported));

        assert_eq!(import_theme("nope", "v", "n"), Err(ImportError::NotJson));
        assert_eq!(import_theme("{}", "v", "n"), Err(ImportError::NotTheme));
        assert_eq!(
            import_theme(r#"{"ircordTheme": 9}"#, "v", "n"),
            Err(ImportError::Newer)
        );
        let long = format!(
            r#"{{"ircordTheme": 1, "css": "{}"}}"#,
            "a".repeat(MAX_THEME_CSS + 1)
        );
        assert_eq!(import_theme(&long, "v", "n"), Err(ImportError::CssTooLong));
        let huge = " ".repeat(MAX_IMPORT + 1);
        assert_eq!(import_theme(&huge, "v", "n"), Err(ImportError::TooLarge));
    }

    #[test]
    fn colour_helpers_match_the_web_client() {
        assert_eq!(mix(0x000000, 0xffffff, 0.5), 0x808080);
        assert_eq!(parse_hex("#ABCdef"), Some(0xabcdef));
        assert_eq!(parse_hex("abcdef"), None);
        assert_eq!(parse_hex("#abcde"), None);
        assert_eq!(to_hex(0x0a0b0c), "#0a0b0c");
        assert!((contrast(0x000000, 0xffffff) - 21.).abs() < 1e-9);
        assert_eq!(font_family(AppearanceFont::System), None);
        assert_eq!(
            message_font_family(AppearanceMessageFont::Serif),
            Some("Source Serif 4")
        );
    }
}

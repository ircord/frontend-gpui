//! IRCord's colours, on top of gpui-kit's defaults. A dark palette close to
//! the web client's default theme.

use gpui_kit::{Hsla, Rgba, rgb};

pub fn c(hex: u32) -> Hsla {
    Hsla::from(rgb(hex))
}

pub fn rgba(hex: u32, alpha: f32) -> Hsla {
    let mut color: Rgba = rgb(hex);
    color.a = alpha;
    Hsla::from(color)
}

pub const RAIL: u32 = 0x1e1f22;
pub const SIDEBAR: u32 = 0x2b2d31;
pub const MAIN: u32 = 0x313338;
pub const INPUT: u32 = 0x383a40;
pub const HOVER: u32 = 0x35373c;
pub const SELECTED: u32 = 0x404249;
pub const TEXT: u32 = 0xdbdee1;
pub const MUTED: u32 = 0x949ba4;
pub const FAINT: u32 = 0x6d6f78;
pub const ACCENT: u32 = 0x5865f2;
pub const LINK: u32 = 0x00a8fc;
pub const DANGER: u32 = 0xf23f43;
pub const WARNING: u32 = 0xf0b232;
pub const ONLINE: u32 = 0x23a55a;
pub const AWAY: u32 = 0xf0b232;
pub const OFFLINE: u32 = 0x80848e;
pub const MENTION_BG: u32 = 0xf23f43;

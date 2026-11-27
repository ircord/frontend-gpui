//! Ircoid's picture: a synthwave night over a neon grid, drawn as plain
//! lists of shapes so the lists are tested without a window.
//!
//! GPUI's gradients have two stops and it cannot bend the picture, so the
//! sky is two stacked gradients, the sun a half disc cut by bands of sky,
//! and the CRT is only scanlines and darkened edges. The colours are a fixed
//! palette: the stage is a picture, like the animated avatar frames, and
//! does not follow the theme.
//!
//! All positions are pixels from the stage's top left; [`paint`] moves them
//! to where the stage is drawn.

use gpui_kit::*;
use ircord_client::ircoid::{FIELD_H, FIELD_W, WALL};

use super::BAR_H;

/// The palette, as in the mockups.
pub mod palette {
    pub const VOID: u32 = 0x0a0118;
    pub const NIGHT: u32 = 0x1d0b45;
    pub const DUSK: u32 = 0x4a1263;
    pub const FLOOR: u32 = 0x2a0a4a;
    pub const BAR: u32 = 0x12063a;
    pub const CYAN: u32 = 0x2de2e6;
    pub const PINK: u32 = 0xff2bd6;
    pub const VIOLET: u32 = 0xb967ff;
    pub const MINT: u32 = 0x05ffa1;
    pub const SUN: u32 = 0xfff275;
    pub const ORANGE: u32 = 0xff9933;
    pub const RED: u32 = 0xff3864;
    pub const WHITE_HOT: u32 = 0xf6f0ff;
    pub const SLATE: u32 = 0x6b6f8a;
}

/// A palette colour at `alpha`.
pub fn color(hex: u32, alpha: f32) -> Hsla {
    let mut c = rgb(hex);
    c.a = alpha;
    c.into()
}

/// A box, in pixels from the stage's top left.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    fn bounds(&self, origin: Point<Pixels>) -> Bounds<Pixels> {
        Bounds::new(
            point(origin.x + px(self.x), origin.y + px(self.y)),
            size(px(self.w), px(self.h)),
        )
    }
}

/// One thing the stage paints.
#[derive(Debug, Clone, PartialEq)]
pub enum Shape {
    /// A filled rounded box.
    Fill { at: Rect, radius: f32, color: Hsla },
    /// The outline of a rounded box.
    Outline {
        at: Rect,
        radius: f32,
        width: f32,
        color: Hsla,
    },
    /// A soft light: the blurred shadow of a box.
    Glow {
        at: Rect,
        radius: f32,
        blur: f32,
        color: Hsla,
    },
    /// A box whose colour runs from `from` to `to` toward `angle` (as CSS
    /// counts it: 180 is downward, 90 rightward).
    Gradient {
        at: Rect,
        radius: f32,
        angle: f32,
        from: Hsla,
        to: Hsla,
    },
    /// A four-cornered piece: the grid's lines to the vanishing point.
    Piece {
        corners: [(f32, f32); 4],
        color: Hsla,
    },
}

impl Shape {
    /// The box it covers, for tests.
    pub fn extent(&self) -> Rect {
        match self {
            Shape::Fill { at, .. }
            | Shape::Outline { at, .. }
            | Shape::Glow { at, .. }
            | Shape::Gradient { at, .. } => *at,
            Shape::Piece { corners, .. } => {
                let xs = corners.iter().map(|c| c.0);
                let ys = corners.iter().map(|c| c.1);
                let (x0, x1) = (
                    xs.clone().fold(f32::MAX, f32::min),
                    xs.fold(f32::MIN, f32::max),
                );
                let (y0, y1) = (
                    ys.clone().fold(f32::MAX, f32::min),
                    ys.fold(f32::MIN, f32::max),
                );
                Rect::new(x0, y0, x1 - x0, y1 - y0)
            }
        }
    }
}

/// The far end of the sun's gradient, below the horizon: halfway from the
/// sun's yellow to it is pink.
const SUN_FOOT: u32 = 0xff00ff;

/// The horizon, in field units from the top.
pub const HORIZON: f32 = 182.;
/// The sun's radius, in field units.
pub const SUN_R: f32 = 62.;
/// The grid's lines across the floor.
const CROSS_LINES: u32 = 11;
/// The grid's lines to the vanishing point, either side of the middle.
const RAYS: i32 = 12;
/// The spacing of the rays where they leave the field, in field units.
const RAY_SPACING: f32 = 44.;
/// Bands cutting the sun, from the horizon up.
const SUN_BANDS: usize = 6;
/// Stars in the sky.
const STARS: u32 = 46;
/// A scanline every this many pixels.
pub const SCANLINE_EVERY: f32 = 3.;

/// Pixels per field unit on a stage `w` wide.
pub fn scale(w: f32) -> f32 {
    w / FIELD_W
}

/// The stage's height for its width: the field and the status bar.
pub fn height(w: f32) -> f32 {
    w * (FIELD_H + BAR_H) / FIELD_W
}

/// A number in [0, 1) from `n`, the same every time.
fn hash(n: u32) -> f32 {
    let mut x = n.wrapping_mul(0x9e37_79b9) ^ 0x85eb_ca6b;
    x ^= x >> 15;
    x = x.wrapping_mul(0x2c1b_3c6d);
    x ^= x >> 12;
    (x & 0xffff) as f32 / 65536.
}

/// Where the grid's cross lines are, as shares of the floor from the
/// horizon (0) to the field's bottom (1). `scroll` (0 to 1) moves them
/// toward the viewer; 1 is the same as 0.
pub fn cross_lines(scroll: f32) -> Vec<f32> {
    let scroll = scroll.clamp(0., 1.);
    (0..=CROSS_LINES)
        .map(|n| (n as f32 + scroll) / CROSS_LINES as f32)
        .filter(|d| *d > 0.0001 && *d <= 1.0001)
        // Further apart toward the viewer.
        .map(|d| d.min(1.) * d.min(1.))
        .collect()
}

/// The bands cutting the sun: each one's top and height, in field units.
pub fn sun_bands() -> Vec<(f32, f32)> {
    (0..SUN_BANDS)
        .map(|n| {
            let n = n as f32;
            // Thicker and closer together toward the horizon.
            let thick = 3.4 - n * 0.45;
            let bottom = HORIZON - 3. - n * 7.5;
            (bottom - thick, thick)
        })
        .collect()
}

/// How wide the sun is at height `y` (field units).
pub fn sun_width_at(y: f32) -> f32 {
    let dy = HORIZON - y;
    if !(0. ..=SUN_R).contains(&dy) {
        return 0.;
    }
    2. * (SUN_R * SUN_R - dy * dy).sqrt()
}

/// The picture under the game on a stage `w` × `h` pixels: sky, stars,
/// sun, floor, grid, horizon and the neon walls. `scroll` moves the grid's
/// cross lines; keep it 0 with reduced motion.
pub fn backdrop(w: f32, h: f32, scroll: f32) -> Vec<Shape> {
    use palette::*;
    let k = scale(w);
    let field_h = (FIELD_H * k).min(h);
    let horizon = HORIZON * k;
    let mut out = Vec::new();

    // The sky in two gradients, void to night to dusk.
    let half = horizon / 2.;
    out.push(Shape::Gradient {
        at: Rect::new(0., 0., w, half),
        radius: 0.,
        angle: 180.,
        from: color(VOID, 1.),
        to: color(NIGHT, 1.),
    });
    out.push(Shape::Gradient {
        at: Rect::new(0., half, w, horizon - half),
        radius: 0.,
        angle: 180.,
        from: color(NIGHT, 1.),
        to: color(DUSK, 1.),
    });

    // Stars, thinning toward the horizon.
    for n in 0..STARS {
        let x = hash(n * 2) * (w - 4.) + 1.;
        let y = hash(n * 2 + 1).powf(1.6) * (horizon - 12.) + 2.;
        let size = if hash(n + 97) > 0.85 { 2. } else { 1. };
        let alpha = 0.35 + 0.55 * hash(n + 31);
        out.push(Shape::Fill {
            at: Rect::new(x, y, size, size),
            radius: 0.,
            color: color(WHITE_HOT, alpha),
        });
    }

    // The sun: a glow, the half disc, and bands of sky across its foot.
    let sun_r = SUN_R * k;
    let sun = Rect::new(w / 2. - sun_r, horizon - sun_r, sun_r * 2., sun_r);
    out.push(Shape::Glow {
        at: sun,
        radius: sun_r,
        blur: 28. * k,
        color: color(PINK, 0.45),
    });
    // A whole disc whose gradient reaches pink at the horizon, its middle;
    // the floor covers the lower half. (A box keeps its corners to half its
    // shorter side, so a half disc cannot be drawn on its own.)
    out.push(Shape::Gradient {
        at: Rect::new(sun.x, sun.y, sun.w, sun_r * 2.),
        radius: sun_r,
        angle: 180.,
        from: color(SUN, 1.),
        to: color(SUN_FOOT, 1.),
    });
    for (top, thick) in sun_bands() {
        let width = sun_width_at(top + thick).max(sun_width_at(top)) * k + 2.;
        let width = width.min(sun_r * 2.);
        out.push(Shape::Fill {
            at: Rect::new(w / 2. - width / 2., top * k, width, thick * k),
            radius: 0.,
            color: color(DUSK, 1.),
        });
    }

    // The floor, from the horizon to the field's bottom.
    out.push(Shape::Gradient {
        at: Rect::new(0., horizon, w, field_h - horizon),
        radius: 0.,
        angle: 180.,
        from: color(FLOOR, 1.),
        to: color(VOID, 1.),
    });

    // The grid: rays to the vanishing point...
    let vx = w / 2.;
    let floor = field_h - horizon;
    for i in -RAYS..=RAYS {
        let foot = vx + i as f32 * RAY_SPACING * k * 2.2;
        // Where the ray leaves the stage: the bottom, or a side.
        let (ex, ey) = if (0. ..=w).contains(&foot) {
            (foot, field_h)
        } else {
            let edge = if foot < 0. { 0. } else { w };
            let t = (edge - vx) / (foot - vx);
            (edge, horizon + floor * t)
        };
        let near = 0.5 + 0.6 * ((ey - horizon) / floor);
        let alpha = 0.35 + 0.35 * ((ey - horizon) / floor);
        let (l, r) = ((ex - near).max(0.), (ex + near).min(w));
        out.push(Shape::Piece {
            corners: [(vx - 0.2, horizon), (vx + 0.2, horizon), (r, ey), (l, ey)],
            color: color(PINK, alpha),
        });
    }
    // ...and lines across, further apart toward the viewer.
    for d in cross_lines(scroll) {
        let y = horizon + floor * d;
        let thick = (0.6 + 1.2 * d).min(field_h - y).max(0.);
        if thick <= 0. {
            continue;
        }
        out.push(Shape::Fill {
            at: Rect::new(0., y, w, thick),
            radius: 0.,
            color: color(PINK, 0.25 + 0.5 * d),
        });
    }

    // The horizon and its glow.
    out.push(Shape::Glow {
        at: Rect::new(0., horizon - 1., w, 2.),
        radius: 0.,
        blur: 10. * k,
        color: color(PINK, 0.8),
    });
    out.push(Shape::Fill {
        at: Rect::new(0., horizon - 1., w, 2.),
        radius: 0.,
        color: color(WHITE_HOT, 0.9),
    });

    // The neon walls: left, right and top.
    let wall = (WALL * k).max(2.);
    for at in [
        Rect::new(0., 0., wall, field_h),
        Rect::new(w - wall, 0., wall, field_h),
        Rect::new(0., 0., w, wall),
    ] {
        out.push(Shape::Glow {
            at,
            radius: 0.,
            blur: 8. * k,
            color: color(CYAN, 0.7),
        });
        out.push(Shape::Fill {
            at,
            radius: 0.,
            color: color(CYAN, 0.95),
        });
    }

    // The status bar's ground.
    if h > field_h {
        out.push(Shape::Fill {
            at: Rect::new(0., field_h, w, h - field_h),
            radius: 0.,
            color: color(BAR, 1.),
        });
        out.push(Shape::Fill {
            at: Rect::new(0., field_h, w, 1.),
            radius: 0.,
            color: color(CYAN, 0.6),
        });
    }
    out
}

/// The layer over everything: scanlines and darkened edges.
pub fn overlay(w: f32, h: f32) -> Vec<Shape> {
    let mut out = Vec::new();
    let mut y = 1.;
    while y + 1. <= h {
        out.push(Shape::Fill {
            at: Rect::new(0., y, w, 1.),
            radius: 0.,
            color: color(palette::VOID, 0.22),
        });
        y += SCANLINE_EVERY;
    }
    let dark = color(palette::VOID, 0.55);
    let clear = color(palette::VOID, 0.);
    let (side, top) = (w * 0.07, h * 0.08);
    for (at, angle) in [
        (Rect::new(0., 0., side, h), 90.),
        (Rect::new(w - side, 0., side, h), 270.),
        (Rect::new(0., 0., w, top), 180.),
        (Rect::new(0., h - top, w, top), 0.),
    ] {
        out.push(Shape::Gradient {
            at,
            radius: 0.,
            angle,
            from: dark,
            to: clear,
        });
    }
    out
}

/// Paints `shapes` on a stage whose top left is `origin`.
pub fn paint(shapes: &[Shape], origin: Point<Pixels>, window: &mut Window) {
    for shape in shapes {
        match shape {
            Shape::Fill { at, radius, color } => {
                window.paint_quad(fill(at.bounds(origin), *color).corner_radii(px(*radius)));
            }
            Shape::Outline {
                at,
                radius,
                width,
                color,
            } => window.paint_quad(quad(
                at.bounds(origin),
                px(*radius),
                transparent_black(),
                px(*width),
                *color,
                BorderStyle::Solid,
            )),
            Shape::Glow {
                at,
                radius,
                blur,
                color,
            } => window.paint_drop_shadows(
                at.bounds(origin),
                Corners::all(px(*radius)),
                &[BoxShadow {
                    color: *color,
                    offset: point(px(0.), px(0.)),
                    blur_radius: px(*blur),
                    spread_radius: px(0.),
                    inset: false,
                }],
            ),
            Shape::Gradient {
                at,
                radius,
                angle,
                from,
                to,
            } => window.paint_quad(
                fill(
                    at.bounds(origin),
                    linear_gradient(
                        *angle,
                        linear_color_stop(*from, 0.),
                        linear_color_stop(*to, 1.),
                    ),
                )
                .corner_radii(px(*radius)),
            ),
            Shape::Piece { corners, color } => {
                let at = |(x, y): (f32, f32)| point(origin.x + px(x), origin.y + px(y));
                let mut path = PathBuilder::fill();
                path.move_to(at(corners[0]));
                for corner in &corners[1..] {
                    path.line_to(at(*corner));
                }
                path.close();
                if let Ok(path) = path.build() {
                    window.paint_path(path, *color);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        HORIZON, SCANLINE_EVERY, SUN_R, Shape, backdrop, cross_lines, height, overlay, scale,
        sun_bands,
    };

    const SIZES: [f32; 3] = [320., 800., 1200.];

    fn inside(shape: &Shape, w: f32, h: f32) -> bool {
        // A glow's blur may spill; its box may not.
        let r = shape.extent();
        r.x >= -0.01 && r.y >= -0.01 && r.x + r.w <= w + 0.01 && r.y + r.h <= h + 0.01
    }

    #[test]
    fn nothing_lies_outside_the_stage() {
        for w in SIZES {
            let h = height(w);
            for scroll in [0., 0.3, 0.99] {
                for shape in backdrop(w, h, scroll).iter().chain(&overlay(w, h)) {
                    assert!(inside(shape, w, h), "{w}: {shape:?}");
                }
            }
        }
    }

    #[test]
    fn the_sun_bands_are_no_wider_than_the_sun() {
        for (top, thick) in sun_bands() {
            assert!(top + thick < HORIZON, "above the horizon");
            assert!(top > HORIZON - SUN_R, "inside the sun");
        }
        let w = 800.;
        let k = scale(w);
        let shapes = backdrop(w, height(w), 0.);
        for (top, _) in sun_bands() {
            let band = shapes
                .iter()
                .map(Shape::extent)
                .find(|r| (r.y - top * k).abs() < 0.01)
                .expect("each band is drawn");
            assert!(band.w <= SUN_R * 2. * k + 0.01);
        }
    }

    #[test]
    fn cross_lines_get_further_apart_toward_the_viewer() {
        let lines = cross_lines(0.);
        assert!(lines.len() >= 8);
        let gaps: Vec<f32> = lines.windows(2).map(|p| p[1] - p[0]).collect();
        assert!(gaps.windows(2).all(|g| g[1] > g[0]), "{gaps:?}");
    }

    #[test]
    fn the_grid_scroll_wraps() {
        let close = |a: &[f32], b: &[f32]| {
            a.len() == b.len() && a.iter().zip(b).all(|(x, y)| (x - y).abs() < 0.001)
        };
        assert!(close(&cross_lines(0.), &cross_lines(1.)));
        assert!(!close(&cross_lines(0.), &cross_lines(0.5)));
    }

    #[test]
    fn the_same_input_gives_the_same_picture() {
        assert_eq!(backdrop(800., 632., 0.25), backdrop(800., 632., 0.25));
        assert_eq!(overlay(800., 632.), overlay(800., 632.));
    }

    #[test]
    fn scanlines_every_three_pixels() {
        let lines: Vec<f32> = overlay(400., 316.)
            .iter()
            .filter_map(|s| match s {
                Shape::Fill { at, .. } => Some(at.y),
                _ => None,
            })
            .collect();
        assert!(lines.len() > 100);
        assert!(
            lines
                .windows(2)
                .all(|p| (p[1] - p[0] - SCANLINE_EVERY).abs() < 0.001)
        );
    }
}

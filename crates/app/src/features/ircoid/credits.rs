//! The credits behind the high score table's `/credits` line, in the style of
//! an 80s/90s BBS intro: a drifting starfield, three copper bars weaving
//! behind a pixel logo, the contributors as an IRC names list in a framed
//! box, and a sine scroller cycling through the palette.
//!
//! Everything here is a pure function of the time since the screen opened
//! (`t`, seconds), in game units on the 400 × 316 stage, so it is tested
//! without a window; the view scales it as it scales the game. Text is
//! returned as plain records the view draws as text elements over the
//! picture. Each term takes its remainder in `f64` before it becomes `f32`.

use std::f64::consts::TAU;

use gpui_kit::*;
use ircord_client::ircoid::Rng;

use super::stage::{Rect, Shape, palette};

/// Stars in the field.
pub const STARS: usize = 56;
/// The stars' layers: speed (units a second), size and opacity.
const LAYERS: [(f32, f32, f32); 3] = [(6., 1., 0.35), (14., 1.5, 0.6), (28., 2., 0.9)];
/// The logo's pixel, origin and letter pitch.
pub const PIXEL: f32 = 6.;
pub const LOGO_X: f32 = 95.;
pub const LOGO_Y: f32 = 24.;
pub const LETTER_PITCH: f32 = 36.;
/// The names box.
pub const BOX: Rect = Rect {
    x: 70.,
    y: 100.,
    w: 260.,
    h: 108.,
};
/// The scroller's band, its baseline and wave.
pub const BAND_TOP: f32 = 222.;
pub const BAND_BOTTOM: f32 = 290.;
const WAVE_MID: f32 = 256.;
const WAVE_HEIGHT: f32 = 11.;
/// The scroller's text size, cell and speed (units a second).
pub const SCROLL_SIZE: f32 = 16.;
pub const CELL: f32 = 9.6;
pub const SCROLL_SPEED: f32 = 96.;
/// A character cell is this share of its font size wide, and a line this
/// share tall (JetBrains Mono).
pub const CELL_SHARE: f32 = 0.6;
pub const LINE_SHARE: f32 = 1.32;
/// Names typed a second.
pub const TYPING: f32 = 45.;
/// A page turns by itself after this many seconds.
pub const PAGE_SECS: f64 = 8.;

/// `IRCOID` in 5 × 7 pixel letters.
const LETTERS: [[&str; 7]; 6] = [
    [
        "#####", "..#..", "..#..", "..#..", "..#..", "..#..", "#####",
    ],
    [
        "####.", "#...#", "#...#", "####.", "#.#..", "#..#.", "#...#",
    ],
    [
        ".####", "#....", "#....", "#....", "#....", "#....", ".####",
    ],
    [
        ".###.", "#...#", "#...#", "#...#", "#...#", "#...#", ".###.",
    ],
    [
        "#####", "..#..", "..#..", "..#..", "..#..", "..#..", "#####",
    ],
    [
        "####.", "#...#", "#...#", "#...#", "#...#", "#...#", "####.",
    ],
];

/// A palette colour.
fn hex(c: u32) -> Rgba {
    rgb(c)
}

/// `a` mixed `k` of the way toward `b`.
pub fn mix(a: Rgba, b: Rgba, k: f32) -> Rgba {
    let by = |a: f32, b: f32| a + (b - a) * k;
    Rgba {
        r: by(a.r, b.r),
        g: by(a.g, b.g),
        b: by(a.b, b.b),
        a: by(a.a, b.a),
    }
}

fn with_alpha(mut c: Rgba, a: f32) -> Hsla {
    c.a = a;
    c.into()
}

fn white() -> Rgba {
    hex(0xffffff)
}

/// The colour ramp: Cyan, Mint, Sun, Orange, Pink, Violet and back to Cyan,
/// at `p` (its fractional part).
pub fn ramp(p: f32) -> Rgba {
    use palette::*;
    const STOPS: [u32; 6] = [CYAN, MINT, SUN, ORANGE, PINK, VIOLET];
    let p = p.rem_euclid(1.) * STOPS.len() as f32;
    let i = p.floor() as usize % STOPS.len();
    let next = (i + 1) % STOPS.len();
    mix(hex(STOPS[i]), hex(STOPS[next]), p - p.floor())
}

/// `(a * t) mod m`, worked out in `f64`.
fn wrap(a: f64, t: f64, m: f64) -> f32 {
    (a * t).rem_euclid(m) as f32
}

/// A star: where it starts and which layer it is in.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Star {
    pub x0: f32,
    pub y: f32,
    pub layer: usize,
    pub cyan: bool,
}

/// The stars, the same every time.
pub fn star_field() -> Vec<Star> {
    let mut rng = Rng::new(0x1C01D);
    (0..STARS)
        .map(|i| Star {
            x0: rng.below(4000) as f32 / 10.,
            y: rng.below(2960) as f32 / 10.,
            layer: i % 3,
            cyan: i % 12 == 0,
        })
        .collect()
}

/// The stars at `t`, drifting left and wrapping.
pub fn stars(field: &[Star], t: f64) -> Vec<Shape> {
    field
        .iter()
        .map(|star| {
            let (speed, size, alpha) = LAYERS[star.layer];
            let x = (f64::from(star.x0) - f64::from(speed) * t).rem_euclid(400.) as f32;
            let x = x.min(400. - size);
            let color = if star.cyan {
                palette::CYAN
            } else {
                palette::WHITE_HOT
            };
            Shape::Fill {
                at: Rect::new(x, star.y, size, size),
                radius: 0.,
                color: with_alpha(hex(color), alpha),
            }
        })
        .collect()
}

/// The copper bars' centres at `t`.
pub fn bar_centres(t: f64) -> [f32; 3] {
    let phase = wrap(TAU / 4., t, TAU);
    [0., 1., 2.].map(|k: f32| 45. + 26. * (phase + k * std::f32::consts::TAU / 3.).sin())
}

/// The three copper bars, each two bands: edge to middle, middle to edge.
pub fn bars(t: f64) -> Vec<Shape> {
    use palette::*;
    let mut out = Vec::new();
    for (centre, color) in bar_centres(t).into_iter().zip([PINK, CYAN, SUN]) {
        let edge = with_alpha(mix(hex(color), hex(VOID), 0.72), 0.8);
        let middle = with_alpha(mix(hex(color), white(), 0.4), 0.8);
        out.push(Shape::Gradient {
            at: Rect::new(0., centre - 6., 400., 6.),
            radius: 0.,
            angle: 180.,
            from: edge,
            to: middle,
        });
        out.push(Shape::Gradient {
            at: Rect::new(0., centre, 400., 6.),
            radius: 0.,
            angle: 180.,
            from: middle,
            to: edge,
        });
    }
    out
}

/// The logo's runs of lit pixels: left, top and width, in units.
pub fn logo_runs() -> Vec<(f32, f32, f32)> {
    let mut out = Vec::new();
    for (n, letter) in LETTERS.iter().enumerate() {
        let left = LOGO_X + n as f32 * LETTER_PITCH;
        for (r, row) in letter.iter().enumerate() {
            let mut start: Option<usize> = None;
            for (c, lit) in row.chars().chain(['.']).enumerate() {
                match (lit == '#', start) {
                    (true, None) => start = Some(c),
                    (false, Some(s)) => {
                        out.push((
                            left + s as f32 * PIXEL,
                            LOGO_Y + r as f32 * PIXEL,
                            (c - s) as f32 * PIXEL,
                        ));
                        start = None;
                    }
                    _ => {}
                }
            }
        }
    }
    out
}

/// The logo: a shadow, a cyan and a pink ghost, then the top copy, its rows
/// cycling through the ramp.
pub fn logo(t: f64) -> Vec<Shape> {
    use palette::*;
    let runs = logo_runs();
    let cycle = wrap(0.6, t, 1.);
    let mut out = Vec::new();
    let copies: [(f32, f32, Option<Hsla>); 3] = [
        (1.5, 1.5, Some(with_alpha(hex(VOID), 0.9))),
        (-1.2, 0., Some(with_alpha(hex(CYAN), 0.6))),
        (1.2, 0., Some(with_alpha(hex(PINK), 0.6))),
    ];
    for (dx, dy, color) in copies {
        for &(x, y, w) in &runs {
            out.push(Shape::Fill {
                at: Rect::new(x + dx, y + dy, w, PIXEL),
                radius: 0.,
                color: color.unwrap_or_default(),
            });
        }
    }
    for &(x, y, w) in &runs {
        let row = (y - LOGO_Y) / PIXEL;
        let color = mix(ramp(row / 14. + cycle), white(), 0.2);
        out.push(Shape::Fill {
            at: Rect::new(x, y, w, PIXEL),
            radius: 0.,
            color: color.into(),
        });
    }
    out
}

/// The names box with its double frame and the scroller's band.
pub fn frame() -> Vec<Shape> {
    use palette::*;
    let inner = Rect::new(BOX.x + 3., BOX.y + 3., BOX.w - 6., BOX.h - 6.);
    vec![
        Shape::Fill {
            at: BOX,
            radius: 0.,
            color: with_alpha(hex(BAR), 0.8),
        },
        Shape::Outline {
            at: BOX,
            radius: 0.,
            width: 1.,
            color: with_alpha(hex(VIOLET), 0.95),
        },
        Shape::Outline {
            at: inner,
            radius: 0.,
            width: 0.5,
            color: with_alpha(hex(VIOLET), 0.45),
        },
        Shape::Fill {
            at: Rect::new(0., BAND_TOP, 400., BAND_BOTTOM - BAND_TOP),
            radius: 0.,
            color: with_alpha(hex(BAR), 0.55),
        },
        Shape::Fill {
            at: Rect::new(0., BAND_TOP, 400., 1.),
            radius: 0.,
            color: with_alpha(hex(PINK), 1.),
        },
        Shape::Fill {
            at: Rect::new(0., BAND_BOTTOM - 1., 400., 1.),
            radius: 0.,
            color: with_alpha(hex(CYAN), 1.),
        },
    ]
}

/// The box's title: its left edge, its top, and its size. A quad of the
/// box's colour under it cuts the border.
pub const TITLE_X: f32 = 83.;
pub const TITLE_SIZE: f32 = 8.;

/// A row of the names list: the name as shown, its centre, its size, and how
/// many of its characters are typed so far.
#[derive(Debug, Clone, PartialEq)]
pub struct NameRow {
    pub name: String,
    pub cx: f32,
    pub cy: f32,
    pub size: f32,
    pub shown: usize,
}

/// `name` cut to `max` characters, ending in `…` when cut.
fn clip(name: &str, max: usize) -> String {
    if name.chars().count() <= max {
        name.to_string()
    } else {
        name.chars().take(max - 1).chain(['…']).collect()
    }
}

/// The rows for the names on a page, with `typed` characters shown so far
/// across the list (`None`: all).
pub fn names(page: &[&str], typed: Option<usize>) -> Vec<NameRow> {
    let middle = BOX.y + BOX.h / 2.;
    let one_column = page.len() <= ircord_client::ircoid::credits::ONE_COLUMN;
    let (size, max) = if one_column { (11., 34) } else { (9., 20) };
    let mut left = typed.unwrap_or(usize::MAX);
    let mut out = Vec::new();
    for (i, name) in page.iter().enumerate() {
        let (cx, row, rows) = if one_column {
            (200., i, page.len())
        } else {
            let column = i / 5;
            let rows = if column == 0 { 5 } else { page.len() - 5 };
            (if column == 0 { 137. } else { 263. }, i % 5, rows)
        };
        let cy = middle - (rows as f32 - 1.) * 8. + row as f32 * 16.;
        let name = clip(name, max);
        // The `@` counts as typed with the name.
        let len = name.chars().count() + 1;
        let shown = left.min(len);
        left = left.saturating_sub(len);
        out.push(NameRow {
            name,
            cx,
            cy,
            size,
            shown,
        });
    }
    out
}

/// Characters typed `secs` after a page showed.
pub fn typed(secs: f64) -> usize {
    (secs.max(0.) * f64::from(TYPING)).floor() as usize
}

/// A character of the scroller: its centre, and its colour.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Glyph {
    pub ch: char,
    pub cx: f32,
    pub cy: f32,
    pub color: Rgba,
}

/// How long one pass of `text` takes to cross the stage, in units.
pub fn loop_length(text: &str) -> f64 {
    text.chars().count() as f64 * f64::from(CELL) + 400.
}

/// The scroller's visible characters at `t`.
pub fn scroller(text: &str, t: f64) -> Vec<Glyph> {
    let travelled = wrap(f64::from(SCROLL_SPEED), t, loop_length(text));
    let wave = wrap(TAU / 2.2, t, TAU);
    let colours = wrap(0.25, t, 1.);
    let mut out = Vec::new();
    for (i, ch) in text.chars().enumerate() {
        let x = 400. + CELL * i as f32 - travelled;
        if x >= 400. {
            break;
        }
        if x <= -CELL || ch == ' ' {
            continue;
        }
        let cx = x + CELL / 2.;
        let cy = WAVE_MID + WAVE_HEIGHT * (std::f32::consts::TAU * cx / 170. - wave).sin();
        out.push(Glyph {
            ch,
            cx,
            cy,
            color: ramp(cx / 400. + colours),
        });
    }
    out
}

/// The scroller's text from its parts: each upper-cased, each followed by
/// the separator.
pub fn scroll_text(parts: &[String]) -> String {
    parts
        .iter()
        .map(|p| format!("{}   ···   ", p.to_uppercase()))
        .collect()
}

/// The parts as one paragraph, for reduced motion.
pub fn paragraph(parts: &[String]) -> String {
    parts.join(" · ")
}

/// `I R C O R D   P R E S E N T S`: upper-cased with a space between letters.
pub fn spaced(text: &str) -> String {
    let mut out = String::new();
    for (i, ch) in text.to_uppercase().chars().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        out.push(ch);
    }
    out
}

/// A shape in units, in pixels on a stage `k` pixels a unit.
pub fn scaled(shape: &Shape, k: f32) -> Shape {
    let r = |at: &Rect| Rect::new(at.x * k, at.y * k, at.w * k, at.h * k);
    match shape {
        Shape::Fill { at, radius, color } => Shape::Fill {
            at: r(at),
            radius: radius * k,
            color: *color,
        },
        Shape::Outline {
            at,
            radius,
            width,
            color,
        } => Shape::Outline {
            at: r(at),
            radius: radius * k,
            width: (width * k).max(0.5),
            color: *color,
        },
        Shape::Glow {
            at,
            radius,
            blur,
            color,
        } => Shape::Glow {
            at: r(at),
            radius: radius * k,
            blur: blur * k,
            color: *color,
        },
        Shape::Gradient {
            at,
            radius,
            angle,
            from,
            to,
        } => Shape::Gradient {
            at: r(at),
            radius: radius * k,
            angle: *angle,
            from: *from,
            to: *to,
        },
        Shape::Piece { corners, color } => Shape::Piece {
            corners: corners.map(|(x, y)| (x * k, y * k)),
            color: *color,
        },
    }
}

/// The whole picture under the text at `t`: sky, stars, bars, logo, box and
/// band, in units.
pub fn picture(field: &[Star], t: f64) -> Vec<Shape> {
    let mut out = vec![Shape::Gradient {
        at: Rect::new(0., 0., 400., 316.),
        radius: 0.,
        angle: 180.,
        from: with_alpha(hex(palette::VOID), 1.),
        to: with_alpha(hex(palette::NIGHT), 1.),
    }];
    out.extend(stars(field, t));
    out.extend(bars(t));
    out.extend(logo(t));
    out.extend(frame());
    out
}

#[cfg(test)]
mod tests {
    use super::{
        BOX, CELL, Shape, bar_centres, bars, frame, logo, logo_runs, loop_length, names, ramp,
        scroller, star_field, stars, typed,
    };
    use gpui_kit::rgb;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 0.01
    }

    #[test]
    fn the_stars_stay_on_the_stage_and_wrap() {
        let field = star_field();
        assert_eq!(field.len(), 56);
        for t in [0., 3.7, 1000.] {
            for star in stars(&field, t) {
                let r = star.extent();
                assert!(
                    r.x >= 0. && r.x + r.w <= 400. && r.y >= 0. && r.y + r.h <= 298.,
                    "{r:?}"
                );
            }
        }
        let far = field.iter().position(|s| s.layer == 0).unwrap();
        let a = stars(&field, 1.).swap_remove(far).extent();
        let b = stars(&field, 1. + 400. / 6.).swap_remove(far).extent();
        assert!(close(a.x, b.x) && close(a.y, b.y));
    }

    #[test]
    fn three_bars_of_two_bands_swing_within_their_range() {
        assert_eq!(bars(0.).len(), 6);
        let [a, b, c] = bar_centres(0.);
        // 45 ± 26 sin 120°, that is ± 22.52.
        let near = |x: f32, y: f32| (x - y).abs() < 0.05;
        assert!(
            near(a, 45.) && near(b, 67.5) && near(c, 22.5),
            "{a} {b} {c}"
        );
        for n in 0..400 {
            for centre in bar_centres(n as f64 * 0.137) {
                assert!((19. ..=71.).contains(&centre));
            }
        }
    }

    #[test]
    fn the_logo_is_57_runs_of_95_pixels_in_its_place() {
        let runs = logo_runs();
        assert_eq!(runs.len(), 57);
        let pixels: f32 = runs.iter().map(|r| r.2 / 6.).sum();
        assert_eq!(pixels, 95.);
        let shapes = logo(2.5);
        assert_eq!(shapes.len(), 57 * 4);
        for shape in shapes {
            let r = shape.extent();
            assert!(
                r.x >= 93. && r.x + r.w <= 307. && r.y >= 24. && r.y + r.h <= 68.,
                "{r:?}"
            );
        }
    }

    #[test]
    fn the_ramp_runs_from_cyan_through_orange_back_to_cyan() {
        let cyan = rgb(0x2de2e6);
        let orange = rgb(0xff9933);
        let same = |a: gpui_kit::Rgba, b: gpui_kit::Rgba| {
            close(a.r, b.r) && close(a.g, b.g) && close(a.b, b.b)
        };
        assert!(same(ramp(0.), cyan));
        assert!(same(ramp(1.), cyan));
        assert!(same(ramp(0.5), orange));
    }

    #[test]
    fn the_scroller_enters_from_the_right_and_waves() {
        let text = "WELCOME TO THE IRCOID CREDITS   ···   ";
        assert!(scroller(text, 0.).is_empty());
        let at_one = scroller(text, 1.);
        assert!(close(at_one[0].cx - CELL / 2., 304.));
        let mid = scroller(text, 5.);
        assert!(!mid.is_empty());
        assert!(mid.iter().all(|g| g.ch != ' '));
        for t in [0.5, 2., 5., 9.3] {
            for g in scroller(text, t) {
                assert!((245. ..=267.).contains(&g.cy), "{}", g.cy);
            }
        }
        // Neighbours are a cell apart.
        let w: Vec<_> = scroller("ABCDEFGHIJ", 2.);
        assert!(w.windows(2).all(|p| close(p[1].cx - p[0].cx, CELL)));
        let period = loop_length(text) / 96.;
        let a = scroller(text, 3.);
        let b = scroller(text, 3. + period);
        assert_eq!(a.len(), b.len());
        // One loop later the same characters are in the same places; the
        // wave keeps its own beat.
        assert!(
            a.iter()
                .zip(&b)
                .all(|(x, y)| x.ch == y.ch && (x.cx - y.cx).abs() < 0.05)
        );
    }

    #[test]
    fn names_sit_in_one_or_two_columns_and_long_ones_are_cut() {
        let two = names(&["Ada Lovelace", "Alan Turing"], None);
        assert!(close(two[0].cy, 146.) && close(two[1].cy, 162.));
        assert!(two.iter().all(|r| close(r.cx, 200.)));
        let six = names(&["a", "b", "c", "d", "e", "f"], None);
        assert!(close(six[0].cx, 137.) && close(six[5].cx, 263.));
        let long = "x".repeat(40);
        let cut = names(&[long.as_str()], None);
        assert!(cut[0].name.ends_with('…'));
        assert_eq!(cut[0].name.chars().count(), 34);
        for row in names(&["a", "b", "c", "d", "e", "f", "g", "h", "i", "j"], None) {
            assert!(row.cy > BOX.y && row.cy < BOX.y + BOX.h);
        }
    }

    #[test]
    fn names_are_typed_out() {
        let list = ["Ada Lovelace", "Alan Turing", "Grace Hopper"];
        let total: usize = list.iter().map(|n| n.chars().count() + 1).sum();
        let shown = |secs: f64| -> usize {
            names(&list, Some(typed(secs)))
                .iter()
                .map(|r| r.shown)
                .sum()
        };
        assert_eq!(shown(0.), 0);
        assert_eq!(shown(1.).min(45), total.min(45));
        assert_eq!(shown(100.), total);
        let whole = names(&list, None);
        assert!(whole.iter().all(|r| r.shown == r.name.chars().count() + 1));
    }

    #[test]
    fn the_frame_is_box_and_band() {
        assert!(
            frame()
                .iter()
                .all(|s| matches!(s, Shape::Fill { .. } | Shape::Outline { .. }))
        );
    }
}

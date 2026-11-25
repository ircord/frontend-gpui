//! The `/attract` comet. The row gets a thin rounded outline; a bright head
//! leaves the avatar, runs clockwise around the outline twice with a
//! tapering trail, and sheds sparks that keep a little of its speed, fly
//! off at their own angle and fall over the rows below. At [`LANDS`] it
//! lands on the avatar and throws a burst; a ring leaves the avatar at
//! take-off, after the first lap and at landing.
//!
//! [`shapes`] works the frame out from the time and the places the row and
//! avatar were painted at, with no window, and every spark's place is a
//! function of the time alone ([`attract::noise`]). [`layer`] is the canvas
//! over the timeline that paints the shapes after the rows, in the same
//! frame, so the comet follows its row while the list scrolls or new
//! messages push it, and nothing is laid out for it.

use std::f32::consts::TAU;

use gpui_kit::prelude::*;
use gpui_kit::*;

use super::attract::{self, Stage, Target, envelope, noise};
use crate::theme::Theme;

/// How long the comet takes for one lap of the row.
pub const LAP: f32 = 1.3;
/// When it lands back on the avatar and bursts.
pub const LANDS: f32 = 2. * LAP;
/// The sparks shed on the way.
const SPARKS: u32 = 120;
/// The sparks of the landing burst.
const BURST: u32 = 26;
/// The longest a spark lives.
const SPARK_LIFE: f32 = 1.15;
/// How long a replaced comet takes to fade, apart from its sparks.
const SETTLE: f32 = 0.15;
/// How long the landing ring takes to spread: until the effect ends.
const LANDING_RING: f32 = attract::ATTRACT_MS as f32 / 1000. - LANDS;
/// A spark's fall, in px per second squared.
const GRAVITY: f32 = 280.;

/// The comet's colours.
#[derive(Debug, Clone, Copy)]
pub struct Palette {
    pub accent: Hsla,
    /// The brightest part: near white on dark, a vivid accent on light.
    pub hot: Hsla,
    pub gold: Hsla,
    /// The very core of the head.
    pub core: Hsla,
}

impl Palette {
    pub fn new(theme: &Theme) -> Self {
        let a = theme.accent;
        if theme.dark {
            Palette {
                accent: a,
                hot: hsla(a.h, 0.95, 0.9, 1.),
                gold: rgb(0xffe08a).into(),
                core: white(),
            }
        } else {
            // On a light page a glow has to be a colour, not a brightness.
            Palette {
                accent: a,
                hot: hsla(a.h, 0.95, 0.6, 1.),
                gold: rgb(0xf5a300).into(),
                core: white(),
            }
        }
    }
}

/// One thing the layer paints.
#[derive(Debug, Clone, PartialEq)]
pub enum Shape {
    /// A filled rounded box: dots and the outline's tint.
    Fill {
        at: Bounds<Pixels>,
        radius: f32,
        color: Hsla,
    },
    /// The outline of a rounded box: the row's outline and the rings.
    Outline {
        at: Bounds<Pixels>,
        radius: f32,
        width: f32,
        color: Hsla,
    },
    /// A soft light: the blurred shadow of a box.
    Glow {
        at: Bounds<Pixels>,
        radius: f32,
        blur: f32,
        color: Hsla,
    },
    /// A band whose colour runs from `from` to `to` in the direction of
    /// travel (`angle`, as CSS counts it): the trail.
    Streak {
        at: Bounds<Pixels>,
        angle: f32,
        from: Hsla,
        to: Hsla,
    },
}

/// One comet to draw: the time into its attract and, for one that a newer
/// attract replaced, the time it was replaced at.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Moment {
    pub t: f32,
    pub replaced: Option<f32>,
}

impl Moment {
    /// Whether nothing of it is left to draw.
    pub fn done(&self) -> bool {
        let end = attract::ATTRACT_MS as f32 / 1000.;
        self.t >= end || self.replaced.is_some_and(|at| self.t >= at + SPARK_LIFE)
    }
}

pub(super) fn rect(x: f32, y: f32, w: f32, h: f32) -> Bounds<Pixels> {
    Bounds::new(point(px(x), px(y)), size(px(w), px(h)))
}

pub(super) fn f(p: Pixels) -> f32 {
    f32::from(p)
}

pub(super) fn ease_out(q: f32) -> f32 {
    1. - (1. - q.clamp(0., 1.)).powi(3)
}

/// The row's outline as the path the comet runs along, clockwise from the
/// top left corner.
#[derive(Debug, Clone, Copy)]
pub struct Track {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

/// How far the track sits inside the row at the top, in px.
const INSET: f32 = 1.;
/// How far the track reaches past the row's bottom edge, in px. Letters hang
/// below the baseline (g, p, y), so with the track inside the row by
/// [`INSET`] at the bottom too, the outline sat about 1 px from them where
/// it keeps 3 px from the capitals. Moving it 2 px down (1 px past the row)
/// levels the gap under the text with the gap above it.
const DESCENT: f32 = 1.;

impl Track {
    pub fn new(row: Bounds<Pixels>) -> Self {
        Track {
            x: f(row.left()) + 5.,
            y: f(row.top()) + INSET,
            w: (f(row.size.width) - 10.).max(1.),
            h: (f(row.size.height) - INSET + DESCENT).max(1.),
        }
    }

    fn length(&self) -> f32 {
        2. * (self.w + self.h)
    }

    fn bounds(&self) -> Bounds<Pixels> {
        rect(self.x, self.y, self.w, self.h)
    }

    /// The comet's speed, so that a lap always takes [`LAP`].
    fn speed(&self) -> f32 {
        self.length() / LAP
    }

    /// The place `d` px along the track and the direction of travel there.
    fn at(&self, d: f32) -> ((f32, f32), (f32, f32)) {
        let d = d.rem_euclid(self.length());
        let (w, h) = (self.w, self.h);
        if d < w {
            ((self.x + d, self.y), (1., 0.))
        } else if d < w + h {
            ((self.x + w, self.y + d - w), (0., 1.))
        } else if d < 2. * w + h {
            ((self.x + w - (d - w - h), self.y + h), (-1., 0.))
        } else {
            ((self.x, self.y + h - (d - 2. * w - h)), (0., -1.))
        }
    }

    /// The four sides as `(from, to, angle of travel)` in px along the track.
    fn sides(&self) -> [(f32, f32, f32); 4] {
        let (w, h) = (self.w, self.h);
        [
            (0., w, 90.),
            (w, w + h, 180.),
            (w + h, 2. * w + h, 270.),
            (2. * w + h, 2. * (w + h), 0.),
        ]
    }

    /// The part of the track from `from` to `to` (both on one side) as a
    /// band `thick` wide.
    fn band(&self, from: f32, to: f32, thick: f32) -> Bounds<Pixels> {
        let ((ax, ay), _) = self.at(from + 0.001);
        let ((bx, by), _) = self.at(to - 0.001);
        let (x0, x1) = (ax.min(bx), ax.max(bx));
        let (y0, y1) = (ay.min(by), ay.max(by));
        rect(
            x0 - thick / 2.,
            y0 - thick / 2.,
            x1 - x0 + thick,
            y1 - y0 + thick,
        )
    }
}

/// Where the comet's head is at `t`, or `None` once it has landed.
pub fn head(row: Bounds<Pixels>, t: f32) -> Option<Point<Pixels>> {
    let track = Track::new(row);
    (t < LANDS).then(|| {
        let ((x, y), _) = track.at(track.speed() * t.max(0.));
        point(px(x), px(y))
    })
}

/// The avatar's centre and radius. Without one in view (its row scrolled
/// away above, or a layout that draws no pictures) the comet lands where
/// it took off, the outline's top left corner, whatever the layout.
fn origin(track: &Track, avatar: Option<Bounds<Pixels>>) -> (f32, f32, f32) {
    match avatar {
        Some(a) => {
            let r = f(a.size.width) / 2.;
            (f(a.left()) + r, f(a.top()) + f(a.size.height) / 2., r)
        }
        None => (track.x, track.y, 0.),
    }
}

/// Collects shapes, leaving out what would not show.
#[derive(Default)]
pub(super) struct Shapes(pub(super) Vec<Shape>);

impl Shapes {
    pub(super) fn fill(&mut self, at: Bounds<Pixels>, radius: f32, color: Hsla) {
        if color.a > 0.01 {
            self.0.push(Shape::Fill { at, radius, color });
        }
    }

    pub(super) fn dot(&mut self, x: f32, y: f32, r: f32, color: Hsla) {
        if r > 0.2 {
            self.fill(rect(x - r, y - r, 2. * r, 2. * r), r, color);
        }
    }

    pub(super) fn outline(&mut self, at: Bounds<Pixels>, radius: f32, width: f32, color: Hsla) {
        if color.a > 0.01 {
            self.0.push(Shape::Outline {
                at,
                radius,
                width,
                color,
            });
        }
    }

    pub(super) fn ring(&mut self, x: f32, y: f32, r: f32, width: f32, color: Hsla) {
        self.outline(rect(x - r, y - r, 2. * r, 2. * r), r, width, color);
    }

    pub(super) fn blob(&mut self, x: f32, y: f32, r: f32, blur: f32, color: Hsla) {
        if color.a > 0.01 {
            self.0.push(Shape::Glow {
                at: rect(x - r, y - r, 2. * r, 2. * r),
                radius: r,
                blur,
                color,
            });
        }
    }

    pub(super) fn streak(&mut self, at: Bounds<Pixels>, angle: f32, from: Hsla, to: Hsla) {
        if from.a > 0.01 || to.a > 0.01 {
            self.0.push(Shape::Streak {
                at,
                angle,
                from,
                to,
            });
        }
    }
}

/// The comet at `moment` on the row painted at `row`, whose group's avatar
/// was painted at `avatar` (if it is in view). A replaced comet fades in
/// [`SETTLE`] and sheds nothing more; the sparks already in the air finish
/// falling.
pub fn shapes(
    row: Bounds<Pixels>,
    avatar: Option<Bounds<Pixels>>,
    moment: Moment,
    pal: &Palette,
) -> Vec<Shape> {
    let Moment { t, replaced } = moment;
    let mut out = Shapes::default();
    if moment.done() {
        return out.0;
    }
    let e = envelope(t);
    let settle = replaced.map_or(1., |at| (1. - (t - at) / SETTLE).clamp(0., 1.));
    // Sparks and bursts born after this are never made.
    let shed_until = replaced.unwrap_or(f32::INFINITY);
    let track = Track::new(row);
    let (ox, oy, r) = origin(&track, avatar);
    let steady = e * settle;

    // The ring that stays around the avatar while the comet plays.
    if avatar.is_some() {
        out.ring(ox, oy, r + 4., 2., pal.accent.opacity(0.9 * steady));
    }

    let length = track.length();
    let outline = track.bounds();
    out.fill(outline, 7., pal.accent.opacity(0.07 * steady));
    out.outline(outline, 7., 1., pal.accent.opacity(0.4 * steady));

    let speed = track.speed();
    let flying = t < LANDS;
    let at = speed * t.min(LANDS);
    // The trail: a band on each side it still covers, fading to its end,
    // and gone a moment after landing.
    let tail = (0.3 * length).min(560.);
    let fade = if flying {
        1.
    } else {
        (1. - (t - LANDS) / 0.25).max(0.)
    };
    for lap in [-1., 0., 1., 2.] {
        for (from, to, angle) in track.sides() {
            let (from, to) = (from + lap * length, to + lap * length);
            let (s0, s1) = (from.max(at - tail), to.min(at));
            if s1 - s0 < 0.5 {
                continue;
            }
            let alpha = |s: f32| ((s - (at - tail)) / tail).powf(1.6) * fade * steady;
            let (a0, a1) = (alpha(s0), alpha(s1));
            let (b0, b1) = (s0 - lap * length, s1 - lap * length);
            out.streak(
                track.band(b0, b1, 13.),
                angle,
                pal.accent.opacity(0.26 * a0),
                pal.accent.opacity(0.26 * a1),
            );
            out.streak(
                track.band(b0, b1, 3.5),
                angle,
                pal.hot.opacity(a0),
                pal.hot.opacity(a1),
            );
        }
    }
    if flying {
        let ((x, y), _) = track.at(at);
        out.blob(x, y, 9., 22., pal.accent.opacity(0.9 * steady));
        out.blob(x, y, 5., 8., pal.hot.opacity(steady));
        out.dot(x, y, 4.2, pal.core.opacity(steady));
    }

    // Sparks shed along the way: each keeps a little of the comet's speed
    // backwards, flies off at its own angle and falls.
    for n in 0..SPARKS {
        let born = LANDS * (n as f32 + noise(n, 7)) / SPARKS as f32;
        let age = t - born;
        let life = 0.55 + 0.6 * noise(n, 8);
        if born >= shed_until || age < 0. || age > life {
            continue;
        }
        let ((x, y), (dx, dy)) = track.at(speed * born);
        let angle = TAU * noise(n, 9);
        let fly = 25. + 95. * noise(n, 10);
        let (vx, vy) = (
            -70. * dx + fly * angle.cos(),
            -70. * dy + fly * angle.sin() - 30.,
        );
        let (sx, sy) = (x + vx * age, y + vy * age + 0.5 * GRAVITY * age * age);
        let left = (1. - age / life).powf(1.3) * e;
        let color = match (noise(n, 11) * 3.) as u32 {
            0 => pal.gold,
            1 => pal.accent,
            _ => pal.hot,
        };
        let big = (1.4 + 2.2 * noise(n, 12)) * left.sqrt();
        if n % 4 == 0 {
            out.blob(sx, sy, big, 6., color.opacity(0.7 * left));
        }
        out.dot(sx, sy, big, color.opacity(left));
    }

    // A ring leaves the avatar at take-off, after the first lap and at
    // landing. The landing ring lives until the effect ends, so it has
    // faded by then and nothing pops when the layer goes.
    for pass in [0., LAP, LANDS] {
        // The landing ring stays bright while everything else fades out.
        let (life, big, strength) = if pass == LANDS {
            (LANDING_RING, 22., e.max(0.4))
        } else {
            (0.55, 12., e)
        };
        let age = (t - pass) / life;
        if (0. ..1.).contains(&age) {
            out.ring(
                ox,
                oy,
                r + 4. + big * ease_out(age),
                2.,
                pal.hot.opacity((1. - age) * strength * settle),
            );
        }
    }
    // The landing burst.
    let age = t - LANDS;
    if age >= 0. && LANDS < shed_until {
        for n in 0..BURST {
            // Gone by the time the effect ends.
            let life = 0.28 + 0.12 * noise(n, 21);
            if age > life {
                continue;
            }
            let angle = TAU * (n as f32 + noise(n, 22)) / BURST as f32;
            let fly = 70. + 150. * noise(n, 23);
            let (sx, sy) = (
                ox + (r + 4.) * angle.cos() + fly * angle.cos() * age,
                oy + (r + 4.) * angle.sin() + fly * angle.sin() * age + 0.5 * 320. * age * age,
            );
            let left = 1. - age / life;
            let color = if n % 3 == 0 { pal.gold } else { pal.hot };
            out.dot(sx, sy, 1.6 + 1.8 * left, color.opacity(left));
        }
    }
    out.0
}

pub(super) fn paint(shape: &Shape, window: &mut Window) {
    match shape {
        Shape::Fill { at, radius, color } => {
            window.paint_quad(fill(*at, *color).corner_radii(px(*radius)));
        }
        Shape::Outline {
            at,
            radius,
            width,
            color,
        } => window.paint_quad(quad(
            *at,
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
            *at,
            Corners::all(px(*radius)),
            &[BoxShadow {
                color: *color,
                offset: point(px(0.), px(0.)),
                blur_radius: px(*blur),
                spread_radius: px(0.),
                inset: false,
            }],
        ),
        Shape::Streak {
            at,
            angle,
            from,
            to,
        } => window.paint_quad(fill(
            *at,
            linear_gradient(
                *angle,
                linear_color_stop(*from, 0.),
                linear_color_stop(*to, 1.),
            ),
        )),
    }
}

/// The canvas over the timeline that paints `comets` (oldest first) from
/// the places their rows and avatars reported to `stage` while they were
/// painted. A comet whose row is out of view draws nothing. The layer takes
/// no clicks; `bottom` leaves the typing line under the list alone, except
/// for the [`DESCENT`] px the layer reaches into it so the outline of the
/// last row is not cut off (the typing line has room above its text).
pub fn layer(
    stage: Stage,
    comets: Vec<(Target, Moment)>,
    pal: Palette,
    bottom: Pixels,
) -> AnyElement {
    div()
        .id("attract-comet")
        .test_support()
        .absolute()
        .top_0()
        .left_0()
        .right_0()
        .bottom(bottom - px(DESCENT))
        .child(
            canvas(
                |_, _, _| (),
                move |bounds, _, window, _| {
                    let places = stage.take();
                    window.with_content_mask(Some(ContentMask { bounds }), |window| {
                        for (target, moment) in &comets {
                            let Some(row) = places.rows.get(&target.row) else {
                                continue;
                            };
                            let avatar = places.avatars.get(&target.avatar).copied();
                            for shape in shapes(*row, avatar, *moment, &pal) {
                                paint(&shape, window);
                            }
                        }
                    });
                },
            )
            .size_full(),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::{
        BURST, LANDS, LAP, Moment, Palette, SPARK_LIFE, Shape, Track, f, head, rect, shapes,
    };
    use crate::theme::Theme;
    use gpui_kit::{Bounds, Pixels};

    fn row() -> Bounds<Pixels> {
        rect(0., 100., 800., 40.)
    }

    fn avatar() -> Bounds<Pixels> {
        rect(16., 80., 40., 40.)
    }

    fn pal() -> Palette {
        Palette::new(&Theme::dark())
    }

    fn at(t: f32) -> Moment {
        Moment { t, replaced: None }
    }

    fn dots(shapes: &[Shape]) -> Vec<Bounds<Pixels>> {
        shapes
            .iter()
            .filter_map(|s| match s {
                Shape::Fill { at, .. } if f(at.size.width) < 10. => Some(*at),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn the_head_laps_the_row_twice_and_lands() {
        let r = row();
        let start = head(r, 0.).unwrap();
        assert_eq!((f(start.x), f(start.y)), (5., 101.), "it starts top left");
        // A quarter of the way along the top edge goes right.
        let later = head(r, 0.2).unwrap();
        assert!(f(later.x) > 200. && f(later.y) == 101.);
        // Half a lap later it is on the bottom edge.
        let below = head(r, LAP / 2. + 0.01).unwrap();
        assert!((f(below.y) - 141.).abs() < 0.01, "{below:?}");
        // After a lap it is back where it started.
        let lap = head(r, LAP).unwrap();
        assert!((f(lap.x) - 5.).abs() < 0.5 && (f(lap.y) - 101.).abs() < 0.5);
        assert_eq!(head(r, LANDS), None, "it has landed");
    }

    #[test]
    fn a_lap_takes_the_same_time_on_a_tall_row() {
        let tall = rect(0., 100., 800., 400.);
        let half = head(tall, LAP / 2. + 0.01).unwrap();
        assert!((f(half.y) - 501.).abs() < 0.01);
    }

    #[test]
    fn the_outline_clears_the_descenders_as_far_as_the_capitals() {
        // The top is inset into the row; the bottom reaches past the row.
        let r = row();
        let track = Track::new(r);
        let top = f(track.bounds().top()) - f(r.top());
        let bottom = f(track.bounds().bottom()) - f(r.bottom());
        assert_eq!((top, bottom), (1., 1.), "gap above equals gap below");
    }

    #[test]
    fn sparks_fall_below_the_row() {
        let lowest = |t: f32| {
            dots(&shapes(row(), Some(avatar()), at(t), &pal()))
                .iter()
                .map(|b| f(b.bottom()))
                .fold(0., f32::max)
        };
        assert!(lowest(1.5) > 140. + 20., "sparks reach the rows below");
    }

    #[test]
    fn the_landing_bursts_around_the_avatar() {
        let near = |t: f32| {
            dots(&shapes(row(), Some(avatar()), at(t), &pal()))
                .iter()
                .filter(|b| {
                    let (x, y) = (f(b.center().x) - 36., f(b.center().y) - 100.);
                    (x * x + y * y).sqrt() < 40.
                })
                .count()
        };
        assert!(near(LANDS + 0.05) >= BURST as usize - 2);
        assert!(near(LANDS + 0.05) > near(LANDS - 0.1) + 10);
    }

    #[test]
    fn it_fades_in_and_ends_after_three_seconds() {
        let shapes_at = |t| shapes(row(), Some(avatar()), at(t), &pal());
        assert!(shapes_at(0.).is_empty(), "nothing pops in");
        assert!(shapes_at(0.5).len() > 20);
        assert!(shapes_at(3.).is_empty());
        assert!(at(3.).done() && !at(2.99).done());
    }

    #[test]
    fn with_no_avatar_in_view_it_lands_where_it_took_off() {
        // The outline's top left corner, in any layout.
        let near = |t: f32| {
            dots(&shapes(row(), None, at(t), &pal()))
                .iter()
                .filter(|b| {
                    let (x, y) = (f(b.center().x) - 5., f(b.center().y) - 101.);
                    (x * x + y * y).sqrt() < 16.
                })
                .count()
        };
        assert!(near(LANDS + 0.05) >= BURST as usize - 2);
        assert!(near(LANDS + 0.05) > near(LANDS - 0.1) + 10);
    }

    #[test]
    fn nothing_is_left_to_pop_when_it_ends() {
        let alpha = |s: &Shape| match s {
            Shape::Fill { color, .. }
            | Shape::Outline { color, .. }
            | Shape::Glow { color, .. } => color.a,
            Shape::Streak { from, to, .. } => from.a.max(to.a),
        };
        let last = shapes(row(), Some(avatar()), at(2.999), &pal());
        assert!(
            last.iter().all(|s| alpha(s) < 0.02),
            "still showing at 2.999 s: {last:?}"
        );
    }

    #[test]
    fn a_replaced_comet_only_lets_its_sparks_land() {
        let replaced = Moment {
            t: 1.3,
            replaced: Some(1.0),
        };
        let left = shapes(row(), Some(avatar()), replaced, &pal());
        assert!(!left.is_empty(), "sparks are still falling");
        assert!(
            left.iter().all(|s| match s {
                Shape::Fill { at, .. } | Shape::Glow { at, .. } => f(at.size.width) < 30.,
                _ => false,
            }),
            "no outline, trail or head: {left:?}"
        );
        // Once the last of them has landed, nothing is left.
        let gone = Moment {
            t: 1.0 + SPARK_LIFE,
            replaced: Some(1.0),
        };
        assert!(gone.done());
        assert!(shapes(row(), Some(avatar()), gone, &pal()).is_empty());
    }

    #[test]
    fn a_frame_depends_only_on_its_time() {
        let once = shapes(row(), Some(avatar()), at(1.234), &pal());
        let again = shapes(row(), Some(avatar()), at(1.234), &pal());
        assert_eq!(once, again);
    }
}

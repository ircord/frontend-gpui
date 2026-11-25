//! Avatar frames that move, on the profile card and its preview in Settings
//! (`avatar::framed_live`); the member list keeps its frames still.
//!
//! - **Spectrum** (the gradient frame): the web draws a conic sweep from the
//!   person's colour through amber, cyan and violet. GPUI's gradients have two
//!   stops, so the ring is drawn as [`SEGMENTS`] small pieces, each with its
//!   own colour, and the sweep turns once in [`SWEEP_SECS`].
//! - **Gold**: a glint crosses the frame for [`GLINT_RUN`] seconds every
//!   [`GLINT_SECS`].
//! - **Glow**: the blur of the glow breathes between [`BLUR_LOW`] and
//!   [`BLUR_HIGH`] pixels.
//!
//! What each frame looks like at a time is a pure function of that time, so
//! it is tested without a window; the canvases only read the clock. They ask
//! the display for its next frame while they are on screen, so a card that
//! is closed costs nothing, and with reduced motion they are not used at all.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use gpui_kit::*;

/// The sweep turns once in this many seconds.
pub const SWEEP_SECS: f32 = 8.;
/// A glint crosses the gold frame this often, in seconds...
pub const GLINT_SECS: f32 = 4.;
/// ...and takes this long to cross it.
pub const GLINT_RUN: f32 = 0.9;
/// The glow breathes once in this many seconds.
pub const BREATH_SECS: f32 = 3.;
/// The glow's blur at its smallest and largest, in pixels.
pub const BLUR_LOW: f32 = 14.;
pub const BLUR_HIGH: f32 = 22.;
/// The pieces a ring is drawn in. At the largest frame the chord of a piece
/// lies a tenth of a pixel inside the circle.
pub const SEGMENTS: usize = 48;
/// The width of the ring of a gradient or gold frame, in pixels.
pub const BAND: f32 = 3.;
/// Where the web's conic gradient starts, in turns clockwise from the top
/// (`from 210deg`).
const SWEEP_FROM: f32 = 210. / 360.;

/// How far the sweep has turned at `t` seconds, in turns.
pub fn sweep_turn(t: f64) -> f32 {
    (t / f64::from(SWEEP_SECS)).rem_euclid(1.) as f32
}

/// The colour `k` of the way from `a` to `b`.
pub fn mix(a: Rgba, b: Rgba, k: f32) -> Rgba {
    let k = k.clamp(0., 1.);
    let by = |a: f32, b: f32| a + (b - a) * k;
    Rgba {
        r: by(a.r, b.r),
        g: by(a.g, b.g),
        b: by(a.b, b.b),
        a: by(a.a, b.a),
    }
}

/// The colour of a conic gradient with `stops` spread evenly over one turn
/// (the last is the first again), `u` of a turn from where it starts.
pub fn sweep_color(stops: &[Rgba; 5], u: f32) -> Rgba {
    let at = u.rem_euclid(1.) * 4.;
    let from = (at.floor() as usize).min(3);
    mix(stops[from], stops[from + 1], at - from as f32)
}

/// The colour of piece `n` of the spectrum ring at `t` seconds. Pieces are
/// numbered clockwise from the top; the pattern starts at 210 degrees, as the
/// web's does, and turns clockwise.
pub fn segment_color(stops: &[Rgba; 5], n: usize, t: f64) -> Rgba {
    let at = (n as f32 + 0.5) / SEGMENTS as f32;
    sweep_color(stops, at - SWEEP_FROM - sweep_turn(t))
}

/// Where the glint is across the gold frame at `t` seconds, from 0 at the
/// top left of the ring to 1 at the bottom right, or `None` between glints.
/// It enters and leaves beyond the ring, so it never pops.
pub fn glint_at(t: f64) -> Option<f32> {
    let phase = t.rem_euclid(f64::from(GLINT_SECS)) as f32;
    if phase >= GLINT_RUN {
        return None;
    }
    let run = phase / GLINT_RUN;
    // Slow in, quick across, slow out.
    let eased = run * run * (3. - 2. * run);
    Some(-0.3 + 1.6 * eased)
}

/// How strongly the glint at `at` lights a piece of the ring `along` the
/// diagonal (0 to 1, top left to bottom right): a soft band, 0 to 1.
pub fn glint_strength(along: f32, at: f32) -> f32 {
    let d = (along - at) / 0.14;
    (-d * d).exp()
}

/// Where piece `n` of a ring lies along the glint's diagonal, 0 to 1.
pub fn along_diagonal(n: usize) -> f32 {
    let a = std::f32::consts::TAU * (n as f32 + 0.5) / SEGMENTS as f32;
    // The unit vector from the ring's middle, with y down, on the diagonal
    // that runs to the bottom right.
    let (x, y) = (a.sin(), -a.cos());
    0.5 + (x + y) / (2. * std::f32::consts::SQRT_2)
}

/// The blur of the glow at `t` seconds.
pub fn glow_blur(t: f64) -> f32 {
    let phase = (t / f64::from(BREATH_SECS)).rem_euclid(1.) as f32;
    let breath = 0.5 - 0.5 * (std::f32::consts::TAU * phase).cos();
    BLUR_LOW + (BLUR_HIGH - BLUR_LOW) * breath
}

/// The corners of piece `n` of a ring around `(cx, cy)` between the radii
/// `inner` and `outer`: inner start, outer start, outer end, inner end. The
/// piece runs a third of a piece past its end, so neighbours overlap and
/// leave no hairline between them.
pub fn segment_corners(n: usize, centre: (f32, f32), inner: f32, outer: f32) -> [(f32, f32); 4] {
    let turn = |a: f32, r: f32| {
        let angle = std::f32::consts::TAU * a;
        (centre.0 + r * angle.sin(), centre.1 - r * angle.cos())
    };
    let (from, to) = (
        n as f32 / SEGMENTS as f32,
        (n as f32 + 1.34) / SEGMENTS as f32,
    );
    [
        turn(from, inner),
        turn(from, outer),
        turn(to, outer),
        turn(to, inner),
    ]
}

/// Seconds on the clock the frames share: the same for every frame in every
/// window, so the card and the preview in Settings turn together.
pub fn now() -> f64 {
    START.get_or_init(Instant::now).elapsed().as_secs_f64() + skew()
}

static START: OnceLock<Instant> = OnceLock::new();
/// Seconds the shared clock is set ahead by, as `f64` bits ([`align`]).
static SKEW: AtomicU64 = AtomicU64::new(0);

fn skew() -> f64 {
    f64::from_bits(SKEW.load(Ordering::Relaxed))
}

/// Set the clock so that it stands `secs` into a cycle of `cycle` seconds,
/// for the tests to see a frame at a known moment. It keeps running.
#[doc(hidden)]
pub fn align(cycle: f32, secs: f32) {
    let ran = START.get_or_init(Instant::now).elapsed().as_secs_f64();
    let set = (f64::from(secs) - ran).rem_euclid(f64::from(cycle));
    SKEW.store(set.to_bits(), Ordering::Relaxed);
}

/// How many times each kind of frame was painted since the app started; the
/// tests read it to see what a frame drew.
#[derive(Default)]
struct Painted {
    sweeps: usize,
    glints: usize,
    glows: usize,
}

impl Global for Painted {}

/// `(sweeps, glints, glows)` painted so far: the spectrum ring, a gold glint
/// that was in view, and the glow's shadow.
pub fn painted(cx: &App) -> (usize, usize, usize) {
    cx.try_global::<Painted>()
        .map_or((0, 0, 0), |p| (p.sweeps, p.glints, p.glows))
}

/// The ring drawn as one filled piece, in `color`.
fn piece(window: &mut Window, corners: [(f32, f32); 4], color: Hsla) {
    let at = |(x, y): (f32, f32)| point(px(x), px(y));
    let mut path = PathBuilder::fill();
    path.move_to(at(corners[0]));
    for corner in &corners[1..] {
        path.line_to(at(*corner));
    }
    path.close();
    if let Ok(path) = path.build() {
        window.paint_path(path, color);
    }
}

/// The ring's middle and radii for the box it is drawn in: the band is
/// [`BAND`] wide at the box's edge, and reaches a little inside it, where
/// the picture's disc covers it.
fn ring(bounds: Bounds<Pixels>) -> ((f32, f32), f32, f32) {
    let outer = f32::from(bounds.size.width) / 2.;
    let centre = bounds.center();
    (
        (f32::from(centre.x), f32::from(centre.y)),
        outer - BAND - 0.75,
        outer,
    )
}

/// A canvas filling its parent that draws the spectrum ring: the conic
/// sweep from `color` through amber, cyan and violet, turning.
pub fn spectrum(color: Hsla) -> impl IntoElement {
    let stops = [
        Rgba::from(color),
        rgb(0xf59e0b),
        rgb(0x22d3ee),
        rgb(0xa78bfa),
        Rgba::from(color),
    ];
    canvas(
        |_, _, _| (),
        move |bounds, _, window, cx| {
            cx.default_global::<Painted>().sweeps += 1;
            let (centre, inner, outer) = ring(bounds);
            let t = now();
            for n in 0..SEGMENTS {
                let color = segment_color(&stops, n, t);
                piece(
                    window,
                    segment_corners(n, centre, inner, outer),
                    color.into(),
                );
            }
            window.request_animation_frame();
        },
    )
    .absolute()
    .size_full()
}

/// A canvas filling its parent that draws the gold frame's glint over the
/// ring, while one crosses it. Between glints it draws nothing, and asks for
/// no frames: it wakes the window when the next glint is due.
pub fn glint() -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |bounds, _, window, cx| {
            let t = now();
            let Some(at) = glint_at(t) else {
                wake_for_next_glint(t, window, cx);
                return;
            };
            cx.default_global::<Painted>().glints += 1;
            let (centre, inner, outer) = ring(bounds);
            for n in 0..SEGMENTS {
                let strength = glint_strength(along_diagonal(n), at);
                if strength > 0.02 {
                    piece(
                        window,
                        segment_corners(n, centre, inner, outer),
                        hsla(0.12, 0.9, 0.97, 0.8 * strength),
                    );
                }
            }
            window.request_animation_frame();
        },
    )
    .absolute()
    .size_full()
}

/// Whether a wake-up for the next glint is already waiting, so frames drawn
/// meanwhile by other events do not each set one.
#[derive(Default)]
struct Waiting(bool);

impl Global for Waiting {}

/// Draw the window again when the next glint starts. GPUI draws a frame only
/// when something changed, so a glint would otherwise wait for an event.
fn wake_for_next_glint(t: f64, window: &mut Window, cx: &mut App) {
    if std::mem::replace(&mut cx.default_global::<Waiting>().0, true) {
        return;
    }
    let wait = Duration::from_secs_f64(
        (f64::from(GLINT_SECS) - t.rem_euclid(f64::from(GLINT_SECS))).max(0.) + 0.02,
    );
    let handle = window.window_handle();
    cx.spawn(async move |cx| {
        cx.background_executor().timer(wait).await;
        // The flag is cleared even when the window is gone (closed to the
        // tray meanwhile), or no later window's glint would ever wake.
        cx.update(|cx| cx.default_global::<Waiting>().0 = false);
        cx.update_window(handle, |_, window, _| window.refresh())
            .ok();
    })
    .detach();
}

/// A canvas filling its parent, the glow frame's inside (its 2 px border
/// is outside it), that draws the glow's shadow around the frame, breathing.
pub fn glow(color: Hsla) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |bounds, _, window, cx| {
            cx.default_global::<Painted>().glows += 1;
            let bounds = bounds.dilate(px(2.));
            window.paint_drop_shadows(
                bounds,
                Corners::all(bounds.size.width / 2.),
                &[BoxShadow {
                    color: color.opacity(0.65),
                    offset: point(px(0.), px(0.)),
                    blur_radius: px(glow_blur(now())),
                    spread_radius: px(2.),
                    inset: false,
                }],
            );
            window.request_animation_frame();
        },
    )
    .absolute()
    .size_full()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(v: u32) -> Rgba {
        rgb(v)
    }

    fn stops() -> [Rgba; 5] {
        [
            hex(0xff0000),
            hex(0xf59e0b),
            hex(0x22d3ee),
            hex(0xa78bfa),
            hex(0xff0000),
        ]
    }

    fn near(a: Rgba, b: Rgba) -> bool {
        (a.r - b.r).abs() + (a.g - b.g).abs() + (a.b - b.b).abs() < 1e-4
    }

    #[::core::prelude::v1::test]
    fn the_sweep_turns_once_in_eight_seconds() {
        assert_eq!(sweep_turn(0.), 0.);
        assert!((sweep_turn(2.) - 0.25).abs() < 1e-6);
        assert!(sweep_turn(8.) < 1e-6 || sweep_turn(8.) > 0.999_999);
        assert!((sweep_turn(8. + 2.) - sweep_turn(2.)).abs() < 1e-5);
        assert!(sweep_turn(-2.) > 0.7, "never negative");
    }

    #[::core::prelude::v1::test]
    fn the_sweep_runs_through_the_four_colours_and_back() {
        let s = stops();
        assert!(near(sweep_color(&s, 0.), s[0]));
        assert!(near(sweep_color(&s, 0.25), s[1]));
        assert!(near(sweep_color(&s, 0.5), s[2]));
        assert!(near(sweep_color(&s, 0.75), s[3]));
        assert!(near(sweep_color(&s, 1.), s[0]), "a full turn is the start");
        assert!(near(sweep_color(&s, -0.25), s[3]), "before the start wraps");
        // Half way between two stops is their mix.
        assert!(near(sweep_color(&s, 0.125), mix(s[0], s[1], 0.5)));
    }

    #[::core::prelude::v1::test]
    fn the_ring_starts_at_the_webs_angle_and_turns_clockwise() {
        let s = stops();
        // At rest the first stop lies at 210 degrees: the piece just past it.
        let first = (0..SEGMENTS)
            .min_by(|a, b| {
                let d = |n: &usize| {
                    let c = segment_color(&s, *n, 0.);
                    (c.r - s[0].r).abs() + c.g + c.b
                };
                d(a).total_cmp(&d(b))
            })
            .unwrap();
        let angle = (first as f32 + 0.5) / SEGMENTS as f32 * 360.;
        assert!((angle - 210.).abs() < 8., "{angle}");
        // A quarter turn later the pattern is a quarter of the ring further on.
        let n = 5;
        let later = segment_color(
            &s,
            (n + SEGMENTS / 4) % SEGMENTS,
            f64::from(SWEEP_SECS) / 4.,
        );
        assert!(near(later, segment_color(&s, n, 0.)));
    }

    #[::core::prelude::v1::test]
    fn neighbouring_pieces_differ_a_little_and_never_jump() {
        let s = stops();
        for t in [0., 1.3, 5.9] {
            for n in 0..SEGMENTS {
                let (a, b) = (
                    segment_color(&s, n, t),
                    segment_color(&s, (n + 1) % SEGMENTS, t),
                );
                let step = (a.r - b.r).abs() + (a.g - b.g).abs() + (a.b - b.b).abs();
                assert!(step < 0.2, "piece {n} at {t}: {step}");
            }
        }
    }

    #[::core::prelude::v1::test]
    fn a_glint_crosses_for_under_a_second_every_four() {
        assert_eq!(glint_at(0.), Some(-0.3));
        let mid = glint_at(f64::from(GLINT_RUN) / 2.).unwrap();
        assert!((mid - 0.5).abs() < 1e-5, "{mid}");
        let end = glint_at(f64::from(GLINT_RUN) - 0.001).unwrap();
        assert!(end > 1.2, "it leaves the ring: {end}");
        assert_eq!(glint_at(f64::from(GLINT_RUN)), None);
        assert_eq!(glint_at(3.9), None);
        assert_eq!(glint_at(f64::from(GLINT_SECS)), glint_at(0.));
        assert!(
            (glint_at(f64::from(GLINT_SECS) * 5. + 0.3).unwrap() - glint_at(0.3).unwrap()).abs()
                < 1e-3
        );
        // It only goes forward.
        let mut last = f32::MIN;
        for n in 0..90 {
            let at = glint_at(f64::from(n) * 0.01).unwrap();
            assert!(at >= last);
            last = at;
        }
    }

    #[::core::prelude::v1::test]
    fn the_glint_is_a_soft_band_across_the_diagonal() {
        assert_eq!(glint_strength(0.5, 0.5), 1.);
        assert!(glint_strength(0.6, 0.5) < 1. && glint_strength(0.6, 0.5) > 0.4);
        assert!(glint_strength(1., 0.) < 0.001);
        // Pieces at the top left of the ring come first, the bottom right last.
        let (top_left, bottom_right) = (
            along_diagonal(SEGMENTS * 7 / 8),
            along_diagonal(SEGMENTS * 3 / 8),
        );
        assert!(
            top_left < 0.1 && bottom_right > 0.9,
            "{top_left} {bottom_right}"
        );
        assert!((0..SEGMENTS).all(|n| (0. ..=1.).contains(&along_diagonal(n))));
    }

    #[::core::prelude::v1::test]
    fn the_glow_breathes_between_its_limits() {
        assert!((glow_blur(0.) - BLUR_LOW).abs() < 1e-4);
        assert!((glow_blur(f64::from(BREATH_SECS) / 2.) - BLUR_HIGH).abs() < 1e-4);
        assert!((glow_blur(f64::from(BREATH_SECS)) - BLUR_LOW).abs() < 1e-4);
        assert!((0..300).all(|n| {
            let b = glow_blur(f64::from(n) * 0.037);
            (BLUR_LOW - 1e-4..=BLUR_HIGH + 1e-4).contains(&b)
        }));
    }

    #[::core::prelude::v1::test]
    fn a_piece_lies_between_the_radii_at_its_angle() {
        let centre = (100., 100.);
        let [a, b, c, d] = segment_corners(0, centre, 40., 45.);
        // The first piece starts at the top: straight above the middle.
        assert!((a.0 - 100.).abs() < 1e-3 && (a.1 - 60.).abs() < 1e-3);
        assert!((b.0 - 100.).abs() < 1e-3 && (b.1 - 55.).abs() < 1e-3);
        // And runs clockwise, a little further than its own share.
        assert!(c.0 > 100. && d.0 > 100.);
        let dist = |p: (f32, f32)| ((p.0 - 100.).powi(2) + (p.1 - 100.).powi(2)).sqrt();
        assert!((dist(c) - 45.).abs() < 1e-3 && (dist(d) - 40.).abs() < 1e-3);
        // A quarter of the way round is on the right.
        let [a, ..] = segment_corners(SEGMENTS / 4, centre, 40., 45.);
        assert!((a.0 - 140.).abs() < 1e-3 && (a.1 - 100.).abs() < 1e-3);
    }

    #[::core::prelude::v1::test]
    fn the_clock_runs_forward_and_is_shared() {
        let (a, b) = (now(), now());
        assert!(b >= a);
    }
}

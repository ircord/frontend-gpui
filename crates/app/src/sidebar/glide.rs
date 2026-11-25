//! The sidebar's selection highlight, which glides from the row it was on to
//! the one just opened on a spring, instead of jumping.
//!
//! The highlight is one shape drawn behind the rows of the list; the open
//! row only draws its text in the open colour. The list reports where the
//! open row was laid out ([`Shared::report`]), and the canvas under the rows
//! ([`layer`]) moves the highlight towards it. A spring keeps its speed when
//! the target changes, so stepping through channels with the keyboard makes
//! the highlight follow in one motion.
//!
//! The motion ([`Highlight`]) is a pure function of time. It starts when the
//! open conversation changes and asks for the display's next frame only until
//! it has arrived, so a sidebar nobody touches draws nothing. With reduced
//! motion the highlight is not used: the open row draws its own background,
//! as it did before.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Instant;

use gpui_kit::*;

use crate::theme::Theme;

/// The spring the highlight rides: about 220 ms to settle, with barely a
/// trace of overshoot.
pub const SPRING: SpringConfig = SpringConfig::new(600., 44., 1.);
/// Closer than this (in pixels) to its target, and slower than [`REST_SPEED`],
/// the highlight counts as arrived.
const REST_DISTANCE: f32 = 0.2;
/// Pixels per second.
const REST_SPEED: f32 = 8.;

/// Where the highlight is, in pixels from the top of the list's content: its
/// top edge and its height. Rows differ in height, so both move.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Span {
    pub top: f32,
    pub height: f32,
}

/// The highlight's motion. Times are seconds on any clock the caller keeps,
/// so a test drives it without waiting.
#[derive(Debug, Clone, PartialEq)]
pub struct Highlight {
    top: SpringState,
    height: SpringState,
    target: Span,
    at: f64,
}

impl Highlight {
    /// A highlight at rest on `to`.
    pub fn at_rest(to: Span, now: f64) -> Self {
        let still = |position| SpringState {
            position,
            velocity: 0.,
        };
        Self {
            top: still(to.top),
            height: still(to.height),
            target: to,
            at: now,
        }
    }

    /// Run the spring forward to `now`.
    fn advance(&mut self, now: f64) {
        let dt = (now - self.at).max(0.) as f32;
        self.top = SPRING.step(self.top, self.target.top, dt);
        self.height = SPRING.step(self.height, self.target.height, dt);
        self.at = self.at.max(now);
        if self.settled() {
            // Arrived: stop on the row itself, not a fraction of a pixel off.
            let on = |position| SpringState {
                position,
                velocity: 0.,
            };
            self.top = on(self.target.top);
            self.height = on(self.target.height);
        }
    }

    /// Aim at `to` from wherever the highlight is at `now`, keeping its speed.
    pub fn retarget(&mut self, to: Span, now: f64) {
        if to != self.target {
            self.advance(now);
            self.target = to;
        }
    }

    /// The highlight at `now`.
    pub fn sample(&mut self, now: f64) -> Span {
        self.advance(now);
        Span {
            top: self.top.position,
            height: self.height.position,
        }
    }

    /// Whether the highlight has arrived and stopped.
    pub fn settled(&self) -> bool {
        let done = |s: SpringState, target: f32| {
            (s.position - target).abs() < REST_DISTANCE && s.velocity.abs() < REST_SPEED
        };
        done(self.top, self.target.top) && done(self.height, self.target.height)
    }
}

/// Where the open row is, measured from the highlight's own canvas, which
/// is laid out with the rows and so moves with the list's scroll.
#[derive(Clone, Debug, PartialEq)]
struct Target {
    /// The network whose list this is, so a switch of network (another
    /// list) places the highlight without gliding across it.
    network: String,
    span: Span,
    /// The row's left edge from the canvas's, and its width.
    left: Pixels,
    width: Pixels,
}

#[derive(Default)]
struct State {
    /// The open row as this frame laid it out; `None` when it is not shown.
    target: Option<Target>,
    highlight: Option<(String, Highlight)>,
}

/// What the highlight drew in the last frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Shown {
    pub span: Span,
    /// The target it is moving to; equal to `span` once it has arrived.
    pub target: Span,
    pub moving: bool,
    /// Where it was drawn, in the window.
    pub bounds: Bounds<Pixels>,
}

/// The highlight of one sidebar, shared by the list that reports the open
/// row and the canvas that draws it. One per sidebar, so two windows never
/// mix.
#[derive(Clone)]
pub struct Shared {
    state: Rc<RefCell<State>>,
    epoch: Instant,
}

impl Default for Shared {
    fn default() -> Self {
        Self {
            state: Rc::default(),
            epoch: Instant::now(),
        }
    }
}

/// What the sidebar's highlight drew in the last frame of the window that
/// drew one last.
#[derive(Default)]
struct Last(Option<Shown>);

impl Global for Last {}

/// The highlight as the last frame drew it, or `None` when it drew none
/// (reduced motion, or no open conversation in view).
pub fn shown(cx: &App) -> Option<Shown> {
    cx.try_global::<Last>().and_then(|l| l.0)
}

/// The sidebar is drawn again: what the last frame drew is no longer what
/// is there, until the highlight draws.
pub(super) fn forget(cx: &mut App) {
    cx.default_global::<Last>().0 = None;
}

impl Shared {
    /// The list was laid out. `children` are the bounds of its children as
    /// laid out (before the list's scroll), the first being the highlight's
    /// canvas; `open` is the index of the open row, if it is one of them.
    pub fn report(&self, network: String, children: &[Bounds<Pixels>], open: Option<usize>) {
        let target = open.and_then(|open| {
            let (row, canvas) = (children.get(open)?, children.first()?);
            Some(Target {
                network,
                span: Span {
                    top: f32::from(row.origin.y - canvas.origin.y),
                    height: f32::from(row.size.height),
                },
                left: row.origin.x - canvas.origin.x,
                width: row.size.width,
            })
        });
        self.state.borrow_mut().target = target;
    }

    /// Draw the highlight for the frame, moving it towards the open row.
    fn paint(&self, canvas: Bounds<Pixels>, theme: &Theme, window: &mut Window, cx: &mut App) {
        let now = self.epoch.elapsed().as_secs_f64();
        let mut state = self.state.borrow_mut();
        let Some(target) = state.target.clone() else {
            state.highlight = None;
            drop(state);
            cx.default_global::<Last>().0 = None;
            return;
        };
        match &mut state.highlight {
            Some((network, highlight)) if *network == target.network => {
                highlight.retarget(target.span, now)
            }
            slot => *slot = Some((target.network.clone(), Highlight::at_rest(target.span, now))),
        }
        let (_, highlight) = state.highlight.as_mut().expect("set above");
        let span = highlight.sample(now);
        let moving = !highlight.settled();
        let bounds = Bounds::new(
            point(
                canvas.origin.x + target.left,
                canvas.origin.y + px(span.top),
            ),
            size(target.width, px(span.height.max(0.))),
        );
        drop(state);
        cx.default_global::<Last>().0 = Some(Shown {
            span,
            target: target.span,
            moving,
            bounds,
        });
        paint_selection(bounds, theme, window);
        if moving {
            window.request_animation_frame();
        }
    }
}

/// The open row's look: its tint, its edge and, in Hibiki, its gradient
/// and glow, as the row drew them itself.
fn paint_selection(bounds: Bounds<Pixels>, theme: &Theme, window: &mut Window) {
    let corners = Corners::all(theme.radius);
    let edge = theme.accent.opacity(if theme.hibiki { 0.58 } else { 0.26 });
    let fill: Background = if theme.hibiki {
        window.paint_drop_shadows(
            bounds,
            corners,
            &[BoxShadow {
                color: theme.accent.opacity(0.20),
                offset: point(px(0.), px(4.)),
                blur_radius: px(14.),
                spread_radius: px(0.),
                inset: false,
            }],
        );
        linear_gradient(
            95.,
            linear_color_stop(theme.sidebar.blend(theme.accent.opacity(0.52)), 0.),
            linear_color_stop(theme.sidebar.blend(theme.accent.opacity(0.24)), 1.),
        )
    } else {
        theme.selected.into()
    };
    window.paint_quad(quad(
        bounds,
        corners,
        fill,
        px(1.),
        edge,
        BorderStyle::Solid,
    ));
}

/// The canvas that draws the highlight. It is the list's first child, so the
/// rows paint over it.
pub fn layer(shared: Shared, theme: Theme) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |bounds, _, window, cx| shared.paint(bounds, &theme, window, cx),
    )
    .absolute()
    .size_full()
}

#[cfg(test)]
mod tests {
    use super::*;

    const FRAME: f64 = 1. / 60.;

    fn span(top: f32, height: f32) -> Span {
        Span { top, height }
    }

    /// Run the highlight to `until` seconds at 60 frames a second, from `from`.
    fn run(h: &mut Highlight, from: f64, until: f64) -> Vec<Span> {
        let mut out = Vec::new();
        let mut t = from;
        while t < until {
            t += FRAME;
            out.push(h.sample(t));
        }
        out
    }

    #[::core::prelude::v1::test]
    fn a_highlight_at_rest_stays() {
        let mut h = Highlight::at_rest(span(40., 28.), 0.);
        assert!(h.settled());
        assert_eq!(h.sample(5.), span(40., 28.));
    }

    #[::core::prelude::v1::test]
    fn it_glides_to_the_new_row_and_arrives_within_a_third_of_a_second() {
        let mut h = Highlight::at_rest(span(40., 28.), 0.);
        h.retarget(span(200., 44.), 0.);
        let frames = run(&mut h, 0., 0.35);
        // It starts from where it was, not from the target.
        assert!(frames[0].top < 80., "{:?}", frames[0]);
        // Each frame moves on, and the height follows the row's height.
        assert!(frames.windows(2).take(8).all(|w| w[1].top > w[0].top));
        assert!(frames[2].height > 28. && frames[2].height < 44.);
        // It has arrived, and says so.
        assert!(h.settled(), "{:?}", frames.last());
        let end = frames.last().unwrap();
        assert!((end.top - 200.).abs() < 0.5 && (end.height - 44.).abs() < 0.5);
    }

    #[::core::prelude::v1::test]
    fn it_is_still_on_its_way_after_a_tenth_of_a_second() {
        let mut h = Highlight::at_rest(span(0., 28.), 0.);
        h.retarget(span(300., 28.), 0.);
        h.sample(0.1);
        assert!(!h.settled());
    }

    #[::core::prelude::v1::test]
    fn it_barely_overshoots() {
        let mut h = Highlight::at_rest(span(0., 28.), 0.);
        h.retarget(span(300., 28.), 0.);
        let most = run(&mut h, 0., 1.).iter().map(|s| s.top).fold(0., f32::max);
        assert!(most <= 301., "went as far as {most}");
    }

    #[::core::prelude::v1::test]
    fn the_motion_does_not_depend_on_the_frame_rate() {
        let at = |frames: u32| {
            let mut h = Highlight::at_rest(span(0., 28.), 0.);
            h.retarget(span(300., 44.), 0.);
            let mut last = span(0., 0.);
            for n in 1..=frames {
                last = h.sample(0.12 * f64::from(n) / f64::from(frames));
            }
            last
        };
        let (slow, fast) = (at(2), at(120));
        assert!((slow.top - fast.top).abs() < 0.01, "{slow:?} {fast:?}");
        assert!((slow.height - fast.height).abs() < 0.01);
    }

    #[::core::prelude::v1::test]
    fn a_new_target_keeps_the_speed_it_had() {
        // Two quick steps down the list: the highlight does not stop at the
        // first row on its way to the second.
        let mut h = Highlight::at_rest(span(0., 28.), 0.);
        h.retarget(span(30., 28.), 0.);
        h.sample(0.06);
        let before = h.top.velocity;
        assert!(before > 50., "{before}");
        h.retarget(span(60., 28.), 0.06);
        assert_eq!(h.top.velocity, before);
        let frames = run(&mut h, 0.06, 0.12);
        assert!(frames.windows(2).all(|w| w[1].top >= w[0].top));
        // And it ends on the last target.
        run(&mut h, 0.12, 0.6);
        assert!(h.settled() && (h.sample(1.).top - 60.).abs() < 0.5);
    }

    #[::core::prelude::v1::test]
    fn it_stops_exactly_on_the_row() {
        let mut h = Highlight::at_rest(span(0., 28.), 0.);
        h.retarget(span(123.4, 44.), 0.);
        let end = run(&mut h, 0., 1.).pop().unwrap();
        assert_eq!(end, span(123.4, 44.));
        assert_eq!(h.top.velocity, 0.);
    }

    #[::core::prelude::v1::test]
    fn after_resting_undrawn_it_still_starts_from_its_row() {
        let mut h = Highlight::at_rest(span(40., 28.), 0.);
        h.sample(0.);
        // Nothing asked for a frame for five seconds.
        h.retarget(span(200., 44.), 5.);
        assert_eq!(
            h.sample(5.),
            span(40., 28.),
            "it starts from where it rested"
        );
        assert!(!h.settled());
    }

    #[::core::prelude::v1::test]
    fn retargeting_the_same_row_changes_nothing() {
        let mut h = Highlight::at_rest(span(0., 28.), 0.);
        h.retarget(span(100., 28.), 0.);
        h.sample(0.05);
        let state = h.clone();
        h.retarget(span(100., 28.), 0.05);
        assert_eq!(h, state);
    }

    #[::core::prelude::v1::test]
    fn a_clock_that_runs_backwards_does_not_move_it() {
        let mut h = Highlight::at_rest(span(0., 28.), 1.);
        h.retarget(span(100., 28.), 1.);
        let before = h.sample(1.1);
        assert_eq!(h.sample(0.5), before);
    }

    #[::core::prelude::v1::test]
    fn the_list_reports_the_open_row_from_its_canvas() {
        let at = |y: f32, h: f32| Bounds::new(point(px(8.), px(y)), size(px(200.), px(h)));
        let shared = Shared::default();
        let key = "n1".to_string();
        // The canvas, a heading, two rows: the span is how far the open row
        // is from the canvas, whatever the list's scroll.
        let rows = [at(4., 0.), at(4., 20.), at(26., 28.), at(56., 28.)];
        shared.report(key.clone(), &rows, Some(3));
        let target = shared.state.borrow().target.clone().unwrap();
        assert_eq!(target.span, span(52., 28.));
        assert_eq!((target.left, target.width), (px(0.), px(200.)));
        // No open row in the list, or one that is not there: nothing.
        shared.report(key.clone(), &rows, None);
        assert!(shared.state.borrow().target.is_none());
        shared.report(key, &rows, Some(9));
        assert!(shared.state.borrow().target.is_none());
    }
}

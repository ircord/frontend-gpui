//! Depth for a personal theme's background picture: while you read back
//! into history the picture drifts a few pixels, and it comes back as you
//! return to the newest message. The frosted panels draw their own region of
//! the same picture, so they move with it.
//!
//! The timeline reports how far the view is from the newest message
//! ([`follow`]); [`target`] turns that into the shift the picture is heading
//! for, which grows quickly at first and never passes [`LIMIT`], and
//! [`Shift`] eases the picture towards it. Both are pure functions of their
//! inputs, so the motion is tested without a window. Nothing runs while the
//! shift is at its target: the paint asks for the display's next frame only
//! until it arrives.
//!
//! With reduced motion the picture stays where it was and is not drawn
//! oversize.

use std::time::Instant;

use gpui_kit::*;

use super::get as theme;

/// The most the picture shifts down, in pixels, and so how far it is drawn
/// oversize above its area, so the shift never shows an edge.
pub const LIMIT: f32 = 16.;
/// Nearer than this to the newest message, in pixels, counts as at it: a
/// view a line or two up has not left it, and the fraction of a pixel a
/// list sits off its end is not history.
const NEAR: f32 = 24.;
/// How far past [`NEAR`] into history, in pixels, the shift has covered about
/// two thirds of its way to the limit.
const REACH: f32 = 1500.;
/// The time constant of the ease, in milliseconds.
const EASE_MS: f32 = 180.;
/// Closer than this to its target counts as arrived.
const ARRIVED: f32 = 0.05;

/// The shift the picture heads for when the view is `distance` pixels above
/// the newest message: nothing at it or near it, then growing with
/// diminishing returns, never past [`LIMIT`].
pub fn target(distance: f32) -> f32 {
    LIMIT * (1. - (-(distance - NEAR).max(0.) / REACH).exp())
}

/// Where a shift that left `from` at its start has got to `elapsed` seconds
/// later on its way to `to`. Easing out, so a shift that is asked to turn
/// round does so smoothly from where it is.
pub fn ease(from: f32, to: f32, elapsed: f32) -> f32 {
    to + (from - to) * (-elapsed.max(0.) * 1000. / EASE_MS).exp()
}

/// The picture's shift: where it was when it was last aimed, and what it
/// heads for.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Shift {
    from: f32,
    to: f32,
    /// Seconds on the caller's clock when it was aimed.
    since: f32,
}

impl Shift {
    pub const fn at_rest() -> Self {
        Self {
            from: 0.,
            to: 0.,
            since: 0.,
        }
    }

    /// The shift at `now`; exactly the target once it has arrived.
    pub fn at(&self, now: f32) -> f32 {
        let shift = ease(self.from, self.to, now - self.since);
        if (shift - self.to).abs() < ARRIVED {
            self.to
        } else {
            shift
        }
    }

    /// Head for `to` from wherever the shift is at `now`. A target no
    /// different from the one it already heads for changes nothing, so
    /// the timeline may report on every frame.
    pub fn aim(&mut self, to: f32, now: f32) {
        if (to - self.to).abs() > 0.01 {
            *self = Self {
                from: self.at(now),
                to,
                since: now,
            };
        }
    }

    /// Whether the shift is at its target.
    pub fn arrived(&self, now: f32) -> bool {
        self.at(now) == self.to
    }

    /// Back to nothing at once.
    pub fn reset(&mut self) {
        *self = Self::at_rest();
    }
}

/// The shift of the app's picture, and the clock it is measured on.
struct Depth {
    shift: Shift,
    epoch: Instant,
}

impl Global for Depth {}

impl Default for Depth {
    fn default() -> Self {
        Self {
            shift: Shift::at_rest(),
            epoch: Instant::now(),
        }
    }
}

impl Depth {
    fn now(&self) -> f32 {
        self.epoch.elapsed().as_secs_f32()
    }
}

/// The view is `distance` pixels above the newest message (0 at it). Draws
/// the windows again when the picture has to move. With reduced motion the
/// picture stays put.
pub fn follow(distance: f32, cx: &mut App) {
    let aim = if theme(cx).reduced_motion {
        0.
    } else {
        target(distance)
    };
    let depth = cx.default_global::<Depth>();
    let now = depth.now();
    let before = depth.shift;
    depth.shift.aim(aim, now);
    if depth.shift != before {
        cx.refresh_windows();
    }
}

/// The picture is not behind a conversation any more: it eases back.
pub fn release(cx: &mut App) {
    follow(0., cx);
}

/// How far the picture is shifted now, and whether it is still on its way
/// (the caller then asks for the next frame). Nothing is shifted with
/// reduced motion.
pub fn current(cx: &mut App) -> (f32, bool) {
    if theme(cx).reduced_motion {
        cx.default_global::<Depth>().shift.reset();
        return (0., false);
    }
    let depth = cx.default_global::<Depth>();
    let now = depth.now();
    (depth.shift.at(now), !depth.shift.arrived(now))
}

/// `area` as the picture is placed in it while the shift is `shift`: taller
/// by [`LIMIT`] at the top, and moved down by the shift, so its top edge
/// reaches the area's own at the full shift and lies above it before. With
/// reduced motion it is `area`, as it always was.
pub fn placed_in(area: Bounds<Pixels>, shift: f32, cx: &App) -> Bounds<Pixels> {
    if theme(cx).reduced_motion {
        return area;
    }
    Bounds::new(
        point(area.origin.x, area.origin.y + px(shift - LIMIT)),
        size(area.size.width, area.size.height + px(LIMIT)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[::core::prelude::v1::test]
    fn nothing_shifts_at_the_newest_message_and_never_past_the_limit() {
        assert_eq!(target(0.), 0.);
        assert_eq!(target(-40.), 0., "below the end is the end");
        assert_eq!(target(NEAR), 0., "a line or two up is still at the end");
        assert!(target(NEAR + 1.) > 0.);
        let steps: Vec<f32> = [100., 500., 1500., 5000., 50_000., 1e9]
            .iter()
            .map(|d| target(*d))
            .collect();
        assert!(steps.windows(2).all(|w| w[1] > w[0] - 1e-6), "{steps:?}");
        assert!(steps.iter().all(|s| (0. ..=LIMIT).contains(s)), "{steps:?}");
        // It is already noticeable after a screen or two of history, and
        // has eased towards the limit after many.
        assert!(target(1000.) > 5. && target(1000.) < 10.);
        assert!(target(NEAR + REACH) > 9. && target(NEAR + REACH) < 11.);
        assert!(target(50_000.) > 15.9);
    }

    #[::core::prelude::v1::test]
    fn the_ease_runs_from_where_it_started_to_where_it_is_going() {
        assert_eq!(ease(2., 12., 0.), 2.);
        let mid = ease(0., 16., 0.18);
        assert!(mid > 8. && mid < 12., "{mid}");
        assert!((ease(0., 16., 2.) - 16.).abs() < 0.01);
        // Only forwards in time.
        assert_eq!(ease(0., 16., -1.), 0.);
    }

    #[::core::prelude::v1::test]
    fn the_shift_never_passes_its_target_in_either_direction() {
        for (from, to) in [(0., 16.), (16., 0.), (3., 9.), (9., 3.)] {
            let mut last = from;
            for n in 1..200 {
                let now = ease(from, to, n as f32 * 0.01);
                let (lo, hi) = (from.min(to), from.max(to));
                assert!(now >= lo - 1e-4 && now <= hi + 1e-4, "{from}->{to}: {now}");
                assert!((now - to).abs() <= (last - to).abs() + 1e-4);
                last = now;
            }
        }
    }

    #[::core::prelude::v1::test]
    fn a_shift_starts_when_aimed_and_arrives() {
        let mut shift = Shift::at_rest();
        assert!(shift.arrived(0.));
        shift.aim(16., 1.);
        assert_eq!(shift.at(1.), 0.);
        assert!(!shift.arrived(1.));
        assert!(shift.at(1.1) > 4.);
        assert!(!shift.arrived(1.5));
        assert!(shift.arrived(3.));
        assert!((shift.at(3.) - 16.).abs() < 0.05);
    }

    #[::core::prelude::v1::test]
    fn turning_round_starts_from_where_it_is() {
        let mut shift = Shift::at_rest();
        shift.aim(16., 0.);
        let there = shift.at(0.2);
        shift.aim(0., 0.2);
        assert!((shift.at(0.2) - there).abs() < 1e-4, "no jump");
        assert!(shift.at(0.3) < there);
        assert!(shift.arrived(5.) && shift.at(5.).abs() < 0.05);
    }

    #[::core::prelude::v1::test]
    fn aiming_at_the_same_place_does_not_restart_it() {
        let mut shift = Shift::at_rest();
        shift.aim(10., 0.);
        let before = shift;
        shift.aim(10., 0.1);
        shift.aim(10.004, 0.2);
        assert_eq!(shift, before, "the timeline may report every frame");
    }

    #[::core::prelude::v1::test]
    fn reset_is_at_once() {
        let mut shift = Shift::at_rest();
        shift.aim(16., 0.);
        shift.reset();
        assert_eq!(shift.at(0.5), 0.);
        assert!(shift.arrived(0.));
    }
}

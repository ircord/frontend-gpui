//! The glide to the newest message (package P6), as the web client draws it:
//! each frame covers a share of the distance left, the share growing with
//! the time since the last frame, and the glide ends within 700 ms even when
//! it has not arrived. The step is a pure function, so the motion is tested
//! without a window or a timer; the view drives the frames.

use std::time::Duration;

/// The time constant of the ease, in milliseconds.
const EASE_MS: f32 = 55.0;
/// A glide ends at the latest this long after it starts.
pub const LIMIT: Duration = Duration::from_millis(700);
/// Closer than this to the end counts as arrived.
pub const ARRIVED: f32 = 1.0;

/// How far to scroll this frame. `gap` is the distance left in pixels,
/// positive when the end is below. The step is at least a pixel, and never
/// overshoots the end.
pub fn step(gap: f32, since_last: Duration) -> f32 {
    let distance = gap.abs();
    let fraction = 1.0 - (-since_last.as_secs_f32() * 1000.0 / EASE_MS).exp();
    gap.signum() * (distance * fraction).max(1.0).min(distance)
}

/// Whether the glide is over: the end is reached, or the time is up.
pub fn finished(gap: f32, elapsed: Duration) -> bool {
    gap.abs() <= ARRIVED || elapsed >= LIMIT
}

#[cfg(test)]
mod tests {
    use super::*;

    const FRAME: Duration = Duration::from_millis(16);

    #[test]
    fn each_frame_moves_towards_the_end_without_overshoot() {
        let mut gap = 1000.0_f32;
        for _ in 0..200 {
            if gap <= ARRIVED {
                break;
            }
            let by = step(gap, FRAME);
            assert!(by > 0.0 && by <= gap, "moved {by} of {gap}");
            gap -= by;
        }
        assert!(gap <= ARRIVED + 1e-3, "gap {gap} after many frames");
    }

    #[test]
    fn a_frame_moves_at_least_one_pixel() {
        assert_eq!(step(500.0, Duration::ZERO), 1.0);
        assert_eq!(step(-500.0, Duration::ZERO), -1.0);
        // A gap smaller than a pixel is covered whole, not overshot.
        assert_eq!(step(0.4, FRAME), 0.4);
    }

    #[test]
    fn a_frame_covers_about_a_quarter_of_the_gap_at_sixty_hertz() {
        let by = step(400.0, FRAME);
        // 1 - e^(-16/55) is about 0.25.
        assert!((by - 100.0).abs() < 2.0, "moved {by}");
    }

    #[test]
    fn the_glide_ends_at_the_limit_or_on_arrival() {
        assert!(!finished(100.0, Duration::from_millis(100)));
        assert!(finished(100.0, LIMIT));
        assert!(finished(0.5, Duration::ZERO));
    }
}

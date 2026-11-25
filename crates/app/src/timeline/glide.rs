//! The glide to the newest message (package P6), as the web client draws it:
//! each frame covers a share of the distance left, the share growing with
//! the time since the last frame, and the glide ends within 700 ms even when
//! it has not arrived. The step is a pure function, so the motion is tested
//! without a window or a timer; the view drives the frames.
//!
//! The same glide carries a message that arrives while you are at the newest
//! one: the list holds its place, the new row sits just below, and the view
//! eases the gap shut in about 150 ms ([`ARRIVAL_EASE_MS`]) instead of
//! moving every row at once.

use std::time::Duration;

/// The time constant of the ease to the newest message, in milliseconds.
pub const EASE_MS: f32 = 55.0;
/// The time constant for a message that arrives: the rows are home within
/// about 150 ms (the gap is under a pixel after `ln(gap)` constants).
pub const ARRIVAL_EASE_MS: f32 = 30.0;
/// A glide ends at the latest this long after it starts.
pub const LIMIT: Duration = Duration::from_millis(700);
/// Closer than this to the end counts as arrived.
pub const ARRIVED: f32 = 1.0;

/// How far to scroll this frame, for an ease with time constant `ease_ms`.
/// `gap` is the distance left in pixels,
/// positive when the end is below. The step is at least a pixel, and never
/// overshoots the end.
pub fn step_with(gap: f32, since_last: Duration, ease_ms: f32) -> f32 {
    let distance = gap.abs();
    let fraction = 1.0 - (-since_last.as_secs_f32() * 1000.0 / ease_ms).exp();
    gap.signum() * (distance * fraction).max(1.0).min(distance)
}

/// Whether the glide is over: the end is reached, or the time is up.
pub fn finished(gap: f32, elapsed: Duration) -> bool {
    gap.abs() <= ARRIVED || elapsed >= LIMIT
}

/// Whether `new` is `old` with rows added after its last: a message that
/// arrived, as opposed to history read, an edit, or a pending row swapped
/// for the sent message.
pub fn arrival(old: &[String], new: &[String]) -> bool {
    !old.is_empty() && new.len() > old.len() && new[..old.len()] == *old
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step(gap: f32, since_last: Duration) -> f32 {
        step_with(gap, since_last, EASE_MS)
    }

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

    #[test]
    fn an_arrival_eases_home_in_about_150_ms_at_any_frame_rate() {
        // 60, 144 and 30 frames a second all arrive in the same time.
        for frame in [7, 16, 33] {
            let frame = Duration::from_millis(frame);
            let (mut gap, mut at) = (90.0_f32, Duration::ZERO);
            while gap > ARRIVED {
                gap -= step_with(gap, frame, ARRIVAL_EASE_MS);
                at += frame;
            }
            let ms = at.as_millis();
            assert!((100..=220).contains(&ms), "{frame:?} frames took {ms} ms");
        }
        // Faster than the ease to the newest message.
        assert!(step_with(100.0, FRAME, ARRIVAL_EASE_MS) > step(100.0, FRAME));
    }

    #[test]
    fn a_message_arrives_when_rows_are_added_after_the_last() {
        let rows = |n: &[&str]| n.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let old = rows(&["a", "b"]);
        assert!(arrival(&old, &rows(&["a", "b", "c"])));
        assert!(arrival(&old, &rows(&["a", "b", "c", "d"])));
        // Older history, an edit, a swapped pending row and nothing new.
        assert!(!arrival(&old, &rows(&["z", "a", "b"])));
        assert!(!arrival(&old, &rows(&["a", "b2"])));
        assert!(!arrival(&old, &old));
        assert!(!arrival(&old, &rows(&["a"])));
        // A conversation that had no rows loads, it does not receive.
        assert!(!arrival(&[], &old));
    }
}

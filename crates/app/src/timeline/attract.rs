//! `/attract`: a sparkle on the avatar and a shine across the message, for
//! three seconds, as the web client draws it (`shared/utils/attract.ts`,
//! `presence.css`). This file is the part with no window: which rows play and
//! how far into each pulse the effect is. The view starts and ends it and
//! the message layout draws it.
//!
//! The effect is three pulses of a third of `ATTRACT_MS` each. With reduced
//! motion the pulses are left out and the static marks stay (the tint, the
//! bar and the ring), as the web does.

use std::time::Duration;

use super::rows::Row;

/// How long the effect lasts; the backend coalesces what it receives for as
/// long.
pub const ATTRACT_MS: u64 = 3000;
/// How many pulses the effect makes.
const PULSES: u64 = 3;
/// The time between two redrawn frames: about 30 a second.
pub const FRAME: Duration = Duration::from_millis(33);

/// The rows an attract plays on, by message ID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    /// The row that shines.
    pub row: String,
    /// The row whose avatar sparkles: the head of `row`'s group.
    pub avatar: String,
}

/// The row that shines and the row whose avatar sparkles. A grouped
/// continuation has no avatar, so the sparkle goes to the head of its group.
/// When the exact message is not loaded, `member`'s newest loaded message
/// stands in; with none, nothing plays.
pub fn resolve(rows: &[Row], message: &str, member: &str) -> Option<Target> {
    let found = |matches: &dyn Fn(&super::rows::MessageRow) -> bool| {
        rows.iter()
            .rposition(|r| matches!(r, Row::Message(m) if matches(m)))
    };
    let row = found(&|m| m.id == message).or_else(|| found(&|m| m.sender_id == member))?;
    let mut avatar = row;
    while let Row::Message(m) = &rows[avatar] {
        if m.header {
            break;
        }
        match rows[..avatar]
            .iter()
            .rposition(|r| matches!(r, Row::Message(_)))
        {
            Some(before) => avatar = before,
            None => break,
        }
    }
    let id = |ix: usize| match &rows[ix] {
        Row::Message(m) => m.id.clone(),
        _ => String::new(),
    };
    Some(Target {
        row: id(row),
        avatar: id(avatar),
    })
}

/// What a message row draws of the effect.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fx {
    /// The row shines.
    pub row: bool,
    /// The avatar sparkles.
    pub avatar: bool,
    /// How far into the pulse the effect is, from 0 to 1. `None` with
    /// reduced motion: only the static marks show.
    pub pulse: Option<f32>,
}

/// The effect on the message `id` after `elapsed`, or `None` when it plays
/// on neither its row nor its avatar, or is over.
pub fn fx(target: &Target, id: &str, elapsed: Duration, reduced: bool) -> Option<Fx> {
    let (row, avatar) = (target.row == id, target.avatar == id);
    ((row || avatar) && !over(elapsed)).then(|| Fx {
        row,
        avatar,
        pulse: (!reduced).then(|| pulse(elapsed)),
    })
}

/// Whether the effect has run its course.
pub fn over(elapsed: Duration) -> bool {
    elapsed.as_millis() as u64 >= ATTRACT_MS
}

/// How far into the current pulse `elapsed` is.
pub fn pulse(elapsed: Duration) -> f32 {
    let cycle = (ATTRACT_MS / PULSES) as f32;
    (elapsed.as_millis() as f32 % cycle) / cycle
}

/// The ring around the avatar spreads and fades: `(spread in px, opacity)`.
pub fn ring(pulse: f32) -> (f32, f32) {
    let t = (pulse / 0.7).min(1.);
    (10. * t, 1. - t)
}

/// The twinkles around the avatar: `(opacity, scale)`.
pub fn twinkle(pulse: f32) -> (f32, f32) {
    let opacity = if pulse < 0.3 {
        pulse / 0.3
    } else if pulse < 0.7 {
        1. - (pulse - 0.3) / 0.4
    } else {
        0.
    };
    (opacity, 0.6 + 0.55 * (pulse / 0.7).min(1.))
}

/// The shine across the row: where its centre is, as a share of the row's
/// width (it enters from the left and leaves at the right), and its opacity.
pub fn shine(pulse: f32) -> (f32, f32) {
    let sweep = (pulse / 0.8).min(1.);
    let opacity = if pulse < 0.8 {
        1.
    } else {
        1. - (pulse - 0.8) / 0.2
    };
    (-0.3 + 1.6 * sweep, opacity)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::timeline::rows::MessageRow;
    use std::rc::Rc;

    fn row(id: &str, sender: &str, header: bool) -> Row {
        Row::Message(Rc::new(MessageRow {
            id: id.into(),
            sender_id: sender.into(),
            header,
            ..empty()
        }))
    }

    fn empty() -> MessageRow {
        MessageRow {
            id: String::new(),
            version: 0,
            header: true,
            sender: String::new(),
            sender_id: String::new(),
            sender_color: None,
            time: String::new(),
            action: false,
            notice: false,
            own: false,
            body: Default::default(),
            edited: false,
            deleted: false,
            reply: None,
            reply_id: None,
            mention: false,
            focused: false,
            reactions: Vec::new(),
            attachments: Vec::new(),
            images: Vec::new(),
            links: Vec::new(),
        }
    }

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn the_shine_is_on_the_message_and_the_sparkle_on_the_head_of_its_group() {
        let rows = vec![
            Row::Loading,
            row("a", "m-bob", true),
            row("b", "m-bob", false),
            row("c", "m-bob", false),
            row("d", "m-eve", true),
        ];
        let at = |id: &str| resolve(&rows, id, "m-bob").unwrap();
        assert_eq!((at("c").row.as_str(), at("c").avatar.as_str()), ("c", "a"));
        assert_eq!((at("a").row.as_str(), at("a").avatar.as_str()), ("a", "a"));
        assert_eq!((at("d").row.as_str(), at("d").avatar.as_str()), ("d", "d"));
    }

    #[test]
    fn a_message_that_is_not_loaded_falls_back_to_the_senders_newest() {
        let rows = vec![
            row("a", "m-bob", true),
            row("b", "m-eve", true),
            row("c", "m-bob", true),
        ];
        assert_eq!(resolve(&rows, "gone", "m-bob").unwrap().row, "c");
        assert_eq!(resolve(&rows, "gone", "m-eve").unwrap().row, "b");
        assert_eq!(resolve(&rows, "gone", "m-nobody"), None);
        assert_eq!(resolve(&[], "a", "m-bob"), None);
    }

    #[test]
    fn it_plays_on_two_rows_and_for_three_seconds() {
        let target = Target {
            row: "c".into(),
            avatar: "a".into(),
        };
        let on = |id: &str, at: u64| fx(&target, id, ms(at), false);
        assert_eq!(on("b", 0), None);
        let (row, avatar) = (on("c", 10).unwrap(), on("a", 10).unwrap());
        assert!(row.row && !row.avatar && avatar.avatar && !avatar.row);
        assert!(on("c", 2999).is_some());
        assert_eq!(on("c", 3000), None);
        assert_eq!(on("a", 5000), None);
    }

    #[test]
    fn reduced_motion_keeps_the_marks_and_drops_the_pulses() {
        let target = Target {
            row: "a".into(),
            avatar: "a".into(),
        };
        let still = fx(&target, "a", ms(500), true).unwrap();
        assert!(still.row && still.avatar && still.pulse.is_none());
        assert!(fx(&target, "a", ms(500), false).unwrap().pulse.is_some());
        assert_eq!(fx(&target, "a", ms(3000), true), None);
    }

    #[test]
    fn three_pulses_make_the_effect() {
        assert_eq!(pulse(ms(0)), 0.);
        assert!((pulse(ms(500)) - 0.5).abs() < 1e-6);
        assert!(pulse(ms(1000)) < 1e-6, "the second pulse starts");
        assert!((pulse(ms(2500)) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn each_part_follows_the_webs_keyframes() {
        // The ring starts solid and gone by 70 %.
        assert_eq!(ring(0.), (0., 1.));
        assert_eq!(ring(0.7), (10., 0.));
        assert_eq!(ring(0.9), (10., 0.));
        // The twinkle peaks at 30 % and is gone by 70 %.
        assert_eq!(twinkle(0.).0, 0.);
        assert!((twinkle(0.3).0 - 1.).abs() < 1e-6);
        assert!(twinkle(0.7).0.abs() < 1e-6);
        assert!((twinkle(0.).1 - 0.6).abs() < 1e-6 && (twinkle(0.7).1 - 1.15).abs() < 1e-6);
        // The shine crosses the row by 80 %, then fades.
        assert_eq!(shine(0.), (-0.3, 1.));
        let (centre, opacity) = shine(0.8);
        assert!((centre - 1.3).abs() < 1e-6 && opacity == 1.);
        assert!(shine(0.99).1 < 0.1);
    }
}

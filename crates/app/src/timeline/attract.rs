//! `/attract`: for three seconds a comet laps the message's row, sheds
//! sparks that fall over the rows below, and lands on its sender's avatar
//! with a burst (`comet`). This file says which rows play and keeps where
//! they were painted; the view starts and ends the effect.
//!
//! With reduced motion nothing moves: the row keeps a tint with a bar on
//! the left and the avatar a ring, as the web client does.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::{Bounds, Pixels};

use super::rows::Row;

/// How long the effect lasts; the backend coalesces what it receives for as
/// long.
pub const ATTRACT_MS: u64 = 3000;
/// The time between two frames the view asks for itself: about 30 a second.
/// While the comet moves the window also draws at the display's own rate.
pub const FRAME: Duration = Duration::from_millis(33);

/// The rows an attract plays on, by message ID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    /// The row the comet laps.
    pub row: String,
    /// The row whose avatar the comet leaves and lands on: the head of
    /// `row`'s group.
    pub avatar: String,
}

/// The row the comet laps and the row whose avatar it lands on. A grouped
/// continuation has no avatar, so the comet lands on the head of its group.
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

/// Where the rows and avatars an attract plays on were painted in this
/// frame, by message ID. Each row and avatar that plays reports its place
/// while it is painted (`message::attract`), and the comet layer over the
/// timeline, painted after the rows, reads them. One per timeline, so two
/// windows never mix their places.
#[derive(Clone, Default)]
pub struct Stage(Rc<RefCell<Places>>);

/// What a [`Stage`] holds for one frame.
#[derive(Debug, Default)]
pub struct Places {
    pub rows: HashMap<String, Bounds<Pixels>>,
    pub avatars: HashMap<String, Bounds<Pixels>>,
}

impl Stage {
    /// The row of message `id` was painted at `bounds`.
    pub fn row(&self, id: &str, bounds: Bounds<Pixels>) {
        self.0.borrow_mut().rows.insert(id.to_owned(), bounds);
    }

    /// The avatar on message `id` was painted at `bounds`.
    pub fn avatar(&self, id: &str, bounds: Bounds<Pixels>) {
        self.0.borrow_mut().avatars.insert(id.to_owned(), bounds);
    }

    /// The places of this frame, leaving the stage empty for the next.
    pub fn take(&self) -> Places {
        std::mem::take(&mut *self.0.borrow_mut())
    }
}

impl std::fmt::Debug for Stage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Stage")
    }
}

impl PartialEq for Stage {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}

/// What a message row draws of the effect.
#[derive(Debug, Clone, PartialEq)]
pub struct Fx {
    /// The comet laps this row.
    pub row: bool,
    /// The comet takes off from and lands on this avatar.
    pub avatar: bool,
    /// Where the row reports its place, while the comet moves. `None` with
    /// reduced motion: the row and avatar show the static marks instead.
    pub stage: Option<Stage>,
    /// An attract that was replaced and is letting its sparks land: the
    /// row only reports its place.
    pub settling: bool,
}

/// The effect on the message `id` after `elapsed`, or `None` when it plays
/// on neither its row nor its avatar, or is over. `stage` is `None` with
/// reduced motion.
pub fn fx(target: &Target, id: &str, elapsed: Duration, stage: Option<&Stage>) -> Option<Fx> {
    let (row, avatar) = (target.row == id, target.avatar == id);
    ((row || avatar) && !over(elapsed)).then(|| Fx {
        row,
        avatar,
        stage: stage.cloned(),
        settling: false,
    })
}

/// Whether the effect has run its course.
pub fn over(elapsed: Duration) -> bool {
    elapsed.as_millis() as u64 >= ATTRACT_MS
}

/// Seconds into the effect.
pub fn seconds(elapsed: Duration) -> f32 {
    elapsed.as_secs_f32()
}

/// Fades the whole effect in over its first 100 ms and out over its last
/// 350 ms, so nothing pops.
pub fn envelope(t: f32) -> f32 {
    let end = ATTRACT_MS as f32 / 1000.;
    (t / 0.1).clamp(0., 1.) * ((end - t) / 0.35).clamp(0., 1.)
}

/// A number from 0 to 1 for particle `n` of stream `salt`: the same every
/// frame, so particles keep no state and a frame never depends on the one
/// before.
pub fn noise(n: u32, salt: u32) -> f32 {
    let mut x = n.wrapping_mul(0x9E37_79B9) ^ salt.wrapping_mul(0x85EB_CA6B);
    x ^= x >> 15;
    x = x.wrapping_mul(0x2C1B_3C6D);
    x ^= x >> 12;
    x = x.wrapping_mul(0x297A_2D39);
    x ^= x >> 15;
    (x & 0xFFFF) as f32 / 65535.
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::timeline::rows::MessageRow;
    use gpui_kit::px;
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
            copy: Default::default(),
        }
    }

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn the_comet_laps_the_message_and_lands_on_the_head_of_its_group() {
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
        let stage = Stage::default();
        let on = |id: &str, at: u64| fx(&target, id, ms(at), Some(&stage));
        assert_eq!(on("b", 0), None);
        let (row, avatar) = (on("c", 10).unwrap(), on("a", 10).unwrap());
        assert!(row.row && !row.avatar && avatar.avatar && !avatar.row);
        assert!(on("c", 2999).is_some());
        assert_eq!(on("c", 3000), None);
        assert_eq!(on("a", 5000), None);
    }

    #[test]
    fn reduced_motion_keeps_the_marks_and_has_no_stage() {
        let target = Target {
            row: "a".into(),
            avatar: "a".into(),
        };
        let still = fx(&target, "a", ms(500), None).unwrap();
        assert!(still.row && still.avatar && still.stage.is_none());
        assert!(
            fx(&target, "a", ms(500), Some(&Stage::default()))
                .unwrap()
                .stage
                .is_some()
        );
        assert_eq!(fx(&target, "a", ms(3000), None), None);
    }

    #[test]
    fn the_stage_holds_one_frame() {
        let stage = Stage::default();
        let at = Bounds::new(
            gpui_kit::point(px(1.), px(2.)),
            gpui_kit::size(px(3.), px(4.)),
        );
        stage.row("a", at);
        stage.avatar("b", at);
        let places = stage.take();
        assert_eq!((places.rows["a"], places.avatars["b"]), (at, at));
        assert!(stage.take().rows.is_empty(), "the next frame starts empty");
        assert_eq!(stage.clone(), stage);
        assert_ne!(Stage::default(), stage);
    }

    #[test]
    fn it_fades_in_and_out() {
        assert_eq!(envelope(0.), 0.);
        assert!((envelope(0.05) - 0.5).abs() < 1e-6);
        assert_eq!(envelope(1.5), 1.);
        assert!(envelope(2.9) < 1. && envelope(2.9) > 0.);
        assert_eq!(envelope(3.), 0.);
    }

    #[test]
    fn noise_is_the_same_every_frame_and_spread() {
        assert_eq!(noise(7, 3), noise(7, 3));
        let all: Vec<f32> = (0..1000).map(|n| noise(n, 9)).collect();
        assert!(all.iter().all(|v| (0. ..=1.).contains(v)));
        let mean = all.iter().sum::<f32>() / all.len() as f32;
        assert!((mean - 0.5).abs() < 0.05, "mean {mean}");
        assert!(all.iter().any(|v| *v < 0.1) && all.iter().any(|v| *v > 0.9));
    }
}

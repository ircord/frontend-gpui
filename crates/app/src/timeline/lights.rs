//! Two small lights from the comet's kit, played only for news: sparks that
//! leave a reaction chip when a reaction is added to a message on screen, and
//! one glint that crosses the row of a message that mentions you when it
//! arrives. Loaded history and a switch to another conversation never play,
//! as with the screen reader's announcements (`announce`).
//!
//! The motion is a function of the time alone ([`spark_shapes`],
//! [`glint_shapes`]), drawn with the comet's shapes by [`layer`], a canvas
//! over the timeline that paints after the rows from the places the chip or
//! row reported while it was painted ([`Stage`]). With reduced motion nothing
//! plays and the row looks as it does without this.

use std::cell::RefCell;
use std::collections::HashMap;
use std::f32::consts::TAU;
use std::rc::Rc;

use gpui_kit::prelude::*;
use gpui_kit::*;

use super::attract::noise;
use super::comet::{Palette, Shape, Shapes, ease_out, f, paint, rect};
use super::rows::Row;

/// How long a reaction's sparks last, in seconds.
pub const SPARK_SECS: f32 = 0.4;
/// How long the glint takes to cross a row.
pub const GLINT_SECS: f32 = 0.6;
/// How long the chip's border flashes.
const FLASH_SECS: f32 = 0.25;
/// The sparks of one reaction.
const SPARKS: u32 = 10;
/// A spark's fall, in px per second squared.
const GRAVITY: f32 = 280.;
/// The widest the glint's band is, in px.
const GLINT_WIDTH: f32 = 140.;
/// How long the sidebar's mention badge takes to send out its ring.
pub const RING_SECS: f32 = 0.5;
/// How far the badge's ring spreads, in px.
const RING_REACH: f32 = 12.;
/// The most lights at once; a flood of news plays the newest.
pub const MOST: usize = 12;

/// Which light plays.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Kind {
    /// Sparks from the chip of `emoji` under message `message`.
    Sparks { message: String, emoji: String },
    /// A glint across the row of message `message`.
    Glint { message: String },
}

impl Kind {
    /// How long it plays, in seconds.
    pub fn secs(&self) -> f32 {
        match self {
            Kind::Sparks { .. } => SPARK_SECS,
            Kind::Glint { .. } => GLINT_SECS,
        }
    }

    /// The key a place is reported under.
    pub fn place(&self) -> String {
        match self {
            Kind::Sparks { message, emoji } => chip_key(message, emoji),
            Kind::Glint { message } => row_key(message),
        }
    }
}

pub fn row_key(message: &str) -> String {
    format!("row\0{message}")
}

pub fn chip_key(message: &str, emoji: &str) -> String {
    format!("chip\0{message}\0{emoji}")
}

/// The ring a mention badge sends out when its count rises, `t` seconds in:
/// how far it has spread past the badge and how bright it still is. `None`
/// before it starts and once it is over.
pub fn badge_ring(t: f32) -> Option<(f32, f32)> {
    (0. ..RING_SECS)
        .contains(&t)
        .then(|| (RING_REACH * ease_out(t / RING_SECS), 1. - t / RING_SECS))
}

/// Whether the light `kind`, started `elapsed` ago, has played out.
pub fn over(kind: &Kind, elapsed: std::time::Duration) -> bool {
    elapsed.as_secs_f32() >= kind.secs()
}

/// What changed between two frames of rows that is news: a reaction added to
/// a message that was already on screen, and a message that mentions you
/// after the newest one that was. Nothing when the old rows hold no message
/// (the history is still loading) or the new rows do not continue them
/// (older history read in front).
pub fn news(old: &[Row], new: &[Row]) -> Vec<Kind> {
    let before: HashMap<&str, &super::rows::MessageRow> = old
        .iter()
        .filter_map(|r| match r {
            Row::Message(m) => Some((m.id.as_str(), &**m)),
            _ => None,
        })
        .collect();
    if before.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    // Messages are news when they come after the first one already known.
    let mut after_known = false;
    for row in new {
        let Row::Message(m) = row else { continue };
        match before.get(m.id.as_str()) {
            Some(was) => {
                after_known = true;
                for (emoji, count) in &m.reactions {
                    let had = was
                        .reactions
                        .iter()
                        .find(|(e, _)| e == emoji)
                        .map_or(0, |(_, n)| *n);
                    if *count > had && !m.deleted {
                        out.push(Kind::Sparks {
                            message: m.id.clone(),
                            emoji: emoji.clone(),
                        });
                    }
                }
            }
            None => {
                if after_known && m.mention {
                    out.push(Kind::Glint {
                        message: m.id.clone(),
                    });
                }
            }
        }
    }
    out
}

/// The places chips and rows were painted at in this frame, by key
/// ([`Kind::place`]). One per timeline, so two windows never mix theirs.
#[derive(Clone, Default)]
pub struct Stage(Rc<RefCell<HashMap<String, Bounds<Pixels>>>>);

impl Stage {
    pub fn report(&self, key: &str, bounds: Bounds<Pixels>) {
        self.0.borrow_mut().insert(key.to_owned(), bounds);
    }

    /// The places of this frame, leaving the stage empty for the next.
    pub fn take(&self) -> HashMap<String, Bounds<Pixels>> {
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

/// What a message row draws of the lights playing on it: where to report
/// its place, for the row and for the chips that spark.
#[derive(Debug, Clone, PartialEq)]
pub struct Fx {
    pub stage: Stage,
    /// The row reports its place: a glint crosses it.
    pub row: bool,
    /// The emoji of the chips that report theirs.
    pub chips: Vec<String>,
}

/// A layer the size of its parent that draws nothing and reports where it
/// was painted.
pub fn probe(stage: Stage, key: String) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |bounds, _, _, _| stage.report(&key, bounds),
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

/// The sparks of a reaction at `t` seconds on the chip painted at `chip`: the
/// chip's border flashes the accent, and about ten sparks leave it and fall.
/// The landing burst of the comet at a third of its size.
pub fn spark_shapes(chip: Bounds<Pixels>, t: f32, pal: &Palette) -> Vec<Shape> {
    let mut out = Shapes::default();
    if !(0. ..SPARK_SECS).contains(&t) {
        return out.0;
    }
    let (cx, cy) = (f(chip.center().x), f(chip.center().y));
    let flash = (1. - t / FLASH_SECS).max(0.);
    out.outline(chip, 6., 1.5, pal.accent.opacity(0.9 * flash));
    out.fill(chip, 6., pal.accent.opacity(0.18 * flash));
    for n in 0..SPARKS {
        let life = 0.25 + 0.15 * noise(n, 31);
        if t > life {
            continue;
        }
        let angle = TAU * (n as f32 + noise(n, 32)) / SPARKS as f32;
        let fly = 28. + 50. * noise(n, 33);
        let (dx, dy) = (angle.cos(), angle.sin());
        // They leave from the chip's edge, not its middle.
        let edge = f(chip.size.height) / 2.;
        let (sx, sy) = (
            cx + dx * edge + dx * fly * t,
            cy + dy * edge + dy * fly * t + 0.5 * GRAVITY * t * t,
        );
        let left = 1. - t / life;
        let color = if n % 3 == 0 { pal.gold } else { pal.hot };
        out.dot(sx, sy, 0.9 + 1.2 * left, color.opacity(left));
    }
    out.0
}

/// The glint at `t` seconds on the row painted at `row`: one soft band of
/// light that crosses it once, over the tint it already has. The caller
/// clips it to the row.
pub fn glint_shapes(row: Bounds<Pixels>, t: f32, pal: &Palette) -> Vec<Shape> {
    let mut out = Shapes::default();
    if !(0. ..GLINT_SECS).contains(&t) {
        return out.0;
    }
    let q = t / GLINT_SECS;
    let reach = f(row.size.width) + 2. * GLINT_WIDTH;
    let x = f(row.left()) - GLINT_WIDTH + reach * ease_out(q);
    // Fades in at the left edge and out at the right, so nothing pops.
    let strength = (q / 0.15).min(1.) * ((1. - q) / 0.25).min(1.);
    let (top, height) = (f(row.top()), f(row.size.height));
    let half = GLINT_WIDTH / 2.;
    let clear = pal.hot.opacity(0.);
    let lit = pal.hot.opacity(0.32 * strength);
    out.streak(rect(x - half, top, half, height), 90., clear, lit);
    out.streak(rect(x, top, half, height), 90., lit, clear);
    out.0
}

/// The shapes of one light at `t` seconds, from where it was painted.
pub fn shapes(kind: &Kind, place: Bounds<Pixels>, t: f32, pal: &Palette) -> Vec<Shape> {
    match kind {
        Kind::Sparks { .. } => spark_shapes(place, t, pal),
        Kind::Glint { .. } => glint_shapes(place, t, pal),
    }
}

/// The canvas over the timeline that paints `lights` (each with the seconds
/// it has played) from the places `stage` was told. A light whose chip or row
/// is out of view draws nothing. The layer takes no clicks; `bottom` leaves
/// the typing line under the list alone.
pub fn layer(stage: Stage, lights: Vec<(Kind, f32)>, pal: Palette, bottom: Pixels) -> AnyElement {
    div()
        .id("timeline-lights")
        .test_support()
        .absolute()
        .top_0()
        .left_0()
        .right_0()
        .bottom(bottom)
        .child(
            canvas(
                |_, _, _| (),
                move |bounds, _, window, _| {
                    let places = stage.take();
                    window.with_content_mask(Some(ContentMask { bounds }), |window| {
                        for (kind, t) in &lights {
                            let Some(place) = places.get(&kind.place()) else {
                                continue;
                            };
                            // A glint stays on its row.
                            let clip = match kind {
                                Kind::Glint { .. } => *place,
                                Kind::Sparks { .. } => bounds,
                            };
                            window.with_content_mask(
                                Some(ContentMask { bounds: clip }),
                                |window| {
                                    for shape in shapes(kind, *place, *t, &pal) {
                                        paint(&shape, window);
                                    }
                                },
                            );
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
        GLINT_SECS, Kind, RING_SECS, SPARK_SECS, SPARKS, Stage, badge_ring, chip_key, glint_shapes,
        news, row_key, spark_shapes,
    };
    use crate::theme::Theme;
    use crate::timeline::comet::{Palette, Shape, f, rect};
    use crate::timeline::rows::{MessageRow, Row};
    use gpui_kit::{Bounds, Pixels};
    use std::rc::Rc;

    fn pal() -> Palette {
        Palette::new(&Theme::dark())
    }

    fn chip() -> Bounds<Pixels> {
        rect(100., 200., 44., 22.)
    }

    fn row() -> Bounds<Pixels> {
        rect(0., 100., 800., 40.)
    }

    fn message(id: &str, mention: bool, reactions: &[(&str, usize)]) -> Row {
        Row::Message(Rc::new(MessageRow {
            id: id.into(),
            version: 0,
            header: true,
            sender: String::new(),
            sender_id: "m-bob".into(),
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
            mention,
            focused: false,
            reactions: reactions.iter().map(|(e, n)| (e.to_string(), *n)).collect(),
            attachments: Vec::new(),
            images: Vec::new(),
            links: Vec::new(),
            copy: Default::default(),
        }))
    }

    fn sparks(message: &str, emoji: &str) -> Kind {
        Kind::Sparks {
            message: message.into(),
            emoji: emoji.into(),
        }
    }

    fn glint(message: &str) -> Kind {
        Kind::Glint {
            message: message.into(),
        }
    }

    #[test]
    fn a_new_reaction_on_a_message_on_screen_sparks() {
        let old = [message("a", false, &[("👍", 1)]), message("b", false, &[])];
        let new = [
            message("a", false, &[("👍", 2), ("🎉", 1)]),
            message("b", false, &[]),
        ];
        assert_eq!(news(&old, &new), vec![sparks("a", "👍"), sparks("a", "🎉")]);
    }

    #[test]
    fn a_reaction_taken_back_or_unchanged_does_not() {
        let old = [message("a", false, &[("👍", 2)])];
        assert!(news(&old, &old).is_empty());
        assert!(news(&old, &[message("a", false, &[("👍", 1)])]).is_empty());
        assert!(news(&old, &[message("a", false, &[])]).is_empty());
    }

    #[test]
    fn a_mention_that_arrives_glints_but_one_in_loaded_history_does_not() {
        let old = [message("b", false, &[]), message("c", false, &[])];
        // Newer after the newest known: news.
        let new = [
            message("b", false, &[]),
            message("c", false, &[]),
            message("d", true, &[]),
            message("e", false, &[]),
        ];
        assert_eq!(news(&old, &new), vec![glint("d")]);
        // Older history read in front of the known messages: not news.
        let older = [
            message("a", true, &[]),
            message("b", false, &[]),
            message("c", false, &[]),
        ];
        assert!(news(&old, &older).is_empty());
    }

    #[test]
    fn loading_the_first_page_is_not_news() {
        let new = [message("a", true, &[("👍", 3)])];
        assert!(news(&[], &new).is_empty());
        assert!(news(&[Row::Loading], &new).is_empty());
    }

    #[test]
    fn the_sparks_leave_the_chip_and_fall_for_four_tenths_of_a_second() {
        let at = |t| spark_shapes(chip(), t, &pal());
        let dots = |t| {
            at(t)
                .iter()
                .filter(|s| matches!(s, Shape::Fill { at, .. } if f(at.size.width) < 8.))
                .count()
        };
        assert!(dots(0.02) >= SPARKS as usize - 1, "about ten sparks leave");
        assert!(dots(0.3) < dots(0.02), "they go out as they fall");
        assert!(at(SPARK_SECS).is_empty() && at(1.).is_empty());
        // They fall: later sparks sit lower than they would have flown.
        let lowest = |t| {
            at(t)
                .iter()
                .filter_map(|s| match s {
                    Shape::Fill { at, .. } if f(at.size.width) < 8. => Some(f(at.center().y)),
                    _ => None,
                })
                .fold(f32::MIN, f32::max)
        };
        assert!(lowest(0.2) > f(chip().center().y) + 11.);
    }

    #[test]
    fn the_chip_border_flashes_then_goes() {
        let outline = |t| {
            spark_shapes(chip(), t, &pal())
                .into_iter()
                .find_map(|s| match s {
                    Shape::Outline { color, .. } => Some(color.a),
                    _ => None,
                })
        };
        assert!(outline(0.01).unwrap() > 0.8);
        assert!(outline(0.2).unwrap() < outline(0.05).unwrap());
        assert_eq!(outline(0.3), None);
    }

    #[test]
    fn the_glint_crosses_the_row_once() {
        let band = |t| {
            glint_shapes(row(), t, &pal())
                .iter()
                .map(|s| match s {
                    Shape::Streak { at, .. } => f(at.center().x),
                    _ => panic!("only the band"),
                })
                .sum::<f32>()
                / 2.
        };
        assert!(band(0.01) < 100., "it starts at the left edge");
        assert!(band(0.3) > band(0.1) && band(0.5) > band(0.3));
        assert!(band(0.59) > 700., "and leaves at the right");
        assert!(glint_shapes(row(), GLINT_SECS, &pal()).is_empty());
    }

    #[test]
    fn nothing_pops_in_or_out() {
        let alpha = |t| {
            glint_shapes(row(), t, &pal())
                .iter()
                .map(|s| match s {
                    Shape::Streak { from, to, .. } => from.a.max(to.a),
                    _ => 0.,
                })
                .fold(0., f32::max)
        };
        assert!(alpha(0.) < 0.01 && alpha(0.599) < 0.02);
        assert!((alpha(0.3) - 0.32).abs() < 0.01, "it is faint at its best");
    }

    #[test]
    fn a_frame_depends_only_on_its_time() {
        assert_eq!(
            spark_shapes(chip(), 0.17, &pal()),
            spark_shapes(chip(), 0.17, &pal())
        );
        assert_eq!(
            glint_shapes(row(), 0.31, &pal()),
            glint_shapes(row(), 0.31, &pal())
        );
    }

    #[test]
    fn the_badge_ring_spreads_and_fades_in_half_a_second() {
        let (near, bright) = badge_ring(0.).unwrap();
        assert_eq!((near, bright), (0., 1.));
        let (far, faint) = badge_ring(0.45).unwrap();
        assert!(far > 10. && far <= 12. && faint < 0.15);
        assert!(badge_ring(0.2).unwrap().0 < far);
        assert_eq!(badge_ring(RING_SECS), None);
        assert_eq!(badge_ring(-0.1), None);
    }

    #[test]
    fn places_are_reported_per_frame() {
        let stage = Stage::default();
        stage.report(&chip_key("a", "👍"), chip());
        stage.report(&row_key("a"), row());
        let places = stage.take();
        assert_eq!(places[&sparks("a", "👍").place()], chip());
        assert_eq!(places[&glint("a").place()], row());
        assert!(stage.take().is_empty());
    }
}

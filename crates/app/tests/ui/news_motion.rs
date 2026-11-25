//! News that moves: a message that arrives glides into the conversation
//! instead of moving every row at once, a reaction added to a message on
//! screen sends sparks from its chip, and a message that mentions you gets
//! one glint across its row. All of it starts from the event, ends by
//! itself, plays only for news (never for loaded history or rows out of view) and gives way to
//! today's plain behaviour with reduced motion.

use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui_kit::test::TestWindowExt;
use gpui_kit::{AppContext, TestAppContext};
use ircord_app::session::SessionEvent;
use ircord_app::timeline::{self, lights::Kind};
use ircord_client::contract::*;
use ircord_client::testing::{CannedServer, Reply, frame};
use serde_json::json;

use crate::fixture::{Fixture, HANG_GUARD, bootstrap, message, open_with, wait};

fn numbered(i: u32) -> Message {
    let mut m = message(&format!("m{i}"), "c-general", &format!("message {i}"));
    m.created_at = format!("2026-11-{:02}T10:{:02}:00Z", 1 + i / 60, i % 60);
    m
}

/// `#general` holds m1..m100: a conversation taller than its view, so a new
/// message has somewhere to glide from.
fn serve_long_history(server: &CannedServer) {
    server.route_fn("GET", "/api/v1/conversations/*/messages", |r| {
        if !r.path.ends_with("/c-general/messages") {
            return Reply::json(200, json!({"items": [], "nextCursor": null}));
        }
        let page = MessagePage {
            items: (1..=100).map(numbered).collect(),
            ..Default::default()
        };
        Reply::json(200, serde_json::to_value(page).unwrap())
    });
}

fn reduce_motion(cx: &mut TestAppContext) {
    cx.update(|cx| {
        let mut theme = ircord_app::theme::get(cx).clone();
        theme.reduced_motion = true;
        ircord_app::theme::set(theme, cx);
    });
}

fn has_message(f: &Fixture, cx: &mut gpui_kit::App, id: &str) -> bool {
    f.session.read(cx).client().is_some_and(|c| {
        c.read()
            .timelines
            .get("c-general")
            .is_some_and(|t| t.find(id).is_some())
    })
}

/// Draw a frame and say where row `id` is: its top, and how far its bottom
/// is above the bottom of the list (the typing line sits under it).
fn place(f: &Fixture, cx: &mut TestAppContext, id: &'static str) -> Option<(f32, f32)> {
    cx.run_until_parked();
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        let row = window.try_find(id)?;
        let list = window.find("timeline").bounds();
        let bounds = row.bounds();
        Some((
            f32::from(bounds.top()),
            f32::from(list.bottom() - bounds.bottom()) - 20.,
        ))
    })
    .unwrap()
}

fn open_long(cx: &mut TestAppContext) -> Fixture {
    let f = open_with(cx, true, serve_long_history);
    wait(cx, "the whole history", |cx| has_message(&f, cx, "m100"));
    cx.run_until_parked();
    cx.update_window(f.window, |_, window, cx| window.render_frame(cx))
        .unwrap();
    f
}

fn arrive(f: &Fixture, seq: i64, id: &str, text: &str) {
    let mut m = message(id, "c-general", text);
    m.created_at = "2026-11-21T10:00:00Z".into();
    f.server.push(frame(seq, "message.created", &m));
}

fn gliding(cx: &mut TestAppContext) -> Option<(bool, f32)> {
    cx.update(|cx| timeline::glide(cx))
}

/// Sample m100's top frame after frame until the glide has ended.
fn rise_until_home(f: &Fixture, cx: &mut TestAppContext, from: f32) -> Vec<f32> {
    let deadline = Instant::now() + HANG_GUARD;
    let mut tops = vec![from];
    loop {
        if let Some((top, _)) = place(f, cx, "message-m100") {
            tops.push(top);
        }
        if gliding(cx).is_none() {
            return tops;
        }
        assert!(Instant::now() < deadline, "the glide did not end");
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[gpui_kit::test]
fn a_message_that_arrives_glides_in_and_ends_at_the_newest(cx: &mut TestAppContext) {
    let f = open_long(cx);
    // At the newest message: m100 sits against the bottom, nothing glides.
    let (_, rest) = place(&f, cx, "message-m100").expect("m100 is drawn");
    assert!(rest.abs() < 2., "m100 starts at the bottom, {rest}");
    assert_eq!(gliding(cx), None);
    let before = place(&f, cx, "message-m100").unwrap().0;

    // The glide runs on the wall clock and is over about 150 ms after the
    // message lands, and a loaded machine can spend longer than that inside
    // one `run_until_parked`. So the state is read at the event, not after a
    // wait: this subscriber is called right after the timeline's own, which
    // starts the glide from the same event.
    let began = Rc::new(Cell::new(None));
    let spy = began.clone();
    let session = f.session.clone();
    cx.update(|cx| {
        cx.subscribe(&session, move |session, event: &SessionEvent, cx| {
            let arrived = matches!(event, SessionEvent::Changed(_))
                && session.read(cx).client().is_some_and(|c| {
                    c.read()
                        .timelines
                        .get("c-general")
                        .is_some_and(|t| t.find("m101").is_some())
                });
            if arrived && spy.get().is_none() {
                spy.set(Some(timeline::glide(cx)));
            }
        })
        .detach();
    });
    arrive(&f, 4, "m101", "a new message that is long enough to see");
    wait(cx, "the new message", |cx| has_message(&f, cx, "m101"));
    // The view holds its place and glides: the state says so. Today every
    // row would jump at once.
    let (arrival, gap) = began.get().flatten().expect("the arrival glides");
    assert!(arrival && gap >= 0., "an arrival glide: {arrival} {gap}");
    // The rows only rise, and end one row higher with it home.
    let tops = rise_until_home(&f, cx, before);
    assert!(
        tops.windows(2).all(|w| w[1] <= w[0] + 0.5),
        "the rows only rise: {tops:?}"
    );
    let (_, rest) = place(&f, cx, "message-m101").expect("m101 is drawn");
    assert!(rest.abs() < 2., "m101 ends at the bottom, {rest}");
    let height = before - place(&f, cx, "message-m100").unwrap().0;
    assert!(height > 20., "the new row is {height} px tall");
    // It ended following the end: another message does the same.
    arrive(&f, 5, "m102", "and another");
    wait(cx, "the second message", |cx| has_message(&f, cx, "m102"));
    let deadline = Instant::now() + HANG_GUARD;
    while !place(&f, cx, "message-m102").is_some_and(|(_, rest)| rest.abs() < 2.) {
        assert!(Instant::now() < deadline, "the second glide did not end");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[gpui_kit::test]
fn a_second_message_during_the_glide_extends_it_and_never_throws_the_view_up(
    cx: &mut TestAppContext,
) {
    let f = open_long(cx);
    let before = place(&f, cx, "message-m100").unwrap().0;
    arrive(&f, 4, "m101", "a new message that is long enough to see");
    wait(cx, "the first message", |cx| has_message(&f, cx, "m101"));
    // Another one while the first is still on its way (a machine too busy
    // to get here in time only makes this the plain case).
    arrive(&f, 5, "m102", "a second one right behind it");
    wait(cx, "the second message", |cx| has_message(&f, cx, "m102"));
    let tops = rise_until_home(&f, cx, before);
    // The rows never go down (the hold was applied twice and threw the view
    // a viewport up before), and both rows end up in place.
    assert!(
        tops.windows(2).all(|w| w[1] <= w[0] + 0.5),
        "the rows only rise: {tops:?}"
    );
    let (_, rest) = place(&f, cx, "message-m102").expect("m102 is drawn");
    assert!(rest.abs() < 2., "m102 ends at the bottom, {rest}");
    let (_, rest) = place(&f, cx, "message-m101").expect("m101 is drawn");
    assert!(rest > 10., "m101 is above m102, {rest}");
    let end = place(&f, cx, "message-m100").unwrap().0;
    assert!(before - end > 30., "both rows' height: {before} to {end}");
}

#[gpui_kit::test]
fn with_reduced_motion_a_message_takes_its_place_at_once(cx: &mut TestAppContext) {
    let f = open_long(cx);
    reduce_motion(cx);
    arrive(&f, 4, "m101", "a new message that is long enough to see");
    wait(cx, "the new message", |cx| has_message(&f, cx, "m101"));
    // Today's behaviour: the list follows the end in the first frame.
    place(&f, cx, "message-m101");
    let (_, rest) = place(&f, cx, "message-m101").expect("the new row is drawn");
    assert!(rest.abs() < 2., "at the bottom at once, {rest}");
}

#[gpui_kit::test]
fn scrolled_up_a_message_does_not_pull_the_view_down(cx: &mut TestAppContext) {
    let f = open_long(cx);
    cx.update_window(f.window, |_, window, cx| {
        let up =
            gpui_kit::ScrollDelta::Pixels(gpui_kit::point(gpui_kit::px(0.), gpui_kit::px(600.)));
        window.scroll("timeline", up, cx);
        window.render_frame(cx);
    })
    .unwrap();
    cx.run_until_parked();
    let before = place(&f, cx, "message-m90").map(|p| p.0);
    arrive(&f, 4, "m101", "new while reading older");
    wait(cx, "the new message", |cx| has_message(&f, cx, "m101"));
    std::thread::sleep(Duration::from_millis(300));
    let after = place(&f, cx, "message-m90").map(|p| p.0);
    assert_eq!(before, after, "the reader stays where they are");
}

/// Draw a frame and return the lights it drew.
fn lit(f: &Fixture, cx: &mut TestAppContext) -> Vec<(Kind, f32)> {
    cx.run_until_parked();
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        timeline::lit(cx)
    })
    .unwrap()
}

fn drawn(f: &Fixture, cx: &mut TestAppContext, id: &'static str) -> bool {
    cx.update_window(f.window, |_, window, _| window.try_find(id).is_some())
        .unwrap()
}

fn wait_until_lit(f: &Fixture, cx: &mut TestAppContext, shown: bool) {
    let deadline = Instant::now() + HANG_GUARD;
    while lit(f, cx).is_empty() == shown {
        assert!(
            Instant::now() < deadline,
            "the lights were never {}",
            if shown { "lit" } else { "out" }
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn react(f: &Fixture, seq: i64, emoji: &str, members: &[&str]) {
    let mut m = message("2", "c-general", "second line");
    m.reactions = vec![Reaction {
        emoji: emoji.into(),
        member_ids: members.iter().map(|m| m.to_string()).collect(),
    }];
    f.server.push(frame(seq, "message.updated", &m));
}

fn open_short(cx: &mut TestAppContext) -> Fixture {
    let f = crate::fixture::open(cx, true);
    bootstrap(&f, cx);
    f
}

#[gpui_kit::test]
fn history_lights_nothing(cx: &mut TestAppContext) {
    let f = open_short(cx);
    assert!(lit(&f, cx).is_empty());
    assert!(!drawn(&f, cx, "timeline-lights"));
}

#[gpui_kit::test]
fn a_reaction_sparks_from_its_chip_and_the_sparks_end(cx: &mut TestAppContext) {
    let f = open_short(cx);
    react(&f, 4, "👍", &["m-eve"]);
    wait_until_lit(&f, cx, true);
    let lights = lit(&f, cx);
    assert_eq!(
        lights.iter().map(|(k, _)| k.clone()).collect::<Vec<_>>(),
        vec![Kind::Sparks {
            message: "2".into(),
            emoji: "👍".into()
        }]
    );
    assert!(
        drawn(&f, cx, "timeline-lights"),
        "the layer is over the rows"
    );
    assert!(lights[0].1 < 0.4);
    // A second person adding the same reaction sparks again.
    react(&f, 5, "👍", &["m-eve", "m-bob"]);
    wait(cx, "the second reaction", |cx| {
        f.session.read(cx).client().is_some_and(|c| {
            c.read()
                .message("c-general", "2")
                .is_some_and(|m| m.reactions[0].member_ids.len() == 2)
        })
    });
    // The sparks end by themselves, and the layer goes with them.
    wait_until_lit(&f, cx, false);
    assert!(!drawn(&f, cx, "timeline-lights"));
}

#[gpui_kit::test]
fn a_reaction_taken_back_sparks_nothing(cx: &mut TestAppContext) {
    let f = open_short(cx);
    react(&f, 4, "👍", &["m-eve", "m-bob"]);
    wait_until_lit(&f, cx, true);
    wait_until_lit(&f, cx, false);
    react(&f, 5, "👍", &["m-eve"]);
    wait(cx, "the reaction taken back", |cx| {
        f.session.read(cx).client().is_some_and(|c| {
            c.read()
                .message("c-general", "2")
                .is_some_and(|m| m.reactions[0].member_ids.len() == 1)
        })
    });
    assert!(lit(&f, cx).is_empty());
}

#[gpui_kit::test]
fn a_mention_that_arrives_glints_across_its_row_once(cx: &mut TestAppContext) {
    let f = open_short(cx);
    arrive(&f, 4, "3", "hey @me, are you there?");
    wait_until_lit(&f, cx, true);
    let lights = lit(&f, cx);
    assert!(
        lights.iter().any(|(k, _)| *k
            == Kind::Glint {
                message: "3".into()
            }),
        "{lights:?}"
    );
    assert!(drawn(&f, cx, "timeline-lights"));
    wait_until_lit(&f, cx, false);
    // A message that does not mention you has none.
    arrive(&f, 5, "4", "nothing to see");
    wait(cx, "the plain message", |cx| has_message(&f, cx, "4"));
    assert!(lit(&f, cx).is_empty());
}

#[gpui_kit::test]
fn with_reduced_motion_nothing_plays(cx: &mut TestAppContext) {
    let f = open_short(cx);
    reduce_motion(cx);
    react(&f, 4, "👍", &["m-eve"]);
    arrive(&f, 5, "3", "hey @me");
    wait(cx, "both events", |cx| {
        has_message(&f, cx, "3")
            && f.session.read(cx).client().is_some_and(|c| {
                c.read()
                    .message("c-general", "2")
                    .is_some_and(|m| !m.reactions.is_empty())
            })
    });
    assert!(lit(&f, cx).is_empty());
    assert!(!drawn(&f, cx, "timeline-lights"));
}

fn mention_in_random(f: &Fixture, seq: i64, count: i64) {
    f.server.push(frame(
        seq,
        "conversation.counts",
        &ConversationCounts {
            id: "c-random".into(),
            mention_count: count,
            unread_count: 3 + count,
            unread_from: None,
        },
    ));
}

fn mentions_in_random(f: &Fixture, cx: &mut TestAppContext) -> i64 {
    cx.update(|cx| {
        f.session.read(cx).client().map_or(0, |c| {
            c.read()
                .conversation("c-random")
                .map_or(0, |c| c.mention_count)
        })
    })
}

#[gpui_kit::test]
fn the_sidebar_badge_sends_out_a_ring_when_its_count_rises(cx: &mut TestAppContext) {
    let f = open_short(cx);
    // The badge as first drawn has no ring: it is not news.
    cx.update_window(f.window, |_, window, cx| window.render_frame(cx))
        .unwrap();
    assert!(!drawn(&f, cx, "mention-ring"));
    let before = mentions_in_random(&f, cx);
    mention_in_random(&f, 4, before + 1);
    wait(cx, "the count to rise", |cx| {
        f.session.read(cx).client().is_some_and(|c| {
            c.read()
                .conversation("c-random")
                .is_some_and(|c| c.mention_count > before)
        })
    });
    cx.run_until_parked();
    cx.update_window(f.window, |_, window, cx| window.render_frame(cx))
        .unwrap();
    assert!(drawn(&f, cx, "mention-ring"), "the ring is out");
    // It ends by itself, and nothing is drawn while nothing happens.
    let deadline = Instant::now() + HANG_GUARD;
    loop {
        cx.run_until_parked();
        cx.update_window(f.window, |_, window, cx| window.render_frame(cx))
            .unwrap();
        if !drawn(&f, cx, "mention-ring") {
            break;
        }
        assert!(Instant::now() < deadline, "the ring never ended");
        std::thread::sleep(Duration::from_millis(5));
    }
    // The count clears (the conversation is read), then rises again: the
    // first mention after that rings too.
    mention_in_random(&f, 5, 0);
    wait(cx, "the count to clear", |cx| {
        f.session.read(cx).client().is_some_and(|c| {
            c.read()
                .conversation("c-random")
                .is_some_and(|c| c.mention_count == 0)
        })
    });
    cx.run_until_parked();
    cx.update_window(f.window, |_, window, cx| window.render_frame(cx))
        .unwrap();
    assert!(!drawn(&f, cx, "mention-ring"), "a fall rings nothing");
    mention_in_random(&f, 6, 1);
    wait(cx, "the count to rise again", |cx| {
        f.session.read(cx).client().is_some_and(|c| {
            c.read()
                .conversation("c-random")
                .is_some_and(|c| c.mention_count == 1)
        })
    });
    cx.run_until_parked();
    cx.update_window(f.window, |_, window, cx| window.render_frame(cx))
        .unwrap();
    assert!(
        drawn(&f, cx, "mention-ring"),
        "the first mention after a clear rings"
    );
}

#[gpui_kit::test]
fn with_reduced_motion_the_badge_sends_no_ring(cx: &mut TestAppContext) {
    let f = open_short(cx);
    cx.update_window(f.window, |_, window, cx| window.render_frame(cx))
        .unwrap();
    reduce_motion(cx);
    let before = mentions_in_random(&f, cx);
    mention_in_random(&f, 4, before + 1);
    wait(cx, "the count to rise", |cx| {
        f.session.read(cx).client().is_some_and(|c| {
            c.read()
                .conversation("c-random")
                .is_some_and(|c| c.mention_count > before)
        })
    });
    cx.run_until_parked();
    cx.update_window(f.window, |_, window, cx| window.render_frame(cx))
        .unwrap();
    assert!(!drawn(&f, cx, "mention-ring"));
}

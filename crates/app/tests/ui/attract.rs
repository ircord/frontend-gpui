//! `/attract` in the timeline: the command is sent, and the event that
//! answers it (or someone else's) sends a comet round the message and onto
//! its group's avatar for three seconds, then stops. With reduced motion the
//! row and avatar are marked instead, for as long.

use std::time::{Duration, Instant};

use gpui_kit::test::TestWindowExt;
use gpui_kit::{AppContext, TestAppContext};
use ircord_client::contract::MessagePage;
use ircord_client::store::Connection;
use ircord_client::testing::{CannedServer, Reply, ephemeral};
use serde_json::json;

use crate::fixture::{Fixture, HANG_GUARD, bootstrap, message, open_with, snapshot};
use ircord_app::timeline::{self, attract::Target, comet::Moment};

fn open(cx: &mut TestAppContext) -> Fixture {
    open_and(cx, |_| {})
}

/// [`open`], with `setup` run on the server too.
fn open_and(cx: &mut TestAppContext, setup: impl FnOnce(&CannedServer)) -> Fixture {
    let mut snapshot = snapshot();
    snapshot.networks[0].features.attract = true;
    // The command is for channel operators.
    let general = &mut snapshot.conversations[0];
    general.member_ids.push("u-me".into());
    general.member_modes.insert("u-me".into(), "@".into());
    let f = open_with(cx, true, |server| {
        server.snapshot(&snapshot);
        server.route("POST", "/api/v1/conversations/*/attract", 202, json!({}));
        setup(server);
    });
    bootstrap(&f, cx);
    crate::fixture::wait(cx, "the live socket", |cx| {
        f.session
            .read(cx)
            .client()
            .is_some_and(|c| c.read().connection == Some(Connection::Live))
    });
    f
}

/// Whether the element `id` is drawn in the next frame.
fn drawn(f: &Fixture, cx: &mut TestAppContext, id: &'static str) -> bool {
    cx.run_until_parked();
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.try_find(id).is_some()
    })
    .unwrap()
}

fn wait_for(f: &Fixture, cx: &mut TestAppContext, id: &'static str, shown: bool) {
    let deadline = Instant::now() + HANG_GUARD;
    while drawn(f, cx, id) != shown {
        assert!(
            Instant::now() < deadline,
            "{id} was never {}",
            if shown { "drawn" } else { "removed" }
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Draw a frame and return the comets it drew.
fn comets(f: &Fixture, cx: &mut TestAppContext) -> Vec<(Target, Moment)> {
    cx.run_until_parked();
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        timeline::comets(cx)
    })
    .unwrap()
}

/// How many callbacks a frame drawn now leaves for the display's next frame.
fn next_frame_asks(f: &Fixture, cx: &mut TestAppContext) -> usize {
    cx.run_until_parked();
    cx.update_window(f.window, |_, window, cx| {
        // Earlier frames' requests are not this frame's.
        window.simulate_next_frame(cx);
        window.render_frame(cx);
        window.simulate_next_frame(cx)
    })
    .unwrap()
}

fn attract_event(f: &Fixture, message: &str, member: &str) {
    f.server.push(ephemeral(
        "attract",
        &json!({"conversationId": "c-general", "memberId": member, "messageId": message}),
    ));
}

#[gpui_kit::test]
fn the_command_is_sent_and_the_event_plays_on_the_message_and_its_groups_avatar(
    cx: &mut TestAppContext,
) {
    let f = open(cx);
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.click("composer-input", cx);
        window.input("/attract", cx);
    })
    .unwrap();
    // The first Enter takes the suggested command, the second sends it.
    for _ in 0..2 {
        cx.run_until_parked();
        cx.update_window(f.window, |_, window, cx| {
            window.render_frame(cx);
            window.press("enter", cx);
        })
        .unwrap();
    }
    cx.run_until_parked();
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
    })
    .unwrap();
    f.server.wait_for_nth(
        "POST",
        "/api/v1/conversations/c-general/attract",
        1,
        HANG_GUARD,
    );
    assert!(
        !drawn(&f, cx, "attract-row-2"),
        "nothing plays before the event"
    );

    // The server answers with the event; messages 1 and 2 are one group.
    attract_event(&f, "2", "m-bob");
    wait_for(&f, cx, "attract-row-2", true);
    // The comet's layer lies over the row, which shows no static marks.
    assert!(drawn(&f, cx, "attract-comet"), "the comet plays");
    // The picture stays where it was, and the comet takes off from it.
    let (face, marker) = cx
        .update_window(f.window, |_, window, cx| {
            window.render_frame(cx);
            (
                window.find("sender-avatar-1").bounds(),
                window.find("attract-avatar-1").bounds(),
            )
        })
        .unwrap();
    assert_eq!(face, marker, "the avatar's place is its picture's");
    // It moves: each frame asks the display for the next one, and a later
    // frame draws it further along.
    assert!(next_frame_asks(&f, cx) >= 1, "the comet asks for frames");
    let first = comets(&f, cx);
    std::thread::sleep(Duration::from_millis(200));
    let later = comets(&f, cx);
    assert_eq!((first.len(), later.len()), (1, 1));
    assert_eq!(first[0].0.row, "2");
    assert!(
        later[0].1.t >= first[0].1.t + 0.15,
        "the comet moved on from {:?} to {:?}",
        first[0].1,
        later[0].1
    );
    assert!(!drawn(&f, cx, "attract-tint-2") && !drawn(&f, cx, "attract-ring-1"));
    let (layer, row) = cx
        .update_window(f.window, |_, window, cx| {
            window.render_frame(cx);
            (
                window.find("attract-comet").bounds(),
                window.find("attract-row-2").bounds(),
            )
        })
        .unwrap();
    assert!(
        layer.contains(&row.origin) && row.size.width > gpui_kit::px(100.),
        "the comet's layer {layer:?} covers the row {row:?}"
    );
    assert!(
        layer.bottom() >= row.bottom() + gpui_kit::px(ircord_app::timeline::comet::REACH),
        "the comet's layer {layer:?} reaches under the last row {row:?}"
    );
    assert!(
        drawn(&f, cx, "attract-avatar-1"),
        "the comet lands on the group's head"
    );
    assert!(
        !drawn(&f, cx, "attract-row-1"),
        "only the message is lapped"
    );
    assert!(
        !drawn(&f, cx, "attract-avatar-2"),
        "a continuation has no avatar"
    );

    // It ends by itself.
    wait_for(&f, cx, "attract-row-2", false);
    assert!(!drawn(&f, cx, "attract-avatar-1"));
    assert!(!drawn(&f, cx, "attract-comet"));
}

#[gpui_kit::test]
fn with_reduced_motion_the_row_and_avatar_are_marked_and_nothing_moves(cx: &mut TestAppContext) {
    let f = open(cx);
    cx.update(|cx| {
        let mut theme = ircord_app::theme::get(cx).clone();
        theme.reduced_motion = true;
        ircord_app::theme::set(theme, cx);
    });
    let started = Instant::now();
    attract_event(&f, "2", "m-bob");
    wait_for(&f, cx, "attract-tint-2", true);
    assert!(
        drawn(&f, cx, "attract-ring-1"),
        "the group's head is ringed"
    );
    assert!(!drawn(&f, cx, "attract-comet"), "no comet");
    assert!(comets(&f, cx).is_empty());
    // The marks stay for the effect's three seconds, then go.
    std::thread::sleep(Duration::from_millis(1500));
    assert!(drawn(&f, cx, "attract-tint-2"));
    wait_for(&f, cx, "attract-tint-2", false);
    assert!(started.elapsed() >= Duration::from_millis(2900));
    assert!(!drawn(&f, cx, "attract-ring-1"));
}

#[gpui_kit::test]
fn someone_elses_attract_for_a_message_that_is_not_loaded_picks_their_newest(
    cx: &mut TestAppContext,
) {
    let f = open(cx);
    attract_event(&f, "not-loaded", "m-bob");
    wait_for(&f, cx, "attract-row-2", true);
    assert!(drawn(&f, cx, "attract-avatar-1"));
    // A person with no message here makes nothing play.
    wait_for(&f, cx, "attract-row-2", false);
    attract_event(&f, "not-loaded", "m-eve");
    crate::fixture::wait(cx, "the attract is stored", |cx| {
        f.session.read(cx).client().is_some_and(|c| {
            c.read()
                .reading
                .attract("c-general")
                .is_some_and(|a| a.member_id == "m-eve")
        })
    });
    assert!(!drawn(&f, cx, "attract-row-1"));
    assert!(!drawn(&f, cx, "attract-row-2"));
}

#[gpui_kit::test]
fn a_second_attract_restarts_the_first(cx: &mut TestAppContext) {
    let f = open(cx);
    attract_event(&f, "1", "m-bob");
    wait_for(&f, cx, "attract-row-1", true);
    std::thread::sleep(Duration::from_millis(2000));
    attract_event(&f, "2", "m-bob");
    wait_for(&f, cx, "attract-row-2", true);
    assert!(
        !drawn(&f, cx, "attract-row-1"),
        "it moved to the new message"
    );
    assert!(drawn(&f, cx, "attract-comet"));
    // 2 s after the first it would have ended; the new one plays its own 3 s.
    std::thread::sleep(Duration::from_millis(1500));
    assert!(drawn(&f, cx, "attract-row-2"));
    wait_for(&f, cx, "attract-row-2", false);
}

#[gpui_kit::test]
fn a_replaced_comet_lets_its_sparks_land_on_its_own_row(cx: &mut TestAppContext) {
    // Eve's message is its own group, so the two attracts share no row.
    let f = open_and(cx, |server| {
        server.route_fn("GET", "/api/v1/conversations/*/messages", |request| {
            let items = if request.path.ends_with("/c-general/messages") {
                let mut eve = message("2", "c-general", "from eve");
                eve.sender_id = "m-eve".into();
                vec![message("1", "c-general", "hello"), eve]
            } else {
                Vec::new()
            };
            Reply::json(
                200,
                serde_json::to_value(MessagePage {
                    items,
                    ..Default::default()
                })
                .unwrap(),
            )
        });
    });
    attract_event(&f, "1", "m-bob");
    wait_for(&f, cx, "attract-row-1", true);
    std::thread::sleep(Duration::from_millis(1000));
    attract_event(&f, "2", "m-eve");
    wait_for(&f, cx, "attract-row-2", true);
    let taken_over = Instant::now();

    // Both are drawn: the old one only as its falling sparks.
    let both = comets(&f, cx);
    assert_eq!(both.len(), 2, "{both:?}");
    assert_eq!((both[0].0.row.as_str(), both[1].0.row.as_str()), ("1", "2"));
    let replaced = both[0].1.replaced.expect("the old comet was replaced");
    assert!(replaced >= 0.9, "replaced {replaced} s in");
    assert_eq!(both[1].1.replaced, None);
    // The old row still reports its place, but no longer counts as playing.
    assert!(!drawn(&f, cx, "attract-row-1"));
    assert!(drawn(&f, cx, "attract-avatar-2"));

    // Once its sparks have landed it is gone, long before the new one ends.
    let deadline = Instant::now() + HANG_GUARD;
    while comets(&f, cx).len() == 2 {
        assert!(Instant::now() < deadline, "the old comet never went");
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(taken_over.elapsed() < Duration::from_millis(2500));
    let left = comets(&f, cx);
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].0.row, "2");
    wait_for(&f, cx, "attract-row-2", false);
}

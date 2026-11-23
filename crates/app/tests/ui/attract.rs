//! `/attract` in the timeline: the command is sent, and the event that
//! answers it (or someone else's) sends a comet round the message and onto
//! its group's avatar for three seconds, then stops. With reduced motion the
//! row and avatar are marked instead, for as long.

use std::time::{Duration, Instant};

use gpui_kit::test::TestWindowExt;
use gpui_kit::{AppContext, TestAppContext};
use ircord_client::store::Connection;
use ircord_client::testing::ephemeral;
use serde_json::json;

use crate::fixture::{Fixture, HANG_GUARD, bootstrap, open_with, snapshot};

fn open(cx: &mut TestAppContext) -> Fixture {
    let mut snapshot = snapshot();
    snapshot.networks[0].features.attract = true;
    // The command is for channel operators.
    let general = &mut snapshot.conversations[0];
    general.member_ids.push("u-me".into());
    general.member_modes.insert("u-me".into(), "@".into());
    let f = open_with(cx, true, |server| {
        server.snapshot(&snapshot);
        server.route("POST", "/api/v1/conversations/*/attract", 202, json!({}));
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

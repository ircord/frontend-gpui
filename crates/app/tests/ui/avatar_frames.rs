//! Avatar frames that move: on the profile card the spectrum frame turns,
//! the gold frame glints now and then and the glow breathes, each only while
//! the card is open; the member list keeps its frames still; with reduced
//! motion the card shows the frames as it always did.

use gpui_kit::test::TestWindowExt;
use gpui_kit::{AppContext, TestAppContext};
use ircord_app::ui::frame_motion;
use ircord_client::contract::*;
use ircord_client::testing::{CannedServer, Reply};
use serde_json::json;

use crate::fixture::{Fixture, HANG_GUARD, open_with, snapshot, wait};

/// #general with bob wearing `frame` in his accent.
fn open(cx: &mut TestAppContext, frame_of_bob: &str) -> Fixture {
    let mut s = snapshot();
    for m in &mut s.members {
        if m.id == "m-bob" {
            m.avatar_frame = Some(frame_of_bob.into());
            m.profile_accent_color = Some("#22d3ee".into());
        }
    }
    let f = open_with(cx, true, move |server: &CannedServer| {
        server.snapshot(&s);
        server.route_fn("GET", "/api/v1/notifications/settings", |_| {
            Reply::json(200, serde_json::to_value(Settings::default()).unwrap())
        });
        server.route("PUT", "/api/v1/notifications/settings", 200, json!({}));
    });
    wait(cx, "#general", |cx| {
        let session = f.session.read(cx);
        session.conversation.as_deref() == Some("c-general")
            && session.client().is_some_and(|c| {
                c.read()
                    .timelines
                    .get("c-general")
                    .is_some_and(|t| t.loaded)
            })
    });
    f
}

fn frame(f: &Fixture, cx: &mut TestAppContext) {
    cx.run_until_parked();
    cx.update_window(f.window, |_, window, cx| window.render_frame(cx))
        .unwrap();
}

fn shows(f: &Fixture, cx: &mut TestAppContext, id: &'static str) -> bool {
    cx.run_until_parked();
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.try_find(id).is_some()
    })
    .unwrap()
}

fn wait_for(f: &Fixture, cx: &mut TestAppContext, id: &'static str) {
    let deadline = std::time::Instant::now() + HANG_GUARD;
    while !shows(f, cx, id) {
        assert!(std::time::Instant::now() < deadline, "{id} never showed");
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

fn open_card(f: &Fixture, cx: &mut TestAppContext, frame_id: &'static str) {
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.click("member-m-bob", cx);
    })
    .unwrap();
    cx.run_until_parked();
    wait_for(f, cx, frame_id);
}

fn close_card(f: &Fixture, cx: &mut TestAppContext) {
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        ircord_app::ui::dialog::close(window, cx);
    })
    .unwrap();
    cx.run_until_parked();
}

fn painted(cx: &mut TestAppContext) -> (usize, usize, usize) {
    cx.update(|cx| frame_motion::painted(cx))
}

/// How many callbacks a frame drawn now leaves for the display's next frame.
fn next_frame_asks(f: &Fixture, cx: &mut TestAppContext) -> usize {
    cx.run_until_parked();
    cx.update_window(f.window, |_, window, cx| {
        window.simulate_next_frame(cx);
        window.render_frame(cx);
        window.simulate_next_frame(cx)
    })
    .unwrap()
}

fn reduce_motion(cx: &mut TestAppContext) {
    cx.update(|cx| {
        let mut theme = ircord_app::theme::get(cx).clone();
        theme.reduced_motion = true;
        ircord_app::theme::set(theme, cx);
    });
}

#[gpui_kit::test]
fn the_spectrum_turns_while_the_card_is_open_and_stops_when_it_closes(cx: &mut TestAppContext) {
    let f = open(cx, "gradient");
    // The member list draws its frame still: nothing is painted by it.
    wait_for(&f, cx, "avatar-frame-member-m-bob-gradient");
    frame(&f, cx);
    assert_eq!(painted(cx), (0, 0, 0), "the member list is still");
    assert_eq!(next_frame_asks(&f, cx), 0);

    open_card(&f, cx, "avatar-frame-profile-gradient");
    let before = painted(cx);
    frame(&f, cx);
    frame(&f, cx);
    let after = painted(cx);
    assert!(after.0 >= before.0 + 2, "{before:?} then {after:?}");
    assert_eq!((after.1, after.2), (0, 0));
    assert!(next_frame_asks(&f, cx) >= 1, "it asks for the next frame");

    close_card(&f, cx);
    let gone = painted(cx);
    frame(&f, cx);
    frame(&f, cx);
    assert_eq!(
        painted(cx),
        gone,
        "nothing is drawn once the card is closed"
    );
    assert_eq!(next_frame_asks(&f, cx), 0, "and no frames are asked for");
}

#[gpui_kit::test]
fn the_glow_breathes_on_the_card(cx: &mut TestAppContext) {
    let f = open(cx, "glow");
    open_card(&f, cx, "avatar-frame-profile-glow");
    let before = painted(cx);
    frame(&f, cx);
    assert!(painted(cx).2 > before.2);
    assert!(next_frame_asks(&f, cx) >= 1);
    close_card(&f, cx);
    assert_eq!(next_frame_asks(&f, cx), 0);
}

/// The frames a card asks for when it has a frame that does not move: the
/// card has a few of its own (its lookup, say).
fn baseline(cx: &mut TestAppContext) -> usize {
    let f = open(cx, "ring");
    open_card(&f, cx, "avatar-frame-profile-ring");
    next_frame_asks(&f, cx)
}

#[gpui_kit::test]
fn the_gold_frame_glints_now_and_then_and_is_idle_between(cx: &mut TestAppContext) {
    let still = baseline(cx);
    let f = open(cx, "gold");
    open_card(&f, cx, "avatar-frame-profile-gold");
    // Just after a glint starts it is drawn and a frame is asked for.
    cx.update(|_| frame_motion::align(frame_motion::GLINT_SECS, 0.2));
    let before = painted(cx);
    frame(&f, cx);
    assert!(painted(cx).1 > before.1, "the glint is drawn");
    assert_eq!(next_frame_asks(&f, cx), still + 1);

    // Between glints nothing is drawn and no frame is asked for: the window
    // is woken when the next one is due.
    cx.update(|_| frame_motion::align(frame_motion::GLINT_SECS, 2.));
    let quiet = painted(cx);
    frame(&f, cx);
    frame(&f, cx);
    assert_eq!(painted(cx), quiet);
    assert_eq!(next_frame_asks(&f, cx), still);
}

#[gpui_kit::test]
fn with_reduced_motion_the_card_keeps_its_frames_still(cx: &mut TestAppContext) {
    let still = baseline(cx);
    for kind in ["gradient", "gold", "glow"] {
        let f = open(cx, kind);
        reduce_motion(cx);
        let before = painted(cx);
        let id: &'static str = match kind {
            "gradient" => "avatar-frame-profile-gradient",
            "gold" => "avatar-frame-profile-gold",
            _ => "avatar-frame-profile-glow",
        };
        open_card(&f, cx, id);
        cx.update(|_| frame_motion::align(frame_motion::GLINT_SECS, 0.2));
        frame(&f, cx);
        frame(&f, cx);
        // The frame is there, drawn as it was before: no moving canvas.
        assert!(shows(&f, cx, id), "{kind}");
        assert_eq!(painted(cx), before, "{kind}");
        assert_eq!(next_frame_asks(&f, cx), still, "{kind}");
    }
}

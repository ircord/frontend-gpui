//! The sidebar's selection highlight: it glides from the open row to the
//! next on a spring instead of jumping, follows the list's scroll, asks for
//! frames only while it moves, and with reduced motion the open row draws its
//! own background at once, as it always did.

use std::time::{Duration, Instant};

use gpui_kit::test::TestWindowExt;
use gpui_kit::{AppContext, Bounds, Pixels, ScaledPixels, ScrollDelta, TestAppContext, point, px};
use ircord_app::sidebar::glide::{self, Shown};
use ircord_client::contract::Snapshot;
use ircord_client::testing::{CannedServer, Reply};
use serde_json::json;

use crate::fixture::{Fixture, HANG_GUARD, open_with, snapshot, wait};

fn documents(server: &CannedServer) {
    server.route("GET", "/api/v1/app/preferences", 200, json!({}));
    server.route_fn("PUT", "/api/v1/app/preferences", |r| {
        Reply::json(200, r.json())
    });
}

/// #general (open), #random, and `more` channels after them.
fn workspace(more: usize) -> Snapshot {
    let mut s = snapshot();
    let template = s.conversations[1].clone();
    for n in 0..more {
        let mut c = template.clone();
        c.id = format!("c-ch{n:02}");
        // After #general and #random, so #general is the one that opens.
        c.name = format!("zz{n:02}");
        c.unread_count = 0;
        c.mention_count = 0;
        s.conversations.push(c);
    }
    s
}

fn open(cx: &mut TestAppContext, more: usize) -> Fixture {
    let s = workspace(more);
    let f = open_with(cx, true, move |server| {
        server.snapshot(&s);
        documents(server);
    });
    wait(cx, "the workspace and the preferences", |cx| {
        let session = f.session.read(cx);
        let Some(client) = session.client() else {
            return false;
        };
        session.conversation.as_deref() == Some("c-general")
            && client.read().documents.preferences.is_loaded()
    });
    f
}

/// Draw a frame and say what the highlight drew in it.
fn shown(f: &Fixture, cx: &mut TestAppContext) -> Option<Shown> {
    cx.run_until_parked();
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        glide::shown(cx)
    })
    .unwrap()
}

fn row(f: &Fixture, cx: &mut TestAppContext, id: &'static str) -> Bounds<Pixels> {
    cx.update_window(f.window, |_, window, _| window.find(id).bounds())
        .unwrap()
}

fn click(f: &Fixture, cx: &mut TestAppContext, id: &'static str) {
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.click(id, cx);
    })
    .unwrap();
    cx.run_until_parked();
}

/// Draw frames until the highlight has arrived, and return its last.
fn arrive(f: &Fixture, cx: &mut TestAppContext) -> Shown {
    let deadline = Instant::now() + HANG_GUARD;
    loop {
        let s = shown(f, cx).expect("a highlight");
        if !s.moving {
            return s;
        }
        assert!(Instant::now() < deadline, "it never arrived: {s:?}");
        std::thread::sleep(Duration::from_millis(10));
    }
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

#[gpui_kit::test]
fn the_highlight_glides_to_the_row_just_opened_and_then_rests(cx: &mut TestAppContext) {
    let f = open(cx, 0);
    // At the start it lies on the open row and is at rest.
    let start = shown(&f, cx).expect("the open row is highlighted");
    assert!(!start.moving);
    assert_eq!(start.bounds, row(&f, cx, "conversation-c-general"));
    assert_eq!(next_frame_asks(&f, cx), 0, "nothing moves, no frames asked");

    // Opening #random sends it down the list; the first frame after still
    // has it on the row it left.
    click(&f, cx, "conversation-c-random");
    let first = shown(&f, cx).unwrap();
    let target = row(&f, cx, "conversation-c-random");
    assert!(first.moving, "{first:?}");
    assert!(first.target.top > start.span.top);
    assert!(first.span.top < first.target.top);
    assert!(next_frame_asks(&f, cx) >= 1, "it asks for the next frame");
    assert_eq!(
        f.session.read_with(cx, |s, _| s.conversation.clone()),
        Some("c-random".into()),
        "the conversation itself opens at once"
    );

    // A moment later it is between the two rows (measured down the list's
    // content: opening #random also removes a line above the list).
    std::thread::sleep(Duration::from_millis(60));
    let between = shown(&f, cx).unwrap();
    assert!(
        between.span.top > start.span.top && between.span.top < between.target.top,
        "{between:?} is not between {start:?} and {:?}",
        first.target
    );

    // It arrives exactly on the row, then asks for nothing more.
    let end = arrive(&f, cx);
    assert_eq!(end.bounds, target);
    assert_eq!(next_frame_asks(&f, cx), 0, "at rest it draws no frames");
    assert_eq!(shown(&f, cx).unwrap().bounds, target);
}

#[gpui_kit::test]
fn the_highlight_follows_a_list_that_scrolls(cx: &mut TestAppContext) {
    let f = open(cx, 40);
    let before = shown(&f, cx).expect("the open row is highlighted");
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.scroll(
            "conversation-list",
            ScrollDelta::Pixels(point(px(0.), px(-200.))),
            cx,
        );
    })
    .unwrap();
    cx.run_until_parked();
    // The list scrolled, and the highlight went with its row.
    let s = shown(&f, cx).expect("still laid out");
    assert_eq!(s.bounds, row(&f, cx, "conversation-c-general"));
    assert!(
        s.bounds.origin.y < before.bounds.origin.y - px(100.),
        "{:?} after {:?}",
        s.bounds,
        before.bounds
    );
    assert!(!s.moving, "scrolling is not a glide: {s:?}");

    // Open a row that is in view, and the highlight lands on it.
    let ids = [
        "conversation-c-ch10",
        "conversation-c-ch12",
        "conversation-c-ch14",
    ];
    let visible = cx
        .update_window(f.window, |_, window, _| {
            ids.into_iter()
                .find(|id| window.try_find(*id).is_some_and(|e| e.visible()))
        })
        .unwrap()
        .expect("a channel in view");
    click(&f, cx, visible);
    let end = arrive(&f, cx);
    assert_eq!(end.bounds, row(&f, cx, visible));
    // Scrolling again keeps it on its row, with nothing in flight.
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.scroll(
            "conversation-list",
            ScrollDelta::Pixels(point(px(0.), px(40.))),
            cx,
        );
    })
    .unwrap();
    let after = shown(&f, cx).unwrap();
    assert!(!after.moving, "{after:?}");
    assert_eq!(after.bounds, row(&f, cx, visible));
}

#[gpui_kit::test]
fn with_reduced_motion_the_open_row_draws_its_own_highlight_at_once(cx: &mut TestAppContext) {
    let f = open(cx, 0);
    cx.update(|cx| {
        let mut theme = ircord_app::theme::get(cx).clone();
        theme.reduced_motion = true;
        ircord_app::theme::set(theme, cx);
    });
    assert_eq!(shown(&f, cx), None, "no highlight layer");

    click(&f, cx, "conversation-c-random");
    cx.run_until_parked();
    let selected = cx.update(|cx| ircord_app::theme::get(cx).selected);
    let target = row(&f, cx, "conversation-c-random");
    // The row's own background is the highlight, on the new row in the first
    // frame after the click.
    let (drawn, scale) = cx
        .update_window(f.window, |_, window, cx| {
            window.render_frame(cx);
            let scale = window.scale_factor();
            let quads = window
                .painted_quads()
                .into_iter()
                .filter(|q| q.background.as_solid() == Some(selected))
                .map(|q| q.bounds)
                .collect::<Vec<_>>();
            (quads, scale)
        })
        .unwrap();
    // Quads are in device pixels, snapped to them.
    let same = |q: &Bounds<ScaledPixels>| {
        let near = |a: ScaledPixels, b: Pixels| (a.0 - f32::from(b) * scale).abs() < 1.;
        near(q.origin.x, target.origin.x)
            && near(q.origin.y, target.origin.y)
            && near(q.size.width, target.size.width)
            && near(q.size.height, target.size.height)
    };
    assert!(drawn.iter().any(same), "{drawn:?} lacks {target:?}");
    assert_eq!(shown(&f, cx), None);
    assert_eq!(next_frame_asks(&f, cx), 0);
}

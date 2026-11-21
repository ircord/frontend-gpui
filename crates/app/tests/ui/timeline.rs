//! Tests for package R1: reading the timeline against the canned server —
//! a reply's quote jumping to an old message and the pill back to the
//! present, the "New messages" line before the first unread row, marking a
//! message unread, jumping to a date and exporting the log.

use std::time::Duration;

use gpui_kit::test::{TestEventExt, TestQueryExt, TestWindowExt};
use gpui_kit::{AppContext, MouseButton, TestAppContext};
use ircord_client::contract::*;
use ircord_client::testing::{CannedServer, Reply};
use serde_json::json;

use crate::fixture::{Fixture, bootstrap, message, open_with, wait};

const WAIT: Duration = Duration::from_secs(5);

fn at(i: u32) -> String {
    format!("2026-11-{:02}T10:{:02}:00Z", 1 + i / 60, i % 60)
}

fn numbered(i: u32) -> Message {
    let mut m = message(&format!("m{i}"), "c-general", &format!("message {i}"));
    m.created_at = at(i);
    m
}

/// `#general` holds m100..m119 as its newest page (m119 replies to the old
/// m3); a window around m3 is m1..m40, with newer history after it.
fn serve_history(server: &CannedServer) {
    server.route_fn("GET", "/api/v1/conversations/*/messages", |r| {
        if !r.path.ends_with("/c-general/messages") {
            return Reply::json(200, json!({"items": [], "nextCursor": null}));
        }
        if let Some(around) = r.query_param("around") {
            let page = MessagePage {
                items: (1..=40).map(numbered).collect(),
                next_cursor: None,
                has_newer: Some(true),
                ..Default::default()
            };
            assert!(around.starts_with('m'));
            return Reply::json(200, serde_json::to_value(page).unwrap());
        }
        if r.query_param("before").is_some() {
            return Reply::json(200, json!({"items": [], "nextCursor": null}));
        }
        let mut items: Vec<Message> = (100..120).map(numbered).collect();
        items.last_mut().unwrap().reply_to = Some(ircord_client::contract::Reply {
            id: "m3".into(),
            nick: "bob".into(),
            text: "message 3".into(),
        });
        let page = MessagePage {
            items,
            next_cursor: Some("older".into()),
            ..Default::default()
        };
        Reply::json(200, serde_json::to_value(page).unwrap())
    });
}

fn timeline_has(f: &Fixture, cx: &mut gpui_kit::App, id: &str) -> bool {
    let session = f.session.read(cx);
    let Some(client) = session.client() else {
        return false;
    };
    client
        .read()
        .timelines
        .get("c-general")
        .is_some_and(|t| t.find(id).is_some())
}

fn frame(f: &Fixture, cx: &mut TestAppContext, check: impl FnOnce(&mut gpui_kit::Window)) {
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        check(window);
    })
    .unwrap();
}

#[gpui_kit::test]
fn a_reply_jumps_to_an_old_message_and_the_pill_returns(cx: &mut TestAppContext) {
    let f = open_with(cx, true, serve_history);
    wait(cx, "the newest page", |cx| timeline_has(&f, cx, "m119"));
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.click("reply-m119", cx);
    })
    .unwrap();
    wait(cx, "the window around m3", |cx| timeline_has(&f, cx, "m3"));
    let around = f
        .server
        .requests_to("GET", "/api/v1/conversations/c-general/messages")
        .into_iter()
        .find_map(|r| r.query_param("around"));
    assert_eq!(around.as_deref(), Some("m3"));
    cx.update(|cx| {
        let session = f.session.read(cx);
        let w = session.client().unwrap().read();
        assert_eq!(w.focused("c-general"), Some("m3"));
        assert!(w.timelines["c-general"].has_newer);
        assert!(w.timelines["c-general"].find("m119").is_none());
    });
    cx.run_until_parked();
    frame(&f, cx, |window| {
        assert!(window.find("message-m3").visible(), "scrolled to m3");
        assert!(
            window
                .find_by_label("Viewing older messages · Jump to present")
                .visible()
        );
    });
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.click("jump-present", cx);
    })
    .unwrap();
    wait(cx, "the present again", |cx| {
        timeline_has(&f, cx, "m119") && !timeline_has(&f, cx, "m3")
    });
    cx.run_until_parked();
    frame(&f, cx, |window| {
        assert!(window.find("message-m119").visible());
        assert!(window.try_find("jump-present").is_none());
    });
    cx.update(|cx| {
        let session = f.session.read(cx);
        let w = session.client().unwrap().read();
        assert!(!w.timelines["c-general"].has_newer);
        assert_eq!(w.focused("c-general"), None);
    });
}

#[gpui_kit::test]
fn the_unread_line_sits_before_the_first_unread_row(cx: &mut TestAppContext) {
    // #random has 3 unread in the fixture's snapshot.
    let f = open_with(cx, true, |server| {
        server.route_fn("GET", "/api/v1/conversations/*/messages", |r| {
            let items: Vec<Message> = if r.path.ends_with("/c-random/messages") {
                (1..=6)
                    .map(|i| {
                        let mut m = message(&format!("r{i}"), "c-random", &format!("r {i}"));
                        m.created_at = at(i);
                        m
                    })
                    .collect()
            } else {
                vec![message("1", "c-general", "hello")]
            };
            Reply::json(200, json!({"items": items, "nextCursor": null}))
        });
        server.route("PATCH", "/api/v1/conversations/*", 204, json!(null));
    });
    wait(cx, "the bootstrap", |cx| {
        f.session.read(cx).client().is_some()
    });
    wait(cx, "#general", |cx| {
        f.session.read(cx).conversation.as_deref() == Some("c-general")
    });
    cx.update(|cx| {
        f.session
            .update(cx, |s, cx| s.select_conversation("c-random", cx))
    });
    wait(cx, "#random's history", |cx| {
        let session = f.session.read(cx);
        let w = session.client().unwrap().read();
        w.timelines.get("c-random").is_some_and(|t| t.loaded)
    });
    cx.run_until_parked();
    frame(&f, cx, |window| {
        let line = window.find("unread-line").bounds();
        let above = window.find("message-r3").bounds();
        let first_unread = window.find("message-r4").bounds();
        assert!(above.bottom() <= line.top(), "r3 is read, above the line");
        assert!(line.bottom() <= first_unread.top(), "the line is before r4");
        assert!(window.find_by_label("New messages").visible());
    });
    // Opening it read it, but the line stays for this visit.
    cx.update(|cx| {
        let session = f.session.read(cx);
        let w = session.client().unwrap().read();
        assert_eq!(w.conversation("c-random").unwrap().unread_count, 0);
    });
}

#[gpui_kit::test]
fn mark_as_unread_tells_the_server_and_draws_the_line(cx: &mut TestAppContext) {
    let f = open_with(cx, true, |server| {
        server.route("PATCH", "/api/v1/conversations/*", 204, json!(null));
    });
    bootstrap(&f, cx);
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.hover("message-2", cx);
        window.right_click("message-2", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        let item = window.find_by_label("Mark as unread").bounds().center();
        window.pointer_down(item, MouseButton::Left, cx);
        window.pointer_up(item, MouseButton::Left, cx);
    })
    .unwrap();
    let patch = f
        .server
        .wait_for_nth("PATCH", "/api/v1/conversations/c-general", 1, WAIT);
    let body = patch.json();
    // The read on opening may come first; the unread one names message 2.
    let all: Vec<_> = f
        .server
        .requests_to("PATCH", "/api/v1/conversations/c-general")
        .into_iter()
        .map(|r| r.json())
        .collect();
    assert!(
        all.iter().any(|b| b["unreadFrom"] == "2"),
        "{all:?} (first {body})"
    );
    cx.run_until_parked();
    frame(&f, cx, |window| {
        let line = window.find("unread-line").bounds();
        assert!(line.bottom() <= window.find("message-2").bounds().top());
        assert!(window.find("message-1").bounds().bottom() <= line.top());
    });
    cx.update(|cx| {
        let session = f.session.read(cx);
        let w = session.client().unwrap().read();
        assert_eq!(
            w.conversation("c-general").unwrap().unread_from.as_deref(),
            Some("2")
        );
    });
}

#[gpui_kit::test]
fn jump_to_a_date_lands_on_its_first_message(cx: &mut TestAppContext) {
    let f = open_with(cx, true, |server| {
        server.route_fn("GET", "/api/v1/conversations/*/messages", |r| {
            if r.query_param("before").as_deref() == Some("older") {
                let mut old = message("0", "c-general", "from the day before");
                old.created_at = "2026-11-19T22:00:00Z".into();
                let mut day = message("0b", "c-general", "that morning");
                day.created_at = "2026-11-20T12:00:00Z".into();
                return Reply::json(200, json!({"items": [old, day], "nextCursor": null}));
            }
            let items = vec![
                message("1", "c-general", "hello"),
                message("2", "c-general", "second line"),
            ];
            Reply::json(200, json!({"items": items, "nextCursor": "older"}))
        });
    });
    bootstrap(&f, cx);
    cx.update_window(f.window, |_, window, cx| {
        ircord_app::timeline::jump_to_date(&f.session, "c-general", "2026-11-20", window, cx)
    })
    .unwrap();
    wait(cx, "the jump to 2026-11-20", |cx| {
        let session = f.session.read(cx);
        let w = session.client().unwrap().read();
        w.focused("c-general") == Some("0b")
    });
    let before = f
        .server
        .requests_to("GET", "/api/v1/conversations/c-general/messages")
        .into_iter()
        .filter_map(|r| r.query_param("before"))
        .collect::<Vec<_>>();
    assert_eq!(before, ["older"], "one older page was enough");
    cx.run_until_parked();
    frame(&f, cx, |window| {
        assert!(window.find("message-0b").visible())
    });
}

#[gpui_kit::test]
fn export_writes_the_conversation_as_a_log(cx: &mut TestAppContext) {
    let f = open_with(cx, true, |_| {});
    bootstrap(&f, cx);
    let dir = std::env::temp_dir().join(format!(
        "ircord-export-{}",
        ircord_client::client::random_id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("general.log");
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.right_click("conversation-c-general", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        let item = window.find_by_label("Export log…").bounds().center();
        window.pointer_down(item, MouseButton::Left, cx);
        window.pointer_up(item, MouseButton::Left, cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert!(cx.did_prompt_for_new_path());
    let chosen = file.clone();
    cx.simulate_new_path_selection(move |suggested| {
        assert!(suggested.is_dir());
        Some(chosen)
    });
    wait(cx, "the log file", |_| file.exists());
    let log = std::fs::read_to_string(&file).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    let lines: Vec<&str> = log.lines().collect();
    assert!(
        lines[0].starts_with("Log of #general on Home, exported "),
        "{log}"
    );
    assert!(
        lines[2].ends_with("] <bob> hello world https://ircord.com"),
        "{log}"
    );
    assert!(lines[3].ends_with("] <bob> second line"), "{log}");
}

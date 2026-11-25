//! Copying a stretch of the timeline writes the log the web client writes:
//! a day line, then `[time] <nick> text` for each message and `*** …` for
//! each event, with another day line where the selection crosses midnight.
//! A selection inside one message copies as selected.

use chrono::{DateTime, Local};
use gpui_kit::test::TestWindowExt;
use gpui_kit::{AppContext, TestAppContext};
use ircord_client::contract::*;
use ircord_client::testing::Reply;
use serde_json::json;

use crate::fixture::{Fixture, message, open_with, snapshot, wait};

fn open(cx: &mut TestAppContext, items: Vec<Message>) -> Fixture {
    let count = items.len();
    let f = open_with(cx, true, |server| {
        server.snapshot(&snapshot());
        let page = serde_json::to_value(MessagePage {
            items,
            ..Default::default()
        })
        .unwrap();
        server.route_fn("GET", "/api/v1/conversations/*/messages", move |r| {
            if r.path.ends_with("/c-general/messages") {
                Reply::json(200, page.clone())
            } else {
                Reply::json(200, json!({"items": []}))
            }
        });
    });
    wait(cx, "#general's history", |cx| {
        let session = f.session.read(cx);
        let Some(client) = session.client() else {
            return false;
        };
        let w = client.read();
        w.timelines
            .get("c-general")
            .is_some_and(|t| t.loaded && t.entries.len() == count)
    });
    cx.run_until_parked();
    f
}

fn sent(id: &str, at: &str, text: &str) -> Message {
    let mut m = message(id, "c-general", text);
    m.created_at = at.into();
    m
}

/// `[HH:MM]` as the timeline shows it in this time zone.
fn clock(at: &str) -> String {
    DateTime::parse_from_rfc3339(at)
        .unwrap()
        .with_timezone(&Local)
        .format("%H:%M")
        .to_string()
}

/// The day line as pasted, in this time zone.
fn day(at: &str) -> String {
    DateTime::parse_from_rfc3339(at)
        .unwrap()
        .with_timezone(&Local)
        .format("--- %A, %B %-d, %Y ---")
        .to_string()
}

fn copy_between(f: &Fixture, cx: &mut TestAppContext, from: &str, to: &str) -> String {
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        // From the start of the first text to the end of the second.
        let (first, last) = (
            window.find(from.to_string()).bounds(),
            window.find(to.to_string()).bounds(),
        );
        window.drag(
            gpui_kit::point(first.left(), first.center().y),
            gpui_kit::point(last.right(), last.center().y),
            cx,
        );
        window.press("ctrl-c", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.read_from_clipboard().and_then(|c| c.text()).unwrap()
}

#[gpui_kit::test]
fn copying_two_messages_writes_a_day_line_and_log_lines(cx: &mut TestAppContext) {
    let (a, b) = ("2026-11-24T12:10:05Z", "2026-11-24T12:10:40Z");
    let f = open(
        cx,
        vec![
            sent("1", a, "that should be implemented already"),
            sent("2", b, "the go backend exposes it"),
        ],
    );
    let copied = copy_between(&f, cx, "text-1-0-block", "text-2-0-block");
    assert_eq!(
        copied,
        format!(
            "{}\n[{}] <bob> that should be implemented already\n[{}] <bob> the go backend exposes it",
            day(a),
            clock(a),
            clock(b)
        )
    );
}

#[gpui_kit::test]
fn a_selection_across_midnight_gets_a_second_day_line(cx: &mut TestAppContext) {
    let (a, b) = ("2026-11-23T12:00:00Z", "2026-11-24T12:00:00Z");
    let f = open(cx, vec![sent("1", a, "yesterday"), sent("2", b, "today")]);
    let copied = copy_between(&f, cx, "text-1-0-block", "text-2-0-block");
    assert_eq!(
        copied,
        format!(
            "{}\n[{}] <bob> yesterday\n{}\n[{}] <bob> today",
            day(a),
            clock(a),
            day(b),
            clock(b)
        )
    );
}

#[gpui_kit::test]
fn a_selection_inside_one_message_copies_without_the_log_line(cx: &mut TestAppContext) {
    let f = open(
        cx,
        vec![
            sent("1", "2026-11-24T12:00:00Z", "alpha beta gamma"),
            sent("2", "2026-11-24T12:01:00Z", "second"),
        ],
    );
    let copied = cx
        .update_window(f.window, |_, window, cx| {
            window.render_frame(cx);
            let b = window.find("text-1-0-block").bounds();
            window.drag(
                gpui_kit::point(b.left(), b.center().y),
                gpui_kit::point(b.right(), b.center().y),
                cx,
            );
            window.press("ctrl-c", cx);
        })
        .map(|_| cx.read_from_clipboard().and_then(|c| c.text()))
        .unwrap();
    assert_eq!(copied.as_deref(), Some("alpha beta gamma"));
}

#[gpui_kit::test]
fn an_event_between_messages_copies_as_a_log_line(cx: &mut TestAppContext) {
    let (a, e, b) = (
        "2026-11-24T12:00:00Z",
        "2026-11-24T12:01:00Z",
        "2026-11-24T12:02:00Z",
    );
    let mut join = sent("2", e, "");
    join.kind = MessageKind::Event;
    join.sender_id = "m-bob".into();
    join.event = Some(MembershipEvent {
        actor_id: "m-bob".into(),
        r#type: MembershipEventType::Join,
        ..Default::default()
    });
    let f = open(cx, vec![sent("1", a, "hello"), join, sent("3", b, "again")]);
    let copied = copy_between(&f, cx, "text-1-0-block", "text-3-0-block");
    assert_eq!(
        copied,
        format!(
            "{}\n[{}] <bob> hello\n[{}] *** bob joined\n[{}] <bob> again",
            day(a),
            clock(a),
            clock(e),
            clock(b)
        )
    );
}

#[gpui_kit::test]
fn a_collapsed_run_between_messages_copies_every_event(cx: &mut TestAppContext) {
    let (a, b, c) = (
        "2026-11-24T12:00:00Z",
        "2026-11-24T12:00:30Z",
        "2026-11-24T12:01:00Z",
    );
    let join = |id: &str, at: &str, who: &str| {
        let mut m = sent(id, at, "");
        m.kind = MessageKind::Event;
        m.sender_id = who.into();
        m.event = Some(MembershipEvent {
            actor_id: who.into(),
            r#type: MembershipEventType::Join,
            ..Default::default()
        });
        m
    };
    let f = open(
        cx,
        vec![
            sent("1", a, "hello"),
            join("2", b, "m-bob"),
            join("4", b, "m-eve"),
            sent("3", c, "again"),
        ],
    );
    let copied = copy_between(&f, cx, "text-1-0-block", "text-3-0-block");
    assert_eq!(
        copied,
        format!(
            "{}\n[{}] <bob> hello\n[{}] *** bob joined\n[{}] *** eve joined\n[{}] <bob> again",
            day(a),
            clock(a),
            clock(b),
            clock(b),
            clock(c)
        )
    );
}

#[gpui_kit::test]
fn blocks_of_one_message_copy_in_reading_order(cx: &mut TestAppContext) {
    let f = open(
        cx,
        vec![sent(
            "1",
            "2026-11-24T12:00:00Z",
            "intro line\n```\nfn main() {}\n```\nouro line",
        )],
    );
    let copied = copy_between(&f, cx, "text-1-0-block", "text-1-1-block");
    let (intro, code, outro) = (
        copied.find("intro line"),
        copied.find("fn main() {}"),
        copied.find("ouro line"),
    );
    assert!(
        intro < code && code < outro && intro.is_some(),
        "{copied:?}"
    );
}

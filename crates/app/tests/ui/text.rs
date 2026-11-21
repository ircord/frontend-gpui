//! Tests for package T1, message text: code blocks, quotes and lists, mention
//! and channel pills, joining a channel from its name, message-link pills,
//! and Markdown in the box sent as IRC formatting.

use std::time::Duration;

use gpui_kit::test::{TestEventExt, TestQueryExt, TestWindowExt};
use gpui_kit::{AppContext, MouseButton, TestAppContext};
use ircord_client::contract::*;
use ircord_client::testing::{CannedServer, Reply};
use serde_json::json;

use crate::fixture::{Fixture, message, open_with, snapshot, wait};

const WAIT: Duration = Duration::from_secs(5);

/// The server's origin in the fixture is the canned server's; message links
/// are built on it.
fn history(origin: &str) -> Vec<Message> {
    vec![
        message(
            "1",
            "c-general",
            "look:\n```rust\nfn main() {}\n```\n> quoted\n- one\n- two",
        ),
        message(
            "2",
            "c-general",
            &format!(
                "hey @me, ask @bob in #random or #newchan, @here. {origin}/channel/c-random?m=9"
            ),
        ),
    ]
}

fn open(cx: &mut TestAppContext, setup: impl FnOnce(&CannedServer)) -> Fixture {
    let f = open_with(cx, true, |server| {
        server.snapshot(&snapshot());
        let page = serde_json::to_value(MessagePage {
            items: history(&server.address().origin()),
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
        setup(server);
    });
    wait(cx, "#general's history", |cx| {
        let session = f.session.read(cx);
        let Some(client) = session.client() else {
            return false;
        };
        let w = client.read();
        session.conversation.as_deref() == Some("c-general")
            && w.timelines
                .get("c-general")
                .is_some_and(|t| t.loaded && t.entries.len() == 2)
    });
    // Pills are placed from the layout of the frame before.
    draw(&f, cx);
    f
}

fn draw(f: &Fixture, cx: &mut TestAppContext) {
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.render_frame(cx);
    })
    .unwrap();
    cx.run_until_parked();
}

fn click_label(f: &Fixture, cx: &mut TestAppContext, label: &str) {
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        let at = window.find_by_label(label).bounds().center();
        window.pointer_down(at, MouseButton::Left, cx);
        window.pointer_up(at, MouseButton::Left, cx);
    })
    .unwrap();
    cx.run_until_parked();
}

fn conversation(f: &Fixture, cx: &mut TestAppContext) -> Option<String> {
    cx.update(|cx| f.session.read(cx).conversation.clone())
}

#[gpui_kit::test]
fn code_blocks_quotes_lists_and_pills_are_drawn(cx: &mut TestAppContext) {
    let f = open(cx, |_| {});
    cx.update_window(f.window, |_, window, _| {
        let code = window.find("code-1-0");
        assert!(code.visible());
        assert!(window.find_by_label("rust code").visible());
        assert!(window.try_find("code-1-0-copy").is_some());
        assert!(window.try_find("list-1-0").is_some());
        assert!(window.try_find("list-1-0-1").is_some());
        // Your own nick, someone here and a group are pills; channels say
        // whether you are in them.
        assert!(window.find_by_label("Mention of me").visible());
        assert!(window.find_by_label("Mention of bob").visible());
        assert!(window.find_by_label("Mention of here").visible());
        assert!(window.find_by_label("Open #random").visible());
        assert!(window.find_by_label("Join #newchan").visible());
        assert!(window.find_by_label("Message in #random").visible());
    })
    .unwrap();
}

#[gpui_kit::test]
fn a_joined_channel_opens(cx: &mut TestAppContext) {
    let f = open(cx, |_| {});
    click_label(&f, cx, "Open #random");
    assert_eq!(conversation(&f, cx).as_deref(), Some("c-random"));
}

#[gpui_kit::test]
fn a_message_link_opens_its_conversation(cx: &mut TestAppContext) {
    let f = open(cx, |_| {});
    click_label(&f, cx, "Message in #random");
    assert_eq!(conversation(&f, cx).as_deref(), Some("c-random"));
}

#[gpui_kit::test]
fn a_channel_you_are_not_in_asks_before_joining(cx: &mut TestAppContext) {
    let f = open(cx, |server| {
        server.route_fn("POST", "/api/v1/conversations", |r| {
            let body = r.json();
            assert_eq!(body["networkId"], "n1");
            Reply::json(
                200,
                json!({"id": "c-new", "networkId": "n1", "kind": "channel",
                       "name": "newchan", "favorite": false, "joined": true,
                       "memberIds": [], "memberModes": {}, "modes": "", "topic": "",
                       "unreadCount": 0, "mentionCount": 0}),
            )
        });
    });
    click_label(&f, cx, "Join #newchan");
    // Nothing is joined until the question is answered.
    assert_eq!(conversation(&f, cx).as_deref(), Some("c-general"));
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("dialog").is_some());
        window.click("ok", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let body = f
        .server
        .wait_for("POST", "/api/v1/conversations", WAIT)
        .json();
    assert_eq!(body["name"], "#newchan");
    wait(cx, "the new channel to open", |cx| {
        f.session.read(cx).conversation.as_deref() == Some("c-new")
    });
}

#[gpui_kit::test]
fn markdown_in_the_box_is_sent_as_irc_formatting(cx: &mut TestAppContext) {
    let f = open(cx, |server| {
        server.route_fn("POST", "/api/v1/conversations/*/messages", |r| {
            let body = r.json();
            Reply::json(
                202,
                json!({"clientId": body["clientId"], "conversationId": "c-general",
                       "state": "queued"}),
            )
        });
    });
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.click("composer-input", cx);
        window.input("**bold** and `code`", cx);
        window.press("enter", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let body = f
        .server
        .wait_for("POST", "/api/v1/conversations/c-general/messages", WAIT)
        .json();
    assert_eq!(body["text"], "\x02bold\x02 and \x11code\x11");
}

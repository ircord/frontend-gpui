//! Sending text as the web client does: a Windows paste (`\r\n` line
//! breaks) goes out with plain `\n`, a long multi-paragraph paste goes out
//! whole for the server to split into a multiline batch, and a failed send
//! stays in the timeline with its reason and the web's buttons instead of a
//! bar over the message box.

use std::time::Duration;

use gpui_kit::test::{TestQueryExt, TestWindowExt};
use gpui_kit::{AppContext, ClipboardItem, TestAppContext};
use ircord_app::features::message_actions;
use ircord_client::contract::*;
use ircord_client::store::SendState;
use ircord_client::testing::{CannedServer, Reply, frame};
use serde_json::json;

use crate::fixture::{Fixture, message, open_with, snapshot, wait};

const WAIT: Duration = Duration::from_secs(5);
const SEND: &str = "/api/v1/conversations/c-general/messages";

/// A short quote of about 300 characters in paragraphs, copied on Windows,
/// so every line break is `\r\n`: the shape of text the server refused.
const PASTED: &str = "> Garden notes, all done by 09:30:\r\n\r\nWatered the tomatoes along the south fence.\r\n\r\nMoved the basil.\r\n\r\nPicked the last beans myself.\r\n\r\nCovered the lettuce again.\r\n\r\nNothing else changed; the shed is locked and the hose is back where it was before the rain started.";

/// The fixture's network with Ergo's multiline limits.
fn multiline() -> Snapshot {
    let mut s = snapshot();
    s.networks[0].features.multiline = Some(MultilineLimits {
        max_bytes: 4096,
        max_lines: 100,
    });
    s
}

fn open(cx: &mut TestAppContext, setup: impl FnOnce(&CannedServer)) -> Fixture {
    let f = open_with(cx, true, |server| {
        server.snapshot(&multiline());
        setup(server);
    });
    wait(cx, "#general's history and the live socket", |cx| {
        let session = f.session.read(cx);
        let Some(client) = session.client() else {
            return false;
        };
        let w = client.read();
        session.conversation.as_deref() == Some("c-general")
            && w.connection == Some(ircord_client::store::Connection::Live)
            && w.timelines
                .get("c-general")
                .is_some_and(|t| t.entries.len() == 2)
    });
    f
}

/// Accept every send and hold it as queued.
fn accept(server: &CannedServer) {
    server.route_fn("POST", SEND, |request| {
        let body = request.json();
        Reply::json(
            200,
            json!({"clientId": body["clientId"], "conversationId": "c-general",
                   "state": "queued"}),
        )
    });
}

fn draw(f: &Fixture, cx: &mut TestAppContext) {
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.render_frame(cx);
    })
    .unwrap();
    cx.run_until_parked();
}

fn click(f: &Fixture, cx: &mut TestAppContext, id: &str) {
    let id: &'static str = Box::leak(id.to_string().into_boxed_str());
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.click(id, cx);
    })
    .unwrap();
    cx.run_until_parked();
}

fn focus_box(f: &Fixture, cx: &mut TestAppContext) {
    click(f, cx, "composer-input");
}

fn press(f: &Fixture, cx: &mut TestAppContext, key: &str) {
    cx.update_window(f.window, |_, window, cx| window.press(key, cx))
        .unwrap();
    cx.run_until_parked();
}

fn type_text(f: &Fixture, cx: &mut TestAppContext, text: &str) {
    cx.update_window(f.window, |_, window, cx| window.input(text, cx))
        .unwrap();
    cx.run_until_parked();
    draw(f, cx);
}

fn shown(f: &Fixture, cx: &mut TestAppContext, id: &str) -> bool {
    let id: &'static str = Box::leak(id.to_string().into_boxed_str());
    let mut seen = false;
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        seen = window.try_find(id).is_some_and(|e| e.visible());
    })
    .unwrap();
    seen
}

/// The one send in #general, once it is in the outbox.
fn only_send(f: &Fixture, cx: &mut TestAppContext) -> String {
    cx.update(|cx| {
        let session = f.session.read(cx);
        let w = session.client().unwrap().read();
        let pending = w.pending_in("c-general");
        assert_eq!(pending.len(), 1, "one send");
        pending[0].client_id.clone()
    })
}

#[gpui_kit::test]
fn a_quote_pasted_on_windows_sends_with_plain_line_breaks(cx: &mut TestAppContext) {
    let f = open(cx, accept);
    cx.update(|cx| cx.write_to_clipboard(ClipboardItem::new_string(PASTED.into())));
    focus_box(&f, cx);
    press(&f, cx, "ctrl-v");
    draw(&f, cx);
    assert!(
        !shown(&f, cx, "composer-limit"),
        "a 300-character quote is well within the limits"
    );
    press(&f, cx, "enter");
    let sent = f.server.wait_for("POST", SEND, WAIT).json();
    let text = sent["text"].as_str().unwrap();
    assert!(
        !text.contains('\r'),
        "the server refuses any carriage return"
    );
    assert_eq!(text, PASTED.replace("\r\n", "\n"));
}

#[gpui_kit::test]
fn pasted_bullets_use_markdown_and_replace_the_selection_in_one_edit(cx: &mut TestAppContext) {
    let f = open(cx, accept);
    let select = if cfg!(target_os = "macos") {
        "cmd-a"
    } else {
        "ctrl-a"
    };
    let paste = if cfg!(target_os = "macos") {
        "cmd-v"
    } else {
        "ctrl-v"
    };
    let undo = if cfg!(target_os = "macos") {
        "cmd-z"
    } else {
        "ctrl-z"
    };
    focus_box(&f, cx);
    type_text(&f, cx, "Earlier draft");
    cx.update(|cx| {
        cx.write_to_clipboard(ClipboardItem::new_string("• parent\r\n  ◦ child".into()))
    });
    press(&f, cx, select);
    press(&f, cx, paste);
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find_by_label("Message #general").value(),
            Some("- parent\n  - child")
        );
    })
    .unwrap();
    press(&f, cx, undo);
    cx.update(|cx| assert_eq!(message_actions::box_text(cx), "Earlier draft"));
    press(&f, cx, select);
    press(&f, cx, paste);
    press(&f, cx, "enter");
    let sent = f.server.wait_for("POST", SEND, WAIT).json();
    assert_eq!(sent["text"], "- parent\n  - child");
}

#[gpui_kit::test]
fn typed_carriage_returns_never_reach_the_server(cx: &mut TestAppContext) {
    let f = open(cx, accept);
    focus_box(&f, cx);
    type_text(&f, cx, "first\r\nsecond\rthird");
    press(&f, cx, "enter");
    let sent = f.server.wait_for("POST", SEND, WAIT).json();
    assert_eq!(sent["text"], "first\nsecond\nthird");
}

#[gpui_kit::test]
fn a_long_paste_goes_out_whole_for_the_server_to_split(cx: &mut TestAppContext) {
    let f = open(cx, accept);
    // Three paragraphs, one of them far longer than a 512-byte IRC line:
    // the server sends it as one multiline batch, cutting long lines with
    // `draft/multiline-concat`, as it does for the web client.
    let long = "word ".repeat(300);
    let text = format!("Opening paragraph.\r\n\r\n{long}\r\n\r\nClosing paragraph.");
    cx.update(|cx| cx.write_to_clipboard(ClipboardItem::new_string(text.clone())));
    focus_box(&f, cx);
    press(&f, cx, "ctrl-v");
    draw(&f, cx);
    assert!(!shown(&f, cx, "composer-limit"));
    press(&f, cx, "enter");
    let sent = f.server.wait_for("POST", SEND, WAIT).json();
    assert_eq!(sent["text"], text.replace("\r\n", "\n"));
    assert_eq!(
        f.server.requests_to("POST", SEND).len(),
        1,
        "one message, not one per line"
    );
}

#[gpui_kit::test]
fn a_failed_send_stays_in_the_timeline_with_the_webs_buttons(cx: &mut TestAppContext) {
    let f = open(cx, |server| {
        server.route(
            "POST",
            SEND,
            400,
            json!({"title": "Bad Request", "detail": "The network refused it."}),
        );
        server.route_reply("DELETE", "/api/v1/commands/*", Reply::empty());
    });
    focus_box(&f, cx);
    type_text(&f, cx, "doomed **words**");
    press(&f, cx, "enter");
    let id = only_send(&f, cx);
    wait(cx, "the send to fail", |cx| {
        let session = f.session.read(cx);
        let w = session.client().unwrap().read();
        w.pending_in("c-general")
            .iter()
            .all(|p| matches!(p.state, SendState::Failed(_)))
    });
    draw(&f, cx);

    // One row in the timeline holds the text, the reason and the buttons;
    // nothing is drawn over the message box.
    assert!(shown(&f, cx, &format!("pending-{id}")));
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        let status: &'static str = Box::leak(format!("pending-status-{id}").into_boxed_str());
        assert_eq!(window.find(status).label(), Some("The network refused it."));
    })
    .unwrap();
    for button in [
        "retry",
        "failed-send-to-box",
        "failed-send-copy",
        "failed-send-discard",
    ] {
        assert!(shown(&f, cx, &format!("{button}-{id}")), "{button}");
    }
    assert!(!shown(&f, cx, &format!("failed-send-{id}")));
    // It follows someone else's message: your name and its time head it,
    // and the text reads without IRC formatting codes.
    assert!(shown(&f, cx, &format!("pending-sender-{id}")));
    assert!(shown(&f, cx, &format!("pending-time-{id}")));
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        let row: &'static str = Box::leak(format!("pending-{id}").into_boxed_str());
        assert_eq!(window.find(row).label(), Some("doomed words"));
    })
    .unwrap();
    assert!(!shown(&f, cx, "composer-mode"));

    // A message arriving later is drawn above it: like the web client, the
    // outbox stays at the end of the conversation until it is resolved.
    f.server.push(frame(
        4,
        "message.created",
        &message("3", "c-general", "a later message"),
    ));
    wait(cx, "the later message", |cx| {
        let session = f.session.read(cx);
        let w = session.client().unwrap().read();
        w.timelines["c-general"].entries.len() == 3
    });
    draw(&f, cx);
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        let row: &'static str = Box::leak(format!("pending-{id}").into_boxed_str());
        assert!(window.find("message-3").bounds().bottom() <= window.find(row).bounds().top());
        assert!(
            window.find(row).bounds().bottom() <= window.find("composer-input").bounds().top(),
            "the failed row scrolls with the timeline, above the box"
        );
    })
    .unwrap();

    // Copy text, then Remove from outbox after confirming.
    click(&f, cx, &format!("failed-send-copy-{id}"));
    assert_eq!(
        cx.read_from_clipboard().and_then(|c| c.text()).as_deref(),
        Some("doomed **words**"),
        "copied as typed, Markdown and all"
    );
    click(&f, cx, &format!("failed-send-discard-{id}"));
    click(&f, cx, "ok");
    f.server
        .wait_for("DELETE", &format!("/api/v1/commands/{id}"), WAIT);
    cx.update(|cx| {
        let session = f.session.read(cx);
        assert!(session.client().unwrap().read().pending(&id).is_none());
        assert_eq!(message_actions::box_text(cx), "");
    });
    assert!(!shown(&f, cx, &format!("pending-{id}")));
}

#[gpui_kit::test]
fn move_to_draft_puts_a_failed_send_back_in_the_box(cx: &mut TestAppContext) {
    let f = open(cx, |server| {
        server.route(
            "POST",
            SEND,
            400,
            json!({"detail": "The network refused it."}),
        );
        server.route_reply("DELETE", "/api/v1/commands/*", Reply::empty());
    });
    focus_box(&f, cx);
    type_text(&f, cx, "try **again**");
    press(&f, cx, "enter");
    let id = only_send(&f, cx);
    wait(cx, "the send to fail", |cx| {
        let session = f.session.read(cx);
        let w = session.client().unwrap().read();
        w.pending_in("c-general")
            .iter()
            .all(|p| matches!(p.state, SendState::Failed(_)))
    });
    click(&f, cx, &format!("failed-send-to-box-{id}"));
    f.server
        .wait_for("DELETE", &format!("/api/v1/commands/{id}"), WAIT);
    cx.update(|cx| {
        assert_eq!(
            message_actions::box_text(cx),
            "try **again**",
            "back as typed, Markdown and all"
        );
        let session = f.session.read(cx);
        assert!(session.client().unwrap().read().pending(&id).is_none());
    });
}

#[gpui_kit::test]
fn a_second_send_right_after_the_first_has_no_header(cx: &mut TestAppContext) {
    let f = open(cx, |server| {
        server.route(
            "POST",
            SEND,
            400,
            json!({"detail": "The network refused it."}),
        );
    });
    focus_box(&f, cx);
    type_text(&f, cx, "first");
    press(&f, cx, "enter");
    type_text(&f, cx, "second");
    press(&f, cx, "enter");
    let ids: Vec<String> = cx.update(|cx| {
        let session = f.session.read(cx);
        let w = session.client().unwrap().read();
        w.pending_in("c-general")
            .iter()
            .map(|p| p.client_id.clone())
            .collect()
    });
    assert_eq!(ids.len(), 2);
    draw(&f, cx);
    assert!(shown(&f, cx, &format!("pending-time-{}", ids[0])));
    assert!(
        !shown(&f, cx, &format!("pending-time-{}", ids[1])),
        "it continues your message above, as on the web"
    );
}

#[gpui_kit::test]
fn a_long_windows_paste_is_measured_as_it_will_be_sent(cx: &mut TestAppContext) {
    let f = open(cx, accept);
    // 100 lines of 39 characters: 4098 characters as pasted with `\r\n`,
    // 3999 once the line breaks are plain, within every limit.
    let line = "x".repeat(39);
    let text = vec![line; 100].join("\r\n");
    assert_eq!(text.chars().count(), 4098);
    cx.update(|cx| cx.write_to_clipboard(ClipboardItem::new_string(text.clone())));
    focus_box(&f, cx);
    press(&f, cx, "ctrl-v");
    draw(&f, cx);
    assert!(
        !shown(&f, cx, "composer-limit"),
        "no length warning for text that fits once sent"
    );
    press(&f, cx, "enter");
    let sent = f.server.wait_for("POST", SEND, WAIT).json();
    assert_eq!(sent["text"].as_str().unwrap().chars().count(), 3999);
}

//! Tests for package A1: message actions against the canned server — reply,
//! edit with Up, react, delete, unavailable actions with their reason, and
//! failed sends restored from the outbox and moved back to the box.

use std::time::Duration;

use gpui_kit::test::{TestEventExt, TestQueryExt, TestWindowExt};
use gpui_kit::{AppContext, MouseButton, TestAppContext};
use ircord_app::features::message_actions::{self, Mode};
use ircord_client::contract::*;
use ircord_client::testing::{CannedServer, Reply};
use serde_json::{Value, json};

use crate::fixture::{Fixture, message, open_with, snapshot, wait};

const WAIT: Duration = Duration::from_secs(5);

/// The fixture's snapshot on a network that offers every message action,
/// signed in to the account `me`.
fn capable() -> Snapshot {
    let mut s = snapshot();
    let n = &mut s.networks[0];
    n.account = Some("me".into());
    n.features.edit = true;
    n.features.reactions = true;
    n.features.replies = true;
    n.features.redact = true;
    s
}

/// bob's message 1 (with a network ID), bob's 2 without one, your own 3
/// and bob's 4 replying to it.
fn history() -> Vec<Message> {
    let mut one = message("1", "c-general", "hello there");
    one.upstream_id = Some("up-1".into());
    let two = message("2", "c-general", "no network id");
    let mut three = message("3", "c-general", "my words");
    three.sender_id = "u-me".into();
    three.account = Some("me".into());
    three.upstream_id = Some("up-3".into());
    let mut four = message("4", "c-general", "a reply");
    four.upstream_id = Some("up-4".into());
    four.reply_to = Some(Reply_ {
        id: "3".into(),
        nick: "me".into(),
        text: "my words".into(),
    });
    vec![one, two, three, four]
}

type Reply_ = ircord_client::contract::Reply;

fn serve(server: &CannedServer, snapshot: Snapshot, history: Vec<Message>) {
    server.snapshot(&snapshot);
    let page = serde_json::to_value(MessagePage {
        items: history,
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
}

fn open(cx: &mut TestAppContext, setup: impl FnOnce(&CannedServer)) -> Fixture {
    let f = open_with(cx, true, |server| {
        serve(server, capable(), history());
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
                .is_some_and(|t| t.entries.len() == 4)
    });
    f
}

fn stored(f: &Fixture, cx: &mut TestAppContext, id: &str) -> Option<Message> {
    cx.update(|cx| {
        let session = f.session.read(cx);
        let w = session.client().unwrap().read();
        w.message("c-general", id).cloned()
    })
}

/// Right-click a message and let its menu open.
fn open_menu(f: &Fixture, cx: &mut TestAppContext, id: &'static str) {
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.hover(id, cx);
        window.right_click(id, cx);
    })
    .unwrap();
    cx.run_until_parked();
}

/// Click the menu entry labelled `label`.
fn pick(f: &Fixture, cx: &mut TestAppContext, label: &str) {
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        let item = window.find_by_label(label).bounds().center();
        window.pointer_down(item, MouseButton::Left, cx);
        window.pointer_up(item, MouseButton::Left, cx);
    })
    .unwrap();
    cx.run_until_parked();
}

fn answered(server: &CannedServer, method: &str, path: &str) -> Value {
    server.wait_for(method, path, WAIT).json()
}

#[gpui_kit::test]
fn reply_from_the_toolbar_sends_with_the_quoted_message(cx: &mut TestAppContext) {
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
        window.hover("message-1", cx);
        window.click("message-toolbar-reply", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update(|cx| {
        assert!(matches!(
            message_actions::mode("c-general", cx),
            Some(Mode::Reply { message, nick, .. }) if message == "1" && nick == "bob"
        ));
    });
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.find_by_label("Replying to bob").visible());
        window.input("right back", cx);
        window.press("enter", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let body = answered(
        &f.server,
        "POST",
        "/api/v1/conversations/c-general/messages",
    );
    assert_eq!(body["replyToId"], "1");
    assert_eq!(body["text"], "right back");
    cx.update(|cx| assert_eq!(message_actions::mode("c-general", cx), None));
}

#[gpui_kit::test]
fn escape_stops_replying(cx: &mut TestAppContext) {
    let f = open(cx, |_| {});
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.hover("message-1", cx);
        window.click("message-toolbar-reply", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.press("escape", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update(|cx| assert_eq!(message_actions::mode("c-general", cx), None));
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("composer-mode").is_none());
    })
    .unwrap();
}

#[gpui_kit::test]
fn up_in_an_empty_box_edits_your_newest_message(cx: &mut TestAppContext) {
    let f = open(cx, |server| {
        server.route_fn("PATCH", "/api/v1/conversations/*/messages/*", |r| {
            let mut edited = history()[2].clone();
            edited.text = r.json()["text"].as_str().unwrap().to_string();
            edited.edited_at = Some("2026-11-21T10:05:00Z".into());
            Reply::json(200, json!({"message": edited}))
        });
    });
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.click("composer-input", cx);
        window.press("up", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update(|cx| {
        assert!(matches!(
            message_actions::mode("c-general", cx),
            Some(Mode::Edit { message, .. }) if message == "3"
        ));
        assert_eq!(message_actions::box_text(cx), "my words");
    });
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.find_by_label("Editing your message").visible());
        window.input(", fixed", cx);
        window.press("enter", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let body = answered(
        &f.server,
        "PATCH",
        "/api/v1/conversations/c-general/messages/3",
    );
    assert_eq!(body["text"], "my words, fixed");
    wait(cx, "the edit in the timeline", |cx| {
        let session = f.session.read(cx);
        let w = session.client().unwrap().read();
        w.message("c-general", "3")
            .is_some_and(|m| m.text == "my words, fixed" && m.edited_at.is_some())
    });
    cx.update(|cx| {
        assert_eq!(message_actions::mode("c-general", cx), None);
        assert_eq!(message_actions::box_text(cx), "", "the empty draft is back");
    });
}

#[gpui_kit::test]
fn react_from_the_toolbar_and_take_it_back_from_the_chip(cx: &mut TestAppContext) {
    let f = open(cx, |server| {
        server.route_fn("POST", "/api/v1/conversations/*/reactions", |r| {
            let body = r.json();
            let mut m = history()[0].clone();
            m.reactions = vec![ircord_client::contract::Reaction {
                emoji: body["emoji"].as_str().unwrap().into(),
                member_ids: vec!["u-me".into()],
            }];
            Reply::json(200, serde_json::to_value(m).unwrap())
        });
    });
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.hover("message-1", cx);
        window.click("message-toolbar-react", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.find("quick-reactions").visible());
        window.click("quick-reaction-0", cx);
    })
    .unwrap();
    let body = answered(
        &f.server,
        "POST",
        "/api/v1/conversations/c-general/reactions",
    );
    assert_eq!(body, json!({"emoji": "👍", "messageId": "1"}));
    wait(cx, "the reaction on message 1", |cx| {
        let session = f.session.read(cx);
        let w = session.client().unwrap().read();
        w.message("c-general", "1")
            .is_some_and(|m| m.reactions.len() == 1)
    });
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find("quick-reactions").is_none(),
            "the picker closed"
        );
        let chip = window.find("reaction-1-👍");
        assert!(chip.visible());
        window.click("reaction-1-👍", cx);
    })
    .unwrap();
    f.server
        .wait_for_nth("POST", "/api/v1/conversations/c-general/reactions", 2, WAIT);
}

#[gpui_kit::test]
fn unavailable_actions_say_why(cx: &mut TestAppContext) {
    let f = open(cx, |_| {});
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        // Message 2 has no network message ID: nothing can act on it.
        window.hover("message-2", cx);
        window.render_frame(cx);
        assert!(window.try_find("message-toolbar-reply").is_none());
        assert!(window.try_find("message-toolbar-react").is_none());
    })
    .unwrap();
    open_menu(&f, cx, "message-2");
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.find("menu-unavailable-reply").visible());
        assert!(window.find("menu-unavailable-react").visible());
        let why = window.find("menu-unavailable-reason-reply");
        assert!(why.visible());
        assert_eq!(
            why.label(),
            Some("This message has no usable network message ID.")
        );
        // Editing and deleting are for your own messages only.
        assert!(window.try_find("menu-unavailable-edit").is_none());
        assert!(window.try_find_by_label("Edit").is_none());
        assert!(window.try_find_by_label("Delete message").is_none());
        assert!(window.find_by_label("Copy text").visible());
    })
    .unwrap();
    // On a network without replies the entry is not offered at all.
    let mut no_replies = capable();
    no_replies.networks[0].features.replies = false;
    f.server.snapshot(&no_replies);
    cx.update(|cx| {
        let session = f.session.read(cx);
        let client = session.client().unwrap();
        client.change(|w| w.reset(no_replies.clone()));
    });
    cx.run_until_parked();
    wait(cx, "history after the reset", |cx| {
        let session = f.session.read(cx);
        let w = session.client().unwrap().read();
        w.timelines
            .get("c-general")
            .is_some_and(|t| t.entries.len() == 4)
    });
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.hover("message-1", cx);
        assert!(window.try_find("message-toolbar-reply").is_none());
        assert!(window.find("message-toolbar-react").visible());
    })
    .unwrap();
}

#[gpui_kit::test]
fn delete_asks_then_the_message_leaves_and_replies_quote_it_as_deleted(cx: &mut TestAppContext) {
    let f = open(cx, |server| {
        server.route_fn("DELETE", "/api/v1/conversations/*/messages/*", |_| {
            let mut gone = history()[2].clone();
            gone.deleted = Some(true);
            gone.text = String::new();
            Reply::json(
                200,
                json!({"attachmentRetained": false, "authority": "author", "message": gone}),
            )
        });
    });
    open_menu(&f, cx, "message-3");
    pick(&f, cx, "Delete message");
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        // The confirmation, with Cancel beside Delete.
        assert!(window.find_by_label("Cancel").visible());
        let ok = window.find_by_label("Delete").bounds().center();
        window.pointer_down(ok, MouseButton::Left, cx);
        window.pointer_up(ok, MouseButton::Left, cx);
    })
    .unwrap();
    f.server
        .wait_for("DELETE", "/api/v1/conversations/c-general/messages/3", WAIT);
    wait(cx, "message 3 to leave the timeline", |cx| {
        let session = f.session.read(cx);
        let w = session.client().unwrap().read();
        w.message("c-general", "3").is_none()
    });
    let reply = stored(&f, cx, "4").unwrap().reply_to.unwrap();
    assert_eq!(reply.text, ircord_client::store::outbox::DELETED_QUOTE);
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("message-3").is_none());
        assert!(window.find("message-4").visible());
    })
    .unwrap();
}

#[gpui_kit::test]
fn a_failed_send_from_the_outbox_goes_back_into_the_box(cx: &mut TestAppContext) {
    let f = open(cx, |server| {
        server.route(
            "GET",
            "/api/v1/outbox",
            200,
            json!({"more": false, "items": [{
                "clientId": "lost-1", "conversationId": "c-general",
                "createdAt": "2026-11-21T09:00:00Z", "state": "failed",
                "error": "The network refused it.",
                "payload": {"clientId": "lost-1", "conversationId": "c-general",
                            "text": "lost words", "kind": "message"}
            }]}),
        );
        server.route_reply("DELETE", "/api/v1/commands/*", Reply::empty());
    });
    wait(cx, "the failed send restored from the outbox", |cx| {
        let session = f.session.read(cx);
        let w = session.client().unwrap().read();
        w.pending("lost-1").is_some_and(|p| {
            p.state == ircord_client::store::SendState::Failed("The network refused it.".into())
        })
    });
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.find("pending-lost-1").visible());
        window.click("failed-send-to-box-lost-1", cx);
    })
    .unwrap();
    cx.run_until_parked();
    f.server.wait_for("DELETE", "/api/v1/commands/lost-1", WAIT);
    cx.update(|cx| {
        assert_eq!(message_actions::box_text(cx), "lost words");
        let session = f.session.read(cx);
        assert!(session.client().unwrap().read().pending("lost-1").is_none());
    });
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("pending-lost-1").is_none());
    })
    .unwrap();
}

#[gpui_kit::test]
fn earlier_versions_and_copy(cx: &mut TestAppContext) {
    let mut versions = history();
    versions[2].edits = Some(vec![
        MessageEdit {
            at: "2026-11-21T10:01:00Z".into(),
            text: "my wrds".into(),
            upstream_id: "e1".into(),
        },
        MessageEdit {
            at: "2026-11-21T10:02:00Z".into(),
            text: "my words".into(),
            upstream_id: "e2".into(),
        },
    ]);
    versions[2].original_text = Some("my wrd".into());
    versions[2].edited_at = Some("2026-11-21T10:02:00Z".into());
    let f = open(cx, move |server| serve(server, capable(), versions));
    open_menu(&f, cx, "message-3");
    pick(&f, cx, "Show earlier versions");
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.find("versions-3").visible());
        assert!(window.find_by_label("my wrd").visible());
        assert!(window.find_by_label("my wrds").visible());
        assert!(
            window.try_find("version-2").is_none(),
            "the current text is not listed"
        );
    })
    .unwrap();
    cx.update_window(f.window, |_, window, cx| {
        gpui_kit::component::WindowExt::close_dialog(window, cx);
    })
    .unwrap();
    cx.run_until_parked();
    open_menu(&f, cx, "message-1");
    pick(&f, cx, "Copy link to message");
    let copied = cx.read_from_clipboard().and_then(|c| c.text());
    let origin = f.server.address().origin();
    assert_eq!(
        copied.as_deref(),
        Some(format!("{origin}/channel/c-general?m=n1%3Aup-1").as_str())
    );
}

#[gpui_kit::test]
fn the_message_actions_button_opens_the_menu_drawn_as_the_webs(cx: &mut TestAppContext) {
    let f = open(cx, |_| {});
    // Your own message: its menu ends with Delete, a destructive row.
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.hover("message-3", cx);
        window.render_frame(cx);
        assert!(window.find("message-actions-menu").visible());
        window.click("message-actions-menu", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.find_by_label("Copy text").visible());
        // Drawn in the danger colour, so through the destructive path.
        assert!(window.find("menu-danger-delete").visible());
        // The pointer leaving the row for the menu keeps it open.
        window.hover("message-1", cx);
        window.render_frame(cx);
        assert!(window.find_by_label("Copy text").visible());
    })
    .unwrap();
}

#[gpui_kit::test]
fn the_conversation_menu_names_its_channel_and_marks_leaving(cx: &mut TestAppContext) {
    let f = open(cx, |_| {});
    let named = |f: &Fixture, cx: &mut TestAppContext| {
        cx.update_window(f.window, |_, window, cx| {
            window.render_frame(cx);
            window
                .find_all_by_label("#general")
                .into_iter()
                .filter(|e| e.visible())
                .count()
        })
        .unwrap()
    };
    let before = named(&f, cx);
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.click("conversation-menu", cx);
    })
    .unwrap();
    cx.run_until_parked();
    // The title row names the channel, as on the web.
    assert!(named(&f, cx) > before);
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.find("menu-danger-sidebar-leave").visible());
    })
    .unwrap();
}

#[gpui_kit::test]
fn every_menu_row_has_an_icon_and_groups_are_separated(cx: &mut TestAppContext) {
    let f = open(cx, |_| {});
    cx.update(|cx| {
        let r = ircord_app::registry::get(cx);
        let mut missing = Vec::new();
        let mut check = |menu: &str, items: Vec<(String, bool)>| {
            for (id, has_icon) in items {
                // The quick reactions show their emoji in the label.
                if !has_icon && !id.starts_with("react-") {
                    missing.push(format!("{menu}: {id}"));
                }
            }
        };
        check(
            "message",
            r.message_menu
                .iter()
                .map(|i| (i.id.to_string(), i.icon.is_some()))
                .collect(),
        );
        // The notification levels are a submenu of choices with a check
        // mark beside the chosen one, as the web's.
        check(
            "conversation",
            r.conversation_menu
                .iter()
                .map(|i| (i.id.to_string(), i.icon.is_some() || i.parent.is_some()))
                .collect(),
        );
        check(
            "member",
            r.member_menu
                .iter()
                .map(|i| (i.id.to_string(), i.icon.is_some()))
                .collect(),
        );
        check(
            "network",
            r.network_menu
                .iter()
                .map(|i| (i.id.to_string(), i.icon.is_some()))
                .collect(),
        );
        check(
            "self",
            r.self_menu
                .iter()
                .map(|i| (i.id.to_string(), i.icon.is_some()))
                .collect(),
        );
        assert!(missing.is_empty(), "menu rows without an icon: {missing:?}");

        // A separator where the hundreds change and before the first
        // destructive row.
        let items = r.conversation_menu.to_vec();
        let breaks = ircord_app::ui::context_menu::separators(&items);
        for (i, item) in items.iter().enumerate().skip(1) {
            let prev = &items[i - 1];
            let expected = prev.order / 100 != item.order / 100 || (item.danger && !prev.danger);
            assert_eq!(breaks[i], expected, "{}", item.id);
        }
    });
    drop(f);
}

/// [`capable`] with Ergo's multiline limits, so line breaks can be sent.
fn multiline(server: &CannedServer) {
    let mut s = capable();
    s.networks[0].features.multiline = Some(MultilineLimits {
        max_bytes: 4096,
        max_lines: 100,
    });
    serve(server, s, history());
}

#[gpui_kit::test]
fn a_reply_pasted_on_windows_goes_out_with_plain_line_breaks(cx: &mut TestAppContext) {
    let f = open(cx, |server| {
        multiline(server);
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
        window.hover("message-1", cx);
        window.click("message-toolbar-reply", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.input("right\r\nback", cx);
        window.press("enter", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let body = answered(
        &f.server,
        "POST",
        "/api/v1/conversations/c-general/messages",
    );
    assert_eq!(body["replyToId"], "1");
    assert_eq!(body["text"], "right\nback");
}

#[gpui_kit::test]
fn an_edit_pasted_on_windows_goes_out_with_plain_line_breaks(cx: &mut TestAppContext) {
    let f = open(cx, |server| {
        multiline(server);
        server.route_fn("PATCH", "/api/v1/conversations/*/messages/*", |r| {
            let mut edited = history()[2].clone();
            edited.text = r.json()["text"].as_str().unwrap().to_string();
            Reply::json(200, json!({"message": edited}))
        });
    });
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.click("composer-input", cx);
        window.press("up", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.input("\r\nfixed", cx);
        window.press("enter", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let body = answered(
        &f.server,
        "PATCH",
        "/api/v1/conversations/c-general/messages/3",
    );
    assert_eq!(body["text"], "my words\nfixed");
}

#[gpui_kit::test]
fn a_failed_reply_or_gif_cannot_move_to_the_draft(cx: &mut TestAppContext) {
    let f = open(cx, |server| {
        server.route(
            "GET",
            "/api/v1/outbox",
            200,
            json!({"more": false, "items": [
                {"clientId": "reply-1", "conversationId": "c-general",
                 "createdAt": "2026-11-21T09:00:00Z", "state": "failed", "error": "No.",
                 "payload": {"clientId": "reply-1", "conversationId": "c-general",
                             "text": "a reply", "kind": "message", "replyToId": "1"}},
                {"clientId": "plain-1", "conversationId": "c-general",
                 "createdAt": "2026-11-21T09:01:00Z", "state": "failed", "error": "No.",
                 "payload": {"clientId": "plain-1", "conversationId": "c-general",
                             "text": "plain words", "kind": "message"}}
            ]}),
        );
    });
    wait(cx, "both failed sends restored", |cx| {
        let session = f.session.read(cx);
        let w = session.client().unwrap().read();
        w.pending("reply-1").is_some() && w.pending("plain-1").is_some()
    });
    cx.update(|cx| {
        let session = f.session.read(cx);
        let w = session.client().unwrap().read();
        assert_eq!(
            w.pending("reply-1").unwrap().reply_to_id.as_deref(),
            Some("1")
        );
    });
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.find("retry-reply-1").visible());
        assert!(
            window.try_find("failed-send-to-box-reply-1").is_none(),
            "the box cannot carry the reply, as on the web"
        );
        assert!(window.find("failed-send-to-box-plain-1").visible());
    })
    .unwrap();
}

#[gpui_kit::test]
fn a_failed_edit_gives_back_the_markdown_you_typed(cx: &mut TestAppContext) {
    let f = open(cx, |server| {
        server.route_fn("PATCH", "/api/v1/conversations/*/messages/*", |_| {
            Reply::json(500, json!({"error": "no"}))
        });
    });
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.click("composer-input", cx);
        window.press("up", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.input(", **fixed**", cx);
        window.press("enter", cx);
    })
    .unwrap();
    cx.run_until_parked();
    wait(cx, "the edit to come back to the box", |cx| {
        matches!(
            message_actions::mode("c-general", cx),
            Some(Mode::Edit { .. })
        ) && message_actions::box_text(cx) == "my words, **fixed**"
    });
}

//! Tests for package FD2: the inbox and saved messages, against the canned
//! server.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use gpui_kit::test::{TestEventExt, TestQueryExt, TestWindowExt};
use gpui_kit::{AppContext, MouseButton, TestAppContext};
use ircord_app::route::Route;
use ircord_client::store::Connection;
use ircord_client::store::inbox::Filter;
use ircord_client::testing::{CannedServer, Reply, frame};
use serde_json::{Value, json};

use crate::fixture::{Fixture, open_with, wait};

const WAIT: Duration = crate::fixture::HANG_GUARD;

/// Open the app signed in, with `setup`'s routes, and wait until it is live
/// with `#general`'s history shown.
fn open(cx: &mut TestAppContext, setup: impl FnOnce(&CannedServer)) -> Fixture {
    let f = open_with(cx, true, setup);
    wait(cx, "the live socket and #general's history", |cx| {
        let session = f.session.read(cx);
        let Some(client) = session.client() else {
            return false;
        };
        let w = client.read();
        session.conversation.as_deref() == Some("c-general")
            && w.connection == Some(Connection::Live)
            && w.timelines
                .get("c-general")
                .is_some_and(|t| t.loaded && t.entries.len() == 2)
    });
    f
}

fn click(f: &Fixture, cx: &mut TestAppContext, id: &'static str) {
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.click(id, cx);
    })
    .unwrap();
    cx.run_until_parked();
}

fn label(f: &Fixture, cx: &mut TestAppContext, id: &'static str) -> Option<String> {
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window
            .try_find(id)
            .and_then(|e| e.label().map(str::to_string))
    })
    .unwrap()
}

fn shows(f: &Fixture, cx: &mut TestAppContext, id: &'static str) -> bool {
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.try_find(id).is_some()
    })
    .unwrap()
}

fn has_label(f: &Fixture, cx: &mut TestAppContext, label: &str) -> bool {
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.try_find_by_label(label).is_some()
    })
    .unwrap()
}

/// Right-click a message, then click the menu entry labelled `label`.
fn menu(f: &Fixture, cx: &mut TestAppContext, message: &'static str, label: &str) {
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.hover(message, cx);
        window.right_click(message, cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        let item = window.find_by_label(label).bounds().center();
        window.pointer_down(item, MouseButton::Left, cx);
        window.pointer_up(item, MouseButton::Left, cx);
    })
    .unwrap();
    cx.run_until_parked();
}

fn saved_item(saved_at: &str) -> Value {
    json!({
        "conversationId": "c-general", "messageId": "1", "networkId": "n1",
        "savedAt": saved_at, "status": "available",
        "message": {"id": "1", "conversationId": "c-general", "senderId": "m-bob",
                    "createdAt": "2026-11-21T10:00:00Z", "kind": "message",
                    "text": "hello \u{2}world\u{2} https://ircord.com", "reactions": []}
    })
}

#[gpui_kit::test]
fn save_a_message_from_its_menu_and_find_it_on_the_saved_page(cx: &mut TestAppContext) {
    let saved = Arc::new(AtomicBool::new(false));
    let f = open(cx, |server| {
        server.route(
            "GET",
            "/api/v1/saved-message-ids",
            200,
            json!({"items": []}),
        );
        let on_put = saved.clone();
        server.route_fn("PUT", "/api/v1/saved-messages/1", move |_| {
            on_put.store(true, Ordering::SeqCst);
            Reply::json(200, saved_item("2026-11-21T11:00:00Z"))
        });
        let on_get = saved.clone();
        server.route_fn("GET", "/api/v1/saved-messages", move |_| {
            let items = if on_get.load(Ordering::SeqCst) {
                vec![saved_item("2026-11-21T11:00:00Z")]
            } else {
                vec![]
            };
            Reply::json(200, json!({ "items": items }))
        });
    });

    menu(&f, cx, "message-1", "Save for later");
    let put = f.server.wait_for("PUT", "/api/v1/saved-messages/1", WAIT);
    assert_eq!(put.json(), json!({"conversationId": "c-general"}));
    wait(cx, "the saved mark", |cx| {
        let session = f.session.read(cx);
        let w = session.client().unwrap().read();
        w.saved.is_saved("1") && w.saved.pending.is_empty()
    });

    click(&f, cx, "open-saved");
    wait(cx, "the saved messages page", |cx| {
        f.session.read(cx).route() == Some(Route::Saved)
    });
    wait(cx, "the saved message listed", |cx| {
        let session = f.session.read(cx);
        let w = session.client().unwrap().read();
        w.saved.list.loaded && w.saved.list.items.len() == 1
    });
    assert_eq!(
        label(&f, cx, "saved-item-1").as_deref(),
        Some("bob in #general: hello world https://ircord.com")
    );
    assert_eq!(label(&f, cx, "saved-meta").as_deref(), Some("1 saved"));

    // Opening it goes back to the message in its conversation.
    click(&f, cx, "saved-item-1");
    wait(cx, "#general again", |cx| {
        f.session.read(cx).route() == Some(Route::Conversation("c-general".into()))
    });
    assert!(shows(&f, cx, "message-1"));
}

#[gpui_kit::test]
fn the_saved_page_filters_and_removes(cx: &mut TestAppContext) {
    let f = open(cx, |server| {
        server.route(
            "GET",
            "/api/v1/saved-message-ids",
            200,
            json!({"items": [{"conversationId": "c-general", "messageId": "1",
                              "networkId": "n1", "savedAt": "2026-11-21T11:00:00Z"}]}),
        );
        server.route_fn("GET", "/api/v1/saved-messages", |r| {
            let items = match r.query_param("q").as_deref() {
                Some("nothing") => vec![],
                _ => vec![saved_item("2026-11-21T11:00:00Z")],
            };
            Reply::json(200, json!({ "items": items }))
        });
        server.route_reply("DELETE", "/api/v1/saved-messages/1", Reply::empty());
    });
    wait(cx, "the saved IDs", |cx| {
        let session = f.session.read(cx);
        session.client().unwrap().read().saved.ids_loaded
    });
    // A saved message offers to be removed instead.
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.hover("message-1", cx);
        window.right_click("message-1", cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert!(has_label(&f, cx, "Remove from saved"));
    assert!(!has_label(&f, cx, "Save for later"));
    cx.update_window(f.window, |_, window, cx| {
        window.press("escape", cx);
    })
    .unwrap();

    cx.update(|cx| ircord_app::features::saved::open(&f.session, cx));
    wait(cx, "the saved message listed", |cx| {
        let session = f.session.read(cx);
        session.client().unwrap().read().saved.list.items.len() == 1
    });

    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.click("saved-filter", cx);
        window.input("nothing", cx);
    })
    .unwrap();
    let asked = f
        .server
        .wait_for_nth("GET", "/api/v1/saved-messages", 2, WAIT);
    assert!(asked.query_param("q").is_some());
    wait(cx, "nothing matching", |cx| {
        let session = f.session.read(cx);
        let w = session.client().unwrap().read();
        w.saved.list.query == "nothing" && w.saved.list.loaded && !w.saved.list.loading
    });
    assert_eq!(
        label(&f, cx, "saved-empty").as_deref(),
        Some("No saved messages match “nothing”.")
    );

    // Clearing the filter lists it again; remove it from there.
    cx.update_window(f.window, |_, window, cx| {
        for _ in 0.."nothing".len() {
            window.press("backspace", cx);
        }
    })
    .unwrap();
    wait(cx, "the message listed again", |cx| {
        let session = f.session.read(cx);
        let w = session.client().unwrap().read();
        w.saved.list.query.is_empty() && w.saved.list.items.len() == 1
    });
    click(&f, cx, "saved-remove-1");
    f.server
        .wait_for("DELETE", "/api/v1/saved-messages/1", WAIT);
    wait(cx, "the message gone from the page", |cx| {
        let session = f.session.read(cx);
        let w = session.client().unwrap().read();
        !w.saved.is_saved("1") && w.saved.list.items.is_empty() && w.saved.pending.is_empty()
    });
    assert!(shows(&f, cx, "saved-empty"));
}

fn inbox_item(id: &str, at: &str, dismissed: bool, reasons: &[&str]) -> Value {
    json!({
        "conversationId": "c-general", "createdAt": at, "dismissed": dismissed,
        "excerpt": format!("ping {id}"), "kind": "message", "messageId": id,
        "networkId": "n1", "reasons": reasons, "senderId": "m-bob",
        "status": "available", "unread": !dismissed
    })
}

#[gpui_kit::test]
fn the_inbox_shows_its_badge_lists_by_day_and_dismisses(cx: &mut TestAppContext) {
    let dismissed = Arc::new(AtomicBool::new(false));
    let f = open(cx, |server| {
        let state = dismissed.clone();
        server.route_fn("GET", "/api/v1/inbox/summary", move |_| {
            let unread = if state.load(Ordering::SeqCst) { 1 } else { 2 };
            Reply::json(200, json!({ "unread": unread }))
        });
        let state = dismissed.clone();
        server.route_fn("GET", "/api/v1/inbox", move |r| {
            let gone = state.load(Ordering::SeqCst);
            let items = match r.query_param("filter").as_deref() {
                Some("dismissed") if gone => {
                    vec![inbox_item("i1", "2026-11-21T10:00:00Z", true, &["mention"])]
                }
                Some("dismissed") => vec![],
                _ if gone => vec![inbox_item("i2", "2026-11-20T09:00:00Z", false, &["direct"])],
                _ => vec![
                    inbox_item("i1", "2026-11-21T10:00:00Z", false, &["mention"]),
                    inbox_item("i2", "2026-11-20T09:00:00Z", false, &["direct"]),
                ],
            };
            let unread = if gone { 1 } else { 2 };
            Reply::json(200, json!({ "items": items, "unread": unread }))
        });
        let state = dismissed.clone();
        server.route_fn("PUT", "/api/v1/inbox/i1/dismissed", move |_| {
            state.store(true, Ordering::SeqCst);
            Reply::empty()
        });
    });

    wait(cx, "the unread count", |cx| {
        let session = f.session.read(cx);
        session.client().unwrap().read().inbox.unread == Some(2)
    });
    assert_eq!(
        label(&f, cx, "open-inbox").as_deref(),
        Some("Inbox, 2 unread")
    );

    click(&f, cx, "open-inbox");
    wait(cx, "the inbox listed", |cx| {
        let session = f.session.read(cx);
        let w = session.client().unwrap().read();
        session.route() == Some(Route::Inbox)
            && w.inbox
                .list(Filter::All)
                .is_some_and(|l| l.items.len() == 2)
    });
    let first = f.server.wait_for("GET", "/api/v1/inbox", WAIT);
    assert_eq!(first.query_param("filter").as_deref(), Some("all"));
    assert_eq!(
        label(&f, cx, "inbox-item-i1").as_deref(),
        Some("bob in #general: ping i1")
    );
    assert_eq!(label(&f, cx, "inbox-meta").as_deref(), Some("2 unread"));
    // Two days 25 hours apart, two headings in any time zone.
    let headings = cx
        .update_window(f.window, |_, window, cx| {
            window.render_frame(cx);
            window
                .within("inbox-list")
                .find_all_by_role(gpui_kit::Role::Heading)
                .len()
        })
        .unwrap();
    assert_eq!(headings, 2);
    assert!(shows(&f, cx, "inbox-unread-i1"));
    assert_eq!(
        label(&f, cx, "inbox-reason-i1-0").as_deref(),
        Some("Mention")
    );
    assert_eq!(
        label(&f, cx, "inbox-reason-i2-0").as_deref(),
        Some("Direct message")
    );

    // Dismissing hides it at once, then the count and the list are read again.
    click(&f, cx, "inbox-toggle-i1");
    f.server.wait_for("PUT", "/api/v1/inbox/i1/dismissed", WAIT);
    wait(cx, "i1 dismissed and the count read again", |cx| {
        let session = f.session.read(cx);
        let w = session.client().unwrap().read();
        w.inbox.unread == Some(1)
            && w.inbox.pending.is_empty()
            && w.inbox
                .list(Filter::All)
                .is_some_and(|l| !l.loading && l.items.len() == 1 && l.items[0].message_id == "i2")
    });
    assert!(!shows(&f, cx, "inbox-item-i1"));

    // The Dismissed view lists it, to restore.
    click(&f, cx, "inbox-view-dismissed");
    wait(cx, "the dismissed view", |cx| {
        let session = f.session.read(cx);
        let w = session.client().unwrap().read();
        w.inbox
            .list(Filter::Dismissed)
            .is_some_and(|l| l.loaded && l.items.len() == 1)
    });
    assert_eq!(
        label(&f, cx, "inbox-toggle-i1").as_deref(),
        Some("Restore to inbox")
    );
}

#[gpui_kit::test]
fn a_new_inbox_item_reads_the_badge_again(cx: &mut TestAppContext) {
    let f = open(cx, |server| {
        server.route("GET", "/api/v1/inbox/summary", 200, json!({"unread": 0}));
    });
    wait(cx, "the first count", |cx| {
        let session = f.session.read(cx);
        session.client().unwrap().read().inbox.unread == Some(0)
    });
    assert_eq!(label(&f, cx, "open-inbox").as_deref(), Some("Inbox"));
    f.server
        .route("GET", "/api/v1/inbox/summary", 200, json!({"unread": 1}));
    f.server.push(frame(
        4,
        "inbox.added",
        &json!({"conversationId": "c-general", "createdAt": "2026-11-21T10:00:00Z",
                "messageId": "1", "networkId": "n1", "reasons": ["mention"],
                "senderId": "m-bob"}),
    ));
    wait(cx, "the count read again", |cx| {
        let session = f.session.read(cx);
        session.client().unwrap().read().inbox.unread == Some(1)
    });
    assert_eq!(
        label(&f, cx, "open-inbox").as_deref(),
        Some("Inbox, 1 unread")
    );
}

/// The fixture's workspace with a picture for bob on this server.
fn with_bobs_picture(server: &CannedServer) {
    let mut s = crate::fixture::snapshot();
    for m in &mut s.members {
        if m.id == "m-bob" {
            m.avatar_url = Some("/files/bob/avatar.png".into());
        }
    }
    server.snapshot(&s);
}

fn press(f: &Fixture, cx: &mut TestAppContext, key: &str) {
    cx.update_window(f.window, |_, window, cx| window.press(key, cx))
        .unwrap();
    cx.run_until_parked();
}

#[gpui_kit::test]
fn inbox_rows_carry_the_senders_picture_and_the_keys_the_bar_names(cx: &mut TestAppContext) {
    let f = open(cx, |server| {
        with_bobs_picture(server);
        server.route("GET", "/api/v1/inbox/summary", 200, json!({"unread": 2}));
        server.route(
            "GET",
            "/api/v1/inbox",
            200,
            json!({"items": [
                inbox_item("i1", "2026-11-21T10:00:00Z", false, &["mention"]),
                inbox_item("i2", "2026-11-21T09:00:00Z", false, &["reply"]),
            ], "unread": 2}),
        );
        server.route_fn("PUT", "/api/v1/inbox/i2/dismissed", |_| Reply::empty());
    });
    click(&f, cx, "open-inbox");
    wait(cx, "the inbox listed", |cx| {
        let session = f.session.read(cx);
        let w = session.client().unwrap().read();
        w.inbox
            .list(Filter::All)
            .is_some_and(|l| l.loaded && l.items.len() == 2)
    });
    // The large-title header, the chip, the close button and the key bar.
    assert_eq!(label(&f, cx, "inbox-page-title").as_deref(), Some("Inbox"));
    assert_eq!(label(&f, cx, "inbox-meta").as_deref(), Some("2 unread"));
    assert_eq!(
        label(&f, cx, "inbox-page-close").as_deref(),
        Some("Return to conversation")
    );
    let hints = label(&f, cx, "inbox-page-hints").unwrap();
    assert!(
        hints.contains("↑ ↓ to choose") && hints.contains("E dismiss"),
        "{hints}"
    );
    // Bob's picture from this server, beside his name.
    assert!(shows(&f, cx, "avatar-picture-inbox-i1"));
    assert!(shows(&f, cx, "author-inbox-i1"));

    // Down chooses the second row; E dismisses it.
    press(&f, cx, "down");
    press(&f, cx, "e");
    f.server.wait_for("PUT", "/api/v1/inbox/i2/dismissed", WAIT);

    // Escape goes back to the conversation that was open.
    press(&f, cx, "escape");
    wait(cx, "back in #general", |cx| {
        f.session.read(cx).route() == Some(Route::Conversation("c-general".into()))
    });

    // So does the close button.
    click(&f, cx, "open-inbox");
    wait(cx, "the inbox again", |cx| {
        f.session.read(cx).route() == Some(Route::Inbox)
    });
    click(&f, cx, "inbox-page-close");
    wait(cx, "back in #general", |cx| {
        f.session.read(cx).route() == Some(Route::Conversation("c-general".into()))
    });
}

#[gpui_kit::test]
fn saved_rows_carry_a_picture_and_enter_opens_the_chosen_one(cx: &mut TestAppContext) {
    let f = open(cx, |server| {
        with_bobs_picture(server);
        server.route(
            "GET",
            "/api/v1/saved-messages",
            200,
            json!({"items": [saved_item("2026-11-21T11:00:00Z")]}),
        );
    });
    click(&f, cx, "open-saved");
    wait(cx, "the saved list", |cx| {
        let session = f.session.read(cx);
        let w = session.client().unwrap().read();
        session.route() == Some(Route::Saved)
            && w.saved.list.loaded
            && w.saved.list.items.len() == 1
    });
    assert!(shows(&f, cx, "avatar-picture-saved-1"));
    assert_eq!(label(&f, cx, "saved-meta").as_deref(), Some("1 saved"));
    assert!(shows(&f, cx, "saved-page-close"));
    // The filter has the caret; Enter opens the chosen message.
    press(&f, cx, "enter");
    wait(cx, "the message in #general", |cx| {
        f.session.read(cx).route() == Some(Route::Conversation("c-general".into()))
    });
}

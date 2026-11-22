//! Every menu lists its rows in the web client's order, with the web's
//! separators (`crate::menu_order`, from the frontend's `useMenus.ts` and
//! `SelfMenu.vue`): the conversation menu with its Notifications submenu,
//! the message, member, network and self menus.

use gpui_kit::test::{TestEventExt, TestQueryExt, TestWindowExt};
use std::time::Duration;

use gpui_kit::{AppContext, MouseButton, TestAppContext};
use ircord_client::contract::*;
use ircord_client::store::Connection;
use ircord_client::testing::{CannedServer, Reply};
use serde_json::json;

use crate::fixture::{Fixture, menu_rows, message, open_with, snapshot, wait};

/// The fixture's workspace on a network that offers every message action:
/// you are an operator in #general (which has channel-wide settings), and
/// talk with bob directly.
fn workspace() -> Snapshot {
    let mut s = snapshot();
    let n = &mut s.networks[0];
    n.account = Some("me".into());
    n.features.edit = true;
    n.features.reactions = true;
    n.features.replies = true;
    n.features.redact = true;
    let general = &mut s.conversations[0];
    general.member_ids.push("u-me".into());
    general.member_modes.insert("u-me".into(), "@".into());
    general.channel_settings = Some(ChannelSettings {
        notifications: ChannelSettingsNotifications::Default,
        ..Default::default()
    });
    let mut bob = s.conversations[1].clone();
    bob.id = "d-bob".into();
    bob.name = "bob".into();
    bob.kind = ConversationKind::Direct;
    bob.unread_count = 0;
    bob.mention_count = 0;
    s.conversations.push(bob);
    s
}

/// bob's message 1 and your own 2, both with a network ID.
fn history() -> Vec<Message> {
    let mut one = message("1", "c-general", "hello there");
    one.upstream_id = Some("up-1".into());
    let mut two = message("2", "c-general", "my words");
    two.sender_id = "u-me".into();
    two.account = Some("me".into());
    two.upstream_id = Some("up-2".into());
    vec![one, two]
}

fn serve(server: &CannedServer) {
    server.snapshot(&workspace());
    let page = serde_json::to_value(MessagePage {
        items: history(),
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
    server.route("GET", "/api/v1/app/preferences", 200, json!({}));
    server.route_reply(
        "PUT",
        "/api/v1/conversations/c-general/channel-settings",
        Reply::json(500, json!({})),
    );
    let settings = serde_json::to_value(Settings::default()).unwrap();
    server.route("GET", "/api/v1/notifications/settings", 200, settings);
    server.route(
        "GET",
        "/api/v1/conversations/c-general/channel-guide",
        200,
        json!({"available": false, "groups": [], "operator": false, "status": "none"}),
    );
}

fn open(cx: &mut TestAppContext) -> Fixture {
    let f = open_with(cx, true, serve);
    wait(cx, "#general's history, live, and the documents", |cx| {
        let session = f.session.read(cx);
        let Some(client) = session.client() else {
            return false;
        };
        let w = client.read();
        session.conversation.as_deref() == Some("c-general")
            && w.connection == Some(Connection::Live)
            && w.documents.preferences.is_loaded()
            && w.timelines
                .get("c-general")
                .is_some_and(|t| t.entries.len() == 2)
    });
    f
}

fn frame(
    f: &Fixture,
    cx: &mut TestAppContext,
    run: impl FnOnce(&mut gpui_kit::Window, &mut gpui_kit::App),
) {
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        run(window, cx);
    })
    .unwrap();
    cx.run_until_parked();
}

/// Right-click `id` and read its menu's rows.
fn right_click(f: &Fixture, cx: &mut TestAppContext, id: &'static str) -> Vec<String> {
    frame(f, cx, |window, cx| {
        window.hover(id, cx);
        window.right_click(id, cx);
    });
    rows(f, cx, false)
}

fn rows(f: &Fixture, cx: &mut TestAppContext, submenu: bool) -> Vec<String> {
    let mut rows = Vec::new();
    frame(f, cx, |window, _| rows = menu_rows(window, submenu));
    rows
}

/// Point at the row labelled `label`, which opens its submenu, and read it.
fn submenu(f: &Fixture, cx: &mut TestAppContext, label: &str) -> Vec<String> {
    frame(f, cx, |window, cx| {
        let at = window.find_by_label(label).bounds().center();
        window.pointer_move(at, None, cx);
    });
    rows(f, cx, true)
}

fn close(f: &Fixture, cx: &mut TestAppContext) {
    frame(f, cx, |window, cx| window.press("escape", cx));
}

#[gpui_kit::test]
fn the_conversation_menu_is_in_the_web_order(cx: &mut TestAppContext) {
    let f = open(cx);
    // A channel you are an operator in: the web's
    // favourite, notifications | channel tools | history | leave.
    let rows = right_click(&f, cx, "conversation-c-general");
    // Only this client: Links and Gallery, with the history.
    assert_eq!(
        rows,
        vec![
            "#general",
            "-",
            "Add to favourites",
            "Notifications",
            "Move down",
            "-",
            "Channel settings…",
            "Bans and lists…",
            "Edit topic…",
            "Invite someone…",
            "Copy channel name",
            "-",
            "Jump to a date…",
            "Export log…",
            "Links in this channel",
            "Gallery",
            "-",
            "Leave channel",
        ]
    );
    // The channel's levels are for everyone, and the default is chosen.
    assert_eq!(
        submenu(&f, cx, "Notifications"),
        vec![
            "For everyone in this channel",
            "-",
            "Default (your default: mentions)",
            "All messages",
            "Only mentions",
            "Nothing (mute)",
        ]
    );
    frame(&f, cx, |window, cx| {
        let chosen = window.find_by_label("Default (your default: mentions)");
        assert_eq!(chosen.checked(), Some(true));
        assert_ne!(window.find_by_label("All messages").checked(), Some(true));
        // As an operator you choose it for everyone.
        let at = window.find_by_label("All messages").bounds().center();
        window.pointer_down(at, MouseButton::Left, cx);
        window.pointer_up(at, MouseButton::Left, cx);
    });
    let body = f
        .server
        .wait_for(
            "PUT",
            "/api/v1/conversations/c-general/channel-settings",
            Duration::from_secs(5),
        )
        .json();
    assert_eq!(body, json!({"notifications": "all"}));
    close(&f, cx);

    // A direct conversation: the person's profile where a channel's tools
    // are, and your own levels.
    let rows = right_click(&f, cx, "conversation-d-bob");
    assert_eq!(
        rows,
        vec![
            "bob",
            "-",
            "Add to favourites",
            "Notifications",
            "-",
            "Profile",
            "-",
            "Jump to a date…",
            "Export log…",
            "-",
            "Close conversation",
        ]
    );
    assert_eq!(
        submenu(&f, cx, "Notifications"),
        vec![
            "Default (all messages)",
            "-",
            "All messages",
            "Only mentions",
            "Nothing (mute)",
        ]
    );
    close(&f, cx);
}

#[gpui_kit::test]
fn the_message_menu_is_in_the_web_order(cx: &mut TestAppContext) {
    let f = open(cx);
    // bob's: the quick reactions (the web's row of emoji) | add a reaction,
    // reply, mention, copy, save, mark unread | his profile | delete, which
    // you may as an operator.
    assert_eq!(
        right_click(&f, cx, "message-1"),
        vec![
            "bob",
            "-",
            "React with 👍",
            "React with ❤\u{fe0f}",
            "React with 😂",
            "React with 😮",
            "React with 😢",
            "-",
            "Add reaction",
            "Reply",
            "Mention bob",
            "Copy text",
            "Copy link to message",
            "Save for later",
            "Mark as unread",
            "-",
            "Profile",
            "-",
            "Delete message…",
        ]
    );
    close(&f, cx);
    // Your own: Edit where Mention was.
    assert_eq!(
        right_click(&f, cx, "message-2"),
        vec![
            "me",
            "-",
            "React with 👍",
            "React with ❤\u{fe0f}",
            "React with 😂",
            "React with 😮",
            "React with 😢",
            "-",
            "Add reaction",
            "Reply",
            "Edit",
            "Copy text",
            "Copy link to message",
            "Save for later",
            "Mark as unread",
            "-",
            "Profile",
            "-",
            "Delete message",
        ]
    );
}

#[gpui_kit::test]
fn the_member_menu_is_in_the_web_order(cx: &mut TestAppContext) {
    let f = open(cx);
    // Only this client: Invite to a channel, after WHOIS.
    assert_eq!(
        right_click(&f, cx, "member-m-eve"),
        vec![
            "eve",
            "-",
            "Profile",
            "Message",
            "Mention",
            "Copy nick",
            "Ignore",
            "WHOIS in the server window",
            "Invite to a channel…",
            "-",
            "Make operator",
            "Give voice",
            "-",
            "Kick…",
            "Ban…",
            "Delete recent messages…",
        ]
    );
}

#[gpui_kit::test]
fn the_network_menu_is_in_the_web_order(cx: &mut TestAppContext) {
    let f = open(cx);
    // Only this client: Create a channel, Message someone and Change
    // nickname beside the web's; Add a network on its own.
    assert_eq!(
        right_click(&f, cx, "network-n1"),
        vec![
            "Home",
            "-",
            "Mark all as read",
            "Server window",
            "Join a channel…",
            "Create a channel…",
            "Message someone…",
            "Edit network…",
            "Change nickname…",
            "-",
            "Disconnect",
            "Copy server address",
            "-",
            "Add a network…",
            "-",
            "Remove network",
        ]
    );
}

#[gpui_kit::test]
fn the_self_menu_is_in_the_web_order(cx: &mut TestAppContext) {
    let f = open(cx);
    frame(&f, cx, |window, cx| window.click("self-menu", cx));
    // The web's status card, then you | settings | log out. Only this
    // client: Drafts.
    assert_eq!(
        rows(&f, cx, false),
        vec![
            "Set a status…",
            "-",
            "View your profile",
            "Edit profile",
            "Set yourself away",
            "Drafts",
            "-",
            "Preferences",
            "Notifications",
            "Account settings",
            "-",
            "Log out",
        ]
    );
}

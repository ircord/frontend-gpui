//! Tests for package N1 against the canned server: the rail's icons and
//! dots, drag ordering with Move up and down and Sort A to Z, collapsed
//! sections, the folded list and its widths, the conversation and network
//! menus, the quick switcher (Ctrl+K) and each default key.

use std::time::Duration;

use gpui_kit::test::{TestEventExt, TestQueryExt, TestWindowExt};
use gpui_kit::{AppContext, Keystroke, MouseButton, TestAppContext, point, px};
use ircord_app::features::shortcuts::{self, Gifs, PauseTheme};
use ircord_app::route::Route;
use ircord_app::sidebar::layout;
use ircord_client::contract::*;
use ircord_client::store::sidebar_order::Layout;
use ircord_client::testing::{CannedServer, Reply};
use serde_json::{Value, json};

use crate::fixture::{Fixture, open_with, snapshot, wait};

const WAIT: Duration = crate::fixture::HANG_GUARD;

/// Two networks. Home has #general (open first), #random (unread, a
/// mention), #quiet and a direct message with bob; Away is in error and has
/// #lobby.
fn workspace() -> Snapshot {
    let mut s = snapshot();
    let mut away = s.networks[0].clone();
    away.id = "n2".into();
    away.name = "Away".into();
    away.status = NetworkStatus::Error;
    s.networks.push(away);
    let mut quiet = s.conversations[0].clone();
    quiet.id = "c-quiet".into();
    quiet.name = "quiet".into();
    quiet.topic = String::new();
    let mut bob = quiet.clone();
    bob.id = "d-bob".into();
    bob.name = "bob".into();
    bob.kind = ConversationKind::Direct;
    let mut lobby = quiet.clone();
    lobby.id = "c-lobby".into();
    lobby.name = "lobby".into();
    lobby.network_id = "n2".into();
    s.conversations.extend([quiet, bob, lobby]);
    s
}

/// Serve the documents the sidebar reads, echoing saves.
fn documents(server: &CannedServer) {
    server.route("GET", "/api/v1/app/preferences", 200, json!({}));
    server.route_fn("PUT", "/api/v1/app/preferences", |r| {
        Reply::json(200, r.json())
    });
    let settings = serde_json::to_value(Settings::default()).unwrap();
    server.route("GET", "/api/v1/notifications/settings", 200, settings);
    server.route_fn("PUT", "/api/v1/notifications/settings", |r| {
        Reply::json(200, r.json())
    });
}

fn open(cx: &mut TestAppContext, setup: impl FnOnce(&CannedServer)) -> Fixture {
    let f = open_with(cx, true, |server| {
        server.snapshot(&workspace());
        documents(server);
        setup(server);
    });
    wait(cx, "the workspace and the preferences", |cx| {
        let session = f.session.read(cx);
        let Some(client) = session.client() else {
            return false;
        };
        let w = client.read();
        session.conversation.as_deref() == Some("c-general") && w.documents.preferences.is_loaded()
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

fn click(f: &Fixture, cx: &mut TestAppContext, id: &'static str) {
    frame(f, cx, |window, cx| window.click(id, cx));
}

fn press(f: &Fixture, cx: &mut TestAppContext, key: &str) {
    frame(f, cx, |window, cx| window.press(key, cx));
}

/// Right-click `id` and pick the menu entry labelled `label`.
fn menu(f: &Fixture, cx: &mut TestAppContext, id: &'static str, label: &str) {
    frame(f, cx, |window, cx| {
        window.hover(id, cx);
        window.right_click(id, cx);
    });
    frame(f, cx, |window, cx| {
        let item = window.find_by_label(label).bounds().center();
        window.pointer_down(item, MouseButton::Left, cx);
        window.pointer_up(item, MouseButton::Left, cx);
    });
}

fn open_conversation(f: &Fixture, cx: &mut gpui_kit::App) -> Option<String> {
    f.session.read(cx).conversation.clone()
}

fn y(window: &gpui_kit::Window, id: &'static str) -> f32 {
    f32::from(window.find(id).bounds().origin.y)
}

/// The channel order saved with the `n`th preferences save.
fn saved_order(server: &CannedServer, n: usize) -> Value {
    let body = server
        .wait_for_nth("PUT", "/api/v1/app/preferences", n, WAIT)
        .json();
    body["preferences"]["sidebarOrder"]["n1/channels"].clone()
}

#[gpui_kit::test]
fn the_rail_draws_patterns_and_dots_only_for_problems(cx: &mut TestAppContext) {
    let f = open(cx, |_| {});
    frame(&f, cx, |window, _| {
        assert!(window.find("network-n1-glyph").visible());
        assert!(window.find("network-n2-glyph").visible());
        // #random has a mention; Away is in error; nothing else needs a dot.
        assert!(window.find("network-n1-mention").visible());
        assert!(window.find("network-n2-offline").visible());
        assert!(window.try_find("network-n1-offline").is_none());
        assert_eq!(window.find("network-n1").label(), Some("Home, mentions"));
        assert_eq!(
            window.find("network-n2").label(),
            Some("Away, connection error")
        );
        assert_eq!(window.find("network-n1").selected(), Some(true));
    });
}

#[gpui_kit::test]
fn dragging_and_moving_keep_your_order_and_sort_forgets_it(cx: &mut TestAppContext) {
    let f = open(cx, |_| {});
    frame(&f, cx, |window, cx| {
        assert!(y(window, "conversation-c-general") < y(window, "conversation-c-quiet"));
        window.drag_to("conversation-c-quiet", "conversation-c-general", cx);
    });
    assert_eq!(
        saved_order(&f.server, 1),
        json!(["c-quiet", "c-general", "c-random"])
    );
    frame(&f, cx, |window, _| {
        assert!(y(window, "conversation-c-quiet") < y(window, "conversation-c-general"));
        assert!(window.find("section-channels-sort").visible());
    });

    menu(&f, cx, "conversation-c-quiet", "Move down");
    assert_eq!(
        saved_order(&f.server, 2),
        json!(["c-general", "c-quiet", "c-random"])
    );

    click(&f, cx, "section-channels-sort");
    assert_eq!(saved_order(&f.server, 3), Value::Null, "back to A to Z");
    frame(&f, cx, |window, _| {
        assert!(window.try_find("section-channels-sort").is_none());
    });
}

#[gpui_kit::test]
fn sections_collapse_and_the_list_folds_and_resizes(cx: &mut TestAppContext) {
    let f = open(cx, |_| {});
    click(&f, cx, "section-channels");
    frame(&f, cx, |window, _| {
        assert_eq!(window.find("section-channels").expanded(), Some(false));
        // The open conversation and the one with a mention stay.
        assert!(window.find("conversation-c-general").visible());
        assert!(window.find("conversation-c-random").visible());
        assert!(window.try_find("conversation-c-quiet").is_none());
        assert!(window.find("conversation-d-bob").visible());
    });
    click(&f, cx, "section-channels");
    frame(&f, cx, |window, _| {
        assert!(window.find("conversation-c-quiet").visible());
    });

    click(&f, cx, "sidebar-hide");
    frame(&f, cx, |window, _| {
        assert!(window.try_find("conversation-list").is_none());
        assert!(window.find("folded-c-quiet").visible());
    });
    click(&f, cx, "folded-c-quiet");
    cx.update(|cx| assert_eq!(open_conversation(&f, cx).as_deref(), Some("c-quiet")));
    click(&f, cx, "sidebar-restore");
    frame(&f, cx, |window, _| {
        assert!(window.find("conversation-list").visible());
    });

    // Drag the list's edge 60 px wider, and the panel's 40 px wider.
    let (sidebar_width, panel_width) = cx.update(|cx| {
        let layout = layout::get(cx);
        (layout.sidebar_width, layout.panel_width)
    });
    frame(&f, cx, |window, cx| {
        let handle = window.find("resize-sidebar").bounds();
        assert_eq!(handle.size.width, px(8.));
        let from = point(handle.left() + px(1.), handle.center().y);
        window.drag(from, point(from.x + px(60.), from.y), cx);
        let from = window.find("resize-panel").bounds().center();
        window.drag(from, point(from.x - px(40.), from.y), cx);
    });
    cx.update(|cx| {
        let l = layout::get(cx);
        assert!(
            (l.sidebar_width - sidebar_width - 60.).abs() < 0.5,
            "{}",
            l.sidebar_width
        );
        assert!(
            (l.panel_width - panel_width - 40.).abs() < 0.5,
            "{}",
            l.panel_width
        );
    });
    cx.executor().advance_clock(Duration::from_secs(1));
    cx.run_until_parked();
    cx.update(|cx| {
        let saved: Layout = f.session.read(cx).config.section("sidebar");
        assert!(saved.show_list);
        assert!((saved.sidebar_width - sidebar_width - 60.).abs() <= 3.);
        assert!((saved.panel_width - panel_width - 40.).abs() <= 3.);
    });
    frame(&f, cx, |window, cx| {
        window.click("resize-sidebar", cx);
        window.press("left", cx);
    });
    cx.update(|cx| assert_eq!(layout::get(cx).sidebar_width, sidebar_width + 44.));
    frame(&f, cx, |window, cx| {
        window.double_click("resize-sidebar", cx)
    });
    cx.update(|cx| assert_eq!(layout::get(cx).sidebar_width, sidebar_width));
}

#[gpui_kit::test]
fn the_unread_filter_and_mark_all_read(cx: &mut TestAppContext) {
    let f = open(cx, |_| {});
    click(&f, cx, "sidebar-unread-only");
    frame(&f, cx, |window, _| {
        assert!(window.find("conversation-c-random").visible());
        assert!(
            window.find("conversation-c-general").visible(),
            "open stays"
        );
        assert!(window.try_find("conversation-c-quiet").is_none());
    });
    click(&f, cx, "sidebar-unread-only");
    click(&f, cx, "sidebar-mark-network-read");
    f.server
        .wait_for("PATCH", "/api/v1/conversations/c-random", WAIT);
    cx.update(|cx| {
        let session = f.session.read(cx);
        let w = session.client().unwrap().read();
        assert_eq!(w.conversation("c-random").unwrap().mention_count, 0);
    });
}

#[gpui_kit::test]
fn the_conversation_menu(cx: &mut TestAppContext) {
    let f = open(cx, |server| {
        let mut favourite = workspace().conversations[2].clone();
        favourite.favorite = true;
        let favourite = serde_json::to_value(favourite).unwrap();
        server.route("PATCH", "/api/v1/conversations/c-quiet", 200, favourite);
        server.route_reply("DELETE", "/api/v1/conversations/*", Reply::empty());
    });
    menu(&f, cx, "conversation-c-quiet", "Add to favourites");
    let body = f
        .server
        .wait_for("PATCH", "/api/v1/conversations/c-quiet", WAIT)
        .json();
    assert_eq!(body, json!({"favorite": true}));
    wait(cx, "#quiet in Favourites", |cx| {
        let session = f.session.read(cx);
        let w = session.client().unwrap().read();
        w.conversation("c-quiet").unwrap().favorite
    });
    frame(&f, cx, |window, _| {
        assert!(window.find("section-favorites").visible());
        assert!(y(window, "conversation-c-quiet") < y(window, "conversation-c-general"));
    });

    // Your own level for a direct message, in the notification settings,
    // which the first menu read.
    wait(cx, "the notification settings", |cx| {
        let session = f.session.read(cx);
        let w = session.client().unwrap().read();
        w.documents.notification_settings.is_loaded()
    });
    frame(&f, cx, |window, cx| {
        window.hover("conversation-d-bob", cx);
        window.right_click("conversation-d-bob", cx);
    });
    frame(&f, cx, |window, cx| {
        let at = window.find_by_label("Notifications").bounds().center();
        window.pointer_move(at, None, cx);
    });
    frame(&f, cx, |window, cx| {
        let item = window.find_by_label("Nothing (mute)").bounds().center();
        window.pointer_down(item, MouseButton::Left, cx);
        window.pointer_up(item, MouseButton::Left, cx);
    });
    let body = f
        .server
        .wait_for("PUT", "/api/v1/notifications/settings", WAIT)
        .json();
    assert_eq!(body["levels"], json!({"d-bob": "none"}));

    // A channel's level is for its operators: shown, but disabled, under a
    // heading that says why.
    frame(&f, cx, |window, cx| {
        window.hover("conversation-c-general", cx);
        window.right_click("conversation-c-general", cx);
    });
    frame(&f, cx, |window, cx| {
        assert!(window.find_by_label("Copy channel name").visible());
        let at = window.find_by_label("Notifications").bounds().center();
        window.pointer_move(at, None, cx);
    });
    frame(&f, cx, |window, cx| {
        assert!(window.find_by_label("Set by channel operators").visible());
        let item = window.find_by_label("Nothing (mute)").bounds().center();
        window.pointer_down(item, MouseButton::Left, cx);
        window.pointer_up(item, MouseButton::Left, cx);
    });
    std::thread::sleep(Duration::from_millis(100));
    cx.run_until_parked();
    let channel_settings = "/api/v1/conversations/c-general/channel-settings";
    assert!(f.server.requests_to("PUT", channel_settings).is_empty());
    press(&f, cx, "escape");

    menu(&f, cx, "conversation-c-random", "Leave channel");
    frame(&f, cx, |window, cx| {
        let ok = window.find_by_label("Leave channel").bounds().center();
        window.pointer_down(ok, MouseButton::Left, cx);
        window.pointer_up(ok, MouseButton::Left, cx);
    });
    f.server
        .wait_for("DELETE", "/api/v1/conversations/c-random", WAIT);

    menu(&f, cx, "conversation-d-bob", "Close conversation");
    f.server
        .wait_for("DELETE", "/api/v1/conversations/d-bob", WAIT);
    wait(cx, "bob's conversation gone", |cx| {
        let session = f.session.read(cx);
        let w = session.client().unwrap().read();
        w.conversation("d-bob").is_none()
    });
}

#[gpui_kit::test]
fn the_network_menu(cx: &mut TestAppContext) {
    let f = open(cx, |server| {
        let mut down = workspace().networks[0].clone();
        down.status = NetworkStatus::Disconnected;
        let down = serde_json::to_value(down).unwrap();
        server.route("PATCH", "/api/v1/networks/n1", 200, down);
        server.route_reply("DELETE", "/api/v1/networks/n2", Reply::empty());
    });
    menu(&f, cx, "network-n1", "Mark all as read");
    f.server
        .wait_for("PATCH", "/api/v1/conversations/c-random", WAIT);

    menu(&f, cx, "network-n1", "Disconnect");
    let body = f
        .server
        .wait_for("PATCH", "/api/v1/networks/n1", WAIT)
        .json();
    assert_eq!(body, json!({"status": "disconnected"}));
    wait(cx, "Home disconnected", |cx| {
        let session = f.session.read(cx);
        let w = session.client().unwrap().read();
        w.network("n1").unwrap().status == NetworkStatus::Disconnected
    });
    frame(&f, cx, |window, _| {
        assert!(window.find("network-n1-offline").visible());
    });

    // Away is in error, so it offers Connect; removing it asks first.
    frame(&f, cx, |window, cx| window.right_click("network-n2", cx));
    frame(&f, cx, |window, _| {
        assert!(window.find_by_label("Connect").visible());
        assert!(window.try_find_by_label("Disconnect").is_none());
    });
    press(&f, cx, "escape");
    menu(&f, cx, "network-n2", "Remove network");
    frame(&f, cx, |window, cx| {
        let ok = window.find_by_label("Remove").bounds().center();
        window.pointer_down(ok, MouseButton::Left, cx);
        window.pointer_up(ok, MouseButton::Left, cx);
    });
    f.server.wait_for("DELETE", "/api/v1/networks/n2", WAIT);
    wait(cx, "Away gone", |cx| {
        let session = f.session.read(cx);
        let w = session.client().unwrap().read();
        w.network("n2").is_none() && w.conversation("c-lobby").is_none()
    });
}

#[gpui_kit::test]
fn ctrl_k_switches_conversations(cx: &mut TestAppContext) {
    let f = open(cx, |_| {});
    press(&f, cx, "ctrl-k");
    frame(&f, cx, |window, cx| {
        assert!(window.find("switcher-input").visible());
        window.input("qui", cx);
    });
    frame(&f, cx, |window, _| {
        assert_eq!(
            window.find("switcher-result-0").label(),
            Some("#quiet, Home")
        );
        assert_eq!(window.find("switcher-result-0").selected(), Some(true));
        assert!(window.find_by_label("Search messages for “qui”").visible());
    });
    press(&f, cx, "enter");
    cx.update(|cx| assert_eq!(open_conversation(&f, cx).as_deref(), Some("c-quiet")));
    frame(&f, cx, |window, _| {
        assert!(window.try_find("switcher-input").is_none());
    });

    // Arrows move through the results; a network takes you there.
    press(&f, cx, "ctrl-k");
    frame(&f, cx, |window, cx| window.input("away", cx));
    frame(&f, cx, |window, _| {
        assert_eq!(
            window.find("switcher-result-0").label(),
            Some("Away, Network")
        );
    });
    press(&f, cx, "down");
    press(&f, cx, "up");
    press(&f, cx, "enter");
    cx.update(|cx| assert_eq!(f.session.read(cx).network.as_deref(), Some("n2")));
}

#[gpui_kit::test]
fn each_default_key(cx: &mut TestAppContext) {
    let f = open(cx, |_| {});
    let at = |cx: &mut TestAppContext| cx.update(|cx| open_conversation(&f, cx));
    // Sidebar order in Home: #general, #quiet, #random, then @bob.
    press(&f, cx, "alt-down");
    assert_eq!(at(cx).as_deref(), Some("c-quiet"));
    press(&f, cx, "alt-up");
    assert_eq!(at(cx).as_deref(), Some("c-general"));
    press(&f, cx, "alt-up");
    assert_eq!(at(cx).as_deref(), Some("d-bob"), "wraps around");

    press(&f, cx, "alt-shift-down");
    assert_eq!(at(cx).as_deref(), Some("c-random"), "the mention");
    // Opening #random read it; Escape marks the open one read anyway.
    press(&f, cx, "escape");
    press(&f, cx, "alt-shift-down");
    assert_eq!(
        at(cx).as_deref(),
        Some("c-random"),
        "nothing else is unread"
    );

    press(&f, cx, "ctrl-1");
    assert_eq!(at(cx).as_deref(), Some("c-general"));
    press(&f, cx, "ctrl-3");
    assert_eq!(at(cx).as_deref(), Some("c-random"));
    frame(&f, cx, |window, _| {
        assert_eq!(
            window.find("conversation-c-quiet").description(),
            Some("Ctrl 2")
        );
    });

    press(&f, cx, "alt-left");
    assert_eq!(at(cx).as_deref(), Some("c-general"), "back");
    press(&f, cx, "alt-right");
    assert_eq!(at(cx).as_deref(), Some("c-random"), "forward");

    press(&f, cx, "ctrl-alt-down");
    cx.update(|cx| assert_eq!(f.session.read(cx).network.as_deref(), Some("n2")));
    press(&f, cx, "ctrl-alt-up");
    cx.update(|cx| assert_eq!(f.session.read(cx).network.as_deref(), Some("n1")));

    press(&f, cx, "shift-escape");
    press(&f, cx, "ctrl-,");
    cx.update(|cx| assert_eq!(f.session.read(cx).route(), Some(Route::Settings)));
    press(&f, cx, "ctrl-,");
    cx.update(|cx| assert_ne!(f.session.read(cx).route(), Some(Route::Settings)));

    press(&f, cx, "ctrl-/");
    frame(&f, cx, |window, _| {
        assert!(window.find("shortcut-list").visible());
        assert_eq!(
            window.find("shortcut-search").label(),
            Some("Search and quick switcher: Ctrl K")
        );
        assert_eq!(
            window.find("shortcut-help").label(),
            Some("Keyboard shortcuts: ?, Ctrl /")
        );
        assert_eq!(window.find("shortcut-inbox").label(), Some("Inbox: No key"));
    });
    press(&f, cx, "escape");
    frame(&f, cx, |window, _| {
        assert!(
            window.try_find("shortcut-list").is_none(),
            "Escape closes the help"
        );
    });

    // Focus the message box; then “?” is text, not the help.
    press(&f, cx, "alt-shift-c");
    frame(&f, cx, |window, cx| window.input("?", cx));
    frame(&f, cx, |window, _| {
        assert!(window.try_find("shortcut-list").is_none());
    });

    // The keys other packages handle are bound to their actions.
    frame(&f, cx, |window, _| {
        for (action, keys) in [
            (&Gifs as &dyn gpui_kit::Action, "ctrl-g"),
            (&PauseTheme, "shift-f8"),
        ] {
            let binding = window
                .highest_precedence_binding_for_action(action)
                .unwrap_or_else(|| panic!("{keys} is not bound"));
            let want = Keystroke::parse(keys).unwrap();
            let got = binding.keystrokes()[0].inner();
            assert_eq!(
                (&got.key, got.modifiers),
                (&want.key, want.modifiers),
                "{keys}"
            );
        }
    });
    let _ = shortcuts::group_of("search");
}

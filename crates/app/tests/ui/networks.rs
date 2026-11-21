//! Tests for package NW1: adding, editing and removing networks, the
//! discard question, connect and disconnect, and the settings list.

use std::time::Duration;

use gpui_kit::test::{TestEventExt, TestQueryExt, TestWindowExt};
use gpui_kit::{AppContext, MouseButton, TestAppContext};
use ircord_app::features::network_form;
use ircord_app::route::Route;
use ircord_client::contract::*;
use ircord_client::testing::{CannedServer, Reply};
use serde_json::{Value, json};

use crate::fixture::{Fixture, bootstrap, open_with, snapshot, wait};

const WAIT: Duration = Duration::from_secs(5);

/// The network the server answers a save with: `n2`, or `n1` edited.
fn answer(id: &str, name: &str, host: &str) -> Value {
    json!({"id": id, "name": name, "nick": "me", "host": host, "port": 6697,
           "tls": true, "status": "connecting", "initials": "EX", "color": "",
           "auth": {"method": "none", "passwordSet": false}, "autoJoin": ["#lobby"],
           "capabilities": [], "isupport": {}, "perform": [], "features": {}})
}

/// Presets, an address check that always succeeds, and saving.
fn serve(server: &CannedServer) {
    server.route(
        "GET",
        "/api/v1/networks/presets",
        200,
        json!({"configured": true, "items": [{
            "authMethod": "none", "autoJoin": ["#lobby"], "color": "mint",
            "host": "irc.example.net", "initial": "E", "name": "Example",
            "passwordProvided": false, "port": 6697, "tls": true}]}),
    );
    server.route(
        "POST",
        "/api/v1/networks/check",
        200,
        json!({"reachable": true, "detail": ""}),
    );
    server.route(
        "POST",
        "/api/v1/networks",
        200,
        answer("n2", "Example", "irc.example.net"),
    );
}

fn open(cx: &mut TestAppContext, setup: impl FnOnce(&CannedServer)) -> Fixture {
    let f = open_with(cx, true, |server| {
        serve(server);
        setup(server);
    });
    bootstrap(&f, cx);
    f
}

/// Click the element labelled `label` (dialog buttons, menu entries).
fn press_label(f: &Fixture, cx: &mut TestAppContext, label: &str) {
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        let at = window.find_by_label(label).bounds().center();
        window.pointer_down(at, MouseButton::Left, cx);
        window.pointer_up(at, MouseButton::Left, cx);
    })
    .unwrap();
    cx.run_until_parked();
}

fn click(f: &Fixture, cx: &mut TestAppContext, id: &'static str) {
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.click(id, cx);
    })
    .unwrap();
    cx.run_until_parked();
}

/// Replace the text of the input `id`.
fn type_into(f: &Fixture, cx: &mut TestAppContext, id: &'static str, text: &str) {
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.click(id, cx);
        window.press("ctrl-a", cx);
        window.press("backspace", cx);
        window.input(text, cx);
    })
    .unwrap();
    cx.run_until_parked();
}

fn showing(f: &Fixture, cx: &mut TestAppContext, id: &'static str) -> bool {
    cx.run_until_parked();
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.try_find(id).is_some_and(|e| e.visible())
    })
    .unwrap()
}

fn wait_showing(f: &Fixture, cx: &mut TestAppContext, id: &'static str, shown: bool) {
    let deadline = std::time::Instant::now() + WAIT;
    while showing(f, cx, id) != shown {
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for {id} to be {}",
            if shown { "shown" } else { "gone" }
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn networks(f: &Fixture, cx: &mut TestAppContext) -> Vec<String> {
    cx.update(|cx| {
        let session = f.session.read(cx);
        let w = session.client().unwrap().read();
        w.networks.iter().map(|n| n.name.clone()).collect()
    })
}

fn open_add(f: &Fixture, cx: &mut TestAppContext) {
    let session = f.session.clone();
    cx.update_window(f.window, |_, window, cx| {
        network_form::open_add(&session, window, cx)
    })
    .unwrap();
    wait_showing(f, cx, "network-form", true);
}

fn open_edit(f: &Fixture, cx: &mut TestAppContext) {
    let session = f.session.clone();
    cx.update_window(f.window, |_, window, cx| {
        network_form::open_edit(&session, "n1", window, cx)
    })
    .unwrap();
    wait_showing(f, cx, "network-form", true);
}

#[gpui_kit::test]
fn a_preset_fills_the_form_and_the_checked_address_is_added(cx: &mut TestAppContext) {
    let f = open(cx, |_| {});
    open_add(&f, cx);
    wait_showing(&f, cx, "network-preset-0", true);
    click(&f, cx, "network-preset-0");
    f.server.wait_for("POST", "/api/v1/networks/check", WAIT);
    let check = f.server.requests_to("POST", "/api/v1/networks/check");
    assert_eq!(
        check.last().unwrap().json(),
        json!({"host": "irc.example.net", "port": 6697, "tls": true})
    );
    // Saving waits for the check to come back.
    let deadline = std::time::Instant::now() + WAIT;
    while f.server.requests_to("POST", "/api/v1/networks").is_empty() {
        click(&f, cx, "network-submit");
        assert!(std::time::Instant::now() < deadline, "the form never saved");
        std::thread::sleep(Duration::from_millis(20));
    }
    let added = f.server.requests_to("POST", "/api/v1/networks");
    let body = added[0].json();
    assert_eq!(body["name"], "Example");
    assert_eq!(body["host"], "irc.example.net");
    assert_eq!(body["nick"], "me", "your account's nick");
    assert_eq!(body["autoJoin"], json!(["#lobby"]));
    assert_eq!(body["perform"], json!([]));
    assert_eq!(body["auth"], json!({"method": "none"}));
    wait(cx, "the new network in the workspace", |cx| {
        let session = f.session.read(cx);
        session.client().unwrap().read().network("n2").is_some()
    });
    wait_showing(&f, cx, "network-form", false);
}

#[gpui_kit::test]
fn problems_show_and_an_unreachable_address_cannot_be_saved(cx: &mut TestAppContext) {
    let f = open(cx, |server| {
        server.route(
            "POST",
            "/api/v1/networks/check",
            200,
            json!({"reachable": false, "detail": "Connection refused by irc.nowhere.test."}),
        );
    });
    open_add(&f, cx);
    type_into(&f, cx, "network-name", "Nowhere");
    type_into(&f, cx, "network-host", "ircs://irc.nowhere.test");
    type_into(&f, cx, "network-nick", "1bad");
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window
                .find_by_label("Enter a server name or IP address, without ircs:// or a port.")
                .visible()
        );
    })
    .unwrap();
    type_into(&f, cx, "network-host", "irc.nowhere.test");
    type_into(&f, cx, "network-nick", "good");
    wait_showing(&f, cx, "network-check-retry", true);
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find("network-check").label(),
            Some("Connection refused by irc.nowhere.test.")
        );
    })
    .unwrap();
    click(&f, cx, "network-submit");
    std::thread::sleep(Duration::from_millis(50));
    cx.run_until_parked();
    assert!(
        f.server.requests_to("POST", "/api/v1/networks").is_empty(),
        "an unreachable address is not saved"
    );
}

#[gpui_kit::test]
fn typed_changes_are_not_thrown_away_without_asking(cx: &mut TestAppContext) {
    let f = open(cx, |_| {});
    // Nothing typed: Escape closes at once.
    open_edit(&f, cx);
    cx.update_window(f.window, |_, window, cx| window.press("escape", cx))
        .unwrap();
    wait_showing(&f, cx, "network-form", false);
    // Typed: Escape asks, and keeping the changes keeps the form.
    open_edit(&f, cx);
    type_into(&f, cx, "network-name", "Renamed");
    cx.update_window(f.window, |_, window, cx| window.press("escape", cx))
        .unwrap();
    cx.run_until_parked();
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.find_by_label("Discard").visible());
    })
    .unwrap();
    cx.update_window(f.window, |_, window, cx| window.press("escape", cx))
        .unwrap();
    cx.run_until_parked();
    assert!(showing(&f, cx, "network-form"), "kept");
    // The Cancel button asks too; Discard closes both.
    click(&f, cx, "network-cancel");
    press_label(&f, cx, "Discard");
    wait_showing(&f, cx, "network-form", false);
    assert!(
        f.server
            .requests_to("PATCH", "/api/v1/networks/n1")
            .is_empty(),
        "nothing was saved"
    );
}

#[gpui_kit::test]
fn an_edit_saves_without_a_check_and_remove_asks_first(cx: &mut TestAppContext) {
    let f = open(cx, |server| {
        server.route(
            "PATCH",
            "/api/v1/networks/n1",
            200,
            answer("n1", "Renamed", "irc"),
        );
        server.route_reply("DELETE", "/api/v1/networks/n1", Reply::empty());
    });
    open_edit(&f, cx);
    type_into(&f, cx, "network-name", "Renamed");
    click(&f, cx, "network-submit");
    let patch = f.server.wait_for("PATCH", "/api/v1/networks/n1", WAIT);
    let body = patch.json();
    assert_eq!(body["name"], "Renamed");
    assert_eq!(body["host"], "irc");
    assert_eq!(body["port"], 6667);
    assert_eq!(body["tls"], false);
    assert!(body.get("status").is_none());
    assert!(
        f.server
            .requests_to("POST", "/api/v1/networks/check")
            .is_empty(),
        "the saved address is not checked again"
    );
    wait_showing(&f, cx, "network-form", false);
    assert_eq!(networks(&f, cx), ["Renamed"]);

    open_edit(&f, cx);
    click(&f, cx, "network-remove");
    press_label(&f, cx, "Remove");
    f.server.wait_for("DELETE", "/api/v1/networks/n1", WAIT);
    wait(cx, "the network to leave the workspace", |cx| {
        let session = f.session.read(cx);
        let w = session.client().unwrap().read();
        w.networks.is_empty() && w.conversations.is_empty()
    });
    wait_showing(&f, cx, "network-form", false);
}

#[gpui_kit::test]
fn the_network_menu_edits_and_changes_the_nick(cx: &mut TestAppContext) {
    let f = open(cx, |server| {
        let mut renamed: Network = serde_json::from_value(answer("n1", "Home", "irc")).unwrap();
        renamed.nick = "newme".into();
        renamed.status = NetworkStatus::Connected;
        server.route(
            "PATCH",
            "/api/v1/networks/n1/self",
            200,
            serde_json::to_value(renamed).unwrap(),
        );
    });
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.right_click("network-n1", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find_by_label("Connect").is_none(),
            "already connected"
        );
        assert!(window.find_by_label("Edit network…").visible());
        assert!(window.find_by_label("Add a network…").visible());
    })
    .unwrap();
    press_label(&f, cx, "Change nickname…");
    type_into(&f, cx, "prompt-input", "newme");
    press_label(&f, cx, "Change");
    let changed = f.server.wait_for("PATCH", "/api/v1/networks/n1/self", WAIT);
    assert_eq!(changed.json(), json!({"nick": "newme"}));

    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.right_click("network-n1", cx);
    })
    .unwrap();
    cx.run_until_parked();
    press_label(&f, cx, "Edit network…");
    wait_showing(&f, cx, "network-form", true);
}

#[gpui_kit::test]
fn settings_list_the_networks_and_register_channels(cx: &mut TestAppContext) {
    let f = open(cx, |server| {
        server.route(
            "GET",
            "/api/v1/app/preferences",
            200,
            json!({"preferences": {"registerChannels": true}}),
        );
        server.route_fn("PUT", "/api/v1/app/preferences", |r| {
            Reply::json(200, json!({"preferences": r.json()["preferences"]}))
        });
        let mut down: Network = serde_json::from_value(answer("n1", "Home", "irc")).unwrap();
        down.status = NetworkStatus::Disconnected;
        server.route(
            "PATCH",
            "/api/v1/networks/n1",
            200,
            serde_json::to_value(down).unwrap(),
        );
    });
    f.session
        .update(cx, |s, cx| s.navigate(Route::Settings, cx));
    cx.run_until_parked();
    click(&f, cx, "settings-section-networks");
    wait_showing(&f, cx, "network-setting-n1", true);
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("network-status-n1").label(), Some("Connected"));
        assert!(window.find("network-edit-n1").visible());
        assert!(window.find("network-add").visible());
        assert!(window.find("setting-register-channels").visible());
        assert!(window.find("setting-network-list").visible());
    })
    .unwrap();
    f.server.wait_for("GET", "/api/v1/app/preferences", WAIT);
    std::thread::sleep(Duration::from_millis(50));
    cx.run_until_parked();
    click(&f, cx, "network-register-channels-switch");
    let saved = f.server.wait_for("PUT", "/api/v1/app/preferences", WAIT);
    assert_eq!(saved.json()["preferences"]["registerChannels"], false);
    // Disconnect from the list.
    click(&f, cx, "network-connect-n1");
    let patch = f.server.wait_for("PATCH", "/api/v1/networks/n1", WAIT);
    assert_eq!(patch.json(), json!({"status": "disconnected"}));
    wait(cx, "the network to show disconnected", |cx| {
        let session = f.session.read(cx);
        let w = session.client().unwrap().read();
        w.network("n1")
            .is_some_and(|n| n.status == NetworkStatus::Disconnected)
    });
    let deadline = std::time::Instant::now() + WAIT;
    while cx
        .update_window(f.window, |_, window, cx| {
            window.render_frame(cx);
            window.find("network-status-n1").label() != Some("Disconnected")
        })
        .unwrap()
    {
        assert!(
            std::time::Instant::now() < deadline,
            "the list never redrew"
        );
        cx.run_until_parked();
        std::thread::sleep(Duration::from_millis(5));
    }
    click(&f, cx, "network-edit-n1");
    wait_showing(&f, cx, "network-form", true);
}

#[gpui_kit::test]
fn an_account_without_networks_gets_the_quick_set_up(cx: &mut TestAppContext) {
    let f = open_with(cx, true, |server| {
        serve(server);
        let mut s = snapshot();
        s.networks.clear();
        s.conversations.clear();
        server.snapshot(&s);
    });
    wait_showing(&f, cx, "network-form", true);
    // The first preset is filled in, so only the nickname is asked for.
    wait_showing(&f, cx, "network-summary", true);
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("network-host").is_none(), "folded away");
        assert_eq!(
            window.find("network-summary").label(),
            Some("irc.example.net:6697 · TLS · #lobby")
        );
    })
    .unwrap();
    type_into(&f, cx, "network-nick", "newcomer");
    let deadline = std::time::Instant::now() + WAIT;
    while f.server.requests_to("POST", "/api/v1/networks").is_empty() {
        click(&f, cx, "network-submit");
        assert!(
            std::time::Instant::now() < deadline,
            "the set-up never saved"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    let body = f.server.requests_to("POST", "/api/v1/networks")[0].json();
    assert_eq!(body["nick"], "newcomer");
    assert_eq!(body["host"], "irc.example.net");
    wait_showing(&f, cx, "network-setup-done", true);
    assert_eq!(networks(&f, cx), ["Example"]);
    // The network joins its channel: the app goes to it.
    let lobby: Conversation = serde_json::from_value(json!({
        "id": "c-lobby", "networkId": "n2", "kind": "channel", "name": "lobby",
        "favorite": false, "joined": true, "memberIds": [], "memberModes": {},
        "modes": "", "topic": "", "unreadCount": 0, "mentionCount": 0}))
    .unwrap();
    f.server.push(ircord_client::testing::frame(
        4,
        "conversation.updated",
        &lobby,
    ));
    wait(cx, "the set-up to give way to #lobby", |cx| {
        f.session.read(cx).route() == Some(Route::Conversation("c-lobby".into()))
    });
}

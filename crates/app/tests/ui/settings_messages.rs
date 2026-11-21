//! Tests for package S2: Settings → Messages, Privacy and Shortcuts, and
//! the message layout they change, against the canned server.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui_kit::test::TestWindowExt;
use gpui_kit::{AppContext, TestAppContext, Window};
use ircord_app::route::Route;
use ircord_client::testing::{CannedServer, Reply};
use serde_json::{Value, json};

use crate::fixture::{Fixture, bootstrap, open_with, wait};

const WAIT: Duration = Duration::from_secs(5);

fn notification_settings() -> Value {
    json!({"allDevicesDelay": 0, "autoAwayDelay": 0, "channelDefault": "all",
        "directDefault": "all", "lastDeviceDelay": 0, "levels": {}, "ignore": [],
        "quietHours": {"enabled": false, "start": "22:00", "end": "07:00", "timezone": "UTC"}})
}

/// The three documents S2 edits, each stored the way the server keeps it, so
/// a read after a save answers with what was saved.
fn serve(server: &CannedServer, preferences: Value) {
    let documents = [
        ("/api/v1/app/appearance", json!({"settings": {}})),
        (
            "/api/v1/app/preferences",
            json!({"preferences": preferences}),
        ),
        ("/api/v1/notifications/settings", notification_settings()),
    ];
    for (path, initial) in documents {
        let stored = Arc::new(Mutex::new(initial));
        let read = stored.clone();
        server.route_fn("GET", path, move |_| {
            Reply::json(200, read.lock().unwrap().clone())
        });
        server.route_fn("PUT", path, move |r| {
            *stored.lock().unwrap() = r.json();
            Reply::json(200, r.json())
        });
    }
}

fn signed_in(cx: &mut TestAppContext, preferences: Value) -> Fixture {
    let f = open_with(cx, true, |server| serve(server, preferences));
    bootstrap(&f, cx);
    f
}

fn frame(f: &Fixture, cx: &mut TestAppContext, run: impl FnOnce(&mut Window, &mut gpui_kit::App)) {
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

fn shows(f: &Fixture, cx: &mut TestAppContext, id: &'static str) -> bool {
    let mut found = false;
    frame(f, cx, |window, _| found = window.try_find(id).is_some());
    found
}

fn wait_shows(f: &Fixture, cx: &mut TestAppContext, id: &'static str, shown: bool) {
    let window = f.window;
    wait(cx, id, move |cx| {
        window
            .update(cx, |_, window, cx| {
                window.render_frame(cx);
                window.try_find(id).is_some() == shown
            })
            .unwrap_or(false)
    });
}

fn open_section(f: &Fixture, cx: &mut TestAppContext, section: &'static str) {
    f.session
        .update(cx, |s, cx| s.navigate(Route::Settings, cx));
    cx.run_until_parked();
    click(f, cx, section);
}

/// Open settings on `row` (a settings search's landing), and wait until
/// `id` in it is in view.
fn land(f: &Fixture, cx: &mut TestAppContext, row: &'static str, id: &'static str) {
    let session = f.session.clone();
    cx.update(|cx| ircord_app::settings::open(&session, Some(row), cx));
    cx.run_until_parked();
    let window = f.window;
    wait(cx, id, move |cx| {
        window
            .update(cx, |_, window, cx| {
                window.render_frame(cx);
                window.try_find(id).is_some_and(|e| e.visible())
            })
            .unwrap_or(false)
    });
}

fn back_to_general(f: &Fixture, cx: &mut TestAppContext) {
    f.session
        .update(cx, |s, cx| s.select_conversation("c-general", cx));
    cx.run_until_parked();
    wait_shows(f, cx, "message-1", true);
}

fn route(f: &Fixture, cx: &mut TestAppContext) -> Option<Route> {
    cx.update(|cx| f.session.read(cx).route())
}

#[gpui_kit::test]
fn the_irc_layout_puts_every_message_on_its_own_line(cx: &mut TestAppContext) {
    let f = signed_in(cx, json!({}));
    // Cozy: one picture and name over bob's two messages.
    wait_shows(&f, cx, "message-2", true);
    assert!(shows(&f, cx, "sender-avatar-1"));
    assert!(!shows(&f, cx, "sender-2"), "a grouped message has no name");

    open_section(&f, cx, "settings-section-messages");
    wait_shows(&f, cx, "message-layout-irc", true);
    click(&f, cx, "message-layout-irc");
    let put = f.server.wait_for("PUT", "/api/v1/app/appearance", WAIT);
    assert_eq!(put.json()["settings"]["messageLayout"], "irc");

    back_to_general(&f, cx);
    wait_shows(&f, cx, "sender-2", true);
    assert!(
        !shows(&f, cx, "sender-avatar-1"),
        "IRC lines have no pictures"
    );

    // The reset puts the cozy layout back.
    land(&f, cx, "messages-reset", "messages-reset-button");
    click(&f, cx, "messages-reset-button");
    let put = f
        .server
        .wait_for_nth("PUT", "/api/v1/app/appearance", 2, WAIT);
    assert_eq!(put.json()["settings"]["messageLayout"], "cozy");
    assert_eq!(put.json()["settings"]["messageSize"], 14);
    back_to_general(&f, cx);
    wait_shows(&f, cx, "sender-avatar-1", true);
}

#[gpui_kit::test]
fn text_size_clock_and_channel_activity_are_saved(cx: &mut TestAppContext) {
    let f = signed_in(cx, json!({}));
    land(&f, cx, "text-size", "message-size-more");
    click(&f, cx, "message-size-more");
    let put = f.server.wait_for("PUT", "/api/v1/app/appearance", WAIT);
    assert_eq!(put.json()["settings"]["messageSize"], 15);
    wait(cx, "the size to show", |cx| {
        ircord_app::message::layout::settings(cx).size == 15
    });
    std::thread::sleep(Duration::from_millis(400));
    land(&f, cx, "clock", "message-clock-12h");
    click(&f, cx, "message-clock-12h");
    let put = f
        .server
        .wait_for_nth("PUT", "/api/v1/app/appearance", 2, WAIT);
    assert_eq!(put.json()["settings"]["timeFormat"], "12h");
    assert_eq!(put.json()["settings"]["messageSize"], 15, "kept");
    land(&f, cx, "channel-activity", "channel-activity-hide");
    click(&f, cx, "channel-activity-hide");
    let put = f.server.wait_for("PUT", "/api/v1/app/preferences", WAIT);
    assert_eq!(put.json()["preferences"]["events"], "hide");
}

#[gpui_kit::test]
fn the_startup_channel_opens_when_signing_in(cx: &mut TestAppContext) {
    let f = open_with(cx, true, |server| {
        serve(
            server,
            json!({"startup": {"mode": "channel", "channelId": "c-random"}}),
        )
    });
    wait(cx, "#random opened at startup", |cx| {
        f.session.read(cx).conversation.as_deref() == Some("c-random")
    });
}

#[gpui_kit::test]
fn a_new_shortcut_key_works_and_a_removed_one_stops(cx: &mut TestAppContext) {
    let f = signed_in(cx, json!({}));
    // Alt+Down goes to the next conversation by default.
    frame(&f, cx, |window, cx| {
        window.click("composer-input", cx);
        window.press("alt-down", cx);
    });
    assert_eq!(route(&f, cx), Some(Route::Conversation("c-random".into())));
    back_to_general(&f, cx);

    open_section(&f, cx, "settings-section-shortcuts");
    wait_shows(&f, cx, "shortcut-add-inbox", true);
    // Inbox starts without a key: give it Ctrl+Shift+Y.
    click(&f, cx, "shortcut-add-inbox");
    wait_shows(&f, cx, "shortcut-recording-inbox", true);
    frame(&f, cx, |window, cx| window.press("ctrl-shift-y", cx));
    let put = f.server.wait_for("PUT", "/api/v1/app/preferences", WAIT);
    assert_eq!(
        put.json()["preferences"]["shortcuts"],
        json!({"inbox": [{"key": "y", "mod": true, "shift": true}]})
    );
    wait_shows(&f, cx, "shortcut-recording-inbox", false);
    wait_shows(&f, cx, "shortcut-reset-inbox", true);
    std::thread::sleep(Duration::from_millis(400));
    // And take Alt+Down away from Next conversation.
    click(&f, cx, "shortcut-remove-next-0");
    let put = f
        .server
        .wait_for_nth("PUT", "/api/v1/app/preferences", 2, WAIT);
    assert_eq!(put.json()["preferences"]["shortcuts"]["next"], json!([]));
    wait(cx, "the keys bound again", |cx| {
        cx.try_global::<ircord_app::registry::BoundKeys>()
            .is_some_and(|b| b.0.get("next").is_some_and(Vec::is_empty))
    });

    back_to_general(&f, cx);
    frame(&f, cx, |window, cx| {
        window.click("composer-input", cx);
        window.press("alt-down", cx);
    });
    assert_eq!(
        route(&f, cx),
        Some(Route::Conversation("c-general".into())),
        "Alt+Down does nothing now"
    );
    frame(&f, cx, |window, cx| window.press("ctrl-shift-y", cx));
    assert_eq!(route(&f, cx), Some(Route::Inbox));
}

#[gpui_kit::test]
fn a_key_in_use_asks_to_move_it(cx: &mut TestAppContext) {
    let f = signed_in(cx, json!({}));
    open_section(&f, cx, "settings-section-shortcuts");
    wait_shows(&f, cx, "shortcut-add-inbox", true);
    click(&f, cx, "shortcut-add-inbox");
    frame(&f, cx, |window, cx| window.press("alt-down", cx));
    wait_shows(&f, cx, "shortcut-problem", true);
    click(&f, cx, "shortcut-move");
    let put = f.server.wait_for("PUT", "/api/v1/app/preferences", WAIT);
    assert_eq!(
        put.json()["preferences"]["shortcuts"],
        json!({"inbox": [{"key": "ArrowDown", "alt": true}], "next": []})
    );
    // Enter belongs to the message box.
    std::thread::sleep(Duration::from_millis(400));
    click(&f, cx, "shortcut-add-saved");
    frame(&f, cx, |window, cx| window.press("enter", cx));
    wait_shows(&f, cx, "shortcut-problem", true);
    assert!(!shows(&f, cx, "shortcut-move"));
}

#[gpui_kit::test]
fn privacy_shows_nsfw_and_edits_the_ignore_list(cx: &mut TestAppContext) {
    let f = signed_in(cx, json!({"nsfwNetworks": {"n1": true}}));
    open_section(&f, cx, "settings-section-privacy");
    wait_shows(&f, cx, "ignore-input", true);
    // An older per-network opt-in counts as on; switching off clears it.
    click(&f, cx, "nsfw-content-switch");
    let put = f.server.wait_for("PUT", "/api/v1/app/preferences", WAIT);
    assert_eq!(put.json()["preferences"]["showNsfw"], false);
    assert_eq!(put.json()["preferences"]["nsfwNetworks"], json!({}));

    frame(&f, cx, |window, cx| {
        window.click("ignore-input", cx);
        window.input("*!*@troll.example", cx);
        window.press("enter", cx);
    });
    let put = f
        .server
        .wait_for("PUT", "/api/v1/notifications/settings", WAIT);
    assert_eq!(put.json()["ignore"], json!(["*!*@troll.example"]));
    wait_shows(&f, cx, "ignored-remove-0", true);
    std::thread::sleep(Duration::from_millis(400));
    click(&f, cx, "ignored-remove-0");
    let put = f
        .server
        .wait_for_nth("PUT", "/api/v1/notifications/settings", 2, WAIT);
    assert_eq!(put.json()["ignore"], json!([]));
}

#[gpui_kit::test]
fn settings_search_finds_the_new_rows(cx: &mut TestAppContext) {
    let _f = signed_in(cx, json!({}));
    cx.update(|cx| {
        let sections = ircord_app::registry::get(cx).settings_sections.to_vec();
        for (query, row) in [
            ("irc style", "message-layout"),
            ("12-hour", "clock"),
            ("mask", "ignored-people"),
            ("nsfw", "nsfw-content"),
            ("hotkeys", "shortcut-list"),
            ("startup", "startup-conversation"),
        ] {
            let found = ircord_app::settings::search(&sections, query);
            assert!(
                found
                    .iter()
                    .any(|f| f.row.as_ref().is_some_and(|r| r.id == row)),
                "{query} finds {row}"
            );
        }
    });
}

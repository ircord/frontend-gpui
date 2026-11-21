//! Tests for package S3: Settings → Notifications against the canned
//! server: the settings document round-trips, this device is set up and
//! tested, and another device's choices save.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui_kit::test::TestWindowExt;
use gpui_kit::{AppContext, TestAppContext};
use ircord_client::testing::{CannedServer, Reply};
use serde_json::{Value, json};

use crate::fixture::{Fixture, bootstrap, open_with, wait};

const WAIT: Duration = Duration::from_secs(5);

fn device(id: &str, current: bool, initialized: bool, delivery: &str) -> Value {
    json!({"id": id, "current": current, "name": "", "browser": if current { "IRCord-GPUI" } else { "Firefox" },
           "os": if current { "Linux" } else { "Android" }, "formFactor": "desktop",
           "signedInAt": "2026-11-01T10:00:00Z", "status": "connected",
           "notifications": {"initialized": initialized, "delivery": delivery, "content": "inherit",
                             "sound": "mentions", "preview": true, "revision": 1}})
}

/// The settings document as stored, the device list, and saves that answer
/// with what they were sent.
fn serve(server: &CannedServer, here_initialized: bool) {
    let stored = Arc::new(Mutex::new(json!({
        "channelDefault": "mentions", "directDefault": "all",
        "levels": {"c-gone": "none"}, "highlightWords": ["release"], "ignore": [],
        "quietHours": {"enabled": false, "start": "22:00", "end": "07:00", "timeZone": "UTC"},
        "lastDeviceDelay": 30, "allDevicesDelay": 300, "autoAwayDelay": 30
    })));
    let read = stored.clone();
    server.route_fn("GET", "/api/v1/notifications/settings", move |_| {
        Reply::json(200, read.lock().unwrap().clone())
    });
    server.route_fn("PUT", "/api/v1/notifications/settings", move |r| {
        *stored.lock().unwrap() = r.json();
        Reply::json(200, r.json())
    });
    let devices = Arc::new(Mutex::new(vec![
        device(
            "d-here",
            true,
            here_initialized,
            if here_initialized { "open" } else { "off" },
        ),
        device("d-phone", false, true, "push"),
    ]));
    let list = devices.clone();
    server.route_fn("GET", "/api/v1/auth/devices", move |_| {
        Reply::json(200, json!({"devices": list.lock().unwrap().clone()}))
    });
    for id in ["d-here", "d-phone"] {
        let devices = devices.clone();
        server.route_fn(
            "PUT",
            &format!("/api/v1/auth/devices/{id}/notifications"),
            move |r| {
                let body = r.json();
                let mut list = devices.lock().unwrap();
                let d = list.iter_mut().find(|d| d["id"] == id).unwrap();
                let n = &mut d["notifications"];
                for key in ["content", "delivery", "sound", "preview"] {
                    n[key] = body[key].clone();
                }
                n["initialized"] = json!(true);
                n["revision"] = json!(n["revision"].as_i64().unwrap() + 1);
                Reply::json(200, d.clone())
            },
        );
    }
}

fn signed_in(cx: &mut TestAppContext, here_initialized: bool) -> Fixture {
    let f = open_with(cx, true, |s| serve(s, here_initialized));
    bootstrap(&f, cx);
    f
}

fn render(f: &Fixture, cx: &mut TestAppContext) {
    cx.update_window(f.window, |_, window, cx| window.render_frame(cx))
        .unwrap();
}

/// Wait until `id` is drawn.
fn shows(f: &Fixture, cx: &mut TestAppContext, id: &'static str) {
    let window = f.window;
    wait(cx, id, move |cx| {
        window
            .update(cx, |_, window, cx| {
                window.render_frame(cx);
                window.try_find(id).is_some()
            })
            .unwrap_or(false)
    });
}

fn selected(f: &Fixture, cx: &mut TestAppContext, id: &'static str) -> bool {
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.find(id).selected() == Some(true)
    })
    .unwrap()
}

fn label(f: &Fixture, cx: &mut TestAppContext, id: &'static str) -> Option<String> {
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.find(id).label().map(str::to_string)
    })
    .unwrap()
}

/// Open the settings on `row`, scrolled to it.
fn land(f: &Fixture, cx: &mut TestAppContext, row: &str, shown: &'static str) {
    let session = f.session.clone();
    cx.update(|cx| ircord_app::settings::open(&session, Some(row), cx));
    shows(f, cx, shown);
    render(f, cx);
}

fn click(f: &Fixture, cx: &mut TestAppContext, id: &'static str) {
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.click(id, cx);
    })
    .unwrap();
    cx.run_until_parked();
}

fn type_enter(f: &Fixture, cx: &mut TestAppContext, id: &'static str, text: &str) {
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.click(id, cx);
        window.press("ctrl-a", cx);
        window.input(text, cx);
        window.press("enter", cx);
    })
    .unwrap();
    cx.run_until_parked();
}

#[gpui_kit::test]
fn account_wide_settings_round_trip(cx: &mut TestAppContext) {
    let f = signed_in(cx, true);
    land(
        &f,
        cx,
        "channel-notifications",
        "channel-notifications-choice-all",
    );
    assert!(selected(&f, cx, "channel-notifications-choice-mentions"));
    assert!(selected(&f, cx, "direct-notifications-choice-all"));
    click(&f, cx, "channel-notifications-choice-all");
    let put = f
        .server
        .wait_for("PUT", "/api/v1/notifications/settings", WAIT);
    assert_eq!(put.json()["channelDefault"], "all");
    assert_eq!(put.json()["highlightWords"], json!(["release"]));
    wait(cx, "the saved default", |cx| {
        let session = f.session.read(cx);
        let w = session.client().unwrap().read();
        w.documents.notification_settings.server().is_some_and(|s| {
            s.channel_default == ircord_client::contract::SettingsChannelDefault::All
        })
    });
    assert!(selected(&f, cx, "channel-notifications-choice-all"));

    // A highlight word, added with Enter; a duplicate is refused.
    land(&f, cx, "highlight-words", "highlight-input");
    type_enter(&f, cx, "highlight-input", "deploy");
    let put = f
        .server
        .wait_for_nth("PUT", "/api/v1/notifications/settings", 2, WAIT);
    assert_eq!(put.json()["highlightWords"], json!(["release", "deploy"]));
    shows(&f, cx, "highlight-remove-deploy");
    type_enter(&f, cx, "highlight-input", "Deploy");
    assert_eq!(
        label(&f, cx, "highlight-problem").as_deref(),
        Some("That word is already on the list.")
    );
    click(&f, cx, "highlight-remove-release");
    let put = f
        .server
        .wait_for_nth("PUT", "/api/v1/notifications/settings", 3, WAIT);
    assert_eq!(put.json()["highlightWords"], json!(["deploy"]));

    // The last-used device's delay, typed; every device's stays.
    land(&f, cx, "notify-last-device", "notify-last-device-input");
    type_enter(&f, cx, "notify-last-device-input", "2m");
    let put = f
        .server
        .wait_for_nth("PUT", "/api/v1/notifications/settings", 4, WAIT);
    assert_eq!(put.json()["lastDeviceDelay"], 120);
    assert_eq!(put.json()["allDevicesDelay"], 300);
    // Something that is not a duration says so and saves nothing.
    type_enter(&f, cx, "notify-last-device-input", "soon");
    shows(&f, cx, "notify-last-device-input-problem");
    assert_eq!(
        f.server
            .requests_to("PUT", "/api/v1/notifications/settings")
            .len(),
        4
    );

    // Quiet hours, on, in this device's time zone.
    land(&f, cx, "quiet-hours", "quiet-hours-switch");
    click(&f, cx, "quiet-hours-switch");
    let put = f
        .server
        .wait_for_nth("PUT", "/api/v1/notifications/settings", 5, WAIT);
    assert_eq!(put.json()["quietHours"]["enabled"], true);
    assert_eq!(
        put.json()["quietHours"]["timeZone"],
        json!(ircord_client::store::notify_settings::local_time_zone())
    );
    shows(&f, cx, "quiet-hours-start");

    // Your own level for a conversation that is gone, back to the default.
    land(&f, cx, "conversation-levels", "conversation-level-c-gone");
    click(&f, cx, "conversation-level-c-gone-reset");
    let put = f
        .server
        .wait_for_nth("PUT", "/api/v1/notifications/settings", 6, WAIT);
    assert_eq!(put.json()["levels"], json!({}));
}

#[gpui_kit::test]
fn this_device_is_set_up_once_tested_and_its_choices_save(cx: &mut TestAppContext) {
    let f = signed_in(cx, false);
    // The server alerts no device that was never set up: the app does it.
    let setup = f
        .server
        .wait_for("PUT", "/api/v1/auth/devices/d-here/notifications", WAIT);
    assert_eq!(setup.json()["migrate"], true);
    assert_eq!(setup.json()["delivery"], "open");

    land(&f, cx, "notifications-status", "notifications-send-test");
    assert_eq!(
        label(&f, cx, "notifications-status-summary").as_deref(),
        Some("Notifications are on")
    );
    assert!(selected(&f, cx, "notifications-device-choice-open"));
    click(&f, cx, "notifications-send-test");
    shows(&f, cx, "notifications-test-result");
    let shown = cx.shown_system_notifications();
    assert!(
        shown
            .iter()
            .any(|n| n.tag.as_ref() == "ircord-test-notification"
                && n.body.as_ref() == "This is a test notification."),
        "a test notification shows: {shown:?}"
    );

    // This device's sound; then off, which hides the test.
    click(&f, cx, "this-device-sound-off");
    let put = f
        .server
        .wait_for_nth("PUT", "/api/v1/auth/devices/d-here/notifications", 2, WAIT);
    assert_eq!(put.json()["sound"], "off");
    assert_eq!(put.json().get("migrate"), None);
    wait(cx, "the saved sound", |cx| {
        let session = f.session.read(cx);
        let w = session.client().unwrap().read();
        w.notify_settings.saving.is_empty()
    });
    click(&f, cx, "notifications-device-choice-off");
    let put = f
        .server
        .wait_for_nth("PUT", "/api/v1/auth/devices/d-here/notifications", 3, WAIT);
    assert_eq!(put.json()["delivery"], "off");
    assert_eq!(put.json()["sound"], "off", "the other choices stay");
    shows(&f, cx, "notifications-enable");
    assert_eq!(
        label(&f, cx, "notifications-status-summary").as_deref(),
        Some("Notifications are off on this device")
    );
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("notifications-send-test").is_none());
    })
    .unwrap();
    assert_eq!(
        f.server
            .requests_to("PUT", "/api/v1/auth/devices/d-here/notifications")
            .iter()
            .filter(|r| r.json().get("migrate").is_some())
            .count(),
        1,
        "set up only once"
    );
}

#[gpui_kit::test]
fn another_device_changes_and_a_conflict_says_so(cx: &mut TestAppContext) {
    let f = signed_in(cx, true);
    land(&f, cx, "device-notifications", "device-d-phone");
    assert_eq!(
        label(&f, cx, "device-d-phone").as_deref(),
        Some("Firefox on Android")
    );
    assert!(selected(&f, cx, "d-phone-delivery-push"));
    click(&f, cx, "d-phone-content-mentions");
    let put = f
        .server
        .wait_for("PUT", "/api/v1/auth/devices/d-phone/notifications", WAIT);
    assert_eq!(
        put.json(),
        json!({"content": "mentions", "delivery": "push", "sound": "mentions", "preview": true,
               "revision": 1, "name": ""})
    );
    wait(cx, "the saved choice", |cx| {
        let session = f.session.read(cx);
        let w = session.client().unwrap().read();
        w.notify_settings.saving.is_empty()
    });
    assert!(selected(&f, cx, "d-phone-content-mentions"));

    // Changed elsewhere meanwhile: the server refuses, the app says so.
    f.server.route_reply(
        "PUT",
        "/api/v1/auth/devices/d-phone/notifications",
        Reply::problem(409, "device notification settings changed"),
    );
    click(&f, cx, "d-phone-delivery-off");
    shows(&f, cx, "d-phone-error");
    assert_eq!(
        label(&f, cx, "d-phone-error").as_deref(),
        Some("These settings changed on another device. Review the latest values and try again.")
    );
}

#[gpui_kit::test]
fn search_finds_notification_rows(cx: &mut TestAppContext) {
    let f = signed_in(cx, true);
    let session = f.session.clone();
    cx.update(|cx| ircord_app::settings::open(&session, None, cx));
    shows(&f, cx, "settings-search");
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.click("settings-search", cx);
        window.input("do not disturb", cx);
    })
    .unwrap();
    shows(&f, cx, "settings-result-quiet-hours");
    click(&f, cx, "settings-result-quiet-hours");
    shows(&f, cx, "setting-quiet-hours");
    assert_eq!(
        label(&f, cx, "settings-content").as_deref(),
        Some("Notifications")
    );
}

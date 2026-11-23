//! Tests for package DK1, desktop integration: links handed to the app (as
//! a second copy or the system hands them), the tray mark's count, and
//! Settings → This device. The single-copy hand-over itself, link parsing
//! and window placement are tested without a window in
//! `features/desktop.rs`.

use std::time::Duration;

use gpui_kit::test::TestWindowExt;
use gpui_kit::{AppContext, TestAppContext};
use ircord_app::features::desktop;
use ircord_client::config::ConfigDir;
use ircord_client::testing::CannedServer;
use serde_json::json;

use crate::fixture::{Fixture, bootstrap, open, open_with, wait};

const WAIT: Duration = crate::fixture::HANG_GUARD;

fn signed_in(cx: &mut TestAppContext, setup: impl FnOnce(&CannedServer)) -> Fixture {
    let f = open_with(cx, true, setup);
    bootstrap(&f, cx);
    f
}

fn receive(f: &Fixture, cx: &mut TestAppContext, link: &str) {
    let (session, link) = (f.session.clone(), link.to_string());
    cx.update(|cx| desktop::receive(&session, vec![link], cx));
    cx.run_until_parked();
}

fn viewing(f: &Fixture, cx: &mut TestAppContext) -> Option<String> {
    cx.update(|cx| f.session.read(cx).conversation.clone())
}

/// Wait until `drawn` holds of the window, rendering between checks.
fn shows(f: &Fixture, cx: &mut TestAppContext, drawn: impl Fn(&gpui_kit::Window) -> bool) {
    let window = f.window;
    wait(cx, "the view", move |cx| {
        window
            .update(cx, |_, window, cx| {
                window.render_frame(cx);
                drawn(window)
            })
            .unwrap_or(false)
    });
}

#[gpui_kit::test]
fn a_handed_link_opens_a_joined_channel_by_its_host_or_network(cx: &mut TestAppContext) {
    let f = signed_in(cx, |_| {});
    // The fixture's network is on the host `irc`.
    receive(&f, cx, "irc://irc/#random");
    assert_eq!(viewing(&f, cx).as_deref(), Some("c-random"));
    // `irc.irc` is the same network under another host name.
    receive(&f, cx, "irc://irc.irc:6697/general");
    assert_eq!(viewing(&f, cx).as_deref(), Some("c-general"));
}

#[gpui_kit::test]
fn a_handed_link_to_a_new_channel_asks_before_joining(cx: &mut TestAppContext) {
    let f = signed_in(cx, |server| {
        server.route(
            "POST",
            "/api/v1/conversations",
            200,
            json!({"id": "c-rust", "networkId": "n1", "kind": "channel", "name": "rust",
                   "favorite": false, "joined": true, "memberIds": [], "memberModes": {},
                   "modes": "", "topic": "", "unreadCount": 0, "mentionCount": 0}),
        );
    });
    receive(&f, cx, "irc://irc/%23rust");
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("dialog").is_some(), "asks first");
        assert!(
            f.server
                .requests_to("POST", "/api/v1/conversations")
                .is_empty()
        );
        window.click("ok", cx);
    })
    .unwrap();
    f.server
        .wait_for_nth("POST", "/api/v1/conversations", 1, WAIT);
    wait(cx, "#rust to open", |cx| {
        f.session.read(cx).conversation.as_deref() == Some("c-rust")
    });
}

#[gpui_kit::test]
fn a_link_that_arrives_before_the_workspace_waits_for_it(cx: &mut TestAppContext) {
    let f = open(cx, true);
    receive(&f, cx, "irc://irc/#random");
    wait(cx, "the link to open once signed in", |cx| {
        f.session.read(cx).conversation.as_deref() == Some("c-random")
    });
}

#[gpui_kit::test]
fn a_handed_message_link_opens_its_conversation(cx: &mut TestAppContext) {
    let f = signed_in(cx, |_| {});
    let origin = f.server.address().origin().to_string();
    receive(&f, cx, &format!("{origin}/channel/c-random?m=9"));
    assert_eq!(viewing(&f, cx).as_deref(), Some("c-random"));
}

#[gpui_kit::test]
fn the_tray_counts_mentions_and_can_be_switched_off(cx: &mut TestAppContext) {
    let f = signed_in(cx, |_| {});
    // #random has one mention among three unread; channels count mentions.
    cx.update(|cx| assert_eq!(desktop::tray_count(f.session.read(cx), cx), 1));
    let session = f.session.clone();
    cx.update(|cx| desktop::set_tray_mark(&session, false, cx));
    cx.update(|cx| assert_eq!(desktop::tray_count(f.session.read(cx), cx), 0));
    let config: ConfigDir = cx.update(|cx| f.session.read(cx).config.clone());
    let saved: desktop::Saved = config.section(desktop::SECTION);
    assert_eq!(saved.get("trayMark"), Some(&0.0));
}

#[gpui_kit::test]
fn this_device_shows_links_notifications_and_the_tray_mark(cx: &mut TestAppContext) {
    let f = signed_in(cx, |_| {});
    let session = f.session.clone();
    cx.update(|cx| ircord_app::settings::open(&session, Some("desktop-irc-links"), cx));
    shows(&f, cx, |window| {
        window.try_find("desktop-irc-links-state").is_some()
    });
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("settings-content").label(), Some("This device"));
        assert!(window.try_find("desktop-notifications-test").is_some());
        assert!(window.try_find("desktop-notifications-set-up").is_some());
        window.click("desktop-tray-mark-switch", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update(|cx| assert_eq!(desktop::tray_count(f.session.read(cx), cx), 0));
    // "Set up" goes to S3's notification settings for this device.
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.click("desktop-notifications-set-up", cx);
    })
    .unwrap();
    shows(&f, cx, |window| {
        window.find("settings-content").label() == Some("Notifications")
    });
}

#[gpui_kit::test]
fn the_window_place_is_saved_when_it_changes(cx: &mut TestAppContext) {
    let f = signed_in(cx, |_| {});
    cx.simulate_window_resize(
        f.window,
        gpui_kit::size(gpui_kit::px(900.), gpui_kit::px(700.)),
    );
    // Saving waits for the moving and resizing to pause.
    cx.run_until_parked();
    cx.executor().advance_clock(Duration::from_secs(1));
    let config: ConfigDir = cx.update(|cx| f.session.read(cx).config.clone());
    wait(cx, "the new size in the device state", move |_| {
        let saved: desktop::Saved = config.section(desktop::SECTION);
        saved.get("width") == Some(&900.0) && saved.get("maximized") == Some(&0.0)
    });
}

//! Tests for package DK1, desktop integration: links handed to the app (as
//! a second copy or the system hands them), the tray mark's count,
//! Settings → This device, and the offer of a menu entry to an AppImage. The single-copy hand-over itself, link parsing
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

/// A folder of the test's own under the system's temp folder, removed when
/// dropped.
#[cfg(target_os = "linux")]
struct Scratch(std::path::PathBuf);

#[cfg(target_os = "linux")]
impl Scratch {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!(
            "ircord-appimage-{}",
            ircord_client::client::random_id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Scratch(dir)
    }
}

#[cfg(target_os = "linux")]
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(target_os = "linux")]
fn offer_menu_entry(
    f: &Fixture,
    cx: &mut TestAppContext,
    appimage: &std::path::Path,
    data: &std::path::Path,
) {
    let (session, appimage, data) = (f.session.clone(), appimage.to_owned(), data.to_owned());
    cx.update(|cx| desktop::check_menu_entry(&session, appimage, data, cx));
}

/// Whether the offer shows, once the window has drawn it so (waiting for
/// `expected`, then reading what is drawn).
#[cfg(target_os = "linux")]
fn menu_offered_is(f: &Fixture, cx: &mut TestAppContext, expected: bool) -> bool {
    let window = f.window;
    let drawn = move |cx: &mut gpui_kit::App| {
        window
            .update(cx, |_, window, cx| {
                window.refresh();
                window.render_frame(cx);
                window
                    .try_find("appimage-menu-banner")
                    .is_some_and(|e| e.visible())
            })
            .unwrap_or(false)
    };
    wait(cx, "the menu entry offer", move |cx| drawn(cx) == expected);
    cx.update(drawn)
}

#[cfg(target_os = "linux")]
fn menu_offered(f: &Fixture, cx: &mut TestAppContext) -> bool {
    menu_offered_is(f, cx, true)
}

#[cfg(target_os = "linux")]
fn no_menu_offer(f: &Fixture, cx: &mut TestAppContext) -> bool {
    !menu_offered_is(f, cx, false)
}

#[cfg(target_os = "linux")]
#[gpui_kit::test]
fn an_appimage_without_a_menu_entry_is_offered_one_that_starts_it(cx: &mut TestAppContext) {
    let f = signed_in(cx, |_| {});
    let scratch = Scratch::new();
    let data = scratch.0.join("share");
    let appimage = scratch.0.join("Applications/IRCord GPUI.AppImage");
    offer_menu_entry(&f, cx, &appimage, &data);
    assert!(menu_offered(&f, cx));
    let problems = cx
        .update_window(f.window, |_, window, cx| {
            crate::audit::audit("menu entry offer", window, cx)
        })
        .unwrap();
    assert!(problems.is_empty(), "{problems:#?}");
    cx.update_window(f.window, |_, window, cx| {
        let text = window
            .find("appimage-menu-text")
            .label()
            .unwrap()
            .to_string();
        assert!(text.contains("IRCord GPUI.AppImage"), "{text}");
        window.click("appimage-menu-add", cx);
    })
    .unwrap();
    let entry = data.join("applications/com.ircord.gpui.desktop");
    wait(cx, "the menu entry", |_| entry.is_file());
    wait(cx, "the offer to go", |cx| {
        f.window
            .update(cx, |_, window, cx| {
                window.render_frame(cx);
                window.try_find("appimage-menu-banner").is_none()
            })
            .unwrap_or(false)
    });
    let text = std::fs::read_to_string(&entry).unwrap();
    assert!(
        text.contains(&format!("Exec=\"{}\" %u", appimage.display())),
        "{text}"
    );
    // The next start finds it in place and asks nothing.
    offer_menu_entry(&f, cx, &appimage, &data);
    assert!(no_menu_offer(&f, cx));
}

#[cfg(target_os = "linux")]
#[gpui_kit::test]
fn not_now_is_remembered_for_that_appimage(cx: &mut TestAppContext) {
    let f = signed_in(cx, |_| {});
    let scratch = Scratch::new();
    let data = scratch.0.join("share");
    let appimage = scratch.0.join("IRCord.AppImage");
    offer_menu_entry(&f, cx, &appimage, &data);
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.click("appimage-menu-decline", cx);
    })
    .unwrap();
    assert!(no_menu_offer(&f, cx));
    assert!(!data.join("applications").exists(), "nothing was written");
    offer_menu_entry(&f, cx, &appimage, &data);
    assert!(no_menu_offer(&f, cx), "asked again after Not now");
    // Another AppImage is asked about.
    offer_menu_entry(&f, cx, &scratch.0.join("Other.AppImage"), &data);
    assert!(menu_offered(&f, cx));
}

#[cfg(target_os = "linux")]
#[gpui_kit::test]
fn an_entry_for_a_moved_appimage_follows_it_without_asking(cx: &mut TestAppContext) {
    let f = signed_in(cx, |_| {});
    let scratch = Scratch::new();
    let data = scratch.0.join("share");
    let before = scratch.0.join("Downloads/IRCord.AppImage");
    let after = scratch.0.join("Applications/IRCord.AppImage");
    ircord_platform::appimage::add_menu_entry(&data, &before).unwrap();
    offer_menu_entry(&f, cx, &after, &data);
    assert!(no_menu_offer(&f, cx));
    wait(cx, "the entry to move", |_| {
        ircord_platform::appimage::menu_entry(&data, &after)
            == ircord_platform::appimage::MenuEntry::Current
    });
}

//! The first version's screens: sign-in, the workspace, sending,
//! notifications and signing out; and the shell's layout (V3): the top bar,
//! your menu, the sidebar's pages and the conversation header.

use std::time::Duration;

use gpui_kit::test::{TestEventExt, TestQueryExt, TestWindowExt};
use gpui_kit::{AppContext, MouseButton, SystemNotificationResponse, TestAppContext, Window};
use ircord_app::Screen;
use ircord_app::route::Route;
use ircord_app::session::Phase;
use ircord_client::contract::*;

use crate::fixture::{bootstrap, message, open, wait};
use ircord_client::testing::frame;

#[gpui_kit::test]
fn an_invalid_address_shows_why(cx: &mut TestAppContext) {
    let f = open(cx, false);
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.click("address", cx);
        window.input("http://chat.example.com", cx);
        window.click("submit", cx);
        let error = window.find("sign-in-error");
        assert_eq!(
            error.label(),
            Some("Use HTTPS, or use HTTP for a local server.")
        );
    })
    .unwrap();
    cx.update(|cx| assert!(matches!(f.session.read(cx).phase, Phase::ChooseServer)));
}

#[gpui_kit::test]
fn a_remembered_sign_in_opens_the_workspace(cx: &mut TestAppContext) {
    let f = open(cx, true);
    bootstrap(&f, cx);
    cx.update(|cx| {
        assert!(matches!(f.view.read(cx).screen(), Screen::Workspace(_)));
        // The first conversation is selected after the bootstrap.
        assert_eq!(
            f.session.read(cx).conversation.as_deref(),
            Some("c-general")
        );
    });
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.find("network-n1").visible());
        assert!(window.find("conversation-c-random").visible());
        assert!(window.find("member-list").visible());
        assert!(window.find("message-1").visible());
        assert_eq!(window.find("link-1-0").label(), Some("https://ircord.com"));
        // Switching conversations from the sidebar.
        window.click("conversation-c-random", cx);
    })
    .unwrap();
    cx.update(|cx| {
        assert_eq!(f.session.read(cx).conversation.as_deref(), Some("c-random"));
    });
}

#[gpui_kit::test]
fn the_composer_sends_on_enter(cx: &mut TestAppContext) {
    let f = open(cx, true);
    bootstrap(&f, cx);
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.click("composer-input", cx);
        window.input("hello from the test", cx);
        window.press("enter", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update(|cx| {
        let session = f.session.read(cx);
        let client = session.client().unwrap();
        let w = client.read();
        let pending = w.pending_in("c-general");
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].text, "hello from the test");
    });
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        let id = {
            let session = f.session.read(cx);
            let client = session.client().unwrap();
            let w = client.read();
            format!("pending-{}", w.pending_in("c-general")[0].client_id)
        };
        let id: &'static str = Box::leak(id.into_boxed_str());
        assert!(window.find(id).visible());
    })
    .unwrap();
}

#[gpui_kit::test]
fn a_live_message_appears_and_a_notification_opens_its_conversation(cx: &mut TestAppContext) {
    let f = open(cx, true);
    bootstrap(&f, cx);
    f.server.push(frame(
        4,
        "message.created",
        &message("3", "c-general", "live one"),
    ));
    wait(cx, "the live message", |cx| {
        let session = f.session.read(cx);
        let w = session.client().unwrap().read();
        w.timelines["c-general"].entries.len() == 3
    });
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.find("message-3").visible());
    })
    .unwrap();

    let alert = Alert {
        alert_id: "a1".into(),
        conversation_id: "c-random".into(),
        title: "bob in #random".into(),
        body: "are you there?".into(),
        delivery: AlertDelivery::Os,
        preview: true,
        ..Default::default()
    };
    // Away from the window, the alert is a toast: it goes to the test sink.
    let toasts = crate::notifications::record_toasts(cx);
    f.session.update(cx, |s, _| s.set_window_active(false));
    f.session
        .update(cx, |s, cx| ircord_app::notify::show(s, alert, cx));
    cx.run_until_parked();
    let shown = toasts.borrow().clone();
    assert_eq!(shown.len(), 1);
    assert_eq!(shown[0].title, "bob in #random");
    cx.update(|cx| ircord_app::notify::install(f.session.clone(), cx));
    cx.simulate_system_notification_response(SystemNotificationResponse {
        tag: "a1".into(),
        action_id: None,
    });
    cx.run_until_parked();
    cx.update(|cx| {
        assert_eq!(f.session.read(cx).conversation.as_deref(), Some("c-random"));
    });
}

#[gpui_kit::test]
fn signing_out_returns_to_the_sign_in_screen(cx: &mut TestAppContext) {
    let f = open(cx, true);
    bootstrap(&f, cx);
    click(&f, cx, "self-menu");
    click_label(&f, cx, "Log out");
    cx.update(|cx| {
        assert!(matches!(f.session.read(cx).phase, Phase::SignIn { .. }));
        assert!(matches!(f.view.read(cx).screen(), Screen::SignIn(_)));
    });
}

const WAIT: Duration = Duration::from_secs(5);

fn draw(
    f: &crate::fixture::Fixture,
    cx: &mut TestAppContext,
    check: impl FnOnce(&mut Window, &mut gpui_kit::App),
) {
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        check(window, cx);
    })
    .unwrap();
    cx.run_until_parked();
}

fn click(f: &crate::fixture::Fixture, cx: &mut TestAppContext, id: &'static str) {
    draw(f, cx, |window, cx| window.click(id, cx));
}

fn click_label(f: &crate::fixture::Fixture, cx: &mut TestAppContext, label: &str) {
    draw(f, cx, |window, cx| {
        let at = window.find_by_label(label).bounds().center();
        window.pointer_down(at, MouseButton::Left, cx);
        window.pointer_up(at, MouseButton::Left, cx);
    });
}

fn route(f: &crate::fixture::Fixture, cx: &mut TestAppContext) -> Option<Route> {
    cx.update(|cx| f.session.read(cx).route())
}

#[gpui_kit::test]
fn the_top_bar_holds_the_networks_settings_and_your_menu(cx: &mut TestAppContext) {
    let f = open(cx, true);
    bootstrap(&f, cx);
    draw(&f, cx, |window, _| {
        let bar = window.find("top-bar").bounds();
        let tab = window.find("network-n1");
        assert_eq!(tab.selected(), Some(true));
        assert_eq!(tab.label(), Some("Home, mentions"));
        // The tabs, Settings and your button sit in the bar, above the
        // sidebar, and the old rail is gone.
        for id in ["network-n1", "add-network", "open-settings", "self-menu"] {
            let b = window.find(id).bounds();
            assert!(bar.contains(&b.center()), "{id} is in the top bar");
        }
        assert!(window.find("sidebar-find").bounds().top() >= bar.bottom());
        assert!(window.try_find("network-rail").is_none());
        assert_eq!(window.find("self-status").label(), Some("Online"));
    });

    click(&f, cx, "open-settings");
    assert_eq!(route(&f, cx), Some(Route::Settings));
    click(&f, cx, "nav-back");
    assert_eq!(route(&f, cx), Some(Route::Conversation("c-general".into())));

    click(&f, cx, "self-menu");
    draw(&f, cx, |window, _| {
        for label in [
            "Set a status…",
            "Edit profile",
            "Preferences",
            "Notifications",
            "Account settings",
            "Log out",
        ] {
            assert!(window.find_by_label(label).visible(), "{label}");
        }
    });
    click_label(&f, cx, "Preferences");
    assert_eq!(route(&f, cx), Some(Route::Settings));

    click(&f, cx, "add-network");
    draw(&f, cx, |window, _| {
        assert!(window.find("network-form").visible())
    });
}

#[gpui_kit::test]
fn the_sidebar_opens_with_search_and_its_pages(cx: &mut TestAppContext) {
    let f = open(cx, true);
    bootstrap(&f, cx);
    draw(&f, cx, |window, _| {
        let order = [
            "sidebar-find",
            "open-inbox",
            "open-saved",
            "sidebar-links",
            "sidebar-server",
            "sidebar-unread-only",
            "conversation-list",
            "connection-indicator",
        ];
        let tops: Vec<f32> = order
            .iter()
            .map(|id| f32::from(window.find(*id).bounds().top()))
            .collect();
        for (pair, ids) in tops.windows(2).zip(order.windows(2)) {
            assert!(pair[0] < pair[1], "{} above {}", ids[0], ids[1]);
        }
        // Nothing of the old footer: signing out is in your menu.
        assert!(window.try_find("sign-out").is_none());
        // Inbox and Saved left the conversation header.
        let header = window.find("conversation-header").bounds();
        assert!(!header.contains(&window.find("open-inbox").bounds().center()));
    });
    for (id, expected) in [
        ("open-inbox", Route::Inbox),
        ("open-saved", Route::Saved),
        ("sidebar-links", Route::Links(None)),
        ("sidebar-server", Route::ServerWindow("n1".into())),
    ] {
        click(&f, cx, id);
        assert_eq!(route(&f, cx), Some(expected.clone()));
        draw(&f, cx, |window, _| {
            assert_eq!(
                window.find(id).selected(),
                Some(true),
                "{id} shows it is open"
            );
        });
    }
    click(&f, cx, "sidebar-network-menu");
    draw(&f, cx, |window, _| {
        assert!(window.find_by_label("Mark all as read").visible());
    });
    draw(&f, cx, |window, cx| window.press("escape", cx));
    click(&f, cx, "search-conversation-toggle");
    draw(&f, cx, |window, _| {
        assert!(window.find("conversation-search").visible())
    });
    click(&f, cx, "sidebar-find");
    draw(&f, cx, |window, _| {
        assert!(window.find("search-page").visible());
        assert_eq!(window.find("search-input").focused(), Some(true));
        assert_eq!(window.find("sidebar-find").selected(), Some(true));
        assert!(window.try_find("resize-panel").is_none());
    });
}

#[gpui_kit::test]
fn the_conversation_header_has_the_star_the_members_and_the_menu(cx: &mut TestAppContext) {
    let f = open(cx, true);
    bootstrap(&f, cx);
    draw(&f, cx, |window, _| {
        assert_eq!(window.find("conversation-title").label(), Some("#general"));
        assert_eq!(
            window.find("header-favourite").label(),
            Some("Add #general to favourites")
        );
        let members = window.find("members-toggle");
        assert!(
            members
                .label()
                .is_some_and(|l| l.starts_with("Hide members, "))
        );
        assert!(window.find("member-list").visible());
        // Settings moved to the top bar.
        assert!(
            !window
                .find("conversation-header")
                .bounds()
                .contains(&window.find("open-settings").bounds().center())
        );
    });
    click(&f, cx, "members-toggle");
    draw(&f, cx, |window, _| {
        assert!(window.try_find("member-list").is_none());
        assert!(
            window
                .find("members-toggle")
                .label()
                .is_some_and(|l| l.starts_with("Show members, "))
        );
    });
    click(&f, cx, "members-toggle");
    draw(&f, cx, |window, _| {
        assert!(window.find("member-list").visible())
    });

    click(&f, cx, "header-favourite");
    let body = f
        .server
        .wait_for("PATCH", "/api/v1/conversations/c-general", WAIT)
        .json();
    assert_eq!(body, serde_json::json!({"favorite": true}));

    click(&f, cx, "conversation-menu");
    draw(&f, cx, |window, _| {
        assert!(window.find_by_label("Copy channel name").visible());
    });
}

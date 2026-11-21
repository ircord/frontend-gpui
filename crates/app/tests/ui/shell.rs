//! The first version's screens: sign-in, the workspace, sending,
//! notifications and signing out.

use gpui_kit::test::TestWindowExt;
use gpui_kit::{AppContext, SystemNotificationResponse, TestAppContext};
use ircord_app::Screen;
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
    f.session
        .update(cx, |s, cx| ircord_app::notify::show(s, alert, cx));
    let shown = cx.shown_system_notifications();
    assert_eq!(shown.len(), 1);
    assert_eq!(shown[0].title.as_ref(), "bob in #random");
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
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.click("sign-out", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update(|cx| {
        assert!(matches!(f.session.read(cx).phase, Phase::SignIn { .. }));
        assert!(matches!(f.view.read(cx).screen(), Screen::SignIn(_)));
    });
}

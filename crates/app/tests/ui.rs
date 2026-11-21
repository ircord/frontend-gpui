//! Headless UI tests: real views in GPUI test windows, driven by simulated
//! clicks and keys. The client points at a closed port, so nothing reaches a
//! network; the workspace is filled the way a bootstrap would fill it.

use std::sync::Arc;

use gpui_kit::test::TestWindowExt;
use gpui_kit::{
    AppContext, Bounds, Entity, Point, SystemNotificationResponse, TestAppContext, WindowBounds,
    WindowOptions, px, size,
};
use ircord_app::session::{Phase, Session};
use ircord_app::{MainView, Screen};
use ircord_client::ServerAddress;
use ircord_client::config::ConfigDir;
use ircord_client::contract::*;
use ircord_client::credentials::{CredentialStore, MemoryStore};
use ircord_client::store::Changes;

struct Fixture {
    session: Entity<Session>,
    window: gpui_kit::AnyWindowHandle,
    view: Entity<MainView>,
    _dir: tempdir::Dir,
}

mod tempdir {
    pub struct Dir(pub std::path::PathBuf);
    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    pub fn new() -> Dir {
        let dir =
            std::env::temp_dir().join(format!("ircord-ui-{}", ircord_client::client::random_id()));
        std::fs::create_dir_all(&dir).unwrap();
        Dir(dir)
    }
}

/// A closed local port: every request fails at once.
const NOWHERE: &str = "http://127.0.0.1:9";

fn open(cx: &mut TestAppContext, signed_in: bool) -> Fixture {
    cx.update(|cx| {
        ircord_app::notify::identify(cx);
        gpui_kit::init(cx);
    });
    let dir = tempdir::new();
    let config = ConfigDir::at(dir.0.join("config"));
    let credentials: Arc<dyn CredentialStore> = Arc::new(MemoryStore::default());
    if signed_in {
        let server = ServerAddress::parse(NOWHERE).unwrap();
        config.save_server(&server).unwrap();
        credentials.save(&server, "ircord_session=test").unwrap();
    }
    let runtime = ircord_client::client::runtime().unwrap();
    let session = cx.new(|cx| Session::new(runtime, config, credentials, cx));
    let (window, view) = cx.update(|cx| {
        gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: Point::default(),
                    size: size(px(1200.), px(800.)),
                })),
                ..Default::default()
            },
            cx,
            |window, cx| cx.new(|cx| MainView::new(session.clone(), window, cx)),
        )
        .unwrap()
    });
    Fixture {
        session,
        window,
        view,
        _dir: dir,
    }
}

fn snapshot() -> Snapshot {
    serde_json::from_value(serde_json::json!({
        "version": 1, "cursor": 3, "demo": false,
        "user": {"id": "u-me", "account": "me", "nick": "me", "displayName": "",
                 "bio": "", "color": "", "role": "member", "status": "online"},
        "capabilities": {}, "invitations": [],
        "members": [
            {"id": "m-bob", "account": "bob", "nick": "bob", "displayName": "", "bio": "",
             "color": "", "role": "member", "status": "online"},
            {"id": "m-eve", "account": "eve", "nick": "eve", "displayName": "", "bio": "",
             "color": "", "role": "member", "status": "offline"}
        ],
        "networks": [{"id": "n1", "name": "Home", "nick": "me", "host": "irc", "port": 6667,
                      "tls": false, "status": "connected", "initials": "HM", "color": "",
                      "auth": {"method": "none", "passwordSet": false}, "autoJoin": [],
                      "capabilities": [], "isupport": {"PREFIX": "(ov)@+"}, "perform": [],
                      "features": {"typing": true}}],
        "conversations": [
            {"id": "c-general", "networkId": "n1", "kind": "channel", "name": "general",
             "favorite": false, "joined": true, "memberIds": ["m-bob", "m-eve"],
             "memberModes": {"m-bob": "@"}, "modes": "", "topic": "Welcome",
             "unreadCount": 0, "mentionCount": 0},
            {"id": "c-random", "networkId": "n1", "kind": "channel", "name": "random",
             "favorite": false, "joined": true, "memberIds": [], "memberModes": {},
             "modes": "", "topic": "", "unreadCount": 3, "mentionCount": 1}
        ]
    }))
    .unwrap()
}

fn message(id: &str, conversation: &str, text: &str) -> Message {
    Message {
        id: id.into(),
        conversation_id: conversation.into(),
        created_at: "2026-11-21T10:00:00Z".into(),
        kind: MessageKind::Message,
        sender_id: "m-bob".into(),
        text: text.into(),
        ..Default::default()
    }
}

/// Fill the workspace as a bootstrap would.
fn bootstrap(f: &Fixture, cx: &mut TestAppContext) {
    cx.update(|cx| {
        let session = f.session.read(cx);
        let client = session.client().expect("signed in").clone();
        client.change(|w| w.reset(snapshot()));
        client.change(|w| {
            w.load_newest(
                "c-general",
                MessagePage {
                    items: vec![
                        message("1", "c-general", "hello \x02world\x02 https://ircord.com"),
                        message("2", "c-general", "second line"),
                    ],
                    ..Default::default()
                },
            )
        });
    });
    cx.run_until_parked();
}

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
        window.click("composer", cx);
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
    cx.update(|cx| {
        let client = f.session.read(cx).client().unwrap().clone();
        client.change(|w| {
            w.apply(
                &event::ServerEvent::Durable {
                    seq: 4,
                    event: event::Durable::MessageCreated(message("3", "c-general", "live one")),
                },
                0,
            )
        });
    });
    cx.run_until_parked();
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
    let _ = Changes::default();
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

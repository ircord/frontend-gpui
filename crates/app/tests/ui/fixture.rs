//! The shared fixture: a session in a GPUI test window, with the app's
//! registry and features installed, signed in to the client crate's
//! `CannedServer`. The server answers the bootstrap with [`snapshot`] and the
//! history of `#general` with two messages; a test adds routes for whatever
//! else its feature calls (`f.server.route(…)`) before the call happens.
//!
//! The client does real I/O on its own thread, so the fixture allows GPUI's
//! test scheduler to park, and [`wait`] polls for a condition instead of
//! assuming one `run_until_parked` is enough.

use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::{
    AppContext, Bounds, Entity, Point, TestAppContext, WindowBounds, WindowOptions, px, size,
};
use ircord_app::MainView;
use ircord_app::session::Session;
use ircord_client::ServerAddress;
use ircord_client::config::ConfigDir;
use ircord_client::contract::*;
use ircord_client::credentials::{CredentialStore, MemoryStore};
use ircord_client::testing::{CANNED_COOKIE, CannedServer, Reply};

pub struct Fixture {
    pub session: Entity<Session>,
    pub window: gpui_kit::AnyWindowHandle,
    pub view: Entity<MainView>,
    pub server: CannedServer,
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

pub fn open(cx: &mut TestAppContext, signed_in: bool) -> Fixture {
    open_with(cx, signed_in, |_| {})
}

/// [`open`], with `setup` run on the server before the session starts.
pub fn open_with(
    cx: &mut TestAppContext,
    signed_in: bool,
    setup: impl FnOnce(&CannedServer),
) -> Fixture {
    cx.update(|cx| {
        ircord_app::notify::identify(cx);
        gpui_kit::init(cx);
        ircord_app::init(cx);
        // No UI test reaches the real media layer or the sound devices; a
        // voice test installs its own fake with the answers it needs.
        let fake: Arc<dyn ircord_client::voice::Connector> =
            Arc::new(ircord_client::testing::voice::FakeConnector::default());
        ircord_app::features::voice::use_connector(cx, fake);
    });
    cx.executor().allow_parking();
    let runtime = ircord_client::client::runtime().unwrap();
    let server = CannedServer::start(&runtime);
    server.snapshot(&snapshot());
    server.route_fn("GET", "/api/v1/conversations/*/messages", |request| {
        let items = if request.path.ends_with("/c-general/messages") {
            vec![
                message("1", "c-general", "hello \x02world\x02 https://ircord.com"),
                message("2", "c-general", "second line"),
            ]
        } else {
            Vec::new()
        };
        Reply::json(
            200,
            serde_json::to_value(MessagePage {
                items,
                ..Default::default()
            })
            .unwrap(),
        )
    });
    setup(&server);
    let dir = tempdir::new();
    let config = ConfigDir::at(dir.0.join("config"));
    let credentials: Arc<dyn CredentialStore> = Arc::new(MemoryStore::default());
    if signed_in {
        let address: ServerAddress = server.address();
        config.save_server(&address).unwrap();
        credentials.save(&address, CANNED_COOKIE).unwrap();
    }
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
        server,
        _dir: dir,
    }
}

pub fn snapshot() -> Snapshot {
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
                      "features": {"typing": true, "messageLineBytes": 512}}],
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

pub fn message(id: &str, conversation: &str, text: &str) -> Message {
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

/// How long a test waits for something to happen before it calls the wait
/// hung. A passing test never gets near it; it is long because a loaded
/// machine, such as a CI runner or a parallel build, can run a test far slower
/// than a developer's, and a failing test reports only after it.
pub const HANG_GUARD: Duration = Duration::from_secs(30);

/// A reply the server holds back until the test lets go of it, so no timer
/// decides whether the reply or the client's own deadline comes first.
/// Drop the returned sender to release it; a held reply is also released after
/// a minute, so a test that forgets cannot hang the run.
pub fn held_reply(
    reply: impl Fn() -> Reply + Send + Sync + 'static,
) -> (
    std::sync::mpsc::Sender<()>,
    impl Fn(&ircord_client::testing::Recorded) -> Reply + Send + Sync + 'static,
) {
    let (release, held) = std::sync::mpsc::channel::<()>();
    let held = std::sync::Mutex::new(held);
    (release, move |_| {
        let _ = held.lock().unwrap().recv_timeout(Duration::from_secs(60));
        reply()
    })
}

/// Run GPUI until `condition` holds, or fail after [`HANG_GUARD`].
pub fn wait(cx: &mut TestAppContext, what: &str, condition: impl Fn(&mut gpui_kit::App) -> bool) {
    let deadline = Instant::now() + HANG_GUARD;
    loop {
        cx.run_until_parked();
        if cx.update(|cx| condition(cx)) {
            return;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Wait for the bootstrap and for `#general`'s history to show.
pub fn bootstrap(f: &Fixture, cx: &mut TestAppContext) {
    wait(cx, "the bootstrap and #general's history", |cx| {
        let session = f.session.read(cx);
        let Some(client) = session.client() else {
            return false;
        };
        let w = client.read();
        session.conversation.as_deref() == Some("c-general")
            && w.timelines
                .get("c-general")
                .is_some_and(|t| t.loaded && t.entries.len() == 2)
    });
}

/// The rows of the open menu, top to bottom: each row's label, and `-` for a
/// separator. The rows of an open submenu are left out; give `submenu` to
/// read those instead.
pub fn menu_rows(window: &gpui_kit::Window, submenu: bool) -> Vec<String> {
    use gpui_kit::test::TestQueryExt;
    use gpui_kit::{ElementId, Role};
    let in_submenu = |path: &[ElementId]| {
        path.iter()
            .any(|id| *id == ElementId::Name("submenu".into()))
    };
    let items: Vec<_> = window
        .find_all_by_role(Role::MenuItem)
        .into_iter()
        .filter(|i| i.visible() && in_submenu(i.path()) == submenu)
        .collect();
    let mut rows: Vec<(f32, String)> = Vec::new();
    for item in &items {
        let bounds = item.bounds();
        let row = match item.label() {
            Some(label) => label.to_string(),
            // A row drawn by hand wraps a labelled element; a separator
            // holds nothing.
            None if items.iter().any(|other| {
                other.label().is_some() && bounds.contains(&other.bounds().center())
            }) =>
            {
                continue;
            }
            None => "-".to_string(),
        };
        rows.push((f32::from(bounds.origin.y), row));
    }
    rows.sort_by(|a, b| a.0.total_cmp(&b.0));
    rows.into_iter().map(|(_, row)| row).collect()
}

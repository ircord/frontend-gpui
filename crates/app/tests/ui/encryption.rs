//! End-to-end encrypted direct messages (experimental), through the views:
//! the lock and the panel start an encrypted chat with a peer (a headless
//! client with its own engine, against the same fake backend); the composer
//! says the message is encrypted, and what is typed reaches the peer only as
//! ciphertext; the peer's answer is decrypted into the timeline; turning it
//! off asks first. A second device of the account shows its QR code and the
//! code, and the first device approves it. Each view passes the audit.

use std::sync::Arc;
use std::time::Duration;

use gpui_kit::test::{TestEventExt, TestQueryExt, TestWindowExt};
use gpui_kit::{AppContext, TestAppContext};
use ircord_client::Client;
use ircord_client::e2ee::vault::VaultKey;
use ircord_client::e2ee::{Engine, Texts};
use ircord_client::events::Update;
use ircord_client::store::Workspace;
use ircord_client::store::e2ee::DeviceState;
use ircord_client::testing::{CannedServer, E2eeBackend, e2ee_snapshot, wait_live, wait_until};
use tokio::runtime::Runtime;

use crate::fixture::{Fixture, open_with, wait};

const WAIT: Duration = crate::fixture::HANG_GUARD;

/// A peer's (or another device's) headless client and engine.
struct Peer {
    client: Client,
    engine: Engine,
    _server: CannedServer,
    _dir: tempfile_dir::Dir,
    _updates: futures::channel::mpsc::UnboundedReceiver<Update>,
}

mod tempfile_dir {
    pub struct Dir(pub std::path::PathBuf);
    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    pub fn new() -> Dir {
        let dir = std::env::temp_dir().join(format!(
            "ircord-e2ee-peer-{}",
            ircord_client::client::random_id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Dir(dir)
    }
}

fn texts() -> Texts {
    Texts {
        control: Box::new(|on, name, _| format!("{name} turned {on}")),
        unreadable: Box::new(|why| format!("{why:?}")),
    }
}

fn start_peer(
    rt: &Arc<Runtime>,
    server: CannedServer,
    snapshot: &ircord_client::contract::Snapshot,
    open: &str,
) -> Peer {
    server.snapshot(snapshot);
    let (client, updates) = Client::start(rt.clone(), server.api());
    let dir = tempfile_dir::new();
    let engine = Engine::start(
        client.handle().clone(),
        rt.handle().clone(),
        dir.0.clone(),
        VaultKey::random(),
        "Bob's laptop".into(),
        texts(),
    );
    wait_live(&client);
    client.open(open);
    Peer {
        client,
        engine,
        _server: server,
        _dir: dir,
        _updates: updates,
    }
}

fn has_text(w: &Workspace, conversation: &str, text: &str) -> bool {
    w.timelines
        .get(conversation)
        .is_some_and(|t| t.entries.iter().any(|e| e.message.text == text))
}

fn app<R>(f: &Fixture, cx: &mut TestAppContext, read: impl Fn(&Workspace) -> R) -> R {
    cx.update(|cx| read(&f.session.read(cx).client().unwrap().read()))
}

fn draw(f: &Fixture, cx: &mut TestAppContext) {
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.render_frame(cx);
    })
    .unwrap();
    cx.run_until_parked();
}

fn click(f: &Fixture, cx: &mut TestAppContext, id: &str) {
    let id: &'static str = Box::leak(id.to_string().into_boxed_str());
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.click(id, cx);
    })
    .unwrap();
    cx.run_until_parked();
}

fn shown(f: &Fixture, cx: &mut TestAppContext, id: &str) -> bool {
    let id: &'static str = Box::leak(id.to_string().into_boxed_str());
    let mut seen = false;
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        seen = window.try_find(id).is_some_and(|e| e.visible());
    })
    .unwrap();
    seen
}

/// Scroll the encryption panel until `id` is in view.
fn reveal(f: &Fixture, cx: &mut TestAppContext, id: &str) {
    for _ in 0..20 {
        if shown(f, cx, id) {
            return;
        }
        cx.update_window(f.window, |_, window, cx| {
            let down = gpui_kit::ScrollDelta::Pixels(gpui_kit::point(
                gpui_kit::px(0.),
                gpui_kit::px(-120.),
            ));
            window.scroll("encryption-panel", down, cx);
            window.render_frame(cx);
        })
        .unwrap();
        cx.run_until_parked();
    }
}

fn present(f: &Fixture, cx: &mut TestAppContext, id: &str) -> bool {
    let id: &'static str = Box::leak(id.to_string().into_boxed_str());
    let mut seen = false;
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        seen = window.try_find(id).is_some();
    })
    .unwrap();
    seen
}

fn label(f: &Fixture, cx: &mut TestAppContext, id: &str) -> String {
    let id: &'static str = Box::leak(id.to_string().into_boxed_str());
    let mut text = String::new();
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        text = window
            .try_find(id)
            .and_then(|e| e.label().map(str::to_string))
            .unwrap_or_default();
    })
    .unwrap();
    text
}

fn audit(f: &Fixture, cx: &mut TestAppContext, view: &str) {
    let problems = cx
        .update_window(f.window, |_, window, cx| {
            crate::audit::audit(view, window, cx)
        })
        .unwrap();
    assert!(problems.is_empty(), "{problems:#?}");
}

#[gpui_kit::test]
fn an_encrypted_chat_through_the_lock_the_composer_and_the_panel(cx: &mut TestAppContext) {
    let backend = E2eeBackend::new();
    let shared = backend.clone();
    let f = open_with(cx, true, move |server| {
        shared.serve(server, "me", "d-bob", "m-bob", "bob");
        server.snapshot(&e2ee_snapshot("me", "d-bob", "m-bob", "bob"));
    });
    let rt = ircord_client::client::runtime().unwrap();
    let server = CannedServer::start(&rt);
    backend.serve(&server, "bob", "d-me", "m-me", "me");
    let bob = start_peer(
        &rt,
        server,
        &e2ee_snapshot("bob", "d-me", "m-me", "me"),
        "d-me",
    );
    wait_until(&bob.client, WAIT, "bob set up", |w| w.e2ee.device.ready());
    wait(cx, "this device set up", |cx| {
        f.session
            .read(cx)
            .client()
            .is_some_and(|c| c.read().e2ee.device.ready())
    });
    f.session
        .update(cx, |s, cx| s.select_conversation("d-bob", cx));
    draw(&f, cx);
    assert!(shown(&f, cx, "encryption-lock"));
    assert_eq!(label(&f, cx, "encryption-lock"), "Not end-to-end encrypted");
    audit(&f, cx, "an unencrypted direct conversation");

    click(&f, cx, "encryption-lock");
    wait(cx, "the panel", |cx| {
        f.session.read(cx).panel.as_deref() == Some(ircord_app::features::encryption::PANEL)
    });
    assert!(shown(&f, cx, "encryption-panel-caveat"));
    wait(cx, "the peer checked", |_| true);
    click(&f, cx, "encryption-panel-start");
    wait(cx, "encryption on", |cx| {
        f.session
            .read(cx)
            .client()
            .is_some_and(|c| c.read().e2ee.is_on("d-bob"))
    });
    wait_until(&bob.client, WAIT, "bob's side on", |w| w.e2ee.is_on("d-me"));
    wait(cx, "the control line", |cx| {
        let s = f.session.read(cx);
        let w = s.client().unwrap().read();
        has_text(
            &w,
            "d-bob",
            "You turned on end-to-end encryption. Messages from here on can only be read on your devices and the other person's.",
        )
    });
    draw(&f, cx);
    assert_eq!(
        label(&f, cx, "encryption-lock"),
        "End-to-end encrypted (experimental)"
    );
    assert_eq!(
        label(&f, cx, "encryption-composer"),
        "Encrypted message to bob"
    );
    wait(cx, "the security code", |_| true);
    for _ in 0..200 {
        if shown(&f, cx, "encryption-panel-digits") {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
        cx.run_until_parked();
    }
    let digits = label(&f, cx, "encryption-panel-digits");
    assert_eq!(digits.split(' ').count(), 12, "{digits}");
    // Drawn below the digits, where the panel scrolls to it.
    assert!(present(&f, cx, "encryption-panel-qr"));
    audit(&f, cx, "an encrypted direct conversation with its panel");

    click(&f, cx, "composer-input");
    cx.update_window(f.window, |_, window, cx| {
        window.input("hello from the app", cx)
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(f.window, |_, window, cx| window.press("enter", cx))
        .unwrap();
    cx.run_until_parked();
    wait_until(&bob.client, WAIT, "bob reads it", |w| {
        has_text(w, "d-me", "hello from the app")
    });
    assert!(backend.plaintext().is_empty(), "{:?}", backend.plaintext());

    bob.engine.send("d-me", "hi back", None);
    wait(cx, "the answer decrypted", |cx| {
        let s = f.session.read(cx);
        let w = s.client().unwrap().read();
        has_text(&w, "d-bob", "hi back")
    });
    assert!(app(&f, cx, |w| has_text(w, "d-bob", "hello from the app")));

    reveal(&f, cx, "encryption-panel-verify");
    click(&f, cx, "encryption-panel-verify");
    wait(cx, "verified", |cx| {
        f.session.read(cx).client().is_some_and(|c| {
            c.read()
                .e2ee
                .conversation("d-bob")
                .is_some_and(|c| c.verified)
        })
    });
    draw(&f, cx);
    assert_eq!(
        label(&f, cx, "encryption-lock"),
        "End-to-end encrypted, bob verified (experimental)"
    );

    click(&f, cx, "encryption-panel-turn-off");
    click(&f, cx, "ok");
    wait(cx, "encryption off", |cx| {
        f.session
            .read(cx)
            .client()
            .is_some_and(|c| !c.read().e2ee.is_on("d-bob"))
    });
    wait_until(&bob.client, WAIT, "bob sees it off", |w| {
        !w.e2ee.is_on("d-me")
    });
}

#[gpui_kit::test]
fn a_new_device_shows_its_codes_and_another_approves_it(cx: &mut TestAppContext) {
    let backend = E2eeBackend::new();
    let rt = ircord_client::client::runtime().unwrap();
    // The account's first device, headless.
    let server = CannedServer::start(&rt);
    backend.serve(&server, "me", "d-bob", "m-bob", "bob");
    let phone = start_peer(
        &rt,
        server,
        &e2ee_snapshot("me", "d-bob", "m-bob", "bob"),
        "d-bob",
    );
    wait_until(&phone.client, WAIT, "the phone set up", |w| {
        w.e2ee.device.ready()
    });

    let shared = backend.clone();
    let f = open_with(cx, true, move |server| {
        shared.serve_device(server, "me");
        server.snapshot(&e2ee_snapshot("me", "d-bob", "m-bob", "bob"));
    });
    wait(cx, "this device waiting", |cx| {
        f.session
            .read(cx)
            .client()
            .is_some_and(|c| matches!(c.read().e2ee.device, DeviceState::Waiting { .. }))
    });
    let session = f.session.clone();
    cx.update(|cx| ircord_app::settings::open(&session, Some("encryption-device"), cx));
    draw(&f, cx);
    assert!(shown(&f, cx, "encryption-settings-qr"));
    assert!(shown(&f, cx, "encryption-settings-experimental"));

    // The code the web client asks for: 20 digits in groups of five, and
    // the request is already in the mailbox every approving device reads.
    let code = label(&f, cx, "encryption-settings-code");
    assert_eq!(code.len(), 23, "20 digits in groups of five: {code}");
    assert_eq!(backend.approvals_to("me", "pending").len(), 1);
    audit(&f, cx, "settings while waiting for approval");

    phone.engine.approve(&code);
    wait(cx, "approved", |cx| {
        f.session
            .read(cx)
            .client()
            .is_some_and(|c| c.read().e2ee.device.ready())
    });
    draw(&f, cx);
    assert!(!shown(&f, cx, "encryption-settings-qr"));
    assert!(shown(&f, cx, "encryption-settings-fingerprint"));
    wait(cx, "both devices listed", |cx| {
        f.session
            .read(cx)
            .client()
            .is_some_and(|c| c.read().e2ee.devices.len() == 2)
    });
    audit(&f, cx, "settings once approved");
    let _ = cx.update(|cx| {
        let s = f.session.read(cx);
        let w = s.client().unwrap().read();
        w.e2ee.devices.iter().filter(|d| d.approved).count()
    });
    assert!(
        cx.update_window(f.window, |_, window, _| window
            .find_all_by_role(gpui_kit::Role::ListItem)
            .len())
            .unwrap()
            >= 2
    );
}

/// Your channel with bob and eve, on a server with encryption on; no direct
/// conversation with bob yet. Eve has no account, so encryption can't reach her.
fn channel_snapshot() -> ircord_client::contract::Snapshot {
    let mut s = crate::fixture::snapshot();
    let home = &mut s.networks[0];
    home.account = Some("me".into());
    home.features.encryption = true;
    s.capabilities.encryption = Some(ircord_client::contract::EncryptionCapability {
        experimental: true,
        max_payload: 3800,
    });
    let eve = s.members.iter_mut().find(|m| m.id == "m-eve").unwrap();
    eve.account = "*".into();
    s
}

/// Right-click `id` and wait for its menu.
fn person_menu(f: &Fixture, cx: &mut TestAppContext, id: &'static str) {
    wait(cx, id, |cx| {
        f.window
            .update(cx, |_, window, cx| {
                window.render_frame(cx);
                window.try_find(id).is_some_and(|e| e.visible())
            })
            .unwrap_or(false)
    });
    cx.update_window(f.window, |_, window, cx| {
        window.hover(id, cx);
        window.right_click(id, cx);
    })
    .unwrap();
    cx.run_until_parked();
}

#[gpui_kit::test]
fn the_person_menu_starts_an_encrypted_chat(cx: &mut TestAppContext) {
    let backend = E2eeBackend::new();
    let shared = backend.clone();
    let dm = e2ee_snapshot("me", "d-bob", "m-bob", "bob")
        .conversations
        .into_iter()
        .find(|c| c.id == "d-bob")
        .unwrap();
    let f = open_with(cx, true, move |server| {
        shared.serve(server, "me", "d-bob", "m-bob", "bob");
        server.snapshot(&channel_snapshot());
        server.route(
            "POST",
            "/api/v1/conversations",
            200,
            serde_json::to_value(&dm).unwrap(),
        );
    });
    let rt = ircord_client::client::runtime().unwrap();
    let server = CannedServer::start(&rt);
    backend.serve(&server, "bob", "d-me", "m-me", "me");
    let bob = start_peer(
        &rt,
        server,
        &e2ee_snapshot("bob", "d-me", "m-me", "me"),
        "d-me",
    );
    wait_until(&bob.client, WAIT, "bob set up", |w| w.e2ee.device.ready());
    wait(cx, "this device set up", |cx| {
        f.session
            .read(cx)
            .client()
            .is_some_and(|c| c.read().e2ee.device.ready())
    });
    f.session
        .update(cx, |s, cx| s.select_conversation("c-general", cx));
    draw(&f, cx);

    // Eve has no account: the entry says why it can't start.
    person_menu(&f, cx, "member-m-eve");
    assert!(shown(&f, cx, "menu-unavailable-encryption-start-person"));
    assert_eq!(
        label(&f, cx, "menu-unavailable-reason-encryption-start-person"),
        "Encryption works between people on this IRCord server for now."
    );
    cx.update_window(f.window, |_, window, cx| window.press("escape", cx))
        .unwrap();
    cx.run_until_parked();

    // Bob: it opens the direct conversation and starts encrypting it.
    person_menu(&f, cx, "member-m-bob");
    assert!(!present(&f, cx, "menu-unavailable-encryption-start-person"));
    cx.update_window(f.window, |_, window, cx| {
        let at = window
            .find_all_by_role(gpui_kit::Role::MenuItem)
            .into_iter()
            .find(|i| i.visible() && i.label() == Some("Start encrypted chat"))
            .expect("the entry")
            .bounds()
            .center();
        window.pointer_down(at, gpui_kit::MouseButton::Left, cx);
        window.pointer_up(at, gpui_kit::MouseButton::Left, cx);
    })
    .unwrap();
    cx.run_until_parked();
    wait(cx, "the direct conversation and the panel", |cx| {
        let s = f.session.read(cx);
        s.conversation.as_deref() == Some("d-bob")
            && s.panel.as_deref() == Some(ircord_app::features::encryption::PANEL)
    });
    wait(cx, "encryption on", |cx| {
        f.session
            .read(cx)
            .client()
            .is_some_and(|c| c.read().e2ee.is_on("d-bob"))
    });
    wait_until(&bob.client, WAIT, "bob's side on", |w| w.e2ee.is_on("d-me"));
    assert!(backend.plaintext().is_empty(), "{:?}", backend.plaintext());

    // Once on, the person menu no longer offers it.
    f.session.update(cx, |s, cx| {
        s.select_conversation("c-general", cx);
        s.set_panel(None, cx);
    });
    draw(&f, cx);
    person_menu(&f, cx, "member-m-bob");
    assert!(!present(&f, cx, "menu-unavailable-encryption-start-person"));
    let mut rows = Vec::new();
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        rows = crate::fixture::menu_rows(window, false);
    })
    .unwrap();
    assert!(rows.iter().any(|r| r == "Message"), "{rows:?}");
    assert!(
        !rows.iter().any(|r| r == "Start encrypted chat"),
        "already encrypted: {rows:?}"
    );
}

/// The other way round: this device set up encryption first, and a new
/// device of the account (as the web client's would) waits with its code.
/// The approval form counts it, refuses a wrong code, and approves the right
/// one typed in.
#[gpui_kit::test]
fn this_device_approves_a_new_one_by_its_code(cx: &mut TestAppContext) {
    let backend = E2eeBackend::new();
    let shared = backend.clone();
    let f = open_with(cx, true, move |server| {
        shared.serve(server, "me", "d-bob", "m-bob", "bob");
        server.snapshot(&e2ee_snapshot("me", "d-bob", "m-bob", "bob"));
    });
    wait(cx, "this device set up", |cx| {
        f.session
            .read(cx)
            .client()
            .is_some_and(|c| c.read().e2ee.device.ready())
    });
    let session = f.session.clone();
    cx.update(|cx| ircord_app::settings::open(&session, Some("encryption-device"), cx));
    draw(&f, cx);
    assert!(shown(&f, cx, "encryption-approve"));
    assert_eq!(
        label(&f, cx, "encryption-approve-waiting"),
        "No device is waiting. Open Settings → Encryption on the new device first."
    );

    let rt = ircord_client::client::runtime().unwrap();
    let server = CannedServer::start(&rt);
    backend.serve_device(&server, "me");
    let laptop = start_peer(
        &rt,
        server,
        &e2ee_snapshot("me", "d-bob", "m-bob", "bob"),
        "d-bob",
    );
    wait_until(&laptop.client, WAIT, "the laptop to wait", |w| {
        matches!(w.e2ee.device, DeviceState::Waiting { .. })
    });
    let DeviceState::Waiting { code, .. } = laptop.client.read().e2ee.device.clone() else {
        unreachable!()
    };
    for _ in 0..200 {
        cx.update(|cx| ircord_app::settings::open(&session, Some("encryption-device"), cx));
        draw(&f, cx);
        if app(&f, cx, |w| w.e2ee.waiting_approvals == 1) {
            break;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    draw(&f, cx);
    assert_eq!(
        label(&f, cx, "encryption-approve-waiting"),
        "1 device is waiting for approval."
    );
    audit(&f, cx, "settings with a device waiting for approval");

    type_enter(&f, cx, "encryption-approve-code", "11111 11111 11111 11111");
    wait(cx, "the refusal", |cx| {
        f.session.read(cx).client().is_some_and(|c| {
            matches!(
                c.read().e2ee.approving,
                ircord_client::store::e2ee::Approving::Failed(_)
            )
        })
    });
    draw(&f, cx);
    assert_eq!(
        label(&f, cx, "encryption-approve-outcome"),
        "No waiting device has this code. Check the code, or open it on the new device again."
    );

    type_enter(&f, cx, "encryption-approve-code", &code);
    wait(cx, "approved here", |cx| {
        f.session
            .read(cx)
            .client()
            .is_some_and(|c| c.read().e2ee.approving == ircord_client::store::e2ee::Approving::Done)
    });
    for _ in 0..200 {
        laptop.engine.poke();
        std::thread::sleep(Duration::from_millis(25));
        if laptop.client.read().e2ee.device.ready() {
            break;
        }
    }
    assert!(
        laptop.client.read().e2ee.device.ready(),
        "the laptop is approved"
    );
    draw(&f, cx);
    assert_eq!(
        label(&f, cx, "encryption-approve-outcome"),
        "The device is approved."
    );
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

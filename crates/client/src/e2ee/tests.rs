//! Two (and three) devices against the fake backend in
//! `crate::testing::E2eeBackend`: set-up, starting an encrypted chat,
//! sending both ways, history from the vault after a restart, turning it off,
//! approving a second device, and a peer who starts fresh.

use std::sync::Arc;
use std::time::Duration;

use ircord_contract::{MessageKind, Snapshot};
use tokio::runtime::Runtime;

use super::approval;
use super::vault::VaultKey;
use super::{Engine, Texts};
use crate::Client;
use crate::client::runtime;
use crate::events::Update;
use crate::store::Workspace;
use crate::store::e2ee::{ApprovalFailure, Approving, Attention, DeviceState, PLACEHOLDER};
use crate::testing::{CannedServer, E2eeBackend, e2ee_snapshot, frame, wait_live, wait_until};

const WAIT: Duration = Duration::from_secs(20);

fn texts() -> Texts {
    Texts {
        control: Box::new(|on, name, mine| {
            format!(
                "{name} turned {} (mine: {mine})",
                if on { "on" } else { "off" }
            )
        }),
        unreadable: Box::new(|why| format!("unreadable: {why:?}")),
    }
}

struct Device {
    server: CannedServer,
    client: Client,
    engine: Engine,
    _updates: futures::channel::mpsc::UnboundedReceiver<Update>,
}

fn device(
    rt: &Arc<Runtime>,
    server: CannedServer,
    snapshot: &Snapshot,
    dir: &std::path::Path,
    key: &VaultKey,
    conversation: &str,
) -> Device {
    server.snapshot(snapshot);
    let (client, updates) = Client::start(rt.clone(), server.api());
    let engine = Engine::start(
        client.handle().clone(),
        rt.handle().clone(),
        dir.to_path_buf(),
        key.clone(),
        "IRCord GPUI test".into(),
        texts(),
    );
    wait_live(&client);
    client.open(conversation);
    Device {
        server,
        client,
        engine,
        _updates: updates,
    }
}

#[allow(clippy::too_many_arguments)]
fn person(
    rt: &Arc<Runtime>,
    backend: &E2eeBackend,
    account: &str,
    conversation: &str,
    peer_member: &str,
    peer: &str,
    dir: &std::path::Path,
    key: &VaultKey,
) -> Device {
    let server = CannedServer::start(rt);
    backend.serve(&server, account, conversation, peer_member, peer);
    let snapshot = e2ee_snapshot(account, conversation, peer_member, peer);
    device(rt, server, &snapshot, dir, key, conversation)
}

fn texts_in(w: &Workspace, conversation: &str) -> Vec<String> {
    w.timelines
        .get(conversation)
        .map(|t| t.entries.iter().map(|e| e.message.text.clone()).collect())
        .unwrap_or_default()
}

fn has_text(w: &Workspace, conversation: &str, text: &str) -> bool {
    texts_in(w, conversation).iter().any(|t| t == text)
}

fn ready(d: &Device) {
    wait_until(&d.client, WAIT, "the device to be set up", |w| {
        w.e2ee.device.ready()
    });
}

#[test]
fn an_encrypted_chat_from_start_to_turning_it_off() {
    let rt = runtime().unwrap();
    let backend = E2eeBackend::new();
    let (da, db) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (ka, kb) = (VaultKey::random(), VaultKey::random());
    let alice = person(
        &rt,
        &backend,
        "alice",
        "dm-bob",
        "m-bob",
        "bob",
        da.path(),
        &ka,
    );
    let bob = person(
        &rt,
        &backend,
        "bob",
        "dm-alice",
        "m-alice",
        "alice",
        db.path(),
        &kb,
    );
    ready(&alice);
    ready(&bob);
    assert_eq!(backend.devices("alice").len(), 1);
    assert!(matches!(
        alice.client.read().e2ee.device,
        DeviceState::Ready { approver: true }
    ));
    assert!(alice.client.read().e2ee.fingerprint.is_some());

    alice.engine.start_chat("dm-bob");
    wait_until(&alice.client, WAIT, "alice's side on", |w| {
        w.e2ee.is_on("dm-bob")
    });
    wait_until(&bob.client, WAIT, "bob's side on", |w| {
        w.e2ee.is_on("dm-alice")
    });
    wait_until(&bob.client, WAIT, "the control line", |w| {
        has_text(w, "dm-alice", "alice turned on (mine: false)")
    });
    let group = backend.binding("alice", "dm-bob");
    assert!(group.is_some());
    assert_eq!(group, backend.binding("bob", "dm-alice"));

    alice.engine.send("dm-bob", "hello bob", None);
    wait_until(&bob.client, WAIT, "bob to read it", |w| {
        has_text(w, "dm-alice", "hello bob")
    });
    wait_until(&alice.client, WAIT, "alice's own copy", |w| {
        w.outbox.is_empty() && has_text(w, "dm-bob", "hello bob")
    });
    bob.engine.send("dm-alice", "/me waves", None);
    wait_until(&alice.client, WAIT, "alice to read it", |w| {
        w.timelines["dm-bob"]
            .entries
            .iter()
            .any(|e| e.message.text == "waves" && e.message.kind == MessageKind::Action)
    });

    // A server cannot make up the text of an encrypted message, nor pass a
    // message off as someone else's.
    let mut fake = crate::testing::message("fake-1", "dm-bob", "m-bob", "trust me, I'm encrypted");
    fake.encrypted = Some(ircord_contract::Encrypted {
        version: 1,
        class: ircord_contract::EncryptedClass::Message,
        payload: "AAAA".into(),
    });
    fake.sender_account = Some("bob".into());
    backend.deliver("alice", fake);
    // Bob's next message, shown as Alice's own.
    backend.tamper(Some(Arc::new(|m: &mut ircord_contract::Message| {
        m.id = "moved-1".into();
        m.sender_id = crate::testing::ME.into();
    })));
    bob.engine.send("dm-alice", "said by bob", None);
    wait_until(&alice.client, WAIT, "both judged", |w| {
        w.e2ee.shown.contains_key("fake-1") && w.e2ee.shown.contains_key("moved-1")
    });
    backend.tamper(None);
    {
        let w = alice.client.read();
        assert!(!has_text(&w, "dm-bob", "trust me, I'm encrypted"));
        assert_eq!(
            w.e2ee.shown["moved-1"],
            crate::store::e2ee::Shown::Unreadable(super::vault::Unreadable::Forged)
        );
    }

    // Nothing went over the wire in the clear.
    assert!(backend.plaintext().is_empty(), "{:?}", backend.plaintext());
    for m in backend.history("bob") {
        assert_eq!(m.text, PLACEHOLDER);
        assert!(m.encrypted.is_some());
    }
    assert!(!format!("{:?}", backend.history("bob")).contains("hello bob"));

    // A long message goes in parts and arrives whole.
    let long = "x".repeat(6000);
    alice.engine.send("dm-bob", &long, None);
    wait_until(&bob.client, WAIT, "the long message", |w| {
        has_text(w, "dm-alice", &long)
    });

    // Bob restarts: history comes from his vault, since MLS decrypts once.
    let server = bob.server;
    drop(bob.engine);
    drop(bob.client);
    let mut snapshot = e2ee_snapshot("bob", "dm-alice", "m-alice", "alice");
    snapshot.cursor = backend.cursor("bob");
    let bob = device(&rt, server, &snapshot, db.path(), &kb, "dm-alice");
    wait_until(&bob.client, WAIT, "history from the vault", |w| {
        has_text(w, "dm-alice", "hello bob") && has_text(w, "dm-alice", "waves")
    });
    wait_until(&bob.client, WAIT, "the state from the vault", |w| {
        w.e2ee.is_on("dm-alice")
    });
    // The parts before the last stay hidden.
    wait_until(&bob.client, WAIT, "the long message from the vault", |w| {
        has_text(w, "dm-alice", &long)
            && w.e2ee
                .shown
                .values()
                .filter(|s| **s == crate::store::e2ee::Shown::Hidden)
                .count()
                >= 2
    });

    // Search finds it on this device.
    let hits = rt.block_on(bob.engine.search("hello", 10));
    assert_eq!(hits.len(), 1);

    alice.engine.turn_off("dm-bob");
    wait_until(&bob.client, WAIT, "bob's side off", |w| {
        !w.e2ee.is_on("dm-alice")
    });
    wait_until(&alice.client, WAIT, "alice's side off", |w| {
        !w.e2ee.is_on("dm-bob")
    });
    assert_eq!(backend.binding("alice", "dm-bob"), None);
}

#[test]
fn a_second_device_is_approved_by_code_and_reads_what_comes_after() {
    let rt = runtime().unwrap();
    let backend = E2eeBackend::new();
    let (da, db, da2) = (
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
    );
    let alice = person(
        &rt,
        &backend,
        "alice",
        "dm-bob",
        "m-bob",
        "bob",
        da.path(),
        &VaultKey::random(),
    );
    let bob = person(
        &rt,
        &backend,
        "bob",
        "dm-alice",
        "m-alice",
        "alice",
        db.path(),
        &VaultKey::random(),
    );
    ready(&alice);
    ready(&bob);
    alice.engine.start_chat("dm-bob");
    wait_until(&bob.client, WAIT, "bob's side on", |w| {
        w.e2ee.is_on("dm-alice")
    });
    bob.engine.send("dm-alice", "before the new device", None);
    wait_until(&alice.client, WAIT, "alice reads it", |w| {
        has_text(w, "dm-bob", "before the new device")
    });

    // Alice's laptop.
    let server = CannedServer::start(&rt);
    backend.serve_device(&server, "alice");
    let snapshot = e2ee_snapshot("alice", "dm-bob", "m-bob", "bob");
    let laptop = device(
        &rt,
        server,
        &snapshot,
        da2.path(),
        &VaultKey::random(),
        "dm-bob",
    );
    wait_until(&laptop.client, WAIT, "the laptop to wait", |w| {
        matches!(&w.e2ee.device, DeviceState::Waiting { .. })
    });
    let (qr, code) = match &laptop.client.read().e2ee.device {
        DeviceState::Waiting { qr, code } => (qr.clone(), code.clone()),
        _ => unreachable!(),
    };
    // The code the web client's approval form takes: 20 digits in groups
    // of five, and the QR code it scans carries the same digits.
    let digits = approval::normalize(&code).expect("a 20-digit code");
    assert_eq!(code, approval::format_code(&digits));
    assert_eq!(qr, approval::qr_text(&digits));
    // The request sits in the account's mailbox, where every approving
    // device (the web client's too) looks, made under that code.
    let posted = backend.approvals_to("alice", approval::MAILBOX);
    assert_eq!(posted.len(), 1, "one request in the mailbox");
    let request = crate::api::e2ee::bytes(&posted[0].1).unwrap();
    assert!(approval::open_request(&digits, &request).is_some());
    for _ in 0..200 {
        alice.engine.poke();
        if alice.client.read().e2ee.waiting_approvals == 1 {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(
        alice.client.read().e2ee.waiting_approvals,
        1,
        "the phone counts it"
    );

    // A code no waiting device made is refused, and so is a short one.
    alice.engine.approve("11111 11111 11111 11111");
    wait_until(&alice.client, WAIT, "the refusal", |w| {
        w.e2ee.approving == Approving::Failed(ApprovalFailure::NotFound)
    });
    alice.engine.approve("1234");
    wait_until(&alice.client, WAIT, "the short code", |w| {
        w.e2ee.approving == Approving::Failed(ApprovalFailure::BadCode)
    });
    assert!(!laptop.client.read().e2ee.device.ready());

    alice.engine.approve(&code);
    wait_until(&alice.client, WAIT, "the approval", |w| {
        w.e2ee.approving == Approving::Done
    });
    assert!(
        backend.approvals_to("alice", approval::MAILBOX).is_empty(),
        "the answered request is gone"
    );
    for _ in 0..200 {
        laptop.engine.poke();
        alice.engine.poke();
        std::thread::sleep(Duration::from_millis(50));
        if laptop.client.read().e2ee.device.ready()
            && backend.welcomes_waiting("alice") == 0
            && backend.devices("alice").len() == 2
        {
            let w = laptop.client.read();
            if w.e2ee.is_on("dm-bob") {
                break;
            }
        }
    }
    assert!(laptop.client.read().e2ee.device.ready());
    assert!(
        laptop.client.read().e2ee.is_on("dm-bob"),
        "the approval carried the conversation"
    );
    // The phone adds the laptop to its group; the laptop joins from the
    // Welcome (the phone made the group, so no device of Alice's took one
    // before).
    for _ in 0..200 {
        alice.engine.poke();
        laptop.engine.poke();
        std::thread::sleep(Duration::from_millis(50));
        if backend.welcomes_taken("alice") >= 1 && backend.welcomes_waiting("alice") == 0 {
            break;
        }
    }
    assert!(backend.welcomes_taken("alice") >= 1, "the laptop joined");

    // Bob's next message reaches both of Alice's devices; the laptop never
    // gets the earlier one's keys.
    bob.engine.send("dm-alice", "to both of you", None);
    wait_until(&alice.client, WAIT, "the phone reads it", |w| {
        has_text(w, "dm-bob", "to both of you")
    });
    for (seq, m) in backend.history("alice").into_iter().enumerate() {
        laptop
            .server
            .push(frame(seq as i64 + 1, "message.created", &m));
    }
    wait_until(&laptop.client, WAIT, "the laptop reads the new one", |w| {
        has_text(w, "dm-bob", "to both of you")
    });
    wait_until(&laptop.client, WAIT, "the earlier one marked", |w| {
        has_text(
            w,
            "dm-bob",
            &format!("unreadable: {:?}", super::vault::Unreadable::BeforeDevice),
        )
    });
    let w = laptop.client.read();
    assert!(!has_text(&w, "dm-bob", "before the new device"));
    assert!(has_text(
        &w,
        "dm-bob",
        &format!("unreadable: {:?}", super::vault::Unreadable::BeforeDevice)
    ));
}

#[test]
fn a_peer_who_starts_fresh_is_shown_and_acknowledged() {
    let rt = runtime().unwrap();
    let backend = E2eeBackend::new();
    let (da, db) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let alice = person(
        &rt,
        &backend,
        "alice",
        "dm-bob",
        "m-bob",
        "bob",
        da.path(),
        &VaultKey::random(),
    );
    let bob = person(
        &rt,
        &backend,
        "bob",
        "dm-alice",
        "m-alice",
        "alice",
        db.path(),
        &VaultKey::random(),
    );
    ready(&alice);
    ready(&bob);
    alice.engine.start_chat("dm-bob");
    wait_until(&bob.client, WAIT, "bob's side on", |w| {
        w.e2ee.is_on("dm-alice")
    });
    alice.engine.set_verified("dm-bob", true);
    wait_until(&alice.client, WAIT, "verified", |w| {
        w.e2ee.conversation("dm-bob").is_some_and(|c| c.verified)
    });
    let code = rt.block_on(alice.engine.code("dm-bob")).unwrap();
    assert_eq!(code.groups.len(), 12);
    assert_eq!(
        Some(code.groups.clone()),
        rt.block_on(bob.engine.code("dm-alice")).map(|c| c.groups)
    );

    bob.engine.start_fresh();
    std::thread::sleep(Duration::from_millis(300));
    wait_until(&bob.client, WAIT, "bob set up again", |w| {
        w.e2ee.device.ready()
    });
    bob.engine.send("dm-alice", "after starting fresh", None);
    for _ in 0..100 {
        alice.engine.poke();
        std::thread::sleep(Duration::from_millis(50));
        if alice
            .client
            .read()
            .e2ee
            .conversation("dm-bob")
            .is_some_and(|c| c.attention.is_some())
        {
            break;
        }
    }
    {
        let w = alice.client.read();
        let c = w.e2ee.conversation("dm-bob").unwrap();
        assert!(matches!(c.attention, Some(Attention::StartedFresh { .. })));
        assert!(!c.verified, "a new identity is not verified");
        assert!(!has_text(&w, "dm-bob", "after starting fresh"));
    }
    alice.engine.acknowledge("dm-bob");
    wait_until(&alice.client, WAIT, "the waiting message", |w| {
        has_text(w, "dm-bob", "after starting fresh")
    });
    alice.engine.send("dm-bob", "welcome back", None);
    wait_until(&bob.client, WAIT, "bob reads the answer", |w| {
        has_text(w, "dm-alice", "welcome back")
    });
    assert!(backend.plaintext().is_empty());
}

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
        left: Box::new(|name, mine| format!("{name} left (mine: {mine})")),
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

/// A group id as text: 32 lowercase hex digits (`docs/wire.md` 7.6).
fn is_hex_group(text: &str) -> bool {
    text.len() == 32 && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
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
    assert!(group.as_deref().is_some_and(is_hex_group), "{group:?}");
    assert_eq!(group, backend.binding("bob", "dm-alice"));
    assert_eq!(backend.groups(), vec![group.clone().unwrap()]);

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
    // The group log is named in hex (`docs/wire.md` 7.6).
    let group = backend.binding("alice", "dm-bob").unwrap();
    assert!(backend.commits(&group) >= 1, "the commit went to {group}");

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

/// Alice and Bob, each with one device, set up.
struct Pair {
    rt: Arc<Runtime>,
    backend: E2eeBackend,
    alice: Device,
    /// None while his app is not running.
    bob: Option<Device>,
    bob_dir: tempfile::TempDir,
    bob_key: VaultKey,
    _alice_dir: tempfile::TempDir,
}

fn pair() -> Pair {
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
    Pair {
        rt,
        backend,
        alice,
        bob: Some(bob),
        bob_dir: db,
        bob_key: kb,
        _alice_dir: da,
    }
}

impl Pair {
    fn bob(&self) -> &Device {
        self.bob.as_ref().expect("bob's app runs")
    }

    /// Bob's app quits; `away` runs; then it starts again from his vault.
    fn bob_away(&mut self, away: impl FnOnce(&Pair)) {
        let Device { server, .. } = self.bob.take().unwrap();
        away(self);
        let mut snapshot = e2ee_snapshot("bob", "dm-alice", "m-alice", "alice");
        snapshot.cursor = self.backend.cursor("bob");
        let bob = device(
            &self.rt,
            server,
            &snapshot,
            self.bob_dir.path(),
            &self.bob_key,
            "dm-alice",
        );
        ready(&bob);
        self.bob = Some(bob);
    }

    fn turn_on(&self, starter: &Device, conversation: &str) {
        starter.engine.start_chat(conversation);
        wait_until(&starter.client, WAIT, "the starter's side on", |w| {
            w.e2ee.is_on(conversation) || w.e2ee.errors.contains_key(conversation)
        });
        assert_eq!(starter.client.read().e2ee.errors.get(conversation), None);
        self.both(true);
    }

    fn turn_off(&self, who: &Device, conversation: &str) {
        who.engine.turn_off(conversation);
        self.both(false);
        assert_eq!(who.client.read().e2ee.errors.get(conversation), None);
    }

    /// Both sides show encryption `on`, and the server agrees: one group,
    /// bound for both, or none.
    fn both(&self, on: bool) {
        wait_until(&self.alice.client, WAIT, "alice's side", |w| {
            w.e2ee.is_on("dm-bob") == on
        });
        wait_until(&self.bob().client, WAIT, "bob's side", |w| {
            w.e2ee.is_on("dm-alice") == on
        });
        let bindings = || {
            (
                self.backend.binding("alice", "dm-bob"),
                self.backend.binding("bob", "dm-alice"),
            )
        };
        // The other side binds its own conversation when it reads the notice.
        let deadline = std::time::Instant::now() + WAIT;
        while bindings().1.is_some() != on && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        let (a, b) = bindings();
        assert_eq!(a.is_some(), on, "alice's binding: {a:?}");
        assert_eq!(a, b, "both bound to one group");
    }

    /// A message each way is read on the other side.
    fn talk(&self, words: &str) {
        let (to_bob, to_alice) = (format!("{words} to bob"), format!("{words} to alice"));
        self.alice.engine.send("dm-bob", &to_bob, None);
        wait_until(&self.bob().client, WAIT, "bob reads it", |w| {
            has_text(w, "dm-alice", &to_bob)
        });
        self.bob().engine.send("dm-alice", &to_alice, None);
        wait_until(&self.alice.client, WAIT, "alice reads it", |w| {
            has_text(w, "dm-bob", &to_alice)
        });
        assert!(
            self.backend.plaintext().is_empty(),
            "{:?}",
            self.backend.plaintext()
        );
    }
}

#[test]
fn encryption_turns_off_and_on_again_from_either_side() {
    let p = pair();
    p.turn_on(&p.alice, "dm-bob");
    let first = p.backend.binding("alice", "dm-bob");
    p.talk("first");
    p.turn_off(&p.alice, "dm-bob");
    // The server answered the first "turn on again" with 409 here.
    p.turn_on(&p.alice, "dm-bob");
    assert_ne!(p.backend.binding("alice", "dm-bob"), first);
    p.talk("second");
    p.turn_off(p.bob(), "dm-alice");
    p.turn_on(p.bob(), "dm-alice");
    p.talk("third");
    p.turn_off(&p.alice, "dm-bob");
    p.turn_on(p.bob(), "dm-alice");
    p.talk("fourth");
}

#[test]
fn an_off_sent_while_the_other_was_away_holds_for_both() {
    let mut p = pair();
    p.bob_away(|p| {
        p.alice.engine.start_chat("dm-bob");
        wait_until(&p.alice.client, WAIT, "alice's side on", |w| {
            w.e2ee.is_on("dm-bob")
        });
        p.alice.engine.send("dm-bob", "while you were away", None);
        wait_until(&p.alice.client, WAIT, "alice's message sent", |w| {
            w.outbox.is_empty() && has_text(w, "dm-bob", "while you were away")
        });
        p.alice.engine.turn_off("dm-bob");
        wait_until(&p.alice.client, WAIT, "alice's side off", |w| {
            !w.e2ee.is_on("dm-bob")
        });
        assert_eq!(p.backend.binding("alice", "dm-bob"), None);
    });
    // Bob joins the group from its Welcome first, then reads the history.
    wait_until(&p.bob().client, WAIT, "bob reads the history", |w| {
        has_text(w, "dm-alice", "while you were away")
            && has_text(w, "dm-alice", "alice turned off (mine: false)")
    });
    assert!(p.backend.welcomes_taken("bob") >= 1);
    // Until here his Welcome bound the turned-off group again.
    p.both(false);
    // Reading it all again from the vault changes nothing.
    p.bob_away(|_| {});
    wait_until(&p.bob().client, WAIT, "bob's history again", |w| {
        has_text(w, "dm-alice", "alice turned off (mine: false)")
    });
    std::thread::sleep(Duration::from_millis(300));
    p.both(false);
    p.turn_on(&p.alice, "dm-bob");
    p.talk("back");
    p.turn_off(p.bob(), "dm-alice");
    p.turn_on(p.bob(), "dm-alice");
    p.talk("again");
}

#[test]
fn a_group_an_older_client_bound_again_is_replaced_and_ended() {
    let p = pair();
    p.turn_on(&p.alice, "dm-bob");
    let first = p.backend.binding("alice", "dm-bob").unwrap();
    p.turn_off(&p.alice, "dm-bob");
    // An older client of Bob's missed the "off" and bound the group again:
    // the server names it to Bob and nothing to Alice.
    p.backend.force_bind("bob", "dm-alice", "alice", &first);
    p.turn_on(&p.alice, "dm-bob");
    let second = p.backend.binding("alice", "dm-bob").unwrap();
    assert_ne!(second, first);
    p.talk("replaced");
    // Again, while the second group is on: turning off ends the old one too.
    p.backend.force_bind("bob", "dm-alice", "alice", &first);
    p.turn_off(&p.alice, "dm-bob");
    p.turn_on(p.bob(), "dm-alice");
    p.talk("after");
}

#[test]
fn a_conversation_from_before_hex_group_ids_keeps_its_group() {
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
    alice.engine.start_chat("dm-bob");
    wait_until(&bob.client, WAIT, "bob's side on", |w| {
        w.e2ee.is_on("dm-alice")
    });
    alice.engine.send("dm-bob", "before", None);
    wait_until(&bob.client, WAIT, "bob reads it", |w| {
        has_text(w, "dm-alice", "before")
    });

    // Both stop, and everything is put as 0.9.0 left it: the group named in
    // base64url on the backend and in both vaults.
    let hex = backend.binding("alice", "dm-bob").unwrap();
    let legacy = super::b64url(&super::core::GroupIdBytes::from_hex(&hex).unwrap().0);
    assert_eq!(legacy.len(), 22);
    let (sa, sb) = (alice.server, bob.server);
    drop((alice.engine, alice.client, bob.engine, bob.client));
    std::thread::sleep(Duration::from_millis(300));
    for (dir, key, conversation) in [(da.path(), &ka, "dm-bob"), (db.path(), &kb, "dm-alice")] {
        let mut vault = super::vault::Vault::open(dir, key.clone()).unwrap();
        let mut record = vault.conversation(conversation).cloned().unwrap();
        assert_eq!(record.group.as_deref(), Some(hex.as_str()));
        record.group = Some(legacy.clone());
        vault
            .put(super::vault::Record::Conversation(record))
            .unwrap();
    }
    backend.rename_group(&hex, &legacy);
    let earlier = sa.requests().len();

    let mut snapshot = e2ee_snapshot("alice", "dm-bob", "m-bob", "bob");
    snapshot.cursor = backend.cursor("alice");
    let alice = device(&rt, sa, &snapshot, da.path(), &ka, "dm-bob");
    let mut snapshot = e2ee_snapshot("bob", "dm-alice", "m-alice", "alice");
    snapshot.cursor = backend.cursor("bob");
    let bob = device(&rt, sb, &snapshot, db.path(), &kb, "dm-alice");
    ready(&alice);
    ready(&bob);
    wait_until(&alice.client, WAIT, "history from the vault", |w| {
        has_text(w, "dm-bob", "before") && w.e2ee.is_on("dm-bob")
    });

    alice.engine.send("dm-bob", "after", None);
    wait_until(&bob.client, WAIT, "bob reads it", |w| {
        has_text(w, "dm-alice", "after")
    });
    bob.engine.send("dm-alice", "and back", None);
    wait_until(&alice.client, WAIT, "alice reads it", |w| {
        has_text(w, "dm-bob", "and back")
    });

    // The same group, under its old name: no new group, and the log is read
    // where it is.
    assert_eq!(backend.groups(), vec![legacy.clone()]);
    assert_eq!(backend.binding("alice", "dm-bob"), Some(legacy.clone()));
    let commits = "/api/v1/e2ee/groups/*/commits";
    let reads: Vec<String> = alice.server.requests()[earlier..]
        .iter()
        .filter(|r| r.method == "GET" && r.path.ends_with("/commits"))
        .map(|r| r.path.clone())
        .collect();
    assert!(!reads.is_empty(), "no reads of {commits}");
    assert!(reads.iter().all(|p| p.contains(&legacy)), "{reads:?}");
    assert!(backend.plaintext().is_empty());
}

/// The conversation of `d`'s only encrypted group, once it has one.
fn group_conversation(d: &Device) -> String {
    wait_until(&d.client, WAIT, "a group", |w| !w.e2ee.groups.is_empty());
    let id = d.client.read().e2ee.groups.keys().next().unwrap().clone();
    d.client.open(&id);
    id
}

#[test]
fn an_encrypted_group_across_two_servers_from_invitation_to_leaving() {
    let rt = runtime().unwrap();
    let backend = E2eeBackend::new();
    let dirs: Vec<_> = (0..3).map(|_| tempfile::tempdir().unwrap()).collect();
    let keys: Vec<_> = (0..3).map(|_| VaultKey::random()).collect();
    let alice = person(
        &rt,
        &backend,
        "alice",
        "dm-bob",
        "m-bob",
        "bob",
        dirs[0].path(),
        &keys[0],
    );
    let bob = person(
        &rt,
        &backend,
        "bob",
        "dm-alice",
        "m-alice",
        "alice",
        dirs[1].path(),
        &keys[1],
    );
    // Carol's keys are on another IRCord server of the same network.
    backend.elsewhere("carol");
    let carol = person(
        &rt,
        &backend,
        "carol",
        "dm-x",
        "m-x",
        "x",
        dirs[2].path(),
        &keys[2],
    );
    for d in [&alice, &bob, &carol] {
        ready(d);
    }
    // Each account's identity signs its list of devices.
    wait_until(&carol.client, WAIT, "carol's device list", |_| {
        backend.device_list("carol").is_some()
    });

    alice
        .engine
        .create_group("Weekend plans", vec!["bob".into(), "carol".into()]);
    let a = group_conversation(&alice);
    {
        let w = alice.client.read();
        let g = &w.e2ee.groups[&a];
        assert_eq!(g.title, "Weekend plans");
        assert!(g.creator && g.admin);
        assert_eq!(g.members.len(), 3);
        assert_eq!(g.verified(), (0, 2));
        assert!(w.e2ee.is_on(&a));
        assert!(crate::store::e2ee::guarded(&w, &a), "never in the clear");
    }
    let chat = alice.client.read().e2ee.groups[&a].chat.clone();
    assert_eq!(backend.channel_members(&chat), vec!["alice".to_string()]);

    // An invitation to answer, with the title only members can read.
    for d in [&bob, &carol] {
        wait_until(&d.client, WAIT, "the invitation", |w| {
            w.e2ee
                .invitations
                .iter()
                .any(|i| i.title == "Weekend plans" && i.inviter == "alice" && i.people == 3)
        });
        d.engine.accept_invitation(&chat);
    }
    let b = group_conversation(&bob);
    let c = group_conversation(&carol);
    wait_until(&bob.client, WAIT, "no invitation left", |w| {
        w.e2ee.invitations.is_empty()
    });
    assert_eq!(
        crate::store::parties::title(
            &bob.client.read(),
            bob.client.read().conversation(&b).unwrap()
        ),
        "Weekend plans",
        "listed by its title"
    );

    alice.engine.send(&a, "hello group", None);
    for (d, conv) in [(&bob, &b), (&carol, &c)] {
        wait_until(&d.client, WAIT, "the group message", |w| {
            has_text(w, conv, "hello group")
        });
    }
    carol.engine.send(&c, "hi from the other server", None);
    for (d, conv) in [(&alice, &a), (&bob, &b)] {
        wait_until(&d.client, WAIT, "carol's message", |w| {
            has_text(w, conv, "hi from the other server")
        });
    }

    // Only the creator and admins remove people; bob is neither.
    assert!(!bob.client.read().e2ee.groups[&b].admin);
    alice.engine.remove_person(&a, "carol");
    wait_until(&carol.client, WAIT, "carol out of the group", |w| {
        w.e2ee.groups.is_empty()
    });
    wait_until(&alice.client, WAIT, "two people left", |w| {
        w.e2ee.groups[&a].members.len() == 2
    });
    alice.engine.send(&a, "without carol", None);
    wait_until(&bob.client, WAIT, "bob reads on", |w| {
        has_text(w, &b, "without carol")
    });
    assert!(!has_text(&carol.client.read(), &c, "without carol"));
    assert!(
        alice.client.read().e2ee.groups[&a]
            .events
            .iter()
            .any(|e| matches!(e, crate::store::e2ee::GroupEvent::Removed { account, .. } if account == "carol"))
    );

    // Bob leaves: alice's client sees his "left" and removes him.
    bob.engine.leave_group(&b);
    wait_until(&alice.client, WAIT, "the left line", |w| {
        has_text(w, &a, "bob left (mine: false)")
    });
    wait_until(&alice.client, WAIT, "bob removed", |w| {
        w.e2ee.groups[&a].members.len() == 1
    });
    wait_until(&bob.client, WAIT, "bob's group gone", |w| {
        w.e2ee.groups.is_empty()
    });

    assert!(backend.plaintext().is_empty(), "{:?}", backend.plaintext());
    for account in ["alice", "bob", "carol"] {
        for m in backend.history(account) {
            assert!(m.encrypted.is_some() && m.text == PLACEHOLDER, "{m:?}");
        }
    }
}

#[test]
fn a_direct_chat_with_someone_on_another_server_goes_over_irc() {
    let rt = runtime().unwrap();
    let backend = E2eeBackend::new();
    let (da, dd) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (ka, kd) = (VaultKey::random(), VaultKey::random());
    let alice = person(
        &rt,
        &backend,
        "alice",
        "dm-dave",
        "m-dave",
        "dave",
        da.path(),
        &ka,
    );
    backend.elsewhere("dave");
    let dave = person(
        &rt,
        &backend,
        "dave",
        "dm-alice",
        "m-alice",
        "alice",
        dd.path(),
        &kd,
    );
    ready(&alice);
    ready(&dave);

    alice.engine.start_chat("dm-dave");
    wait_until(&dave.client, WAIT, "dave's side on", |w| {
        w.e2ee.is_on("dm-alice")
    });
    // No group log on this server: the chat is carried over IRC.
    assert!(backend.groups().is_empty(), "{:?}", backend.groups());
    let chats = backend.chats("alice");
    assert_eq!(chats.len(), 1);
    assert_eq!((chats[0].1.as_str(), chats[0].2), ("direct", true));
    assert!(is_hex_group(&chats[0].0), "{}", chats[0].0);
    wait_until(&dave.client, WAIT, "dave joined", |_| {
        backend.chats("dave").first().is_some_and(|c| c.2)
    });

    alice.engine.send("dm-dave", "across servers", None);
    wait_until(&dave.client, WAIT, "dave reads it", |w| {
        has_text(w, "dm-alice", "across servers")
    });
    dave.engine.send("dm-alice", "and back", None);
    wait_until(&alice.client, WAIT, "alice reads it", |w| {
        has_text(w, "dm-dave", "and back")
    });

    alice.engine.turn_off("dm-dave");
    wait_until(&dave.client, WAIT, "dave's side off", |w| {
        !w.e2ee.is_on("dm-alice")
    });
    wait_until(&alice.client, WAIT, "alice's chat ended", |_| {
        backend.chats("alice").is_empty()
    });
    assert!(backend.plaintext().is_empty(), "{:?}", backend.plaintext());
}

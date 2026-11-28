//! End-to-end encryption (experimental) between two fresh accounts on the
//! server's home network: both devices set up, one starts an encrypted chat,
//! messages go both ways and are read on the other side, the backend refuses
//! plaintext into the conversation, and turning it off reaches both.
//!
//! It needs a backend with `IRCORD_E2EE=experimental` and an associated
//! Ergo, so every account gets the home network that encryption works on:
//! `cargo xtask integration` sets `IRCORD_IT_E2EE` when it starts one so.

use std::time::Duration;

use ircord_client::api::Api;
use ircord_client::commands::run::{Plan, execute};
use ircord_client::contract as c;
use ircord_client::e2ee::vault::VaultKey;
use ircord_client::e2ee::{Engine, Texts};
use ircord_client::store::Workspace;
use ircord_client::store::e2ee::PLACEHOLDER;
use ircord_client::{Client, ServerAddress};

use crate::support::{environment, unique, wait_for_backend};

const WAIT: Duration = Duration::from_secs(60);

fn wait_until(client: &Client, timeout: Duration, what: &str, ok: impl Fn(&Workspace) -> bool) {
    let deadline = std::time::Instant::now() + timeout;
    while !ok(&client.read()) {
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn wait_live(client: &Client) {
    wait_until(client, WAIT, "the socket to go live", |w| {
        w.connection == Some(ircord_client::store::Connection::Live)
    });
}

fn texts() -> Texts {
    Texts {
        control: Box::new(|on, name, mine| format!("{name} on={on} mine={mine}")),
        unreadable: Box::new(|why| format!("unreadable {why:?}")),
    }
}

async fn signup(address: &ServerAddress, username: &str, password: &str) -> String {
    let http = reqwest::Client::new();
    let response = http
        .post(address.join("/api/v1/auth/signup"))
        .header("Origin", address.origin())
        .json(&serde_json::json!({"username": username, "password": password}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 201, "signup");
    response
        .headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find(|v| v.starts_with("ircord_session="))
        .map(|v| v.split(';').next().unwrap().to_string())
        .expect("a session")
}

struct Person {
    client: Client,
    engine: Engine,
    _dir: tempfile::TempDir,
    _updates: futures::channel::mpsc::UnboundedReceiver<ircord_client::events::Update>,
}

fn person(
    rt: &std::sync::Arc<tokio::runtime::Runtime>,
    address: &ServerAddress,
    name: &str,
) -> Person {
    let password = ircord_client::client::random_id();
    let cookie = rt.block_on(signup(address, name, &password));
    let api = Api::new(address.clone());
    api.set_cookie(Some(cookie));
    let (client, updates) = Client::start(rt.clone(), api);
    wait_live(&client);
    wait_until(&client, WAIT, "the home network to carry encryption", |w| {
        w.capabilities.encryption.is_some() && w.networks.iter().any(|n| n.features.encryption)
    });
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::start(
        client.handle().clone(),
        rt.handle().clone(),
        dir.path().to_path_buf(),
        VaultKey::random(),
        format!("live test {name}"),
        texts(),
    );
    wait_until(&client, WAIT, "the device set up", |w| {
        w.e2ee.device.ready()
    });
    Person {
        client,
        engine,
        _dir: dir,
        _updates: updates,
    }
}

fn home(w: &Workspace) -> String {
    w.networks
        .iter()
        .find(|n| n.features.encryption)
        .map(|n| n.id.clone())
        .unwrap()
}

fn direct_with(w: &Workspace, nick: &str) -> Option<String> {
    w.conversations
        .iter()
        .find(|c| c.kind == c::ConversationKind::Direct && c.name.eq_ignore_ascii_case(nick))
        .map(|c| c.id.clone())
}

fn has_text(w: &Workspace, conversation: &str, text: &str) -> bool {
    w.timelines
        .get(conversation)
        .is_some_and(|t| t.entries.iter().any(|e| e.message.text == text))
}

#[test]
fn two_accounts_chat_encrypted_through_the_backend() {
    let Some((server, _)) = environment() else {
        return;
    };
    if std::env::var_os("IRCORD_IT_E2EE").is_none() {
        eprintln!("skipped: the backend runs without encryption");
        return;
    }
    let rt = ircord_client::client::runtime().unwrap();
    let address = ServerAddress::parse(&server).unwrap();
    wait_for_backend(&rt, &address);
    let (a, b) = (unique("ea"), unique("eb"));
    let alice = person(&rt, &address, &a);
    let bob = person(&rt, &address, &b);

    // An ordinary direct conversation first, so both sides have it.
    let network = home(&alice.client.read());
    let done = rt
        .block_on(execute(
            &alice.client,
            Plan::Direct {
                network,
                member: None,
                nickname: Some(b.clone()),
                text: None,
            },
        ))
        .unwrap()
        .expect("open the direct conversation");
    let at_alice = done.open.expect("a conversation");
    alice.client.open(&at_alice);
    alice.client.send(&at_alice, "hello in the clear");
    wait_until(&bob.client, WAIT, "bob's side of the conversation", |w| {
        direct_with(w, &a).is_some()
    });
    let at_bob = direct_with(&bob.client.read(), &a).unwrap();
    bob.client.open(&at_bob);
    // Bob has not spoken yet: the network is asked who he is.
    alice.engine.check_peer(&at_alice);
    wait_until(&alice.client, WAIT, "bob's account known and set up", |w| {
        ircord_client::e2ee::engine::peer_of(w, &at_alice).is_ok()
            && ircord_client::e2ee::engine::blocked(w, &at_alice).is_none()
    });

    alice.engine.start_chat(&at_alice);
    wait_until(&alice.client, WAIT, "alice's side on", |w| {
        w.e2ee.is_on(&at_alice) || w.e2ee.errors.contains_key(&at_alice)
    });
    assert_eq!(alice.client.read().e2ee.errors.get(&at_alice), None);
    wait_until(&bob.client, WAIT, "bob's side on", |w| {
        w.e2ee.is_on(&at_bob)
    });

    alice.engine.send(&at_alice, "only for bob", None);
    wait_until(&bob.client, WAIT, "bob reads it", |w| {
        has_text(w, &at_bob, "only for bob")
    });
    bob.engine.send(&at_bob, "only for alice", None);
    wait_until(&alice.client, WAIT, "alice reads it", |w| {
        has_text(w, &at_alice, "only for alice")
    });

    // What the server keeps is the placeholder and ciphertext.
    let page = rt
        .block_on(bob.client.api().messages(&at_bob, None, 50))
        .unwrap();
    let stored: Vec<_> = page
        .items
        .iter()
        .filter(|m| m.encrypted.is_some())
        .collect();
    assert!(stored.len() >= 3, "control, two messages: {}", stored.len());
    assert!(stored.iter().all(|m| m.text == PLACEHOLDER));
    assert!(!format!("{:?}", page.items).contains("only for"));

    // The backend refuses plaintext into it.
    let plain = c::MessageInputBody {
        client_id: ircord_client::client::random_id(),
        text: "sneaking past".into(),
        ..Default::default()
    };
    let refused = rt.block_on(alice.client.api().send_message(&at_alice, &plain));
    assert!(
        matches!(
            refused,
            Err(ircord_client::api::ApiError::Http { status: 409, .. })
        ),
        "{refused:?}"
    );

    alice.engine.turn_off(&at_alice);
    wait_until(&bob.client, WAIT, "bob sees it off", |w| {
        !w.e2ee.is_on(&at_bob)
    });
    let state = rt
        .block_on(alice.client.api().e2ee_conversation(&at_alice))
        .unwrap();
    assert!(!state.encrypted);
}

//! End-to-end encryption (experimental) between two fresh accounts on the
//! server's home network: both devices set up, one starts an encrypted chat,
//! messages go both ways and are read on the other side, the backend refuses
//! plaintext into the conversation, turning it off reaches both, and it
//! turns on and off again from either side.
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

pub(crate) const WAIT: Duration = Duration::from_secs(60);

pub(crate) fn wait_until(
    client: &Client,
    timeout: Duration,
    what: &str,
    ok: impl Fn(&Workspace) -> bool,
) {
    let deadline = std::time::Instant::now() + timeout;
    while !ok(&client.read()) {
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

pub(crate) fn wait_live(client: &Client) {
    wait_until(client, WAIT, "the socket to go live", |w| {
        w.connection == Some(ircord_client::store::Connection::Live)
    });
}

pub(crate) fn texts() -> Texts {
    Texts {
        control: Box::new(|on, name, mine| format!("{name} on={on} mine={mine}")),
        unreadable: Box::new(|why| format!("unreadable {why:?}")),
        left: Box::new(|name, mine| format!("{name} left mine={mine}")),
    }
}

pub(crate) async fn signup(address: &ServerAddress, username: &str, password: &str) -> String {
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

pub(crate) struct Person {
    pub(crate) client: Client,
    pub(crate) engine: Engine,
    _dir: tempfile::TempDir,
    _updates: futures::channel::mpsc::UnboundedReceiver<ircord_client::events::Update>,
}

pub(crate) fn person(
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

pub(crate) fn home(w: &Workspace) -> String {
    w.networks
        .iter()
        .find(|n| n.features.encryption)
        .map(|n| n.id.clone())
        .unwrap()
}

pub(crate) fn direct_with(w: &Workspace, nick: &str) -> Option<String> {
    w.conversations
        .iter()
        .find(|c| c.kind == c::ConversationKind::Direct && c.name.eq_ignore_ascii_case(nick))
        .map(|c| c.id.clone())
}

pub(crate) fn has_text(w: &Workspace, conversation: &str, text: &str) -> bool {
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

    // On again, from either side, and off from the other: each time a new
    // group that both sides and the server agree on.
    bob.engine.check_peer(&at_bob);
    wait_until(&bob.client, WAIT, "alice's account known to bob", |w| {
        ircord_client::e2ee::engine::peer_of(w, &at_bob).is_ok()
            && ircord_client::e2ee::engine::blocked(w, &at_bob).is_none()
    });
    let sides = [(&alice, &at_alice), (&bob, &at_bob)];
    let encrypted = |p: &Person, c: &str| {
        rt.block_on(p.client.api().e2ee_conversation(c))
            .unwrap()
            .encrypted
    };
    for (round, (starter, ender)) in [(0, 1), (1, 0), (1, 1), (0, 0)].into_iter().enumerate() {
        let (who, at) = sides[starter];
        who.engine.start_chat(at);
        wait_until(&who.client, WAIT, "the starter's side on", |w| {
            w.e2ee.is_on(at) || w.e2ee.errors.contains_key(at)
        });
        assert_eq!(who.client.read().e2ee.errors.get(at), None, "round {round}");
        for (p, c) in sides {
            wait_until(&p.client, WAIT, "both sides on", |w| w.e2ee.is_on(c));
        }
        let text = format!("round {round}");
        sides[1 - starter]
            .0
            .engine
            .send(sides[1 - starter].1, &text, None);
        wait_until(&who.client, WAIT, "the starter reads it", |w| {
            has_text(w, at, &text)
        });
        // Polled outside the workspace's lock: the call may need it.
        let deadline = std::time::Instant::now() + WAIT;
        while !sides.iter().all(|(p, c)| encrypted(p, c)) {
            assert!(
                std::time::Instant::now() < deadline,
                "round {round}: the server to refuse plaintext from both"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
        let (who, at) = sides[ender];
        who.engine.turn_off(at);
        for (p, c) in sides {
            wait_until(&p.client, WAIT, "both sides off", |w| !w.e2ee.is_on(c));
        }
        assert_eq!(who.client.read().e2ee.errors.get(at), None, "round {round}");
        for (p, c) in sides {
            assert!(!encrypted(p, c), "round {round}: still bound");
        }
    }
}

/// Open (or find) the direct conversation with `nick` from `p`'s side,
/// saying something in the clear so the other side has it too.
fn direct(rt: &tokio::runtime::Runtime, p: &Person, nick: &str) -> String {
    let network = home(&p.client.read());
    let done = rt
        .block_on(execute(
            &p.client,
            Plan::Direct {
                network,
                member: None,
                nickname: Some(nick.to_string()),
                text: None,
            },
        ))
        .unwrap()
        .expect("open the direct conversation");
    let id = done.open.expect("a conversation");
    p.client.open(&id);
    p.client.send(&id, "hello in the clear");
    id
}

/// Wait until `p` has a direct conversation with `nick`, and open it.
fn direct_from(p: &Person, nick: &str) -> String {
    wait_until(&p.client, WAIT, "the other side of the conversation", |w| {
        direct_with(w, nick).is_some()
    });
    let id = direct_with(&p.client.read(), nick).unwrap();
    p.client.open(&id);
    id
}

/// Start an encrypted chat in `p`'s conversation `at` once the peer is known.
fn start_encrypted(p: &Person, at: &str) {
    p.engine.check_peer(at);
    wait_until(&p.client, WAIT, "the peer's account and keys known", |w| {
        ircord_client::e2ee::engine::peer_of(w, at).is_ok()
            && ircord_client::e2ee::engine::blocked(w, at).is_none()
    });
    p.engine.start_chat(at);
    wait_until(&p.client, WAIT, "the starter's side on", |w| {
        w.e2ee.is_on(at) || w.e2ee.errors.contains_key(at)
    });
    assert_eq!(p.client.read().e2ee.errors.get(at), None);
}

/// The group's conversation at `p`, once it is there, opened.
fn group_at(p: &Person) -> String {
    wait_until(&p.client, WAIT, "the group", |w| !w.e2ee.groups.is_empty());
    let id = p.client.read().e2ee.groups.keys().next().unwrap().clone();
    p.client.open(&id);
    id
}

/// Two IRCord servers on one IRC network, two people on each, and a plain
/// IRC user: encrypted direct messages across the servers, started from
/// either side; an encrypted group created on server A with members on B,
/// where every member reads every message, the plain user reads nothing and
/// what it writes is marked; a member removed reads nothing sent after; and
/// neither server keeps any plaintext.
#[test]
fn two_servers_one_network_direct_chats_and_a_group() {
    let Some((server, _)) = environment() else {
        return;
    };
    let (Ok(server_b), Some(_)) = (
        std::env::var("IRCORD_IT_SERVER_B"),
        std::env::var_os("IRCORD_IT_E2EE"),
    ) else {
        eprintln!("skipped: needs two backends with encryption (IRCORD_IT_E2EE=1)");
        return;
    };
    let rt = ircord_client::client::runtime().unwrap();
    let a = ServerAddress::parse(&server).unwrap();
    let b = ServerAddress::parse(&server_b).unwrap();
    wait_for_backend(&rt, &a);
    wait_for_backend(&rt, &b);
    let names: Vec<String> = ["ga", "gb", "gc", "gd"].iter().map(|p| unique(p)).collect();
    let alice = person(&rt, &a, &names[0]);
    let bob = person(&rt, &a, &names[1]);
    let carol = person(&rt, &b, &names[2]);
    let dave = person(&rt, &b, &names[3]);
    let (na, nb, nc, nd) = (&names[0], &names[1], &names[2], &names[3]);

    // Across the servers, from A: alice and carol.
    let at_alice = direct(&rt, &alice, nc);
    let at_carol = direct_from(&carol, na);
    start_encrypted(&alice, &at_alice);
    wait_until(&carol.client, WAIT, "carol's side on", |w| {
        w.e2ee.is_on(&at_carol)
    });
    alice.engine.send(&at_alice, "across to B", None);
    wait_until(&carol.client, WAIT, "carol reads it", |w| {
        has_text(w, &at_carol, "across to B")
    });
    carol.engine.send(&at_carol, "back to A", None);
    wait_until(&alice.client, WAIT, "alice reads it", |w| {
        has_text(w, &at_alice, "back to A")
    });
    // Carried over IRC: no group log on either server.
    let chats = rt.block_on(alice.client.api().e2ee_chats()).unwrap();
    assert!(
        chats
            .iter()
            .any(|c| c.kind == c::E2EEChatKind::Direct && c.state == c::E2EEChatState::Joined),
        "{chats:?}"
    );

    // And from B: dave starts one with bob.
    let at_dave = direct(&rt, &dave, nb);
    let at_bob = direct_from(&bob, nd);
    start_encrypted(&dave, &at_dave);
    wait_until(&bob.client, WAIT, "bob's side on", |w| {
        w.e2ee.is_on(&at_bob)
    });
    bob.engine.send(&at_bob, "bob on A", None);
    wait_until(&dave.client, WAIT, "dave reads it", |w| {
        has_text(w, &at_dave, "bob on A")
    });

    // A group made on A, with people on both servers.
    alice
        .engine
        .create_group("Two servers", vec![nb.clone(), nc.clone(), nd.clone()]);
    let g_alice = group_at(&alice);
    let chat = alice.client.read().e2ee.groups[&g_alice].chat.clone();
    for p in [&bob, &carol, &dave] {
        wait_until(&p.client, WAIT, "the invitation", |w| {
            w.e2ee
                .invitations
                .iter()
                .any(|i| i.title == "Two servers" && i.people == 4)
        });
        p.engine.accept_invitation(&chat);
    }
    let g_bob = group_at(&bob);
    let g_carol = group_at(&carol);
    let g_dave = group_at(&dave);

    // A plain IRC user, let in by the group's creator.
    let channel = format!(
        "#{}",
        alice
            .client
            .read()
            .conversation(&g_alice)
            .unwrap()
            .name
            .trim_start_matches('#')
    );
    assert!(
        ircord_client::store::e2ee::is_group_channel(&channel),
        "{channel}"
    );
    assert!(
        ircord_client::store::e2ee::guarded(&alice.client.read(), &g_alice),
        "a group's channel never gets plaintext from here"
    );
    // Nor does the backend take any from its own users.
    let plain = c::MessageInputBody {
        client_id: ircord_client::client::random_id(),
        text: "sneaking past".into(),
        ..Default::default()
    };
    let refused = rt.block_on(alice.client.api().send_message(&g_alice, &plain));
    assert!(
        matches!(
            refused,
            Err(ircord_client::api::ApiError::Http { status: 409, .. })
        ),
        "{refused:?}"
    );
    let (irc, _) = environment().unwrap();
    let plain_nick = unique("plain");
    let mut plain = rt.block_on(async {
        let host = crate::support::irc_host_for_test(&irc);
        let stream = tokio::net::TcpStream::connect(&host).await.unwrap();
        let (read, mut write) = stream.into_split();
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
        write
            .write_all(
                format!("CAP REQ :message-tags\r\nCAP END\r\nNICK {plain_nick}\r\nUSER {plain_nick} 0 * :{plain_nick}\r\n")
                    .as_bytes(),
            )
            .await
            .unwrap();
        let mut lines = tokio::io::BufReader::new(read).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if line.starts_with("PING") {
                let pong = line.replacen("PING", "PONG", 1) + "\r\n";
                write.write_all(pong.as_bytes()).await.unwrap();
            }
            if line.split(' ').nth(1) == Some("001") {
                break;
            }
        }
        (lines, write)
    });
    let network = home(&alice.client.read());
    rt.block_on(execute(
        &alice.client,
        Plan::Raw {
            network,
            line: format!("INVITE {plain_nick} {channel}"),
            then: None,
        },
    ))
    .unwrap()
    .unwrap();
    rt.block_on(async {
        use tokio::io::AsyncWriteExt;
        tokio::time::sleep(Duration::from_secs(1)).await;
        plain
            .1
            .write_all(format!("JOIN {channel}\r\n").as_bytes())
            .await
            .unwrap();
    });

    alice.engine.send(&g_alice, "hello both servers", None);
    for (p, g) in [(&bob, &g_bob), (&carol, &g_carol), (&dave, &g_dave)] {
        wait_until(&p.client, WAIT, "the group message", |w| {
            has_text(w, g, "hello both servers")
        });
    }
    dave.engine.send(&g_dave, "dave on B", None);
    for (p, g) in [(&alice, &g_alice), (&bob, &g_bob), (&carol, &g_carol)] {
        wait_until(&p.client, WAIT, "dave's message", |w| {
            has_text(w, g, "dave on B")
        });
    }

    // The plain user read placeholders only; what it says is marked.
    let seen = rt.block_on(async {
        use tokio::io::AsyncWriteExt;
        plain
            .1
            .write_all(format!("PRIVMSG {channel} :plain words\r\n").as_bytes())
            .await
            .unwrap();
        let mut seen = Vec::new();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
        while let Ok(Ok(Some(line))) = tokio::time::timeout_at(deadline, plain.0.next_line()).await
        {
            seen.push(line);
        }
        seen
    });
    let all = seen.join("\n");
    assert!(
        !all.contains("hello both servers") && !all.contains("dave on B"),
        "{all}"
    );
    assert!(
        all.contains("JOIN"),
        "the plain user is in the channel: {all}"
    );
    wait_until(&bob.client, WAIT, "the plain line marked", |w| {
        w.e2ee.conversation(&g_bob).is_some_and(|c| c.unencrypted)
            && has_text(w, &g_bob, "plain words")
    });

    // Dave (on B) is removed: he reads nothing sent after.
    alice.engine.remove_person(&g_alice, nd);
    wait_until(&dave.client, WAIT, "dave out of the group", |w| {
        w.e2ee.groups.is_empty()
    });
    alice.engine.send(&g_alice, "without dave", None);
    for (p, g) in [(&bob, &g_bob), (&carol, &g_carol)] {
        wait_until(&p.client, WAIT, "the message after", |w| {
            has_text(w, g, "without dave")
        });
    }
    std::thread::sleep(Duration::from_secs(2));
    assert!(!has_text(&dave.client.read(), &g_dave, "without dave"));

    // Neither server keeps any of it in the clear.
    for (p, convs) in [
        (&alice, vec![&at_alice, &g_alice]),
        (&bob, vec![&at_bob, &g_bob]),
        (&carol, vec![&at_carol, &g_carol]),
        (&dave, vec![&at_dave]),
    ] {
        for conv in convs {
            let page = rt
                .block_on(p.client.api().messages(conv, None, 100))
                .unwrap();
            let dump = format!("{:?}", page.items);
            for secret in [
                "across to B",
                "back to A",
                "bob on A",
                "hello both servers",
                "dave on B",
                "without dave",
            ] {
                assert!(!dump.contains(secret), "{secret} stored in the clear");
            }
        }
    }
}

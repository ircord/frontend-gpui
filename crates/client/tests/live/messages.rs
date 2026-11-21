//! Live cases for package A1: edit, react and (where the network allows it)
//! delete through a real backend.
//!
//! Ergo lets only a logged-in account edit or delete its messages, so the
//! case first registers a NickServ account for its nick and adds the network
//! with SASL.

use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::StreamExt;
use futures::channel::mpsc::UnboundedReceiver;
use ircord_client::api::Api;
use ircord_client::client::{random_id, runtime, sign_in};
use ircord_client::credentials::{CredentialStore, MemoryStore};
use ircord_client::events::Update;
use ircord_client::features::{Context, MessageAction, Reason};
use ircord_client::store::{Connection, Workspace};
use ircord_client::{Client, ServerAddress};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::support::{environment, irc_host_for_test, unique, wait_for_backend};

const CHANNEL: &str = "#gpui-a1";

/// Register `nick` with NickServ on the IRC server, then disconnect.
async fn register_nickserv(irc: &str, nick: &str, password: &str) {
    let stream = tokio::net::TcpStream::connect(irc)
        .await
        .expect("reach the IRC server");
    let (read, mut write) = stream.into_split();
    write
        .write_all(format!("NICK {nick}\r\nUSER {nick} 0 * :{nick}\r\n").as_bytes())
        .await
        .unwrap();
    let mut lines = BufReader::new(read).lines();
    let registered = tokio::time::timeout(Duration::from_secs(20), async {
        while let Ok(Some(line)) = lines.next_line().await {
            if line.starts_with("PING") {
                let pong = line.replacen("PING", "PONG", 1) + "\r\n";
                write.write_all(pong.as_bytes()).await.unwrap();
            }
            if line.split(' ').nth(1) == Some("001") {
                write
                    .write_all(format!("PRIVMSG NickServ :REGISTER {password}\r\n").as_bytes())
                    .await
                    .unwrap();
            }
            if line.contains("Account created") {
                return true;
            }
        }
        false
    })
    .await
    .unwrap_or(false);
    assert!(registered, "NickServ did not register {nick}");
    let _ = write.write_all(b"QUIT :done\r\n").await;
    // Wait for the server to close the connection, so the nick is free again.
    let _ = tokio::time::timeout(Duration::from_secs(10), async {
        while let Ok(Some(_)) = lines.next_line().await {}
    })
    .await;
}

/// Sign up `username` and add the IRC network, logging in with SASL as the
/// NickServ account of the same name.
async fn setup_account_with_sasl(
    address: &ServerAddress,
    username: &str,
    password: &str,
    irc: &str,
    sasl_password: &str,
) {
    let http = reqwest::Client::new();
    let origin = address.origin();
    let signup = http
        .post(address.join("/api/v1/auth/signup"))
        .header("Origin", &origin)
        .json(&serde_json::json!({"username": username, "password": password}))
        .send()
        .await
        .unwrap();
    assert_eq!(signup.status(), 201, "signup");
    let cookie = signup
        .headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find(|v| v.starts_with("ircord_session="))
        .map(|v| v.split(';').next().unwrap().to_string())
        .expect("signup sets a session");
    let (host, port) = irc.rsplit_once(':').unwrap();
    let network = http
        .post(address.join("/api/v1/networks"))
        .header("Origin", &origin)
        .header("Cookie", &cookie)
        .json(&serde_json::json!({
            "name": "Test", "host": host, "port": port.parse::<u16>().unwrap(), "tls": false,
            "nick": username, "autoJoin": [CHANNEL],
            "auth": {"method": "sasl-plain", "account": username, "password": sasl_password}
        }))
        .send()
        .await
        .unwrap();
    let status = network.status();
    assert!(
        status.is_success(),
        "add network: {status} {}",
        network.text().await.unwrap_or_default()
    );
}

/// Wait up to `seconds` for `done`, letting updates arrive.
fn until(
    rt: &tokio::runtime::Runtime,
    client: &Client,
    updates: &mut UnboundedReceiver<Update>,
    seconds: u64,
    what: &str,
    done: impl Fn(&Workspace) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(seconds);
    while !done(&client.read()) {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        let next = rt.block_on(async {
            tokio::time::timeout(Duration::from_millis(300), updates.next())
                .await
                .ok()
                .flatten()
        });
        if let Some(Update::SignedOut) = next {
            panic!("signed out while waiting for {what}");
        }
    }
}

#[test]
fn edits_reacts_and_deletes_through_a_real_backend() {
    let Some((server, irc)) = environment() else {
        return;
    };
    let rt = runtime().unwrap();
    let address = ServerAddress::parse(&server).unwrap();
    wait_for_backend(&rt, &address);
    let username = unique("a1");
    let password = random_id();
    let sasl_password = random_id()[..20].to_string();

    rt.block_on(register_nickserv(
        &irc_host_for_test(&irc),
        &username,
        &sasl_password,
    ));
    rt.block_on(setup_account_with_sasl(
        &address,
        &username,
        &password,
        &irc,
        &sasl_password,
    ));

    let api = Api::new(address.clone());
    let store: Arc<dyn CredentialStore> = Arc::new(MemoryStore::default());
    rt.block_on(sign_in(&api, store.as_ref(), &username, &password))
        .expect("sign in");
    let (client, mut updates) = Client::start(rt.clone(), api);

    until(&rt, &client, &mut updates, 30, "the live socket", |w| {
        w.connection == Some(Connection::Live)
    });
    let channel = CHANNEL.trim_start_matches('#');
    until(&rt, &client, &mut updates, 45, "the joined channel", |w| {
        w.conversations
            .iter()
            .any(|c| c.name.eq_ignore_ascii_case(channel) && c.joined)
    });
    let conversation = client
        .read()
        .conversations
        .iter()
        .find(|c| c.name.eq_ignore_ascii_case(channel))
        .map(|c| c.id.clone())
        .unwrap();
    // SASL logged in and the network offers editing.
    until(
        &rt,
        &client,
        &mut updates,
        30,
        "the account and edit",
        |w| {
            w.networks
                .first()
                .is_some_and(|n| n.account.is_some() && n.features.edit && n.features.reactions)
        },
    );
    client.open(&conversation);
    until(&rt, &client, &mut updates, 15, "history", |w| {
        w.timelines.get(&conversation).is_some_and(|t| t.loaded)
    });

    // Send one, and wait for it to come back with its network ID.
    let text = format!("before {}", &random_id()[..6]);
    client.send(&conversation, &text);
    let own = |w: &Workspace| {
        let network = w.conversations[0].network_id.clone();
        w.timelines.get(&conversation).and_then(|t| {
            t.entries
                .iter()
                .map(|e| &e.message)
                .find(|m| {
                    w.is_self(&m.sender_id, &network) && m.text == text && m.upstream_id.is_some()
                })
                .map(|m| m.id.clone())
        })
    };
    until(&rt, &client, &mut updates, 20, "the sent message", |w| {
        own(w).is_some()
    });
    let id = own(&client.read()).unwrap();
    let redact;
    {
        // The client's own rules agree the message can be edited and reacted to.
        let w = client.read();
        let m = w.message(&conversation, &id).unwrap();
        let allowed = Context::of(&w, &conversation, true);
        assert_eq!(allowed.action_reason(m, MessageAction::Edit), None);
        assert_eq!(allowed.action_reason(m, MessageAction::Reactions), None);
        redact = allowed.action_reason(m, MessageAction::Redact);
        // The test server's Ergo does not enable message redaction.
        assert!(
            matches!(redact, None | Some(Reason::RedactUnsupported)),
            "{redact:?}"
        );
    }

    // Edit.
    let edited = format!("after {}", &random_id()[..6]);
    let answer = rt
        .block_on(client.api().edit_message(&conversation, &id, &edited))
        .expect("edit");
    client.change(|w| w.message_answered(answer.message));
    until(&rt, &client, &mut updates, 20, "the edit", |w| {
        w.message(&conversation, &id)
            .is_some_and(|m| m.text == edited && m.edited_at.is_some())
    });

    // React, then take it back.
    let answer = rt
        .block_on(client.api().toggle_reaction(&conversation, &id, "👍"))
        .expect("react");
    assert!(answer.reactions.iter().any(|r| r.emoji == "👍"));
    client.change(|w| w.message_answered(answer));
    until(&rt, &client, &mut updates, 20, "the reaction", |w| {
        w.message(&conversation, &id)
            .is_some_and(|m| m.reactions.iter().any(|r| r.emoji == "👍"))
    });
    let answer = rt
        .block_on(client.api().toggle_reaction(&conversation, &id, "👍"))
        .expect("take the reaction back");
    client.change(|w| w.message_answered(answer));
    until(&rt, &client, &mut updates, 20, "the reaction gone", |w| {
        w.message(&conversation, &id).is_some_and(|m| {
            m.reactions
                .iter()
                .all(|r| r.emoji != "👍" || r.member_ids.is_empty())
        })
    });

    // Delete, where the network allows it: the message leaves the timeline.
    if redact.is_none() {
        rt.block_on(client.api().delete_message(&conversation, &id, None, false))
            .expect("delete");
        until(&rt, &client, &mut updates, 20, "the deletion", |w| {
            w.message(&conversation, &id).is_none()
        });
    } else {
        eprintln!("skipped deleting: the network does not support redaction");
    }

    // Nothing is left in the outbox.
    let outbox = rt.block_on(client.api().outbox_all()).expect("outbox");
    assert!(
        outbox.iter().all(|e| e.conversation_id != conversation),
        "{outbox:?}"
    );
    rt.block_on(client.sign_out(store)).unwrap();
    eprintln!("live A1 case passed for {username}");
}

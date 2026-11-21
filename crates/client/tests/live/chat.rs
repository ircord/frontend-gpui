//! The first live case: it signs up a fresh account, signs in through the
//! client, bootstraps, joins a channel, receives a message a plain IRC user
//! sends there as a live event, sends one back and checks the IRC user
//! receives it.

use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::StreamExt;
use ircord_client::api::Api;
use ircord_client::client::{runtime, sign_in};
use ircord_client::credentials::{CredentialStore, MemoryStore};
use ircord_client::events::Update;
use ircord_client::store::Connection;
use ircord_client::{Client, ServerAddress};
use tokio::io::AsyncWriteExt;

use crate::support::{
    environment, irc_host_for_test, irc_user, setup_account, unique, wait_for_backend,
};

const CHANNEL: &str = "#gpui-it";

#[test]
fn signs_in_bootstraps_receives_live_and_sends() {
    let Some((server, irc)) = environment() else {
        return;
    };
    let rt = runtime().unwrap();
    let address = ServerAddress::parse(&server).unwrap();
    let username = unique("gpui");
    let password = ircord_client::client::random_id();

    wait_for_backend(&rt, &address);

    // A fresh account and an IRC network for it, set up over plain HTTP.
    let cookie = rt.block_on(setup_account(&address, &username, &password, &irc, CHANNEL));
    drop(cookie);

    // Sign in through the client and keep the session in a credential store.
    let api = Api::new(address.clone());
    rt.block_on(api.verify_server())
        .expect("the address is an IRCord server");
    let store: Arc<dyn CredentialStore> = Arc::new(MemoryStore::default());
    rt.block_on(sign_in(&api, store.as_ref(), &username, &password))
        .expect("sign in");
    let saved = store
        .load(&address)
        .unwrap()
        .expect("the session was stored");
    assert!(saved.starts_with("ircord_session="));

    let (client, mut updates) = Client::start(rt.clone(), api);
    let next = |updates: &mut futures::channel::mpsc::UnboundedReceiver<Update>, wait: Duration| {
        rt.block_on(async {
            tokio::time::timeout(wait, updates.next())
                .await
                .ok()
                .flatten()
        })
    };

    // Bootstrap and go live.
    let deadline = Instant::now() + Duration::from_secs(30);
    while client.read().connection != Some(Connection::Live) {
        assert!(
            Instant::now() < deadline,
            "never went live: {:?}",
            client.read().connection
        );
        if let Some(Update::SignedOut) = next(&mut updates, Duration::from_millis(500)) {
            panic!("signed out while connecting");
        }
    }
    assert_eq!(
        client.read().networks.len(),
        1,
        "the bootstrap has the network"
    );

    // The backend joins the channel on the IRC network.
    let deadline = Instant::now() + Duration::from_secs(45);
    let conversation = loop {
        let found = client
            .read()
            .conversations
            .iter()
            .find(|c| c.name.eq_ignore_ascii_case(CHANNEL.trim_start_matches('#')) && c.joined)
            .map(|c| c.id.clone());
        if let Some(id) = found {
            break id;
        }
        assert!(Instant::now() < deadline, "the channel never appeared");
        next(&mut updates, Duration::from_millis(500));
    };
    client.open(&conversation);
    let deadline = Instant::now() + Duration::from_secs(15);
    while !client
        .read()
        .timelines
        .get(&conversation)
        .is_some_and(|t| t.loaded)
    {
        assert!(Instant::now() < deadline, "history never loaded");
        next(&mut updates, Duration::from_millis(300));
    }

    // A plain IRC user joins and speaks; the client gets it as a live event.
    let (irc_lines, irc_write) =
        rt.block_on(irc_user(&irc_host_for_test(&irc), "ircuser", CHANNEL));
    let irc_write = Arc::new(tokio::sync::Mutex::new(irc_write));
    let greeting = format!(
        "hello from irc {}",
        &ircord_client::client::random_id()[..6]
    );
    {
        let irc_write = irc_write.clone();
        let line = format!("PRIVMSG {CHANNEL} :{greeting}\r\n");
        rt.block_on(async move {
            irc_write
                .lock()
                .await
                .write_all(line.as_bytes())
                .await
                .unwrap()
        });
    }
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut live_event = false;
    loop {
        let seen = client
            .read()
            .timelines
            .get(&conversation)
            .is_some_and(|t| t.entries.iter().any(|e| e.message.text == greeting));
        if seen && live_event {
            break;
        }
        assert!(Instant::now() < deadline, "the IRC message never arrived");
        if let Some(Update::Changed(changes)) = next(&mut updates, Duration::from_millis(500))
            && changes.timelines.contains(&conversation)
        {
            live_event = true;
        }
    }

    // Send back; the IRC user receives it and the send settles.
    let reply = format!(
        "hello from gpui {}",
        &ircord_client::client::random_id()[..6]
    );
    client.send(&conversation, &reply);
    assert_eq!(client.read().pending_in(&conversation).len(), 1);
    let heard = rt.block_on(async {
        tokio::time::timeout(Duration::from_secs(20), async {
            let mut lines = irc_lines;
            while let Ok(Some(line)) = lines.next_line().await {
                if line.starts_with("PING") {
                    let pong = line.replacen("PING", "PONG", 1) + "\r\n";
                    irc_write
                        .lock()
                        .await
                        .write_all(pong.as_bytes())
                        .await
                        .unwrap();
                }
                if line.contains("PRIVMSG") && line.ends_with(&reply) {
                    return true;
                }
            }
            false
        })
        .await
        .unwrap_or(false)
    });
    assert!(heard, "the IRC user never received the message");
    let deadline = Instant::now() + Duration::from_secs(15);
    while !client.read().pending_in(&conversation).is_empty() {
        assert!(Instant::now() < deadline, "the send never settled");
        next(&mut updates, Duration::from_millis(300));
    }
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let w = client.read();
        let found = w.timelines[&conversation].entries.iter().any(|e| {
            e.message.text == reply
                && w.is_self(&e.message.sender_id, &w.conversations[0].network_id)
        });
        drop(w);
        if found {
            break;
        }
        if Instant::now() > deadline {
            let w = client.read();
            let tail: Vec<_> = w.timelines[&conversation]
                .entries
                .iter()
                .rev()
                .take(3)
                .map(|e| &e.message)
                .collect();
            panic!("the sent message is not in the timeline: {tail:#?}");
        }
        next(&mut updates, Duration::from_millis(300));
    }

    // Mark read and sign out.
    client.mark_read(&conversation);
    rt.block_on(client.sign_out(store.clone())).unwrap();
    assert_eq!(store.load(&address).unwrap(), None);
    eprintln!("live test passed for {username}");
}

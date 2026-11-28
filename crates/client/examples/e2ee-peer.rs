//! The other person in `CAPTURE_ONLY=e2ee cargo xtask capture`: a headless
//! client with its own encryption engine, so the app has someone to start an
//! encrypted chat with.
//!
//! `e2ee-peer <server> <username> <password> <nick to talk to> <folder>`
//! signs up (or in), waits until encryption is set up, says hello in the
//! clear, and answers each encrypted message it reads. It writes `ready`
//! into `<folder>` once set up and `on` once the chat is encrypted, and runs
//! until it is stopped.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::time::Duration;

use ircord_client::api::Api;
use ircord_client::commands::run::{Plan, execute};
use ircord_client::contract as c;
use ircord_client::e2ee::vault::VaultKey;
use ircord_client::e2ee::{Engine, Texts};
use ircord_client::store::Workspace;
use ircord_client::{Client, ServerAddress};

fn wait(client: &Client, what: &str, ok: impl Fn(&Workspace) -> bool) {
    let deadline = std::time::Instant::now() + Duration::from_secs(120);
    while !ok(&client.read()) {
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let [_, server, username, password, nick, folder] = args.as_slice() else {
        eprintln!("usage: e2ee-peer <server> <username> <password> <nick> <folder>");
        std::process::exit(2);
    };
    let folder = PathBuf::from(folder);
    std::fs::create_dir_all(&folder).unwrap();
    let rt = ircord_client::client::runtime().unwrap();
    let address = ServerAddress::parse(server).unwrap();
    let api = Api::new(address.clone());
    let signed = rt.block_on(async {
        let http = reqwest::Client::new();
        let _ = http
            .post(address.join("/api/v1/auth/signup"))
            .header("Origin", address.origin())
            .json(&serde_json::json!({"username": username, "password": password}))
            .send()
            .await;
        api.login(username, password).await
    });
    api.set_cookie(Some(signed.expect("sign in")));
    let (client, _updates) = Client::start(rt.clone(), api);
    wait(&client, "the home network to carry encryption", |w| {
        w.networks.iter().any(|n| n.features.encryption)
    });
    let engine = Engine::start(
        client.handle().clone(),
        rt.handle().clone(),
        folder.join("vault"),
        VaultKey::random(),
        "Bob's laptop".into(),
        Texts {
            invited: Box::new(|| "invited".to_string()),
            joined: Box::new(|account| format!("{account} joined")),
            control: Box::new(|on, name, _| {
                format!("{name} turned encryption {}", if on { "on" } else { "off" })
            }),
            unreadable: Box::new(|why| format!("{why:?}")),
            left: Box::new(|name, _| format!("{name} left")),
        },
    );
    wait(&client, "encryption set up", |w| w.e2ee.device.ready());
    std::fs::write(folder.join("ready"), b"").unwrap();
    println!("e2ee-peer: {username} is set up");

    let network = client
        .read()
        .networks
        .iter()
        .find(|n| n.features.encryption)
        .map(|n| n.id.clone())
        .unwrap();
    let done = rt
        .block_on(execute(
            &client,
            Plan::Direct {
                network,
                member: None,
                nickname: Some(nick.clone()),
                text: None,
            },
        ))
        .unwrap()
        .expect("open the direct conversation");
    let conversation = done.open.expect("a conversation");
    client.open(&conversation);
    client.send(
        &conversation,
        "Hi! Could we switch this chat to end-to-end encryption?",
    );

    let mut answered = BTreeSet::new();
    let mut said_on = false;
    loop {
        std::thread::sleep(Duration::from_millis(300));
        let (on, fresh) = {
            let w = client.read();
            let on = w.e2ee.is_on(&conversation);
            let network = w
                .conversation(&conversation)
                .map(|c| c.network_id.clone())
                .unwrap_or_default();
            let fresh: Vec<(String, String)> = w
                .timelines
                .get(&conversation)
                .map(|t| {
                    t.entries
                        .iter()
                        .map(|e| &e.message)
                        .filter(|m| {
                            m.encrypted.is_some()
                                && m.kind == c::MessageKind::Message
                                && !w.is_self(&m.sender_id, &network)
                                && !answered.contains(&m.id)
                        })
                        .map(|m| (m.id.clone(), m.text.clone()))
                        .collect()
                })
                .unwrap_or_default();
            (on, fresh)
        };
        if on && !said_on {
            said_on = true;
            std::fs::write(folder.join("on"), b"").unwrap();
            std::thread::sleep(Duration::from_secs(1));
            engine.send(
                &conversation,
                "Thanks! The lock is closed on my side too.",
                None,
            );
        }
        for (id, text) in fresh {
            answered.insert(id.clone());
            if text.starts_with('[') {
                continue;
            }
            engine.send(&conversation, &format!("Got it: “{text}”"), Some(id));
        }
    }
}

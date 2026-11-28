//! A member of an encrypted group in `CAPTURE_ONLY=e2ee-group cargo xtask
//! capture`: a headless client with its own encryption engine.
//!
//! `e2ee-group <server> <username> <password> <folder> create <title> <account>…`
//! signs up (or in), waits until encryption is set up and writes `ready`,
//! then waits for `go` in `<folder>` and starts the group `<title>` with
//! those accounts, writing `created`.
//! `e2ee-group <server> <username> <password> <folder> join` joins every
//! group it is invited to, writing `joined`.
//! Either says its line (`<folder>/say`'s text) in the group once that file
//! appears, writing `said`, and runs until it is stopped.

use std::path::PathBuf;
use std::time::Duration;

use ircord_client::api::Api;
use ircord_client::e2ee::vault::VaultKey;
use ircord_client::e2ee::{Engine, Texts};
use ircord_client::store::Workspace;
use ircord_client::{Client, ServerAddress};

fn wait(client: &Client, what: &str, ok: impl Fn(&Workspace) -> bool) {
    let deadline = std::time::Instant::now() + Duration::from_secs(180);
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
    let [_, server, username, password, folder, mode, rest @ ..] = args.as_slice() else {
        eprintln!(
            "usage: e2ee-group <server> <username> <password> <folder> create <title> <account>… | join"
        );
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
        format!("{username}'s laptop"),
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
    println!("e2ee-group: {username} is set up");

    match mode.as_str() {
        "create" => {
            let [title, accounts @ ..] = rest else {
                eprintln!("create needs a title and accounts");
                std::process::exit(2);
            };
            while !folder.join("go").exists() {
                std::thread::sleep(Duration::from_millis(200));
            }
            engine.create_group(title, accounts.to_vec());
            wait(&client, "the group", |w| !w.e2ee.groups.is_empty());
            std::fs::write(folder.join("created"), b"").unwrap();
            println!("e2ee-group: {username} created {title}");
        }
        _ => {
            wait(&client, "an invitation", |w| !w.e2ee.invitations.is_empty());
            let chat = client.read().e2ee.invitations[0].chat.clone();
            engine.accept_invitation(&chat);
            wait(&client, "the group", |w| !w.e2ee.groups.is_empty());
            std::fs::write(folder.join("joined"), b"").unwrap();
            println!("e2ee-group: {username} joined");
        }
    }
    let group = client.read().e2ee.groups.keys().next().unwrap().clone();
    client.open(&group);
    loop {
        std::thread::sleep(Duration::from_millis(300));
        let say = folder.join("say");
        if let Ok(text) = std::fs::read_to_string(&say) {
            let _ = std::fs::remove_file(&say);
            engine.send(&group, text.trim(), None);
            std::fs::write(folder.join("said"), b"").unwrap();
        }
    }
}

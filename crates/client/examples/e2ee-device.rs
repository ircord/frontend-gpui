//! One of an account's devices, headless, for checking device approval
//! against a real backend and another client (the web client, or the app in
//! `CAPTURE_ONLY=e2ee cargo xtask capture`).
//!
//! `e2ee-device <server> <username> <password> <folder> wait` signs in (or
//! up) and, when another device set up encryption first, waits for
//! approval: it writes the approval code it shows into `<folder>/code` and
//! `approved` once approved.
//!
//! `e2ee-device <server> <username> <password> <folder> approve` sets up
//! encryption (this being the account's first device), writes `ready`, then
//! approves the code written into `<folder>/approve` and writes `approved`,
//! or `failed` with the reason.
//!
//! Either way it runs until it is stopped.

use std::path::PathBuf;
use std::time::Duration;

use ircord_client::api::Api;
use ircord_client::e2ee::vault::VaultKey;
use ircord_client::e2ee::{Engine, Texts};
use ircord_client::store::Workspace;
use ircord_client::store::e2ee::{Approving, DeviceState};
use ircord_client::{Client, ServerAddress};

fn wait(client: &Client, engine: &Engine, what: &str, ok: impl Fn(&Workspace) -> bool) {
    let deadline = std::time::Instant::now() + Duration::from_secs(600);
    while !ok(&client.read()) {
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        engine.poke();
        std::thread::sleep(Duration::from_millis(200));
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let [_, server, username, password, folder, mode] = args.as_slice() else {
        eprintln!("usage: e2ee-device <server> <username> <password> <folder> wait|approve");
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
    let engine = Engine::start(
        client.handle().clone(),
        rt.handle().clone(),
        folder.join("vault"),
        VaultKey::random(),
        "IRCord GPUI (headless)".into(),
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
    match mode.as_str() {
        "wait" => {
            wait(&client, &engine, "the approval code", |w| {
                matches!(
                    w.e2ee.device,
                    DeviceState::Waiting { .. } | DeviceState::Ready { .. }
                )
            });
            if let DeviceState::Waiting { code, .. } = client.read().e2ee.device.clone() {
                std::fs::write(folder.join("code"), &code).unwrap();
                println!("e2ee-device: {username} waits for approval with {code}");
            }
            wait(&client, &engine, "the approval", |w| w.e2ee.device.ready());
            std::fs::write(folder.join("approved"), b"").unwrap();
            println!("e2ee-device: {username} is approved");
        }
        "approve" => {
            wait(&client, &engine, "encryption set up", |w| {
                w.e2ee.device.ready()
            });
            std::fs::write(folder.join("ready"), b"").unwrap();
            println!("e2ee-device: {username} is set up");
            let file = folder.join("approve");
            while !file.exists() {
                engine.poke();
                std::thread::sleep(Duration::from_millis(200));
            }
            engine.approve(&std::fs::read_to_string(&file).unwrap());
            wait(&client, &engine, "the approval", |w| {
                matches!(w.e2ee.approving, Approving::Done | Approving::Failed(_))
            });
            match client.read().e2ee.approving.clone() {
                Approving::Done => {
                    std::fs::write(folder.join("approved"), b"").unwrap();
                    println!("e2ee-device: approved the other device");
                }
                other => {
                    std::fs::write(folder.join("failed"), format!("{other:?}")).unwrap();
                    println!("e2ee-device: could not approve: {other:?}");
                }
            }
        }
        _ => {
            eprintln!("the mode is wait or approve");
            std::process::exit(2);
        }
    }
    loop {
        engine.poke();
        std::thread::sleep(Duration::from_secs(1));
    }
}

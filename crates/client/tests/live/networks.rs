//! Live cases for package NW1: add, edit, remove a network.
//!
//! Start each case with `let Some((server, irc)) = crate::support::environment() else { return };`.

use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::StreamExt;
use ircord_client::api::Api;
use ircord_client::api::networks::{self as nets, Form};
use ircord_client::client::{random_id, runtime, sign_in};
use ircord_client::contract::{NetworkStatus, SelfUpdate};
use ircord_client::credentials::{CredentialStore, MemoryStore};
use ircord_client::store::Connection;
use ircord_client::{Client, ServerAddress};

use crate::support::{environment, setup_account, unique, wait_for_backend};

#[test]
fn a_network_is_checked_added_edited_disconnected_and_removed() {
    let Some((server, irc)) = environment() else {
        return;
    };
    let rt = runtime().unwrap();
    let address = ServerAddress::parse(&server).unwrap();
    let username = unique("gpui");
    let password = random_id();
    wait_for_backend(&rt, &address);
    rt.block_on(setup_account(
        &address,
        &username,
        &password,
        &irc,
        &format!("#{}", unique("nw")),
    ));
    let api = Api::new(address.clone());
    let store: Arc<dyn CredentialStore> = Arc::new(MemoryStore::default());
    rt.block_on(sign_in(&api, store.as_ref(), &username, &password))
        .expect("sign in");
    let (client, mut updates) = Client::start(rt.clone(), api.clone());
    let mut pump = |what: &str, done: &dyn Fn(&Client) -> bool| {
        let deadline = Instant::now() + Duration::from_secs(45);
        while !done(&client) {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            rt.block_on(async {
                let _ = tokio::time::timeout(Duration::from_millis(300), updates.next()).await;
            });
        }
    };
    pump("live", &|c| c.read().connection == Some(Connection::Live));

    // The presets answer, with the server's own or none.
    let page = rt.block_on(api.network_presets()).expect("presets");
    let _ = nets::presets(Some(page));

    // The address check, as the form runs it.
    let (host, port) = irc.rsplit_once(':').unwrap();
    let mut form = Form::new(&unique("nw"));
    form.name = "Second".into();
    form.host = host.into();
    form.port = port.into();
    form.tls = false;
    form.auto_join = unique("nwj");
    assert!(
        !form.problems(None, &[]).any(),
        "{:?}",
        form.problems(None, &[])
    );
    let (h, p, tls) = form.endpoint().expect("a valid address");
    let check = rt.block_on(api.check_network(&h, p, tls)).expect("check");
    assert!(
        check.reachable,
        "the IRC server is reachable: {}",
        check.detail
    );
    let unreachable = rt
        .block_on(api.check_network("127.0.0.1", 1, false))
        .expect("check");
    assert!(!unreachable.reachable);
    assert!(!unreachable.detail.is_empty(), "says why");

    // Add, and the workspace hears of it.
    let added = rt.block_on(api.add_network(&form.config())).expect("add");
    let id = added.id.clone();
    client.change(|w| nets::saved(w, added));
    pump("the new network to connect", &|c| {
        c.read()
            .network(&id)
            .is_some_and(|n| n.status == NetworkStatus::Connected)
    });

    // Change the nick while connected.
    let nick = unique("nwn");
    rt.block_on(api.update_self(
        &id,
        &SelfUpdate {
            nick: Some(nick.clone()),
            ..Default::default()
        },
    ))
    .expect("change nick");
    pump("the new nick", &|c| {
        c.read()
            .network(&id)
            .is_some_and(|n| n.nick.eq_ignore_ascii_case(&nick))
    });

    // Edit: rename, and the saved address needs no check.
    let existing = client.read().network(&id).cloned().unwrap();
    let mut edit = Form::of(&existing);
    assert!(!edit.needs_check(Some(&existing)));
    edit.name = "Renamed".into();
    let saved = rt
        .block_on(api.update_network(&id, &edit.update()))
        .expect("edit");
    assert_eq!(saved.name, "Renamed");
    pump("the rename", &|c| {
        c.read().network(&id).is_some_and(|n| n.name == "Renamed")
    });

    // Disconnect.
    let down = rt
        .block_on(api.connect_network(&id, false))
        .expect("disconnect");
    client.change(|w| nets::saved(w, down));
    pump("the disconnect", &|c| {
        c.read()
            .network(&id)
            .is_some_and(|n| n.status == NetworkStatus::Disconnected)
    });

    // Remove.
    rt.block_on(api.remove_network(&id)).expect("remove");
    client.change(|w| nets::removed(w, &id));
    pump("the network to go", &|c| c.read().network(&id).is_none());
    let names: Vec<String> = client
        .read()
        .networks
        .iter()
        .map(|n| n.name.clone())
        .collect();
    assert_eq!(names, ["Test"], "the first network stays");
}

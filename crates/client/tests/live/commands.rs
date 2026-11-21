//! Live cases for package C1: /join, /topic and /nick, planned and run as
//! the composer runs them, against a real backend and IRC server.
//!
//! Start each case with `let Some((server, irc)) = crate::support::environment() else { return };`.

use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::StreamExt;
use ircord_client::api::Api;
use ircord_client::client::{random_id, runtime, sign_in};
use ircord_client::commands::{self, Plan};
use ircord_client::credentials::{CredentialStore, MemoryStore};
use ircord_client::store::Connection;
use ircord_client::{Client, ServerAddress};

use crate::support::{environment, setup_account, unique, wait_for_backend};

#[test]
fn join_topic_and_nick_reach_the_network() {
    let Some((server, irc)) = environment() else {
        return;
    };
    let home = format!("#{}", unique("cmd"));
    let rt = runtime().unwrap();
    let address = ServerAddress::parse(&server).unwrap();
    let username = unique("gpui");
    let password = random_id();
    wait_for_backend(&rt, &address);
    rt.block_on(setup_account(&address, &username, &password, &irc, &home));

    let api = Api::new(address.clone());
    let store: Arc<dyn CredentialStore> = Arc::new(MemoryStore::default());
    rt.block_on(sign_in(&api, store.as_ref(), &username, &password))
        .expect("sign in");
    let (client, mut updates) = Client::start(rt.clone(), api);
    let mut pump = |what: &str, done: &dyn Fn(&Client) -> bool| {
        let deadline = Instant::now() + Duration::from_secs(45);
        while !done(&client) {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            rt.block_on(async {
                let _ = tokio::time::timeout(Duration::from_millis(300), updates.next()).await;
            });
        }
    };
    let joined = |name: String| {
        move |c: &Client| {
            c.read()
                .conversations
                .iter()
                .any(|x| x.name.eq_ignore_ascii_case(&name) && x.joined)
        }
    };
    pump("live", &|c| c.read().connection == Some(Connection::Live));
    pump("the home channel", &joined(home[1..].to_string()));
    let id_of = |client: &Client, name: &str| {
        client
            .read()
            .conversations
            .iter()
            .find(|x| x.name.eq_ignore_ascii_case(name))
            .map(|x| x.id.clone())
            .unwrap()
    };
    let home_id = id_of(&client, &home[1..]);
    let run = |client: &Client, conversation: &str, input: &str| {
        let plan = commands::plan(&client.read(), conversation, input)
            .unwrap_or_else(|p| panic!("{input}: {p:?}"));
        rt.block_on(commands::execute(client, plan))
            .unwrap()
            .unwrap_or_else(|e| panic!("{input}: {e}"))
    };

    // /join opens a new channel, and the network puts you in it.
    let other = unique("cmdj");
    let done = run(&client, &home_id, &format!("/join #{other}"));
    let opened = done.open.expect("/join opens the channel");
    pump("the joined channel", &joined(other.clone()));
    assert_eq!(id_of(&client, &other), opened);

    // /topic there, as the operator who created it.
    let topic = format!("set by /topic {}", &random_id()[..6]);
    let planned = commands::plan(&client.read(), &opened, &format!("/topic {topic}")).unwrap();
    assert!(matches!(planned, Plan::Raw { .. }));
    run(&client, &opened, &format!("/topic {topic}"));
    pump("the new topic", &|c| {
        c.read()
            .conversation(&opened)
            .is_some_and(|x| x.topic == topic)
    });

    // /nick changes your nick on the network.
    let nick = unique("nk");
    run(&client, &home_id, &format!("/nick {nick}"));
    let network = client
        .read()
        .conversation(&home_id)
        .unwrap()
        .network_id
        .clone();
    pump("the new nick", &|c| {
        c.read()
            .network(&network)
            .is_some_and(|n| n.nick.eq_ignore_ascii_case(&nick))
    });
    eprintln!("live commands passed for {username}");
}

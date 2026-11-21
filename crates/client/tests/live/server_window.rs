//! Live cases for package SV1: server log and latency.
//!
//! Start each case with `let Some((server, irc)) = crate::support::environment() else { return };`.

use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::StreamExt;
use ircord_client::api::Api;
use ircord_client::client::{random_id, runtime, sign_in};
use ircord_client::contract::{NetworkStatus, ServerLineKind};
use ircord_client::credentials::{CredentialStore, MemoryStore};
use ircord_client::store::Connection;
use ircord_client::store::server_log::{self, Buffer};
use ircord_client::{Client, ServerAddress};

use crate::support::{environment, setup_account, unique, wait_for_backend};

#[test]
fn the_server_log_latency_and_a_raw_line() {
    let Some((server, irc)) = environment() else {
        return;
    };
    let home = format!("#{}", unique("sv"));
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
    pump("live", &|c| c.read().connection == Some(Connection::Live));
    pump("the network to connect", &|c| {
        c.read()
            .networks
            .first()
            .is_some_and(|n| n.status == NetworkStatus::Connected)
    });
    let network = client.read().networks[0].id.clone();

    // The log holds the welcome and the MOTD.
    server_log::load(client.handle(), &network, Buffer::Log);
    pump("the server log", &|c| {
        c.read()
            .server_log
            .log(&network, Buffer::Log)
            .is_some_and(|l| l.loaded && !l.lines.is_empty())
    });
    {
        let w = client.read();
        let log = w.server_log.log(&network, Buffer::Log).unwrap();
        assert_eq!(log.error, None);
        assert!(
            log.lines.iter().any(|l| l.command == "001"),
            "no welcome in {:?}",
            log.lines.iter().map(|l| &l.command).collect::<Vec<_>>()
        );
    }

    // A raw line shows as sent, and the server's answer arrives live.
    let token = unique("raw");
    rt.block_on(server_log::send_raw(
        client.handle(),
        &network,
        format!("WHOIS {token}"),
    ))
    .unwrap()
    .expect("send the raw line");
    pump("the sent line in the log", &|c| {
        c.read()
            .server_log
            .log(&network, Buffer::Log)
            .is_some_and(|l| {
                l.lines
                    .iter()
                    .any(|x| x.kind == ServerLineKind::Outgoing && x.text.contains(&token))
                    || l.stale
            })
    });
    // Read again if the server said the log changed meanwhile.
    server_log::load(client.handle(), &network, Buffer::Log);
    pump("the answer to WHOIS", &|c| {
        c.read()
            .server_log
            .log(&network, Buffer::Log)
            .is_some_and(|l| {
                // ERR_NOSUCHNICK or the end of the WHOIS, naming the token.
                l.lines.iter().any(|x| {
                    x.kind != ServerLineKind::Outgoing
                        && (x.text.contains(&token) || x.params.iter().any(|p| p == &token))
                })
            })
    });

    // Details, latency and the device's ping.
    server_log::load_connection(client.handle(), &network);
    server_log::load_latency(client.handle(), &network);
    rt.block_on(server_log::ping(client.handle())).unwrap();
    pump("the details and the latency", &|c| {
        let w = c.read();
        let s = &w.server_log;
        s.connections
            .get(&network)
            .is_some_and(|x| x.value.is_some())
            && s.latency.get(&network).is_some_and(|x| x.value.is_some())
    });
    {
        let w = client.read();
        let s = &w.server_log;
        let info = s.connections[&network].value.as_ref().unwrap();
        assert_eq!(info.network_id, network);
        assert!(!info.server_name.is_empty());
        assert!(!info.isupport.is_empty(), "ISUPPORT is read on connect");
        let latency = s.latency[&network].value.as_ref().unwrap();
        assert!(latency.interval_seconds > 0);
        assert!(latency.window_seconds > 0);
        assert_eq!(s.device.len(), 1, "the ping is timed");
        assert!(!s.ping_failing);
    }

    // The trace reads too.
    server_log::load(client.handle(), &network, Buffer::Trace);
    pump("the trace", &|c| {
        c.read()
            .server_log
            .log(&network, Buffer::Trace)
            .is_some_and(|l| l.loaded)
    });
    assert_eq!(
        client
            .read()
            .server_log
            .log(&network, Buffer::Trace)
            .unwrap()
            .error,
        None
    );
    eprintln!("live server window passed for {username}");
}

//! Live cases for package L1: resolve a link in a message.
//!
//! Start each case with `let Some((server, irc)) = crate::support::environment() else { return };`.

use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::StreamExt;
use ircord_client::api::{Api, ApiError};
use ircord_client::client::{random_id, runtime, sign_in};
use ircord_client::contract::{LinkMediaResponseKind, MessageInputBody};
use ircord_client::credentials::{CredentialStore, MemoryStore};
use ircord_client::store::Connection;
use ircord_client::store::link_media;
use ircord_client::{Client, ServerAddress};

use crate::support::{environment, setup_account, unique, wait_for_backend};

#[test]
fn resolves_a_link_and_finds_a_linked_message() {
    let Some((server, irc)) = environment() else {
        return;
    };
    let channel = format!("#{}", unique("lm"));
    let rt = runtime().unwrap();
    let address = ServerAddress::parse(&server).unwrap();
    let username = unique("gpui");
    let password = random_id();
    wait_for_backend(&rt, &address);
    rt.block_on(setup_account(
        &address, &username, &password, &irc, &channel,
    ));

    let api = Api::new(address.clone());
    let store: Arc<dyn CredentialStore> = Arc::new(MemoryStore::default());
    rt.block_on(sign_in(&api, store.as_ref(), &username, &password))
        .expect("sign in");
    let (client, mut updates) = Client::start(rt.clone(), api.clone());
    let mut pump = |what: &str, deadline: Instant, done: &dyn Fn(&Client) -> bool| {
        while !done(&client) {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            rt.block_on(async {
                let _ = tokio::time::timeout(Duration::from_millis(300), updates.next()).await;
            });
        }
    };
    pump("live", Instant::now() + Duration::from_secs(30), &|c| {
        c.read().connection == Some(Connection::Live)
    });
    let name = channel.trim_start_matches('#').to_string();
    pump(
        "the channel",
        Instant::now() + Duration::from_secs(45),
        &|c| {
            c.read()
                .conversations
                .iter()
                .any(|x| x.name.eq_ignore_ascii_case(&name) && x.joined)
        },
    );
    let (conversation, network) = {
        let w = client.read();
        let c = w
            .conversations
            .iter()
            .find(|x| x.name.eq_ignore_ascii_case(&name))
            .unwrap();
        (c.id.clone(), c.network_id.clone())
    };
    let _ = network;

    // A link the server cannot reach still answers, as a card with a reason.
    let link = "https://preview.invalid/page";
    let body = MessageInputBody {
        text: format!("see {link}."),
        client_id: random_id(),
        ..Default::default()
    };
    let answer = rt
        .block_on(api.send_message(&conversation, &body))
        .expect("send the message");
    client.change(|w| w.send_answered(&answer));
    client.open(&conversation);
    pump(
        "the message with the link",
        Instant::now() + Duration::from_secs(20),
        &|c| {
            c.read()
                .timelines
                .get(&conversation)
                .is_some_and(|t| t.entries.iter().any(|e| e.message.text.contains(link)))
        },
    );
    let message = {
        let w = client.read();
        let t = w.timelines.get(&conversation).unwrap();
        t.entries
            .iter()
            .find(|e| e.message.text.contains(link))
            .unwrap()
            .message
            .id
            .clone()
    };
    let preview = rt
        .block_on(link_media::resolve(
            api.clone(),
            conversation.clone(),
            message.clone(),
            link.to_string(),
        ))
        .expect("resolve the link");
    assert_eq!(preview.kind, LinkMediaResponseKind::Card);
    let card = preview.card.expect("a card");
    assert_eq!(card.site, "preview.invalid");
    assert!(card.reason.is_some(), "it says why there is no preview");

    // A link that is not in the message is not resolved.
    let other =
        rt.block_on(api.resolve_link_media(&conversation, &message, "https://other.invalid/"));
    assert!(
        matches!(other, Err(ApiError::Http { status: 404, .. })),
        "{other:?}"
    );

    // A message link finds your copy of the message.
    let found = rt
        .block_on(api.locate_message(&message))
        .expect("locate the message");
    assert_eq!(found.conversation_id, conversation);
    client.change(|w| {
        assert!(
            !w.begin_linked_message(&conversation, &message),
            "a loaded message is not looked up"
        );
        Default::default()
    });
}

//! Live cases for package FD1: find a sent message through a real backend,
//! by its text, by its sender and in its conversation, and page past it.

use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::StreamExt;
use ircord_client::api::Api;
use ircord_client::client::{random_id, runtime, sign_in};
use ircord_client::credentials::{CredentialStore, MemoryStore};
use ircord_client::search_filter::{Chip, Filters, Query, Source, is_jumpable};
use ircord_client::store::Connection;
use ircord_client::{Client, ServerAddress};

use crate::support::{environment, setup_account, unique, wait_for_backend};

#[test]
fn finds_a_sent_message() {
    let Some((server, irc)) = environment() else {
        return;
    };
    let rt = runtime().unwrap();
    let address = ServerAddress::parse(&server).unwrap();
    let username = unique("gpuis");
    let password = random_id();
    wait_for_backend(&rt, &address);
    let channel = format!("#{}", unique("fd1"));
    rt.block_on(setup_account(
        &address, &username, &password, &irc, &channel,
    ));

    let api = Api::new(address.clone());
    let store: Arc<dyn CredentialStore> = Arc::new(MemoryStore::default());
    rt.block_on(sign_in(&api, store.as_ref(), &username, &password))
        .expect("sign in");
    let (client, mut updates) = Client::start(rt.clone(), api.clone());
    let mut next = |wait: Duration| {
        rt.block_on(async { tokio::time::timeout(wait, updates.next()).await.ok() });
    };
    let name = channel.trim_start_matches('#');
    let deadline = Instant::now() + Duration::from_secs(45);
    let conversation = loop {
        let found = client
            .read()
            .conversations
            .iter()
            .find(|c| c.name.eq_ignore_ascii_case(name) && c.joined)
            .map(|c| c.id.clone());
        if let (Some(id), Some(Connection::Live)) = (found, client.read().connection.clone()) {
            break id;
        }
        assert!(Instant::now() < deadline, "the channel never appeared");
        next(Duration::from_millis(500));
    };
    client.open(&conversation);

    // Say something nobody else has said.
    let tag = format!("needle{}", &random_id()[..8]).to_lowercase();
    client.send(&conversation, &format!("the {tag} in the haystack"));
    let deadline = Instant::now() + Duration::from_secs(30);
    let sent = loop {
        let id = client.read().timelines.get(&conversation).and_then(|t| {
            t.entries
                .iter()
                .find(|e| e.message.text.contains(&tag) && !e.message.id.is_empty())
                .map(|e| (e.message.id.clone(), e.message.sender_id.clone()))
        });
        if let Some(found) = id
            && !client.read().outbox.iter().any(|p| p.text.contains(&tag))
        {
            break found;
        }
        assert!(Instant::now() < deadline, "the message never arrived");
        next(Duration::from_millis(300));
    };

    // Search everywhere for its text; indexing may take a moment.
    let deadline = Instant::now() + Duration::from_secs(20);
    let page = loop {
        let page = rt
            .block_on(api.search(&Query::new(&tag, Filters::default(), Source::Retained)))
            .expect("search");
        if !page.items.is_empty() {
            break page;
        }
        assert!(Instant::now() < deadline, "search never found {tag}");
        std::thread::sleep(Duration::from_millis(500));
    };
    let hit = &page.items[0];
    assert_eq!(hit.message.id, sent.0, "{page:?}");
    assert_eq!(hit.conversation.id, conversation);
    assert!(is_jumpable(hit));

    // The final word is a prefix, and filters narrow the same search.
    let prefix = &tag[..tag.len() - 2];
    let scoped = Filters::default()
        .with(Chip::From(sent.1.clone()))
        .scoped(&conversation);
    let page = rt
        .block_on(api.search(&Query::new(prefix, scoped, Source::Retained)))
        .expect("search with filters");
    assert!(
        page.items.iter().any(|r| r.message.id == sent.0),
        "{page:?}"
    );

    // Someone else, or a day long before, finds nothing.
    let nobody = Filters::default()
        .with(Chip::From("m-nobody".into()))
        .scoped(&conversation);
    let page = rt
        .block_on(api.search(&Query::new(&tag, nobody, Source::Retained)))
        .expect("search from nobody");
    assert!(page.items.is_empty(), "{page:?}");
    let long_ago = Filters::default().with(Chip::Before("2001-01-01".into()));
    let page = rt
        .block_on(api.search(&Query::new(&tag, long_ago, Source::Retained)))
        .expect("search long ago");
    assert!(page.items.is_empty(), "{page:?}");

    // Asking for server history in one conversation is answered, whatever
    // this IRC server supports, and says where the results came from.
    let page = rt
        .block_on(api.search(&Query::new(
            &tag,
            Filters::default().scoped(&conversation),
            Source::Upstream,
        )))
        .expect("server history search");
    assert!(page.source.is_some(), "{page:?}");
}

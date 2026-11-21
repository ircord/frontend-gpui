//! Live cases for package FD3: links posted in a channel are listed, and
//! its pictures make up the gallery.
//!
//! Start each case with `let Some((server, irc)) = crate::support::environment() else { return };`.

use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::StreamExt;
use ircord_client::api::Api;
use ircord_client::api::links::LinkFilter;
use ircord_client::client::{random_id, runtime, sign_in};
use ircord_client::contract::{LinkCardKind, LinkPage, MessageInputBody};
use ircord_client::credentials::{CredentialStore, MemoryStore};
use ircord_client::store::Connection;
use ircord_client::{Client, ServerAddress};

use crate::support::{environment, setup_account, unique, wait_for_backend};

#[test]
fn posted_links_are_listed_and_pictures_make_the_gallery() {
    let Some((server, irc)) = environment() else {
        return;
    };
    let channel = format!("#{}", unique("lv"));
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
    let conversation = client
        .read()
        .conversations
        .iter()
        .find(|x| x.name.eq_ignore_ascii_case(&name))
        .map(|x| x.id.clone())
        .unwrap();

    // A page link, posted twice, and a picture.
    let tag = unique("fd3");
    let page = format!("https://links.invalid/{tag}");
    let picture = format!("https://links.invalid/{tag}.png");
    for text in [
        format!("read this: {page}"),
        format!("again {page}"),
        format!("a cat {picture}"),
    ] {
        let body = MessageInputBody {
            text,
            client_id: random_id(),
            ..Default::default()
        };
        rt.block_on(api.send_message(&conversation, &body))
            .expect("send a message");
    }

    let list = |filter: &LinkFilter, want: &dyn Fn(&LinkPage) -> bool| -> LinkPage {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let page = rt
                .block_on(api.list_links(filter, None))
                .expect("list the links");
            if want(&page) {
                return page;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for the catalog: {page:?}"
            );
            std::thread::sleep(Duration::from_millis(300));
        }
    };

    // The channel's links: the page once, as posted twice, newest first.
    let all = LinkFilter {
        conversations: vec![conversation.clone()],
        ..Default::default()
    };
    let page_of = list(&all, &|p| p.items.len() >= 2);
    let first = &page_of.items[0];
    assert_eq!(first.url, picture, "the newest post is first");
    assert_eq!(first.kind, LinkCardKind::Image);
    let shared = page_of
        .items
        .iter()
        .find(|c| c.url == page)
        .expect("the page link");
    assert_eq!(shared.count, 2, "two posts of one page are one card");
    assert_eq!(shared.host, "links.invalid");
    assert!(
        shared
            .sources
            .iter()
            .all(|s| s.conversation_id == conversation)
    );

    // Text and site narrow it.
    let again = LinkFilter {
        q: "again".into(),
        ..all.clone()
    };
    let found = list(&again, &|p| !p.items.is_empty());
    assert!(found.items.iter().all(|c| c.url == page), "{found:?}");
    let elsewhere = LinkFilter {
        host: "example.com".into(),
        ..all.clone()
    };
    let none = rt
        .block_on(api.list_links(&elsewhere, None))
        .expect("list by host");
    assert!(none.items.is_empty(), "{none:?}");

    // The gallery's filter keeps the picture only.
    let gallery = list(&LinkFilter::pictures(&conversation), &|p| {
        !p.items.is_empty()
    });
    assert_eq!(gallery.items.len(), 1, "{gallery:?}");
    assert_eq!(gallery.items[0].url, picture);
}

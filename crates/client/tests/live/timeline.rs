//! Live cases for package R1: a window of history around an older message,
//! paging from it back to the present, locating a message by its ID, and
//! marking a conversation unread from a message, through a real backend.

use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::StreamExt;
use futures::channel::mpsc::UnboundedReceiver;
use ircord_client::api::Api;
use ircord_client::client::{runtime, sign_in};
use ircord_client::credentials::{CredentialStore, MemoryStore};
use ircord_client::events::Update;
use ircord_client::store::Connection;
use ircord_client::store::timeline::NEWER_SIZE;
use ircord_client::{Client, ServerAddress};
use tokio::io::AsyncWriteExt;

use crate::support::{
    environment, irc_host_for_test, irc_user, setup_account, unique, wait_for_backend,
};

const CHANNEL: &str = "#gpui-r1";
const SPOKEN: usize = 8;

#[test]
fn around_an_old_message_back_to_the_present_and_unread() {
    let Some((server, irc)) = environment() else {
        return;
    };
    let rt = runtime().unwrap();
    let address = ServerAddress::parse(&server).unwrap();
    let username = unique("gpuir");
    let password = ircord_client::client::random_id();
    wait_for_backend(&rt, &address);
    rt.block_on(setup_account(&address, &username, &password, &irc, CHANNEL));

    let api = Api::new(address.clone());
    let store: Arc<dyn CredentialStore> = Arc::new(MemoryStore::default());
    rt.block_on(sign_in(&api, store.as_ref(), &username, &password))
        .expect("sign in");
    let (client, mut updates) = Client::start(rt.clone(), api.clone());
    let next = |updates: &mut UnboundedReceiver<Update>, wait: Duration| {
        rt.block_on(async {
            tokio::time::timeout(wait, updates.next())
                .await
                .ok()
                .flatten()
        })
    };
    let deadline = Instant::now() + Duration::from_secs(45);
    let conversation = loop {
        let found = client
            .read()
            .conversations
            .iter()
            .find(|c| c.name.eq_ignore_ascii_case(CHANNEL.trim_start_matches('#')) && c.joined)
            .map(|c| c.id.clone());
        if let (Some(id), Some(Connection::Live)) = (found, client.read().connection.clone()) {
            break id;
        }
        assert!(Instant::now() < deadline, "the channel never appeared");
        next(&mut updates, Duration::from_millis(500));
    };
    client.open(&conversation);

    // A plain IRC user says a few things.
    let nick = unique("r1u");
    let (_lines, mut write) = rt.block_on(irc_user(&irc_host_for_test(&irc), &nick, CHANNEL));
    let tag = &ircord_client::client::random_id()[..6];
    for i in 0..SPOKEN {
        let line = format!("PRIVMSG {CHANNEL} :r1 {tag} {i}\r\n");
        rt.block_on(async {
            write.write_all(line.as_bytes()).await.unwrap();
            tokio::time::sleep(Duration::from_millis(150)).await;
        });
    }
    let deadline = Instant::now() + Duration::from_secs(30);
    let ids: Vec<String> = loop {
        let w = client.read();
        let ids: Vec<String> = w
            .timelines
            .get(&conversation)
            .map(|t| {
                t.entries
                    .iter()
                    .filter(|e| e.message.text.starts_with(&format!("r1 {tag} ")))
                    .map(|e| e.message.id.clone())
                    .collect()
            })
            .unwrap_or_default();
        drop(w);
        if ids.len() == SPOKEN {
            break ids;
        }
        assert!(
            Instant::now() < deadline,
            "only {} messages arrived",
            ids.len()
        );
        next(&mut updates, Duration::from_millis(300));
    };

    // A small window around the oldest has newer history after it.
    let oldest = ids[0].clone();
    let page = rt
        .block_on(api.messages_around(&conversation, &oldest, 3))
        .expect("read around a message");
    assert!(page.items.iter().any(|m| m.id == oldest), "{page:?}");
    assert_eq!(page.has_newer, Some(true), "{page:?}");
    client.change(|w| w.open_around(&conversation, &oldest, page));
    assert_eq!(client.read().focused(&conversation), Some(oldest.as_str()));

    // Paging newer reaches the present.
    let mut pages = 0;
    loop {
        let mut boundary = None;
        client.change(|w| {
            boundary = w.begin_newer(&conversation);
            Default::default()
        });
        let Some(boundary) = boundary else { break };
        let page = rt
            .block_on(api.messages_around(&conversation, &boundary, NEWER_SIZE))
            .map_err(|e| e.to_string());
        client.change(|w| w.load_newer(&conversation, &boundary, page));
        let t = client.read().timelines[&conversation].clone();
        assert!(
            !t.newer_failed,
            "a page towards the present did not advance"
        );
        pages += 1;
        assert!(pages < 10, "paging newer never reached the present");
    }
    let t = client.read().timelines[&conversation].clone();
    assert!(!t.has_newer);
    let held: Vec<&str> = t.entries.iter().map(|e| e.message.id.as_str()).collect();
    for id in &ids {
        assert!(held.contains(&id.as_str()), "{id} is missing after paging");
    }

    // Locating a message names its conversation.
    let found = rt
        .block_on(api.locate_message(&ids[3]))
        .expect("locate a message");
    assert_eq!(found.conversation_id, conversation);

    // Unread from a message: the server reports the boundary back.
    rt.block_on(api.mark_unread(&conversation, &ids[5]))
        .expect("mark unread");
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let from = client
            .read()
            .conversation(&conversation)
            .and_then(|c| c.unread_from.clone());
        if from.as_deref() == Some(ids[5].as_str()) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the unread boundary never came back: {from:?}"
        );
        next(&mut updates, Duration::from_millis(300));
    }
    client.change(|w| {
        w.begin_visit(&conversation);
        Default::default()
    });
    let line = client
        .read()
        .unread_line_at(&conversation)
        .map(str::to_string);
    assert_eq!(line.as_deref(), Some(ids[5].as_str()));
    let _ = rt.block_on(write.write_all(b"QUIT :done\r\n"));
}

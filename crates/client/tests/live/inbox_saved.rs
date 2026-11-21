//! Live cases for package FD2: a mention lands in the inbox, is dismissed
//! and restored; a message is saved, found on the saved list (also by its
//! text) and removed, through a real backend.
//!
//! Start each case with `let Some((server, irc)) = crate::support::environment() else { return };`.

use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::StreamExt;
use futures::channel::mpsc::UnboundedReceiver;
use ircord_client::api::Api;
use ircord_client::client::{runtime, sign_in};
use ircord_client::contract::InboxItemReasonsItem;
use ircord_client::credentials::{CredentialStore, MemoryStore};
use ircord_client::events::Update;
use ircord_client::store::inbox::{self, Filter};
use ircord_client::store::{Connection, Workspace, saved};
use ircord_client::{Client, ServerAddress};
use tokio::io::AsyncWriteExt;

use crate::support::{
    environment, irc_host_for_test, irc_user, setup_account, unique, wait_for_backend,
};

const CHANNEL: &str = "#gpui-fd2";

#[test]
fn a_mention_reaches_the_inbox_and_a_saved_message_the_saved_list() {
    let Some((server, irc)) = environment() else {
        return;
    };
    let rt = runtime().unwrap();
    let address = ServerAddress::parse(&server).unwrap();
    let username = unique("gpuif");
    let password = ircord_client::client::random_id();
    wait_for_backend(&rt, &address);
    rt.block_on(setup_account(&address, &username, &password, &irc, CHANNEL));

    let api = Api::new(address.clone());
    let store: Arc<dyn CredentialStore> = Arc::new(MemoryStore::default());
    rt.block_on(sign_in(&api, store.as_ref(), &username, &password))
        .expect("sign in");
    let (client, mut updates) = Client::start(rt.clone(), api.clone());
    let next = |updates: &mut UnboundedReceiver<Update>| {
        rt.block_on(async {
            tokio::time::timeout(Duration::from_millis(250), updates.next())
                .await
                .ok()
                .flatten()
        })
    };
    let mut until = |what: &str, secs: u64, done: &dyn Fn(&Workspace) -> bool| {
        let deadline = Instant::now() + Duration::from_secs(secs);
        while !done(&client.read()) {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            next(&mut updates);
        }
    };
    until("the channel, live", 45, &|w| {
        w.connection == Some(Connection::Live)
            && w.conversations
                .iter()
                .any(|c| c.joined && c.name.eq_ignore_ascii_case(CHANNEL.trim_start_matches('#')))
    });
    let conversation = client
        .read()
        .conversations
        .iter()
        .find(|c| c.name.eq_ignore_ascii_case(CHANNEL.trim_start_matches('#')))
        .map(|c| c.id.clone())
        .unwrap();
    client.open(&conversation);
    inbox::keep(&client);
    saved::keep(&client);
    until("the inbox count and the saved IDs", 20, &|w| {
        w.inbox.unread.is_some() && w.saved.ids_loaded
    });

    // A plain IRC user mentions the account's nick (an @mention).
    let nick = unique("fd2u");
    let (_lines, mut write) = rt.block_on(irc_user(&irc_host_for_test(&irc), &nick, CHANNEL));
    let tag = ircord_client::client::random_id()[..6].to_string();
    let line = format!("PRIVMSG {CHANNEL} :@{username} fd2 {tag}\r\n");
    rt.block_on(write.write_all(line.as_bytes())).unwrap();
    let text = format!("@{username} fd2 {tag}");
    until("the mention in the timeline", 30, &|w| {
        w.timelines
            .get(&conversation)
            .is_some_and(|t| t.entries.iter().any(|e| e.message.text == text))
    });
    let message = client
        .read()
        .timelines
        .get(&conversation)
        .and_then(|t| t.entries.iter().find(|e| e.message.text == text))
        .map(|e| e.message.id.clone())
        .unwrap();

    // The inbox.added event reads the count again; the list has it as a mention.
    until("the inbox count", 30, &|w| w.inbox.unread.unwrap_or(0) >= 1);
    inbox::load(client.handle(), Filter::Mentions);
    until("the inbox item", 20, &|w| {
        w.inbox.list(Filter::Mentions).is_some_and(|l| {
            l.items.iter().any(|i| {
                i.message_id == message && i.reasons.contains(&InboxItemReasonsItem::Mention)
            })
        })
    });

    // Dismissed, it moves to the Dismissed view; restored, it comes back.
    rt.block_on(inbox::set_dismissed(client.handle(), &message, true))
        .unwrap()
        .expect("dismiss");
    inbox::load(client.handle(), Filter::Dismissed);
    until("the dismissed item", 20, &|w| {
        w.inbox
            .list(Filter::Dismissed)
            .is_some_and(|l| !l.loading && l.items.iter().any(|i| i.message_id == message))
    });
    rt.block_on(inbox::set_dismissed(client.handle(), &message, false))
        .unwrap()
        .expect("restore");
    inbox::load(client.handle(), Filter::All);
    until("the restored item", 20, &|w| {
        w.inbox.list(Filter::All).is_some_and(|l| {
            !l.loading
                && l.items
                    .iter()
                    .any(|i| i.message_id == message && !i.dismissed)
        })
    });

    // Saved, it is marked (also by the saved.added event) and listed.
    rt.block_on(saved::set_saved(
        client.handle(),
        &conversation,
        &message,
        true,
    ))
    .unwrap()
    .expect("save");
    until("the saved mark", 20, &|w| {
        w.saved.is_saved(&message) && w.saved.pending.is_empty()
    });
    saved::load(client.handle(), "");
    until("the saved list", 20, &|w| {
        let l = &w.saved.list;
        l.loaded
            && !l.loading
            && l.items.iter().any(|i| {
                i.message_id == message && i.message.as_ref().is_some_and(|m| m.text == text)
            })
    });
    saved::load(client.handle(), &tag);
    until("the saved list filtered by its text", 20, &|w| {
        let l = &w.saved.list;
        l.query == tag && l.loaded && !l.loading && l.items.len() == 1
    });
    saved::load(client.handle(), "nothing-like-it");
    until("no match", 20, &|w| {
        let l = &w.saved.list;
        l.query == "nothing-like-it" && l.loaded && !l.loading && l.items.is_empty()
    });

    // Removed, the mark goes (also by the saved.removed event).
    rt.block_on(saved::set_saved(
        client.handle(),
        &conversation,
        &message,
        false,
    ))
    .unwrap()
    .expect("remove");
    saved::load_ids(client.handle());
    until("the mark gone", 20, &|w| {
        !w.saved.is_saved(&message) && !w.saved.ids_loading
    });
}

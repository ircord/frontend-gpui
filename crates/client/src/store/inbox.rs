//! The inbox: `inbox.added`, `inbox.updated` and the badge count.
//!
//! Owned by package FD2. The state lives in [`State`] (the workspace's
//! `inbox` field): the unread count the badge shows, and one list per inbox
//! view, each read when it is shown. Every change is reported as
//! `Changes::topic(TOPIC)`.
//!
//! The inbox holds mentions, replies to your messages and direct messages,
//! kept by the server whether or not a device was open. Dismissing an item
//! only hides it; it never marks its conversation read. Anything that may
//! change what the inbox shows (a new item, a dismissal on any device, a
//! read, a deletion, a removed conversation) marks the count and the lists
//! out of date: [`keep`] reads the count again, and the page reads the list
//! it shows. Follows the web client's `app/queries/inbox.ts`.

use std::collections::{HashMap, HashSet};

use ircord_contract::event::{Durable, ServerEvent};
use ircord_contract::*;

use super::{Changes, Workspace};
use crate::api::ApiError;
use crate::client::{Client, Handle};

/// The topic this sub-state reports in [`Changes`].
pub const TOPIC: &str = "inbox";
/// Items asked for per page (the server allows 1 to 100).
pub const PAGE: u32 = 30;

/// One view of the inbox.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Filter {
    /// Every undismissed item.
    #[default]
    All,
    /// Undismissed items still unread in their conversation.
    Unread,
    Mentions,
    Replies,
    Direct,
    /// Dismissed items, to restore.
    Dismissed,
}

impl Filter {
    pub const ALL: [Filter; 6] = [
        Filter::All,
        Filter::Unread,
        Filter::Mentions,
        Filter::Replies,
        Filter::Direct,
        Filter::Dismissed,
    ];

    /// The `filter` the server takes.
    pub fn as_str(self) -> &'static str {
        match self {
            Filter::All => "all",
            Filter::Unread => "unread",
            Filter::Mentions => "mentions",
            Filter::Replies => "replies",
            Filter::Direct => "direct",
            Filter::Dismissed => "dismissed",
        }
    }

    /// Whether an item that is (or is not) dismissed belongs in this view.
    fn holds_dismissed(self, dismissed: bool) -> bool {
        (self == Filter::Dismissed) == dismissed
    }
}

#[derive(Debug, Clone, Default)]
pub struct State {
    /// Undismissed items still unread in their conversation; `None` until read.
    pub unread: Option<i64>,
    /// The count may be out of date and is read again.
    pub unread_stale: bool,
    pub unread_loading: bool,
    /// Each view's list, once it has been shown.
    pub lists: HashMap<Filter, List>,
    /// Items being dismissed or restored right now.
    pub pending: HashSet<String>,
    /// Each conversation's unread count and manual boundary, to tell a read
    /// (which reads its inbox items) from new messages.
    counts: HashMap<String, (i64, Option<String>)>,
    /// [`keep`] has installed its watcher.
    kept: bool,
}

impl State {
    pub fn list(&self, filter: Filter) -> Option<&List> {
        self.lists.get(&filter)
    }

    /// Everything the inbox shows may have changed.
    fn changed(&mut self) -> Changes {
        self.unread_stale = true;
        for list in self.lists.values_mut() {
            list.stale |= list.loaded;
        }
        Changes::topic(TOPIC)
    }

    fn holds(&self, message: &str) -> bool {
        self.lists
            .values()
            .any(|l| l.items.iter().any(|i| i.message_id == message))
    }

    /// Show an item as dismissed or restored in every loaded list.
    fn mark(&mut self, message: &str, dismissed: bool) {
        for (filter, list) in self.lists.iter_mut() {
            for item in list.items.iter_mut() {
                if item.message_id == message {
                    item.dismissed = dismissed;
                }
            }
            if !filter.holds_dismissed(dismissed) {
                list.items.retain(|i| i.message_id != message);
            }
        }
    }
}

/// One view's items, newest first.
#[derive(Debug, Clone, Default)]
pub struct List {
    pub items: Vec<InboxItem>,
    /// Where older items continue, if there are more.
    pub next: Option<String>,
    pub loaded: bool,
    pub loading: bool,
    pub loading_more: bool,
    /// Something listed changed: read the list again when it is showing.
    pub stale: bool,
    pub error: Option<String>,
    pub more_error: Option<String>,
    generation: u64,
}

impl List {
    /// Whether the view showing this list should read it (again).
    pub fn wanted(list: Option<&List>) -> bool {
        list.is_none_or(|l| !l.loading && (l.stale || (!l.loaded && l.error.is_none())))
    }
}

/// Called for every fresh event after the workspace's own handling.
pub(super) fn apply(w: &mut Workspace, event: &ServerEvent, _now_ms: i64) -> Changes {
    let ServerEvent::Durable { event, .. } = event else {
        return Changes::default();
    };
    let state = &mut w.inbox;
    match event {
        Durable::InboxAdded(entry) if !entry.message_id.is_empty() => state.changed(),
        Durable::InboxUpdated(change) if !change.message_id.is_empty() => {
            state.mark(&change.message_id, change.dismissed);
            state.changed()
        }
        // A deleted message drops its excerpt and leaves the unread count.
        Durable::MessageUpdated(m) if m.deleted == Some(true) && state.holds(&m.id) => {
            state.changed()
        }
        Durable::MessageUpdated(m)
            if m.deleted == Some(true) && state.unread.is_some_and(|n| n > 0) =>
        {
            state.changed()
        }
        Durable::ConversationRemoved(_) | Durable::NetworkRemoved(_) => state.changed(),
        Durable::ConversationUpdated(c) => read(state, &c.id, c.unread_count, &c.unread_from),
        Durable::ConversationCounts(c) => read(state, &c.id, c.unread_count, &c.unread_from),
        _ => Changes::default(),
    }
}

/// A conversation's counts changed. Reading it (here or on another device)
/// reads its inbox items; with nothing unread there is nothing to refresh,
/// and busy channels change their counts on every message.
fn read(state: &mut State, id: &str, unread: i64, from: &Option<String>) -> Changes {
    let now = (unread, from.clone());
    let Some(before) = state.counts.insert(id.to_string(), now) else {
        return Changes::default();
    };
    let boundary_moved = before.1 != *from;
    let lowered = unread < before.0;
    if boundary_moved || (lowered && state.unread != Some(0)) {
        state.changed()
    } else {
        Changes::default()
    }
}

/// Called when a bootstrap replaces the workspace, before it is replaced.
pub(super) fn reset(w: &mut Workspace, snapshot: &Snapshot) -> Changes {
    let state = &mut w.inbox;
    state.counts = snapshot
        .conversations
        .iter()
        .map(|c| (c.id.clone(), (c.unread_count, c.unread_from.clone())))
        .collect();
    state.changed()
}

/// Read the unread count now, and again whenever it may be out of date. Call
/// it once per client; later calls do nothing.
pub fn keep(client: &Client) {
    if client.read().inbox.kept {
        return;
    }
    let mut first = false;
    client.change(|w| {
        first = !w.inbox.kept;
        w.inbox.kept = true;
        Changes::default()
    });
    if !first {
        return;
    }
    client.watch(|handle, changes| {
        if changes.touches(TOPIC) {
            let wanted = {
                let w = handle.read();
                w.inbox.unread_stale && !w.inbox.unread_loading
            };
            if wanted {
                load_unread(handle);
            }
        }
    });
    load_unread(client.handle());
}

/// Read the unread count.
pub fn load_unread(handle: &Handle) {
    let mut started = false;
    handle.change(|w| {
        if w.inbox.unread_loading {
            return Changes::default();
        }
        started = true;
        w.inbox.unread_loading = true;
        w.inbox.unread_stale = false;
        Changes::default()
    });
    if !started {
        return;
    }
    handle.call(
        |api| async move { api.inbox_summary().await },
        |w, result| {
            let state = &mut w.inbox;
            state.unread_loading = false;
            if let Ok(summary) = result {
                state.unread = Some(summary.unread.max(0));
            }
            // A change that came in while reading reads again (see `keep`).
            Changes::topic(TOPIC)
        },
    );
}

/// Read the first page of a view, replacing its list.
pub fn load(handle: &Handle, filter: Filter) {
    let mut generation = 0;
    handle.change(|w| {
        let list = w.inbox.lists.entry(filter).or_default();
        list.generation += 1;
        generation = list.generation;
        list.loading = true;
        list.loading_more = false;
        list.stale = false;
        list.error = None;
        list.more_error = None;
        Changes::topic(TOPIC)
    });
    handle.call(
        move |api| async move { api.inbox(filter.as_str(), None, PAGE).await },
        move |w, result| {
            let state = &mut w.inbox;
            let list = state.lists.entry(filter).or_default();
            if list.generation != generation {
                return Changes::default();
            }
            list.loading = false;
            match result {
                Ok(page) => {
                    list.items = page.items;
                    list.next = page.next_cursor;
                    list.loaded = true;
                    state.unread = Some(page.unread.max(0));
                }
                Err(e) => list.error = Some(e.to_string()),
            }
            Changes::topic(TOPIC)
        },
    );
}

/// Read a view's next older page and add it to its list.
pub fn load_more(handle: &Handle, filter: Filter) {
    let mut from = None;
    let mut generation = 0;
    handle.change(|w| {
        let Some(list) = w.inbox.lists.get_mut(&filter) else {
            return Changes::default();
        };
        if list.loading || list.loading_more || list.next.is_none() {
            return Changes::default();
        }
        list.loading_more = true;
        list.more_error = None;
        from = list.next.clone();
        generation = list.generation;
        Changes::topic(TOPIC)
    });
    let Some(from) = from else {
        return;
    };
    handle.call(
        move |api| async move { api.inbox(filter.as_str(), Some(&from), PAGE).await },
        move |w, result| {
            let list = w.inbox.lists.entry(filter).or_default();
            if list.generation != generation {
                return Changes::default();
            }
            list.loading_more = false;
            match result {
                Ok(page) => {
                    for item in page.items {
                        if !list.items.iter().any(|i| i.message_id == item.message_id) {
                            list.items.push(item);
                        }
                    }
                    list.next = page.next_cursor;
                }
                Err(e) => list.more_error = Some(e.to_string()),
            }
            Changes::topic(TOPIC)
        },
    );
}

/// Dismiss an item, or restore a dismissed one. It moves at once; the lists
/// and the count are read again afterwards, whether or not the server agreed.
/// The handle answers with the server's result.
pub fn set_dismissed(
    handle: &Handle,
    message: &str,
    dismissed: bool,
) -> tokio::task::JoinHandle<Result<(), ApiError>> {
    handle.change(|w| {
        w.inbox.pending.insert(message.to_string());
        w.inbox.mark(message, dismissed);
        Changes::topic(TOPIC)
    });
    let back = handle.clone();
    let message = message.to_string();
    handle.request(move |api| async move {
        let result = api.set_inbox_dismissed(&message, dismissed).await;
        back.change(|w| {
            w.inbox.pending.remove(&message);
            w.inbox.changed()
        });
        result
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::tests_support::snapshot;

    fn durable(seq: i64, event: Durable) -> ServerEvent {
        ServerEvent::Durable { seq, event }
    }

    fn item(id: &str, dismissed: bool) -> InboxItem {
        InboxItem {
            conversation_id: "c-a".into(),
            created_at: "2026-11-21T10:00:00Z".into(),
            message_id: id.into(),
            network_id: "n1".into(),
            dismissed,
            status: InboxItemStatus::Available,
            unread: true,
            ..Default::default()
        }
    }

    fn loaded(items: Vec<InboxItem>) -> List {
        List {
            items,
            loaded: true,
            ..Default::default()
        }
    }

    fn settled() -> Workspace {
        let mut w = Workspace::from_snapshot(snapshot(1));
        w.inbox.unread_stale = false;
        w.inbox.unread = Some(1);
        w
    }

    #[test]
    fn inbox_added_reads_the_count_and_the_lists_again() {
        let mut w = settled();
        w.inbox.lists.insert(Filter::All, loaded(vec![]));
        let c = w.apply(
            &durable(
                2,
                Durable::InboxAdded(InboxEntry {
                    message_id: "m1".into(),
                    conversation_id: "c-a".into(),
                    ..Default::default()
                }),
            ),
            0,
        );
        assert!(c.touches(TOPIC));
        assert!(w.inbox.unread_stale);
        assert!(w.inbox.lists[&Filter::All].stale);
        assert!(List::wanted(w.inbox.list(Filter::All)));
    }

    #[test]
    fn inbox_updated_moves_the_item_between_views() {
        let mut w = settled();
        w.inbox.lists.insert(
            Filter::All,
            loaded(vec![item("m1", false), item("m2", false)]),
        );
        w.inbox.lists.insert(Filter::Dismissed, loaded(vec![]));
        let dismissed = |seq, dismissed| {
            durable(
                seq,
                Durable::InboxUpdated(InboxChange {
                    dismissed,
                    message_id: "m1".into(),
                }),
            )
        };
        let c = w.apply(&dismissed(2, true), 0);
        assert!(c.touches(TOPIC));
        let all: Vec<&str> = w.inbox.lists[&Filter::All]
            .items
            .iter()
            .map(|i| i.message_id.as_str())
            .collect();
        assert_eq!(all, ["m2"], "a dismissed item leaves All at once");
        assert!(w.inbox.unread_stale);

        w.inbox
            .lists
            .insert(Filter::Dismissed, loaded(vec![item("m1", true)]));
        w.apply(&dismissed(3, false), 0);
        assert!(w.inbox.lists[&Filter::Dismissed].items.is_empty());
    }

    #[test]
    fn reading_a_conversation_reads_the_count_again_but_new_messages_do_not() {
        let mut w = settled();
        let counts = |seq, unread| {
            durable(
                seq,
                Durable::ConversationCounts(ConversationCounts {
                    id: "c-b".into(),
                    unread_count: unread,
                    ..Default::default()
                }),
            )
        };
        // c-b starts with 2 unread in the snapshot.
        assert!(w.apply(&counts(2, 3), 0).topics.is_empty());
        assert!(!w.inbox.unread_stale);
        let c = w.apply(&counts(3, 0), 0);
        assert!(c.touches(TOPIC));
        assert!(w.inbox.unread_stale);

        // With nothing unread in the inbox, a read changes nothing there.
        w.inbox.unread_stale = false;
        w.inbox.unread = Some(0);
        w.apply(&counts(4, 5), 0);
        w.apply(&counts(5, 0), 0);
        assert!(!w.inbox.unread_stale);
    }

    #[test]
    fn a_deleted_message_in_the_inbox_reads_it_again() {
        let mut w = settled();
        w.inbox.unread = Some(0);
        w.inbox
            .lists
            .insert(Filter::All, loaded(vec![item("m1", false)]));
        let deleted = Message {
            id: "m1".into(),
            conversation_id: "c-a".into(),
            deleted: Some(true),
            ..Default::default()
        };
        let c = w.apply(&durable(2, Durable::MessageUpdated(deleted)), 0);
        assert!(c.touches(TOPIC));
        assert!(w.inbox.lists[&Filter::All].stale);
    }

    #[test]
    fn a_bootstrap_reads_the_count_again() {
        let mut w = settled();
        w.reset(snapshot(9));
        assert!(w.inbox.unread_stale);
    }

    #[test]
    fn filters_name_the_server_views() {
        let names: Vec<&str> = Filter::ALL.iter().map(|f| f.as_str()).collect();
        assert_eq!(
            names,
            [
                "all",
                "unread",
                "mentions",
                "replies",
                "direct",
                "dismissed"
            ]
        );
    }
}

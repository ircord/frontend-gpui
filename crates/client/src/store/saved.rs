//! Saved messages: `saved.added` and `saved.removed`.
//!
//! Owned by package FD2. The state lives in [`State`] (the workspace's
//! `saved` field): which messages are saved (for Save for later in the
//! message menu) and the page of saved messages the Saved page shows. Every
//! change is reported as `Changes::topic(TOPIC)`, or `Changes::topic_id(TOPIC,
//! message)` for one message's saved mark.
//!
//! The requests are free functions over a [`Handle`]. [`keep`] reads the
//! saved IDs once per client and again whenever they may be out of date.
//! Follows the web client's `app/queries/saved.ts`.

use std::collections::{HashMap, HashSet};

use ircord_contract::event::{Durable, ServerEvent};
use ircord_contract::*;

use super::{Changes, Workspace};
use crate::api::ApiError;
use crate::client::{Client, Handle};

/// The topic this sub-state reports in [`Changes`].
pub const TOPIC: &str = "saved";
/// Saved messages asked for per page (the server allows 1 to 100).
pub const PAGE: u32 = 30;

#[derive(Debug, Clone, Default)]
pub struct State {
    /// Saved message IDs, each with its conversation.
    pub ids: HashMap<String, String>,
    /// Whether [`State::ids`] has been read from the server.
    pub ids_loaded: bool,
    /// The IDs may be out of date and are read again.
    pub ids_stale: bool,
    pub ids_loading: bool,
    /// The Saved page's list.
    pub list: List,
    /// Messages being saved or removed right now.
    pub pending: HashSet<String>,
    /// [`keep`] has installed its watcher.
    kept: bool,
}

impl State {
    pub fn is_saved(&self, message: &str) -> bool {
        self.ids.contains_key(message)
    }
}

/// Saved messages as the Saved page lists them, for one filter text.
#[derive(Debug, Clone, Default)]
pub struct List {
    /// The filter text the items match.
    pub query: String,
    pub items: Vec<SavedItem>,
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

/// Called for every fresh event after the workspace's own handling.
pub(super) fn apply(w: &mut Workspace, event: &ServerEvent, _now_ms: i64) -> Changes {
    let ServerEvent::Durable { event, .. } = event else {
        return Changes::default();
    };
    let state = &mut w.saved;
    match event {
        Durable::SavedAdded(saved) if !saved.message_id.is_empty() => {
            state
                .ids
                .insert(saved.message_id.clone(), saved.conversation_id.clone());
            state.list.stale |= state.list.loaded;
            let mut changes = Changes::topic(TOPIC);
            changes.merge(Changes::topic_id(TOPIC, &saved.message_id));
            changes
        }
        Durable::SavedRemoved(removed) if !removed.message_id.is_empty() => {
            state.ids.remove(&removed.message_id);
            state
                .list
                .items
                .retain(|i| i.message_id != removed.message_id);
            let mut changes = Changes::topic(TOPIC);
            changes.merge(Changes::topic_id(TOPIC, &removed.message_id));
            changes
        }
        // A saved message was edited or deleted: the listed copy follows it.
        Durable::MessageUpdated(message) => {
            let mut patched = false;
            for item in state.list.items.iter_mut() {
                if item.message_id == message.id {
                    *item = patch(item.clone(), message);
                    patched = true;
                }
            }
            if patched {
                Changes::topic(TOPIC)
            } else {
                Changes::default()
            }
        }
        // The network's history went, and the bookmarks into it with it.
        Durable::NetworkRemoved(_) | Durable::ConversationRemoved(_) => {
            state.ids_stale = true;
            state.list.stale |= state.list.loaded;
            Changes::topic(TOPIC)
        }
        _ => Changes::default(),
    }
}

/// Called when a bootstrap replaces the workspace, before it is replaced.
pub(super) fn reset(w: &mut Workspace, _snapshot: &Snapshot) -> Changes {
    let state = &mut w.saved;
    state.ids_stale = true;
    state.list.stale |= state.list.loaded;
    Changes::topic(TOPIC)
}

/// A listed item with its source message's new content: a deleted one
/// becomes a stub.
pub fn patch(mut item: SavedItem, message: &Message) -> SavedItem {
    if item.message_id != message.id || item.status == SavedItemStatus::Unavailable {
        return item;
    }
    if message.deleted == Some(true) {
        item.status = SavedItemStatus::Deleted;
        item.message = Some(Message {
            id: message.id.clone(),
            conversation_id: message.conversation_id.clone(),
            sender_id: message.sender_id.clone(),
            created_at: message.created_at.clone(),
            kind: message.kind,
            deleted: Some(true),
            ..Default::default()
        });
    } else {
        item.status = SavedItemStatus::Available;
        item.message = Some(message.clone());
    }
    item
}

/// Read the saved IDs now, and again whenever they may be out of date. Call
/// it once per client; later calls do nothing.
pub fn keep(client: &Client) {
    if client.read().saved.kept {
        return;
    }
    let mut first = false;
    client.change(|w| {
        first = !w.saved.kept;
        w.saved.kept = true;
        Changes::default()
    });
    if !first {
        return;
    }
    client.watch(|handle, changes| {
        if changes.touches(TOPIC) {
            let wanted = {
                let w = handle.read();
                w.saved.ids_stale && !w.saved.ids_loading
            };
            if wanted {
                load_ids(handle);
            }
        }
    });
    load_ids(client.handle());
}

/// Read which messages are saved.
pub fn load_ids(handle: &Handle) {
    let mut started = false;
    handle.change(|w| {
        if w.saved.ids_loading {
            return Changes::default();
        }
        started = true;
        w.saved.ids_loading = true;
        w.saved.ids_stale = false;
        Changes::default()
    });
    if !started {
        return;
    }
    handle.call(
        |api| async move { api.saved_message_ids().await },
        |w, result| {
            let state = &mut w.saved;
            state.ids_loading = false;
            match result {
                Ok(refs) => {
                    // Saves still on their way keep their mark.
                    let pending: Vec<(String, String)> = state
                        .pending
                        .iter()
                        .filter_map(|id| state.ids.get(id).map(|c| (id.clone(), c.clone())))
                        .collect();
                    state.ids = refs
                        .items
                        .into_iter()
                        .map(|r| (r.message_id, r.conversation_id))
                        .collect();
                    state.ids.extend(pending);
                    state.ids_loaded = true;
                }
                Err(_) => state.ids_loaded = true,
            }
            Changes::topic(TOPIC)
        },
    );
}

/// Read the first page of saved messages matching `query`, replacing the list.
pub fn load(handle: &Handle, query: &str) {
    let mut generation = 0;
    let query = query.trim().to_string();
    handle.change(|w| {
        let list = &mut w.saved.list;
        list.generation += 1;
        generation = list.generation;
        if list.query != query {
            list.query = query.clone();
            list.items.clear();
            list.next = None;
            list.loaded = false;
        }
        list.loading = true;
        list.loading_more = false;
        list.stale = false;
        list.error = None;
        list.more_error = None;
        Changes::topic(TOPIC)
    });
    handle.call(
        move |api| async move { api.saved_messages(&query, None, PAGE).await },
        move |w, result| {
            let list = &mut w.saved.list;
            if list.generation != generation {
                return Changes::default();
            }
            list.loading = false;
            match result {
                Ok(page) => {
                    list.items = page.items;
                    list.next = page.next_cursor;
                    list.loaded = true;
                }
                Err(e) => list.error = Some(e.to_string()),
            }
            Changes::topic(TOPIC)
        },
    );
}

/// Read the next older page and add it to the list.
pub fn load_more(handle: &Handle) {
    let mut from = None;
    let mut generation = 0;
    let mut query = String::new();
    handle.change(|w| {
        let list = &mut w.saved.list;
        if list.loading || list.loading_more || list.next.is_none() {
            return Changes::default();
        }
        list.loading_more = true;
        list.more_error = None;
        from = list.next.clone();
        generation = list.generation;
        query = list.query.clone();
        Changes::topic(TOPIC)
    });
    let Some(from) = from else {
        return;
    };
    handle.call(
        move |api| async move { api.saved_messages(&query, Some(&from), PAGE).await },
        move |w, result| {
            let list = &mut w.saved.list;
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

/// Save `message` for later, or remove it from the saved messages. The mark
/// changes at once and goes back if the server refuses; the handle answers
/// with the server's result.
pub fn set_saved(
    handle: &Handle,
    conversation: &str,
    message: &str,
    saved: bool,
) -> tokio::task::JoinHandle<Result<(), ApiError>> {
    let mut removed = None;
    handle.change(|w| {
        let state = &mut w.saved;
        state.pending.insert(message.to_string());
        if saved {
            state
                .ids
                .insert(message.to_string(), conversation.to_string());
        } else {
            state.ids.remove(message);
            if let Some(at) = state
                .list
                .items
                .iter()
                .position(|i| i.message_id == message)
            {
                removed = Some((at, state.list.items.remove(at)));
            }
        }
        let mut changes = Changes::topic(TOPIC);
        changes.merge(Changes::topic_id(TOPIC, message));
        changes
    });
    let back = handle.clone();
    let (conversation, message) = (conversation.to_string(), message.to_string());
    handle.request(move |api| async move {
        let result = if saved {
            api.save_message(&conversation, &message).await.map(drop)
        } else {
            api.unsave_message(&message).await
        };
        back.change(|w| {
            let state = &mut w.saved;
            state.pending.remove(&message);
            match &result {
                Ok(()) if saved => state.list.stale |= state.list.loaded,
                Ok(()) => {}
                Err(_) if saved => {
                    state.ids.remove(&message);
                }
                Err(_) => {
                    state.ids.insert(message.clone(), conversation.clone());
                    if let Some((at, item)) = removed {
                        let at = at.min(state.list.items.len());
                        state.list.items.insert(at, item);
                    }
                }
            }
            let mut changes = Changes::topic(TOPIC);
            changes.merge(Changes::topic_id(TOPIC, &message));
            changes
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

    fn item(id: &str, text: &str) -> SavedItem {
        SavedItem {
            conversation_id: "c-a".into(),
            message_id: id.into(),
            network_id: "n1".into(),
            saved_at: "2026-11-21T10:00:00Z".into(),
            status: SavedItemStatus::Available,
            message: Some(Message {
                id: id.into(),
                conversation_id: "c-a".into(),
                text: text.into(),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    #[test]
    fn saved_added_marks_the_message_and_the_list_for_a_reread() {
        let mut w = Workspace::from_snapshot(snapshot(1));
        w.saved.list.loaded = true;
        let c = w.apply(
            &durable(
                2,
                Durable::SavedAdded(SavedMessage {
                    conversation_id: "c-a".into(),
                    message_id: "m1".into(),
                    network_id: "n1".into(),
                    saved_at: "2026-11-21T10:00:00Z".into(),
                }),
            ),
            0,
        );
        assert!(c.touches_id(TOPIC, "m1"));
        assert!(w.saved.is_saved("m1"));
        assert!(w.saved.list.stale);
    }

    #[test]
    fn saved_removed_drops_the_mark_and_the_listed_item() {
        let mut w = Workspace::from_snapshot(snapshot(1));
        w.saved.ids.insert("m1".into(), "c-a".into());
        w.saved.list.items = vec![item("m1", "one"), item("m2", "two")];
        let c = w.apply(
            &durable(
                2,
                Durable::SavedRemoved(SavedRemoval {
                    conversation_id: "c-a".into(),
                    message_id: "m1".into(),
                }),
            ),
            0,
        );
        assert!(c.touches(TOPIC));
        assert!(!w.saved.is_saved("m1"));
        let ids: Vec<&str> = w
            .saved
            .list
            .items
            .iter()
            .map(|i| i.message_id.as_str())
            .collect();
        assert_eq!(ids, ["m2"]);
    }

    #[test]
    fn an_edited_or_deleted_source_patches_the_listed_copy() {
        let mut w = Workspace::from_snapshot(snapshot(1));
        w.saved.list.items = vec![item("m1", "one")];
        let mut edited = item("m1", "edited").message.unwrap();
        w.apply(&durable(2, Durable::MessageUpdated(edited.clone())), 0);
        assert_eq!(
            w.saved.list.items[0].message.as_ref().unwrap().text,
            "edited"
        );
        edited.deleted = Some(true);
        let c = w.apply(&durable(3, Durable::MessageUpdated(edited)), 0);
        assert!(c.touches(TOPIC));
        let listed = &w.saved.list.items[0];
        assert_eq!(listed.status, SavedItemStatus::Deleted);
        assert_eq!(listed.message.as_ref().unwrap().text, "");
    }

    #[test]
    fn a_bootstrap_or_a_removed_network_reads_the_ids_again() {
        let mut w = Workspace::from_snapshot(snapshot(1));
        w.saved.ids_stale = false;
        w.apply(
            &durable(2, Durable::NetworkRemoved(Removal { id: "n1".into() })),
            0,
        );
        assert!(w.saved.ids_stale);
        w.saved.ids_stale = false;
        w.reset(snapshot(5));
        assert!(w.saved.ids_stale);
    }

    #[test]
    fn an_empty_event_changes_nothing() {
        let mut w = Workspace::from_snapshot(snapshot(1));
        let c = w.apply(&durable(2, Durable::SavedAdded(SavedMessage::default())), 0);
        assert!(c.is_empty());
    }
}

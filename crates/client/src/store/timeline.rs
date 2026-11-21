//! Timelines: the loaded messages of each open conversation and paging.
//!
//! Owned by package R1 (timeline reading), which adds paging both ways,
//! jumps, the unread line and gap rows here.

use std::collections::HashMap;

use ircord_contract::event::ServerEvent;
use ircord_contract::*;

use super::{Changes, Workspace, parse_time};

/// The newest messages kept per conversation while it follows the live end.
pub const TIMELINE_LIMIT: usize = 2000;
/// The topic this sub-state reports in [`Changes`] (timeline content itself
/// is reported in `Changes::timelines`).
pub const TOPIC: &str = "reading";

/// Reading state shared across timelines (jump targets, the unread line...).
#[derive(Debug, Clone, Default)]
pub struct State {}

/// Called for every fresh event after the workspace's own handling.
pub(super) fn apply(_w: &mut Workspace, _event: &ServerEvent, _now_ms: i64) -> Changes {
    Changes::default()
}

/// Called when a bootstrap replaces the workspace, before it is replaced.
pub(super) fn reset(_w: &mut Workspace, _snapshot: &Snapshot) -> Changes {
    Changes::default()
}

#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    /// `createdAt` in milliseconds, parsed once.
    pub at: i64,
    pub message: Message,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Timeline {
    pub entries: Vec<Entry>,
    /// The `nextCursor` for the page before the oldest entry. `None` once all
    /// history is loaded.
    pub older: Option<String>,
    /// The newest page has arrived (until then the entries are live events
    /// that arrived while it loaded).
    pub loaded: bool,
    pub loading_older: bool,
    pub sync_error: Option<String>,
}

fn entry(message: Message) -> Entry {
    Entry {
        at: parse_time(&message.created_at),
        message,
    }
}

fn order(a: &Entry, b: &Entry) -> std::cmp::Ordering {
    a.at.cmp(&b.at)
        .then(
            a.message
                .position
                .unwrap_or(0)
                .cmp(&b.message.position.unwrap_or(0)),
        )
        .then(a.message.id.cmp(&b.message.id))
}

impl Timeline {
    /// Insert or replace one message, keeping the order.
    pub(super) fn put(&mut self, message: Message) {
        let new = entry(message);
        if let Some(i) = self
            .entries
            .iter()
            .position(|e| e.message.id == new.message.id)
        {
            self.entries.remove(i);
        }
        let at = self
            .entries
            .partition_point(|e| order(e, &new) == std::cmp::Ordering::Less);
        self.entries.insert(at, new);
        self.trim();
    }

    pub(super) fn trim(&mut self) {
        if self.entries.len() > TIMELINE_LIMIT {
            let drop = self.entries.len() - TIMELINE_LIMIT;
            self.entries.drain(..drop);
            // Page on from the oldest message kept, as the web client does.
            let first = &self.entries[0].message.id;
            self.older = Some(format!("message:{}", urlencode(first)));
        }
    }

    pub(super) fn merge(&mut self, items: Vec<Message>) {
        let mut by_id: HashMap<String, Entry> = self
            .entries
            .drain(..)
            .map(|e| (e.message.id.clone(), e))
            .collect();
        for message in items {
            // A live copy that arrived first is newer than the page's copy.
            by_id
                .entry(message.id.clone())
                .or_insert_with(|| entry(message));
        }
        let mut entries: Vec<Entry> = by_id.into_values().collect();
        entries.sort_by(order);
        self.entries = entries;
    }

    pub fn find(&self, id: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.message.id == id)
    }
}

fn urlencode(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes())
        .collect::<String>()
        .replace('+', "%20")
}

impl Workspace {
    /// Start loading a conversation: live events arriving meanwhile are kept.
    pub fn begin_load(&mut self, conversation: &str) {
        self.timelines.entry(conversation.to_string()).or_default();
    }

    /// The newest page arrived.
    pub fn load_newest(&mut self, conversation: &str, page: MessagePage) -> Changes {
        let t = self.timelines.entry(conversation.to_string()).or_default();
        t.merge(page.items);
        t.older = page.next_cursor;
        t.loaded = true;
        t.sync_error = page.sync_error;
        t.trim();
        Changes::timeline(conversation)
    }

    /// Mark an older page as loading. Returns its cursor, or `None` when
    /// there is nothing older or a load is already running.
    pub fn begin_older(&mut self, conversation: &str) -> Option<String> {
        let t = self.timelines.get_mut(conversation)?;
        if !t.loaded || t.loading_older {
            return None;
        }
        let cursor = t.older.clone()?;
        t.loading_older = true;
        Some(cursor)
    }

    pub fn load_older(&mut self, conversation: &str, page: Option<MessagePage>) -> Changes {
        let Some(t) = self.timelines.get_mut(conversation) else {
            return Changes::default();
        };
        t.loading_older = false;
        if let Some(page) = page {
            t.merge(page.items);
            t.older = page.next_cursor;
            if page.sync_error.is_some() {
                t.sync_error = page.sync_error;
            }
        }
        Changes::timeline(conversation)
    }
}

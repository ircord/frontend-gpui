//! Timelines: the loaded messages of each open conversation, paging both
//! ways, windows of older history opened around one message, the "New
//! messages" line and the message a jump landed on.
//!
//! Owned by package R1 (timeline reading).
//!
//! A timeline normally follows the live end. Jumping to an old message
//! replaces it with a window around that message ([`Workspace::open_around`]);
//! the window is *detached* (`has_newer`) until paging newer reaches the
//! present or the newest page is read again, and live messages that are not
//! in it are left for that read.

use std::collections::HashMap;

use ircord_contract::event::{Durable, ServerEvent};
use ircord_contract::*;

use super::{Changes, Workspace, parse_time};

/// The newest messages kept per conversation while it follows the live end.
pub const TIMELINE_LIMIT: usize = 2000;
/// How many messages a window around one message asks for.
pub const AROUND_SIZE: u32 = 60;
/// How many messages a page towards the present asks for (it overlaps the
/// newest message held, which is its centre).
pub const NEWER_SIZE: u32 = 100;
/// The topic this sub-state reports in [`Changes`] (timeline content itself
/// is reported in `Changes::timelines`).
pub const TOPIC: &str = "reading";

/// Reading state shared across timelines.
#[derive(Debug, Clone, Default)]
pub struct State {
    /// Conversation → the unread count and manual unread boundary the server
    /// last reported. Opening a conversation clears its own counts at once,
    /// so the line is placed from this copy.
    reported: HashMap<String, (i64, Option<String>)>,
    /// Conversation → where the "New messages" line sits this visit.
    lines: HashMap<String, Line>,
    /// The message a jump landed on: `(conversation, message)`.
    pub focus: Option<(String, String)>,
    /// Bumped by every jump, so the view scrolls once per jump.
    pub focus_seq: u64,
    /// Conversation → the message the last `attract` pointed at.
    attracts: HashMap<String, Attract>,
}

/// What an `attract` event pointed at: the message to make sparkle, and
/// whoever asked (the stand-in when the message itself is not loaded).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attract {
    pub message_id: String,
    pub member_id: String,
}

impl State {
    /// The last `attract` seen in `conversation`. Nothing expires it: the
    /// view plays it when the event arrives and ignores it afterwards.
    pub fn attract(&self, conversation: &str) -> Option<&Attract> {
        self.attracts.get(conversation)
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Line {
    /// Before the `n`th newest message from someone else, once loaded.
    Count(usize),
    /// Before this message.
    At(String),
}

/// Called for every fresh event after the workspace's own handling.
pub(super) fn apply(w: &mut Workspace, event: &ServerEvent, _now_ms: i64) -> Changes {
    let event = match event {
        ServerEvent::Attract(a) => {
            w.reading.attracts.insert(
                a.conversation_id.clone(),
                Attract {
                    message_id: a.message_id.clone(),
                    member_id: a.member_id.clone(),
                },
            );
            return Changes::default();
        }
        ServerEvent::Durable { event, .. } => event,
        _ => return Changes::default(),
    };
    match event {
        Durable::ConversationCounts(c) => {
            w.reading
                .reported
                .insert(c.id.clone(), (c.unread_count, c.unread_from.clone()));
        }
        Durable::ConversationUpdated(c) => {
            w.reading
                .reported
                .insert(c.id.clone(), (c.unread_count, c.unread_from.clone()));
        }
        _ => {}
    }
    Changes::default()
}

/// Called when a bootstrap replaces the workspace, before it is replaced.
pub(super) fn reset(w: &mut Workspace, snapshot: &Snapshot) -> Changes {
    w.reading.reported = snapshot
        .conversations
        .iter()
        .map(|c| (c.id.clone(), (c.unread_count, c.unread_from.clone())))
        .collect();
    w.reading.lines.clear();
    w.reading.focus = None;
    w.reading.attracts.clear();
    Changes::topic(TOPIC)
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
    /// This is a window into older history: newer messages exist that are
    /// not loaded (live ones arriving meanwhile are left out).
    pub has_newer: bool,
    pub loading_newer: bool,
    /// The last page towards the present failed; paging waits for a retry.
    pub newer_failed: bool,
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
    /// Insert or replace one message, keeping the order. A window into
    /// older history only takes changes to messages it holds.
    pub(super) fn put(&mut self, message: Message) {
        let new = entry(message);
        let held = self
            .entries
            .iter()
            .position(|e| e.message.id == new.message.id);
        if let Some(i) = held {
            self.entries.remove(i);
        } else if self.has_newer {
            return;
        }
        let at = self
            .entries
            .partition_point(|e| order(e, &new) == std::cmp::Ordering::Less);
        self.entries.insert(at, new);
        self.trim();
    }

    /// Drop the oldest messages beyond the limit.
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

    /// Drop the newest messages beyond the limit: older history came in
    /// above, so the window no longer reaches the present.
    fn trim_newest(&mut self) {
        if self.entries.len() > TIMELINE_LIMIT {
            self.entries.truncate(TIMELINE_LIMIT);
            self.has_newer = true;
        }
    }

    pub fn find(&self, id: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.message.id == id)
    }

    /// The window around one message replaces what is held.
    fn replace(&mut self, page: MessagePage) {
        let mut entries: Vec<Entry> = page.items.into_iter().map(entry).collect();
        entries.sort_by(order);
        entries.dedup_by(|a, b| a.message.id == b.message.id);
        self.entries = entries;
        self.older = page.next_cursor;
        self.has_newer = page.has_newer.unwrap_or(false);
        self.loaded = true;
        self.loading_older = false;
        self.loading_newer = false;
        self.newer_failed = false;
        self.sync_error = page.sync_error;
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

    /// The newest page arrived. A window into older history keeps what it
    /// shows (a jump started while the conversation opened must not be
    /// undone); [`Workspace::return_to_present`] replaces it.
    pub fn load_newest(&mut self, conversation: &str, page: MessagePage) -> Changes {
        let t = self.timelines.entry(conversation.to_string()).or_default();
        if t.has_newer {
            t.loaded = true;
        } else {
            t.merge(page.items);
            t.older = page.next_cursor;
            t.loaded = true;
            t.sync_error = page.sync_error;
            t.newer_failed = false;
        }
        t.trim();
        Changes::timeline(conversation)
    }

    /// The newest page, read to leave a window into older history: it
    /// replaces whatever the timeline holds.
    pub fn return_to_present(&mut self, conversation: &str, page: MessagePage) -> Changes {
        let t = self.timelines.entry(conversation.to_string()).or_default();
        t.replace(page);
        t.has_newer = false;
        t.trim();
        let mut changes = self.clear_focus();
        changes.merge(Changes::timeline(conversation));
        changes
    }

    /// Mark an older page as loading. Returns its cursor, or `None` when
    /// there is nothing older or a page is already loading.
    pub fn begin_older(&mut self, conversation: &str) -> Option<String> {
        let t = self.timelines.get_mut(conversation)?;
        if !t.loaded || t.loading_older || t.loading_newer {
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
        match page {
            Some(page) => {
                t.merge(page.items);
                t.older = page.next_cursor;
                t.sync_error = page.sync_error;
                t.trim_newest();
            }
            // Shown as a row to retry from; paging older waits for it.
            None => {
                if t.sync_error.is_none() {
                    t.sync_error = Some(String::new());
                }
            }
        }
        Changes::timeline(conversation)
    }

    /// Mark a page towards the present as loading. Returns the newest
    /// message held, the centre of the window to ask for, or `None` when the
    /// timeline already reaches the present or a page is loading.
    pub fn begin_newer(&mut self, conversation: &str) -> Option<String> {
        let t = self.timelines.get_mut(conversation)?;
        if !t.loaded || !t.has_newer || t.loading_newer || t.loading_older || t.newer_failed {
            return None;
        }
        let boundary = t.entries.last()?.message.id.clone();
        t.loading_newer = true;
        Some(boundary)
    }

    /// A page towards the present arrived (around the newest message held),
    /// or failed. Messages up to the boundary are kept as held; the oldest
    /// are dropped beyond the limit.
    pub fn load_newer(
        &mut self,
        conversation: &str,
        boundary: &str,
        page: Result<MessagePage, String>,
    ) -> Changes {
        let Some(t) = self.timelines.get_mut(conversation) else {
            return Changes::default();
        };
        t.loading_newer = false;
        let page = match page {
            Ok(page) => page,
            Err(_) => {
                t.newer_failed = true;
                return Changes::timeline(conversation);
            }
        };
        let Some(edge) = t.entries.last().filter(|e| e.message.id == boundary) else {
            // The window changed meanwhile (a jump, a re-read): drop the page.
            return Changes::timeline(conversation);
        };
        let edge = edge.clone();
        let mut advanced = false;
        for message in page.items {
            let new = entry(message);
            if order(&new, &edge) == std::cmp::Ordering::Greater
                && t.find(&new.message.id).is_none()
            {
                t.entries.push(new);
                advanced = true;
            }
        }
        t.entries.sort_by(order);
        t.has_newer = page.has_newer.unwrap_or(false);
        if t.has_newer && !advanced {
            // A page that does not move on would be asked for forever.
            t.newer_failed = true;
        }
        if page.sync_error.is_some() {
            t.sync_error = page.sync_error;
        }
        t.trim();
        Changes::timeline(conversation)
    }

    /// Replace a conversation's timeline with the window of history around
    /// one message, and land on it.
    pub fn open_around(&mut self, conversation: &str, message: &str, page: MessagePage) -> Changes {
        let t = self.timelines.entry(conversation.to_string()).or_default();
        t.replace(page);
        self.focus(conversation, message)
    }

    /// Land on `message`: the view scrolls to it and marks it.
    pub fn focus(&mut self, conversation: &str, message: &str) -> Changes {
        self.reading.focus = Some((conversation.to_string(), message.to_string()));
        self.reading.focus_seq += 1;
        let mut changes = Changes::timeline(conversation);
        changes.merge(Changes::topic_id(TOPIC, conversation));
        changes
    }

    /// Forget the message a jump landed on (the person moved on).
    pub fn clear_focus(&mut self) -> Changes {
        match self.reading.focus.take() {
            Some((conversation, _)) => Changes::timeline(&conversation),
            None => Changes::default(),
        }
    }

    /// The message a jump landed on in `conversation`.
    pub fn focused(&self, conversation: &str) -> Option<&str> {
        self.reading
            .focus
            .as_ref()
            .filter(|(c, _)| c == conversation)
            .map(|(_, m)| m.as_str())
    }

    /// Place this visit's "New messages" line from what the server last
    /// reported unread, before opening the conversation clears it. Call
    /// once per visit, when the conversation is shown.
    pub fn begin_visit(&mut self, conversation: &str) {
        let reported = self.reading.reported.remove(conversation);
        let own = self
            .conversation(conversation)
            .map(|c| (c.unread_count, c.unread_from.clone()));
        let (count, from) = match (reported, own) {
            (Some(r), _) if r.0 > 0 || r.1.is_some() => r,
            (_, Some(o)) => o,
            (r, None) => r.unwrap_or((0, None)),
        };
        let line = match from {
            Some(id) => Some(Line::At(id)),
            None if count > 0 => Some(Line::Count(count as usize)),
            None => None,
        };
        match line {
            Some(line) => self.reading.lines.insert(conversation.to_string(), line),
            None => self.reading.lines.remove(conversation),
        };
    }

    /// The message the "New messages" line sits before, if it is loaded.
    /// A count is fixed to its message the first time it can be.
    pub fn unread_line(&mut self, conversation: &str) -> Option<String> {
        let line = self.reading.lines.get(conversation)?.clone();
        let t = self.timelines.get(conversation)?;
        match line {
            Line::At(id) => t.find(&id).map(|_| id),
            Line::Count(n) => {
                if !t.loaded {
                    return None;
                }
                let network = self
                    .conversation(conversation)
                    .map(|c| c.network_id.clone())
                    .unwrap_or_default();
                let others: Vec<&Entry> = t
                    .entries
                    .iter()
                    .filter(|e| {
                        e.message.kind != MessageKind::Event
                            && e.message.kind != MessageKind::System
                            && e.message.event.is_none()
                            && !self.is_self(&e.message.sender_id, &network)
                    })
                    .collect();
                let id = others
                    .get(others.len().saturating_sub(n))
                    .map(|e| e.message.id.clone())?;
                self.reading
                    .lines
                    .insert(conversation.to_string(), Line::At(id.clone()));
                Some(id)
            }
        }
    }

    /// Where the "New messages" line sits, without fixing a count.
    pub fn unread_line_at(&self, conversation: &str) -> Option<&str> {
        match self.reading.lines.get(conversation)? {
            Line::At(id) => Some(id),
            Line::Count(_) => None,
        }
    }

    /// Mark `message` and everything after it unread (the server is told
    /// separately): the line moves there and the conversation counts as
    /// unread again.
    pub fn mark_unread_from(&mut self, conversation: &str, message: &str) -> Changes {
        self.reading
            .lines
            .insert(conversation.to_string(), Line::At(message.to_string()));
        self.reading
            .reported
            .insert(conversation.to_string(), (0, Some(message.to_string())));
        if let Some(c) = self.conversations.iter_mut().find(|c| c.id == conversation) {
            c.unread_from = Some(message.to_string());
        }
        // Not a timeline change: the session marks a shown conversation read
        // on those, which would undo this at once.
        let mut changes = Changes::topic_id(TOPIC, conversation);
        changes.sidebar = true;
        changes
    }

    /// Hide the "New messages" line in `conversation`.
    pub fn clear_unread_line(&mut self, conversation: &str) -> Changes {
        if self.reading.lines.remove(conversation).is_some() {
            Changes::topic_id(TOPIC, conversation)
        } else {
            Changes::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::tests_support::snapshot;

    fn msg(id: &str, minute: u32, sender: &str) -> Message {
        Message {
            id: id.into(),
            conversation_id: "c-a".into(),
            created_at: format!("2026-11-20T10:{minute:02}:00Z"),
            kind: MessageKind::Message,
            sender_id: sender.into(),
            text: id.into(),
            ..Default::default()
        }
    }

    fn page(ids: std::ops::Range<u32>, older: bool, newer: bool) -> MessagePage {
        MessagePage {
            items: ids.map(|i| msg(&format!("m{i}"), i, "m-bob")).collect(),
            next_cursor: older.then(|| "older".to_string()),
            has_newer: Some(newer),
            ..Default::default()
        }
    }

    fn ids(w: &Workspace) -> Vec<String> {
        w.timelines["c-a"]
            .entries
            .iter()
            .map(|e| e.message.id.clone())
            .collect()
    }

    fn live(w: &mut Workspace, seq: i64, m: &Message) {
        let frame = serde_json::json!({"v": 1, "seq": seq, "type": "message.created",
            "data": serde_json::to_value(m).unwrap()});
        w.apply(&ServerEvent::decode(&frame.to_string()).unwrap(), 0);
    }

    #[test]
    fn an_attract_is_kept_for_its_conversation_and_a_new_one_replaces_it() {
        let mut w = Workspace::from_snapshot(snapshot(5));
        let attract = |w: &mut Workspace, message: &str| {
            let frame = serde_json::json!({"v": 1, "type": "attract",
                "data": {"conversationId": "c-a", "memberId": "m-bob", "messageId": message}});
            w.apply(&ServerEvent::decode(&frame.to_string()).unwrap(), 0)
        };
        assert!(w.reading.attract("c-a").is_none());
        let changes = attract(&mut w, "m1");
        assert!(changes.touches_id(crate::store::topic::ATTRACT, "c-a"));
        let first = w.reading.attract("c-a").unwrap().clone();
        assert_eq!(
            (first.message_id.as_str(), first.member_id.as_str()),
            ("m1", "m-bob")
        );
        attract(&mut w, "m2");
        let second = w.reading.attract("c-a").unwrap();
        assert_eq!(second.message_id, "m2");
        assert!(w.reading.attract("c-b").is_none());
    }

    #[test]
    fn a_window_around_an_old_message_pages_back_to_the_present() {
        let mut w = Workspace::from_snapshot(snapshot(5));
        w.load_newest("c-a", page(40..50, true, false));
        w.open_around("c-a", "m10", page(5..15, true, true));
        assert_eq!(w.focused("c-a"), Some("m10"));
        let t = &w.timelines["c-a"];
        assert!(t.has_newer && t.loaded);
        assert_eq!(ids(&w).first().unwrap(), "m5");

        // A live message is not part of the window.
        live(&mut w, 6, &msg("m59", 59, "m-bob"));
        assert!(!ids(&w).contains(&"m59".to_string()));

        // Paging newer asks around the newest held, keeps what is held.
        let boundary = w.begin_newer("c-a").unwrap();
        assert_eq!(boundary, "m14");
        assert_eq!(w.begin_newer("c-a"), None, "one page at a time");
        w.load_newer("c-a", &boundary, Ok(page(10..30, true, true)));
        assert_eq!(ids(&w).len(), 25);
        assert!(w.timelines["c-a"].has_newer);

        // The last page reaches the present.
        let boundary = w.begin_newer("c-a").unwrap();
        w.load_newer("c-a", &boundary, Ok(page(25..50, true, false)));
        let t = &w.timelines["c-a"];
        assert!(!t.has_newer);
        assert_eq!(ids(&w).last().unwrap(), "m49");
        assert_eq!(w.begin_newer("c-a"), None);

        // Live messages join again once it follows the present.
        live(&mut w, 7, &msg("m59", 59, "m-bob"));
        assert_eq!(ids(&w).last().unwrap(), "m59");
    }

    #[test]
    fn only_returning_to_the_present_replaces_a_window() {
        let mut w = Workspace::from_snapshot(snapshot(5));
        w.open_around("c-a", "m10", page(5..15, true, true));
        // The newest page of opening the conversation arrives late.
        w.load_newest("c-a", page(40..50, true, false));
        assert_eq!(ids(&w).first().unwrap(), "m5", "the window stays");
        assert!(w.timelines["c-a"].has_newer);
        w.return_to_present("c-a", page(40..50, true, false));
        assert_eq!(ids(&w).first().unwrap(), "m40", "the window is replaced");
        assert_eq!(w.focused("c-a"), None);
        assert!(!w.timelines["c-a"].has_newer);
    }

    #[test]
    fn a_failed_or_stuck_newer_page_waits_for_a_retry() {
        let mut w = Workspace::from_snapshot(snapshot(5));
        w.open_around("c-a", "m10", page(5..15, true, true));
        let b = w.begin_newer("c-a").unwrap();
        w.load_newer("c-a", &b, Err("offline".into()));
        assert!(w.timelines["c-a"].newer_failed);
        assert_eq!(w.begin_newer("c-a"), None);
        w.timelines.get_mut("c-a").unwrap().newer_failed = false;
        let b = w.begin_newer("c-a").unwrap();
        // The same messages again: no progress.
        w.load_newer("c-a", &b, Ok(page(5..15, true, true)));
        assert!(w.timelines["c-a"].newer_failed);
    }

    #[test]
    fn older_pages_beyond_the_limit_drop_the_newest() {
        let mut w = Workspace::from_snapshot(snapshot(5));
        let n = TIMELINE_LIMIT as u32;
        let items = |r: std::ops::Range<u32>| -> Vec<Message> {
            r.map(|i| Message {
                created_at: format!("2026-11-20T10:00:{:02}.{:06}Z", i / 100_000, i % 100_000),
                ..msg(&format!("m{i:06}"), 0, "m-bob")
            })
            .collect()
        };
        w.load_newest(
            "c-a",
            MessagePage {
                items: items(100..100 + n),
                next_cursor: Some("older".into()),
                ..Default::default()
            },
        );
        let cursor = w.begin_older("c-a").unwrap();
        assert_eq!(cursor, "older");
        w.load_older(
            "c-a",
            Some(MessagePage {
                items: items(0..100),
                next_cursor: None,
                ..Default::default()
            }),
        );
        let t = &w.timelines["c-a"];
        assert_eq!(t.entries.len(), TIMELINE_LIMIT);
        assert_eq!(t.entries[0].message.id, "m000000");
        assert!(t.has_newer, "the window no longer reaches the present");
    }

    #[test]
    fn a_failed_older_page_leaves_a_row_to_retry_from() {
        let mut w = Workspace::from_snapshot(snapshot(5));
        w.load_newest("c-a", page(40..50, true, false));
        w.begin_older("c-a").unwrap();
        w.load_older("c-a", None);
        assert!(w.timelines["c-a"].sync_error.is_some());
        assert_eq!(w.timelines["c-a"].older.as_deref(), Some("older"));
    }

    fn in_b(mut p: MessagePage) -> MessagePage {
        for m in &mut p.items {
            m.conversation_id = "c-b".into();
        }
        p
    }

    #[test]
    fn the_unread_line_survives_opening_and_counts_from_the_end() {
        let mut w = Workspace::from_snapshot(snapshot(5));
        // c-b has 2 unread in the snapshot; opening clears the counts.
        w.mark_read("c-b");
        w.begin_visit("c-b");
        assert_eq!(w.unread_line("c-b"), None, "nothing loaded yet");
        let mut p = in_b(page(0..6, false, false));
        p.items[4].sender_id = "u-me".into();
        w.load_newest("c-b", p);
        // Two newest from others: m3 and m5 (m4 is yours).
        assert_eq!(w.unread_line("c-b").as_deref(), Some("m3"));
        // Fixed once placed: a new arrival does not move it.
        let mut later = msg("m7", 7, "m-bob");
        later.conversation_id = "c-b".into();
        live(&mut w, 6, &later);
        assert_eq!(w.unread_line("c-b").as_deref(), Some("m3"));
        // The next visit has nothing unread.
        w.begin_visit("c-b");
        assert_eq!(w.unread_line("c-b"), None);
    }

    #[test]
    fn marking_unread_moves_the_line_and_counts_on_the_next_visit() {
        let mut w = Workspace::from_snapshot(snapshot(5));
        w.load_newest("c-a", page(0..6, false, false));
        w.mark_unread_from("c-a", "m2");
        assert_eq!(w.unread_line("c-a").as_deref(), Some("m2"));
        assert_eq!(
            w.conversation("c-a").unwrap().unread_from.as_deref(),
            Some("m2")
        );
        w.mark_read("c-a");
        w.begin_visit("c-a");
        assert_eq!(w.unread_line("c-a").as_deref(), Some("m2"));
    }

    #[test]
    fn the_server_reporting_counts_places_the_line() {
        let mut w = Workspace::from_snapshot(snapshot(5));
        let frame = r#"{"v":1,"seq":6,"type":"conversation.counts","data":{"id":"c-a","unreadCount":1,"mentionCount":0}}"#;
        w.apply(&ServerEvent::decode(frame).unwrap(), 0);
        w.mark_read("c-a");
        w.begin_visit("c-a");
        w.load_newest("c-a", page(0..3, false, false));
        assert_eq!(w.unread_line("c-a").as_deref(), Some("m2"));
    }
}

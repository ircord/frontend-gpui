//! The workspace: everything the app shows, changed only through [`Workspace::apply`]
//! (socket events) and a few explicit methods (pages loaded, sends started).
//!
//! The rules follow the web client's chat store (`app/stores/chat.ts`):
//! durable events advance a cursor one at a time; a gap means the client
//! missed something and must bootstrap again; a replayed event at or below
//! the cursor is ignored, except a send acknowledgement.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use ircord_contract::event::{Durable, ServerEvent};
use ircord_contract::*;

/// The newest messages kept per conversation while it follows the live end.
pub const TIMELINE_LIMIT: usize = 2000;
/// How long a typing indicator lasts without a renewal, in milliseconds.
pub const TYPING_TTL_MS: i64 = 6000;
/// The sender ID the server uses for the signed-in user.
pub const SELF_ID: &str = "you";

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SendState {
    Sending,
    Failed(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct PendingSend {
    pub client_id: String,
    pub conversation_id: String,
    pub text: String,
    pub action: bool,
    pub state: SendState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Connection {
    /// Reading the workspace or opening the socket.
    Connecting,
    /// Catching up on events missed while away.
    Replaying,
    Live,
    /// Disconnected; the client tries again after `retry_ms`.
    Offline {
        reason: String,
        retry_ms: u64,
    },
}

/// What an operation touched, so the app redraws only those views.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Changes {
    /// Network rail, conversation list or counts.
    pub sidebar: bool,
    pub members: bool,
    pub timelines: BTreeSet<String>,
    pub typing: BTreeSet<String>,
    /// Conversations whose history changed upstream and should be re-read if open.
    pub refresh: BTreeSet<String>,
    pub notices: Vec<String>,
    /// Missed events: throw the state away and bootstrap again.
    pub resync: bool,
    /// The whole workspace was replaced.
    pub reset: bool,
}

impl Changes {
    pub fn is_empty(&self) -> bool {
        *self == Changes::default()
    }

    pub fn merge(&mut self, other: Changes) {
        self.sidebar |= other.sidebar;
        self.members |= other.members;
        self.timelines.extend(other.timelines);
        self.typing.extend(other.typing);
        self.refresh.extend(other.refresh);
        self.notices.extend(other.notices);
        self.resync |= other.resync;
        self.reset |= other.reset;
    }

    fn timeline(conversation: &str) -> Self {
        Changes {
            timelines: BTreeSet::from([conversation.to_string()]),
            ..Default::default()
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Workspace {
    pub user: Member,
    pub networks: Vec<Network>,
    pub conversations: Vec<Conversation>,
    pub members: HashMap<String, Member>,
    pub invitations: Vec<Invitation>,
    pub capabilities: Capabilities,
    pub timelines: HashMap<String, Timeline>,
    pub outbox: Vec<PendingSend>,
    /// conversation → member → expiry (ms since epoch).
    pub typing: HashMap<String, BTreeMap<String, i64>>,
    pub cursor: i64,
    pub connection: Option<Connection>,
    pub demo: bool,
}

pub fn parse_time(text: &str) -> i64 {
    chrono::DateTime::parse_from_rfc3339(text)
        .map(|t| t.timestamp_millis())
        .unwrap_or(0)
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
    fn put(&mut self, message: Message) {
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

    fn trim(&mut self) {
        if self.entries.len() > TIMELINE_LIMIT {
            let drop = self.entries.len() - TIMELINE_LIMIT;
            self.entries.drain(..drop);
            // Page on from the oldest message kept, as the web client does.
            let first = &self.entries[0].message.id;
            self.older = Some(format!("message:{}", urlencode(first)));
        }
    }

    fn merge(&mut self, items: Vec<Message>) {
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
    pub fn from_snapshot(snapshot: Snapshot) -> Self {
        let mut w = Workspace::default();
        w.reset(snapshot);
        w
    }

    /// Replace everything the server owns with a fresh bootstrap. Pending
    /// sends survive; timelines are dropped and load again when opened.
    pub fn reset(&mut self, snapshot: Snapshot) -> Changes {
        self.user = snapshot.user;
        self.networks = snapshot.networks;
        self.conversations = snapshot.conversations;
        self.members = snapshot
            .members
            .into_iter()
            .map(|m| (m.id.clone(), m))
            .collect();
        self.invitations = snapshot.invitations;
        self.capabilities = snapshot.capabilities;
        self.cursor = snapshot.cursor;
        self.demo = snapshot.demo;
        self.timelines.clear();
        self.typing.clear();
        Changes {
            reset: true,
            sidebar: true,
            members: true,
            ..Default::default()
        }
    }

    pub fn conversation(&self, id: &str) -> Option<&Conversation> {
        self.conversations.iter().find(|c| c.id == id)
    }

    pub fn network(&self, id: &str) -> Option<&Network> {
        self.networks.iter().find(|n| n.id == id)
    }

    /// Whether a sender ID is the signed-in user: `you`, the account's member
    /// ID, or its profile ID on that network.
    pub fn is_self(&self, id: &str, network_id: &str) -> bool {
        id == SELF_ID
            || (!self.user.id.is_empty() && id == self.user.id)
            || self
                .network(network_id)
                .and_then(|n| n.profile.as_ref())
                .is_some_and(|p| p.id == id)
    }

    /// The member behind a sender ID; `you` is your own profile on that network.
    pub fn member(&self, id: &str, network_id: &str) -> Option<&Member> {
        if id == SELF_ID {
            return self
                .network(network_id)
                .and_then(|n| n.profile.as_ref())
                .or(Some(&self.user));
        }
        self.members.get(id)
    }

    /// The name to show for a sender.
    pub fn display_name(&self, id: &str, network_id: &str) -> String {
        if self.is_self(id, network_id)
            && let Some(n) = self.network(network_id)
            && !n.nick.is_empty()
        {
            return n.nick.clone();
        }
        match self.member(id, network_id) {
            Some(m) if !m.display_name.is_empty() => m.display_name.clone(),
            Some(m) if !m.nick.is_empty() => m.nick.clone(),
            _ => id.to_string(),
        }
    }

    /// A network's conversations in sidebar order: favourites, then channels,
    /// then direct messages, each by name.
    pub fn sidebar(&self, network_id: &str) -> Vec<&Conversation> {
        let mut list: Vec<&Conversation> = self
            .conversations
            .iter()
            .filter(|c| c.network_id == network_id)
            .collect();
        list.sort_by_key(|c| {
            (
                !c.favorite,
                c.kind != ConversationKind::Channel,
                c.name.to_lowercase(),
            )
        });
        list
    }

    /// Unread and mention totals for a network's rail badge.
    pub fn network_counts(&self, network_id: &str) -> (i64, i64) {
        self.conversations
            .iter()
            .filter(|c| c.network_id == network_id)
            .fold((0, 0), |(u, m), c| {
                (u + c.unread_count, m + c.mention_count)
            })
    }

    pub fn typing_in(&self, conversation: &str, now_ms: i64) -> Vec<String> {
        let Some(map) = self.typing.get(conversation) else {
            return Vec::new();
        };
        let network = self
            .conversation(conversation)
            .map(|c| c.network_id.clone())
            .unwrap_or_default();
        map.iter()
            .filter(|(id, until)| **until > now_ms && !self.is_self(id, &network))
            .map(|(id, _)| self.display_name(id, &network))
            .collect()
    }

    // ---- Socket events -------------------------------------------------

    /// Apply one frame from the event socket. `now_ms` drives typing expiry.
    pub fn apply(&mut self, event: &ServerEvent, now_ms: i64) -> Changes {
        match event {
            ServerEvent::Typing(t) => self.set_typing(t, now_ms),
            ServerEvent::Resync(_) => Changes {
                resync: true,
                ..Default::default()
            },
            ServerEvent::CommandError(e) => {
                if let Some(id) = &e.client_id
                    && let Some(changes) = self.fail_send(id, &e.message)
                {
                    return changes;
                }
                Changes {
                    notices: vec![e.message.clone()],
                    ..Default::default()
                }
            }
            ServerEvent::Durable { seq, event } => self.apply_durable(*seq, event, now_ms),
            ServerEvent::Unknown { seq: Some(seq), .. } => {
                // An event this client cannot read still counts toward the cursor.
                self.advance(*seq).unwrap_or_default()
            }
            _ => Changes::default(),
        }
    }

    /// `Some(changes)` when the event must be skipped (already seen, or a gap).
    fn advance(&mut self, seq: i64) -> Option<Changes> {
        if seq <= self.cursor {
            return Some(Changes::default());
        }
        if seq > self.cursor + 1 {
            return Some(Changes {
                resync: true,
                ..Default::default()
            });
        }
        self.cursor = seq;
        None
    }

    fn apply_durable(&mut self, seq: i64, event: &Durable, now_ms: i64) -> Changes {
        if seq <= self.cursor {
            // A retried command is acknowledged again with its original number.
            if let Durable::MessageCreated(m) = event
                && m.client_id.is_some()
            {
                return self.put_message(m.clone(), now_ms);
            }
            return Changes::default();
        }
        if let Some(skip) = self.advance(seq) {
            return skip;
        }
        match event {
            Durable::MessageCreated(m) | Durable::MessageUpdated(m) => {
                self.put_message(m.clone(), now_ms)
            }
            Durable::ConversationUpdated(c) => {
                match self.conversations.iter_mut().find(|x| x.id == c.id) {
                    Some(existing) => *existing = c.clone(),
                    None => self.conversations.push(c.clone()),
                }
                Changes {
                    sidebar: true,
                    members: true,
                    ..Changes::timeline(&c.id)
                }
            }
            Durable::ConversationCounts(counts) => {
                if let Some(c) = self.conversations.iter_mut().find(|x| x.id == counts.id) {
                    c.unread_count = counts.unread_count;
                    c.mention_count = counts.mention_count;
                    c.unread_from = counts.unread_from.clone();
                }
                Changes {
                    sidebar: true,
                    ..Default::default()
                }
            }
            Durable::ConversationRemoved(r) => {
                self.conversations.retain(|c| c.id != r.id);
                self.timelines.remove(&r.id);
                Changes {
                    sidebar: true,
                    ..Changes::timeline(&r.id)
                }
            }
            Durable::NetworkUpdated(n) => {
                match self.networks.iter_mut().find(|x| x.id == n.id) {
                    Some(existing) => *existing = n.clone(),
                    None => self.networks.push(n.clone()),
                }
                Changes {
                    sidebar: true,
                    ..Default::default()
                }
            }
            Durable::NetworkRemoved(r) => {
                self.networks.retain(|n| n.id != r.id);
                let gone: Vec<String> = self
                    .conversations
                    .iter()
                    .filter(|c| c.network_id == r.id)
                    .map(|c| c.id.clone())
                    .collect();
                self.conversations.retain(|c| c.network_id != r.id);
                for id in &gone {
                    self.timelines.remove(id);
                }
                Changes {
                    sidebar: true,
                    timelines: gone.into_iter().collect(),
                    ..Default::default()
                }
            }
            Durable::MemberUpdated(m) => {
                self.members.insert(m.id.clone(), m.clone());
                Changes {
                    members: true,
                    ..Default::default()
                }
            }
            Durable::ProfileUpdated(m) => {
                if m.id == self.user.id || m.id == SELF_ID {
                    self.user = m.clone();
                }
                Changes {
                    members: true,
                    sidebar: true,
                    ..Default::default()
                }
            }
            Durable::InvitationReceived(i) => {
                self.invitations.retain(|x| x.id != i.id);
                self.invitations.push(i.clone());
                Changes {
                    sidebar: true,
                    ..Default::default()
                }
            }
            Durable::InvitationRemoved(r) => {
                self.invitations.retain(|x| x.id != r.id);
                Changes {
                    sidebar: true,
                    ..Default::default()
                }
            }
            Durable::HistorySynced(h) => Changes {
                refresh: BTreeSet::from([h.conversation_id.clone()]),
                ..Default::default()
            },
            Durable::HistoryGap(h) | Durable::HistoryFailed(h) => Changes {
                notices: vec![
                    h.reason
                        .clone()
                        .or_else(|| h.message.clone())
                        .unwrap_or_else(|| {
                            "Some history for this conversation is unavailable.".into()
                        }),
                ],
                ..Default::default()
            },
            Durable::ConnectionUpdated(_) | Durable::Other(_) => Changes::default(),
        }
    }

    fn put_message(&mut self, message: Message, _now_ms: i64) -> Changes {
        let id = message.conversation_id.clone();
        let mut changes = Changes::timeline(&id);
        if let Some(client_id) = &message.client_id {
            let before = self.outbox.len();
            self.outbox.retain(|p| &p.client_id != client_id);
            if self.outbox.len() != before {
                changes.timelines.insert(id.clone());
            }
        }
        if let Some(t) = self.typing.get_mut(&id)
            && t.remove(&message.sender_id).is_some()
        {
            changes.typing.insert(id.clone());
        }
        // Conversations not open load fresh history when opened.
        if let Some(timeline) = self.timelines.get_mut(&id) {
            timeline.put(message);
        }
        changes
    }

    fn set_typing(&mut self, t: &TypingData, now_ms: i64) -> Changes {
        let map = self.typing.entry(t.conversation_id.clone()).or_default();
        match t.state {
            TypingDataState::Active => {
                map.insert(t.member_id.clone(), now_ms + TYPING_TTL_MS);
            }
            _ => {
                map.remove(&t.member_id);
            }
        }
        Changes {
            typing: BTreeSet::from([t.conversation_id.clone()]),
            ..Default::default()
        }
    }

    // ---- Explicit operations --------------------------------------------

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

    pub fn queue_send(&mut self, pending: PendingSend) -> Changes {
        let id = pending.conversation_id.clone();
        self.outbox.retain(|p| p.client_id != pending.client_id);
        self.outbox.push(pending);
        Changes::timeline(&id)
    }

    /// The server answered a send.
    pub fn send_answered(&mut self, response: &CommandResponse) -> Changes {
        use CommandResponseState as S;
        match response.state {
            S::Failed | S::Discarded => self
                .fail_send(
                    &response.client_id,
                    response
                        .error
                        .as_deref()
                        .unwrap_or("The message was not sent."),
                )
                .unwrap_or_default(),
            _ => {
                let mut changes = Changes::default();
                if let Some(m) = &response.message
                    && (m.confirmed == Some(true)
                        || (m.unconfirmed != Some(true) && m.upstream_id.is_some()))
                {
                    changes.merge(self.put_message(m.clone(), 0));
                }
                changes
            }
        }
    }

    pub fn fail_send(&mut self, client_id: &str, reason: &str) -> Option<Changes> {
        let p = self.outbox.iter_mut().find(|p| p.client_id == client_id)?;
        p.state = SendState::Failed(reason.to_string());
        Some(Changes::timeline(&p.conversation_id.clone()))
    }

    pub fn retry_send(&mut self, client_id: &str) -> Option<PendingSend> {
        let p = self.outbox.iter_mut().find(|p| p.client_id == client_id)?;
        p.state = SendState::Sending;
        Some(p.clone())
    }

    pub fn pending_in(&self, conversation: &str) -> Vec<&PendingSend> {
        self.outbox
            .iter()
            .filter(|p| p.conversation_id == conversation)
            .collect()
    }

    /// Clear the counts locally once the server has been told.
    pub fn mark_read(&mut self, conversation: &str) -> Changes {
        if let Some(c) = self.conversations.iter_mut().find(|c| c.id == conversation) {
            c.unread_count = 0;
            c.mention_count = 0;
            c.unread_from = None;
        }
        Changes {
            sidebar: true,
            ..Default::default()
        }
    }

    pub fn expire_typing(&mut self, now_ms: i64) -> Changes {
        let mut changes = Changes::default();
        for (conversation, map) in &mut self.typing {
            let before = map.len();
            map.retain(|_, until| *until > now_ms);
            if map.len() != before {
                changes.typing.insert(conversation.clone());
            }
        }
        changes
    }
}

#[cfg(test)]
pub(crate) mod tests_support {
    use super::*;

    pub fn snapshot(cursor: i64) -> Snapshot {
        serde_json::from_value(serde_json::json!({
            "version": 1, "cursor": cursor, "demo": false,
            "user": {"id": "u-me", "account": "me", "nick": "me", "displayName": "Me",
                     "bio": "", "color": "", "role": "member", "status": "online"},
            "capabilities": {},
            "invitations": [],
            "members": [{"id": "m-bob", "account": "bob", "nick": "bob", "displayName": "",
                         "bio": "", "color": "", "role": "member", "status": "away"}],
            "networks": [{"id": "n1", "name": "Home", "nick": "me", "host": "irc", "port": 6667,
                          "tls": false, "status": "connected", "initials": "H", "color": "",
                          "auth": {"method": "none", "passwordSet": false},
                          "autoJoin": [], "capabilities": [], "isupport": {}, "perform": [],
                          "features": {"typing": true, "replies": true}}],
            "conversations": [
                {"id": "c-dm", "networkId": "n1", "kind": "direct", "name": "bob", "favorite": false,
                 "joined": true, "memberIds": ["m-bob"], "memberModes": {}, "modes": "", "topic": "",
                 "unreadCount": 0, "mentionCount": 0},
                {"id": "c-b", "networkId": "n1", "kind": "channel", "name": "beta", "favorite": false,
                 "joined": true, "memberIds": [], "memberModes": {}, "modes": "", "topic": "",
                 "unreadCount": 2, "mentionCount": 1},
                {"id": "c-a", "networkId": "n1", "kind": "channel", "name": "alpha", "favorite": false,
                 "joined": true, "memberIds": [], "memberModes": {}, "modes": "", "topic": "",
                 "unreadCount": 0, "mentionCount": 0},
                {"id": "c-z", "networkId": "n1", "kind": "channel", "name": "zeta", "favorite": true,
                 "joined": true, "memberIds": [], "memberModes": {}, "modes": "", "topic": "",
                 "unreadCount": 0, "mentionCount": 0}
            ]
        }))
        .unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::tests_support::snapshot;
    use super::*;

    fn message(id: &str, at: &str, text: &str) -> Message {
        Message {
            id: id.into(),
            conversation_id: "c-a".into(),
            created_at: at.into(),
            kind: MessageKind::Message,
            sender_id: "m-bob".into(),
            text: text.into(),
            ..Default::default()
        }
    }

    fn created(seq: i64, m: Message) -> ServerEvent {
        ServerEvent::Durable {
            seq,
            event: Durable::MessageCreated(m),
        }
    }

    #[test]
    fn sidebar_puts_favourites_first_then_channels_then_direct_messages() {
        let w = Workspace::from_snapshot(snapshot(1));
        let names: Vec<&str> = w.sidebar("n1").iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["zeta", "alpha", "beta", "bob"]);
        assert_eq!(w.network_counts("n1"), (2, 1));
    }

    #[test]
    fn events_advance_the_cursor_one_at_a_time() {
        let mut w = Workspace::from_snapshot(snapshot(10));
        w.begin_load("c-a");
        let c = w.apply(&created(11, message("1", "2026-11-21T10:00:00Z", "hi")), 0);
        assert!(c.timelines.contains("c-a"));
        assert_eq!(w.cursor, 11);
        assert_eq!(w.timelines["c-a"].entries.len(), 1);
        // A replay of the same event is ignored.
        w.apply(
            &created(11, message("1", "2026-11-21T10:00:00Z", "changed")),
            0,
        );
        assert_eq!(w.timelines["c-a"].entries[0].message.text, "hi");
        // A gap asks for a resync and leaves the cursor alone.
        let c = w.apply(&created(13, message("2", "2026-11-21T10:01:00Z", "x")), 0);
        assert!(c.resync);
        assert_eq!(w.cursor, 11);
    }

    #[test]
    fn unknown_durable_events_still_count() {
        let mut w = Workspace::from_snapshot(snapshot(4));
        let e = ServerEvent::decode(r#"{"v":1,"seq":5,"type":"saved.added","data":{}}"#).unwrap();
        assert!(w.apply(&e, 0).is_empty());
        assert_eq!(w.cursor, 5);
        let e = ServerEvent::decode(r#"{"v":1,"seq":6,"type":"message.created","data":"garbage"}"#)
            .unwrap();
        w.apply(&e, 0);
        assert_eq!(w.cursor, 6);
    }

    #[test]
    fn messages_stay_ordered_and_edits_replace() {
        let mut w = Workspace::from_snapshot(snapshot(0));
        w.begin_load("c-a");
        w.apply(
            &created(1, message("b", "2026-11-21T10:05:00Z", "second")),
            0,
        );
        w.apply(
            &created(2, message("a", "2026-11-21T10:00:00Z", "first")),
            0,
        );
        let mut edited = message("b", "2026-11-21T10:05:00Z", "second, edited");
        edited.edited_at = Some("2026-11-21T10:06:00Z".into());
        w.apply(
            &ServerEvent::Durable {
                seq: 3,
                event: Durable::MessageUpdated(edited),
            },
            0,
        );
        let texts: Vec<&str> = w.timelines["c-a"]
            .entries
            .iter()
            .map(|e| e.message.text.as_str())
            .collect();
        assert_eq!(texts, ["first", "second, edited"]);
    }

    #[test]
    fn live_events_during_a_load_survive_the_page() {
        let mut w = Workspace::from_snapshot(snapshot(0));
        w.begin_load("c-a");
        w.apply(
            &created(1, message("live", "2026-11-21T10:05:00Z", "live")),
            0,
        );
        let page = MessagePage {
            items: vec![message("old", "2026-11-21T09:00:00Z", "old")],
            next_cursor: Some("cursor-1".into()),
            ..Default::default()
        };
        w.load_newest("c-a", page);
        let t = &w.timelines["c-a"];
        assert!(t.loaded);
        assert_eq!(t.entries.len(), 2);
        assert_eq!(t.older.as_deref(), Some("cursor-1"));
        // Older pages prepend, and only one loads at a time.
        assert_eq!(w.begin_older("c-a").as_deref(), Some("cursor-1"));
        assert_eq!(w.begin_older("c-a"), None);
        w.load_older(
            "c-a",
            Some(MessagePage {
                items: vec![message("older", "2026-11-20T09:00:00Z", "older")],
                next_cursor: None,
                ..Default::default()
            }),
        );
        let t = &w.timelines["c-a"];
        assert_eq!(t.entries[0].message.id, "older");
        assert_eq!(t.older, None);
        assert_eq!(w.begin_older("c-a"), None);
    }

    #[test]
    fn conversations_not_open_do_not_keep_messages() {
        let mut w = Workspace::from_snapshot(snapshot(0));
        w.apply(&created(1, message("1", "2026-11-21T10:00:00Z", "hi")), 0);
        assert!(!w.timelines.contains_key("c-a"));
    }

    #[test]
    fn the_echo_of_a_send_settles_it_and_failures_stay_for_retry() {
        let mut w = Workspace::from_snapshot(snapshot(0));
        w.begin_load("c-a");
        for id in ["k1", "k2"] {
            w.queue_send(PendingSend {
                client_id: id.into(),
                conversation_id: "c-a".into(),
                text: "hello".into(),
                action: false,
                state: SendState::Sending,
            });
        }
        let mut echo = message("m1", "2026-11-21T10:00:00Z", "hello");
        echo.sender_id = SELF_ID.into();
        echo.client_id = Some("k1".into());
        w.apply(&created(1, echo.clone()), 0);
        assert_eq!(w.pending_in("c-a").len(), 1);
        // A replayed acknowledgement below the cursor still lands.
        w.apply(&created(1, echo), 0);
        assert_eq!(w.timelines["c-a"].entries.len(), 1);

        let e = ServerEvent::decode(
            r#"{"v":1,"type":"command.error","clientId":"k2","message":"Not connected"}"#,
        )
        .unwrap();
        w.apply(&e, 0);
        assert_eq!(
            w.pending_in("c-a")[0].state,
            SendState::Failed("Not connected".into())
        );
        assert_eq!(w.retry_send("k2").unwrap().state, SendState::Sending);
    }

    #[test]
    fn counts_and_conversation_updates() {
        let mut w = Workspace::from_snapshot(snapshot(0));
        let e = ServerEvent::decode(
            r#"{"v":1,"seq":1,"type":"conversation.counts","data":{"id":"c-a","unreadCount":4,"mentionCount":2}}"#,
        )
        .unwrap();
        assert!(w.apply(&e, 0).sidebar);
        assert_eq!(w.conversation("c-a").unwrap().unread_count, 4);
        w.mark_read("c-a");
        assert_eq!(w.conversation("c-a").unwrap().mention_count, 0);

        let e = ServerEvent::decode(
            r#"{"v":1,"seq":2,"type":"conversation.removed","data":{"id":"c-b"}}"#,
        )
        .unwrap();
        w.apply(&e, 0);
        assert!(w.conversation("c-b").is_none());
    }

    #[test]
    fn typing_expires_and_a_message_ends_it() {
        let mut w = Workspace::from_snapshot(snapshot(0));
        let e = ServerEvent::decode(
            r#"{"v":1,"type":"typing","data":{"conversationId":"c-a","memberId":"m-bob","state":"active"}}"#,
        )
        .unwrap();
        w.apply(&e, 1000);
        assert_eq!(w.typing_in("c-a", 2000), ["bob"]);
        assert!(w.typing_in("c-a", 1000 + TYPING_TTL_MS).is_empty());
        w.apply(&e, 1000);
        w.apply(
            &created(1, message("1", "2026-11-21T10:00:00Z", "done")),
            1500,
        );
        assert!(w.typing_in("c-a", 2000).is_empty());
    }

    #[test]
    fn your_own_messages_show_your_nick_on_that_network() {
        let w = Workspace::from_snapshot(snapshot(0));
        assert_eq!(w.display_name(SELF_ID, "n1"), "me");
        assert_eq!(w.display_name("u-me", "n1"), "me");
        assert!(w.is_self("u-me", "n1") && !w.is_self("m-bob", "n1"));
        assert_eq!(w.display_name("m-bob", "n1"), "bob");
        assert_eq!(w.display_name("m-ghost", "n1"), "m-ghost");
    }

    #[test]
    fn a_long_live_timeline_keeps_the_newest_and_pages_from_the_oldest_kept() {
        let mut w = Workspace::from_snapshot(snapshot(0));
        w.load_newest("c-a", MessagePage::default());
        for i in 0..(TIMELINE_LIMIT as i64 + 5) {
            let at = format!("2026-11-21T10:00:00.{:03}Z", i % 1000);
            let mut m = message(&format!("m{i:05}"), &at, "x");
            m.position = Some(i);
            w.apply(&created(i + 1, m), 0);
        }
        let t = &w.timelines["c-a"];
        assert_eq!(t.entries.len(), TIMELINE_LIMIT);
        assert_eq!(
            t.older.as_deref(),
            Some(format!("message:{}", t.entries[0].message.id).as_str())
        );
    }
}

//! The workspace: everything the app shows, changed only through [`Workspace::apply`]
//! (socket events) and a few explicit methods (pages loaded, sends started).
//!
//! The rules follow the web client's chat store (`app/stores/chat.ts`):
//! durable events advance a cursor one at a time; a gap means the client
//! missed something and must bootstrap again; a replayed event at or below
//! the cursor is ignored, except a send acknowledgement.
//!
//! Each later feature owns one sub-state file here: a `State` struct (a field
//! of [`Workspace`] named after the file), an `apply` hook called for every
//! fresh event after this file's own handling, and a `reset` hook called on
//! every bootstrap. A hook reports what it touched as a topic in [`Changes`]
//! (each file names its own `TOPIC`), so no feature adds a field to
//! `Changes` or an arm to the match below.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use ircord_contract::event::{Document, Durable, ServerEvent};
use ircord_contract::*;

pub mod appearance;
pub mod documents;
pub mod drafts;
pub mod emoji;
pub mod inbox;
pub mod link_media;
pub mod notify_settings;
pub mod outbox;
pub mod parties;
pub mod people;
pub mod read_aloud;
pub mod saved;
pub mod server_log;
pub mod sidebar_order;
pub mod theme_css;
pub mod timeline;

pub use documents::{Documents, Revisioned, SaveState};
pub use outbox::{PendingSend, SendState};
pub use timeline::{Entry, TIMELINE_LIMIT, Timeline};

/// How long a typing indicator lasts without a renewal, in milliseconds.
pub const TYPING_TTL_MS: i64 = 6000;
/// The sender ID the server uses for the signed-in user.
pub const SELF_ID: &str = "you";

/// Topics the workspace itself reports for events no sub-state owns.
pub mod topic {
    /// `attract`: someone asked for attention in a conversation (the ID).
    pub const ATTRACT: &str = "attract";
    /// `channel-services.updated` for a conversation (the ID).
    pub const CHANNEL_SERVICES: &str = "channel_services";
    /// `connection.updated` for a network (the ID).
    pub const CONNECTION: &str = "connection";
    /// `devices.updated`: the account's device list changed.
    pub const DEVICES: &str = "devices";
    /// A revisioned document's state in [`super::Workspace::documents`]
    /// changed (read, edited, saved); the ID is [`super::documents::document_id`].
    pub const DOCUMENTS: &str = "documents";
    /// The server announced a newer revision of a document (same IDs as
    /// [`DOCUMENTS`]); a watched document reads itself again.
    pub const REVISIONS: &str = "revisions";
    /// `emotes.updated`: the emoji catalog has a new version.
    pub const EMOTES: &str = "emotes";
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

/// Which parts of a topic changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Touched {
    /// The whole topic.
    All,
    /// Only these IDs (conversations, networks, messages: the topic says which).
    Ids(BTreeSet<String>),
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
    /// Everything else, by topic: each sub-state file and [`topic`] name theirs.
    pub topics: BTreeMap<&'static str, Touched>,
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
        for (topic, touched) in other.topics {
            self.touch(topic, touched);
        }
    }

    fn timeline(conversation: &str) -> Self {
        Changes {
            timelines: BTreeSet::from([conversation.to_string()]),
            ..Default::default()
        }
    }

    /// A change to the whole of `topic`.
    pub fn topic(topic: &'static str) -> Self {
        let mut c = Changes::default();
        c.touch(topic, Touched::All);
        c
    }

    /// A change to one ID within `topic`.
    pub fn topic_id(topic: &'static str, id: impl Into<String>) -> Self {
        let mut c = Changes::default();
        c.touch(topic, Touched::Ids(BTreeSet::from([id.into()])));
        c
    }

    /// Add a touch to `topic`; touching all of it absorbs single IDs.
    pub fn touch(&mut self, topic: &'static str, touched: Touched) {
        use std::collections::btree_map::Entry;
        match self.topics.entry(topic) {
            Entry::Vacant(e) => {
                e.insert(touched);
            }
            Entry::Occupied(mut e) => match (e.get_mut(), touched) {
                (Touched::All, _) => {}
                (existing, Touched::All) => *existing = Touched::All,
                (Touched::Ids(have), Touched::Ids(more)) => have.extend(more),
            },
        }
    }

    /// Whether anything in `topic` changed, or the whole workspace was replaced.
    pub fn touches(&self, topic: &str) -> bool {
        self.reset || self.topics.contains_key(topic)
    }

    /// Whether `id` within `topic` changed (or all of the topic, or everything).
    pub fn touches_id(&self, topic: &str, id: &str) -> bool {
        self.reset
            || match self.topics.get(topic) {
                Some(Touched::All) => true,
                Some(Touched::Ids(ids)) => ids.contains(id),
                None => false,
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
    /// The newest revision the server announced for each revisioned document.
    pub revisions: BTreeMap<Document, String>,
    /// The revisioned documents, once watched (see `crate::documents`).
    pub documents: documents::Documents,

    // One sub-state per feature, each owned by its file (see the module docs).
    /// R1: reading state across timelines.
    pub reading: timeline::State,
    /// A1: outbox state beyond the pending sends.
    pub sending: outbox::State,
    /// N1: the user's sidebar order and collapsed sections.
    pub sidebar_order: sidebar_order::State,
    /// SV1: each network's server window.
    pub server_log: server_log::State,
    /// P1: profiles, statuses and the ignore list.
    pub people: people::State,
    /// FD2: the inbox.
    pub inbox: inbox::State,
    /// FD2: saved messages.
    pub saved: saved::State,
    /// L1: link previews and their media.
    pub link_media: link_media::State,
    /// E1: the emoji catalog and recent emoji.
    pub emoji: emoji::State,
    /// D1: unsent drafts.
    pub drafts: drafts::State,
    /// PT1: groups and their invitations.
    pub parties: parties::State,
    /// S1: appearance state beyond the documents.
    pub appearance: appearance::State,
    /// S3: notification state beyond the settings document.
    pub notify_settings: notify_settings::State,
}

pub fn parse_time(text: &str) -> i64 {
    chrono::DateTime::parse_from_rfc3339(text)
        .map(|t| t.timestamp_millis())
        .unwrap_or(0)
}

/// A joined text channel that can be reopened by a saved selection.
pub fn can_restore_channel(c: &Conversation) -> bool {
    c.kind == ConversationKind::Channel && c.party.is_none() && c.joined && c.kicked.is_none()
}

/// Every sub-state's hooks, in one fixed order. This list is written once.
macro_rules! each_part {
    ($hook:ident($w:expr, $($arg:expr),*)) => {{
        let mut changes = Changes::default();
        changes.merge(timeline::$hook($w, $($arg),*));
        changes.merge(outbox::$hook($w, $($arg),*));
        changes.merge(sidebar_order::$hook($w, $($arg),*));
        changes.merge(server_log::$hook($w, $($arg),*));
        changes.merge(people::$hook($w, $($arg),*));
        changes.merge(inbox::$hook($w, $($arg),*));
        changes.merge(saved::$hook($w, $($arg),*));
        changes.merge(link_media::$hook($w, $($arg),*));
        changes.merge(emoji::$hook($w, $($arg),*));
        changes.merge(drafts::$hook($w, $($arg),*));
        changes.merge(parties::$hook($w, $($arg),*));
        changes.merge(appearance::$hook($w, $($arg),*));
        changes.merge(notify_settings::$hook($w, $($arg),*));
        changes
    }};
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
        let parts = each_part!(reset(self, &snapshot));
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
        let mut changes = Changes {
            reset: true,
            sidebar: true,
            members: true,
            ..Default::default()
        };
        changes.merge(parts);
        changes
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

    /// Where to land on a network tab, in the same order as the web client.
    pub fn network_conversation(
        &self,
        network_id: &str,
        last_channel: Option<&str>,
    ) -> Option<&Conversation> {
        let conversations = || {
            self.conversations
                .iter()
                .filter(|c| c.network_id == network_id)
        };
        last_channel
            .and_then(|id| conversations().find(|c| c.id == id && can_restore_channel(c)))
            .or_else(|| conversations().find(|c| can_restore_channel(c)))
            .or_else(|| {
                conversations().find(|c| c.kind == ConversationKind::Direct || c.party.is_some())
            })
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
    ///
    /// The workspace handles what it owns first, then every sub-state's
    /// `apply` hook sees the event, unless it was a replay already seen, a
    /// gap, or a frame this client cannot read.
    pub fn apply(&mut self, event: &ServerEvent, now_ms: i64) -> Changes {
        let mut changes = match event {
            ServerEvent::Typing(t) => self.set_typing(t, now_ms),
            ServerEvent::Resync(_) => {
                return Changes {
                    resync: true,
                    ..Default::default()
                };
            }
            ServerEvent::CommandError(e) => e
                .client_id
                .as_deref()
                .and_then(|id| self.fail_send(id, &e.message))
                .unwrap_or_else(|| Changes {
                    notices: vec![e.message.clone()],
                    ..Default::default()
                }),
            ServerEvent::Attract(a) => Changes::topic_id(topic::ATTRACT, &a.conversation_id),
            ServerEvent::EmotesUpdated(_) => Changes::topic(topic::EMOTES),
            ServerEvent::DevicesUpdated => Changes::topic(topic::DEVICES),
            ServerEvent::Durable { seq, event } => {
                if let Some(skip) = self.accept(*seq, event, now_ms) {
                    return skip;
                }
                self.apply_durable(event, now_ms)
            }
            ServerEvent::Unknown { seq: Some(seq), .. } => {
                // An event this client cannot read still counts toward the cursor.
                return self.advance(*seq).unwrap_or_default();
            }
            ServerEvent::Ready(_)
            | ServerEvent::Synced(_)
            | ServerEvent::Alert(_)
            | ServerEvent::Routing(_)
            | ServerEvent::Unknown { seq: None, .. } => Changes::default(),
        };
        changes.merge(each_part!(apply(self, event, now_ms)));
        changes
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

    /// Advance the cursor for a durable event; `Some(changes)` when it is not fresh.
    fn accept(&mut self, seq: i64, event: &Durable, now_ms: i64) -> Option<Changes> {
        if seq <= self.cursor {
            // A retried command is acknowledged again with its original number.
            if let Durable::MessageCreated(m) = event
                && m.client_id.is_some()
            {
                return Some(self.put_message(m.clone(), now_ms));
            }
            return Some(Changes::default());
        }
        self.advance(seq)
    }

    fn apply_durable(&mut self, event: &Durable, now_ms: i64) -> Changes {
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
            Durable::ConnectionUpdated(c) => Changes::topic_id(topic::CONNECTION, &c.network_id),
            Durable::ChannelServicesUpdated(c) => {
                Changes::topic_id(topic::CHANNEL_SERVICES, &c.conversation_id)
            }
            Durable::DocumentUpdated { document, update } => {
                self.revisions.insert(*document, update.revision.clone());
                Changes::topic_id(topic::REVISIONS, documents::document_id(*document))
            }
            // Each of these belongs to a sub-state, whose hook handles it.
            Durable::PartyUpdated(_)
            | Durable::PartyRemoved(_)
            | Durable::ServerLine(_)
            | Durable::ServerLogUpdated(_)
            | Durable::SavedAdded(_)
            | Durable::SavedRemoved(_)
            | Durable::InboxAdded(_)
            | Durable::InboxUpdated(_)
            | Durable::Other(_) => Changes::default(),
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
    fn a_network_reopens_its_last_joined_channel_with_a_safe_fallback() {
        let mut w = Workspace::from_snapshot(snapshot(1));
        let land = |w: &Workspace, last| w.network_conversation("n1", last).map(|c| c.id.clone());
        assert_eq!(land(&w, Some("c-a")).as_deref(), Some("c-a"));
        assert_eq!(land(&w, Some("c-dm")).as_deref(), Some("c-b"));
        for c in &mut w.conversations {
            if c.id == "c-a" {
                c.kicked = Some(Kick::default());
            } else if c.id == "c-z" {
                c.joined = false;
            }
        }
        for id in ["c-a", "c-z", "removed"] {
            assert_eq!(land(&w, Some(id)).as_deref(), Some("c-b"));
        }
        w.conversations
            .iter_mut()
            .find(|c| c.id == "c-b")
            .unwrap()
            .network_id = "n2".into();
        assert_eq!(land(&w, Some("c-b")).as_deref(), Some("c-dm"));
        w.conversations
            .retain(|c| c.kind != ConversationKind::Direct);
        assert_eq!(land(&w, Some("c-a")), None);
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

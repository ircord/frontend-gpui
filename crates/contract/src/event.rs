//! The event socket's envelope.
//!
//! The server sends `{ "v": 1, "type": ..., "seq"?: n, "data"?: ... }`. The
//! generated union in `openapi.json` is untagged and would reject a type this
//! client does not know, so the envelope is decoded by hand: the `type` picks
//! the payload, and an unknown type, or a known type whose payload no longer
//! decodes, becomes [`ServerEvent::Unknown`] carrying its sequence number so
//! the cursor still advances.

use crate::generated::*;
use serde::Deserialize;
use serde_json::Value;

/// One frame from `GET /api/v1/events`.
///
/// Every type the pinned contract names decodes to its own variant (see
/// [`SERVER_EVENT_TYPES`]); the test in `tests/events.rs` fails when a backend
/// bump adds a type this enum does not decode.
#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::large_enum_variant)] // decoded once, then moved
pub enum ServerEvent {
    Ready(SessionReady),
    Synced(SessionSynced),
    Resync(SessionResync),
    CommandError(CommandError),
    Typing(TypingData),
    /// Someone asked for your attention in a conversation (`attract`).
    Attract(AttractData),
    /// The emoji catalog changed; re-read it if its version is newer.
    EmotesUpdated(EmoteVersion),
    /// The account's device list changed; re-read it if shown.
    DevicesUpdated,
    Alert(Alert),
    Routing(RoutingSnapshot),
    /// A durable event with its replay sequence number.
    Durable {
        seq: i64,
        event: Durable,
    },
    /// A frame this client does not understand: no `seq`, or a payload that no longer decodes.
    Unknown {
        kind: String,
        seq: Option<i64>,
    },
}

/// The durable events: each carries a sequence number and is replayed on resume.
#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::large_enum_variant)]
pub enum Durable {
    MessageCreated(Message),
    MessageUpdated(Message),
    ConversationUpdated(Conversation),
    ConversationCounts(ConversationCounts),
    ConversationRemoved(Removal),
    ChannelServicesUpdated(ChannelServicesChange),
    NetworkUpdated(Network),
    NetworkRemoved(Removal),
    MemberUpdated(Member),
    ProfileUpdated(Member),
    InvitationReceived(Invitation),
    InvitationRemoved(Removal),
    PartyUpdated(Party),
    PartyRemoved(PartyRemoval),
    HistorySynced(HistoryUpdate),
    HistoryGap(HistoryUpdate),
    HistoryFailed(HistoryUpdate),
    ConnectionUpdated(ConnectionInfo),
    /// One line of a network's server window.
    ServerLine(ServerLine),
    /// A network's server log changed beyond single lines; re-read it if shown.
    ServerLogUpdated(NetworkInvalidation),
    /// One of the account's revisioned documents changed on the server.
    DocumentUpdated {
        document: Document,
        update: RevisionUpdate,
    },
    SavedAdded(SavedMessage),
    SavedRemoved(SavedRemoval),
    InboxAdded(InboxEntry),
    InboxUpdated(InboxChange),
    /// A durable event type newer than this client. It still advances the cursor.
    Other(String),
}

/// The account documents the server keeps with a revision, each announced by
/// its own `*.updated` event carrying only the new revision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Document {
    /// `client-preferences.updated`, `/api/v1/app/preferences`.
    Preferences,
    /// `appearance.updated`, `/api/v1/app/appearance`.
    Appearance,
    /// `themes.updated`, `/api/v1/app/themes`.
    Themes,
    /// `library.updated`, `/api/v1/app/library`.
    Library,
    /// `notification-settings.updated`, `/api/v1/notifications/settings`.
    NotificationSettings,
}

impl Document {
    pub const ALL: [Document; 5] = [
        Document::Preferences,
        Document::Appearance,
        Document::Themes,
        Document::Library,
        Document::NotificationSettings,
    ];

    /// The event type that announces a new revision.
    pub fn event_type(self) -> &'static str {
        match self {
            Document::Preferences => "client-preferences.updated",
            Document::Appearance => "appearance.updated",
            Document::Themes => "themes.updated",
            Document::Library => "library.updated",
            Document::NotificationSettings => "notification-settings.updated",
        }
    }

    /// Where the document is read and written.
    pub fn path(self) -> &'static str {
        match self {
            Document::Preferences => "/api/v1/app/preferences",
            Document::Appearance => "/api/v1/app/appearance",
            Document::Themes => "/api/v1/app/themes",
            Document::Library => "/api/v1/app/library",
            Document::NotificationSettings => "/api/v1/notifications/settings",
        }
    }

    fn from_event_type(kind: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|d| d.event_type() == kind)
    }
}

/// Every event type this envelope decodes, as the server spells them.
pub const SERVER_EVENT_TYPES: &[&str] = &[
    "session.ready",
    "session.synced",
    "session.resync",
    "command.error",
    "typing",
    "attract",
    "emotes.updated",
    "devices.updated",
    "notification.alert",
    "notification.routing",
    "message.created",
    "message.updated",
    "conversation.updated",
    "conversation.counts",
    "conversation.removed",
    "channel-services.updated",
    "network.updated",
    "network.removed",
    "member.updated",
    "profile.updated",
    "invitation.received",
    "invitation.removed",
    "party.updated",
    "party.removed",
    "history.synced",
    "history.gap",
    "history.sync.failed",
    "connection.updated",
    "server.line",
    "server-log.updated",
    "client-preferences.updated",
    "appearance.updated",
    "themes.updated",
    "library.updated",
    "notification-settings.updated",
    "saved.added",
    "saved.removed",
    "inbox.added",
    "inbox.updated",
];

#[derive(Deserialize)]
struct Envelope {
    #[serde(rename = "type", default)]
    kind: String,
    #[serde(default)]
    seq: Option<i64>,
    #[serde(default)]
    data: Value,
}

impl ServerEvent {
    /// Decode one text frame. Only malformed JSON is an error.
    pub fn decode(text: &str) -> Result<Self, serde_json::Error> {
        Self::decode_value(serde_json::from_str(text)?)
    }

    /// Decode an event from the HTTP replay response or the event socket.
    pub fn decode_value(raw: Value) -> Result<Self, serde_json::Error> {
        let envelope: Envelope = serde_json::from_value(raw.clone())?;
        Ok(Self::from_envelope(raw, envelope))
    }

    /// The durable sequence number, if this frame has one.
    pub fn seq(&self) -> Option<i64> {
        match self {
            ServerEvent::Durable { seq, .. } => Some(*seq),
            ServerEvent::Unknown { seq, .. } => *seq,
            _ => None,
        }
    }

    fn from_envelope(raw: Value, e: Envelope) -> Self {
        fn whole<T: for<'de> Deserialize<'de>>(raw: Value) -> Option<T> {
            serde_json::from_value(raw).ok()
        }
        fn data<T: for<'de> Deserialize<'de>>(data: Value) -> Option<T> {
            serde_json::from_value(data).ok()
        }
        let unknown = |kind: &str, seq| ServerEvent::Unknown {
            kind: kind.to_string(),
            seq,
        };
        let decoded = match e.kind.as_str() {
            "session.ready" => whole(raw).map(ServerEvent::Ready),
            "session.synced" => whole(raw).map(ServerEvent::Synced),
            "session.resync" => whole(raw).map(ServerEvent::Resync),
            "command.error" => whole(raw).map(ServerEvent::CommandError),
            "typing" => data(e.data).map(ServerEvent::Typing),
            "notification.alert" => data(e.data).map(ServerEvent::Alert),
            "notification.routing" => data(e.data).map(ServerEvent::Routing),
            "attract" => data(e.data).map(ServerEvent::Attract),
            "emotes.updated" => data(e.data).map(ServerEvent::EmotesUpdated),
            "devices.updated" => Some(ServerEvent::DevicesUpdated),
            kind => match e.seq {
                Some(seq) => {
                    Self::durable(kind, e.data).map(|event| ServerEvent::Durable { seq, event })
                }
                None => None,
            },
        };
        decoded.unwrap_or_else(|| unknown(&e.kind, e.seq))
    }

    fn durable(kind: &str, d: Value) -> Option<Durable> {
        fn de<T: for<'de> Deserialize<'de>>(d: Value) -> Option<T> {
            serde_json::from_value(d).ok()
        }
        Some(match kind {
            "message.created" => Durable::MessageCreated(de(d)?),
            "message.updated" => Durable::MessageUpdated(de(d)?),
            "conversation.updated" => Durable::ConversationUpdated(de(d)?),
            "conversation.counts" => Durable::ConversationCounts(de(d)?),
            "conversation.removed" => Durable::ConversationRemoved(de(d)?),
            "network.updated" => Durable::NetworkUpdated(de(d)?),
            "network.removed" => Durable::NetworkRemoved(de(d)?),
            "member.updated" => Durable::MemberUpdated(de(d)?),
            "profile.updated" => Durable::ProfileUpdated(de(d)?),
            "invitation.received" => Durable::InvitationReceived(de(d)?),
            "invitation.removed" => Durable::InvitationRemoved(de(d)?),
            "channel-services.updated" => Durable::ChannelServicesUpdated(de(d)?),
            "party.updated" => Durable::PartyUpdated(de(d)?),
            "party.removed" => Durable::PartyRemoved(de(d)?),
            "server.line" => Durable::ServerLine(de(d)?),
            "server-log.updated" => Durable::ServerLogUpdated(de(d)?),
            "saved.added" => Durable::SavedAdded(de(d)?),
            "saved.removed" => Durable::SavedRemoved(de(d)?),
            "inbox.added" => Durable::InboxAdded(de(d)?),
            "inbox.updated" => Durable::InboxUpdated(de(d)?),
            "history.synced" => Durable::HistorySynced(de(d)?),
            "history.gap" => Durable::HistoryGap(de(d)?),
            "history.sync.failed" => Durable::HistoryFailed(de(d)?),
            "connection.updated" => Durable::ConnectionUpdated(de(d)?),
            other => match Document::from_event_type(other) {
                Some(document) => Durable::DocumentUpdated {
                    document,
                    update: de(d)?,
                },
                None => Durable::Other(other.to_string()),
            },
        })
    }

    /// The event type as the server spells it (`Unknown` keeps the one it had).
    pub fn kind(&self) -> &str {
        match self {
            ServerEvent::Ready(_) => "session.ready",
            ServerEvent::Synced(_) => "session.synced",
            ServerEvent::Resync(_) => "session.resync",
            ServerEvent::CommandError(_) => "command.error",
            ServerEvent::Typing(_) => "typing",
            ServerEvent::Attract(_) => "attract",
            ServerEvent::EmotesUpdated(_) => "emotes.updated",
            ServerEvent::DevicesUpdated => "devices.updated",
            ServerEvent::Alert(_) => "notification.alert",
            ServerEvent::Routing(_) => "notification.routing",
            ServerEvent::Durable { event, .. } => event.kind(),
            ServerEvent::Unknown { kind, .. } => kind,
        }
    }
}

impl Durable {
    /// The event type as the server spells it.
    pub fn kind(&self) -> &str {
        match self {
            Durable::MessageCreated(_) => "message.created",
            Durable::MessageUpdated(_) => "message.updated",
            Durable::ConversationUpdated(_) => "conversation.updated",
            Durable::ConversationCounts(_) => "conversation.counts",
            Durable::ConversationRemoved(_) => "conversation.removed",
            Durable::ChannelServicesUpdated(_) => "channel-services.updated",
            Durable::NetworkUpdated(_) => "network.updated",
            Durable::NetworkRemoved(_) => "network.removed",
            Durable::MemberUpdated(_) => "member.updated",
            Durable::ProfileUpdated(_) => "profile.updated",
            Durable::InvitationReceived(_) => "invitation.received",
            Durable::InvitationRemoved(_) => "invitation.removed",
            Durable::PartyUpdated(_) => "party.updated",
            Durable::PartyRemoved(_) => "party.removed",
            Durable::HistorySynced(_) => "history.synced",
            Durable::HistoryGap(_) => "history.gap",
            Durable::HistoryFailed(_) => "history.sync.failed",
            Durable::ConnectionUpdated(_) => "connection.updated",
            Durable::ServerLine(_) => "server.line",
            Durable::ServerLogUpdated(_) => "server-log.updated",
            Durable::DocumentUpdated { document, .. } => document.event_type(),
            Durable::SavedAdded(_) => "saved.added",
            Durable::SavedRemoved(_) => "saved.removed",
            Durable::InboxAdded(_) => "inbox.added",
            Durable::InboxUpdated(_) => "inbox.updated",
            Durable::Other(kind) => kind,
        }
    }
}

/// `session.resume`: the first frame the client sends.
pub fn resume_frame(after: i64, tab: &str, incarnation: &str) -> String {
    serde_json::to_string(&SessionResume {
        after,
        incarnation: Some(incarnation.to_string()),
        sync: Some(true),
        tab: Some(tab.to_string()),
        r#type: "session.resume".to_string(),
        v: 1,
    })
    .expect("resume frame serialises")
}

/// `typing.set`: the only command the socket accepts.
pub fn typing_frame(conversation_id: &str, state: TypingCommandState) -> String {
    serde_json::to_string(&TypingCommand {
        conversation_id: conversation_id.to_string(),
        state,
        r#type: "typing.set".to_string(),
        v: 1,
    })
    .expect("typing frame serialises")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_a_real_session_ready_frame() {
        let e = ServerEvent::decode(
            r#"{"v":1,"type":"session.ready","cursor":7,"routing":true,"emoteVersion":3}"#,
        )
        .unwrap();
        match e {
            ServerEvent::Ready(r) => {
                assert_eq!(r.cursor, 7);
                assert_eq!(r.routing, Some(true));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn unknown_event_types_keep_their_sequence() {
        let e =
            ServerEvent::decode(r#"{"v":1,"seq":12,"type":"brand.new","data":{"x":1}}"#).unwrap();
        assert_eq!(
            e,
            ServerEvent::Durable {
                seq: 12,
                event: Durable::Other("brand.new".into())
            }
        );
        assert_eq!(e.seq(), Some(12));
        let e = ServerEvent::decode(r#"{"v":1,"type":"brand.new.ephemeral"}"#).unwrap();
        assert_eq!(e.seq(), None);
        assert!(matches!(e, ServerEvent::Unknown { .. }));
    }

    #[test]
    fn unused_durable_events_still_advance() {
        // `saved.added` was this test's example until F1 decoded every
        // contract type; a type newer than the contract takes its place.
        let e = ServerEvent::decode(r#"{"v":1,"seq":3,"type":"saved.renamed","data":{}}"#).unwrap();
        assert_eq!(
            e,
            ServerEvent::Durable {
                seq: 3,
                event: Durable::Other("saved.renamed".into())
            }
        );
    }

    #[test]
    fn revisioned_documents_share_one_variant() {
        let e = ServerEvent::decode(
            r#"{"v":1,"seq":4,"type":"themes.updated","data":{"revision":"r9"}}"#,
        )
        .unwrap();
        let ServerEvent::Durable {
            event: Durable::DocumentUpdated { document, update },
            ..
        } = &e
        else {
            panic!("{e:?}")
        };
        assert_eq!(*document, Document::Themes);
        assert_eq!(update.revision, "r9");
        assert_eq!(e.kind(), "themes.updated");
    }

    #[test]
    fn unknown_fields_and_enum_values_are_tolerated() {
        let e = ServerEvent::decode(
            r#"{"v":1,"seq":5,"type":"message.created","data":{"id":"m1","conversationId":"c1",
            "createdAt":"2026-11-21T10:00:00Z","kind":"hologram","senderId":"u1","text":"hi",
            "reactions":[],"futureField":{"deep":true}}}"#,
        )
        .unwrap();
        let ServerEvent::Durable {
            event: Durable::MessageCreated(m),
            ..
        } = e
        else {
            panic!("{e:?}")
        };
        assert_eq!(m.kind, MessageKind::Unknown);
        assert_eq!(m.text, "hi");
    }

    #[test]
    fn resume_frame_matches_the_contract() {
        let v: Value = serde_json::from_str(&resume_frame(9, "tab", "inc")).unwrap();
        assert_eq!(v["type"], "session.resume");
        assert_eq!(v["after"], 9);
        assert_eq!(v["sync"], true);
        assert_eq!(v["v"], 1);
    }
}

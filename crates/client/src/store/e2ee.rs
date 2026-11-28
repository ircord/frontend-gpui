//! End-to-end encryption as the views see it (experimental): this device's
//! state, its devices and approval requests, each encrypted conversation,
//! and how each encrypted message is shown. [`crate::e2ee::Engine`] fills it;
//! nothing here decrypts.
//!
//! Decrypted messages are written into their timeline entries
//! ([`show`]): the text, kind, reply, GIF and files take the place of the
//! server's placeholder, so every view that draws or copies a message sees
//! the plaintext without knowing about encryption.

use std::collections::{BTreeSet, HashMap};

use ircord_contract::event::ServerEvent;
use ircord_contract::*;

use super::{Changes, Workspace};
use crate::e2ee::vault::{Outcome, Unreadable};

/// The topic this sub-state reports in [`Changes`].
pub const TOPIC: &str = "e2ee";

/// What the backend puts in an encrypted message's text.
pub const PLACEHOLDER: &str = "[encrypted message - open IRCord to read]";

/// Where a decrypted file is served from inside the app: its URL is
/// `{server origin}{FILE_PATH}{message id}/{index}`, which the app's image
/// loader answers from this device instead of the network.
pub const FILE_PATH: &str = "/__ircord-e2ee/";

/// This device's encryption state.
#[derive(Debug, Clone, Default, PartialEq)]
pub enum DeviceState {
    /// The server does not offer encryption, or this session has not
    /// checked yet.
    #[default]
    Off,
    /// Opening the local store and reading the key directory.
    Starting,
    /// Set up: encrypted chats work here. `approver` when this device can
    /// approve others (it holds the account's identity key).
    Ready { approver: bool },
    /// A new device, waiting for another of the account's devices to
    /// approve it: the person scans `qr` there or types `code` (20 digits in
    /// groups of five).
    Waiting { qr: String, code: String },
    /// The account started fresh on another device: this one lost its
    /// approval (it keeps what it stored).
    ResetElsewhere,
    /// Something went wrong; the reason is shown.
    Failed(String),
}

impl DeviceState {
    pub fn ready(&self) -> bool {
        matches!(self, DeviceState::Ready { .. })
    }
}

/// One of the account's devices in the key directory.
#[derive(Debug, Clone, PartialEq)]
pub struct OwnDevice {
    pub id: String,
    pub label: String,
    pub this: bool,
    /// Its certificate is signed by the account's identity key.
    pub approved: bool,
    pub key_packages: i64,
}

/// Approving another device from this one, with the code it shows.
#[derive(Debug, Clone, Default, PartialEq)]
pub enum Approving {
    #[default]
    Idle,
    /// Certifying it and adding it to this device's encrypted chats.
    Busy,
    /// The device is approved.
    Done,
    Failed(ApprovalFailure),
}

/// Why another device could not be approved.
#[derive(Debug, Clone, PartialEq)]
pub enum ApprovalFailure {
    /// Not 20 digits.
    BadCode,
    /// No waiting device made its request with that code.
    NotFound,
    Other(String),
}

/// Why a conversation needs the person's attention.
#[derive(Debug, Clone, PartialEq)]
pub enum Attention {
    /// The other person started fresh with new keys; sending waits until the
    /// person acknowledges it (`identity` is the new key, base64url).
    StartedFresh { identity: String },
}

/// One direct conversation's encryption.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Conversation {
    pub on: bool,
    /// The other account.
    pub peer: String,
    /// The person compared security codes with the peer.
    pub verified: bool,
    pub attention: Option<Attention>,
    /// An unencrypted message arrived while encryption was on.
    pub unencrypted: bool,
    /// When it was turned on or off (ms), and by whom (an account).
    pub since: i64,
    pub by: String,
}

/// How an encrypted message is shown.
#[derive(Debug, Clone, PartialEq)]
pub enum Shown {
    Plain,
    /// "… turned on end-to-end encryption", as a system line.
    Control,
    /// Not shown at all (a part of a longer message, an event this client
    /// does not show yet).
    Hidden,
    Unreadable(Unreadable),
    /// Unreadable like the message before it: one line stands for the run.
    Folded,
}

/// Why encryption cannot be started in a conversation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Blocked {
    /// The server does not offer encryption.
    Off,
    /// This device is not set up (or not approved yet).
    NotReady,
    /// Not a direct conversation on the server's home network.
    NotHere,
    /// The network does not carry encrypted messages (the capability gate).
    Network,
    /// The other person is not logged in to an account on this server.
    NoAccount,
    /// The other person has not set up encryption on any device.
    PeerNotSetUp,
}

#[derive(Debug, Clone, Default)]
pub struct State {
    pub device: DeviceState,
    /// This account's identity fingerprint, in groups of four digits.
    pub fingerprint: Option<String>,
    pub devices: Vec<OwnDevice>,
    /// New devices of the account waiting for approval (requests in the
    /// relay's mailbox), as an approving device last counted them.
    pub waiting_approvals: usize,
    pub approving: Approving,
    pub conversations: HashMap<String, Conversation>,
    /// Message ID → how it is shown.
    pub shown: HashMap<String, Shown>,
    /// Message ID → the text written into its entry, so an entry whose text
    /// changed since (a reload, or a server that edits it) is looked at again.
    pub written: HashMap<String, String>,
    /// Messages that arrived in the clear while encryption was on: drawn
    /// marked.
    pub unencrypted: BTreeSet<String>,
    /// Pending sends (client IDs) not drawn: the later parts of a long message.
    pub hidden_sends: BTreeSet<String>,
    /// Conversations with an action under way (starting, turning off).
    pub busy: BTreeSet<String>,
    /// Peers known to have set up encryption, and those known not to have.
    pub peers: HashMap<String, bool>,
    /// The other person's account in a direct conversation, as WHOIS gave
    /// it, while the network has not said it otherwise (they have not spoken
    /// yet). `None`: they are not logged in to an account.
    pub accounts: HashMap<String, Option<String>>,
    /// The last error per conversation, shown until the next attempt.
    pub errors: HashMap<String, String>,
    /// This device has read which of its conversations are encrypted (its
    /// vault). Until then no direct conversation is written to in the clear.
    pub loaded: bool,
    /// Whether the backend says a direct conversation is encrypted, once
    /// asked.
    pub server: HashMap<String, bool>,
    /// Conversations whose backend state is being read.
    pub asking: BTreeSet<String>,
}

impl State {
    pub fn conversation(&self, id: &str) -> Option<&Conversation> {
        self.conversations.get(id)
    }

    /// Whether nothing may be sent in the clear here.
    pub fn is_on(&self, id: &str) -> bool {
        self.conversations.get(id).is_some_and(|c| c.on)
    }
}

/// Whether nothing may be sent in the clear into `conversation`: on a
/// server that offers encryption, a direct conversation that is encrypted
/// here or on the backend, or any direct conversation while this device has
/// not read its own list and the backend has not said otherwise. Failing
/// closed: a server that hides the state gets nothing in the clear.
pub fn guarded(w: &Workspace, conversation: &str) -> bool {
    if w.capabilities.encryption.is_none() {
        return false;
    }
    let Some(c) = w.conversation(conversation) else {
        return false;
    };
    if c.kind != ConversationKind::Direct {
        return false;
    }
    w.e2ee.is_on(conversation)
        || match w.e2ee.server.get(conversation) {
            Some(on) => *on,
            None => !w.e2ee.loaded,
        }
}

/// Called for every fresh event after the workspace's own handling. The
/// engine watches the timelines instead; nothing to do here.
pub(super) fn apply(_w: &mut Workspace, _event: &ServerEvent, _now_ms: i64) -> Changes {
    Changes::default()
}

/// Called when a bootstrap replaces the workspace. The state is this
/// device's, not the server's, so it stays.
pub(super) fn reset(_w: &mut Workspace, _snapshot: &Snapshot) -> Changes {
    Changes::default()
}

/// What a decrypted message becomes in its timeline entry.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct View {
    pub text: String,
    pub kind: MessageKind,
    pub reply: Option<Reply>,
    pub gif: Option<GIF>,
    pub attachments: Vec<Attachment>,
}

/// Write how a message is shown into its entry and the sub-state. `view` is
/// what replaces the placeholder (absent for hidden messages).
pub fn show(
    w: &mut Workspace,
    conversation: &str,
    message: &str,
    mut shown: Shown,
    view: Option<View>,
) -> Changes {
    if shown == Shown::Unreadable(Unreadable::BeforeDevice) {
        let previous = w.timelines.get(conversation).and_then(|t| {
            let at = t.entries.iter().position(|e| e.message.id == message)?;
            let before = t.entries.get(at.checked_sub(1)?)?;
            w.e2ee.shown.get(&before.message.id)
        });
        if matches!(
            previous,
            Some(Shown::Folded) | Some(Shown::Unreadable(Unreadable::BeforeDevice))
        ) {
            shown = Shown::Folded;
        }
    }
    if let Some(view) = view
        && let Some(t) = w.timelines.get_mut(conversation)
        && let Some(e) = t.entries.iter_mut().find(|e| e.message.id == message)
    {
        let m = &mut e.message;
        w.e2ee
            .written
            .insert(message.to_string(), view.text.clone());
        m.text = view.text;
        m.kind = view.kind;
        m.reply_to = view.reply;
        m.gif = view.gif;
        m.attachments = (!view.attachments.is_empty()).then_some(view.attachments);
    }
    w.e2ee.shown.insert(message.to_string(), shown);
    let mut changes = Changes::timeline(conversation);
    changes.touch(TOPIC, super::Touched::Ids([message.to_string()].into()));
    changes
}

/// How an outcome is recorded in [`State::shown`].
pub fn shown_of(outcome: &Outcome) -> Shown {
    match outcome {
        Outcome::Plain(_) => Shown::Plain,
        Outcome::Control { .. } => Shown::Control,
        Outcome::Hidden => Shown::Hidden,
        Outcome::Unreadable { why } => Shown::Unreadable(why.clone()),
    }
}

/// The encrypted messages in `conversation` this device has not written
/// into the timeline yet (or whose text changed since), oldest first, that
/// are not in `skip`. Whatever text the server put in an encrypted message
/// is never shown as it is.
pub fn waiting(w: &Workspace, conversation: &str, skip: &BTreeSet<String>) -> Vec<Message> {
    let Some(t) = w.timelines.get(conversation) else {
        return Vec::new();
    };
    t.entries
        .iter()
        .map(|e| &e.message)
        .filter(|m| {
            m.encrypted.is_some()
                && !skip.contains(&m.id)
                && match w.e2ee.shown.get(&m.id) {
                    None => true,
                    Some(Shown::Hidden) | Some(Shown::Folded) => false,
                    Some(_) => w.e2ee.written.get(&m.id) != Some(&m.text),
                }
        })
        .cloned()
        .collect()
}

/// Whether a message arrived in the clear in a conversation that was
/// encrypted at the time: a chat message without `encrypted`, after
/// encryption was turned on.
pub fn arrived_unencrypted(w: &Workspace, conversation: &str, m: &Message) -> bool {
    let Some(c) = w.e2ee.conversations.get(conversation) else {
        return false;
    };
    c.on && m.encrypted.is_none()
        && matches!(m.kind, MessageKind::Message | MessageKind::Action)
        && m.deleted != Some(true)
        && super::parse_time(&m.created_at) > c.since
}

/// The ID to put in a local file URL for a message's `index`th file.
pub fn file_url(origin: &str, message: &str, index: usize) -> String {
    format!("{origin}{FILE_PATH}{message}/{index}")
}

/// The message and index a local file URL names.
pub fn parse_file_url(url: &str) -> Option<(String, usize)> {
    let (_, rest) = url.split_once(FILE_PATH)?;
    let (message, index) = rest.split_once('/')?;
    Some((message.to_string(), index.parse().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{SnapshotBuilder, message};

    #[test]
    fn a_shown_message_takes_the_place_of_the_placeholder() {
        let mut w = SnapshotBuilder::new()
            .network("n1", "Home")
            .direct("dm", "m-bob")
            .member("m-bob", "bob")
            .workspace();
        let mut m = message("1", "dm", "m-bob", PLACEHOLDER);
        m.encrypted = Some(Encrypted {
            version: 1,
            class: EncryptedClass::Message,
            payload: "AAAA".into(),
        });
        w.load_newest(
            "dm",
            MessagePage {
                items: vec![m],
                ..Default::default()
            },
        );
        assert_eq!(waiting(&w, "dm", &BTreeSet::new()).len(), 1);
        let changes = show(
            &mut w,
            "dm",
            "1",
            Shown::Plain,
            Some(View {
                text: "hello".into(),
                kind: MessageKind::Action,
                ..Default::default()
            }),
        );
        assert!(changes.timelines.contains("dm"));
        assert!(changes.touches_id(TOPIC, "1"));
        let shown = w.message("dm", "1").unwrap();
        assert_eq!(
            (shown.text.as_str(), shown.kind),
            ("hello", MessageKind::Action)
        );
        assert!(waiting(&w, "dm", &BTreeSet::new()).is_empty());
    }

    #[test]
    fn direct_messages_get_no_plaintext_until_known_safe() {
        let mut w = SnapshotBuilder::new()
            .network("n1", "Home")
            .member("m-bob", "bob")
            .direct("dm", "m-bob")
            .channel("c", "general")
            .capabilities(|c| c.encryption = Some(EncryptionCapability::default()))
            .workspace();
        // Before this device read its list, and with nothing from the server.
        assert!(guarded(&w, "dm"));
        assert!(!guarded(&w, "c"), "channels are never encrypted here");
        w.e2ee.loaded = true;
        assert!(!guarded(&w, "dm"));
        w.e2ee.server.insert("dm".into(), true);
        assert!(guarded(&w, "dm"), "the server says it is encrypted");
        w.e2ee.server.insert("dm".into(), false);
        w.e2ee.conversations.insert(
            "dm".into(),
            Conversation {
                on: true,
                ..Default::default()
            },
        );
        assert!(guarded(&w, "dm"), "this device says it is");
        w.capabilities.encryption = None;
        assert!(!guarded(&w, "dm"), "a server without encryption");
    }

    #[test]
    fn local_file_urls_round_trip() {
        let url = file_url("https://chat.example", "m1", 2);
        assert_eq!(parse_file_url(&url), Some(("m1".to_string(), 2)));
        assert_eq!(parse_file_url("https://chat.example/uploads/x"), None);
    }
}

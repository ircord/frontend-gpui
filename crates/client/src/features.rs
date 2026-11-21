//! What a network allows right now, and why not, ported from the web
//! client's `shared/utils/features.ts` (frontend `48c500f`): one decision
//! shared by toolbars, menus and the composer.
//!
//! A refusal is a [`Reason`]: a stable key, the same one the web client's
//! catalog uses, and the English text. The app's translation catalog looks
//! the key up and falls back to [`Reason::english`].

use ircord_contract::{
    Conversation, ConversationKind, Message, MessageKind, Network, NetworkStatus, PartyState,
};

use crate::irc::{Prefixes, channel_rights_for, channel_target};
use crate::store::Workspace;

/// A message action the network may or may not allow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MessageAction {
    Replies,
    Reactions,
    Redact,
    Edit,
}

/// Why something is not possible.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reason {
    SendReconnect,
    SendJoin,
    SendModerated,
    ActionReconnect,
    ActionJoin,
    ActionInvalidId,
    RedactOwnOnly,
    EditOwnOnly,
    EditNotice,
    EditMedia,
    RepliesUnavailable,
    ReactionsUnavailable,
    RedactUnsupported,
    EditUnsupported,
    EditAccountRequired,
    EditOtherAccount,
    RedactAccountRequired,
    RedactOtherAccount,
    TextLength {
        characters: usize,
        bytes: usize,
    },
    TextLineLimit,
    TextMultilineUnsupported {
        bytes: usize,
    },
    TextMultilineBytes {
        bytes: usize,
    },
    TextMultilineLines {
        lines: usize,
    },
    /// The group conversation is not active.
    Party(PartyState),
}

impl Reason {
    /// The web client's catalog key.
    pub fn key(&self) -> &'static str {
        use Reason::*;
        match self {
            SendReconnect => "collectionsUi.features.send.reconnect",
            SendJoin => "collectionsUi.features.send.join",
            SendModerated => "collectionsUi.features.send.moderated",
            ActionReconnect => "collectionsUi.features.action.reconnect",
            ActionJoin => "collectionsUi.features.action.join",
            ActionInvalidId => "collectionsUi.features.action.invalidId",
            RedactOwnOnly => "collectionsUi.features.redact.ownOnly",
            EditOwnOnly => "collectionsUi.features.edit.ownOnly",
            EditNotice => "collectionsUi.features.edit.notice",
            EditMedia => "collectionsUi.features.edit.media",
            RepliesUnavailable => "collectionsUi.features.replies.unavailable",
            ReactionsUnavailable => "collectionsUi.features.reactions.unavailable",
            RedactUnsupported => "collectionsUi.features.redact.unsupported",
            EditUnsupported => "collectionsUi.features.edit.unsupported",
            EditAccountRequired => "collectionsUi.features.edit.accountRequired",
            EditOtherAccount => "collectionsUi.features.edit.otherAccount",
            RedactAccountRequired => "collectionsUi.features.redact.accountRequired",
            RedactOtherAccount => "collectionsUi.features.redact.otherAccount",
            TextLength { .. } => "collectionsUi.features.text.length",
            TextLineLimit => "collectionsUi.features.text.lineLimit",
            TextMultilineUnsupported { .. } => "collectionsUi.features.text.multilineUnsupported",
            TextMultilineBytes { .. } => "collectionsUi.features.text.multilineBytes",
            TextMultilineLines { .. } => "collectionsUi.features.text.multilineLines",
            Party(PartyState::Provisioning) => "workspaceUi.party.state.provisioning",
            Party(PartyState::Recovering) => "workspaceUi.party.state.recovering",
            Party(PartyState::Archived) => "workspaceUi.party.state.archived",
            Party(_) => "workspaceUi.party.state.converted",
        }
    }

    /// The values a translation fills in, by placeholder name.
    pub fn values(&self) -> Vec<(&'static str, usize)> {
        match *self {
            Reason::TextLength { characters, bytes } => {
                vec![("characters", characters), ("bytes", bytes)]
            }
            Reason::TextMultilineUnsupported { bytes } | Reason::TextMultilineBytes { bytes } => {
                vec![("bytes", bytes)]
            }
            Reason::TextMultilineLines { lines } => vec![("lines", lines)],
            _ => Vec::new(),
        }
    }

    /// The English text, as the web client shows it.
    pub fn english(&self) -> String {
        use Reason::*;
        match self {
            SendReconnect => "Reconnect to this network to send messages.".into(),
            SendJoin => "Join this channel to send messages.".into(),
            SendModerated => "This channel is moderated. You need voice or operator privileges to send messages.".into(),
            ActionReconnect => "Reconnect to this network to use this action.".into(),
            ActionJoin => "Join this channel to use this action.".into(),
            ActionInvalidId => "This message has no usable network message ID.".into(),
            RedactOwnOnly => "You can only delete your own messages.".into(),
            EditOwnOnly => "You can only edit your own messages.".into(),
            EditNotice => "Notices cannot be edited.".into(),
            EditMedia => "GIFs and uploads cannot be edited.".into(),
            RepliesUnavailable => "Replies are unavailable on this network.".into(),
            ReactionsUnavailable => "Reactions are unavailable on this network.".into(),
            RedactUnsupported => "This network does not support deleting messages".into(),
            EditUnsupported => "This network does not support editing messages (it needs message-tags, echo-message and account-tag)".into(),
            EditAccountRequired => "Editing needs a logged-in account on this network".into(),
            EditOtherAccount => "This message wasn\u{2019}t sent from the account you\u{2019}re logged into, so other clients would not accept the edit".into(),
            RedactAccountRequired => "Deleting needs a logged-in account on this network".into(),
            RedactOtherAccount => "This message wasn\u{2019}t sent from the account you\u{2019}re logged into, so the network won\u{2019}t delete it".into(),
            TextLength { characters, bytes } => format!("Use at most {characters} characters and {bytes} UTF-8 bytes per message."),
            TextLineLimit => "This network has not supplied a usable message line limit.".into(),
            TextMultilineUnsupported { bytes } => format!("This network does not support multiline messages. Use one line of at most {bytes} UTF-8 bytes."),
            TextMultilineBytes { bytes } => format!("This network allows at most {bytes} UTF-8 bytes per multiline message."),
            TextMultilineLines { lines } => format!("This network allows at most {lines} wire lines per multiline message."),
            Party(PartyState::Provisioning) => "Setting up this conversation\u{2026}".into(),
            Party(PartyState::Recovering) => "Reconnecting this conversation\u{2026}".into(),
            Party(PartyState::Archived) => "This conversation is archived.".into(),
            Party(_) => "This conversation is no longer active.".into(),
        }
    }
}

impl std::fmt::Display for Reason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.english())
    }
}

/// How you can delete other people's messages in a conversation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Moderation {
    /// As a channel operator (`@` and above, the rule Ergo applies).
    Operator,
    /// As a server owner on the network the moderation capability names.
    Owner,
}

/// The longest message the server accepts, in characters and UTF-8 bytes.
pub const MAX_TEXT_CHARACTERS: usize = 4000;
pub const MAX_TEXT_BYTES: usize = 4096;

/// What the workspace knows about the signed-in user in one conversation.
pub struct Context<'a> {
    pub workspace: &'a Workspace,
    pub conversation: Option<&'a Conversation>,
    pub network: Option<&'a Network>,
    /// The socket is live.
    pub live: bool,
}

impl<'a> Context<'a> {
    /// The context of a conversation by ID.
    pub fn of(workspace: &'a Workspace, conversation_id: &str, live: bool) -> Self {
        let conversation = workspace.conversation(conversation_id);
        let network = conversation.and_then(|c| workspace.network(&c.network_id));
        Context {
            workspace,
            conversation,
            network,
            live,
        }
    }

    /// Your own channel modes. The server keys them by your member ID, or by
    /// your profile ID on the network; the web client calls this `you`.
    pub fn own_modes(&self) -> &'a str {
        let Some(c) = self.conversation else {
            return "";
        };
        c.member_modes
            .iter()
            .find(|(id, _)| self.workspace.is_self(id, &c.network_id))
            .map_or("", |(_, modes)| modes.as_str())
    }

    fn is_own(&self, message: &Message) -> bool {
        let network = self.network.map_or("", |n| n.id.as_str());
        self.workspace.is_self(&message.sender_id, network)
    }

    fn connected(&self) -> Option<&'a Network> {
        self.network
            .filter(|n| self.live && n.status == NetworkStatus::Connected)
    }

    fn inactive_party(&self) -> Option<Reason> {
        let party = self.conversation?.party.as_ref()?;
        (party.state != PartyState::Active).then_some(Reason::Party(party.state))
    }

    fn not_joined(&self) -> bool {
        self.conversation
            .is_none_or(|c| c.kind == ConversationKind::Channel && !c.joined)
    }

    /// Why you cannot send here, if you cannot.
    pub fn send_reason(&self) -> Option<Reason> {
        let Some(network) = self.connected() else {
            return Some(Reason::SendReconnect);
        };
        if let Some(reason) = self.inactive_party() {
            return Some(reason);
        }
        if self.not_joined() {
            return Some(Reason::SendJoin);
        }
        let c = self.conversation?;
        if c.kind == ConversationKind::Channel && c.modes.contains('m') {
            let p = Prefixes::of(&network.isupport);
            let permitted = match (p.mode_rank('v'), p.rank(self.own_modes())) {
                (Some(voice), Some(rank)) => rank <= voice,
                _ => false,
            };
            if !permitted {
                return Some(Reason::SendModerated);
            }
        }
        None
    }

    /// How you could moderate other people's messages here.
    pub fn moderation(&self) -> Option<Moderation> {
        let c = self.conversation?;
        if c.kind != ConversationKind::Channel || c.party.is_some() {
            return None;
        }
        if channel_rights_for(self.own_modes(), Some(c), self.network).op {
            return Some(Moderation::Operator);
        }
        let owner_network = self
            .workspace
            .capabilities
            .moderation
            .as_ref()
            .map(|m| m.network_id.as_str());
        (owner_network.is_some() && self.network.map(|n| n.id.as_str()) == owner_network)
            .then_some(Moderation::Owner)
    }

    /// Whether you moderate here as a server owner (even when also an operator).
    pub fn owner_moderation(&self) -> bool {
        let owner_network = self
            .workspace
            .capabilities
            .moderation
            .as_ref()
            .map(|m| m.network_id.as_str());
        owner_network.is_some()
            && self.network.map(|n| n.id.as_str()) == owner_network
            && self
                .conversation
                .is_some_and(|c| c.kind == ConversationKind::Channel && c.party.is_none())
    }

    /// Why `action` is not offered on `message`, if it is not.
    pub fn action_reason(&self, message: &Message, action: MessageAction) -> Option<Reason> {
        let Some(network) = self.connected() else {
            return Some(Reason::ActionReconnect);
        };
        if let Some(reason) = self.inactive_party() {
            return Some(reason);
        }
        if self.not_joined() {
            return Some(Reason::ActionJoin);
        }
        if action != MessageAction::Redact
            && let Some(reason) = self.send_reason()
        {
            return Some(reason);
        }
        let usable_id = message
            .upstream_id
            .as_deref()
            .is_some_and(|id| !id.is_empty() && !id.starts_with('_'));
        let actionable = matches!(
            message.kind,
            MessageKind::Message | MessageKind::Action | MessageKind::Notice
        );
        if !usable_id || message.deleted == Some(true) || !actionable {
            return Some(Reason::ActionInvalidId);
        }
        let own = self.is_own(message);
        if action == MessageAction::Redact && !own && self.moderation().is_none() {
            return Some(Reason::RedactOwnOnly);
        }
        if action == MessageAction::Edit {
            if !own {
                return Some(Reason::EditOwnOnly);
            }
            if message.kind == MessageKind::Notice {
                return Some(Reason::EditNotice);
            }
            if message.gif.is_some()
                || message.attachment.is_some()
                || message.attachments.as_ref().is_some_and(|a| !a.is_empty())
            {
                return Some(Reason::EditMedia);
            }
        }
        let f = &network.features;
        let supported = match action {
            MessageAction::Replies => f.replies,
            MessageAction::Reactions => f.reactions,
            MessageAction::Redact => f.redact,
            MessageAction::Edit => f.edit,
        };
        if !supported {
            return Some(match action {
                MessageAction::Redact => Reason::RedactUnsupported,
                MessageAction::Edit => Reason::EditUnsupported,
                MessageAction::Replies => Reason::RepliesUnavailable,
                MessageAction::Reactions => Reason::ReactionsUnavailable,
            });
        }
        let account = network.account.as_deref().map(str::to_lowercase);
        let from_account = message.account.as_deref().map(str::to_lowercase);
        match action {
            MessageAction::Redact => {
                // Ergo deletes your own messages from your account, or any as an operator.
                if !own || channel_rights_for(self.own_modes(), self.conversation, self.network).op
                {
                    return None;
                }
                match account {
                    None => Some(Reason::RedactAccountRequired),
                    Some(a) if from_account.as_deref() != Some(a.as_str()) => {
                        Some(Reason::RedactOtherAccount)
                    }
                    Some(_) => None,
                }
            }
            MessageAction::Edit => match account {
                None => Some(Reason::EditAccountRequired),
                Some(a) if from_account.as_deref() != Some(a.as_str()) => {
                    Some(Reason::EditOtherAccount)
                }
                Some(_) => None,
            },
            _ => None,
        }
    }

    /// Why `text` cannot be sent as written, mirroring the bounds the server
    /// publishes (after formatting).
    pub fn text_reason(&self, text: &str, action: bool) -> Option<Reason> {
        let (c, n) = (self.conversation?, self.network?);
        text_reason(text, action, c, n)
    }
}

/// Why `text` cannot be sent to `conversation` as written.
pub fn text_reason(
    text: &str,
    action: bool,
    conversation: &Conversation,
    network: &Network,
) -> Option<Reason> {
    let wire = if action {
        format!("\u{1}ACTION {text}\u{1}")
    } else {
        text.to_string()
    };
    if text.chars().count() > MAX_TEXT_CHARACTERS || text.len() > MAX_TEXT_BYTES {
        return Some(Reason::TextLength {
            characters: MAX_TEXT_CHARACTERS,
            bytes: MAX_TEXT_BYTES,
        });
    }
    let target = if conversation.kind == ConversationKind::Channel {
        channel_target(conversation)
    } else {
        conversation
            .target
            .clone()
            .unwrap_or_else(|| conversation.name.clone())
    };
    let budget = network.features.message_line_bytes - format!("PRIVMSG {target} :").len() as i64;
    if budget < 4 {
        return Some(Reason::TextLineLimit);
    }
    let budget = budget as usize;
    if wire.len() <= budget && !wire.contains('\n') {
        return None;
    }
    let Some(limits) = &network.features.multiline else {
        return Some(Reason::TextMultilineUnsupported { bytes: budget });
    };
    if wire.len() as i64 > limits.max_bytes {
        return Some(Reason::TextMultilineBytes {
            bytes: limits.max_bytes.max(0) as usize,
        });
    }
    let mut lines = 0i64;
    for line in wire.split('\n') {
        let mut used = 0;
        lines += 1;
        for ch in line.chars() {
            let size = ch.len_utf8();
            if used + size > budget {
                lines += 1;
                used = 0;
            }
            used += size;
        }
    }
    (lines > limits.max_lines).then(|| Reason::TextMultilineLines {
        lines: limits.max_lines.max(0) as usize,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::tests_support::snapshot;
    use ircord_contract::{Features, MultilineLimits};

    fn workspace() -> Workspace {
        let mut w = Workspace::from_snapshot(snapshot(0));
        w.networks[0].status = NetworkStatus::Connected;
        w.networks[0].account = Some("Me".into());
        w.networks[0].features = Features {
            replies: true,
            reactions: true,
            redact: true,
            edit: true,
            message_line_bytes: 512,
            ..Default::default()
        };
        w
    }

    fn message(sender: &str) -> Message {
        Message {
            id: "m1".into(),
            conversation_id: "c-a".into(),
            sender_id: sender.into(),
            kind: MessageKind::Message,
            upstream_id: Some("up1".into()),
            account: Some("me".into()),
            ..Default::default()
        }
    }

    #[test]
    fn sending_needs_a_connection_a_join_and_voice_when_moderated() {
        let mut w = workspace();
        assert_eq!(Context::of(&w, "c-a", true).send_reason(), None);
        assert_eq!(
            Context::of(&w, "c-a", false).send_reason(),
            Some(Reason::SendReconnect)
        );
        w.conversations.iter_mut().for_each(|c| c.joined = false);
        assert_eq!(
            Context::of(&w, "c-a", true).send_reason(),
            Some(Reason::SendJoin)
        );
        let mut w = workspace();
        let c = w.conversations.iter_mut().find(|c| c.id == "c-a").unwrap();
        c.modes = "m".into();
        assert_eq!(
            Context::of(&w, "c-a", true).send_reason(),
            Some(Reason::SendModerated)
        );
        let c = w.conversations.iter_mut().find(|c| c.id == "c-a").unwrap();
        c.member_modes.insert("u-me".into(), "+".into());
        assert_eq!(Context::of(&w, "c-a", true).send_reason(), None);
    }

    #[test]
    fn editing_is_for_your_own_messages_from_your_account() {
        let w = workspace();
        let cx = Context::of(&w, "c-a", true);
        assert_eq!(
            cx.action_reason(&message("u-me"), MessageAction::Edit),
            None
        );
        assert_eq!(
            cx.action_reason(&message("m-bob"), MessageAction::Edit),
            Some(Reason::EditOwnOnly)
        );
        let mut other = message("u-me");
        other.account = Some("someone".into());
        assert_eq!(
            cx.action_reason(&other, MessageAction::Edit),
            Some(Reason::EditOtherAccount)
        );
        let mut local = message("u-me");
        local.upstream_id = Some("_local".into());
        assert_eq!(
            cx.action_reason(&local, MessageAction::Reactions),
            Some(Reason::ActionInvalidId)
        );
    }

    #[test]
    fn deleting_others_needs_operator_or_owner() {
        let mut w = workspace();
        assert_eq!(
            Context::of(&w, "c-a", true).action_reason(&message("m-bob"), MessageAction::Redact),
            Some(Reason::RedactOwnOnly)
        );
        let c = w.conversations.iter_mut().find(|c| c.id == "c-a").unwrap();
        c.member_modes.insert("u-me".into(), "@".into());
        let cx = Context::of(&w, "c-a", true);
        assert_eq!(cx.moderation(), Some(Moderation::Operator));
        assert_eq!(
            cx.action_reason(&message("m-bob"), MessageAction::Redact),
            None
        );
        let mut w = workspace();
        w.capabilities.moderation = Some(ircord_contract::ModerationCapability {
            network_id: "n1".into(),
        });
        let cx = Context::of(&w, "c-a", true);
        assert_eq!(cx.moderation(), Some(Moderation::Owner));
        assert!(cx.owner_moderation());
        w.networks[0].features.redact = false;
        assert_eq!(
            Context::of(&w, "c-a", true).action_reason(&message("u-me"), MessageAction::Redact),
            Some(Reason::RedactUnsupported)
        );
    }

    #[test]
    fn text_limits_follow_the_line_budget_and_multiline() {
        let mut w = workspace();
        let cx = Context::of(&w, "c-a", true);
        assert_eq!(cx.text_reason("hello", false), None);
        assert_eq!(
            cx.text_reason(&"x".repeat(4001), false),
            Some(Reason::TextLength {
                characters: 4000,
                bytes: 4096
            })
        );
        // `PRIVMSG #alpha :` is 16 bytes of the 512.
        assert_eq!(
            cx.text_reason(&"x".repeat(497), false),
            Some(Reason::TextMultilineUnsupported { bytes: 496 })
        );
        w.networks[0].features.multiline = Some(MultilineLimits {
            max_bytes: 1000,
            max_lines: 2,
        });
        let cx = Context::of(&w, "c-a", true);
        assert_eq!(cx.text_reason("a\nb", false), None);
        assert_eq!(
            cx.text_reason("a\nb\nc", false),
            Some(Reason::TextMultilineLines { lines: 2 })
        );
        assert_eq!(
            cx.text_reason(&"x".repeat(1001), false),
            Some(Reason::TextMultilineBytes { bytes: 1000 })
        );
        assert_eq!(
            Reason::TextMultilineLines { lines: 2 }.values(),
            [("lines", 2)]
        );
        assert_eq!(Reason::SendJoin.key(), "collectionsUi.features.send.join");
    }
}

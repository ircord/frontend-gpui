//! The outbox: messages sent but not yet confirmed by the server, and failed
//! sends kept for a retry.
//!
//! Owned by package A1 (message actions): it also discards failed sends,
//! restores the server's outbox at start, applies the answers to edits,
//! reactions and deletions, and drops deleted messages from the timelines.

use ircord_contract::event::{Durable, ServerEvent};
use ircord_contract::*;

use super::{Changes, Workspace};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum SendState {
    #[default]
    Sending,
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct PendingSend {
    pub client_id: String,
    pub conversation_id: String,
    pub text: String,
    pub action: bool,
    pub state: SendState,
    /// When it was sent, in milliseconds since the epoch: the time on its
    /// row in the timeline.
    pub created_ms: i64,
    /// The message it replies to, kept for a retry.
    pub reply_to_id: Option<String>,
    /// The GIF it carries; its `text` is the GIF's URL.
    pub gif: Option<GIF>,
    /// The uploads it carries, by ID.
    pub attachments: Vec<String>,
    /// End-to-end encrypted: what is sent instead of `text`, which stays on
    /// this device. A retry sends the same ciphertext.
    pub encrypted: Option<Encrypted>,
}

impl PendingSend {
    /// Whether its text can go back into the message box: not a reply, a GIF
    /// or uploads, which the box cannot carry (the web client's `movable`).
    pub fn movable(&self) -> bool {
        self.reply_to_id.is_none()
            && self.gif.is_none()
            && self.attachments.is_empty()
            && self.encrypted.is_none()
    }
}

/// The topic this sub-state reports in [`Changes`] (timeline content itself
/// is reported in `Changes::timelines`).
pub const TOPIC: &str = "sending";

/// Outbox state beyond the pending sends themselves.
#[derive(Debug, Clone, Default)]
pub struct State {}

/// The quote text the server gives a reply whose original was deleted.
pub const DELETED_QUOTE: &str = "Message deleted";

/// Called for every fresh event after the workspace's own handling.
pub(super) fn apply(w: &mut Workspace, event: &ServerEvent, _now_ms: i64) -> Changes {
    match event {
        ServerEvent::Durable {
            event: Durable::MessageUpdated(m),
            ..
        } if m.deleted == Some(true) => w.forget_deleted(&m.conversation_id, &m.id),
        _ => Changes::default(),
    }
}

/// Called when a bootstrap replaces the workspace, before it is replaced.
pub(super) fn reset(_w: &mut Workspace, _snapshot: &Snapshot) -> Changes {
    Changes::default()
}

impl Workspace {
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

    /// The pending send with `client_id`.
    pub fn pending(&self, client_id: &str) -> Option<&PendingSend> {
        self.outbox.iter().find(|p| p.client_id == client_id)
    }

    /// Take a send out of the outbox (the person discarded it).
    pub fn discard_send(&mut self, client_id: &str) -> Option<(PendingSend, Changes)> {
        let at = self.outbox.iter().position(|p| p.client_id == client_id)?;
        let p = self.outbox.remove(at);
        let mut changes = Changes::timeline(&p.conversation_id);
        changes.touch(TOPIC, super::Touched::All);
        Some((p, changes))
    }

    /// Put back the sends the server still holds for this account (after a
    /// restart, or a bootstrap): failed ones to retry or discard, and ones
    /// still on their way. Sends already known or already delivered stay as
    /// they are.
    pub fn restore_outbox(&mut self, entries: Vec<OutboxEntry>) -> Changes {
        let mut changes = Changes::default();
        for e in entries {
            let state = match e.state {
                OutboxEntryState::Failed => SendState::Failed(
                    e.error
                        .clone()
                        .filter(|r| !r.is_empty())
                        .unwrap_or_else(|| "The message was not sent.".into()),
                ),
                OutboxEntryState::Queued | OutboxEntryState::Pending => SendState::Sending,
                _ => continue,
            };
            if self.pending(&e.client_id).is_some() {
                continue;
            }
            let echoed = self.timelines.get(&e.conversation_id).is_some_and(|t| {
                t.entries
                    .iter()
                    .any(|x| x.message.client_id.as_deref() == Some(e.client_id.as_str()))
            });
            if echoed {
                continue;
            }
            changes.timelines.insert(e.conversation_id.clone());
            self.outbox.push(PendingSend {
                client_id: e.client_id,
                conversation_id: e.conversation_id,
                text: e.payload.text,
                action: e.payload.kind == Some(SendInputKind::Action),
                state,
                created_ms: super::parse_time(&e.created_at),
                reply_to_id: e.payload.reply_to_id.filter(|r| !r.is_empty()),
                gif: e.payload.gif,
                attachments: e.payload.attachments.unwrap_or_default(),
                encrypted: e.payload.encrypted,
            });
        }
        if !changes.is_empty() {
            changes.touch(TOPIC, super::Touched::All);
        }
        changes
    }

    /// A loaded message by ID.
    pub fn message(&self, conversation: &str, id: &str) -> Option<&Message> {
        self.timelines
            .get(conversation)?
            .entries
            .iter()
            .find(|e| e.message.id == id)
            .map(|e| &e.message)
    }

    /// The server's answer to an edit, a reaction or a deletion: the message
    /// as it now stands.
    pub fn message_answered(&mut self, message: Message) -> Changes {
        let (conversation, id) = (message.conversation_id.clone(), message.id.clone());
        let deleted = message.deleted == Some(true);
        let mut changes = self.put_message(message, 0);
        if deleted {
            changes.merge(self.forget_deleted(&conversation, &id));
        }
        changes
    }

    /// A deleted message leaves its timeline; replies to it quote it as deleted.
    fn forget_deleted(&mut self, conversation: &str, id: &str) -> Changes {
        let Some(timeline) = self.timelines.get_mut(conversation) else {
            return Changes::default();
        };
        timeline.entries.retain(|e| e.message.id != id);
        for e in &mut timeline.entries {
            if let Some(reply) = &mut e.message.reply_to
                && reply.id == id
            {
                reply.text = DELETED_QUOTE.to_string();
            }
        }
        Changes::timeline(conversation)
    }
}

#[cfg(test)]
mod tests {
    use ircord_contract::event::{Durable, ServerEvent};
    use ircord_contract::*;

    use super::{DELETED_QUOTE, SendState};
    use crate::store::Workspace;
    use crate::testing::{SnapshotBuilder, message};

    fn workspace() -> Workspace {
        let mut w = SnapshotBuilder::new().channel("c-a", "alpha").workspace();
        let mut reply = message("m2", "c-a", "bob", "agreed");
        reply.reply_to = Some(Reply {
            id: "m1".into(),
            nick: "alice".into(),
            text: "shall we?".into(),
        });
        w.load_newest(
            "c-a",
            MessagePage {
                items: vec![message("m1", "c-a", "alice", "shall we?"), reply],
                ..Default::default()
            },
        );
        w
    }

    fn entry(id: &str, state: OutboxEntryState, text: &str) -> OutboxEntry {
        OutboxEntry {
            client_id: id.into(),
            conversation_id: "c-a".into(),
            created_at: "2026-11-21T10:00:00Z".into(),
            error: (state == OutboxEntryState::Failed).then(|| "refused".into()),
            message_id: None,
            payload: SendInput {
                client_id: id.into(),
                conversation_id: "c-a".into(),
                kind: Some(SendInputKind::Action),
                text: text.into(),
                ..Default::default()
            },
            state,
        }
    }

    #[test]
    fn the_outbox_comes_back_failed_and_in_flight_sends_only() {
        let mut w = workspace();
        let changes = w.restore_outbox(vec![
            entry("f", OutboxEntryState::Failed, "waves"),
            entry("q", OutboxEntryState::Queued, "on its way"),
            entry("s", OutboxEntryState::Sent, "delivered"),
            entry("d", OutboxEntryState::Discarded, "dropped"),
        ]);
        assert!(changes.timelines.contains("c-a"));
        assert!(changes.touches(super::TOPIC));
        let pending: Vec<_> = w
            .pending_in("c-a")
            .iter()
            .map(|p| (p.client_id.as_str(), p.state.clone(), p.action))
            .collect();
        assert_eq!(
            pending,
            [
                ("f", SendState::Failed("refused".into()), true),
                ("q", SendState::Sending, true)
            ]
        );
        // A second restore after a bootstrap adds nothing.
        assert!(
            w.restore_outbox(vec![entry("f", OutboxEntryState::Failed, "waves")])
                .is_empty()
        );
        let (gone, changes) = w.discard_send("f").unwrap();
        assert_eq!(gone.text, "waves");
        assert!(changes.timelines.contains("c-a"));
        assert!(w.pending("f").is_none());
        assert!(w.discard_send("f").is_none());
    }

    #[test]
    fn a_deleted_message_leaves_and_replies_quote_it_as_deleted() {
        let mut w = workspace();
        let mut gone = message("m1", "c-a", "alice", "");
        gone.deleted = Some(true);
        let changes = w.apply(
            &ServerEvent::Durable {
                seq: w.cursor + 1,
                event: Durable::MessageUpdated(gone),
            },
            0,
        );
        assert!(changes.timelines.contains("c-a"));
        assert!(w.message("c-a", "m1").is_none());
        let reply = w.message("c-a", "m2").unwrap().reply_to.clone().unwrap();
        assert_eq!(reply.text, DELETED_QUOTE);
    }

    #[test]
    fn answers_to_actions_replace_the_message() {
        let mut w = workspace();
        let mut reacted = message("m1", "c-a", "alice", "shall we?");
        reacted.reactions = vec![Reaction {
            emoji: "👍".into(),
            member_ids: vec!["you".into()],
        }];
        let changes = w.message_answered(reacted);
        assert!(changes.timelines.contains("c-a"));
        assert_eq!(w.message("c-a", "m1").unwrap().reactions.len(), 1);
        let mut deleted = message("m2", "c-a", "bob", "");
        deleted.deleted = Some(true);
        w.message_answered(deleted);
        assert!(w.message("c-a", "m2").is_none());
    }
}

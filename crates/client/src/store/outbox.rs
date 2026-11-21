//! The outbox: messages sent but not yet confirmed by the server, and failed
//! sends kept for a retry.
//!
//! Owned by package A1 (message actions), which adds discarding a failed
//! send and restoring the outbox at start.

use ircord_contract::event::ServerEvent;
use ircord_contract::*;

use super::{Changes, Workspace};

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

/// The topic this sub-state reports in [`Changes`] (timeline content itself
/// is reported in `Changes::timelines`).
pub const TOPIC: &str = "sending";

/// Outbox state beyond the pending sends themselves.
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
}

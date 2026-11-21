//! Groups: `party.updated`, `party.removed` and group invitations.
//!
//! Owned by package PT1. Keep this feature's state in [`State`] (the
//! workspace's `parties` field) and react to events in [`apply`]; report what
//! changed with `Changes::topic(TOPIC)` or `Changes::topic_id(TOPIC, id)`.

use ircord_contract::event::ServerEvent;
use ircord_contract::{Conversation, Snapshot};

use super::{Changes, Workspace};

/// The topic this sub-state reports in [`Changes`].
pub const TOPIC: &str = "parties";

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

/// What a conversation is called in the sidebar and its header: a group's
/// name, else the people in it; any other conversation's name.
pub fn title(_w: &Workspace, c: &Conversation) -> String {
    c.name.clone()
}

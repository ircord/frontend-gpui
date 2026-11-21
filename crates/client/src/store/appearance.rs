//! Appearance and themes beyond the two documents, which live in
//! `Workspace::documents` (`appearance`, `themes`).
//!
//! Owned by package S1. Keep this feature's state in [`State`] (the
//! workspace's `appearance` field) and react to events in [`apply`]; report what
//! changed with `Changes::topic(TOPIC)` or `Changes::topic_id(TOPIC, id)`.

use ircord_contract::Snapshot;
use ircord_contract::event::ServerEvent;

use super::{Changes, Workspace};

/// The topic this sub-state reports in [`Changes`].
pub const TOPIC: &str = "appearance";

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

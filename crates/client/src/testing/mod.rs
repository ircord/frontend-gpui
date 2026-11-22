//! Test support, behind the `test-support` feature: the [`CannedServer`] and
//! builders for workspaces, messages and event frames, so tests in this
//! crate and in the app build the same fixtures.
//!
//! ```ignore
//! let rt = ircord_client::client::runtime().unwrap();
//! let server = CannedServer::start(&rt);
//! server.snapshot(&SnapshotBuilder::new().channel("c-a", "alpha").build());
//! let (client, updates) = Client::start(rt.clone(), server.api());
//! wait_live(&client);
//! server.push(frame(1, "message.created", &message("m1", "c-a", "m-bob", "hi")));
//! ```

mod server;
#[cfg(test)]
mod tests;

use std::time::{Duration, Instant};

use ircord_contract::*;
use serde::Serialize;
use serde_json::json;

pub use server::{CANNED_COOKIE, CannedServer, Recorded, Reply};

use crate::Client;
use crate::store::{Connection, Workspace};

/// The signed-in user's member ID in built snapshots.
pub const ME: &str = "u-me";
/// The network every built snapshot starts with.
pub const NETWORK: &str = "n1";

/// Builds a bootstrap [`Snapshot`]. It starts with you (`u-me`, nick `me`)
/// and one connected network `n1` ("Home") with the usual features on.
#[derive(Debug, Clone)]
pub struct SnapshotBuilder {
    snapshot: Snapshot,
}

impl Default for SnapshotBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl SnapshotBuilder {
    pub fn new() -> Self {
        let snapshot = Snapshot {
            version: 1,
            cursor: 0,
            user: member(ME, "me"),
            networks: vec![network(NETWORK, "Home")],
            ..Default::default()
        };
        SnapshotBuilder { snapshot }
    }

    pub fn cursor(mut self, cursor: i64) -> Self {
        self.snapshot.cursor = cursor;
        self
    }

    /// Add another connected network.
    pub fn network(mut self, id: &str, name: &str) -> Self {
        self.snapshot.networks.push(network(id, name));
        self
    }

    /// Change the last network added.
    pub fn with_network(mut self, f: impl FnOnce(&mut Network)) -> Self {
        f(self.snapshot.networks.last_mut().expect("a network"));
        self
    }

    /// A joined channel on the last network added (`name` without `#`).
    pub fn channel(mut self, id: &str, name: &str) -> Self {
        let network = self.last_network();
        self.snapshot.conversations.push(Conversation {
            id: id.into(),
            network_id: network,
            kind: ConversationKind::Channel,
            name: name.into(),
            joined: true,
            member_ids: vec![ME.into()],
            ..Default::default()
        });
        self
    }

    /// A direct conversation with `member` (added with [`Self::member`]) on the last network.
    pub fn direct(mut self, id: &str, member: &str) -> Self {
        let network = self.last_network();
        let nick = self
            .snapshot
            .members
            .iter()
            .find(|m| m.id == member)
            .map_or_else(|| member.to_string(), |m| m.nick.clone());
        self.snapshot.conversations.push(Conversation {
            id: id.into(),
            network_id: network,
            kind: ConversationKind::Direct,
            name: nick,
            joined: true,
            member_ids: vec![member.into()],
            ..Default::default()
        });
        self
    }

    /// Change the last conversation added.
    pub fn with_conversation(mut self, f: impl FnOnce(&mut Conversation)) -> Self {
        f(self
            .snapshot
            .conversations
            .last_mut()
            .expect("a conversation"));
        self
    }

    /// Another member, who joins every channel added after it.
    pub fn member(mut self, id: &str, nick: &str) -> Self {
        self.snapshot.members.push(member(id, nick));
        self
    }

    /// Put a member into a conversation, with channel modes (`"@"`, `""`).
    pub fn in_conversation(mut self, conversation: &str, member: &str, modes: &str) -> Self {
        let c = self
            .snapshot
            .conversations
            .iter_mut()
            .find(|c| c.id == conversation)
            .expect("the conversation was added");
        if !c.member_ids.iter().any(|m| m == member) {
            c.member_ids.push(member.into());
        }
        if !modes.is_empty() {
            c.member_modes.insert(member.into(), modes.into());
        }
        self
    }

    pub fn capabilities(mut self, f: impl FnOnce(&mut Capabilities)) -> Self {
        f(&mut self.snapshot.capabilities);
        self
    }

    pub fn build(self) -> Snapshot {
        self.snapshot
    }

    pub fn workspace(self) -> Workspace {
        Workspace::from_snapshot(self.build())
    }

    fn last_network(&self) -> String {
        self.snapshot
            .networks
            .last()
            .map(|n| n.id.clone())
            .unwrap_or_default()
    }
}

/// A member with a nick and nothing else.
pub fn member(id: &str, nick: &str) -> Member {
    Member {
        id: id.into(),
        nick: nick.into(),
        account: nick.into(),
        status: MemberStatus::Online,
        ..Default::default()
    }
}

/// A connected network with replies, reactions, typing, editing and deleting.
pub fn network(id: &str, name: &str) -> Network {
    Network {
        id: id.into(),
        name: name.into(),
        nick: "me".into(),
        account: Some("me".into()),
        host: "irc.example.org".into(),
        port: 6697,
        tls: true,
        status: NetworkStatus::Connected,
        initials: name.chars().next().map(String::from).unwrap_or_default(),
        features: Features {
            replies: true,
            reactions: true,
            typing: true,
            redact: true,
            edit: true,
            history: true,
            read_markers: true,
            message_line_bytes: 512,
            ..Default::default()
        },
        ..Default::default()
    }
}

/// A plain message in `conversation` from `sender`, at a fixed time.
pub fn message(id: &str, conversation: &str, sender: &str, text: &str) -> Message {
    Message {
        id: id.into(),
        conversation_id: conversation.into(),
        sender_id: sender.into(),
        text: text.into(),
        kind: MessageKind::Message,
        created_at: "2026-11-21T10:00:00Z".into(),
        upstream_id: Some(format!("up-{id}")),
        account: Some(sender.trim_start_matches("m-").to_string()),
        ..Default::default()
    }
}

/// A durable event frame: `{"v":1,"seq":seq,"type":kind,"data":data}`.
pub fn frame(seq: i64, kind: &str, data: &impl Serialize) -> String {
    json!({"v": 1, "seq": seq, "type": kind, "data": data}).to_string()
}

/// An event frame without a sequence number (typing, alerts...).
pub fn ephemeral(kind: &str, data: &impl Serialize) -> String {
    json!({"v": 1, "type": kind, "data": data}).to_string()
}

/// Wait until `condition` holds for the workspace. Panics after `timeout`.
pub fn wait_until(
    client: &Client,
    timeout: Duration,
    what: &str,
    condition: impl Fn(&Workspace) -> bool,
) {
    let deadline = Instant::now() + timeout;
    while !condition(&client.read()) {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Wait until the client is live on its event socket.
pub fn wait_live(client: &Client) {
    wait_until(
        client,
        Duration::from_secs(10),
        "the socket to go live",
        |w| w.connection == Some(Connection::Live),
    );
}

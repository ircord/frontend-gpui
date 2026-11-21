//! Groups: `party.updated`, `party.removed` and group invitations.
//!
//! Owned by package PT1. A group is a conversation whose `party` holds the
//! group (name, members, owner, state, revision); there is no list of groups
//! beside the conversations. Events and answers can arrive out of order (the
//! socket can report a JOIN before the answer to the accept arrives), so
//! every change is fenced by the group's revision, as the web client's
//! `offline.ts` and chat store do: an older revision never undoes a newer
//! one, a removed group stays removed until a newer revision brings it back,
//! and a group converted to a channel stays a channel.
//!
//! Invitations (to a group or a channel) are the workspace's own
//! `invitations`; [`accepted`] and [`declined`] apply the answers.

use std::collections::HashMap;

use ircord_contract::event::{Durable, ServerEvent};
use ircord_contract::{Conversation, Party, PartyMember, PartyState, Snapshot};

use super::{Changes, Workspace};
use crate::format::mentions::fold;

/// The topic this sub-state reports in [`Changes`], by conversation ID.
pub const TOPIC: &str = "parties";

#[derive(Debug, Clone, Default)]
pub struct State {
    /// The newest revision seen for each group, by conversation ID.
    fences: HashMap<String, Fence>,
}

#[derive(Debug, Clone)]
struct Fence {
    revision: i64,
    removed: bool,
    converted: bool,
    /// The group at that revision, to restore over an older conversation.
    party: Option<Party>,
}

impl State {
    /// Whether the group in `conversation` was removed or converted, as far
    /// as this client knows.
    pub fn gone(&self, conversation: &str) -> bool {
        self.fences
            .get(conversation)
            .is_some_and(|f| f.removed || f.converted)
    }
}

/// Called for every fresh event after the workspace's own handling.
pub(super) fn apply(w: &mut Workspace, event: &ServerEvent, _now_ms: i64) -> Changes {
    let ServerEvent::Durable { event, .. } = event else {
        return Changes::default();
    };
    match event {
        Durable::PartyUpdated(party) => put_party(w, party),
        Durable::PartyRemoved(removal) => {
            remove(w, &removal.conversation_id, removal.revision, false)
        }
        // The workspace replaced the conversation; keep a newer group, and
        // keep removed and converted groups that way.
        Durable::ConversationUpdated(c) => refence(w, &c.id),
        _ => Changes::default(),
    }
}

/// Called when a bootstrap replaces the workspace, before it is replaced.
/// The fences stay: a stale snapshot is still older than what was seen.
pub(super) fn reset(_w: &mut Workspace, _snapshot: &Snapshot) -> Changes {
    Changes::default()
}

/// Apply a group the server sent (an event, or the answer to a change).
pub fn put_party(w: &mut Workspace, party: &Party) -> Changes {
    let id = party.conversation_id.clone();
    if let Some(f) = w.parties.fences.get(&id)
        && (f.converted
            || party.revision < f.revision
            || (f.removed && party.revision <= f.revision))
    {
        return Changes::default();
    }
    let converted = party.state == PartyState::Converted;
    w.parties.fences.insert(
        id.clone(),
        Fence {
            revision: party.revision,
            removed: converted,
            converted,
            party: (!converted).then(|| party.clone()),
        },
    );
    if let Some(c) = w.conversations.iter_mut().find(|c| c.id == id) {
        if converted {
            c.party = None;
        } else if c.party.as_ref().is_none_or(|p| p.revision < party.revision) {
            c.party = Some(party.clone());
        }
    }
    changed(&id)
}

/// You left the group in `conversation` (or were taken out): drop it.
pub fn left(w: &mut Workspace, conversation: &str) -> Changes {
    let revision = w
        .conversation(conversation)
        .and_then(|c| c.party.as_ref())
        .map_or(0, |p| p.revision);
    remove(w, conversation, revision, true)
}

/// The server accepted an invitation: show its conversation and drop the
/// invitation. The socket may have brought the conversation first; then
/// only its group is merged, by revision.
pub fn accepted(w: &mut Workspace, invitation: &str, conversation: Conversation) -> Changes {
    w.invitations.retain(|i| i.id != invitation);
    let id = conversation.id.clone();
    let mut changes = changed(&id);
    if w.conversation(&id).is_some() {
        if let Some(party) = &conversation.party {
            changes.merge(put_party(w, party));
        }
    } else {
        // A newer revision than a removal brings the group back.
        if let Some(party) = &conversation.party
            && let Some(f) = w.parties.fences.get(&id)
            && party.revision > f.revision
        {
            w.parties.fences.remove(&id);
        }
        w.conversations.push(conversation);
        changes.merge(refence(w, &id));
        changes.members = true;
    }
    changes
}

/// The server declined an invitation for you.
pub fn declined(w: &mut Workspace, invitation: &str) -> Changes {
    w.invitations.retain(|i| i.id != invitation);
    Changes {
        sidebar: true,
        ..Changes::topic_id(TOPIC, invitation)
    }
}

fn remove(w: &mut Workspace, conversation: &str, revision: i64, force: bool) -> Changes {
    if let Some(f) = w.parties.fences.get(conversation)
        && (f.converted || (!force && revision < f.revision))
    {
        return Changes::default();
    }
    w.parties.fences.insert(
        conversation.to_string(),
        Fence {
            revision,
            removed: true,
            converted: false,
            party: None,
        },
    );
    let before = w.conversations.len();
    w.conversations.retain(|c| {
        c.id != conversation || c.party.as_ref().is_some_and(|p| p.revision > revision)
    });
    if w.conversations.len() == before {
        return Changes::topic_id(TOPIC, conversation);
    }
    w.timelines.remove(conversation);
    w.typing.remove(conversation);
    w.outbox.retain(|p| p.conversation_id != conversation);
    let mut changes = changed(conversation);
    changes.timelines.insert(conversation.to_string());
    changes
}

/// Hold a replaced conversation to what the fence says.
fn refence(w: &mut Workspace, conversation: &str) -> Changes {
    let Some(f) = w.parties.fences.get(conversation).cloned() else {
        return Changes::default();
    };
    let Some(index) = w.conversations.iter().position(|c| c.id == conversation) else {
        return Changes::default();
    };
    let c = &mut w.conversations[index];
    let incoming = c.party.as_ref().map(|p| p.revision);
    if f.converted {
        if c.party.take().is_none() {
            return Changes::default();
        }
    } else if f.removed {
        if incoming.is_some_and(|r| r <= f.revision) {
            w.conversations.remove(index);
            w.timelines.remove(conversation);
        } else {
            return Changes::default();
        }
    } else if let Some(party) = f.party
        && incoming.is_some_and(|r| r < f.revision)
    {
        c.party = Some(party);
    } else {
        return Changes::default();
    }
    changed(conversation)
}

fn changed(conversation: &str) -> Changes {
    Changes {
        sidebar: true,
        ..Changes::topic_id(TOPIC, conversation)
    }
}

/// What a conversation is called in the sidebar and its header: a group's
/// name, else the people in it but you; any other conversation's name.
pub fn title(w: &Workspace, c: &Conversation) -> String {
    let Some(party) = &c.party else {
        return c.name.clone();
    };
    let name = party.name.trim();
    if !name.is_empty() {
        return name.to_string();
    }
    let others: Vec<&str> = party
        .members
        .iter()
        .filter(|m| !is_me(w, party, m))
        .map(member_name)
        .collect();
    if others.is_empty() {
        c.name.clone()
    } else {
        others.join(", ")
    }
}

/// How a member is named: display name, else services account, else username.
pub fn member_name(m: &PartyMember) -> &str {
    [&m.display_name, &m.services_account, &m.username]
        .into_iter()
        .find(|s| !s.is_empty())
        .map_or("", |s| s.as_str())
}

/// The ID the server knows a member by, for removing or promoting them.
pub fn member_id(m: &PartyMember) -> &str {
    if m.id.is_empty() {
        m.account_id.as_deref().unwrap_or("")
    } else {
        &m.id
    }
}

/// Whether `m` is you: your IRCord account, or your services account on the
/// group's network.
pub fn is_me(w: &Workspace, party: &Party, m: &PartyMember) -> bool {
    let account = m.backend_account_id.as_deref().or(m.account_id.as_deref());
    if !w.user.id.is_empty() && account == Some(w.user.id.as_str()) {
        return true;
    }
    let Some(network) = w.network(&party.network_id) else {
        return false;
    };
    let own = network.account.as_deref().unwrap_or("");
    let mapping = network
        .isupport
        .get("CASEMAPPING")
        .map_or("rfc1459", String::as_str);
    !own.is_empty()
        && !m.services_account.is_empty()
        && fold(own, mapping) == fold(&m.services_account, mapping)
}

/// Whether `m` owns the group.
pub fn is_owner(party: &Party, m: &PartyMember) -> bool {
    member_id(m) == party.owner_id
}

/// Whether you own the group.
pub fn owned(w: &Workspace, party: &Party) -> bool {
    party
        .members
        .iter()
        .any(|m| is_owner(party, m) && is_me(w, party, m))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{SnapshotBuilder, frame};
    use ircord_contract::{Invitation, PartyInvitation, PartyRemoval};

    fn member(id: &str, account: &str, name: &str) -> PartyMember {
        PartyMember {
            id: id.into(),
            backend_account_id: Some(account.into()),
            display_name: name.into(),
            services_account: name.to_lowercase(),
            username: name.to_lowercase(),
            ..Default::default()
        }
    }

    fn party(revision: i64, name: &str) -> Party {
        Party {
            id: "p1".into(),
            conversation_id: "c-group".into(),
            network_id: "n1".into(),
            name: name.into(),
            owner_id: "pm-me".into(),
            revision,
            state: PartyState::Active,
            members: vec![
                member("pm-me", "u-me", "Me"),
                member("pm-bob", "u-bob", "Bob"),
                member("pm-eve", "u-eve", "Eve"),
            ],
            ..Default::default()
        }
    }

    fn group(revision: i64, name: &str) -> Conversation {
        Conversation {
            id: "c-group".into(),
            network_id: "n1".into(),
            kind: ircord_contract::ConversationKind::Channel,
            name: name.into(),
            joined: true,
            party: Some(party(revision, name)),
            ..Default::default()
        }
    }

    fn workspace() -> Workspace {
        let mut w = SnapshotBuilder::new()
            .channel("c-general", "general")
            .cursor(1)
            .workspace();
        w.user.id = "u-me".into();
        w.conversations.push(group(2, "Friends"));
        w
    }

    fn event(w: &mut Workspace, seq: i64, kind: &str, data: &impl serde::Serialize) -> Changes {
        let event = ServerEvent::decode(&frame(seq, kind, data)).unwrap();
        w.apply(&event, 0)
    }

    fn party_of(w: &Workspace) -> Option<&Party> {
        w.conversation("c-group").and_then(|c| c.party.as_ref())
    }

    #[test]
    fn a_newer_group_replaces_the_conversations_and_an_older_one_does_not() {
        let mut w = workspace();
        let changes = event(&mut w, 2, "party.updated", &party(3, "Pals"));
        assert!(changes.sidebar);
        assert!(changes.touches_id(TOPIC, "c-group"));
        assert_eq!(party_of(&w).unwrap().name, "Pals");
        event(&mut w, 3, "party.updated", &party(1, "Old"));
        assert_eq!(party_of(&w).unwrap().name, "Pals");
        assert_eq!(party_of(&w).unwrap().revision, 3);
    }

    #[test]
    fn a_removed_group_leaves_the_sidebar_and_stays_gone() {
        let mut w = workspace();
        let removal = PartyRemoval {
            id: "p1".into(),
            conversation_id: "c-group".into(),
            revision: 4,
        };
        let changes = event(&mut w, 2, "party.removed", &removal);
        assert!(changes.sidebar);
        assert!(w.conversation("c-group").is_none());
        assert!(w.parties.gone("c-group"));
        // An update at or below the removal does not bring it back…
        event(&mut w, 3, "party.updated", &party(4, "Back"));
        assert!(w.conversation("c-group").is_none());
        // …and a conversation update with that group is dropped again.
        event(&mut w, 4, "conversation.updated", &group(4, "Back"));
        assert!(w.conversation("c-group").is_none());
    }

    #[test]
    fn an_older_removal_does_not_remove_a_newer_group() {
        let mut w = workspace();
        let removal = PartyRemoval {
            id: "p1".into(),
            conversation_id: "c-group".into(),
            revision: 1,
        };
        event(&mut w, 2, "party.removed", &removal);
        assert!(w.conversation("c-group").is_some());
    }

    #[test]
    fn a_converted_group_becomes_a_channel_for_good() {
        let mut w = workspace();
        let mut converted = party(5, "Friends");
        converted.state = PartyState::Converted;
        event(&mut w, 2, "party.updated", &converted);
        let c = w.conversation("c-group").unwrap();
        assert!(c.party.is_none(), "it is an ordinary channel now");
        event(&mut w, 3, "party.updated", &party(6, "Again"));
        assert!(w.conversation("c-group").unwrap().party.is_none());
        event(&mut w, 4, "conversation.updated", &group(7, "Again"));
        assert!(w.conversation("c-group").unwrap().party.is_none());
    }

    #[test]
    fn a_conversation_update_keeps_a_newer_group() {
        let mut w = workspace();
        event(&mut w, 2, "party.updated", &party(5, "Newer"));
        event(&mut w, 3, "conversation.updated", &group(3, "Stale"));
        assert_eq!(party_of(&w).unwrap().name, "Newer");
    }

    #[test]
    fn accepting_adds_the_conversation_once_and_drops_the_invitation() {
        let mut w = workspace();
        w.conversations.retain(|c| c.id != "c-group");
        w.invitations.push(Invitation {
            id: "i1".into(),
            network_id: "n1".into(),
            party: Some(PartyInvitation {
                id: "p1".into(),
                name: "Friends".into(),
                ..Default::default()
            }),
            ..Default::default()
        });
        let changes = accepted(&mut w, "i1", group(2, "Friends"));
        assert!(changes.sidebar);
        assert!(w.invitations.is_empty());
        assert_eq!(party_of(&w).unwrap().revision, 2);
        // The socket brought it too: no second conversation.
        accepted(&mut w, "i1", group(3, "Friends"));
        assert_eq!(
            w.conversations.iter().filter(|c| c.id == "c-group").count(),
            1
        );
        assert_eq!(party_of(&w).unwrap().revision, 3);
    }

    #[test]
    fn declining_drops_the_invitation() {
        let mut w = workspace();
        w.invitations.push(Invitation {
            id: "i1".into(),
            channel: "#secret".into(),
            ..Default::default()
        });
        assert!(declined(&mut w, "i1").sidebar);
        assert!(w.invitations.is_empty());
    }

    #[test]
    fn leaving_drops_the_conversation_even_at_the_same_revision() {
        let mut w = workspace();
        left(&mut w, "c-group");
        assert!(w.conversation("c-group").is_none());
        assert!(w.parties.gone("c-group"));
    }

    #[test]
    fn titles_name_the_group_or_the_people_in_it_but_you() {
        let mut w = workspace();
        let c = w.conversation("c-group").unwrap().clone();
        assert_eq!(title(&w, &c), "Friends");
        let mut unnamed = c.clone();
        unnamed.party.as_mut().unwrap().name = "  ".into();
        assert_eq!(title(&w, &unnamed), "Bob, Eve");
        // Known by services account on the network, not the IRCord account.
        w.user.id = "someone-else".into();
        w.networks[0].account = Some("ME".into());
        assert_eq!(title(&w, &unnamed), "Bob, Eve");
        let general = w.conversation("c-general").unwrap().clone();
        assert_eq!(title(&w, &general), "general");
    }

    #[test]
    fn ownership_is_by_member_id() {
        let w = workspace();
        let p = party(1, "x");
        assert!(owned(&w, &p));
        assert!(is_owner(&p, &p.members[0]));
        assert!(!is_owner(&p, &p.members[1]));
        let mut theirs = p.clone();
        theirs.owner_id = "pm-bob".into();
        assert!(!owned(&w, &theirs));
        assert_eq!(member_id(&p.members[1]), "pm-bob");
        assert_eq!(member_name(&p.members[1]), "Bob");
    }
}

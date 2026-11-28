//! Encrypted chats carried over IRC (`/api/v1/e2ee/chats`, phase 2 of
//! ircord/issues#19): direct chats with someone whose keys are on another
//! IRCord server of the home network, and encrypted groups.
//!
//! The backend moves the handshake objects over IRC and orders commits by
//! the IRC server's stamp; this device never takes its word on which commit
//! won. For each epoch it tries the candidates in position order and applies
//! the first one its core accepts (`docs/wire.md` 7.5), its own pending
//! commit among them. Only a commit that was never sent is dropped.
//!
//! A group is an invite-only channel named after its random ID. Its title,
//! creator and admins live inside MLS, so only members see them. The core
//! enforces who may change what; the views offer only what it allows.

use std::collections::{BTreeMap, BTreeSet};

use ircord_contract as c;

use super::super::core::{Accepted, Body, Commit, ControlOp, Conversation as CoreConversation};
use super::super::core::{DeviceRef, GroupIdBytes, Payload};
use super::super::vault::{Conversation, Invitee, Record};
use super::{Engine, Inner, Result, api_err, core_err, group_hex, group_of, key32, view};
use crate::api::ApiError;
use crate::api::e2ee::{bytes, encode};
use crate::store::Changes;
use crate::store::e2ee::{Group, GroupEvent, GroupInvitation, GroupMember};

/// How many group changes a group's history keeps.
const EVENTS_KEPT: usize = 50;

/// One chat as the backend lists it.
#[derive(Clone, Debug)]
pub(super) struct Chat {
    /// The backend's name for it, as its creator wrote the group ID.
    pub name: String,
    pub kind: c::E2EEChatKind,
    pub state: c::E2EEChatState,
    pub conversation: Option<String>,
    /// A direct chat's other person.
    pub peer: Option<String>,
    pub declined: Vec<String>,
}

/// What this device keeps about its groups between syncs.
#[derive(Default)]
pub(super) struct Chats {
    /// The backend's chats, by group ID bytes.
    pub list: BTreeMap<Vec<u8>, Chat>,
    /// Whether the list was read in this session.
    pub read: bool,
    /// Accounts whose identity key in a group differs from this device's
    /// pin, by group.
    pub fresh: BTreeMap<Vec<u8>, BTreeSet<String>>,
    /// Recent changes, by group.
    pub events: BTreeMap<Vec<u8>, Vec<GroupEvent>>,
    /// Who invited this device, as MLS says (not the backend), by group.
    pub inviters: BTreeMap<Vec<u8>, String>,
    /// (group, account) declines this device already acted on.
    pub declines_handled: BTreeSet<(Vec<u8>, String)>,
    /// People this account invited before they had encryption keys, by the
    /// group's backend name. Kept in the vault's `pending.sealed`.
    pub pending: BTreeMap<String, Vec<Invitee>>,
    /// When the pending invitees' keys were last looked for.
    pub pending_checked: Option<std::time::Instant>,
}

/// How often the periodic tick looks for pending invitees' keys.
const PENDING_EVERY: std::time::Duration = std::time::Duration::from_secs(120);

/// The name a new chat goes by on the backend: groups need base64url
/// (their channel is derived from it), direct chats use the specification's
/// hex form. Either way every device uses the name the creator chose.
fn new_name(g: &GroupIdBytes, group: bool) -> String {
    if group {
        super::super::b64url(&g.0)
    } else {
        group_hex(g)
    }
}

fn accounts_of(members: &[DeviceRef]) -> BTreeMap<String, usize> {
    let mut out = BTreeMap::new();
    for m in members {
        *out.entry(m.account.clone()).or_insert(0) += 1;
    }
    out
}

/// The changes between two member lists, as history lines.
fn membership_events(by: &str, before: &[DeviceRef], after: &[DeviceRef]) -> Vec<GroupEvent> {
    let (old, new) = (accounts_of(before), accounts_of(after));
    let mut out = Vec::new();
    for (account, n) in &new {
        match old.get(account) {
            None => out.push(GroupEvent::Added {
                by: by.to_string(),
                account: account.clone(),
            }),
            Some(o) if n > o => out.push(GroupEvent::DeviceAdded {
                account: account.clone(),
            }),
            Some(o) if n < o => out.push(GroupEvent::DeviceRemoved {
                account: account.clone(),
            }),
            _ => {}
        }
    }
    for account in old.keys().filter(|a| !new.contains_key(*a)) {
        out.push(GroupEvent::Removed {
            by: by.to_string(),
            account: account.clone(),
        });
    }
    out
}

impl Engine {
    /// The backend's chat for a group, if it is carried over IRC.
    pub(super) fn chat<'a>(inner: &'a Inner, g: &GroupIdBytes) -> Option<&'a Chat> {
        inner.chats.list.get(&g.0)
    }

    /// Read the backend's list of chats. Returns whether it was read.
    pub(super) async fn read_chats(&self, inner: &mut Inner) -> bool {
        let groups = self.shared.handle.read().capabilities.encryption.is_some();
        if !groups {
            return false;
        }
        match self.api().e2ee_chats().await {
            Ok(items) => {
                inner.chats.list = items
                    .into_iter()
                    .filter_map(|chat| {
                        let g = group_of(&chat.group_id).ok()?;
                        Some((
                            g.0,
                            Chat {
                                name: chat.group_id,
                                kind: chat.kind,
                                state: chat.state,
                                conversation: chat.conversation_id,
                                peer: chat.peer,
                                declined: chat.declined,
                            },
                        ))
                    })
                    .collect();
                inner.chats.read = true;
                true
            }
            // A backend from before phase 2: nothing is carried over IRC.
            Err(ApiError::Http { status: 404, .. }) => {
                inner.chats.read = true;
                true
            }
            Err(e) => {
                log::debug!("e2ee: list chats: {e}");
                false
            }
        }
    }

    /// Process the Welcomes that arrived over IRC, settle every chat's
    /// commits, and show the groups. Returns whether everything was read.
    pub(super) async fn sync_chats(&self, inner: &mut Inner) -> bool {
        if !inner
            .device
            .as_ref()
            .is_some_and(|d| d.certificate().is_some())
        {
            return false;
        }
        let mut complete = self.read_chats(inner).await;
        complete &= self.chat_welcomes(inner).await;
        let groups: Vec<GroupIdBytes> = inner
            .chats
            .list
            .keys()
            .map(|g| GroupIdBytes(g.clone()))
            .filter(|g| inner.device.as_ref().is_some_and(|d| d.epoch(g).is_ok()))
            .collect();
        for g in &groups {
            complete &= self.settle(inner, g).await.is_ok();
        }
        self.remove_declined(inner).await;
        self.publish_groups(inner);
        complete
    }

    /// Join what Welcomes invite this device to. Direct chats are joined at
    /// once, as in phase 1; a group waits for the person's answer.
    async fn chat_welcomes(&self, inner: &mut Inner) -> bool {
        let Some(me) = inner.device.as_ref().map(|d| d.device_id().to_hex()) else {
            return false;
        };
        let welcomes = match self.api().e2ee_chat_welcomes(&me).await {
            Ok(w) => w,
            Err(ApiError::Http { status: 404, .. }) => return true,
            Err(e) => {
                log::debug!("e2ee: list chat Welcomes: {e}");
                return false;
            }
        };
        for welcome in welcomes {
            let Ok(blob) = bytes(&welcome.welcome) else {
                let _ = self.api().e2ee_delete_chat_welcome(welcome.id).await;
                continue;
            };
            let device = inner.device.as_mut().unwrap();
            // The backend hands a new chat's Welcome to every device of the
            // account that started it, this one too when it started it.
            if group_of(&welcome.group_id).is_ok_and(|g| device.epoch(&g).is_ok()) {
                let _ = self.api().e2ee_delete_chat_welcome(welcome.id).await;
                continue;
            }
            let from_self = welcome.sender.eq_ignore_ascii_case(&inner.account);
            match welcome.kind {
                c::E2EEChatWelcomeKind::Direct => match device.join_dm(&blob) {
                    Ok(joined) => {
                        // The Welcome must be for the chat it is filed under,
                        // from the person whose chat it is, or from another
                        // device of this account that started this account's
                        // chat with that same person.
                        let sender_ok = if from_self {
                            inner
                                .chats
                                .list
                                .get(&joined.group_id.0)
                                .and_then(|c| c.peer.as_deref())
                                == Some(joined.peer.as_str())
                        } else {
                            joined.peer == welcome.sender
                        };
                        if !sender_ok
                            || group_of(&welcome.group_id).ok() != Some(joined.group_id.clone())
                        {
                            // Someone else's group passed off as this sender's.
                            let _ = device.forget_group(&joined.group_id);
                        } else if let Err(e) = self.api().e2ee_join_chat(&welcome.group_id).await {
                            log::warn!("e2ee: join a direct chat: {e}");
                        }
                    }
                    Err(super::CoreError::IdentityChanged {
                        account, presented, ..
                    }) => {
                        // Kept until the person acknowledges the change.
                        let conversation = {
                            let w = self.shared.handle.read();
                            w.conversations
                                .iter()
                                .find(|c| {
                                    super::peer_of(&w, &c.id).ok().as_deref() == Some(&account)
                                })
                                .map(|c| c.id.clone())
                        };
                        if let Some(conversation) = conversation {
                            self.attention(&conversation, &account, presented);
                        }
                        continue;
                    }
                    Err(e) => log::warn!("e2ee: a direct Welcome was refused: {e}"),
                },
                c::E2EEChatWelcomeKind::Group => match device.join_group(&blob) {
                    Ok(joined) => {
                        if joined.inviter.account != welcome.sender
                            || group_of(&welcome.group_id).ok() != Some(joined.group_id.clone())
                        {
                            let _ = device.forget_group(&joined.group_id);
                        } else {
                            inner
                                .chats
                                .inviters
                                .insert(joined.group_id.0.clone(), joined.inviter.account.clone());
                            let fresh: BTreeSet<String> = joined
                                .identity_changes
                                .iter()
                                .map(|c| c.account.clone())
                                .collect();
                            if !fresh.is_empty() {
                                inner.chats.fresh.insert(joined.group_id.0.clone(), fresh);
                            }
                        }
                    }
                    Err(e) => log::warn!("e2ee: a group Welcome was refused: {e}"),
                },
                c::E2EEChatWelcomeKind::Unknown => {}
            }
            if let Err(e) = self.api().e2ee_delete_chat_welcome(welcome.id).await {
                log::debug!("e2ee: remove a chat Welcome: {e}");
            }
        }
        // A Welcome may have changed which chats are joined.
        self.read_chats(inner).await;
        true
    }

    /// Apply the commits of one chat, epoch by epoch: of each epoch's
    /// candidates, the first the core accepts. Returns whether this device's
    /// own pending commit was among those applied.
    pub(super) async fn settle(&self, inner: &mut Inner, g: &GroupIdBytes) -> Result<bool> {
        let name = Self::chat(inner, g)
            .map(|c| c.name.clone())
            .ok_or("not-a-chat")?;
        let mut own = false;
        for _ in 0..64 {
            let device = inner.device.as_mut().ok_or("not-ready")?;
            let Ok(epoch) = device.epoch(g) else {
                return Ok(own);
            };
            let commits = self
                .api()
                .e2ee_chat_commits(&name, epoch as i64)
                .await
                .map_err(api_err)?;
            let candidates: Vec<Vec<u8>> = commits
                .iter()
                .filter(|c| c.epoch == epoch as i64)
                .filter_map(|c| bytes(&c.commit).ok())
                .collect();
            if candidates.is_empty() {
                return Ok(own);
            }
            let device = inner.device.as_mut().ok_or("not-ready")?;
            let before = device.members(g).unwrap_or_default();
            let settings_before = device.group_settings(g).ok();
            let resolution = device.apply_candidates(g, &candidates).map_err(core_err)?;
            for (i, e) in &resolution.rejected {
                log::info!("e2ee: candidate {i} for epoch {epoch} refused: {e}");
            }
            let Some((_, accepted)) = resolution.accepted else {
                // None of them follows from this device's state: it lost
                // step. What it missed stays unread here.
                log::warn!("e2ee: no candidate for epoch {epoch} applies");
                return Ok(own);
            };
            let (by, removed_self, changes) = match accepted {
                Accepted::Own { .. } => {
                    own = true;
                    (inner.account.clone(), false, Vec::new())
                }
                Accepted::Applied(a) => (a.committer.account, a.removed_self, a.identity_changes),
            };
            if removed_self {
                let device = inner.device.as_mut().ok_or("not-ready")?;
                let _ = device.forget_group(g);
                self.forget_chat(inner, g).await;
                return Ok(own);
            }
            if !changes.is_empty() {
                let fresh = inner.chats.fresh.entry(g.0.clone()).or_default();
                fresh.extend(changes.into_iter().map(|c| c.account));
            }
            let device = inner.device.as_ref().ok_or("not-ready")?;
            let after = device.members(g).unwrap_or_default();
            let mut events = membership_events(&by, &before, &after);
            if let (Some(old), Ok(new)) = (settings_before, device.group_settings(g)) {
                let was: BTreeSet<&str> = old.admins.iter().map(|a| a.account.as_str()).collect();
                let is: BTreeSet<&str> = new.admins.iter().map(|a| a.account.as_str()).collect();
                for a in is.difference(&was) {
                    events.push(GroupEvent::Admin {
                        by: by.clone(),
                        account: a.to_string(),
                        admin: true,
                    });
                }
                for a in was.difference(&is) {
                    if *a != new.creator.account {
                        events.push(GroupEvent::Admin {
                            by: by.clone(),
                            account: a.to_string(),
                            admin: false,
                        });
                    }
                }
            }
            self.note(inner, g, events);
        }
        Ok(own)
    }

    fn note(&self, inner: &mut Inner, g: &GroupIdBytes, events: Vec<GroupEvent>) {
        if events.is_empty() {
            return;
        }
        let list = inner.chats.events.entry(g.0.clone()).or_default();
        list.extend(events);
        let excess = list.len().saturating_sub(EVENTS_KEPT);
        list.drain(..excess);
    }

    /// Send a commit this device made and settle the epoch. Returns whether
    /// it won; one that was never sent is dropped.
    pub(super) async fn send_commit(
        &self,
        inner: &mut Inner,
        g: &GroupIdBytes,
        commit: Commit,
        body: c::E2EEChatCommitInputBody,
    ) -> Result<bool> {
        let name = Self::chat(inner, g)
            .map(|c| c.name.clone())
            .ok_or("not-a-chat")?;
        let body = c::E2EEChatCommitInputBody {
            epoch: commit.epoch as i64,
            commit: encode(&commit.commit),
            welcome: commit.welcome.as_deref().map(encode),
            ..body
        };
        match self.api().e2ee_chat_commit(&name, &body).await {
            Ok(_) => {}
            // Refused before anything went out (too large, malformed, not a
            // chat of this account, not joined): it was never sent.
            Err(
                e @ ApiError::Http {
                    status: 404 | 409 | 413 | 422,
                    ..
                },
            ) => {
                if let Some(d) = inner.device.as_mut() {
                    let _ = d.discard_commit(g);
                }
                return Err(api_err(e));
            }
            // It may have gone out: it stays pending, and the candidates
            // decide at this sync or a later one.
            Err(e) => {
                log::info!("e2ee: send a commit: {e}");
                return Err(api_err(e));
            }
        }
        // A commit another device stamped first drops this one there.
        self.settle(inner, g).await
    }

    /// This account's own record of being in a chat ends: the backend's,
    /// the vault's and the view's.
    async fn forget_chat(&self, inner: &mut Inner, g: &GroupIdBytes) {
        let chat = inner.chats.list.remove(&g.0);
        inner.chats.fresh.remove(&g.0);
        inner.chats.events.remove(&g.0);
        let conversation = chat.as_ref().and_then(|c| c.conversation.clone());
        if let Some(chat) = &chat
            && chat.kind == c::E2EEChatKind::Group
        {
            let _ = self.api().e2ee_leave_chat(&chat.name, false).await;
        }
        if let Some(conversation) = conversation {
            if let Some(vault) = inner.vault.as_mut() {
                let _ = vault.put(Record::Forget {
                    conversation: conversation.clone(),
                });
            }
            self.change(|w| {
                w.e2ee.groups.remove(&conversation);
                w.e2ee.conversations.remove(&conversation);
                Changes {
                    sidebar: true,
                    ..Changes::topic_id(view::TOPIC, conversation)
                }
            });
        }
    }

    /// Show every group this device is in, and the invitations.
    pub(super) fn publish_groups(&self, inner: &mut Inner) {
        let Some(device) = inner.device.as_ref() else {
            return;
        };
        let me = device.device_id().to_hex();
        let mut groups = Vec::new();
        let mut invitations = Vec::new();
        let mut records = Vec::new();
        for conv in device.conversations().unwrap_or_default() {
            let CoreConversation::Group { group_id, settings } = conv else {
                continue;
            };
            let Some(chat) = Self::chat(inner, &group_id) else {
                continue;
            };
            let members = device.members(&group_id).unwrap_or_default();
            let admins: BTreeSet<String> = device
                .admins(&group_id)
                .unwrap_or_default()
                .into_iter()
                .map(|a| a.account)
                .collect();
            let fresh = inner.chats.fresh.get(&group_id.0);
            let mut people: BTreeMap<String, GroupMember> = BTreeMap::new();
            for m in &members {
                let entry = people
                    .entry(m.account.clone())
                    .or_insert_with(|| GroupMember {
                        account: m.account.clone(),
                        you: m.account == inner.account,
                        admin: admins.contains(&m.account),
                        creator: settings.creator.account == m.account,
                        verified: device.trust().get(&m.account).is_some_and(|p| p.verified),
                        started_fresh: fresh.is_some_and(|f| f.contains(&m.account)),
                        devices: Vec::new(),
                    });
                let id = m.device_id.to_hex();
                entry
                    .devices
                    .push((id.clone(), m.account == inner.account && id == me));
            }
            if chat.state == c::E2EEChatState::Invited {
                invitations.push(GroupInvitation {
                    chat: chat.name.clone(),
                    title: settings.title.clone(),
                    inviter: inner
                        .chats
                        .inviters
                        .get(&group_id.0)
                        .cloned()
                        .unwrap_or_default(),
                    people: people.len(),
                });
                continue;
            }
            let Some(conversation) = chat.conversation.clone() else {
                continue;
            };
            // Only the group's own channel, `#e2ee-<hex ID>`: a server must
            // not dress another conversation up as the group.
            let expected = format!("e2ee-{}", group_hex(&group_id));
            let own_channel = self
                .shared
                .handle
                .read()
                .conversation(&conversation)
                .is_some_and(|c| {
                    c.kind == c::ConversationKind::Channel
                        && c.name
                            .trim_start_matches('#')
                            .eq_ignore_ascii_case(&expected)
                });
            if !own_channel {
                log::warn!("e2ee: the server names another conversation as a group's channel");
                continue;
            }
            let group = Group {
                chat: chat.name.clone(),
                title: settings.title.clone(),
                admin: admins.contains(&inner.account),
                creator: settings.creator.account == inner.account,
                members: people.into_values().collect(),
                events: inner
                    .chats
                    .events
                    .get(&group_id.0)
                    .cloned()
                    .unwrap_or_default(),
                pending: inner
                    .chats
                    .pending
                    .get(&chat.name)
                    .map(|list| list.iter().map(|i| i.account.clone()).collect())
                    .unwrap_or_default(),
            };
            records.push((conversation, chat.name.clone()));
            groups.push(group);
        }
        // Every joined group's channel is encrypted: nothing in the clear.
        for (conversation, name) in &records {
            let known = inner
                .vault
                .as_ref()
                .and_then(|v| v.conversation(conversation))
                .is_some_and(|c| c.on && c.group.as_deref() == Some(name.as_str()));
            if !known && let Some(vault) = inner.vault.as_mut() {
                let record = Conversation {
                    conversation: conversation.clone(),
                    peer: String::new(),
                    group: Some(name.clone()),
                    on: true,
                    since: chrono::Utc::now().timestamp_millis(),
                    by: String::new(),
                };
                if let Err(e) = vault.put(Record::Conversation(record)) {
                    log::warn!("e2ee: store a group: {e}");
                }
            }
        }
        self.change(|w| {
            let mut changes = Changes::topic(view::TOPIC);
            let old: Vec<String> = w.e2ee.groups.keys().cloned().collect();
            let mut by_conversation = std::collections::HashMap::new();
            for ((conversation, _), group) in records.iter().zip(groups) {
                let unencrypted = w
                    .e2ee
                    .conversations
                    .get(conversation)
                    .is_some_and(|c| c.unencrypted);
                let entry = w
                    .e2ee
                    .conversations
                    .entry(conversation.clone())
                    .or_default();
                entry.on = true;
                entry.unencrypted = unencrypted;
                if w.e2ee.groups.get(conversation) != Some(&group) {
                    changes.sidebar = true;
                }
                by_conversation.insert(conversation.clone(), group);
            }
            for gone in old.iter().filter(|c| !by_conversation.contains_key(*c)) {
                w.e2ee.conversations.remove(gone);
                changes.sidebar = true;
            }
            w.e2ee.groups = by_conversation;
            w.e2ee.invitations = invitations;
            changes
        });
    }

    /// Whether this account acts for the group's admins now: an admin, and
    /// the first of the admins (by account) when several are there, so that
    /// one of them removes someone who left or declined.
    fn acting_admin(inner: &Inner, g: &GroupIdBytes, leaving: &str) -> bool {
        let Some(device) = inner.device.as_ref() else {
            return false;
        };
        let admins = device.admins(g).unwrap_or_default();
        admins
            .iter()
            .map(|a| a.account.as_str())
            .filter(|a| *a != leaving)
            .min()
            == Some(inner.account.as_str())
    }

    fn is_admin(inner: &Inner, g: &GroupIdBytes) -> bool {
        inner.device.as_ref().is_some_and(|d| {
            d.admins(g)
                .unwrap_or_default()
                .iter()
                .any(|a| a.account == inner.account)
        })
    }

    /// Accounts that declined this account's invitations leave the group.
    async fn remove_declined(&self, inner: &mut Inner) {
        let todo: Vec<(GroupIdBytes, Vec<String>)> = inner
            .chats
            .list
            .iter()
            .filter(|(_, c)| c.kind == c::E2EEChatKind::Group && !c.declined.is_empty())
            .map(|(g, c)| (GroupIdBytes(g.clone()), c.declined.clone()))
            .collect();
        for (g, declined) in todo {
            // The declines are this account's own invitations': it acts on
            // them while it may, once each.
            if !Self::is_admin(inner, &g) {
                continue;
            }
            let declined: Vec<String> = declined
                .into_iter()
                .filter(|a| {
                    inner
                        .chats
                        .declines_handled
                        .insert((g.0.clone(), a.clone()))
                })
                .collect();
            let present: BTreeSet<String> = inner
                .device
                .as_ref()
                .and_then(|d| d.members(&g).ok())
                .unwrap_or_default()
                .into_iter()
                .map(|m| m.account)
                .collect();
            let gone: Vec<String> = declined
                .into_iter()
                .filter(|a| present.contains(a) && *a != inner.account)
                .collect();
            if !gone.is_empty()
                && let Err(e) = self.remove_now(inner, &g, &gone).await
            {
                log::info!("e2ee: remove who declined: {e}");
            }
        }
    }

    /// Someone sent `left` in a group: the acting admin removes them.
    pub(super) async fn someone_left(&self, inner: &mut Inner, g: &GroupIdBytes, account: &str) {
        self.note(
            inner,
            g,
            vec![GroupEvent::Left {
                account: account.to_string(),
            }],
        );
        if account != inner.account && Self::acting_admin(inner, g, account) {
            let still = inner
                .device
                .as_ref()
                .and_then(|d| d.members(g).ok())
                .is_some_and(|m| m.iter().any(|d| d.account == account));
            if still && let Err(e) = self.remove_now(inner, g, &[account.to_string()]).await {
                log::info!("e2ee: remove who left: {e}");
            }
        }
        self.publish_groups(inner);
    }

    // ---- What the views ask for ----

    /// Run a group action in the background, showing it busy and its error.
    fn group_action<F>(&self, key: &str, f: F)
    where
        F: for<'a> FnOnce(
                &'a Engine,
                &'a mut Inner,
            ) -> std::pin::Pin<
                Box<dyn std::future::Future<Output = Result<()>> + Send + 'a>,
            > + Send
            + 'static,
    {
        let key = key.to_string();
        self.change(|w| {
            w.e2ee.group_busy.insert(key.clone());
            w.e2ee.group_errors.remove(&key);
            Changes::topic(view::TOPIC)
        });
        self.spawn(move |engine| async move {
            let mut inner = engine.shared.inner.lock().await;
            let result = f(&engine, &mut inner).await;
            engine.publish_groups(&mut inner);
            engine.change(|w| {
                w.e2ee.group_busy.remove(&key);
                if let Err(e) = result {
                    w.e2ee.group_errors.insert(key.clone(), e);
                }
                Changes::topic(view::TOPIC)
            });
        });
    }

    /// Start an encrypted group titled `title` with these accounts.
    pub fn create_group(&self, title: &str, accounts: Vec<String>) {
        let title = title.trim().to_string();
        self.group_action("new", move |engine, inner| {
            Box::pin(async move {
                engine
                    .create_group_now(inner, &title, &accounts, Vec::new())
                    .await
            })
        });
    }

    /// Start an encrypted group with these people. Those who have no
    /// encryption keys yet are invited instead: each gets [`Texts::invited`]
    /// as an ordinary message, and is added once their keys appear.
    ///
    /// [`Texts::invited`]: super::Texts::invited
    pub fn create_group_inviting(&self, title: &str, people: Vec<Invitee>) {
        let title = title.trim().to_string();
        self.group_action("new", move |engine, inner| {
            Box::pin(async move {
                let (accounts, pending) = engine.split_by_keys(people).await?;
                engine
                    .create_group_now(inner, &title, &accounts, pending)
                    .await
            })
        });
    }

    /// Add people to the group whose channel is `conversation`, inviting
    /// those without encryption keys as [`Engine::create_group_inviting`]
    /// does.
    pub fn add_people_inviting(&self, conversation: &str, people: Vec<Invitee>) {
        let conversation = conversation.to_string();
        let key = self.group_key(&conversation);
        self.group_action(&key, move |engine, inner| {
            Box::pin(async move {
                let g = engine.group_in(inner, &conversation)?;
                let (accounts, mut pending) = engine.split_by_keys(people).await?;
                engine.add_now(inner, &g, accounts).await?;
                let name = inner.chats.list[&g.0].name.clone();
                let known = inner.chats.pending.get(&name).cloned().unwrap_or_default();
                pending.retain(|i| !known.iter().any(|p| p.account == i.account));
                if !pending.is_empty() {
                    engine.tell_invited(&pending).await;
                    inner.chats.pending.entry(name).or_default().extend(pending);
                    engine.save_pending(inner);
                }
                Ok(())
            })
        });
    }

    /// Who of `people` has encryption keys (their accounts), and who not.
    async fn split_by_keys(&self, people: Vec<Invitee>) -> Result<(Vec<String>, Vec<Invitee>)> {
        let (mut ready, mut pending) = (Vec::new(), Vec::new());
        for person in people {
            match self.api().e2ee_account(&person.account).await {
                Ok(d) if !d.devices.is_empty() => ready.push(person.account),
                Ok(_) | Err(ApiError::Http { status: 404, .. }) => pending.push(person),
                Err(ApiError::Http { status: 504, .. }) => {
                    return Err(format!("peer-no-answer:{}", person.account));
                }
                Err(e) => return Err(api_err(e)),
            }
        }
        Ok((ready, pending))
    }

    async fn create_group_now(
        &self,
        inner: &mut Inner,
        title: &str,
        accounts: &[String],
        mut pending: Vec<Invitee>,
    ) -> Result<()> {
        let mut people: Vec<String> = accounts
            .iter()
            .map(|a| a.trim().to_string())
            .filter(|a| !a.is_empty() && *a != inner.account)
            .collect();
        people.sort();
        people.dedup();
        pending.retain(|i| {
            !i.account.is_empty() && i.account != inner.account && !people.contains(&i.account)
        });
        pending.sort_by(|a, b| a.account.cmp(&b.account));
        pending.dedup_by(|a, b| a.account == b.account);
        if people.is_empty() && pending.is_empty() {
            return Err("group-needs-people".into());
        }
        // The core starts a group only with someone else in it.
        if people.is_empty() {
            return Err("group-nobody-ready".into());
        }
        let mut packages = self.packages_for(inner, &people).await?;
        packages.extend(self.own_packages(inner).await?);
        let device = inner.device.as_mut().ok_or("not-ready")?;
        let created = device.create_group(title, &packages).map_err(group_err)?;
        let g = created.group_id.clone();
        let name = new_name(&g, true);
        let body = c::E2EECreateChatInputBody {
            group_id: name.clone(),
            kind: c::E2EECreateChatInputBodyKind::Group,
            members: Some(people.clone()),
            epoch: device.epoch(&g).unwrap_or(1) as i64,
            welcome: Some(encode(&created.welcome)),
            ..Default::default()
        };
        let made = match self.api().e2ee_create_chat(&body).await {
            Ok(made) => made,
            Err(e) => {
                let _ = inner.device.as_mut().unwrap().forget_group(&g);
                return Err(api_err(e));
            }
        };
        for w in &made.warnings {
            log::info!("e2ee: creating a group: {w}");
        }
        inner.chats.list.insert(
            g.0.clone(),
            Chat {
                name,
                kind: c::E2EEChatKind::Group,
                state: c::E2EEChatState::Joined,
                conversation: made.chat.conversation_id.clone(),
                peer: None,
                declined: Vec::new(),
            },
        );
        let by = inner.account.clone();
        self.note(
            inner,
            &g,
            people
                .iter()
                .map(|a| GroupEvent::Added {
                    by: by.clone(),
                    account: a.clone(),
                })
                .collect(),
        );
        if !pending.is_empty() {
            let name = inner.chats.list[&g.0].name.clone();
            self.tell_invited(&pending).await;
            inner.chats.pending.insert(name, pending);
            self.save_pending(inner);
        }
        Ok(())
    }

    /// Send each invitee the ordinary message that says they were invited
    /// and how to set up encryption, from the home network.
    async fn tell_invited(&self, invitees: &[Invitee]) {
        let network = {
            let w = self.shared.handle.read();
            w.networks
                .iter()
                .find(|n| n.features.encryption)
                .map(|n| n.id.clone())
        };
        let Some(network) = network else {
            return;
        };
        let text = (self.shared.texts.invited)();
        let path = format!(
            "/api/v1/networks/{}/commands",
            crate::api::segment(&network)
        );
        for invitee in invitees {
            let nick = if invitee.nick.is_empty() {
                &invitee.account
            } else {
                &invitee.nick
            };
            // One word that names a person, never a channel or a second line.
            if nick.is_empty()
                || nick.starts_with(['#', '&', ':'])
                || nick.contains(|c: char| c.is_whitespace() || c.is_control() || c == ',')
            {
                log::info!("e2ee: not telling an invitee with an unusable nick");
                continue;
            }
            let body = c::SendRawRequest {
                client_id: crate::client::random_id(),
                line: format!("PRIVMSG {nick} :{text}"),
            };
            if let Err(e) = self
                .api()
                .call_empty(crate::api::Method::POST, &path, &body)
                .await
            {
                log::info!("e2ee: telling an invitee: {e}");
            }
        }
    }

    fn save_pending(&self, inner: &mut Inner) {
        inner.chats.pending.retain(|_, list| !list.is_empty());
        if let Some(vault) = inner.vault.as_ref()
            && let Err(e) = vault.set_pending(&inner.chats.pending)
        {
            log::warn!("e2ee: keeping pending invitees: {e}");
        }
    }

    /// Stop waiting for `account` to set up encryption for the group whose
    /// channel is `conversation`.
    pub fn cancel_pending(&self, conversation: &str, account: &str) {
        let (conversation, account) = (conversation.to_string(), account.to_string());
        let key = self.group_key(&conversation);
        self.group_action(&key, move |engine, inner| {
            Box::pin(async move {
                let g = engine.group_in(inner, &conversation)?;
                let name = inner.chats.list[&g.0].name.clone();
                if let Some(list) = inner.chats.pending.get_mut(&name) {
                    list.retain(|i| i.account != account);
                }
                engine.save_pending(inner);
                Ok(())
            })
        });
    }

    /// Look now whether the people invited to the group in `conversation`
    /// before they had encryption keys have them, and add those who do; as
    /// when the group is opened.
    pub fn check_pending(&self, conversation: &str) {
        let conversation = conversation.to_string();
        self.spawn(move |engine| async move {
            let mut inner = engine.shared.inner.lock().await;
            let Ok(g) = engine.group_in(&inner, &conversation) else {
                return;
            };
            engine.add_pending(&mut inner, Some(g)).await;
            engine.publish_groups(&mut inner);
        });
    }

    /// Add the pending invitees whose keys appeared: in `only`, or in every
    /// group at most every [`PENDING_EVERY`].
    pub(super) async fn add_pending(&self, inner: &mut Inner, only: Option<GroupIdBytes>) {
        if inner.chats.pending.is_empty() {
            return;
        }
        if only.is_none() {
            if inner
                .chats
                .pending_checked
                .is_some_and(|at| at.elapsed() < PENDING_EVERY)
            {
                return;
            }
            inner.chats.pending_checked = Some(std::time::Instant::now());
        }
        let groups: Vec<(GroupIdBytes, String, Option<String>)> = inner
            .chats
            .list
            .iter()
            .filter(|(g, _)| only.as_ref().is_none_or(|o| &o.0 == *g))
            .filter(|(_, c)| inner.chats.pending.contains_key(&c.name))
            .map(|(g, c)| {
                (
                    GroupIdBytes(g.clone()),
                    c.name.clone(),
                    c.conversation.clone(),
                )
            })
            .collect();
        let mut changed = false;
        for (g, name, conversation) in groups {
            let invitees = inner.chats.pending.get(&name).cloned().unwrap_or_default();
            let mut ready = Vec::new();
            for invitee in &invitees {
                if let Ok(d) = self.api().e2ee_account(&invitee.account).await
                    && !d.devices.is_empty()
                {
                    ready.push(invitee.account.clone());
                }
            }
            if ready.is_empty() {
                continue;
            }
            match self.add_now(inner, &g, ready.clone()).await {
                Ok(()) => {
                    if let Some(list) = inner.chats.pending.get_mut(&name) {
                        list.retain(|i| !ready.contains(&i.account));
                    }
                    changed = true;
                    if let Some(conversation) = &conversation {
                        for account in &ready {
                            self.send(conversation, &(self.shared.texts.joined)(account), None);
                        }
                    }
                }
                Err(e) => log::info!("e2ee: adding pending invitees: {e}"),
            }
        }
        if changed {
            self.save_pending(inner);
        }
    }

    /// One key package for each device of these people, checked against
    /// their signed device lists; their identity keys pinned on first sight.
    async fn packages_for(&self, inner: &mut Inner, people: &[String]) -> Result<Vec<Vec<u8>>> {
        let mut packages = Vec::new();
        for account in people {
            let claim = match self.api().e2ee_claim(account).await {
                Ok(c) => c,
                Err(ApiError::Http { status: 404, .. }) => {
                    return Err(format!("peer-not-set-up:{account}"));
                }
                Err(ApiError::Http { status: 504, .. }) => {
                    return Err(format!("peer-no-answer:{account}"));
                }
                Err(e) => return Err(api_err(e)),
            };
            if claim.items.is_empty() {
                return Err(format!("peer-not-set-up:{account}"));
            }
            let device = inner.device.as_mut().ok_or("not-ready")?;
            // A different key from the one pinned is no first contact: the
            // core reports it, and the group shows them as started fresh.
            let _ = device.pin_identity(account, key32(&claim.identity_key)?);
            let listed = claim.device_list.as_ref().and_then(|l| {
                let blob = bytes(&l.list).ok()?;
                device.check_device_list(&blob).ok()
            });
            for item in &claim.items {
                if let Some(list) = &listed
                    && !list.devices.iter().any(|d| d.to_hex() == item.device_id)
                {
                    // A device its own account no longer lists.
                    continue;
                }
                packages.push(bytes(&item.key_package).map_err(api_err)?);
            }
        }
        Ok(packages)
    }

    /// One key package for each other device of this account: the backend
    /// hands them a new chat's Welcome itself.
    async fn own_packages(&self, inner: &mut Inner) -> Result<Vec<Vec<u8>>> {
        let me = inner
            .device
            .as_ref()
            .ok_or("not-ready")?
            .device_id()
            .to_hex();
        // Best effort: the group starts without them rather than not at all.
        let claim = match self.api().e2ee_claim(&inner.account).await {
            Ok(c) => c,
            Err(e) => {
                log::info!("e2ee: claim this account's other devices: {e}");
                return Ok(Vec::new());
            }
        };
        let device = inner.device.as_mut().ok_or("not-ready")?;
        Ok(super::other_devices(device, &claim, &me)
            .filter_map(|i| bytes(&i.key_package).ok())
            .collect())
    }

    /// Add people to the group whose channel is `conversation`.
    pub fn add_people(&self, conversation: &str, accounts: Vec<String>) {
        let conversation = conversation.to_string();
        let key = self.group_key(&conversation);
        self.group_action(&key, move |engine, inner| {
            Box::pin(async move {
                let g = engine.group_in(inner, &conversation)?;
                engine.add_now(inner, &g, accounts).await
            })
        });
    }

    async fn add_now(
        &self,
        inner: &mut Inner,
        g: &GroupIdBytes,
        accounts: Vec<String>,
    ) -> Result<()> {
        let mut people: Vec<String> = accounts
            .iter()
            .map(|a| a.trim().to_string())
            .filter(|a| !a.is_empty())
            .collect();
        people.sort();
        people.dedup();
        let present: BTreeSet<String> = inner
            .device
            .as_ref()
            .and_then(|d| d.members(g).ok())
            .unwrap_or_default()
            .into_iter()
            .map(|m| m.account)
            .collect();
        people.retain(|a| !present.contains(a));
        if people.is_empty() {
            return Ok(());
        }
        self.settle(inner, g).await?;
        let packages = self.packages_for(inner, &people).await?;
        let device = inner.device.as_mut().ok_or("not-ready")?;
        let commit = device.add_devices(g, &packages).map_err(group_err)?;
        let body = c::E2EEChatCommitInputBody {
            added: Some(people),
            ..Default::default()
        };
        if self.send_commit(inner, g, commit, body).await? {
            Ok(())
        } else {
            Err("group-raced".into())
        }
    }

    /// Remove a person from the group whose channel is `conversation`.
    pub fn remove_person(&self, conversation: &str, account: &str) {
        let (conversation, account) = (conversation.to_string(), account.to_string());
        let key = self.group_key(&conversation);
        self.group_action(&key, move |engine, inner| {
            Box::pin(async move {
                let g = engine.group_in(inner, &conversation)?;
                engine.remove_now(inner, &g, &[account]).await
            })
        });
    }

    async fn remove_now(
        &self,
        inner: &mut Inner,
        g: &GroupIdBytes,
        accounts: &[String],
    ) -> Result<()> {
        self.settle(inner, g).await?;
        let device = inner.device.as_mut().ok_or("not-ready")?;
        let commit = device.remove_accounts(g, accounts).map_err(group_err)?;
        let body = c::E2EEChatCommitInputBody {
            removed: Some(accounts.to_vec()),
            ..Default::default()
        };
        if self.send_commit(inner, g, commit, body).await? {
            Ok(())
        } else {
            Err("group-raced".into())
        }
    }

    /// Make a member an admin, or an ordinary member again. The creator's.
    pub fn set_admin(&self, conversation: &str, account: &str, admin: bool) {
        let (conversation, account) = (conversation.to_string(), account.to_string());
        let key = self.group_key(&conversation);
        self.group_action(&key, move |engine, inner| {
            Box::pin(async move {
                let g = engine.group_in(inner, &conversation)?;
                engine.settle(inner, &g).await?;
                let device = inner.device.as_mut().ok_or("not-ready")?;
                let commit = device.set_admin(&g, &account, admin).map_err(group_err)?;
                let body = if admin {
                    c::E2EEChatCommitInputBody {
                        admins: Some(vec![account]),
                        ..Default::default()
                    }
                } else {
                    c::E2EEChatCommitInputBody {
                        members: Some(vec![account]),
                        ..Default::default()
                    }
                };
                if engine.send_commit(inner, &g, commit, body).await? {
                    Ok(())
                } else {
                    Err("group-raced".into())
                }
            })
        });
    }

    /// Leave the group whose channel is `conversation`: a signed `left`
    /// first, so the others see it and an admin removes this account. The
    /// creator hands the group to another member first; alone, it closes.
    pub fn leave_group(&self, conversation: &str) {
        let conversation = conversation.to_string();
        let key = self.group_key(&conversation);
        self.group_action(&key, move |engine, inner| {
            Box::pin(async move { engine.leave_now(inner, &conversation).await })
        });
    }

    async fn leave_now(&self, inner: &mut Inner, conversation: &str) -> Result<()> {
        let g = self.group_in(inner, conversation)?;
        self.settle(inner, &g).await?;
        let device = inner.device.as_mut().ok_or("not-ready")?;
        let settings = device.group_settings(&g).map_err(core_err)?;
        let others: Vec<String> = device
            .members(&g)
            .unwrap_or_default()
            .into_iter()
            .map(|m| m.account)
            .filter(|a| *a != inner.account)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let close = others.is_empty();
        if settings.creator.account == inner.account && !close {
            // An admin takes it over if there is one, else the first member.
            let next = settings
                .admins
                .iter()
                .map(|a| a.account.clone())
                .find(|a| others.contains(a))
                .unwrap_or_else(|| others[0].clone());
            let commit = device.hand_over(&g, &next).map_err(group_err)?;
            let body = c::E2EEChatCommitInputBody {
                admins: Some(vec![next]),
                ..Default::default()
            };
            if !self.send_commit(inner, &g, commit, body).await? {
                return Err("group-raced".into());
            }
        }
        if !close {
            let left = Payload::new(Body::Control {
                op: ControlOp::Left,
            })
            .map_err(core_err)?;
            self.send_payload(inner, conversation, left).await?;
        }
        let name = Self::chat(inner, &g)
            .map(|c| c.name.clone())
            .unwrap_or_default();
        self.api()
            .e2ee_leave_chat(&name, close)
            .await
            .or_else(|e| match e {
                ApiError::Http { status: 404, .. } => Ok(()),
                e => Err(api_err(e)),
            })?;
        if let Some(d) = inner.device.as_mut() {
            let _ = d.forget_group(&g);
        }
        self.forget_chat(inner, &g).await;
        Ok(())
    }

    /// Join a group this account was invited to.
    pub fn accept_invitation(&self, chat: &str) {
        let chat = chat.to_string();
        self.group_action(&chat.clone(), move |engine, inner| {
            Box::pin(async move {
                engine.api().e2ee_join_chat(&chat).await.map_err(api_err)?;
                engine.read_chats(inner).await;
                if let Ok(g) = group_of(&chat) {
                    engine.settle(inner, &g).await?;
                }
                Ok(())
            })
        });
    }

    /// Decline an invitation: forget the group; the inviter's client removes
    /// this account.
    pub fn decline_invitation(&self, chat: &str) {
        let chat = chat.to_string();
        self.group_action(&chat.clone(), move |engine, inner| {
            Box::pin(async move {
                engine
                    .api()
                    .e2ee_decline_chat(&chat)
                    .await
                    .map_err(api_err)?;
                if let Ok(g) = group_of(&chat) {
                    if let Some(d) = inner.device.as_mut() {
                        let _ = d.forget_group(&g);
                    }
                    inner.chats.list.remove(&g.0);
                }
                Ok(())
            })
        });
    }

    /// The group whose channel is `conversation`.
    fn group_in(&self, inner: &Inner, conversation: &str) -> Result<GroupIdBytes> {
        inner
            .chats
            .list
            .iter()
            .find(|(_, c)| c.conversation.as_deref() == Some(conversation))
            .map(|(g, _)| GroupIdBytes(g.clone()))
            .ok_or_else(|| "not-a-group".to_string())
    }

    /// The key a group's busy state and errors are shown under.
    fn group_key(&self, conversation: &str) -> String {
        self.shared
            .handle
            .read()
            .e2ee
            .group(conversation)
            .map_or_else(|| conversation.to_string(), |g| g.chat.clone())
    }

    /// Start an encrypted direct chat carried over IRC, with someone whose
    /// keys another IRCord server holds. Returns the group's name.
    pub(super) async fn new_chat(
        &self,
        inner: &mut Inner,
        conversation: &str,
        peer: &str,
        packages: &[Vec<u8>],
        replaces: Option<String>,
    ) -> Result<String> {
        let device = inner.device.as_mut().ok_or("not-ready")?;
        let started = device.start_dm(peer, packages).map_err(core_err)?;
        let g = started.group_id.clone();
        let name = new_name(&g, false);
        let body = c::E2EECreateChatInputBody {
            group_id: name.clone(),
            kind: c::E2EECreateChatInputBodyKind::Direct,
            conversation_id: Some(conversation.to_string()),
            peer: Some(peer.to_string()),
            epoch: device.epoch(&g).unwrap_or(1) as i64,
            welcome: Some(encode(&started.welcome)),
            replaces,
            ..Default::default()
        };
        match self.api().e2ee_create_chat(&body).await {
            Ok(made) => {
                inner.chats.list.insert(
                    g.0.clone(),
                    Chat {
                        name: name.clone(),
                        kind: c::E2EEChatKind::Direct,
                        state: c::E2EEChatState::Joined,
                        conversation: made.chat.conversation_id,
                        peer: Some(peer.to_string()),
                        declined: Vec::new(),
                    },
                );
                Ok(name)
            }
            Err(e) => {
                let _ = inner.device.as_mut().unwrap().forget_group(&g);
                Err(api_err(e))
            }
        }
    }

    /// The security code with `account`, for verifying someone in a group.
    pub async fn code_for(&self, account: &str) -> Option<super::Code> {
        let inner = self.shared.inner.lock().await;
        let device = inner.device.as_ref()?;
        let code = device.security_code(account).ok()?;
        Some(super::Code {
            groups: code.groups().to_vec(),
            qr: code.qr_payload(),
            verified: device.trust().get(account).is_some_and(|p| p.verified),
        })
    }

    /// Mark `account` verified (or not), wherever they are.
    pub fn set_verified_account(&self, account: &str, verified: bool) {
        let account = account.to_string();
        self.spawn(move |engine| async move {
            let mut inner = engine.shared.inner.lock().await;
            if let Some(device) = inner.device.as_mut()
                && let Err(e) = device.set_verified(&account, verified)
            {
                log::warn!("e2ee: verify {account}: {e}");
                return;
            }
            let records: Vec<Conversation> = inner
                .vault
                .as_ref()
                .map(|v| {
                    v.conversations()
                        .filter(|c| c.peer == account)
                        .cloned()
                        .collect()
                })
                .unwrap_or_default();
            for r in &records {
                engine.publish_conversation(&inner, r);
            }
            engine.publish_groups(&mut inner);
        });
    }
}

/// A core error in a group action, as a short code where the views have
/// words for it.
fn group_err(e: super::CoreError) -> String {
    match e {
        super::CoreError::NeedsUpdate { account, .. } => format!("needs-update:{account}"),
        super::CoreError::NotAdmin(_) => "not-admin".into(),
        e => e.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::e2ee::core::DeviceId;

    fn d(account: &str, id: u8) -> DeviceRef {
        DeviceRef {
            account: account.into(),
            device_id: DeviceId([id; 16]),
        }
    }

    #[test]
    fn membership_changes_read_as_people_not_devices() {
        let before = [d("alice", 1), d("bob", 2), d("carol", 3)];
        let after = [d("alice", 1), d("alice", 4), d("carol", 3), d("dave", 5)];
        let events = membership_events("alice", &before, &after);
        assert!(events.contains(&GroupEvent::DeviceAdded {
            account: "alice".into()
        }));
        assert!(events.contains(&GroupEvent::Added {
            by: "alice".into(),
            account: "dave".into()
        }));
        assert!(events.contains(&GroupEvent::Removed {
            by: "alice".into(),
            account: "bob".into()
        }));
        assert_eq!(events.len(), 3);
    }

    #[test]
    fn new_chats_are_named_as_the_backend_takes_them() {
        let g = GroupIdBytes((0u8..16).collect());
        assert_eq!(new_name(&g, false), "000102030405060708090a0b0c0d0e0f");
        let group = new_name(&g, true);
        assert_eq!(
            group.len(),
            22,
            "base64url: the backend derives the channel from it"
        );
        assert_eq!(group_of(&group).unwrap(), g);
    }
}

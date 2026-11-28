//! The encryption engine: one per signed-in client, when the server offers
//! encryption. It owns this device's [`Device`] and [`Vault`], talks to the
//! backend's `/api/v1/e2ee/` endpoints, and keeps the workspace's view of
//! encryption (`crate::store::e2ee`) up to date.
//!
//! - **Set-up.** The first device of an account creates the identity key
//!   and publishes it; a later one shows an approval code (and its QR code)
//!   and waits for approval ([`super::approval`]). Every set-up device keeps a
//!   stock of key packages in the directory.
//! - **Reading.** It watches the timelines: every encrypted message still
//!   showing the placeholder is decrypted (or looked up in the vault, where
//!   the plaintext is kept, since MLS decrypts a message once) and written
//!   into its entry. Live messages and loaded history take the same path.
//! - **Sending.** [`Engine::send`] encrypts, keeps the plaintext in the
//!   vault, and posts the ciphertext through the outbox, so a retry sends
//!   the same ciphertext. Nothing is ever sent in the clear into a
//!   conversation that is encrypted.
//! - **Group changes** go through the group log: Welcomes and commits are
//!   fetched when a message names a group or epoch this device does not
//!   have, and every half minute.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use ircord_contract as c;
use tokio::sync::Mutex as AsyncMutex;

use super::approval::{self, ConversationRecord, Handover};
use super::core::{
    Body, ControlOp, Device, Envelope, Error as CoreError, GroupIdBytes, Kind, Payload, Received,
};
use super::vault::{
    Conversation, Outcome, Plain, Record, SealedState, Unreadable, Vault, VaultKey,
};
use super::{b64url, files, from_b64url, state};
use crate::api::e2ee::{bytes, encode};
use crate::api::{Api, ApiError};
use crate::client::{Handle, random_id};
use crate::store::e2ee::{
    self as view, ApprovalFailure, Approving, Attention, Blocked, DeviceState, OwnDevice, View,
};
use crate::store::{Changes, PendingSend, SendState, Workspace, parse_time};

/// How often the engine syncs with the group log and the approval relay.
const SYNC_EVERY: Duration = Duration::from_secs(30);
/// The least time between two syncs [`Engine::poke`] asks for.
const POKE_EVERY: Duration = Duration::from_secs(2);
/// How often a device waiting for approval looks for an answer.
const WAITING_EVERY: Duration = Duration::from_secs(3);
/// The relay forgets a message after 10 minutes; a waiting device posts its
/// request again before (as the web client does, every 8 minutes).
const REQUEST_AGAIN: Duration = Duration::from_secs(8 * 60);
/// Key packages a device keeps in the directory, and when it tops them up.
const STOCK: i64 = 20;
const LOW_STOCK: i64 = 10;
/// The largest encrypted file this device downloads to open.
pub const MAX_FILE: usize = 100 << 20;

/// Writes a control line: whether encryption is now on, who did it (a
/// name), and whether it was you.
pub type ControlText = Box<dyn Fn(bool, &str, bool) -> String + Send + Sync>;

/// The interface text the engine writes into timelines. The app gives it,
/// so the text stays in the app's catalogs.
pub struct Texts {
    pub control: ControlText,
    /// Why a message cannot be read.
    pub unreadable: Box<dyn Fn(&Unreadable) -> String + Send + Sync>,
}

/// A decrypted file, ready to show or save.
pub struct OpenedFile {
    pub name: String,
    pub mime: String,
    pub bytes: Vec<u8>,
}

/// The security code with a peer, for verification.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Code {
    pub groups: Vec<String>,
    /// What the QR code carries.
    pub qr: String,
    pub verified: bool,
}

/// Running engines by server origin, so the app's image loader, which has no
/// session at hand, can answer a local file URL.
static ENGINES: Mutex<Vec<(String, Weak<Shared>)>> = Mutex::new(Vec::new());

/// Decrypting the file behind a local file URL
/// (`crate::store::e2ee::file_url`), if a running engine has it.
pub fn local_file(
    url: &str,
) -> Option<impl std::future::Future<Output = Result<OpenedFile>> + Send + 'static> {
    let (message, index) = view::parse_file_url(url)?;
    let engines = ENGINES.lock().unwrap();
    let shared = engines
        .iter()
        .filter(|(origin, _)| url.starts_with(origin.as_str()))
        .find_map(|(_, w)| w.upgrade())?;
    let engine = Engine { shared };
    Some(async move { engine.open_file(&message, index).await })
}

/// See the module docs. Cheap to clone.
#[derive(Clone)]
pub struct Engine {
    shared: Arc<Shared>,
}

struct Shared {
    handle: Handle,
    runtime: tokio::runtime::Handle,
    dir: PathBuf,
    key: VaultKey,
    /// The device's name in the account's device list ("IRCord GPUI on Linux").
    label: String,
    texts: Texts,
    inner: AsyncMutex<Inner>,
    /// Messages being processed, so a burst of changes processes each once.
    inflight: Mutex<BTreeSet<String>>,
    /// When [`Engine::poke`] last synced.
    poked: Mutex<Option<std::time::Instant>>,
    /// Text to send, in the order it was typed.
    queue: tokio::sync::mpsc::UnboundedSender<Outgoing>,
}

/// Text waiting to be encrypted and sent.
struct Outgoing {
    conversation: String,
    text: String,
    action: bool,
    reply_to: Option<String>,
}

#[derive(Default)]
struct Inner {
    vault: Option<Vault>,
    device: Option<Device<SealedState>>,
    /// This account on the home network, as IRC reports it.
    account: String,
    generation: i64,
    /// Waiting for approval: the code shown and the request posted.
    waiting: Option<Waiting>,
    /// Devices this one approved that still have to be added to its groups.
    to_add: BTreeSet<String>,
    started: bool,
    /// (conversation, group) this device bound in this session.
    bound: BTreeSet<(String, String)>,
}

/// A new device's approval request.
struct Waiting {
    /// The 20 digits the person carries to the approving device.
    code: String,
    /// What goes to the mailbox: the certificate request and its HMAC.
    message: Vec<u8>,
    /// When it was last posted; `None` until a post went through.
    posted: Option<std::time::Instant>,
}

type Result<T, E = String> = std::result::Result<T, E>;

fn api_err(e: ApiError) -> String {
    e.to_string()
}

fn core_err(e: CoreError) -> String {
    e.to_string()
}

fn group_b64(g: &GroupIdBytes) -> String {
    b64url(&g.0)
}

fn group_of(text: &str) -> Result<GroupIdBytes> {
    from_b64url(text).map(GroupIdBytes)
}

fn key32(text: &str) -> Result<[u8; 32]> {
    bytes(text)
        .map_err(api_err)?
        .try_into()
        .map_err(|_| "an identity key has the wrong length".to_string())
}

/// The home network's account: the network that passed the capability gate.
fn home_account(w: &Workspace) -> Option<String> {
    w.networks
        .iter()
        .filter(|n| n.features.encryption)
        .find_map(|n| n.account.clone().filter(|a| !a.is_empty() && a != "*"))
}

/// The other person in a direct conversation, by account, if this is one
/// where encryption could work.
pub fn peer_of(w: &Workspace, conversation: &str) -> Result<String, Blocked> {
    if w.capabilities.encryption.is_none() {
        return Err(Blocked::Off);
    }
    let conv = w.conversation(conversation).ok_or(Blocked::NotHere)?;
    if conv.kind != c::ConversationKind::Direct {
        return Err(Blocked::NotHere);
    }
    let network = w.network(&conv.network_id).ok_or(Blocked::NotHere)?;
    if !network.features.encryption {
        return Err(Blocked::Network);
    }
    // The backend lists only you in a direct conversation; a test fixture or
    // an older backend may list the other person too.
    let member = conv
        .member_ids
        .iter()
        .filter(|id| !w.is_self(id, &conv.network_id))
        .find_map(|id| w.members.get(id));
    if member.is_some_and(|m| m.role == c::MemberRole::Bot) {
        return Err(Blocked::NoAccount);
    }
    match member.map(|m| m.account.as_str()) {
        Some(account) if !account.is_empty() && account != "*" => Ok(account.to_string()),
        _ => match w.e2ee.accounts.get(conversation) {
            Some(Some(account)) => Ok(account.clone()),
            _ => Err(Blocked::NoAccount),
        },
    }
}

/// A person's IRCord account, if they have one encryption can reach: not a
/// bot, and signed in to an account on the network.
fn account_of(m: &c::Member) -> Option<String> {
    (m.role != c::MemberRole::Bot && !m.account.is_empty() && m.account != "*")
        .then(|| m.account.clone())
}

/// The direct conversation with `member` on `network`, if there is one.
pub fn direct_with(w: &Workspace, network: &str, member: &str) -> Option<String> {
    w.conversations
        .iter()
        .filter(|conv| conv.network_id == network && conv.kind == c::ConversationKind::Direct)
        .find(|conv| {
            crate::store::people::direct_partner(w, &conv.id).is_some_and(|m| m.id == member)
        })
        .map(|conv| conv.id.clone())
}

/// Why an encrypted chat with `member` on `network` cannot be started now,
/// for the person menu, where there may be no direct conversation yet: the
/// same reasons the conversation's lock gives.
pub fn blocked_with(w: &Workspace, network: &str, member: &str) -> Option<Blocked> {
    if w.capabilities.encryption.is_none() {
        return Some(Blocked::Off);
    }
    if let Some(dm) = direct_with(w, network, member)
        && peer_of(w, &dm).is_ok()
    {
        return blocked(w, &dm);
    }
    if !w.network(network).is_some_and(|n| n.features.encryption) {
        return Some(Blocked::Network);
    }
    let Some(account) = w.members.get(member).and_then(account_of) else {
        return Some(Blocked::NoAccount);
    };
    if !w.e2ee.device.ready() {
        return Some(Blocked::NotReady);
    }
    if w.e2ee.peers.get(&account) == Some(&false) {
        return Some(Blocked::PeerNotSetUp);
    }
    None
}

/// Remember that `conversation` is with `member`'s account, when the person
/// menu opened it: the backend lists only you in a direct conversation, so
/// without this the account would wait for a whois.
pub fn note_peer(w: &mut Workspace, conversation: &str, member: &str) -> Changes {
    let Some(account) = w.members.get(member).and_then(account_of) else {
        return Changes::default();
    };
    if w.e2ee.accounts.get(conversation) == Some(&Some(account.clone())) {
        return Changes::default();
    }
    w.e2ee
        .accounts
        .insert(conversation.to_string(), Some(account));
    Changes::topic_id(view::TOPIC, conversation)
}

/// Why encryption cannot be started in `conversation` now, if it cannot.
pub fn blocked(w: &Workspace, conversation: &str) -> Option<Blocked> {
    let peer = match peer_of(w, conversation) {
        Ok(p) => p,
        Err(b) => return Some(b),
    };
    if !w.e2ee.device.ready() {
        return Some(Blocked::NotReady);
    }
    if w.e2ee.peers.get(&peer) == Some(&false) {
        return Some(Blocked::PeerNotSetUp);
    }
    None
}

impl Engine {
    /// Start the engine for a signed-in client. `dir` is this account's
    /// folder for the vault; `key` its key from the credential store.
    pub fn start(
        handle: Handle,
        runtime: tokio::runtime::Handle,
        dir: PathBuf,
        key: VaultKey,
        label: String,
        texts: Texts,
    ) -> Engine {
        let (queue, outgoing) = tokio::sync::mpsc::unbounded_channel();
        let engine = Engine {
            shared: Arc::new(Shared {
                handle: handle.clone(),
                runtime,
                dir,
                key,
                label,
                texts,
                inner: AsyncMutex::new(Inner::default()),
                inflight: Mutex::default(),
                poked: Mutex::default(),
                queue,
            }),
        };
        let weak = Arc::downgrade(&engine.shared);
        handle.watch(move |_, changes| {
            if let Some(shared) = weak.upgrade() {
                let engine = Engine { shared };
                engine.changed(changes);
            }
        });
        let weak = Arc::downgrade(&engine.shared);
        engine.shared.runtime.spawn(run(weak.clone()));
        engine
            .shared
            .runtime
            .spawn(send_queued(weak.clone(), outgoing));
        let mut engines = ENGINES.lock().unwrap();
        engines.retain(|(_, w)| w.strong_count() > 0);
        engines.push((engine.api().server().origin(), weak));
        drop(engines);
        engine
    }

    pub fn handle(&self) -> &Handle {
        &self.shared.handle
    }

    fn spawn<F>(&self, f: impl FnOnce(Engine) -> F + Send + 'static)
    where
        F: std::future::Future<Output = ()> + Send + 'static,
    {
        let engine = self.clone();
        self.shared.runtime.spawn(async move { f(engine).await });
    }

    fn change(&self, f: impl FnOnce(&mut Workspace) -> Changes) {
        self.shared.handle.change(f);
    }

    fn set_device(&self, state: DeviceState) {
        self.change(|w| {
            if w.e2ee.device == state {
                return Changes::default();
            }
            w.e2ee.device = state;
            Changes::topic(view::TOPIC)
        });
    }

    fn api(&self) -> &Api {
        self.shared.handle.api()
    }

    /// A set of changes arrived: look at the timelines they touched.
    fn changed(&self, changes: &Changes) {
        if changes.topics.contains_key(view::TOPIC)
            && changes.timelines.is_empty()
            && !changes.reset
        {
            return;
        }
        let conversations: Vec<String> = if changes.reset {
            self.shared
                .handle
                .read()
                .timelines
                .keys()
                .cloned()
                .collect()
        } else {
            changes.timelines.iter().cloned().collect()
        };
        if !conversations.is_empty() {
            self.scan(conversations);
        }
    }

    /// Process what is waiting in these conversations.
    fn scan(&self, conversations: Vec<String>) {
        let mut jobs: Vec<(String, c::Message)> = Vec::new();
        let mut plain_in_encrypted: Vec<(String, Vec<String>)> = Vec::new();
        {
            let w = self.shared.handle.read();
            let inflight = self.shared.inflight.lock().unwrap();
            for id in &conversations {
                for m in view::waiting(&w, id, &inflight) {
                    jobs.push((id.clone(), m));
                }
                let fresh: Vec<String> = w
                    .timelines
                    .get(id)
                    .map(|t| {
                        t.entries
                            .iter()
                            .filter(|e| {
                                !w.e2ee.unencrypted.contains(&e.message.id)
                                    && view::arrived_unencrypted(&w, id, &e.message)
                            })
                            .map(|e| e.message.id.clone())
                            .collect()
                    })
                    .unwrap_or_default();
                if !fresh.is_empty() {
                    plain_in_encrypted.push((id.clone(), fresh));
                }
            }
        }
        if !plain_in_encrypted.is_empty() {
            self.change(|w| {
                let mut changes = Changes::default();
                for (id, messages) in &plain_in_encrypted {
                    if let Some(c) = w.e2ee.conversations.get_mut(id) {
                        c.unencrypted = true;
                        changes.merge(Changes::topic_id(view::TOPIC, id.clone()));
                    }
                    w.e2ee.unencrypted.extend(messages.iter().cloned());
                    changes.timelines.insert(id.clone());
                }
                changes
            });
        }
        if jobs.is_empty() {
            return;
        }
        {
            let mut inflight = self.shared.inflight.lock().unwrap();
            jobs.retain(|(_, m)| inflight.insert(m.id.clone()));
        }
        self.spawn(move |engine| async move {
            let ids: Vec<String> = jobs.iter().map(|(_, m)| m.id.clone()).collect();
            engine.process(jobs).await;
            let mut inflight = engine.shared.inflight.lock().unwrap();
            for id in ids {
                inflight.remove(&id);
            }
        });
    }

    async fn process(&self, jobs: Vec<(String, c::Message)>) {
        let mut inner = self.shared.inner.lock().await;
        if !inner.started {
            // Set-up will scan again once it is done.
            return;
        }
        // Whether a sync of Welcomes and commits went through in this batch.
        let mut synced: Option<bool> = None;
        for (conversation, m) in jobs {
            let mut found = self.outcome(&mut inner, &conversation, &m).await;
            if found.is_none() && synced.is_none() {
                // A group or epoch this device does not know yet: fetch the
                // Welcomes and commits, then try once more.
                synced = Some(self.sync_groups(&mut inner).await);
                found = self.outcome(&mut inner, &conversation, &m).await;
            }
            let (outcome, keep) = match found {
                Some(found) => found,
                // A group from someone whose new keys the person has not
                // acknowledged yet: it stays waiting until they do.
                None if self.has_attention(&conversation) => continue,
                // Only final once this device saw the whole group log.
                None => (
                    Outcome::Unreadable {
                        why: Unreadable::BeforeDevice,
                    },
                    synced == Some(true),
                ),
            };
            let ready = inner
                .device
                .as_ref()
                .is_some_and(|d| d.certificate().is_some());
            if keep
                && ready
                && let Some(vault) = inner.vault.as_mut()
            {
                let record = Record::Message {
                    id: m.id.clone(),
                    outcome: outcome.clone(),
                };
                if let Err(e) = vault.put(record) {
                    log::warn!("e2ee: store a message: {e}");
                }
            }
            if let Outcome::Control { on, by, at, group } = &outcome {
                self.controlled(&mut inner, &conversation, *on, by, *at, group.as_deref())
                    .await;
            }
            self.apply(&inner, &conversation, &m, &outcome);
        }
    }

    /// The other account in a conversation: the vault's record, or the
    /// network's.
    fn conversation_peer(&self, inner: &Inner, conversation: &str) -> Option<String> {
        inner
            .vault
            .as_ref()
            .and_then(|v| v.conversation(conversation).map(|c| c.peer.clone()))
            .or_else(|| peer_of(&self.shared.handle.read(), conversation).ok())
    }

    /// What a message is, from the vault or decrypted now, and whether that
    /// is final (kept in the vault). `None` when it names a group or epoch
    /// this device does not have (yet).
    async fn outcome(
        &self,
        inner: &mut Inner,
        conversation: &str,
        m: &c::Message,
    ) -> Option<(Outcome, bool)> {
        let forged = || {
            (
                Outcome::Unreadable {
                    why: Unreadable::Forged,
                },
                false,
            )
        };
        let vault = inner.vault.as_mut()?;
        if let Some(o) = vault.message(&m.id) {
            // A server can file a message under another conversation.
            if let Outcome::Plain(p) = o
                && p.conversation != conversation
            {
                return Some(forged());
            }
            return Some((o.clone(), false));
        }
        if let Some(client_id) = &m.client_id
            && (vault.sent_as(client_id) || vault.sent(client_id).is_some())
        {
            if vault
                .sent(client_id)
                .is_some_and(|p| p.conversation != conversation)
            {
                return Some(forged());
            }
            let link = Record::Linked {
                client_id: client_id.clone(),
                id: m.id.clone(),
            };
            if let Err(e) = vault.put(link) {
                log::warn!("e2ee: link a sent message: {e}");
            }
            return vault.message(&m.id).cloned().map(|o| (o, false));
        }
        let expected_peer = self.conversation_peer(inner, conversation);
        let self_sent = {
            let w = self.shared.handle.read();
            let network = w
                .conversation(conversation)
                .map(|c| c.network_id.clone())
                .unwrap_or_default();
            w.is_self(&m.sender_id, &network)
        };
        let account = inner.account.clone();
        let device = inner.device.as_mut()?;
        device.certificate()?;
        let e = m.encrypted.as_ref()?;
        let class = if e.class == c::EncryptedClass::Auxiliary {
            "a"
        } else {
            "m"
        };
        let tag = format!("{}.{class}.{}", e.version, e.payload);
        let sender = m
            .sender_account
            .clone()
            .or_else(|| m.account.clone())
            .unwrap_or_default();
        let at = parse_time(&m.created_at);
        let outcome = match device.receive(&tag, &sender) {
            Ok(Received::Message {
                payload,
                sender,
                group_id,
            }) => {
                // The group must be this conversation's, and the timeline's
                // sender the one MLS verified: a server must not move a
                // message to another conversation or another person.
                let group_peer = device
                    .groups()
                    .into_iter()
                    .find(|(g, _)| *g == group_id)
                    .map(|(_, p)| p);
                let ours = expected_peer.is_none_or(|p| group_peer.as_deref() == Some(p.as_str()))
                    && (sender.account == account) == self_sent;
                if !ours {
                    return Some(forged());
                }
                match payload.body {
                    Body::Message {
                        text,
                        kind,
                        reply,
                        attachments,
                        gif,
                    } => Outcome::Plain(Plain {
                        conversation: conversation.to_string(),
                        inner_id: payload.id.to_hex(),
                        sender: sender.account,
                        text,
                        action: kind == Kind::Action,
                        reply: reply.map(|r| r.to_hex()),
                        attachments,
                        gif,
                        at,
                    }),
                    Body::Control { op } => Outcome::Control {
                        on: op == ControlOp::EncryptionOn,
                        by: sender.account,
                        at,
                        group: Some(group_b64(&group_id)),
                    },
                    _ => Outcome::Hidden,
                }
            }
            Ok(Received::Partial { .. }) => Outcome::Hidden,
            Ok(Received::Own { .. }) => Outcome::Unreadable {
                why: Unreadable::OwnLost,
            },
            Err(CoreError::UnknownGroup) => return None,
            // Sent before this device was in the group: its keys are gone.
            Err(CoreError::Mls(e))
                if e.contains("TooDistantInThePast") || e.contains("NoPastEpochData") =>
            {
                Outcome::Unreadable {
                    why: Unreadable::BeforeDevice,
                }
            }
            Err(CoreError::Mls(e)) if e.contains("Epoch") || e.contains("epoch") => return None,
            Err(CoreError::IdentityChanged {
                account, presented, ..
            }) => {
                self.attention(conversation, &account, presented);
                return Some((
                    Outcome::Unreadable {
                        why: Unreadable::IdentityChanged,
                    },
                    false,
                ));
            }
            Err(CoreError::SenderMismatch { .. }) => Outcome::Unreadable {
                why: Unreadable::Forged,
            },
            // Possibly passing (storage, a malformed line): shown, not kept.
            Err(e) => {
                return Some((
                    Outcome::Unreadable {
                        why: Unreadable::Failed(e.to_string()),
                    },
                    false,
                ));
            }
        };
        Some((outcome, true))
    }

    /// Write an outcome into the timeline.
    fn apply(&self, inner: &Inner, conversation: &str, m: &c::Message, outcome: &Outcome) {
        let shown = view::shown_of(outcome);
        let mine = |account: &str| account == inner.account;
        let reply = match outcome {
            Outcome::Plain(p) => p
                .reply
                .as_deref()
                .and_then(|r| inner.vault.as_ref()?.by_inner(r).map(str::to_string)),
            _ => None,
        };
        let texts = &self.shared.texts;
        let origin = self.api().server().origin();
        self.change(|w| {
            let network = w
                .conversation(conversation)
                .map(|c| c.network_id.clone())
                .unwrap_or_default();
            let view = match outcome {
                Outcome::Plain(p) => Some(View {
                    text: p.text.clone(),
                    kind: if p.action {
                        c::MessageKind::Action
                    } else {
                        c::MessageKind::Message
                    },
                    reply: reply.and_then(|id| {
                        let quoted = w.message(conversation, &id)?;
                        Some(c::Reply {
                            id: id.clone(),
                            nick: w.display_name(&quoted.sender_id, &network),
                            text: quoted.text.chars().take(200).collect(),
                        })
                    }),
                    gif: p.gif.as_ref().map(|g| c::GIF {
                        url: g.url.clone(),
                        width: g.width.unwrap_or(0).into(),
                        height: g.height.unwrap_or(0).into(),
                        title: g.title.clone().unwrap_or_default(),
                        ..Default::default()
                    }),
                    attachments: p
                        .attachments
                        .iter()
                        .enumerate()
                        .map(|(i, a)| c::Attachment {
                            url: view::file_url(&origin, &m.id, i),
                            name: a.name.clone(),
                            alt: a.name.clone(),
                            // Only pictures keep their type: anything else
                            // would be streamed by a player from a URL that
                            // only this device can answer.
                            content_type: Some(if a.mime.starts_with("image/") {
                                a.mime.clone()
                            } else {
                                files::UPLOAD_TYPE.to_string()
                            }),
                            size: Some(a.size as i64),
                            width: a.width.unwrap_or(0).into(),
                            height: a.height.unwrap_or(0).into(),
                        })
                        .collect(),
                }),
                Outcome::Control { on, by, .. } => {
                    let name = w.display_name(&m.sender_id, &network);
                    Some(View {
                        text: (texts.control)(*on, &name, mine(by)),
                        // Drawn as an event line (a topic change's look);
                        // `System` lines are not drawn.
                        kind: c::MessageKind::Event,
                        ..Default::default()
                    })
                }
                Outcome::Hidden => None,
                Outcome::Unreadable { why } => Some(View {
                    text: (texts.unreadable)(why),
                    kind: c::MessageKind::Notice,
                    ..Default::default()
                }),
            };
            view::show(w, conversation, &m.id, shown, view)
        });
    }

    /// A control event from `group` (absent in what older versions
    /// stored) was read: the rules are in [`super::state`].
    async fn controlled(
        &self,
        inner: &mut Inner,
        conversation: &str,
        on: bool,
        by: &str,
        at: i64,
        group: Option<&str>,
    ) {
        let Some(vault) = inner.vault.as_ref() else {
            return;
        };
        let known = vault.conversation(conversation).cloned();
        let peer = known
            .as_ref()
            .map(|k| k.peer.clone())
            .or_else(|| peer_of(&self.shared.handle.read(), conversation).ok())
            .unwrap_or_else(|| by.to_string());
        let (group, change) = match group {
            Some(g) => (
                Some(g.to_string()),
                state::control_change(known.as_ref(), g, on, at),
            ),
            // Stored by an older version without its group: the newest this
            // device holds with the peer, and only a newer event counts.
            None => {
                let newest = inner.device.as_ref().and_then(|d| {
                    d.groups()
                        .into_iter()
                        .filter(|(_, p)| *p == peer)
                        .max_by_key(|(g, _)| d.epoch(g).unwrap_or(0))
                        .map(|(g, _)| group_b64(&g))
                });
                let newer = known.as_ref().is_none_or(|k| k.since < at);
                (
                    newest.or(known.as_ref().and_then(|k| k.group.clone())),
                    newer.then_some(on),
                )
            }
        };
        let mine = by == inner.account;
        let Some(on) = change else {
            // The "on" of the group this conversation is in already: the
            // Welcome may have come before its starter bound the group.
            if on
                && !mine
                && let (Some(k), Some(g)) = (&known, &group)
                && k.on
                && k.group.as_deref() == Some(g.as_str())
            {
                self.bind_existing(inner, conversation, g, &k.peer).await;
            }
            return;
        };
        let record = Conversation {
            conversation: conversation.to_string(),
            peer,
            group,
            on,
            since: at,
            by: by.to_string(),
        };
        if let Some(vault) = inner.vault.as_mut()
            && let Err(e) = vault.put(Record::Conversation(record.clone()))
        {
            log::warn!("e2ee: store a conversation: {e}");
        }
        if let Some(group) = &record.group {
            if on && !mine {
                // Bind this side too, so the backend refuses plaintext from here.
                self.bind_existing(inner, conversation, group, &record.peer)
                    .await;
            } else if !on {
                // A device of this account may have bound the group again
                // after it was turned off. Nothing changes when it is gone.
                self.unbind(inner, conversation, group, &record.peer).await;
            }
        }
        self.publish_conversation(inner, &record);
    }

    /// Bind this account's side of a conversation to `group`, only if the
    /// server already names it as the conversation's current group: a device
    /// that joined or read a group never changes which group is current, or a
    /// late Welcome or event would bring back a group turned off since. Only
    /// starting encryption replaces a group.
    async fn bind_existing(&self, inner: &mut Inner, conversation: &str, group: &str, peer: &str) {
        let key = (conversation.to_string(), group.to_string());
        if inner.bound.contains(&key) {
            return;
        }
        match self
            .api()
            .e2ee_bind(conversation, group, peer, Some(group))
            .await
        {
            Ok(_) => {
                inner.bound.insert(key);
            }
            Err(e) => log::debug!("e2ee: bind {conversation}: {e}"),
        }
    }

    /// End the server's binding of a conversation to `group`, if that group
    /// is still its current one.
    /// When the server names another group with the same person, made
    /// current again by a client that missed an earlier "off", that ends too.
    async fn unbind(&self, inner: &mut Inner, conversation: &str, group: &str, peer: &str) {
        inner
            .bound
            .remove(&(conversation.to_string(), group.to_string()));
        match self.api().e2ee_turn_off(conversation, group).await {
            Ok(()) => {}
            Err(ApiError::Http { status: 409, .. }) => {
                if self
                    .turn_off_stale(inner, conversation, peer)
                    .await
                    .is_err()
                {
                    log::debug!("e2ee: unbind {conversation}: another group is current");
                }
            }
            Err(e) => log::debug!("e2ee: unbind {conversation}: {e}"),
        }
    }

    /// Show a conversation's state in the workspace.
    fn publish_conversation(&self, inner: &Inner, record: &Conversation) {
        let verified = inner
            .device
            .as_ref()
            .and_then(|d| d.trust().get(&record.peer).map(|p| p.verified))
            .unwrap_or(false);
        let id = record.conversation.clone();
        let (on, peer, since, by) = (
            record.on,
            record.peer.clone(),
            record.since,
            record.by.clone(),
        );
        self.change(|w| {
            let c = w.e2ee.conversations.entry(id.clone()).or_default();
            c.on = on;
            c.peer = peer;
            c.verified = verified;
            c.since = since;
            c.by = by;
            if !on {
                c.unencrypted = false;
            }
            Changes::topic_id(view::TOPIC, id)
        });
    }

    fn has_attention(&self, conversation: &str) -> bool {
        self.shared
            .handle
            .read()
            .e2ee
            .conversation(conversation)
            .is_some_and(|c| c.attention.is_some())
    }

    fn attention(&self, conversation: &str, account: &str, presented: [u8; 32]) {
        let id = conversation.to_string();
        let account = account.to_string();
        self.change(|w| {
            let c = w.e2ee.conversations.entry(id.clone()).or_default();
            c.peer = account;
            c.verified = false;
            c.attention = Some(Attention::StartedFresh {
                identity: b64url(&presented),
            });
            Changes::topic_id(view::TOPIC, id)
        });
    }

    fn error(&self, conversation: &str, error: Option<String>) {
        let id = conversation.to_string();
        self.change(|w| {
            match error {
                Some(e) => w.e2ee.errors.insert(id.clone(), e),
                None => w.e2ee.errors.remove(&id),
            };
            w.e2ee.busy.remove(&id);
            Changes::topic_id(view::TOPIC, id)
        });
    }

    fn busy(&self, conversation: &str) -> bool {
        let id = conversation.to_string();
        let mut already = false;
        self.change(|w| {
            already = !w.e2ee.busy.insert(id.clone());
            w.e2ee.errors.remove(&id);
            Changes::topic_id(view::TOPIC, id)
        });
        already
    }

    // ---- Set-up ----

    async fn setup(&self, inner: &mut Inner) -> Result<()> {
        let account = {
            let w = self.shared.handle.read();
            if w.capabilities.encryption.is_none() {
                return Ok(());
            }
            match home_account(&w) {
                Some(a) => a,
                None => return Ok(()),
            }
        };
        self.set_device(DeviceState::Starting);
        let vault = Vault::open(&self.shared.dir, self.shared.key.clone())?;
        // Which conversations are encrypted is known from here on, whatever
        // the network does next.
        inner.account = account.clone();
        let records: Vec<Conversation> = vault.conversations().cloned().collect();
        inner.vault = Some(vault);
        for r in &records {
            self.publish_conversation(inner, r);
        }
        self.change(|w| {
            w.e2ee.loaded = true;
            Changes::topic(view::TOPIC)
        });
        let directory = self.api().e2ee_directory().await.map_err(api_err)?;
        let server_identity = match &directory.identity {
            Some(id) => Some((key32(&id.identity_key)?, id.generation)),
            None => None,
        };
        let vault = inner.vault.as_ref().ok_or("no vault")?;
        let device = if vault.has_device() {
            Some(Device::open(vault.device_storage()).map_err(core_err)?)
        } else {
            None
        };
        match (device, server_identity) {
            (Some(device), _) if device.account() != account => {
                return Err(format!(
                    "this device's encryption keys are for {}",
                    device.account()
                ));
            }
            (Some(device), Some((key, generation))) if device.identity_key() == Some(key) => {
                inner.generation = generation;
                inner.device = Some(device);
                let listed = directory
                    .devices
                    .iter()
                    .any(|d| d.device_id == inner.device.as_ref().unwrap().device_id().to_hex());
                if !listed {
                    self.publish_device(inner).await?;
                }
                self.ready(inner).await;
            }
            (Some(device), None) if device.holds_identity() => {
                // The directory lost the identity (a server reset): publish it again.
                let identity = device.identity_key().unwrap();
                let published = self
                    .api()
                    .e2ee_publish_identity(&identity, false)
                    .await
                    .map_err(api_err)?;
                inner.generation = published.generation;
                inner.device = Some(device);
                self.publish_device(inner).await?;
                self.ready(inner).await;
            }
            (Some(device), _) if device.certificate().is_none() => {
                inner.device = Some(device);
                self.wait_for_approval(inner).await?;
            }
            (Some(device), _) => {
                // Someone started fresh on another device: this one is no
                // longer approved. It keeps what it stored.
                inner.device = Some(device);
                self.set_device(DeviceState::ResetElsewhere);
            }
            (None, None) => {
                let vault = inner.vault.as_ref().unwrap();
                let device =
                    Device::create_account(vault.device_storage(), &account).map_err(core_err)?;
                let identity = device.identity_key().unwrap();
                let published = self
                    .api()
                    .e2ee_publish_identity(&identity, false)
                    .await
                    .map_err(api_err)?;
                inner.generation = published.generation;
                inner.device = Some(device);
                self.publish_device(inner).await?;
                self.ready(inner).await;
            }
            (None, Some((_, generation))) => {
                inner.generation = generation;
                let vault = inner.vault.as_ref().unwrap();
                let device =
                    Device::create_device(vault.device_storage(), &account).map_err(core_err)?;
                inner.device = Some(device);
                self.wait_for_approval(inner).await?;
            }
        }
        inner.started = true;
        // Show what the vault knows about each conversation.
        let records: Vec<Conversation> = inner
            .vault
            .as_ref()
            .map(|v| v.conversations().cloned().collect())
            .unwrap_or_default();
        for r in &records {
            self.publish_conversation(inner, r);
        }
        Ok(())
    }

    async fn publish_device(&self, inner: &mut Inner) -> Result<()> {
        let device = inner.device.as_mut().ok_or("no device")?;
        let cert = device
            .certificate()
            .ok_or("this device is not approved yet")?
            .to_bytes();
        let last_resort = device.last_resort_key_package().map_err(core_err)?;
        let id = device.device_id().to_hex();
        self.api()
            .e2ee_publish_device(
                &id,
                inner.generation,
                &cert,
                Some(&last_resort),
                &self.shared.label,
            )
            .await
            .map_err(api_err)?;
        Ok(())
    }

    /// Set up and approved: top up, catch up, show it.
    async fn ready(&self, inner: &mut Inner) {
        self.refresh_directory(inner).await;
        self.sync_groups(inner).await;
        let approver = inner.device.as_ref().is_some_and(|d| d.holds_identity());
        let fingerprint = inner.device.as_ref().and_then(|d| {
            let key = d.identity_key()?;
            let f = super::core::fingerprint(d.account(), &key).ok()?;
            Some(super::grouped(&super::hex(&f[..16])))
        });
        self.change(|w| {
            w.e2ee.device = DeviceState::Ready { approver };
            w.e2ee.fingerprint = fingerprint;
            Changes::topic(view::TOPIC)
        });
    }

    /// Read the account's devices, and top up this one's key packages.
    async fn refresh_directory(&self, inner: &mut Inner) {
        let Ok(directory) = self.api().e2ee_directory().await else {
            return;
        };
        let Some(device) = inner.device.as_mut() else {
            return;
        };
        let me = device.device_id().to_hex();
        let identity = device.identity_key();
        let devices: Vec<OwnDevice> = directory
            .devices
            .iter()
            .map(|d| OwnDevice {
                id: d.device_id.clone(),
                label: d.label.clone().unwrap_or_default(),
                this: d.device_id == me,
                approved: bytes(&d.certificate)
                    .ok()
                    .and_then(|c| super::core::DeviceCertificate::from_bytes(&c).ok())
                    .is_some_and(|c| Some(c.identity_key) == identity),
                key_packages: d.key_packages,
            })
            .collect();
        let stock = directory
            .devices
            .iter()
            .find(|d| d.device_id == me)
            .map(|d| d.key_packages);
        if let Some(stock) = stock
            && stock < LOW_STOCK
            && device.certificate().is_some()
        {
            match device.key_packages((STOCK - stock) as usize) {
                Ok(packages) => {
                    if let Err(e) = self.api().e2ee_add_key_packages(&me, &packages).await {
                        log::warn!("e2ee: publish key packages: {e}");
                    }
                }
                Err(e) => log::warn!("e2ee: make key packages: {e}"),
            }
        }
        self.change(|w| {
            w.e2ee.devices = devices;
            Changes::topic(view::TOPIC)
        });
    }

    /// Process Welcomes for this device and commits for its groups.
    /// Returns whether every read of the group log went through.
    async fn sync_groups(&self, inner: &mut Inner) -> bool {
        let Some(device) = inner.device.as_ref() else {
            return false;
        };
        if device.certificate().is_none() {
            return false;
        }
        let me = device.device_id().to_hex();
        let mut complete = true;
        match self.api().e2ee_welcomes(&me).await {
            Ok(welcomes) => {
                for welcome in welcomes {
                    let joined = match bytes(&welcome.welcome) {
                        Ok(b) => inner.device.as_mut().unwrap().join_dm(&b),
                        Err(e) => {
                            log::warn!("e2ee: a Welcome: {e}");
                            continue;
                        }
                    };
                    match joined {
                        Ok(joined) => {
                            let group = group_b64(&joined.group_id);
                            let conversation = {
                                let w = self.shared.handle.read();
                                w.conversations
                                    .iter()
                                    .find(|c| {
                                        peer_of(&w, &c.id).ok().as_deref() == Some(&joined.peer)
                                    })
                                    .map(|c| c.id.clone())
                            };
                            if let Some(conversation) = conversation {
                                let known = inner
                                    .vault
                                    .as_ref()
                                    .and_then(|v| v.conversation(&conversation).cloned());
                                if state::welcome_binds(known.as_ref()) {
                                    self.bind_existing(inner, &conversation, &group, &joined.peer)
                                        .await;
                                }
                                // A group that replaces an encrypted one (the
                                // peer started fresh) is the one to write to now.
                                if let Some(k) = known.filter(|k| k.on)
                                    && let Some(vault) = inner.vault.as_mut()
                                {
                                    let record = Conversation {
                                        group: Some(group.clone()),
                                        ..k
                                    };
                                    let _ = vault.put(Record::Conversation(record));
                                }
                            }
                        }
                        Err(CoreError::IdentityChanged {
                            account, presented, ..
                        }) => {
                            // Kept until the person acknowledges the change.
                            let conversation = {
                                let w = self.shared.handle.read();
                                w.conversations
                                    .iter()
                                    .find(|c| peer_of(&w, &c.id).ok().as_deref() == Some(&account))
                                    .map(|c| c.id.clone())
                            };
                            if let Some(conversation) = conversation {
                                self.attention(&conversation, &account, presented);
                            }
                            continue;
                        }
                        Err(e) => log::warn!("e2ee: a Welcome was refused: {e}"),
                    }
                    if let Err(e) = self.api().e2ee_delete_welcome(welcome.id).await {
                        log::debug!("e2ee: remove a Welcome: {e}");
                    }
                }
            }
            Err(e) => {
                log::debug!("e2ee: list Welcomes: {e}");
                complete = false;
            }
        }
        let groups: Vec<GroupIdBytes> = inner
            .device
            .as_ref()
            .map(|d| d.groups().into_iter().map(|(g, _)| g).collect())
            .unwrap_or_default();
        for group in groups {
            complete &= self.sync_group(inner, &group).await;
        }
        complete
    }

    /// Apply the commits other devices made in one group since this
    /// device's epoch.
    /// Returns whether the log was read.
    async fn sync_group(&self, inner: &mut Inner, group: &GroupIdBytes) -> bool {
        let Some(device) = inner.device.as_mut() else {
            return false;
        };
        let Ok(epoch) = device.epoch(group) else {
            return true;
        };
        let Ok(commits) = self
            .api()
            .e2ee_commits(&group_b64(group), epoch as i64)
            .await
        else {
            return false;
        };
        for commit in commits {
            if commit.epoch < device.epoch(group).unwrap_or(0) as i64 {
                continue;
            }
            let applied = bytes(&commit.commit)
                .map_err(api_err)
                .and_then(|b| device.process_commit(&b).map_err(core_err));
            match applied {
                Ok(applied) if applied.removed_self => {
                    let _ = device.forget_group(group);
                    return true;
                }
                Ok(_) => {}
                Err(e) => {
                    log::warn!("e2ee: a commit was refused: {e}");
                    return true;
                }
            }
        }
        true
    }

    /// A new device: post a request under a new approval code to the
    /// account's mailbox, and show the code.
    async fn wait_for_approval(&self, inner: &mut Inner) -> Result<()> {
        let device = inner.device.as_ref().ok_or("no device")?;
        let request = device.certificate_request().map_err(core_err)?.to_bytes();
        let code = approval::new_code();
        let message = approval::seal_request(&code, &request);
        inner.waiting = Some(Waiting {
            code: code.clone(),
            message,
            posted: None,
        });
        self.post_request(inner).await;
        self.set_device(DeviceState::Waiting {
            qr: approval::qr_text(&code),
            code: approval::format_code(&code),
        });
        Ok(())
    }

    /// Post (or post again) this device's request to the mailbox. A failure
    /// is tried again on the next tick.
    async fn post_request(&self, inner: &mut Inner) {
        let (Some(device), Some(waiting)) = (inner.device.as_ref(), inner.waiting.as_mut()) else {
            return;
        };
        let me = device.device_id().to_hex();
        match self
            .api()
            .e2ee_relay(&me, approval::MAILBOX, &waiting.message)
            .await
        {
            Ok(_) => waiting.posted = Some(std::time::Instant::now()),
            Err(e) => log::info!("e2ee: post the approval request: {e}"),
        }
    }

    /// A new device: read the relay for an approval sealed under its code.
    async fn relay(&self, inner: &mut Inner) {
        let (Some(device), Some(waiting)) = (inner.device.as_ref(), inner.waiting.as_ref()) else {
            return;
        };
        let me = device.device_id().to_hex();
        let Ok(messages) = self.api().e2ee_approvals(&me).await else {
            return;
        };
        let code = waiting.code.clone();
        for m in messages {
            let _ = self.api().e2ee_delete_approval(m.id).await;
            // Only an answer sealed under this device's code counts: the
            // server cannot make one.
            let Ok(handover) = bytes(&m.payload)
                .map_err(api_err)
                .and_then(|b| approval::open_handover(&code, &b))
            else {
                continue;
            };
            if let Err(e) = self.approved(inner, handover).await {
                log::warn!("e2ee: approval: {e}");
                self.set_device(DeviceState::Failed(e));
            }
            return;
        }
    }

    /// The new device got its approval.
    async fn approved(&self, inner: &mut Inner, handover: Handover) -> Result<()> {
        let cert = bytes(&handover.certificate).map_err(api_err)?;
        // The certificate must be signed by the identity the account
        // publishes, not one a relay made up.
        let parsed = super::core::DeviceCertificate::from_bytes(&cert).map_err(core_err)?;
        let directory = self.api().e2ee_directory().await.map_err(api_err)?;
        let published = directory
            .identity
            .as_ref()
            .map(|id| key32(&id.identity_key))
            .transpose()?;
        if published != Some(parsed.identity_key) {
            return Err("the approval is not from this account's identity".into());
        }
        if let Some(id) = &directory.identity {
            inner.generation = id.generation;
        }
        let device = inner.device.as_mut().ok_or("no device")?;
        device.install_certificate(&cert).map_err(core_err)?;
        for v in &handover.verified {
            if v.account == inner.account {
                continue;
            }
            let Ok(key) = key32(&v.identity_key) else {
                continue;
            };
            // A pin that does not match stays as this device found it.
            if device.pin_identity(&v.account, key).is_ok() {
                let _ = device.set_verified(&v.account, true);
            }
        }
        if let Some(vault) = inner.vault.as_mut() {
            for r in handover.conversations {
                let Some(c) = conversation_from_web(r) else {
                    continue;
                };
                if vault.conversation(&c.conversation).is_none() {
                    let _ = vault.put(Record::Conversation(c));
                }
            }
        }
        inner.waiting = None;
        self.publish_device(inner).await?;
        self.ready(inner).await;
        let records: Vec<Conversation> = inner
            .vault
            .as_ref()
            .map(|v| v.conversations().cloned().collect())
            .unwrap_or_default();
        for r in &records {
            self.publish_conversation(inner, r);
        }
        Ok(())
    }

    /// Approve the new device that shows `input`: its approval code, typed
    /// (spaces and dashes allowed) or as its QR code's text.
    pub fn approve(&self, input: &str) {
        let input = input.to_string();
        self.change(|w| {
            w.e2ee.approving = Approving::Busy;
            Changes::topic(view::TOPIC)
        });
        self.spawn(move |engine| async move {
            let mut inner = engine.shared.inner.lock().await;
            let result = engine.approve_now(&mut inner, &input).await;
            engine.change(|w| {
                w.e2ee.approving = match result {
                    Ok(()) => Approving::Done,
                    Err(e) => Approving::Failed(e),
                };
                Changes::topic(view::TOPIC)
            });
            engine.count_waiting(&inner).await;
            engine.add_approved(&mut inner).await;
        });
    }

    async fn approve_now(&self, inner: &mut Inner, input: &str) -> Result<(), ApprovalFailure> {
        let code = approval::normalize(input).ok_or(ApprovalFailure::BadCode)?;
        let other = ApprovalFailure::Other;
        let device = inner
            .device
            .as_ref()
            .ok_or_else(|| other("no device".into()))?;
        if !device.holds_identity() {
            return Err(other(
                "only the device that set up encryption first can approve others".into(),
            ));
        }
        let messages = self
            .api()
            .e2ee_approvals(approval::MAILBOX)
            .await
            .map_err(|e| other(api_err(e)))?;
        let (id, request) = messages
            .iter()
            .find_map(|m| {
                let b = bytes(&m.payload).ok()?;
                approval::open_request(&code, &b).map(|r| (m.id, r))
            })
            .ok_or(ApprovalFailure::NotFound)?;
        // `certify_device` checks the request's signature.
        let cert = device
            .certify_device(&request)
            .map_err(|e| other(core_err(e)))?;
        let verified = device
            .trust()
            .accounts()
            .filter(|(a, p)| *a != inner.account && p.verified)
            .map(|(a, p)| approval::Verified {
                account: a.to_string(),
                identity_key: encode(&p.identity_key),
            })
            .collect();
        let conversations = inner
            .vault
            .as_ref()
            .map(|v| v.conversations().map(conversation_for_web).collect())
            .unwrap_or_default();
        let handover = Handover {
            certificate: encode(&cert.to_bytes()),
            verified,
            conversations,
        };
        let me = device.device_id().to_hex();
        let new = cert.device_id.to_hex();
        self.api()
            .e2ee_relay(&me, &new, &approval::seal_handover(&code, &handover))
            .await
            .map_err(|e| other(api_err(e)))?;
        let _ = self.api().e2ee_delete_approval(id).await;
        inner.to_add.insert(new);
        Ok(())
    }

    /// An approving device: count the requests waiting in the mailbox.
    async fn count_waiting(&self, inner: &Inner) {
        if !inner.device.as_ref().is_some_and(|d| d.holds_identity()) {
            return;
        }
        let Ok(messages) = self.api().e2ee_approvals(approval::MAILBOX).await else {
            return;
        };
        let n = messages.len();
        if self.shared.handle.read().e2ee.waiting_approvals != n {
            self.change(|w| {
                w.e2ee.waiting_approvals = n;
                Changes::topic(view::TOPIC)
            });
        }
    }

    /// Add devices this one approved to its groups, once they published keys.
    async fn add_approved(&self, inner: &mut Inner) {
        if inner.to_add.is_empty() {
            return;
        }
        let Ok(claim) = self.api().e2ee_claim(&inner.account).await else {
            return;
        };
        let account = inner.account.clone();
        let Some(device) = inner.device.as_mut() else {
            return;
        };
        for item in claim.items {
            if !inner.to_add.contains(&item.device_id) {
                continue;
            }
            let Ok(package) = bytes(&item.key_package) else {
                continue;
            };
            let mut all = true;
            for (group, _) in device.groups() {
                let added = device.add_devices(&group, std::slice::from_ref(&package));
                let commit = match added {
                    Ok(commit) => commit,
                    Err(e) => {
                        log::warn!("e2ee: add a device: {e}");
                        all = false;
                        continue;
                    }
                };
                let body = c::E2EECommitInputBody {
                    epoch: commit.epoch as i64,
                    commit: encode(&commit.commit),
                    group_info: commit.group_info.as_deref().map(encode),
                    welcomes: commit.welcome.as_deref().map(|w| {
                        vec![c::E2EEWelcomeInput {
                            account: account.clone(),
                            device_id: item.device_id.clone(),
                            welcome: encode(w),
                        }]
                    }),
                };
                match self.api().e2ee_commit(&group_b64(&group), &body).await {
                    Ok(_) => {
                        let _ = device.confirm_commit(&group);
                    }
                    Err(e) => {
                        let _ = device.discard_commit(&group);
                        log::info!("e2ee: commit lost a race, trying again later: {e}");
                        all = false;
                    }
                }
            }
            if all {
                inner.to_add.remove(&item.device_id);
            }
        }
    }

    // ---- Conversations ----

    /// Ask the backend whether a direct conversation is encrypted, once.
    pub fn check_conversation(&self, conversation: &str) {
        check_conversation(&self.shared.handle, conversation);
    }

    /// Check whether the peer of `conversation` has set up encryption, and
    /// ask the network who they are first if it has not said.
    pub fn check_peer(&self, conversation: &str) {
        let (peer, ask) = {
            let w = self.shared.handle.read();
            match peer_of(&w, conversation) {
                Ok(p) if !w.e2ee.peers.contains_key(&p) => (p, None),
                Ok(_) => return,
                Err(Blocked::NoAccount) if !w.e2ee.accounts.contains_key(conversation) => {
                    let Some(conv) = w.conversation(conversation) else {
                        return;
                    };
                    (
                        String::new(),
                        Some((conv.network_id.clone(), conv.name.clone())),
                    )
                }
                Err(_) => return,
            }
        };
        if let Some((network, nick)) = ask {
            let conversation = conversation.to_string();
            self.spawn(move |engine| async move {
                let Ok(whois) = engine.api().whois(&network, &nick).await else {
                    return;
                };
                let account = whois
                    .account
                    .filter(|a| whois.found && !a.is_empty() && a != "*");
                engine.change(|w| {
                    w.e2ee.accounts.insert(conversation.clone(), account);
                    Changes::topic_id(view::TOPIC, conversation.clone())
                });
                engine.check_peer(&conversation);
            });
            return;
        }
        self.spawn(move |engine| async move {
            let has = match engine.api().e2ee_account(&peer).await {
                Ok(d) => !d.devices.is_empty(),
                Err(ApiError::Http { status: 404, .. }) => false,
                Err(_) => return,
            };
            engine.change(|w| {
                w.e2ee.peers.insert(peer, has);
                Changes::topic(view::TOPIC)
            });
        });
    }

    /// Start an encrypted chat in a direct conversation.
    pub fn start_chat(&self, conversation: &str) {
        if self.busy(conversation) {
            return;
        }
        let conversation = conversation.to_string();
        self.spawn(move |engine| async move {
            let mut inner = engine.shared.inner.lock().await;
            let result = engine.start_now(&mut inner, &conversation).await;
            engine.error(&conversation, result.err());
        });
    }

    async fn start_now(&self, inner: &mut Inner, conversation: &str) -> Result<()> {
        let peer = {
            let w = self.shared.handle.read();
            peer_of(&w, conversation).map_err(|b| format!("{b:?}"))?
        };
        self.new_group(inner, conversation, &peer).await?;
        self.send_payload(
            inner,
            conversation,
            Payload::new(Body::Control {
                op: ControlOp::EncryptionOn,
            })
            .map_err(core_err)?,
        )
        .await?;
        Ok(())
    }

    /// Make a group with every device of both accounts and bind the
    /// conversation to it.
    async fn new_group(&self, inner: &mut Inner, conversation: &str, peer: &str) -> Result<String> {
        let theirs = self.api().e2ee_claim(peer).await.map_err(api_err)?;
        if theirs.items.is_empty() {
            self.change(|w| {
                w.e2ee.peers.insert(peer.to_string(), false);
                Changes::topic(view::TOPIC)
            });
            return Err("peer-not-set-up".into());
        }
        let ours = self
            .api()
            .e2ee_claim(&inner.account)
            .await
            .map_err(api_err)?;
        let device = inner.device.as_mut().ok_or("not-ready")?;
        if device.certificate().is_none() {
            return Err("not-ready".into());
        }
        match device.pin_identity(peer, key32(&theirs.identity_key)?) {
            Ok(_) => {}
            Err(CoreError::IdentityChanged {
                account, presented, ..
            }) => {
                self.attention(conversation, &account, presented);
                return Err("started-fresh".into());
            }
            Err(e) => return Err(core_err(e)),
        }
        let me = device.device_id().to_hex();
        let mut targets: Vec<(String, String)> = Vec::new();
        let mut packages = Vec::new();
        for item in &theirs.items {
            packages.push(bytes(&item.key_package).map_err(api_err)?);
            targets.push((peer.to_string(), item.device_id.clone()));
        }
        for item in ours.items.iter().filter(|i| i.device_id != me) {
            packages.push(bytes(&item.key_package).map_err(api_err)?);
            targets.push((inner.account.clone(), item.device_id.clone()));
        }
        let started = device.start_dm(peer, &packages).map_err(core_err)?;
        let group = group_b64(&started.group_id);
        let epoch = device.epoch(&started.group_id).unwrap_or(1) as i64;
        let welcome = encode(&started.welcome);
        let body = c::E2EECreateGroupInputBody {
            group_id: group.clone(),
            members: vec![inner.account.clone(), peer.to_string()],
            epoch,
            group_info: started.group_info.as_deref().map(encode),
            welcomes: Some(
                targets
                    .iter()
                    .map(|(account, device)| c::E2EEWelcomeInput {
                        account: account.clone(),
                        device_id: device.clone(),
                        welcome: welcome.clone(),
                    })
                    .collect(),
            ),
        };
        if let Err(e) = self.api().e2ee_create_group(&body).await {
            let _ = device.forget_group(&started.group_id);
            return Err(api_err(e));
        }
        let older: Vec<String> = device
            .groups()
            .into_iter()
            .filter(|(g, p)| p == peer && *g != started.group_id)
            .map(|(g, _)| group_b64(&g))
            .collect();
        if let Err(e) = self.bind_started(conversation, &group, peer, &older).await {
            let _ = inner
                .device
                .as_mut()
                .unwrap()
                .forget_group(&started.group_id);
            return Err(api_err(e));
        }
        let record = Conversation {
            conversation: conversation.to_string(),
            peer: peer.to_string(),
            group: Some(group.clone()),
            on: true,
            since: chrono::Utc::now().timestamp_millis(),
            by: inner.account.clone(),
        };
        if let Some(vault) = inner.vault.as_mut() {
            vault.put(Record::Conversation(record.clone()))?;
        }
        self.publish_conversation(inner, &record);
        Ok(group)
    }

    /// Turn encryption off: a signed notice first, then the backend.
    pub fn turn_off(&self, conversation: &str) {
        if self.busy(conversation) {
            return;
        }
        let conversation = conversation.to_string();
        self.spawn(move |engine| async move {
            let mut inner = engine.shared.inner.lock().await;
            let result = engine.turn_off_now(&mut inner, &conversation).await;
            engine.error(&conversation, result.err());
        });
    }

    async fn turn_off_now(&self, inner: &mut Inner, conversation: &str) -> Result<()> {
        let record = inner
            .vault
            .as_ref()
            .and_then(|v| v.conversation(conversation).cloned())
            .ok_or("not-encrypted")?;
        let mut refused = None;
        if let Some(group) = record.group.clone() {
            self.send_payload(
                inner,
                conversation,
                Payload::new(Body::Control {
                    op: ControlOp::EncryptionOff,
                })
                .map_err(core_err)?,
            )
            .await?;
            inner
                .bound
                .remove(&(conversation.to_string(), group.clone()));
            match self.api().e2ee_turn_off(conversation, &group).await {
                // 404: this account has no binding left; the other person's
                // client ended it on reading the notice.
                Ok(()) | Err(ApiError::Http { status: 404, .. }) => {}
                Err(e @ ApiError::Http { status: 409, .. }) => {
                    // The notice is out, so this side is off whatever the
                    // server says; a group it cannot end is shown as an error.
                    if self
                        .turn_off_stale(inner, conversation, &record.peer)
                        .await
                        .is_err()
                    {
                        refused = Some(api_err(e));
                    }
                }
                Err(e) => log::warn!("e2ee: turn off on the server: {e}"),
            }
        }
        let record = Conversation {
            on: false,
            since: chrono::Utc::now().timestamp_millis(),
            by: inner.account.clone(),
            ..record
        };
        if let Some(vault) = inner.vault.as_mut() {
            vault.put(Record::Conversation(record.clone()))?;
        }
        self.publish_conversation(inner, &record);
        refused.map_or(Ok(()), Err)
    }

    /// Make `group`, just started, the conversation's current group. It
    /// replaces the group the server names; after encryption was turned off
    /// it names none to this side, yet a client that missed the "off" may
    /// have made an older group current again, and the server refuses with
    /// 409: that is one of the `older` groups this device shares with the
    /// person, so each is tried as the one to replace.
    async fn bind_started(
        &self,
        conversation: &str,
        group: &str,
        peer: &str,
        older: &[String],
    ) -> Result<(), ApiError> {
        let current = self
            .api()
            .e2ee_conversation(conversation)
            .await
            .ok()
            .and_then(|s| s.group_id)
            .filter(|g| g != group);
        let first = self
            .api()
            .e2ee_bind(conversation, group, peer, current.as_deref())
            .await;
        let Err(error @ ApiError::Http { status: 409, .. }) = first else {
            return first.map(drop);
        };
        for old in older.iter().filter(|g| Some(*g) != current.as_ref()) {
            match self
                .api()
                .e2ee_bind(conversation, group, peer, Some(old))
                .await
            {
                Ok(_) => return Ok(()),
                Err(ApiError::Http { status: 409, .. }) => {}
                Err(e) => return Err(e),
            }
        }
        Err(error)
    }

    /// Turning off was refused because the server names another group, made
    /// current again by a client that missed an earlier "off". When it is a
    /// group with this person, it ends too; otherwise the server would keep
    /// refusing plaintext while this side shows encryption as off.
    async fn turn_off_stale(
        &self,
        inner: &mut Inner,
        conversation: &str,
        peer: &str,
    ) -> Result<(), ()> {
        let server = self
            .api()
            .e2ee_conversation(conversation)
            .await
            .map_err(drop)?;
        let stale = server.group_id.ok_or(())?;
        let theirs = inner.device.as_ref().is_some_and(|d| {
            d.groups()
                .into_iter()
                .any(|(g, p)| p == peer && group_b64(&g) == stale)
        });
        if !theirs {
            return Err(());
        }
        inner
            .bound
            .remove(&(conversation.to_string(), stale.clone()));
        self.api()
            .e2ee_turn_off(conversation, &stale)
            .await
            .map_err(drop)
    }

    /// Send text into an encrypted conversation. `/me ` sends an action.
    pub fn send(&self, conversation: &str, text: &str, reply_to: Option<String>) {
        let conversation = conversation.to_string();
        let (action, text) = match text.strip_prefix("/me ") {
            Some(rest) => (true, rest.to_string()),
            None => (
                false,
                text.strip_prefix("//")
                    .map_or(text.to_string(), |t| format!("/{t}")),
            ),
        };
        let _ = self.shared.queue.send(Outgoing {
            conversation,
            text,
            action,
            reply_to,
        });
    }

    async fn send_now(&self, out: Outgoing) {
        let Outgoing {
            conversation,
            text,
            action,
            reply_to,
        } = out;
        let mut inner = self.shared.inner.lock().await;
        let reply = reply_to.and_then(|id| match inner.vault.as_ref()?.message(&id) {
            Some(Outcome::Plain(p)) => super::core::MessageId::from_hex(&p.inner_id).ok(),
            _ => None,
        });
        let payload = Payload::new(Body::Message {
            text,
            kind: if action { Kind::Action } else { Kind::Message },
            reply,
            attachments: Vec::new(),
            gif: None,
        });
        let result = match payload {
            Ok(p) => self.send_payload(&mut inner, &conversation, p).await,
            Err(e) => Err(core_err(e)),
        };
        if let Err(e) = result {
            self.error(&conversation, Some(e));
        }
    }

    /// Encrypt files, upload them, and send them as one encrypted message.
    pub fn send_files(
        &self,
        conversation: &str,
        network: &str,
        list: Vec<crate::api::uploads::FileData>,
        done: impl FnOnce(Result<(), String>) + Send + 'static,
    ) {
        let conversation = conversation.to_string();
        let network = network.to_string();
        self.spawn(move |engine| async move {
            let mut attachments = Vec::new();
            let mut failed = None;
            for file in &list {
                let sealed = files::seal(&file.bytes);
                let upload = crate::api::uploads::FileData::new(
                    "encrypted",
                    files::UPLOAD_TYPE,
                    sealed.bytes,
                );
                let progress = crate::api::uploads::Progress::default();
                match engine
                    .api()
                    .upload_file(
                        &network,
                        crate::api::uploads::UploadUse::Message,
                        &upload,
                        &progress,
                        crate::api::uploads::STALL,
                    )
                    .await
                {
                    Ok(u) => attachments.push(super::core::Attachment {
                        url: u.url,
                        key: sealed.key,
                        sha256: sealed.sha256,
                        name: file.name.clone(),
                        mime: file.content_type.clone(),
                        size: file.bytes.len() as u64,
                        width: None,
                        height: None,
                    }),
                    Err(e) => {
                        failed = Some(e.to_string());
                        break;
                    }
                }
            }
            if let Some(e) = failed {
                done(Err(e));
                return;
            }
            let mut inner = engine.shared.inner.lock().await;
            let payload = Payload::new(Body::Message {
                text: String::new(),
                kind: Kind::Message,
                reply: None,
                attachments,
                gif: None,
            });
            let result = match payload {
                Ok(p) => engine.send_payload(&mut inner, &conversation, p).await,
                Err(e) => Err(core_err(e)),
            };
            done(result);
        });
    }

    /// Encrypt one payload into the conversation's group and post it.
    async fn send_payload(
        &self,
        inner: &mut Inner,
        conversation: &str,
        payload: Payload,
    ) -> Result<()> {
        let attention = {
            let w = self.shared.handle.read();
            w.e2ee
                .conversation(conversation)
                .and_then(|c| c.attention.clone())
        };
        if attention.is_some() {
            return Err("started-fresh".into());
        }
        let record = inner
            .vault
            .as_ref()
            .and_then(|v| v.conversation(conversation).cloned())
            .ok_or("not-encrypted")?;
        let device = inner.device.as_ref().ok_or("not-ready")?;
        let current = record
            .group
            .as_deref()
            .and_then(|g| group_of(g).ok())
            .filter(|g| device.groups().iter().any(|(id, _)| id == g));
        let group = match current {
            Some(g) => {
                // Devices added since this one last looked must be able to
                // read what it sends.
                self.sync_group(inner, &g).await;
                g
            }
            // Someone started fresh: a new group replaces the old one.
            None => group_of(&self.new_group(inner, conversation, &record.peer).await?)?,
        };
        let device = inner.device.as_mut().ok_or("not-ready")?;
        let tags = device.encrypt(&group, &payload).map_err(core_err)?;
        let (text, action, attachments, gif) = match &payload.body {
            Body::Message {
                text,
                kind,
                attachments,
                gif,
                ..
            } => (
                text.clone(),
                *kind == Kind::Action,
                attachments.clone(),
                gif.clone(),
            ),
            _ => (String::new(), false, Vec::new(), None),
        };
        let shown_text = text.clone();
        let now = chrono::Utc::now().timestamp_millis();
        let vault = inner.vault.as_mut().ok_or("not-ready")?;
        let mut sends = Vec::new();
        for (i, tag) in tags.iter().enumerate() {
            let envelope = Envelope::parse(tag).map_err(core_err)?;
            let client_id = random_id();
            let class = if envelope.class == super::core::Class::Auxiliary {
                c::EncryptedClass::Auxiliary
            } else {
                c::EncryptedClass::Message
            };
            let encrypted = c::Encrypted {
                version: 1,
                class,
                payload: b64url(&envelope.mls_message),
            };
            let record = match (&payload.body, i) {
                (Body::Message { .. }, 0) => Record::Sent {
                    client_id: client_id.clone(),
                    plain: Plain {
                        conversation: conversation.to_string(),
                        inner_id: payload.id.to_hex(),
                        sender: inner.account.clone(),
                        text: text.clone(),
                        action,
                        reply: match &payload.body {
                            Body::Message { reply, .. } => reply.map(|r| r.to_hex()),
                            _ => None,
                        },
                        attachments: attachments.clone(),
                        gif: gif.clone(),
                        at: now,
                    },
                },
                (Body::Control { op }, 0) => Record::SentAs {
                    client_id: client_id.clone(),
                    outcome: Outcome::Control {
                        on: *op == ControlOp::EncryptionOn,
                        by: inner.account.clone(),
                        at: now,
                        group: Some(group_b64(&group)),
                    },
                },
                _ => Record::SentAs {
                    client_id: client_id.clone(),
                    outcome: Outcome::Hidden,
                },
            };
            vault.put(record)?;
            sends.push((client_id, encrypted, i == 0));
        }
        let visible = matches!(payload.body, Body::Message { .. });
        for (client_id, encrypted, first) in sends {
            let pending = PendingSend {
                client_id: client_id.clone(),
                conversation_id: conversation.to_string(),
                text: if first && visible {
                    shown_text.clone()
                } else {
                    String::new()
                },
                action,
                state: SendState::Sending,
                created_ms: crate::events::now_ms(),
                encrypted: Some(encrypted.clone()),
                ..Default::default()
            };
            let hidden = !(first && visible);
            self.change(|w| {
                if hidden {
                    w.e2ee.hidden_sends.insert(client_id.clone());
                }
                w.queue_send(pending)
            });
            let body = c::MessageInputBody {
                client_id: client_id.clone(),
                text: view::PLACEHOLDER.to_string(),
                kind: Some(c::MessageInputBodyKind::Message),
                encrypted: Some(encrypted),
                ..Default::default()
            };
            let result = self.api().send_message(conversation, &body).await;
            self.change(|w| match result {
                Ok(response) => w.send_answered(&response),
                Err(e) => w.fail_send(&client_id, &e.to_string()).unwrap_or_default(),
            });
        }
        Ok(())
    }

    // ---- Trust ----

    /// The security code with the peer of `conversation`.
    pub async fn code(&self, conversation: &str) -> Option<Code> {
        let peer = self
            .shared
            .handle
            .read()
            .e2ee
            .conversation(conversation)?
            .peer
            .clone();
        let inner = self.shared.inner.lock().await;
        let device = inner.device.as_ref()?;
        let code = device.security_code(&peer).ok()?;
        Some(Code {
            groups: code.groups().to_vec(),
            qr: code.qr_payload(),
            verified: device.trust().get(&peer).is_some_and(|p| p.verified),
        })
    }

    /// Mark the peer of `conversation` verified (or not).
    pub fn set_verified(&self, conversation: &str, verified: bool) {
        let conversation = conversation.to_string();
        self.spawn(move |engine| async move {
            let mut inner = engine.shared.inner.lock().await;
            let Some(record) = inner
                .vault
                .as_ref()
                .and_then(|v| v.conversation(&conversation).cloned())
            else {
                return;
            };
            if let Some(device) = inner.device.as_mut()
                && let Err(e) = device.set_verified(&record.peer, verified)
            {
                engine.error(&conversation, Some(e.to_string()));
                return;
            }
            // Every conversation with that person shows it.
            let all: Vec<Conversation> = inner
                .vault
                .as_ref()
                .map(|v| {
                    v.conversations()
                        .filter(|c| c.peer == record.peer)
                        .cloned()
                        .collect()
                })
                .unwrap_or_default();
            for r in &all {
                engine.publish_conversation(&inner, r);
            }
        });
    }

    /// The person acknowledged that the peer started fresh.
    pub fn acknowledge(&self, conversation: &str) {
        let conversation = conversation.to_string();
        self.spawn(move |engine| async move {
            let (peer, identity) = {
                let w = engine.shared.handle.read();
                match w.e2ee.conversation(&conversation) {
                    Some(c) => match &c.attention {
                        Some(Attention::StartedFresh { identity }) => {
                            (c.peer.clone(), identity.clone())
                        }
                        None => return,
                    },
                    None => return,
                }
            };
            let mut inner = engine.shared.inner.lock().await;
            let Ok(key) = from_b64url(&identity)
                .and_then(|k| <[u8; 32]>::try_from(k).map_err(|_| "length".to_string()))
            else {
                return;
            };
            if let Some(device) = inner.device.as_mut()
                && let Err(e) = device.accept_identity_change(&peer, key)
            {
                engine.error(&conversation, Some(e.to_string()));
                return;
            }
            engine.change(|w| {
                for c in w.e2ee.conversations.values_mut().filter(|c| c.peer == peer) {
                    c.attention = None;
                    c.verified = false;
                }
                Changes::topic(view::TOPIC)
            });
            // Join the group the new keys made, kept waiting until now, and
            // read what waited for it.
            engine.sync_groups(&mut inner).await;
            drop(inner);
            let all: Vec<String> = engine
                .shared
                .handle
                .read()
                .timelines
                .keys()
                .cloned()
                .collect();
            engine.scan(all);
        });
    }

    /// Start fresh: a new identity for the account. Every other device loses
    /// its approval; encrypted conversations get new groups as people write.
    pub fn start_fresh(&self) {
        self.spawn(move |engine| async move {
            let mut inner = engine.shared.inner.lock().await;
            if let Err(e) = engine.start_fresh_now(&mut inner).await {
                engine.set_device(DeviceState::Failed(e));
            }
        });
    }

    async fn start_fresh_now(&self, inner: &mut Inner) -> Result<()> {
        let account = inner.account.clone();
        let vault = inner.vault.as_ref().ok_or("not-ready")?;
        // The new identity is made beside the current one, which stays until
        // the server took the new one: a failure leaves this device as it was.
        vault.discard_new_device();
        let fresh =
            Device::create_account(vault.new_device_storage(), &account).map_err(core_err)?;
        let identity = fresh.identity_key().ok_or("no identity")?;
        drop(fresh);
        let published = match self.api().e2ee_publish_identity(&identity, true).await {
            Ok(p) => p,
            Err(e) => {
                vault.discard_new_device();
                return Err(api_err(e));
            }
        };
        vault.promote_device()?;
        let device = Device::open(vault.device_storage()).map_err(core_err)?;
        inner.waiting = None;
        inner.generation = published.generation;
        inner.device = Some(device);
        self.publish_device(inner).await?;
        let records: Vec<Conversation> = inner
            .vault
            .as_ref()
            .map(|v| v.conversations().cloned().collect())
            .unwrap_or_default();
        // Each encrypted conversation keeps its old group's ID: the next
        // message makes a new group that replaces it.
        for r in &records {
            self.publish_conversation(inner, r);
        }
        inner.started = true;
        self.ready(inner).await;
        Ok(())
    }

    /// Remove one of the account's other devices, from the directory and
    /// from every group this device is in.
    pub fn remove_device(&self, id: &str) {
        let id = id.to_string();
        self.spawn(move |engine| async move {
            let mut inner = engine.shared.inner.lock().await;
            if let Err(e) = engine.api().e2ee_delete_device(&id).await {
                log::warn!("e2ee: remove a device: {e}");
            }
            let account = inner.account.clone();
            if let (Some(device), Ok(device_id)) =
                (inner.device.as_mut(), super::core::DeviceId::from_hex(&id))
            {
                let target = super::core::DeviceRef { account, device_id };
                for (group, _) in device.groups() {
                    if !device.members(&group).unwrap_or_default().contains(&target) {
                        continue;
                    }
                    if let Ok(commit) = device.remove_devices(&group, std::slice::from_ref(&target))
                    {
                        let body = c::E2EECommitInputBody {
                            epoch: commit.epoch as i64,
                            commit: encode(&commit.commit),
                            group_info: commit.group_info.as_deref().map(encode),
                            welcomes: None,
                        };
                        match engine.api().e2ee_commit(&group_b64(&group), &body).await {
                            Ok(_) => {
                                let _ = device.confirm_commit(&group);
                            }
                            Err(_) => {
                                let _ = device.discard_commit(&group);
                            }
                        }
                    }
                }
            }
            engine.refresh_directory(&mut inner).await;
        });
    }

    /// Set this device up again after the account started fresh elsewhere:
    /// it becomes a new device waiting for approval, and keeps what it stored.
    pub fn set_up_again(&self) {
        self.spawn(move |engine| async move {
            let mut inner = engine.shared.inner.lock().await;
            let result: Result<()> = async {
                inner.device = None;
                let vault = inner.vault.as_ref().ok_or("not-ready")?;
                vault.drop_device()?;
                let device = Device::create_device(vault.device_storage(), &inner.account)
                    .map_err(core_err)?;
                if let Ok(d) = engine.api().e2ee_directory().await
                    && let Some(id) = d.identity
                {
                    inner.generation = id.generation;
                }
                inner.device = Some(device);
                engine.wait_for_approval(&mut inner).await
            }
            .await;
            if let Err(e) = result {
                engine.set_device(DeviceState::Failed(e));
            }
        });
    }

    /// Local search over decrypted messages, newest first: (message ID,
    /// conversation, sender account, text, time).
    pub async fn search(&self, query: &str, limit: usize) -> Vec<(String, Plain)> {
        let inner = self.shared.inner.lock().await;
        inner
            .vault
            .as_ref()
            .map(|v| {
                v.search(query, limit)
                    .into_iter()
                    .map(|(id, p)| (id, p.clone()))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Download and decrypt a message's `index`th file.
    pub async fn open_file(&self, message: &str, index: usize) -> Result<OpenedFile> {
        let attachment = {
            let inner = self.shared.inner.lock().await;
            match inner.vault.as_ref().and_then(|v| v.message(message)) {
                Some(Outcome::Plain(p)) => p.attachments.get(index).cloned(),
                _ => None,
            }
        }
        .ok_or("this file is not on this device")?;
        let sealed = self
            .api()
            .e2ee_download(&attachment.url, MAX_FILE)
            .await
            .map_err(api_err)?;
        let bytes = files::open(&sealed, &attachment.key, &attachment.sha256)?;
        Ok(OpenedFile {
            name: attachment.name,
            mime: attachment.mime,
            bytes,
        })
    }

    /// Every half minute, or every few seconds while waiting for approval.
    async fn tick(&self) -> Duration {
        let mut inner = self.shared.inner.lock().await;
        if !inner.started {
            if let Err(e) = self.setup(&mut inner).await {
                log::warn!("e2ee: set-up: {e}");
                self.set_device(DeviceState::Failed(e));
                return SYNC_EVERY;
            }
            if inner.started {
                drop(inner);
                let all: Vec<String> = self
                    .shared
                    .handle
                    .read()
                    .timelines
                    .keys()
                    .cloned()
                    .collect();
                self.scan(all);
                return WAITING_EVERY;
            }
            return WAITING_EVERY;
        }
        self.relay(&mut inner).await;
        if let Some(waiting) = &inner.waiting {
            // The same request, so the code shown stays good.
            if waiting.posted.is_none_or(|at| at.elapsed() > REQUEST_AGAIN) {
                self.post_request(&mut inner).await;
            }
            return WAITING_EVERY;
        }
        if inner
            .device
            .as_ref()
            .is_some_and(|d| d.certificate().is_some())
        {
            self.sync_groups(&mut inner).await;
            self.add_approved(&mut inner).await;
            self.refresh_directory(&mut inner).await;
            self.count_waiting(&inner).await;
        }
        if inner.to_add.is_empty() {
            SYNC_EVERY
        } else {
            WAITING_EVERY
        }
    }

    /// Sync now, as when a settings page opens; at most every two seconds,
    /// however often the page draws.
    pub fn poke(&self) {
        {
            let mut last = self.shared.poked.lock().unwrap();
            if last.is_some_and(|at| at.elapsed() < POKE_EVERY) {
                return;
            }
            *last = Some(std::time::Instant::now());
        }
        self.spawn(|engine| async move {
            engine.tick().await;
        });
    }
}

/// Ask the backend whether a direct conversation is encrypted, once, for
/// [`crate::store::e2ee::guarded`]. Works without an engine (one that could
/// not start still must not send in the clear).
pub fn check_conversation(handle: &Handle, conversation: &str) {
    let asked = {
        let w = handle.read();
        w.capabilities.encryption.is_none()
            || w.e2ee.server.contains_key(conversation)
            || w.e2ee.asking.contains(conversation)
            || w.conversation(conversation)
                .is_none_or(|c| c.kind != c::ConversationKind::Direct)
    };
    if asked {
        return;
    }
    let id = conversation.to_string();
    handle.change(|w| {
        w.e2ee.asking.insert(id.clone());
        Changes::default()
    });
    let request = id.clone();
    handle.call(
        move |api| async move { api.e2ee_conversation(&request).await },
        move |w, result| {
            w.e2ee.asking.remove(&id);
            match result {
                Ok(state) => {
                    w.e2ee.server.insert(id.clone(), state.encrypted);
                }
                // Not a conversation encryption works in: nothing to guard.
                Err(ApiError::Http { status: 404, .. }) => {
                    w.e2ee.server.insert(id.clone(), false);
                }
                Err(_) => {}
            }
            Changes::topic_id(view::TOPIC, id)
        },
    );
}

/// A conversation record as the web client keeps it (its group in hex).
fn conversation_for_web(c: &Conversation) -> ConversationRecord {
    ConversationRecord {
        conversation_id: c.conversation.clone(),
        group_id: c
            .group
            .as_deref()
            .and_then(|g| from_b64url(g).ok())
            .map(|g| super::hex(&g))
            .unwrap_or_default(),
        peer: c.peer.clone(),
        on: c.on,
        by: (!c.by.is_empty()).then(|| c.by.clone()),
        at: Some(c.since),
    }
}

/// A web client's conversation record, or `None` when it is not one.
fn conversation_from_web(r: ConversationRecord) -> Option<Conversation> {
    if r.conversation_id.is_empty() || r.peer.is_empty() {
        return None;
    }
    let group = if r.group_id.is_empty() {
        None
    } else {
        Some(b64url(&from_hex(&r.group_id)?))
    };
    Some(Conversation {
        conversation: r.conversation_id,
        peer: r.peer,
        group,
        on: r.on,
        since: r.at.unwrap_or_default(),
        by: r.by.unwrap_or_default(),
    })
}

fn from_hex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(text.get(i..i + 2)?, 16).ok())
        .collect()
}

/// Sends queued text one at a time, in order, until the engine is dropped.
async fn send_queued(
    weak: Weak<Shared>,
    mut outgoing: tokio::sync::mpsc::UnboundedReceiver<Outgoing>,
) {
    while let Some(out) = outgoing.recv().await {
        let Some(shared) = weak.upgrade() else {
            return;
        };
        Engine { shared }.send_now(out).await;
    }
}

/// The engine's own loop: set up, then sync, until the engine is dropped.
async fn run(weak: Weak<Shared>) {
    loop {
        let wait = {
            let Some(shared) = weak.upgrade() else {
                return;
            };
            Engine { shared }.tick().await
        };
        tokio::time::sleep(wait).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::SnapshotBuilder;

    fn workspace() -> Workspace {
        SnapshotBuilder::new()
            .with_network(|n| n.features.encryption = true)
            .member("m-bob", "bob")
            .member("m-carol", "carol")
            .direct("dm", "m-bob")
            .capabilities(|caps| caps.encryption = Some(c::EncryptionCapability::default()))
            .workspace()
    }

    #[test]
    fn the_person_menu_gives_the_locks_reasons() {
        let mut w = workspace();
        assert_eq!(direct_with(&w, "n1", "m-bob").as_deref(), Some("dm"));
        assert_eq!(direct_with(&w, "n1", "m-carol"), None);

        assert_eq!(blocked_with(&w, "n1", "m-carol"), Some(Blocked::NotReady));
        w.e2ee.device = DeviceState::Ready { approver: true };
        assert_eq!(
            blocked_with(&w, "n1", "m-carol"),
            None,
            "no conversation yet"
        );
        assert_eq!(blocked_with(&w, "n1", "m-bob"), None);
        w.e2ee.peers.insert("carol".into(), false);
        assert_eq!(
            blocked_with(&w, "n1", "m-carol"),
            Some(Blocked::PeerNotSetUp)
        );

        w.members.get_mut("m-carol").unwrap().account = "*".into();
        assert_eq!(blocked_with(&w, "n1", "m-carol"), Some(Blocked::NoAccount));
        w.members.get_mut("m-bob").unwrap().role = c::MemberRole::Bot;
        assert_eq!(blocked_with(&w, "n1", "m-bob"), Some(Blocked::NoAccount));

        w.networks[0].features.encryption = false;
        assert_eq!(blocked_with(&w, "n1", "m-carol"), Some(Blocked::Network));
        w.capabilities.encryption = None;
        assert_eq!(blocked_with(&w, "n1", "m-carol"), Some(Blocked::Off));
    }

    #[test]
    fn a_conversation_opened_from_the_menu_knows_its_account() {
        let mut w = workspace();
        let changes = note_peer(&mut w, "dm-new", "m-carol");
        assert!(changes.touches_id(view::TOPIC, "dm-new"));
        assert_eq!(w.e2ee.accounts.get("dm-new"), Some(&Some("carol".into())));
        assert!(note_peer(&mut w, "dm-new", "m-carol").is_empty(), "known");
    }
}

//! This device's encrypted store: the core's state blob and every encrypted
//! message this device has read or sent, in its own folder, sealed with a key
//! kept in the operating system's credential store.
//!
//! - `device.state` holds the core's state blob (keys, groups, pins),
//!   replaced atomically on every change.
//! - `vault.log` is an append-only log of [`Record`]s, one sealed frame each.
//!   Opening reads it whole into memory and builds the search index; a frame
//!   cut short by a crash is dropped.
//!
//! Every frame is XChaCha20-Poly1305 under the vault key with a random nonce,
//! so neither file says anything without the key. Losing the key loses the
//! store: by design there is no other copy (ircord/issues#19).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs::{self, File, OpenOptions};
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use serde::{Deserialize, Serialize};

use super::core as e2ee;

/// Bytes of a vault key.
pub const KEY_LEN: usize = 32;
const NONCE_LEN: usize = 24;
const LOG: &str = "vault.log";
const STATE: &str = "device.state";
/// Bound into every frame, so a frame of one file cannot pass for the other.
const AAD_LOG: &[u8] = b"ircord-gpui e2ee vault log v1";
const AAD_STATE: &[u8] = b"ircord-gpui e2ee device state v1";
/// People invited to a group before they set up encryption: a file of its
/// own beside the log, so versions that do not know it still open the log.
const PENDING: &str = "pending.sealed";
const AAD_PENDING: &[u8] = b"ircord-gpui e2ee pending invitees v1";

/// Someone invited to an encrypted group who has no encryption keys yet:
/// their account, and the nick their notice went to.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Invitee {
    pub account: String,
    pub nick: String,
}
/// A frame larger than this is damage, not data.
const MAX_FRAME: usize = 1 << 24;

/// The key that seals this device's store.
#[derive(Clone)]
pub struct VaultKey([u8; KEY_LEN]);

impl VaultKey {
    pub fn random() -> Self {
        Self(rand::random())
    }

    pub fn from_bytes(bytes: [u8; KEY_LEN]) -> Self {
        Self(bytes)
    }

    /// The key as the credential store keeps it: base64url.
    pub fn encode(&self) -> String {
        super::b64url(&self.0)
    }

    pub fn decode(text: &str) -> Option<Self> {
        let bytes = super::from_b64url(text.trim()).ok()?;
        Some(Self(bytes.try_into().ok()?))
    }

    fn cipher(&self) -> XChaCha20Poly1305 {
        XChaCha20Poly1305::new((&self.0).into())
    }

    fn seal(&self, aad: &[u8], plain: &[u8]) -> Vec<u8> {
        let nonce: [u8; NONCE_LEN] = rand::random();
        let mut out = nonce.to_vec();
        let sealed = self
            .cipher()
            .encrypt(XNonce::from_slice(&nonce), Payload { msg: plain, aad })
            .expect("sealing in memory does not fail");
        out.extend(sealed);
        out
    }

    fn open(&self, aad: &[u8], sealed: &[u8]) -> Option<Vec<u8>> {
        if sealed.len() < NONCE_LEN {
            return None;
        }
        let (nonce, body) = sealed.split_at(NONCE_LEN);
        self.cipher()
            .decrypt(XNonce::from_slice(nonce), Payload { msg: body, aad })
            .ok()
    }
}

impl std::fmt::Debug for VaultKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("VaultKey(..)")
    }
}

/// A decrypted message, as this device stores it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Plain {
    pub conversation: String,
    /// The payload's id, which replies and edits refer to.
    pub inner_id: String,
    /// The sender's account, from the MLS credential.
    pub sender: String,
    pub text: String,
    #[serde(default)]
    pub action: bool,
    /// The payload id this message replies to.
    #[serde(default)]
    pub reply: Option<String>,
    #[serde(default)]
    pub attachments: Vec<e2ee::Attachment>,
    #[serde(default)]
    pub gif: Option<e2ee::Gif>,
    /// When it was sent or received, in milliseconds since the epoch.
    pub at: i64,
}

/// Why a message cannot be shown.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Unreadable {
    /// Sent before this device was added: its keys are on other devices.
    BeforeDevice,
    /// This device sent it, and lost the copy it kept.
    OwnLost,
    /// The sender's encryption keys are not the ones this device trusts.
    IdentityChanged,
    /// The message claims a sender other than the account that sent it.
    Forged,
    /// Anything else the core refused, with its reason.
    Failed(String),
}

/// What this device made of one encrypted message.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "as", rename_all = "kebab-case")]
pub enum Outcome {
    Plain(Plain),
    /// Encryption turned on or off, by `by` (an account), in `group`
    /// (base64url; absent in what older versions stored).
    Control {
        on: bool,
        by: String,
        at: i64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        group: Option<String>,
    },
    /// Someone left an encrypted group (`by`, an account).
    Left {
        by: String,
        at: i64,
    },
    /// Not shown: a part of a longer message, or an event this client does
    /// not show yet.
    Hidden,
    Unreadable {
        why: Unreadable,
    },
}

/// One encrypted conversation, as this device remembers it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Conversation {
    pub conversation: String,
    /// The other account.
    pub peer: String,
    /// The MLS group as the backend names it: 32 hex digits (records
    /// written by 0.9.0 and earlier: base64url); absent once the account or the peer started
    /// fresh, until a new group replaces it.
    pub group: Option<String>,
    /// Encryption is on: nothing is sent in the clear.
    pub on: bool,
    /// When it was turned on or off, and by whom (an account).
    pub since: i64,
    pub by: String,
}

/// What the log holds, one frame each. A later frame replaces an earlier one
/// with the same key.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "r", rename_all = "kebab-case")]
pub enum Record {
    /// A server message's outcome, by its message ID.
    Message {
        id: String,
        outcome: Outcome,
    },
    /// A message this device sent, by the client ID it was sent with, until
    /// its server ID is known.
    Sent {
        client_id: String,
        plain: Plain,
    },
    /// Something else this device sent (a control event, a later part of a
    /// long message), by its client ID: how it is shown once linked.
    SentAs {
        client_id: String,
        outcome: Outcome,
    },
    /// The server ID a sent message got.
    Linked {
        client_id: String,
        id: String,
    },
    Conversation(Conversation),
    /// The conversation's own copy of this device's choice to forget it.
    Forget {
        conversation: String,
    },
}

/// The store, open.
pub struct Vault {
    dir: PathBuf,
    key: VaultKey,
    log: Option<File>,
    messages: HashMap<String, Outcome>,
    sent: HashMap<String, Plain>,
    sent_as: HashMap<String, Outcome>,
    conversations: BTreeMap<String, Conversation>,
    /// Payload id → server message ID, for replies.
    inner: HashMap<String, String>,
    index: Index,
    frames: usize,
}

impl Vault {
    /// Open (or create) the store in `dir`. A frame that does not open with
    /// `key` is an error: the wrong key, or a store from another device.
    pub fn open(dir: &Path, key: VaultKey) -> Result<Self, String> {
        fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        restrict(dir);
        let mut vault = Vault {
            dir: dir.to_path_buf(),
            key,
            log: None,
            messages: HashMap::new(),
            sent: HashMap::new(),
            sent_as: HashMap::new(),
            conversations: BTreeMap::new(),
            inner: HashMap::new(),
            index: Index::default(),
            frames: 0,
        };
        let path = dir.join(LOG);
        let bytes = match fs::read(&path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(format!("{}: {e}", path.display())),
        };
        let mut at = 0;
        let mut good = 0;
        while at + 4 <= bytes.len() {
            let len = u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
            if len > MAX_FRAME || at + 4 + len > bytes.len() {
                break;
            }
            let frame = &bytes[at + 4..at + 4 + len];
            let last = at + 4 + len == bytes.len();
            let Some(plain) = vault.key.open(AAD_LOG, frame) else {
                // The last frame of a store that opened so far was torn by a
                // crash; anything else is the wrong key or damage.
                if last && good > 0 {
                    break;
                }
                return Err("this device's encrypted store does not open with its key".into());
            };
            let record: Record = serde_json::from_slice(&plain)
                .map_err(|e| format!("this device's encrypted store is damaged: {e}"))?;
            vault.apply(record);
            vault.frames += 1;
            at += 4 + len;
            good = at;
        }
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        restrict(&path);
        if good < bytes.len() {
            // A frame cut short by a crash: drop it.
            file.set_len(good as u64)
                .map_err(|e| format!("{}: {e}", path.display()))?;
        }
        vault.log = Some(file);
        if vault.frames > 2 * vault.live().max(500) {
            vault.compact()?;
        }
        Ok(vault)
    }

    /// The people invited to each group (by its backend name) who have no
    /// encryption keys yet. Empty when the file is missing or does not open.
    pub fn pending(&self) -> BTreeMap<String, Vec<Invitee>> {
        let Ok(sealed) = fs::read(self.dir.join(PENDING)) else {
            return BTreeMap::new();
        };
        self.key
            .open(AAD_PENDING, &sealed)
            .and_then(|plain| serde_json::from_slice(&plain).ok())
            .unwrap_or_default()
    }

    /// Keep `pending` in place of what [`Vault::pending`] read.
    pub fn set_pending(&self, pending: &BTreeMap<String, Vec<Invitee>>) -> Result<(), String> {
        let plain = serde_json::to_vec(pending).expect("invitees serialise");
        write_atomic(&self.dir.join(PENDING), &self.key.seal(AAD_PENDING, &plain))
    }

    /// Whether a store exists in `dir`.
    pub fn exists(dir: &Path) -> bool {
        dir.join(LOG).exists() || dir.join(STATE).exists()
    }

    /// The core's storage, sealed with this store's key.
    pub fn device_storage(&self) -> SealedState {
        SealedState {
            path: self.dir.join(STATE),
            key: self.key.clone(),
        }
    }

    /// The core's storage for a device being made, beside the current one
    /// (`device.state.new`), until [`Self::promote_device`] makes it current.
    pub fn new_device_storage(&self) -> SealedState {
        SealedState {
            path: self.dir.join(format!("{STATE}.new")),
            key: self.key.clone(),
        }
    }

    /// Make the device made with [`Self::new_device_storage`] the current one.
    pub fn promote_device(&self) -> Result<(), String> {
        fs::rename(self.dir.join(format!("{STATE}.new")), self.dir.join(STATE))
            .map_err(|e| e.to_string())
    }

    /// Forget a device being made that did not become current.
    pub fn discard_new_device(&self) {
        let _ = fs::remove_file(self.dir.join(format!("{STATE}.new")));
    }

    pub fn has_device(&self) -> bool {
        self.dir.join(STATE).exists()
    }

    /// Throw the core's state away, as starting fresh does; messages stay.
    pub fn drop_device(&self) -> Result<(), String> {
        match fs::remove_file(self.dir.join(STATE)) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }

    fn live(&self) -> usize {
        self.messages.len() + self.sent.len() + self.sent_as.len() + self.conversations.len()
    }

    /// Write `record` durably, then apply it.
    pub fn put(&mut self, record: Record) -> Result<(), String> {
        let frame = self.frame(&record);
        let file = self.log.as_mut().ok_or("the store is closed")?;
        let before = file.metadata().map_err(|e| e.to_string())?.len();
        let written = file.write_all(&frame).and_then(|()| file.sync_data());
        if let Err(e) = written {
            // Leave no partial frame for later frames to follow.
            let _ = file.set_len(before);
            return Err(e.to_string());
        }
        self.frames += 1;
        self.apply(record);
        Ok(())
    }

    fn frame(&self, record: &Record) -> Vec<u8> {
        let plain = serde_json::to_vec(record).expect("records serialise");
        let sealed = self.key.seal(AAD_LOG, &plain);
        let mut frame = (sealed.len() as u32).to_le_bytes().to_vec();
        frame.extend(sealed);
        frame
    }

    fn apply(&mut self, record: Record) {
        match record {
            Record::Message { id, outcome } => {
                if let Outcome::Plain(p) = &outcome {
                    self.inner.insert(p.inner_id.clone(), id.clone());
                    self.index.add(&id, &p.text);
                }
                self.messages.insert(id, outcome);
            }
            Record::Sent { client_id, plain } => {
                self.sent.insert(client_id, plain);
            }
            Record::SentAs { client_id, outcome } => {
                self.sent_as.insert(client_id, outcome);
            }
            Record::Linked { client_id, id } => {
                if let Some(outcome) = self.sent_as.remove(&client_id) {
                    self.messages.insert(id.clone(), outcome);
                }
                if let Some(plain) = self.sent.remove(&client_id) {
                    self.inner.insert(plain.inner_id.clone(), id.clone());
                    self.index.add(&id, &plain.text);
                    self.messages.insert(id, Outcome::Plain(plain));
                }
            }
            Record::Conversation(c) => {
                self.conversations.insert(c.conversation.clone(), c);
            }
            Record::Forget { conversation } => {
                self.conversations.remove(&conversation);
                let ids: Vec<String> = self
                    .messages
                    .iter()
                    .filter(
                        |(_, o)| matches!(o, Outcome::Plain(p) if p.conversation == conversation),
                    )
                    .map(|(id, _)| id.clone())
                    .collect();
                for id in ids {
                    self.index.remove(&id);
                    self.messages.remove(&id);
                }
            }
        }
    }

    /// Rewrite the log with only what is live.
    fn compact(&mut self) -> Result<(), String> {
        let mut records: Vec<Record> = Vec::new();
        records.extend(
            self.conversations
                .values()
                .cloned()
                .map(Record::Conversation),
        );
        records.extend(self.sent.iter().map(|(c, p)| Record::Sent {
            client_id: c.clone(),
            plain: p.clone(),
        }));
        records.extend(self.sent_as.iter().map(|(c, o)| Record::SentAs {
            client_id: c.clone(),
            outcome: o.clone(),
        }));
        records.extend(self.messages.iter().map(|(id, o)| Record::Message {
            id: id.clone(),
            outcome: o.clone(),
        }));
        let mut bytes = Vec::new();
        for r in &records {
            bytes.extend(self.frame(r));
        }
        let path = self.dir.join(LOG);
        write_atomic(&path, &bytes)?;
        self.log = Some(
            OpenOptions::new()
                .append(true)
                .open(&path)
                .map_err(|e| e.to_string())?,
        );
        self.frames = records.len();
        Ok(())
    }

    pub fn message(&self, id: &str) -> Option<&Outcome> {
        self.messages.get(id)
    }

    /// A message this device sent, by its client ID, until it is linked.
    pub fn sent(&self, client_id: &str) -> Option<&Plain> {
        self.sent.get(client_id)
    }

    /// Whether this device sent something other than a message with this
    /// client ID.
    pub fn sent_as(&self, client_id: &str) -> bool {
        self.sent_as.contains_key(client_id)
    }

    /// The server message ID of a payload id, for replies.
    pub fn by_inner(&self, inner: &str) -> Option<&str> {
        self.inner.get(inner).map(String::as_str)
    }

    pub fn conversation(&self, id: &str) -> Option<&Conversation> {
        self.conversations.get(id)
    }

    pub fn conversations(&self) -> impl Iterator<Item = &Conversation> {
        self.conversations.values()
    }

    /// The conversation bound to a group.
    pub fn conversation_of_group(&self, group: &str) -> Option<&Conversation> {
        self.conversations
            .values()
            .find(|c| c.group.as_deref() == Some(group))
    }

    /// Stored messages whose text has every word of `query`, newest first.
    pub fn search(&self, query: &str, limit: usize) -> Vec<(String, &Plain)> {
        let mut hits: Vec<(String, &Plain)> = self
            .index
            .find(query)
            .into_iter()
            .filter_map(|id| match self.messages.get(&id) {
                Some(Outcome::Plain(p)) => Some((id, p)),
                _ => None,
            })
            .collect();
        hits.sort_by(|a, b| b.1.at.cmp(&a.1.at).then_with(|| a.0.cmp(&b.0)));
        hits.truncate(limit);
        hits
    }

    pub fn len(&self) -> usize {
        self.messages
            .values()
            .filter(|o| matches!(o, Outcome::Plain(_)))
            .count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// A word index over stored message text: each word, lowercased, points to
/// the messages that have it. A query matches messages that have, for each of
/// its words, a word starting with it.
#[derive(Default)]
struct Index {
    words: BTreeMap<String, BTreeSet<String>>,
    of: HashMap<String, Vec<String>>,
}

fn words(text: &str) -> impl Iterator<Item = String> + '_ {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
}

impl Index {
    fn add(&mut self, id: &str, text: &str) {
        self.remove(id);
        let mut mine: Vec<String> = words(text).collect();
        mine.sort();
        mine.dedup();
        for w in &mine {
            self.words
                .entry(w.clone())
                .or_default()
                .insert(id.to_string());
        }
        self.of.insert(id.to_string(), mine);
    }

    fn remove(&mut self, id: &str) {
        for w in self.of.remove(id).unwrap_or_default() {
            if let Some(ids) = self.words.get_mut(&w) {
                ids.remove(id);
                if ids.is_empty() {
                    self.words.remove(&w);
                }
            }
        }
    }

    fn find(&self, query: &str) -> BTreeSet<String> {
        let mut result: Option<BTreeSet<String>> = None;
        for q in words(query) {
            let mut hits = BTreeSet::new();
            for (_, ids) in self
                .words
                .range(q.clone()..)
                .take_while(|(w, _)| w.starts_with(&q))
            {
                hits.extend(ids.iter().cloned());
            }
            result = Some(match result {
                None => hits,
                Some(r) => r.intersection(&hits).cloned().collect(),
            });
        }
        result.unwrap_or_default()
    }
}

/// The core's state blob in one sealed file, replaced atomically.
pub struct SealedState {
    path: PathBuf,
    key: VaultKey,
}

impl e2ee::Storage for SealedState {
    fn load(&self) -> Result<Option<Vec<u8>>, String> {
        let mut file = match File::open(&self.path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(format!("{}: {e}", self.path.display())),
        };
        let mut sealed = Vec::new();
        file.read_to_end(&mut sealed).map_err(|e| e.to_string())?;
        self.key
            .open(AAD_STATE, &sealed)
            .map(Some)
            .ok_or_else(|| "this device's encryption keys do not open with its key".to_string())
    }

    fn save(&mut self, state: &[u8]) -> Result<(), String> {
        write_atomic(&self.path, &self.key.seal(AAD_STATE, state))
    }
}

/// Write `bytes` to `path` so a crash leaves the old file or the new one.
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let err = |e: std::io::Error| format!("{}: {e}", path.display());
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".tmp");
    let tmp = path.with_file_name(name);
    let mut file = private_file(&tmp).map_err(err)?;
    file.write_all(bytes).map_err(err)?;
    file.sync_all().map_err(err)?;
    drop(file);
    fs::rename(&tmp, path).map_err(err)?;
    if let Some(dir) = path.parent()
        && let Ok(d) = File::open(dir)
    {
        let _ = d.sync_all();
    }
    Ok(())
}

fn private_file(path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    options.open(path)
}

/// Owner only, on Unix; elsewhere the user's profile folder already is.
fn restrict(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = if path.is_dir() { 0o700 } else { 0o600 };
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(mode));
    }
    #[cfg(not(unix))]
    let _ = path;
}

#[cfg(test)]
mod tests {
    use super::*;
    use e2ee::Storage as _;

    fn plain(conversation: &str, inner: &str, text: &str, at: i64) -> Plain {
        Plain {
            conversation: conversation.into(),
            inner_id: inner.into(),
            sender: "alice".into(),
            text: text.into(),
            action: false,
            reply: None,
            attachments: Vec::new(),
            gif: None,
            at,
        }
    }

    #[test]
    fn messages_survive_a_reopen_and_are_found_by_word() {
        let dir = tempfile::tempdir().unwrap();
        let key = VaultKey::random();
        {
            let mut v = Vault::open(dir.path(), key.clone()).unwrap();
            v.put(Record::Message {
                id: "m1".into(),
                outcome: Outcome::Plain(plain("dm", "i1", "Weekend plans: hiking", 1)),
            })
            .unwrap();
            v.put(Record::Sent {
                client_id: "c1".into(),
                plain: plain("dm", "i2", "Hiking sounds good", 2),
            })
            .unwrap();
            v.put(Record::Linked {
                client_id: "c1".into(),
                id: "m2".into(),
            })
            .unwrap();
        }
        let v = Vault::open(dir.path(), key).unwrap();
        let hits: Vec<_> = v.search("hik", 10).into_iter().map(|(id, _)| id).collect();
        assert_eq!(hits, ["m2", "m1"]);
        assert_eq!(v.search("weekend HIKING", 10).len(), 1);
        assert!(v.search("sailing", 10).is_empty());
        assert_eq!(v.by_inner("i2"), Some("m2"));
        assert!(v.sent("c1").is_none());
    }

    #[test]
    fn pending_invitees_survive_a_reopen_sealed_and_beside_the_log() {
        let dir = tempfile::tempdir().unwrap();
        let key = VaultKey::random();
        let mut pending = BTreeMap::new();
        pending.insert(
            "g1".to_string(),
            vec![Invitee {
                account: "sam".into(),
                nick: "sammy".into(),
            }],
        );
        {
            let v = Vault::open(dir.path(), key.clone()).unwrap();
            assert!(v.pending().is_empty());
            v.set_pending(&pending).unwrap();
        }
        let raw = std::fs::read(dir.path().join(PENDING)).unwrap();
        assert!(!raw.windows(5).any(|w| w == b"sammy"));
        assert_eq!(Vault::open(dir.path(), key).unwrap().pending(), pending);
        let other = Vault::open(tempfile::tempdir().unwrap().path(), VaultKey::random()).unwrap();
        assert!(other.pending().is_empty());
    }

    #[test]
    fn nothing_readable_is_on_disk_and_another_key_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let key = VaultKey::random();
        let mut v = Vault::open(dir.path(), key.clone()).unwrap();
        v.put(Record::Message {
            id: "m1".into(),
            outcome: Outcome::Plain(plain("dm", "i1", "the secret word is marmalade", 1)),
        })
        .unwrap();
        let mut state = v.device_storage();
        state.save(b"device keys marmalade").unwrap();
        drop(v);
        for name in [LOG, STATE] {
            let bytes = fs::read(dir.path().join(name)).unwrap();
            assert!(
                !String::from_utf8_lossy(&bytes).contains("marmalade"),
                "{name}"
            );
        }
        assert_eq!(
            state.load().unwrap().as_deref(),
            Some(&b"device keys marmalade"[..])
        );
        assert!(Vault::open(dir.path(), VaultKey::random()).is_err());
        let other = SealedState {
            path: dir.path().join(STATE),
            key: VaultKey::random(),
        };
        assert!(other.load().is_err());
    }

    #[test]
    fn a_frame_cut_short_is_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let key = VaultKey::random();
        let mut v = Vault::open(dir.path(), key.clone()).unwrap();
        v.put(Record::Message {
            id: "m1".into(),
            outcome: Outcome::Hidden,
        })
        .unwrap();
        drop(v);
        let path = dir.path().join(LOG);
        let mut f = OpenOptions::new().append(true).open(&path).unwrap();
        f.write_all(&[200, 0, 0, 0, 1, 2, 3]).unwrap();
        drop(f);
        let mut v = Vault::open(dir.path(), key.clone()).unwrap();
        assert_eq!(v.message("m1"), Some(&Outcome::Hidden));
        // A last frame of full length that does not open is torn too.
        let mut torn = 40u32.to_le_bytes().to_vec();
        torn.extend([7u8; 40]);
        v.log.as_mut().unwrap().write_all(&torn).unwrap();
        drop(v);
        let mut v = Vault::open(dir.path(), key.clone()).unwrap();
        assert_eq!(v.message("m1"), Some(&Outcome::Hidden));
        v.put(Record::Message {
            id: "m2".into(),
            outcome: Outcome::Hidden,
        })
        .unwrap();
        drop(v);
        let v = Vault::open(dir.path(), key).unwrap();
        assert!(v.message("m2").is_some());
    }

    #[test]
    fn forgetting_a_conversation_drops_its_messages() {
        let dir = tempfile::tempdir().unwrap();
        let mut v = Vault::open(dir.path(), VaultKey::random()).unwrap();
        v.put(Record::Conversation(Conversation {
            conversation: "dm".into(),
            peer: "bob".into(),
            group: Some("g".into()),
            on: true,
            since: 1,
            by: "alice".into(),
        }))
        .unwrap();
        v.put(Record::Message {
            id: "m1".into(),
            outcome: Outcome::Plain(plain("dm", "i1", "hello", 1)),
        })
        .unwrap();
        assert_eq!(v.conversation_of_group("g").unwrap().peer, "bob");
        v.put(Record::Forget {
            conversation: "dm".into(),
        })
        .unwrap();
        assert!(v.conversation("dm").is_none());
        assert!(v.search("hello", 5).is_empty());
    }
}

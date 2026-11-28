//! Approving a new device of the same account (ircord/issues#19, section
//! 4.2). The core's wire specification leaves this handshake out of its
//! version 1; this is the GPUI client's, written down in
//! `docs/e2ee.md` so the web client can speak it too.
//!
//! Four messages pass through the backend's approval relay, each a small
//! JSON object:
//!
//! 1. new → approver, `request`: a commitment to the new device's ephemeral
//!    X25519 key and certificate request, and a label ("IRCord GPUI on
//!    Linux"). The new device also shows the commitment in a QR code.
//! 2. approver → new, `accept`: the approver's ephemeral key.
//! 3. new → approver, `reveal`: the new device's ephemeral key and its
//!    certificate request, which must match the commitment.
//! 4. approver → new, `approve`: the certificate and what the new device
//!    should know (pinned identities, encrypted conversations), sealed with a
//!    key from the two ephemeral keys.
//!
//! After 2 and 3 both devices show the same eight-digit code, derived from
//! the shared secret and everything exchanged. The person checks that the
//! codes match before the approver sends 4. Because the new device committed
//! to its key before it saw the approver's, a relay in the middle cannot
//! choose keys that make two codes agree; and without the shared secret it
//! cannot open or forge message 4.

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Nonce};
use hkdf::Hkdf;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use x25519_dalek::{PublicKey, StaticSecret};

use super::{b64url, from_b64url};

/// The handshake's version, in every message and the QR code.
pub const VERSION: u32 = 1;
/// What the QR code's text starts with.
pub const QR_PREFIX: &str = "ircord-e2ee-approve:1:";
const LABEL_COMMIT: &[u8] = b"ircord-e2ee/1 approval commit";
const LABEL_SALT: &[u8] = b"ircord-e2ee/1 approval";
const INFO_CODE: &[u8] = b"code";
const INFO_KEY: &[u8] = b"key";
const AAD: &[u8] = b"ircord-e2ee/1 approval transfer";

/// One relayed message.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "lowercase")]
pub enum Message {
    Request {
        v: u32,
        /// SHA-256 over the new device's ephemeral key and request, hex.
        commit: String,
        label: String,
    },
    Accept {
        v: u32,
        eph: String,
    },
    Reveal {
        v: u32,
        eph: String,
        /// The core's certificate request, base64url.
        req: String,
    },
    Approve {
        v: u32,
        sealed: String,
    },
    /// The approver declined, or the codes did not match.
    Decline {
        v: u32,
    },
}

impl Message {
    pub fn to_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(self).expect("messages serialise")
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, String> {
        let m: Message =
            serde_json::from_slice(bytes).map_err(|e| format!("approval message: {e}"))?;
        let v = match &m {
            Message::Request { v, .. }
            | Message::Accept { v, .. }
            | Message::Reveal { v, .. }
            | Message::Approve { v, .. }
            | Message::Decline { v } => *v,
        };
        if v != VERSION {
            return Err(format!("approval message version {v}"));
        }
        Ok(m)
    }
}

/// What an approved device receives.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Transfer {
    /// The device certificate, base64url.
    pub certificate: String,
    /// Identities the approver pinned: account, identity key (base64url),
    /// whether the person verified it.
    pub pins: Vec<(String, String, bool)>,
    /// The account's encrypted conversations, so a server cannot make one
    /// look unencrypted to the new device.
    pub conversations: Vec<super::vault::Conversation>,
}

fn commitment(eph: &[u8; 32], request: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(LABEL_COMMIT);
    h.update([0]);
    h.update(eph);
    h.update(request);
    super::hex(&h.finalize())
}

fn key32(text: &str) -> Result<[u8; 32], String> {
    from_b64url(text)?
        .try_into()
        .map_err(|_| "approval key has the wrong length".to_string())
}

/// The secrets both sides derive: the code to compare and the transfer key.
struct Derived {
    code: String,
    key: [u8; 32],
}

fn derive(
    shared: &[u8; 32],
    new_eph: &[u8; 32],
    approver_eph: &[u8; 32],
    request: &[u8],
) -> Derived {
    let hk = Hkdf::<Sha256>::new(Some(LABEL_SALT), shared);
    let transcript = [new_eph.as_slice(), approver_eph.as_slice(), request].concat();
    let mut code = [0u8; 8];
    hk.expand_multi_info(&[INFO_CODE, &transcript], &mut code)
        .expect("8 bytes is a valid length");
    let mut key = [0u8; 32];
    hk.expand_multi_info(&[INFO_KEY, &transcript], &mut key)
        .expect("32 bytes is a valid length");
    let n = u64::from_be_bytes(code) % 100_000_000;
    let digits = format!("{n:08}");
    Derived {
        code: format!("{} {}", &digits[..4], &digits[4..]),
        key,
    }
}

/// The new device's side.
pub struct NewDevice {
    secret: StaticSecret,
    request: Vec<u8>,
    derived: Option<Derived>,
    approver_eph: Option<[u8; 32]>,
}

impl NewDevice {
    /// Start: `request` is the core's certificate request.
    pub fn start(request: Vec<u8>) -> Self {
        Self {
            secret: StaticSecret::from(rand::random::<[u8; 32]>()),
            request,
            derived: None,
            approver_eph: None,
        }
    }

    fn public(&self) -> [u8; 32] {
        PublicKey::from(&self.secret).to_bytes()
    }

    pub fn commitment(&self) -> String {
        commitment(&self.public(), &self.request)
    }

    pub fn request(&self, label: &str) -> Message {
        Message::Request {
            v: VERSION,
            commit: self.commitment(),
            label: label.chars().take(100).collect(),
        }
    }

    /// The QR code's text: the device ID and the commitment.
    pub fn qr_text(&self, device_id: &str) -> String {
        format!("{QR_PREFIX}{device_id}:{}", self.commitment())
    }

    /// The approver's `accept`: answer with `reveal`; the code is then known.
    pub fn accept(&mut self, eph: &str) -> Result<Message, String> {
        if self.approver_eph.is_some() {
            return Err("this device already answered an approval".into());
        }
        let theirs = key32(eph)?;
        let shared = self.secret.diffie_hellman(&PublicKey::from(theirs));
        if !shared.was_contributory() {
            return Err("the other device's key is not acceptable".into());
        }
        self.derived = Some(derive(
            shared.as_bytes(),
            &self.public(),
            &theirs,
            &self.request,
        ));
        self.approver_eph = Some(theirs);
        Ok(Message::Reveal {
            v: VERSION,
            eph: b64url(&self.public()),
            req: b64url(&self.request),
        })
    }

    /// The code to show, once the approver answered.
    pub fn code(&self) -> Option<&str> {
        self.derived.as_ref().map(|d| d.code.as_str())
    }

    /// Open the approver's `approve`.
    pub fn open(&self, sealed: &str) -> Result<Transfer, String> {
        let d = self
            .derived
            .as_ref()
            .ok_or("no approval is under way on this device")?;
        let sealed = from_b64url(sealed)?;
        let plain = ChaCha20Poly1305::new((&d.key).into())
            .decrypt(
                Nonce::from_slice(&[0; 12]),
                Payload {
                    msg: &sealed,
                    aad: AAD,
                },
            )
            .map_err(|_| {
                "the approval did not come from the device that showed the code".to_string()
            })?;
        serde_json::from_slice(&plain).map_err(|e| format!("approval: {e}"))
    }
}

/// The approving device's side.
pub struct Approver {
    secret: StaticSecret,
    commit: String,
    request: Option<Vec<u8>>,
    derived: Option<Derived>,
}

impl Approver {
    /// A `request` arrived: answer with `accept`.
    pub fn start(commit: &str) -> (Self, Message) {
        let me = Self {
            secret: StaticSecret::from(rand::random::<[u8; 32]>()),
            commit: commit.to_string(),
            request: None,
            derived: None,
        };
        let accept = Message::Accept {
            v: VERSION,
            eph: b64url(PublicKey::from(&me.secret).as_bytes()),
        };
        (me, accept)
    }

    /// The new device's `reveal`: check it against the commitment, and
    /// derive the code. Gives the certificate request to certify.
    pub fn reveal(&mut self, eph: &str, req: &str) -> Result<Vec<u8>, String> {
        let theirs = key32(eph)?;
        let request = from_b64url(req)?;
        if commitment(&theirs, &request) != self.commit {
            return Err("the new device's answer does not match its request".into());
        }
        let shared = self.secret.diffie_hellman(&PublicKey::from(theirs));
        if !shared.was_contributory() {
            return Err("the new device's key is not acceptable".into());
        }
        let mine = PublicKey::from(&self.secret).to_bytes();
        self.derived = Some(derive(shared.as_bytes(), &theirs, &mine, &request));
        self.request = Some(request.clone());
        Ok(request)
    }

    pub fn code(&self) -> Option<&str> {
        self.derived.as_ref().map(|d| d.code.as_str())
    }

    /// The certificate request the new device revealed.
    pub fn request(&self) -> Option<&[u8]> {
        self.request.as_deref()
    }

    /// Seal what the new device receives, once the person compared codes.
    pub fn approve(&self, transfer: &Transfer) -> Result<Message, String> {
        let d = self
            .derived
            .as_ref()
            .ok_or("the new device has not answered yet")?;
        let plain = serde_json::to_vec(transfer).map_err(|e| e.to_string())?;
        // The key is used for this one message, so a fixed nonce is safe.
        let sealed = ChaCha20Poly1305::new((&d.key).into())
            .encrypt(
                Nonce::from_slice(&[0; 12]),
                Payload {
                    msg: &plain,
                    aad: AAD,
                },
            )
            .map_err(|_| "sealing failed".to_string())?;
        Ok(Message::Approve {
            v: VERSION,
            sealed: b64url(&sealed),
        })
    }
}

/// Read a QR code's text: the device ID and the commitment.
pub fn parse_qr(text: &str) -> Option<(String, String)> {
    let rest = text.trim().strip_prefix(QR_PREFIX)?;
    let (device, commit) = rest.split_once(':')?;
    (!device.is_empty() && commit.len() == 64).then(|| (device.to_string(), commit.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn transfer() -> Transfer {
        Transfer {
            certificate: "Y2VydA".into(),
            pins: vec![("bob".into(), "a2V5".into(), true)],
            conversations: Vec::new(),
        }
    }

    fn relay(m: &Message) -> Message {
        Message::from_bytes(&m.to_bytes()).unwrap()
    }

    #[test]
    fn both_sides_show_one_code_and_the_transfer_opens() {
        let mut new = NewDevice::start(b"certificate request".to_vec());
        let Message::Request { commit, .. } = relay(&new.request("IRCord GPUI on Linux")) else {
            panic!()
        };
        let (mut approver, accept) = Approver::start(&commit);
        let Message::Accept { eph, .. } = relay(&accept) else {
            panic!()
        };
        let Message::Reveal { eph, req, .. } = relay(&new.accept(&eph).unwrap()) else {
            panic!()
        };
        assert_eq!(approver.reveal(&eph, &req).unwrap(), b"certificate request");
        let code = new.code().unwrap();
        assert_eq!(Some(code), approver.code());
        assert_eq!(code.len(), 9);
        let Message::Approve { sealed, .. } = relay(&approver.approve(&transfer()).unwrap()) else {
            panic!()
        };
        assert_eq!(new.open(&sealed).unwrap(), transfer());
        let (device, c) = parse_qr(&new.qr_text("00ff")).unwrap();
        assert_eq!((device.as_str(), c.as_str()), ("00ff", commit.as_str()));
    }

    #[test]
    fn a_relay_that_swaps_keys_is_caught() {
        let mut new = NewDevice::start(b"real request".to_vec());
        let Message::Request { commit, .. } = new.request("x") else {
            panic!()
        };
        // The relay answers the new device with its own key...
        let (mut relay_side, relay_accept) = Approver::start(&commit);
        let Message::Accept { eph: relay_eph, .. } = relay_accept else {
            panic!()
        };
        let Message::Reveal { eph, req, .. } = new.accept(&relay_eph).unwrap() else {
            panic!()
        };
        relay_side.reveal(&eph, &req).unwrap();
        // ...and cannot pass its own request and key to the real approver,
        // whose commitment came from the new device.
        let (mut approver, _) = Approver::start(&commit);
        let forged = NewDevice::start(b"forged request".to_vec());
        let forged_eph = b64url(&forged.public());
        assert!(
            approver
                .reveal(&forged_eph, &b64url(b"forged request"))
                .is_err()
        );
        assert!(approver.reveal(&forged_eph, &req).is_err());
        // A transfer sealed by anyone but the device that derived the code
        // does not open.
        let (mut other, accept) = Approver::start(&commit);
        let Message::Accept { eph: other_eph, .. } = accept else {
            panic!()
        };
        let mut new2 = NewDevice::start(b"real request".to_vec());
        let Message::Reveal { eph, req, .. } = new2.accept(&other_eph).unwrap() else {
            panic!()
        };
        let _ = other.reveal(&eph, &req);
        let Message::Approve { sealed, .. } = relay_side.approve(&transfer()).unwrap() else {
            panic!()
        };
        assert!(new2.open(&sealed).is_err());
        assert_ne!(new2.code(), relay_side.code());
    }

    #[test]
    fn other_versions_and_bad_qr_codes_are_refused() {
        assert!(Message::from_bytes(br#"{"t":"decline","v":2}"#).is_err());
        assert!(Message::from_bytes(br#"{"t":"decline","v":1}"#).is_ok());
        assert!(parse_qr("ircord-e2ee-approve:1:dev:short").is_none());
        assert!(parse_qr("something else").is_none());
    }
}

//! Approving a new device of the same account (ircord/issues#19, section
//! 4.2). The core's wire specification leaves this handshake out of its
//! version 1; this is the web client's (`shared/e2ee/codec.ts` in
//! ircord/frontend), written down in `docs/e2ee.md`, so a GPUI device and a
//! browser can approve each other.
//!
//! Everything passes through the backend's approval relay, so the server
//! sees all of it except the approval code: 20 random digits the new device
//! shows, as text and in a QR code, and the person carries to the approving
//! device by scanning or typing.
//!
//! 1. The new device makes a certificate request `R` and posts
//!    `R ‖ HMAC(code, R)` to the account's [`MAILBOX`].
//! 2. The approving device takes the code from the person and certifies only
//!    a request whose HMAC checks out under it. It sends back the
//!    certificate, its verified identities and its encrypted conversations
//!    ([`Handover`]), sealed with AES-256-GCM under a key from the code.
//! 3. The new device installs only a certificate that arrives sealed under
//!    its code, so it never takes an identity key from the server.

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

/// The relay mailbox new devices post their requests to: every approved
/// device of the account reads it.
pub const MAILBOX: &str = "pending";
/// How many digits an approval code has.
pub const DIGITS: usize = 20;
/// What the QR code's text starts with; the code follows.
pub const QR_PREFIX: &str = "ircord-e2ee-approve:2:";
const INFO_REQUEST: &[u8] = b"ircord-e2ee/1 approval request";
const INFO_HANDOVER: &[u8] = b"ircord-e2ee/1 approval handover";
const MAC_LEN: usize = 32;
const IV_LEN: usize = 12;

/// A new approval code: 20 uniformly random decimal digits.
pub fn new_code() -> String {
    let mut code = String::with_capacity(DIGITS);
    while code.len() < DIGITS {
        let byte: u8 = rand::random();
        // 250 is the largest multiple of 10 a byte holds: no digit is likelier.
        if byte < 250 {
            code.push(char::from(b'0' + byte % 10));
        }
    }
    code
}

/// The code in groups of five digits, as people read and type it.
pub fn format_code(code: &str) -> String {
    code.as_bytes()
        .chunks(5)
        .map(|c| String::from_utf8_lossy(c).into_owned())
        .collect::<Vec<_>>()
        .join(" ")
}

/// The 20 digits of a typed code (spaces and dashes allowed), or of a
/// scanned QR code's text; `None` when it is neither.
pub fn normalize(input: &str) -> Option<String> {
    let input = input.trim();
    let input = input.strip_prefix(QR_PREFIX).unwrap_or(input);
    let digits: String = input
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .collect();
    (digits.len() == DIGITS && digits.bytes().all(|b| b.is_ascii_digit())).then_some(digits)
}

/// The QR code's text for a code.
pub fn qr_text(code: &str) -> String {
    format!("{QR_PREFIX}{code}")
}

/// A 32-byte key from the code: HKDF-SHA256 with a salt of 32 zero bytes,
/// as the browser's `deriveKey` is given it.
fn key(code: &str, info: &[u8]) -> [u8; 32] {
    let mut out = [0u8; 32];
    Hkdf::<Sha256>::new(Some(&[0u8; 32]), code.as_bytes())
        .expand(info, &mut out)
        .expect("32 bytes is a valid length");
    out
}

fn mac(code: &str) -> Hmac<Sha256> {
    <Hmac<Sha256> as Mac>::new_from_slice(&key(code, INFO_REQUEST)).expect("any key length")
}

/// What the new device posts: the certificate request, then its HMAC.
pub fn seal_request(code: &str, request: &[u8]) -> Vec<u8> {
    let mut m = mac(code);
    m.update(request);
    let mut out = request.to_vec();
    out.extend_from_slice(&m.finalize().into_bytes());
    out
}

/// The certificate request in a posted message, if it was made with `code`.
pub fn open_request(code: &str, message: &[u8]) -> Option<Vec<u8>> {
    if message.len() <= MAC_LEN {
        return None;
    }
    let (request, tag) = message.split_at(message.len() - MAC_LEN);
    let mut m = mac(code);
    m.update(request);
    m.verify_slice(tag).ok()?;
    Some(request.to_vec())
}

/// One verified identity the approver passes on.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Verified {
    pub account: String,
    /// The identity key, standard base64.
    pub identity_key: String,
}

/// One encrypted conversation, as the web client stores it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationRecord {
    pub conversation_id: String,
    /// The MLS group ID, 32 hex digits; empty when the conversation has no
    /// group now.
    pub group_id: String,
    pub peer: String,
    pub on: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<String>,
    /// When encryption was turned on or off, in milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<i64>,
}

/// What an approved device receives.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Handover {
    /// The device certificate, standard base64.
    pub certificate: String,
    /// Identities the approver verified.
    #[serde(default)]
    pub verified: Vec<Verified>,
    /// The account's encrypted conversations, so a server cannot make one
    /// look unencrypted to the new device.
    #[serde(default)]
    pub conversations: Vec<ConversationRecord>,
}

/// Seal the approver's answer under the code: a random 12-byte IV, then the
/// AES-256-GCM ciphertext of the JSON.
pub fn seal_handover(code: &str, handover: &Handover) -> Vec<u8> {
    let plain = serde_json::to_vec(handover).expect("a handover serialises");
    let iv: [u8; IV_LEN] = rand::random();
    let sealed = Aes256Gcm::new(&key(code, INFO_HANDOVER).into())
        .encrypt(Nonce::from_slice(&iv), plain.as_slice())
        .expect("sealing does not fail");
    [iv.as_slice(), &sealed].concat()
}

/// Open an answer sealed under the code; an error for anything else.
pub fn open_handover(code: &str, data: &[u8]) -> Result<Handover, String> {
    if data.len() < IV_LEN + 16 {
        return Err("the approval is too short".into());
    }
    let (iv, sealed) = data.split_at(IV_LEN);
    let plain = Aes256Gcm::new(&key(code, INFO_HANDOVER).into())
        .decrypt(Nonce::from_slice(iv), sealed)
        .map_err(|_| "the approval was not sealed under this device's code".to_string())?;
    serde_json::from_slice(&plain).map_err(|e| format!("approval: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine as _;
    use base64::engine::general_purpose::STANDARD;

    const CODE: &str = "01234567890123456789";

    fn handover() -> Handover {
        Handover {
            certificate: "Y2VydA==".into(),
            verified: vec![Verified {
                account: "bob".into(),
                identity_key: "a2V5".into(),
            }],
            conversations: vec![ConversationRecord {
                conversation_id: "dm-bob".into(),
                group_id: "00112233445566778899aabbccddeeff".into(),
                peer: "bob".into(),
                on: true,
                by: Some("alice".into()),
                at: Some(1_795_841_882_000),
            }],
        }
    }

    #[test]
    fn codes_are_twenty_digits_and_read_back_typed_or_scanned() {
        let code = new_code();
        assert_eq!(code.len(), DIGITS);
        assert!(code.bytes().all(|b| b.is_ascii_digit()));
        assert_eq!(format_code(CODE), "01234 56789 01234 56789");
        assert_eq!(normalize(&format_code(CODE)).as_deref(), Some(CODE));
        assert_eq!(normalize("01234-56789-01234-56789").as_deref(), Some(CODE));
        assert_eq!(normalize(&qr_text(CODE)).as_deref(), Some(CODE));
        assert_eq!(normalize("0123456789"), None);
        assert_eq!(normalize("0123456789012345678x"), None);
        assert_eq!(normalize("ircord-e2ee-approve:1:dev:abcd"), None);
    }

    #[test]
    fn a_request_opens_only_under_its_code() {
        let sealed = seal_request(CODE, b"certificate request");
        assert_eq!(
            open_request(CODE, &sealed).as_deref(),
            Some(b"certificate request".as_slice())
        );
        assert_eq!(open_request("99999999999999999999", &sealed), None);
        let mut changed = sealed.clone();
        changed[0] ^= 1;
        assert_eq!(open_request(CODE, &changed), None);
        assert_eq!(open_request(CODE, &sealed[..MAC_LEN]), None);
    }

    #[test]
    fn a_handover_opens_only_under_its_code() {
        let sealed = seal_handover(CODE, &handover());
        assert_eq!(open_handover(CODE, &sealed).unwrap(), handover());
        assert!(open_handover("99999999999999999999", &sealed).is_err());
        assert!(open_handover(CODE, &sealed[..10]).is_err());
    }

    /// Made by the web client's own `sealApprovalRequest` and `sealHandover`
    /// (ircord/frontend `shared/e2ee/codec.ts`, run in Bun) with the code
    /// above, so the two clients cannot drift apart unnoticed.
    #[test]
    fn the_web_clients_messages_open() {
        let request = STANDARD
            .decode(WEB_REQUEST)
            .expect("the web's request is base64");
        assert_eq!(
            open_request(CODE, &request).as_deref(),
            Some(b"certificate request".as_slice())
        );
        assert_eq!(
            STANDARD.encode(seal_request(CODE, b"certificate request")),
            WEB_REQUEST
        );
        let sealed = STANDARD
            .decode(WEB_HANDOVER)
            .expect("the web's handover is base64");
        assert_eq!(open_handover(CODE, &sealed).unwrap(), handover());
    }

    const WEB_REQUEST: &str =
        "Y2VydGlmaWNhdGUgcmVxdWVzdOgmH81ObO84uDxRX7mxlzmaW8q1OffjXGRqy7YUgcu8";
    const WEB_HANDOVER: &str = "jelmJc1p5iIdyAOUyBOhxK/6nbFpupNHaY8ojXkDp/4uXcJOrSDRqW+cgilEU9l8s62fRLPh9RkhsJp7Bcm0DXeYtedHUPAJtw0QtMOUqVwRPfNg6WWNKqcAe9XZlXW6eahFv72L/IoUhFe+q3IFOEeIeyyNMdVlD7t0Kh199vXKr8Stq3eXXpT5Ji+p2glqLBa3OgP1VDx2F07JiLWBNUCJnVvYpfhoPjy+qgL+zwoMLpPDqOnhxi0LY1y3ux33YlU19jdJFdP+Gp08EDSj8fH1kXWp9vNEgbLUaxfDq6X6JkgYsBGEYPdufu12ez5aD4FUHOaO2HNgIxYF";
}

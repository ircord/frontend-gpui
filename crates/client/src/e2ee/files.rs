//! Encrypted files and QR codes.
//!
//! A file in an encrypted conversation is encrypted before it is uploaded,
//! with a fresh random key that travels only inside the encrypted message
//! (the core's `Attachment`). The scheme is the core's wire specification,
//! section 6.4, which the web client uses too: AES-256-GCM with a random
//! 12-byte nonce, uploaded as `nonce || ciphertext || tag`, no associated
//! data; the attachment's `sha256` is the lowercase hex SHA-256 of the
//! uploaded bytes, checked before decrypting. The core's `seal_file` and
//! `open_file` implement it.
//!
//! Files this client sent in 0.9.0 and earlier used ChaCha20-Poly1305 with a zero nonce and the associated data
//! `ircord-e2ee/1 file`. They still open here; nothing writes them any more.

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Nonce};
use sha2::{Digest, Sha256};

use super::{core, from_b64url, hex};

/// The associated data of the files sent by 0.9.0 and earlier.
const LEGACY_AAD: &[u8] = b"ircord-e2ee/1 file";

/// What an upload of an encrypted file goes up as.
pub const UPLOAD_TYPE: &str = "application/octet-stream";

/// A file sealed for upload.
pub struct Sealed {
    pub bytes: Vec<u8>,
    /// base64url, for the attachment.
    pub key: String,
    /// Hex SHA-256 of `bytes`.
    pub sha256: String,
}

pub fn seal(plain: &[u8]) -> Result<Sealed, String> {
    let sealed = core::seal_file(plain).map_err(|e| e.to_string())?;
    Ok(Sealed {
        bytes: sealed.bytes,
        key: sealed.key,
        sha256: sealed.sha256,
    })
}

/// Check and decrypt a downloaded file.
pub fn open(sealed: &[u8], key: &str, sha256: &str) -> Result<Vec<u8>, String> {
    if hex(&Sha256::digest(sealed)) != sha256 {
        return Err("the file is not the one that was sent".into());
    }
    // Strict base64url without padding: it also refuses non-zero trailing
    // bits, so a key has exactly one text form.
    let raw: [u8; 32] = from_b64url(key)?
        .try_into()
        .map_err(|_| "the file key has the wrong length".to_string())?;
    match core::open_file(sealed, key, sha256) {
        Ok(plain) => Ok(plain),
        Err(_) => open_legacy(sealed, &raw)
            .ok_or_else(|| "the file does not decrypt with its key".to_string()),
    }
}

/// A file sent by this client in 0.9.0 or earlier (see the module comment).
fn open_legacy(sealed: &[u8], key: &[u8; 32]) -> Option<Vec<u8>> {
    ChaCha20Poly1305::new(key.into())
        .decrypt(
            Nonce::from_slice(&[0; 12]),
            Payload {
                msg: sealed,
                aad: LEGACY_AAD,
            },
        )
        .ok()
}

/// A QR code as rows of dark (`true`) and light modules, without the quiet
/// zone around it, which the view draws.
pub fn qr(text: &str) -> Option<Vec<Vec<bool>>> {
    let code = qrcode::QrCode::with_error_correction_level(text, qrcode::EcLevel::M).ok()?;
    let width = code.width();
    let modules: Vec<bool> = code
        .to_colors()
        .into_iter()
        .map(|c| c == qrcode::Color::Dark)
        .collect();
    Some(modules.chunks(width).map(<[bool]>::to_vec).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unhex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    /// `test-vectors/files.json` of ircord/e2ee at 1c01f98 (the pinned core).
    const VECTORS: &str = include_str!("testdata/files.json");

    #[test]
    fn the_specification_s_vectors_open_and_its_refusals_are_refused() {
        let v: serde_json::Value = serde_json::from_str(VECTORS).unwrap();
        let s = |c: &serde_json::Value, k: &str| c[k].as_str().unwrap().to_string();
        for case in v["valid"].as_array().unwrap() {
            let plain = open(
                &unhex(&s(case, "file")),
                &s(case, "key"),
                &s(case, "sha256"),
            );
            assert_eq!(plain.unwrap(), unhex(&s(case, "plaintext")));
        }
        for case in v["invalid"].as_array().unwrap() {
            let opened = open(
                &unhex(&s(case, "file")),
                &s(case, "key"),
                &s(case, "sha256"),
            );
            assert!(opened.is_err(), "accepted: {}", s(case, "why"));
        }
    }

    #[test]
    fn a_file_round_trips_and_tampering_is_caught() {
        let sealed = seal(b"a picture of a cat").unwrap();
        assert_eq!(sealed.bytes.len(), 12 + 18 + 16);
        assert_eq!(sealed.key.len(), 43);
        assert_eq!(
            open(&sealed.bytes, &sealed.key, &sealed.sha256).unwrap(),
            b"a picture of a cat"
        );
        let mut bent = sealed.bytes.clone();
        bent[0] ^= 1;
        assert!(open(&bent, &sealed.key, &sealed.sha256).is_err());
        let fixed = hex(&Sha256::digest(&bent));
        assert!(open(&bent, &sealed.key, &fixed).is_err());
        let other = seal(b"x").unwrap();
        assert!(open(&sealed.bytes, &other.key, &sealed.sha256).is_err());
        let upper = sealed.sha256.to_ascii_uppercase();
        assert!(open(&sealed.bytes, &sealed.key, &upper).is_err());
    }

    #[test]
    fn a_file_sent_before_the_specification_still_opens() {
        let key = [7u8; 32];
        let bytes = ChaCha20Poly1305::new((&key).into())
            .encrypt(
                Nonce::from_slice(&[0; 12]),
                Payload {
                    msg: b"an old picture",
                    aad: LEGACY_AAD,
                },
            )
            .unwrap();
        let sha256 = hex(&Sha256::digest(&bytes));
        let key = super::super::b64url(&key);
        assert_eq!(open(&bytes, &key, &sha256).unwrap(), b"an old picture");
        // Only with its own key.
        let wrong = super::super::b64url(&[8u8; 32]);
        assert!(open(&bytes, &wrong, &sha256).is_err());
    }

    #[test]
    fn a_qr_code_is_square() {
        let rows = qr("ircord-e2ee-approve:1:00ff:abcd").unwrap();
        assert!(rows.len() >= 21);
        assert!(rows.iter().all(|r| r.len() == rows.len()));
        // The finder pattern's corner is dark.
        assert!(rows[0][0]);
    }
}

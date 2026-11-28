//! Encrypted files and QR codes.
//!
//! A file in an encrypted conversation is encrypted before it is uploaded,
//! with a fresh random key that travels only inside the encrypted message
//! (the core's `Attachment`). The scheme, which the core's wire
//! specification leaves to clients in its version 1, is
//! ChaCha20-Poly1305 with that key, a zero nonce (each key seals one file)
//! and the associated data `ircord-e2ee/1 file`; the attachment's `sha256` is
//! the SHA-256 of the encrypted bytes, checked before decrypting. It is
//! written down in `docs/e2ee.md` for the web client.

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Nonce};
use sha2::{Digest, Sha256};

use super::{b64url, from_b64url, hex};

const AAD: &[u8] = b"ircord-e2ee/1 file";

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

pub fn seal(plain: &[u8]) -> Sealed {
    let key: [u8; 32] = rand::random();
    let bytes = ChaCha20Poly1305::new((&key).into())
        .encrypt(
            Nonce::from_slice(&[0; 12]),
            Payload {
                msg: plain,
                aad: AAD,
            },
        )
        .expect("sealing in memory does not fail");
    let sha256 = hex(&Sha256::digest(&bytes));
    Sealed {
        bytes,
        key: b64url(&key),
        sha256,
    }
}

/// Check and decrypt a downloaded file.
pub fn open(sealed: &[u8], key: &str, sha256: &str) -> Result<Vec<u8>, String> {
    if hex(&Sha256::digest(sealed)) != sha256.to_ascii_lowercase() {
        return Err("the file is not the one that was sent".into());
    }
    let key: [u8; 32] = from_b64url(key)?
        .try_into()
        .map_err(|_| "the file key has the wrong length".to_string())?;
    ChaCha20Poly1305::new((&key).into())
        .decrypt(
            Nonce::from_slice(&[0; 12]),
            Payload {
                msg: sealed,
                aad: AAD,
            },
        )
        .map_err(|_| "the file does not decrypt with its key".to_string())
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

    #[test]
    fn a_file_round_trips_and_tampering_is_caught() {
        let sealed = seal(b"a picture of a cat");
        assert_ne!(sealed.bytes, b"a picture of a cat");
        assert_eq!(
            open(&sealed.bytes, &sealed.key, &sealed.sha256).unwrap(),
            b"a picture of a cat"
        );
        let mut bent = sealed.bytes.clone();
        bent[0] ^= 1;
        assert!(open(&bent, &sealed.key, &sealed.sha256).is_err());
        let fixed = hex(&Sha256::digest(&bent));
        assert!(open(&bent, &sealed.key, &fixed).is_err());
        assert!(open(&sealed.bytes, &seal(b"x").key, &sealed.sha256).is_err());
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

//! End-to-end encrypted direct messages (experimental, ircord/issues#19).
//!
//! The design was agreed on that issue: MLS through the shared core
//! (`ircord/e2ee`, here through its native crate), keys only on devices, no
//! recovery key, no server backup and no history archive. A new device is
//! approved by one already set up, or the account starts fresh.
//!
//! - [`core`]: the shared core, pinned by commit in `Cargo.toml`.
//! - [`vault`]: this device's sealed store: the core's state, every
//!   encrypted message it read or sent, and a word index for search.
//! - [`approval`]: the device approval handshake (an approval code, typed or
//!   scanned), the web client's.
//! - [`files`]: files encrypted before upload, and QR codes.
//! - [`engine`]: what the app drives: set-up, starting and turning off,
//!   sending, decrypting live and loaded history, approval, verification.
//! - The workspace's view of it is `crate::store::e2ee`; the HTTP calls are
//!   in `crate::api::e2ee`.
//!
//! Everything is behind the server operator's switch: without
//! `capabilities.encryption` in the bootstrap the engine does nothing.

pub mod approval;
pub mod engine;
pub mod files;
pub mod vault;

#[cfg(test)]
mod tests;

pub use engine::{Engine, Texts};
pub use ircord_e2ee_native as core;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;

pub(crate) fn b64url(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

pub(crate) fn from_b64url(text: &str) -> Result<Vec<u8>, String> {
    URL_SAFE_NO_PAD
        .decode(text)
        .map_err(|e| format!("base64url: {e}"))
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// A fingerprint in groups of four hex digits, as people read one out.
pub fn grouped(hex: &str) -> String {
    hex.as_bytes()
        .chunks(4)
        .map(|c| String::from_utf8_lossy(c).into_owned())
        .collect::<Vec<_>>()
        .join(" ")
}

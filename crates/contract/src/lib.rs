//! IRCord's contract with the backend.
//!
//! [`generated`] holds the types `cargo xtask contract` writes from the pinned
//! `openapi.json`; [`event`] holds the socket envelope, written by hand so an
//! event type this client does not know decodes as [`event::ServerEvent::Unknown`]
//! instead of failing.

pub mod event;
pub mod generated;

pub use generated::*;

/// The backend commit `openapi.json` was taken from.
pub const BACKEND_COMMIT: &str = include_str!("../backend-commit");

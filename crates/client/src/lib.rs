//! The IRCord client core, with no GPUI in it, so everything except drawing
//! is tested without a window.
//!
//! - [`address`]: server address rules.
//! - [`config`]: the files kept between launches.
//! - [`credentials`]: the sign-in, in the operating system's credential store.
//! - [`api`]: HTTP commands.
//! - [`events`] and [`socket`]: the event socket with resume, replay and resync.
//! - [`store`]: the workspace and its pure `apply(event)`.
//! - [`format`]: IRC formatting codes and links.
//! - [`client`]: the handle the app holds.

pub mod address;
pub mod api;
pub mod client;
pub mod config;
pub mod credentials;
pub mod events;
pub mod format;
pub mod socket;
pub mod store;

pub use address::ServerAddress;
pub use client::Client;
pub use ircord_contract as contract;

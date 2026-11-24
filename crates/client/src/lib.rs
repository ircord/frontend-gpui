//! The IRCord client core, with no GPUI in it, so everything except drawing
//! is tested without a window.
//!
//! - [`address`]: server address rules.
//! - [`commands`]: slash commands, their arguments and autocomplete.
//! - [`config`]: the files kept between launches.
//! - [`copy_log`]: what copying a stretch of the timeline writes.
//! - [`credentials`]: the sign-in, in the operating system's credential store.
//! - [`api`]: HTTP commands, one file per area of the contract.
//! - [`events`] and [`socket`]: the event socket with resume, replay and resync.
//! - [`store`]: the workspace and its pure `apply(event)`, with one sub-state
//!   file per feature.
//! - [`format`]: IRC formatting codes and links.
//! - [`client`]: the handle the app holds, with the generic `call`, and
//!   [`documents`]: the server's revisioned documents.
//! - [`features`] and [`irc`]: what a network allows, channel ranks.
//! - [`channel_modes`]: channel modes, list masks and the mask builder.
//! - [`search_filter`]: message search filters, recent searches and where
//!   results come from.
//! - [`testing`] (feature `test-support`): a canned server and fixtures.

pub mod address;
pub mod api;
pub mod channel_modes;
pub mod client;
pub mod commands;
pub mod config;
pub mod copy_log;
pub mod credentials;
pub mod documents;
pub mod emoticons;
pub mod events;
pub mod features;
pub mod format;
pub mod irc;
pub mod search_filter;
pub mod socket;
pub mod store;
#[cfg(any(test, feature = "test-support"))]
pub mod testing;

pub use address::ServerAddress;
pub use client::Client;
pub use ircord_contract as contract;

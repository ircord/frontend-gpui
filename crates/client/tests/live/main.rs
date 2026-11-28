//! The client against a real IRCord backend and a real IRC server.
//!
//! `cargo xtask integration` starts both in disposable containers and runs
//! this test binary with `IRCORD_IT_SERVER` (the backend) and `IRCORD_IT_IRC`
//! (the IRC server as the backend sees it, `host:port`). Without them every
//! case is skipped.
//!
//! Each package with server calls adds its cases to its own module below,
//! using the set-up in [`support`]. The module list is written once.

mod support;

mod chat;

mod account; // S0: password, devices
mod admin; // AD1: administration as an owner
mod appearance; // S1: save, use and remove a personal theme
mod channel_settings; // CH2: topic, modes, bans
mod channels; // CH1: create and join
mod commands; // C1: /join, /topic, /nick
mod e2ee; // end-to-end encrypted direct messages (experimental)
mod emotes; // S5: upload, rename, remove an emoji
mod inbox_saved; // FD2: save a message and find it
mod link_previews; // L1: resolve a link in a message
mod links; // FD3: the links view and the channel gallery
mod messages; // A1: edit and react
mod networks; // NW1: add, edit, remove a network
mod notifications; // S3: change a level
mod onboarding; // S4: sign up and onboarding
mod parties; // PT1: a group between two accounts
mod people; // P1: whois and status
mod search; // FD1: find a sent message
mod server_window; // SV1: server log and latency
mod sidebar; // N1: favourite, order, connect, leave, remove
mod timeline; // R1: around a message, paging newer, unread
mod uploads; // U1: upload a file into a message

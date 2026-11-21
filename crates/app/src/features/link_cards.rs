//! Link previews and the open-in-browser card for players (package L1).
//!
//! The previews draw in each message row (`message/links.rs`) from what
//! the server resolved (`ircord_client::store::link_media`); nothing needs
//! registering.

use gpui_kit::App;

pub fn init(_cx: &mut App) {}

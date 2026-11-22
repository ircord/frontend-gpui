//! One module per feature package. Each has an `init` that registers what
//! the feature shows (menu items, slots, panes, keys) through
//! `crate::registry`; nothing else calls into a feature. This list is
//! written once, so a package only fills in its own module.

use gpui_kit::App;

pub mod admin;
pub mod audio_card;
pub mod channel_settings;
pub mod commands;
pub mod connection;
pub mod create_channel;
pub mod desktop;
pub mod drafts;
pub mod gallery;
pub mod inbox;
pub mod invitations;
pub mod join;
pub mod klipy;
pub mod link_cards;
pub mod links;
pub mod message_actions;
pub mod mode_lists;
pub mod moderation;
pub mod network_form;
pub mod onboarding;
pub mod parties;
pub mod profile;
pub mod registration;
pub mod saved;
pub mod search;
pub mod server_window;
pub mod shortcuts;
pub mod status;
pub mod switcher;
pub mod text;
pub mod updates;
pub mod video;
pub mod viewer;

pub fn init(cx: &mut App) {
    // Timeline reading (R1) lives with the timeline it extends.
    crate::timeline::init(cx);
    admin::init(cx);
    text::init(cx);
    message_actions::init(cx);
    switcher::init(cx);
    shortcuts::init(cx);
    commands::init(cx);
    viewer::init(cx);
    network_form::init(cx);
    server_window::init(cx);
    connection::init(cx);
    profile::init(cx);
    status::init(cx);
    search::init(cx);
    inbox::init(cx);
    saved::init(cx);
    join::init(cx);
    create_channel::init(cx);
    link_cards::init(cx);
    klipy::init(cx);
    drafts::init(cx);
    channel_settings::init(cx);
    mode_lists::init(cx);
    registration::init(cx);
    moderation::init(cx);
    parties::init(cx);
    invitations::init(cx);
    onboarding::init(cx);
    desktop::init(cx);
    links::init(cx);
    gallery::init(cx);
    audio_card::init(cx);
    video::init(cx);
    updates::init(cx);
}

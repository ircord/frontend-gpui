//! Headless UI tests: real views in GPUI test windows, driven by simulated
//! clicks and keys. `fixture` opens a window over a session; each module
//! below holds one area's tests, so packages add tests without editing the
//! same file.

mod fixture;
mod registries;
mod shell;

mod admin;
mod appearance;
mod attract;
mod audio_card;
mod audit;
mod channel_settings;
mod channels;
mod commands;
mod desktop;
mod diagrams;
mod drafts;
mod emoji;
mod emotes;
mod emoticons;
mod inbox_saved;
mod link_previews;
mod links_view;
mod media;
mod menu_order;
mod message_actions;
mod networks;
mod notifications;
mod notify_picture;
mod parties;
mod people;
mod profile;
mod search;
mod sending;
mod server_window;
mod settings;
mod settings_messages;
mod sidebar;
mod text;
mod text_blocks;
mod timeline;
mod updates;
mod uploads;
mod video;

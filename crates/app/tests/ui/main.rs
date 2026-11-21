//! Headless UI tests: real views in GPUI test windows, driven by simulated
//! clicks and keys. `fixture` opens a window over a session; each module
//! below holds one area's tests, so packages add tests without editing the
//! same file.

mod fixture;
mod registries;
mod shell;

mod appearance;
mod channels;
mod commands;
mod drafts;
mod inbox_saved;
mod link_previews;
mod message_actions;
mod networks;
mod notifications;
mod people;
mod search;
mod server_window;
mod settings;
mod sidebar;
mod text;
mod timeline;
mod uploads;

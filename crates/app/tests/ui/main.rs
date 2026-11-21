//! Headless UI tests: real views in GPUI test windows, driven by simulated
//! clicks and keys. `fixture` opens a window over a session; each module
//! below holds one area's tests, so packages add tests without editing the
//! same file.

mod fixture;
mod registries;
mod shell;

mod commands;
mod message_actions;
mod networks;
mod people;
mod server_window;
mod settings;
mod sidebar;
mod text;
mod timeline;
mod uploads;

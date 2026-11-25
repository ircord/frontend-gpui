//! Thin wrappers over `gpui-kit`, so every view draws dialogs, menus, toasts
//! and the rest the same way and in the active theme. Each wrapper gives its
//! control an ID, a role and a label, so headless tests can find it and a
//! screen reader can read it.

pub mod avatar;
pub mod confirm;
pub mod context_menu;
pub mod dialog;
pub mod empty;
pub mod field;
pub mod focus;
pub mod frame_motion;
pub mod icon;
pub mod page;
pub mod people_picker;
pub mod prompt;
pub mod toast;
pub mod tooltip;

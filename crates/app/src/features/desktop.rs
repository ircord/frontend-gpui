//! Desktop integration: irc:// links, a single running copy, the tray mark (package DK1).

use gpui_kit::*;

use crate::session::Session;

pub fn init(_cx: &mut App) {}

/// Where to open the main window, from where it was last; `None` for the
/// default (centred).
pub fn placement(_session: &Session, _cx: &App) -> Option<WindowBounds> {
    None
}

/// Links the system hands the running app (macOS opens `irc://` links this way).
pub fn open_urls(_urls: Vec<String>) {}

/// The Dock icon was clicked while the app runs (macOS).
pub fn reopen(_cx: &mut App) {}

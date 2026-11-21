//! Search across conversations (package FD1).

use gpui_kit::*;

use crate::route::Route;
use crate::session::Session;

pub fn init(_cx: &mut App) {}

/// Open the search page, with `query` typed in when given (the switcher's
/// "search messages for …").
pub fn open(session: &Entity<Session>, query: Option<&str>, cx: &mut App) {
    let _ = query;
    session.update(cx, |s, cx| s.navigate(Route::Search, cx));
}

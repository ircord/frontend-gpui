//! The sender's picture on a notification, as a file the system's toast can
//! show. A later package fills [`resolve`] in; until then there is none.

use std::path::PathBuf;

use gpui_kit::*;
use ircord_client::contract::Alert;

use crate::session::Session;

/// A fixed answer, for tests of the code that calls `resolve`.
#[derive(Default)]
pub struct Fixed(pub Option<PathBuf>);
impl Global for Fixed {}

/// The sender's picture for `alert` as a PNG file, or `None`. M2 fills
/// this in and keeps the `Fixed` check first.
pub fn resolve(_session: &Session, _alert: &Alert, cx: &mut App) -> Task<Option<PathBuf>> {
    if let Some(fixed) = cx.try_global::<Fixed>() {
        return Task::ready(fixed.0.clone());
    }
    Task::ready(None)
}

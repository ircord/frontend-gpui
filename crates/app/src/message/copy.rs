//! What copying a selection of the timeline writes.
//!
//! Each piece of selectable text (a message's paragraph, an event's words)
//! is tagged with the log lines it stands for ([`Tag`]). While it paints, a
//! piece that has some of the selection records it here; when the window
//! copies, each piece answers through [`answer`] with what
//! `ircord_client::copy_log::contribution` says it adds, so a selection that
//! runs over several messages pastes as the web client's log (`--- day ---`,
//! `[19:10] <nick> text`) and a selection inside one copies as selected.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gpui_kit::base::TextSelectionHandle;
use gpui_kit::{App, WindowId};
use ircord_client::copy_log::{self, Entry, Pick};

/// The log lines a piece of text stands for.
#[derive(Clone)]
pub struct Tag {
    /// Tells the piece from every other on screen.
    pub key: String,
    pub entries: Rc<[Entry]>,
}

struct Live {
    handle: TextSelectionHandle,
    entries: Rc<[Entry]>,
    selected: String,
}

thread_local! {
    static LIVE: RefCell<HashMap<(WindowId, String), Live>> = RefCell::new(HashMap::new());
}

/// Note what a piece of text has selected as it paints it.
pub fn record(window: WindowId, tag: &Tag, handle: &TextSelectionHandle, selected: String) {
    LIVE.with(|live| {
        let mut live = live.borrow_mut();
        if selected.is_empty() {
            live.remove(&(window, tag.key.clone()));
        } else {
            live.insert(
                (window, tag.key.clone()),
                Live {
                    handle: handle.clone(),
                    entries: tag.entries.clone(),
                    selected,
                },
            );
        }
    });
}

/// What the piece `key` adds to the clipboard.
pub fn answer(window: WindowId, key: &str, cx: &App) -> String {
    let picks: Vec<Pick> = LIVE.with(|live| {
        let mut live = live.borrow_mut();
        // A piece that scrolled away has lost its selection.
        live.retain(|_, l| l.handle.snapshot(cx).is_some());
        live.iter()
            .filter(|((w, _), _)| *w == window)
            .map(|((_, key), l)| Pick {
                key: key.clone(),
                entries: l.entries.to_vec(),
                selected: l.selected.clone(),
            })
            .collect()
    });
    copy_log::contribution(&picks, key)
}

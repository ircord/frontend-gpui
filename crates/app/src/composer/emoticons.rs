//! Typed emoticons become emoji in the box (package P4), as the web's
//! composer does: whitespace typed right after `:)` turns it into 🙂,
//! Backspace straight after puts the typed text back, and sending finishes a
//! word that ends in an emoticon. The rules are `ircord_client::emoticons`.

use std::rc::Rc;

use gpui_kit::{Context, Window};
use ircord_client::emoticons;
use ircord_client::store::emoji;

use super::ComposerView;
use crate::features::klipy;
use crate::registry::{ComposerCtx, SendFilter, SendOutcome};

/// What a conversion replaced, so an immediate Backspace can undo it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Converted {
    /// Where the emoticon starts in the text (a byte offset).
    start: usize,
    /// The emoticon as typed, with the whitespace that followed it.
    typed: String,
    /// The caret after the conversion.
    end: usize,
    /// The text after the conversion.
    value: String,
}

/// Sending finishes the last word: an emoticon at the end of what is sent
/// becomes its emoji. It runs before commands and Markdown, so both see the
/// emoji (a `/me` line converts too).
pub fn send_filter() -> SendFilter {
    SendFilter {
        id: "emoticons".into(),
        order: 50,
        filter: Rc::new(|_: &ComposerCtx, text, _, _| {
            SendOutcome::Continue(emoticons::convert_trailing(&text))
        }),
    }
}

/// Whether `after` is `before` with one whitespace character typed at `at`,
/// the caret now after it. Pastes and multi-character edits do not count.
fn typed_whitespace(before: &str, at: usize, after: &str, caret: usize) -> bool {
    let (Some(added), Some(head), Some(tail)) =
        (after.get(at..caret), before.get(..at), before.get(at..))
    else {
        return false;
    };
    let mut chars = added.chars();
    matches!((chars.next(), chars.next()), (Some(c), None) if c.is_whitespace())
        && after.get(..at) == Some(head)
        && after.get(caret..) == Some(tail)
}

impl ComposerView {
    /// Runs on every change to the box: whitespace typed right after an
    /// emoticon turns it into its emoji, as the web does. Any other change
    /// ends the chance to undo a conversion.
    pub(super) fn convert_typed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let now = (self.text(cx), self.caret(cx));
        let (before, at) = std::mem::replace(&mut self.seen, now.clone());
        let (after, caret) = now;
        if !self
            .emoticon
            .as_ref()
            .is_some_and(|c| c.value == after && c.end == caret)
        {
            self.emoticon = None;
        }
        if !typed_whitespace(&before, at, &after, caret) {
            return;
        }
        let Some(found) = emoticons::finished(&after, caret) else {
            return;
        };
        let added = &after[at..caret];
        let end = found.start + found.emoji.len() + added.len();
        let value = format!(
            "{}{}{}{}",
            &after[..found.start],
            found.emoji,
            added,
            &after[caret..]
        );
        // The emoji is recorded as picked, so it is among the frequent ones.
        if let Some(item) = emoji::standard()
            .iter()
            .find(|item| item.unicode.as_deref() == Some(found.emoji))
        {
            klipy::record_pick(&self.session, &item.id, cx);
        }
        self.set_text_at(&value, end, window, cx);
        self.emoticon = Some(Converted {
            start: found.start,
            typed: format!("{}{added}", found.typed),
            end,
            value,
        });
    }

    /// Backspace straight after a conversion puts the emoticon back as it was
    /// typed, for when the emoji was not wanted. Returns whether it did.
    pub(super) fn restore_typed(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(last) = self.emoticon.take() else {
            return false;
        };
        let text = self.text(cx);
        let selected = self.input.read(cx).selected_range();
        if text != last.value || selected != (last.end..last.end) {
            return false;
        }
        let next = format!("{}{}{}", &text[..last.start], last.typed, &text[last.end..]);
        self.set_text_at(&next, last.start + last.typed.len(), window, cx);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::typed_whitespace;

    #[test]
    fn one_whitespace_typed_at_the_caret_is_an_insert() {
        assert!(typed_whitespace("hi :)", 5, "hi :) ", 6));
        assert!(typed_whitespace("hi :)", 5, "hi :)\n", 6));
        // A space typed in the middle of the text is an insert too.
        assert!(typed_whitespace("hi :)", 2, "hi  :)", 3));
    }

    #[test]
    fn other_edits_are_not_typed_whitespace() {
        // A paste of two characters, or a letter.
        assert!(!typed_whitespace("hi :)", 5, "hi :)  ", 7));
        assert!(!typed_whitespace("hi :)", 5, "hi :)x", 6));
        // A deletion: the text shrank.
        assert!(!typed_whitespace("hi :) ", 6, "hi :)", 5));
    }
}

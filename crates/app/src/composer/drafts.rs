//! Drafts kept per conversation (package D1): the message box keeps what
//! you typed in each conversation, and gives it back when you return, also
//! after a restart.
//!
//! Each composer follows its input: every change to the text is kept as the
//! draft of the conversation the box shows. When the selection moves, the
//! box takes the draft of the conversation now selected. While a message is
//! being edited (A1) the draft is the text the box held before the edit.

use std::cell::RefCell;
use std::rc::Rc;

use gpui_kit::*;
use ircord_client::store::drafts;

use crate::composer::ComposerView;
use crate::features::drafts as page;
use crate::features::message_actions::{self, Mode};
use crate::session::{Session, SessionEvent};

/// The conversation a composer's text belongs to.
pub type Shown = Rc<RefCell<Option<String>>>;

/// Follow `composer`'s text and the session's selection.
pub fn attach(
    composer: &mut ComposerView,
    session: Entity<Session>,
    window: &mut Window,
    cx: &mut Context<ComposerView>,
) {
    let shown: Shown = Rc::new(RefCell::new(session.read(cx).conversation.clone()));
    page::remember_composer(cx.weak_entity(), shown.clone(), cx);
    let (on_input, on_session, shown_now) = (shown.clone(), shown.clone(), shown);
    let (input_session, events_session) = (session.clone(), session.clone());
    // The input redraws for more than its text (the caret, focus): only a
    // change to the text is kept, so an empty box never wipes a draft.
    let last = RefCell::new(composer.text(cx));
    cx.observe(&composer.input, move |this, _, cx| {
        let text = this.text(cx);
        if *last.borrow() == text {
            return;
        }
        last.replace(text.clone());
        let conversation = on_input.borrow().clone();
        if let Some(conversation) = conversation {
            keep(&input_session, &conversation, text, cx);
        }
    })
    .detach();
    cx.subscribe_in(
        &events_session,
        window,
        move |this, session, event: &SessionEvent, window, cx| match event {
            SessionEvent::Selection | SessionEvent::Phase => {
                switch(this, session, &on_session, window, cx)
            }
            SessionEvent::Changed(changes)
                if changes.reset || changes.topics.contains_key(drafts::TOPIC) =>
            {
                page::ensure_loaded(session.read(cx));
                fill(this, session, &on_session, window, cx);
            }
            _ => {}
        },
    )
    .detach();
    // Drawn after the drafts were read in (the workspace appears once
    // signed in).
    page::ensure_loaded(session.read(cx));
    fill(composer, &session, &shown_now, window, cx);
}

/// Keep `text` as the draft of `conversation`, or the text from before an
/// edit while one is going on.
fn keep(session: &Entity<Session>, conversation: &str, text: String, cx: &mut App) {
    let text = match message_actions::mode(conversation, cx) {
        Some(Mode::Edit { draft, .. }) => draft,
        _ => text,
    };
    page::set(session, conversation, &text, cx);
}

/// The selection moved: keep the text for the conversation left and show
/// the draft of the one selected.
fn switch(
    this: &mut ComposerView,
    session: &Entity<Session>,
    shown: &Shown,
    window: &mut Window,
    cx: &mut Context<ComposerView>,
) {
    let now = session.read(cx).conversation.clone();
    let before = shown.borrow().clone();
    if before == now {
        return fill(this, session, shown, window, cx);
    }
    if let Some(before) = &before {
        keep(session, before, this.text(cx), cx);
    }
    *shown.borrow_mut() = now.clone();
    let text = now
        .as_deref()
        .map(|id| page::text(session, id, cx))
        .unwrap_or_default();
    this.set_text_at(&text, text.len(), window, cx);
    // An edit left half-done comes back as the draft it started from.
    if let Some(id) = now
        && matches!(message_actions::mode(&id, cx), Some(Mode::Edit { .. }))
    {
        window.defer(cx, move |window, cx| {
            message_actions::cancel(&id, window, cx)
        });
    }
}

/// The drafts were read in after the box was drawn: an empty box takes the
/// draft of its conversation.
fn fill(
    this: &mut ComposerView,
    session: &Entity<Session>,
    shown: &Shown,
    window: &mut Window,
    cx: &mut Context<ComposerView>,
) {
    let Some(id) = shown.borrow().clone() else {
        return;
    };
    if !this.text(cx).is_empty() {
        return;
    }
    let text = page::text(session, &id, cx);
    if !text.is_empty() {
        this.set_text_at(&text, text.len(), window, cx);
    }
}

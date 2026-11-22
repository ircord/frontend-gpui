//! Offer an interrupted Theme Studio draft when its editor is opened again.

use std::time::Duration;

use gpui_kit::{App, Entity, Focusable, Task, Window};
use ircord_client::contract::Theme;
use ircord_client::store::appearance::{
    self as look, Mode,
    drafts::{Draft, Store},
};

use super::{Editor, fill_theme_fields, refresh_studio};
use crate::theme::appearance;

pub(super) struct Recovery {
    store: Store,
    original: Theme,
    pub(super) offer: Option<Draft>,
    observed: Option<Draft>,
    pending: Option<Change>,
    timer: Option<Task<()>>,
}

enum Change {
    Remember(Box<Draft>),
    Remove(String),
}

impl Recovery {
    pub(super) fn for_theme(
        state: &Editor,
        original: &Theme,
        new_theme: bool,
        cx: &App,
    ) -> Option<Self> {
        let session = state.session.read(cx);
        let client = session.client()?;
        let store = Store::new(
            &session.config,
            client.api().server(),
            &client.read().user.id,
        )?;
        let offer = store.get().filter(|draft| {
            look::theme_supported(&draft.theme)
                && draft.theme.id.as_ref().is_some_and(|id| !id.is_empty())
                && draft.theme != *original
                && if new_theme {
                    draft.new_theme
                        && !appearance::themes(cx)
                            .iter()
                            .any(|theme| theme.id == draft.theme.id)
                } else {
                    !draft.new_theme && draft.theme.id == original.id
                }
        });
        Some(Self {
            store,
            original: original.clone(),
            offer,
            observed: None,
            pending: None,
            timer: None,
        })
    }

    pub(super) fn flush(&mut self) {
        let result = match self.pending.take() {
            Some(Change::Remember(draft)) => self.store.put(*draft),
            Some(Change::Remove(id)) => self.store.remove(&id),
            None => return,
        };
        if let Err(error) = result {
            log::warn!("save Theme Studio draft: {error}");
        }
    }

    pub(super) fn discard(&mut self, id: &str) {
        self.pending = None;
        self.timer = None;
        if let Err(error) = self.store.remove(id) {
            log::warn!("discard Theme Studio draft: {error}");
        }
    }

    pub(super) fn replace_original(&mut self, theme: &Theme) {
        if let Some(id) = &theme.id {
            self.discard(id);
        }
        self.original = theme.clone();
        self.observed = None;
        self.offer = None;
    }
}

pub(super) fn remember(editor: &Entity<Editor>, theme: &Theme, mode: Mode, cx: &mut App) {
    let Some(id) = &theme.id else { return };
    let state = editor.read(cx);
    let Some(recovery) = &state.recovery else {
        return;
    };
    let next = (*theme != recovery.original).then(|| Draft {
        theme: theme.clone(),
        mode,
        new_theme: state.new_theme,
    });
    if next == recovery.observed {
        return;
    }
    let runtime = state.session.read(cx).runtime.clone();
    let hold = runtime.spawn(async { tokio::time::sleep(Duration::from_millis(250)).await });
    let weak = editor.downgrade();
    let timer = cx.spawn(async move |cx| {
        if hold.await.is_ok() {
            let _ = weak.update(cx, |editor, _| {
                if let Some(recovery) = &mut editor.recovery {
                    recovery.flush();
                }
            });
        }
    });
    editor.update(cx, |state, _| {
        if let Some(recovery) = &mut state.recovery {
            recovery.pending = Some(match &next {
                Some(draft) => Change::Remember(Box::new(draft.clone())),
                None => Change::Remove(id.clone()),
            });
            recovery.observed = next;
            recovery.timer = Some(timer);
        }
    });
}

pub(super) fn restore(editor: &Entity<Editor>, window: &mut Window, cx: &mut App) {
    let draft = editor.update(cx, |state, _| state.recovery.as_mut()?.offer.take());
    let Some(draft) = draft else { return };
    fill_theme_fields(editor, &draft.theme, draft.mode, window, cx);
    editor.update(cx, |state, _| {
        state.new_theme = draft.new_theme;
        state.editing = Some((draft.theme, draft.mode));
        state.edit_generation = state.edit_generation.wrapping_add(1);
        state.notice = None;
    });
    refresh_studio(editor, cx);
    editor.read(cx).name.focus_handle(cx).focus(window, cx);
    cx.refresh_windows();
}

pub(super) fn dismiss(editor: &Entity<Editor>, cx: &mut App) {
    editor.update(cx, |state, _| {
        if let Some(recovery) = &mut state.recovery
            && let Some(draft) = recovery.offer.take()
            && let Some(id) = draft.theme.id
            && !recovery
                .observed
                .as_ref()
                .is_some_and(|draft| draft.theme.id.as_deref() == Some(id.as_str()))
        {
            // The person may have started a fresh edit while the offer was showing.
            if let Err(error) = recovery.store.remove(&id) {
                log::warn!("discard Theme Studio draft: {error}");
            }
        }
    });
    cx.refresh_windows();
}

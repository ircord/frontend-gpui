//! The message box under the timeline, with its registered slots: bars
//! above the box, buttons beside it, suggestions while typing, and send
//! filters that see the text before it is sent.

pub mod drafts;
pub mod expressions;
pub mod reply;
pub mod suggest;
pub mod uploads;

use gpui_kit::TestSupportExt;
use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::prelude::*;
use gpui_kit::*;
use ircord_client::contract::ConversationKind;

use crate::registry::{self, ComposerCtx, SendOutcome, Suggestion};
use crate::session::{Session, SessionEvent};
use crate::theme;

pub struct ComposerView {
    session: Entity<Session>,
    pub input: Entity<TextareaState>,
    _subscriptions: Vec<Subscription>,
}

impl ComposerView {
    pub fn new(session: Entity<Session>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| {
            let mut state = TextareaState::new(window, cx)
                .auto_grow(1, 10)
                .placeholder(t!("composer-placeholder"));
            state.set_submit_on_enter(true, cx);
            state
        });
        let subscriptions = vec![
            cx.subscribe_in(
                &input,
                window,
                |this, state, event: &InputEvent, window, cx| match event {
                    InputEvent::PressEnter { shift: false, .. } => {
                        let text = state.read(cx).value().to_string();
                        this.submit(text, window, cx);
                    }
                    InputEvent::Change => {
                        let has_text = !state.read(cx).value().is_empty();
                        let session = this.session.read(cx);
                        if let (Some(client), Some(id)) = (session.client(), &session.conversation)
                        {
                            client.typing(id, has_text);
                        }
                        cx.notify();
                    }
                    _ => {}
                },
            ),
            cx.subscribe(&session, |_, _, event: &SessionEvent, cx| match event {
                SessionEvent::Selection | SessionEvent::Phase => cx.notify(),
                SessionEvent::Changed(changes) if changes.sidebar => cx.notify(),
                _ => {}
            }),
        ];
        Self {
            session,
            input,
            _subscriptions: subscriptions,
        }
    }

    fn ctx(&self, cx: &mut Context<Self>) -> Option<ComposerCtx> {
        let conversation = self.session.read(cx).conversation.clone()?;
        Some(ComposerCtx {
            session: self.session.clone(),
            conversation,
            composer: cx.entity(),
        })
    }

    /// Run the send filters, send what is left, then the submit hooks (which
    /// also run when the box is empty: the upload tray sends its files).
    pub fn submit(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ctx) = self.ctx(cx) else {
            return;
        };
        if text.trim().is_empty() {
            self.after_submit(&ctx, "", window, cx);
            return;
        }
        let filters = registry::get(cx).composer.send_filters.to_vec();
        let mut text = text;
        for filter in filters {
            match (filter.filter)(&ctx, text, window, cx) {
                SendOutcome::Continue(next) => text = next,
                SendOutcome::Handled => {
                    self.set_text("", window, cx);
                    return;
                }
                SendOutcome::Keep => return,
            }
        }
        let sent = if text.trim().is_empty() {
            String::new()
        } else {
            self.session.update(cx, |s, _| s.send(&text));
            text
        };
        self.set_text("", window, cx);
        self.after_submit(&ctx, &sent, window, cx);
    }

    /// Run the submit hooks once this update is over, so they may change the
    /// composer.
    fn after_submit(
        &mut self,
        ctx: &ComposerCtx,
        sent: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let hooks = registry::get(cx).composer.on_submit.to_vec();
        if hooks.is_empty() {
            return;
        }
        let (ctx, sent) = (ctx.clone(), sent.to_string());
        window.defer(cx, move |window, cx| {
            for hook in hooks {
                (hook.run)(&ctx, &sent, window, cx);
            }
        });
    }

    pub fn text(&self, cx: &App) -> String {
        self.input.read(cx).value().to_string()
    }

    pub fn set_text(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.input
            .update(cx, |s, cx| s.set_value(text.to_string(), window, cx));
    }

    /// Put a suggestion in place of the text it replaces.
    pub fn apply(&mut self, suggestion: &Suggestion, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.text(cx);
        let range =
            suggestion.replace.start.min(text.len())..suggestion.replace.end.min(text.len());
        if !text.is_char_boundary(range.start) || !text.is_char_boundary(range.end) {
            return;
        }
        let next = format!(
            "{}{}{}",
            &text[..range.start],
            suggestion.insert,
            &text[range.end..]
        );
        self.set_text(&next, window, cx);
    }

    fn suggestions(&self, ctx: &ComposerCtx, cx: &App) -> Vec<Suggestion> {
        let text = self.text(cx);
        if text.is_empty() {
            return Vec::new();
        }
        for provider in registry::get(cx).composer.suggestions.iter() {
            let found = (provider.suggest)(ctx, &text, cx);
            if !found.is_empty() {
                return found;
            }
        }
        Vec::new()
    }
}

impl Render for ComposerView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme::get(cx).clone();
        let session = self.session.read(cx);
        let target = match (session.client(), &session.conversation) {
            (Some(client), Some(id)) => client.read().conversation(id).map(|c| {
                let prefix = if c.kind == ConversationKind::Channel {
                    "#"
                } else {
                    "@"
                };
                (format!("{prefix}{}", c.name), c.joined)
            }),
            _ => None,
        };
        let root = div().id("composer").test_support().px_4().pb_4();
        let Some((name, true)) = target else {
            return root
                .text_sm()
                .text_color(theme.muted)
                .child(t!("composer-not-joined"));
        };
        let Some(ctx) = self.ctx(cx) else {
            return root;
        };
        let slots = registry::get(cx);
        let (above, buttons) = (
            slots.composer.above.to_vec(),
            slots.composer.buttons.to_vec(),
        );
        let above: Vec<AnyElement> = above
            .iter()
            .filter_map(|s| (s.render)(&ctx, window, cx))
            .collect();
        let buttons: Vec<AnyElement> = buttons
            .iter()
            .filter_map(|s| (s.render)(&ctx, window, cx))
            .collect();
        let suggestions = self.suggestions(&ctx, cx);
        let mut root = root.children(above);
        if !suggestions.is_empty() {
            let mut list = div()
                .id("composer-suggestions")
                .test_support()
                .role(Role::ListBox)
                .aria_label(t!("composer-suggestions"))
                .mb_1()
                .p_1()
                .rounded_md()
                .bg(theme.sidebar)
                .flex()
                .flex_col();
            for suggestion in suggestions {
                let hover = theme.hover;
                let label = suggestion.label.clone();
                let detail = suggestion.detail.clone();
                list = list.child(
                    div()
                        .id(ElementId::Name(
                            format!("suggestion-{}", suggestion.id).into(),
                        ))
                        .test_support()
                        .role(Role::ListBoxOption)
                        .aria_label(label.clone())
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .flex()
                        .gap_2()
                        .cursor_pointer()
                        .hover(move |d| d.bg(hover))
                        .child(div().text_color(theme.text).child(label))
                        .when_some(detail, |d, detail| {
                            d.child(div().text_color(theme.muted).text_sm().child(detail))
                        })
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.apply(&suggestion, window, cx)
                        })),
                );
            }
            root = root.child(list);
        }
        root.child(
            div()
                .rounded_lg()
                .bg(theme.input)
                .px_2()
                .py_1()
                .flex()
                .items_end()
                .gap_1()
                .child(
                    div()
                        .id("composer-input")
                        .test_support()
                        .flex_1()
                        .min_w_0()
                        .child(
                            Textarea::new(&self.input)
                                .appearance(false)
                                .aria_label(t!("composer-label", target = name)),
                        ),
                )
                .children(buttons),
        )
    }
}

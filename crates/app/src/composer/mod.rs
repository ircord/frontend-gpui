//! The message box under the timeline, with its registered slots: bars
//! above the box, buttons beside it, suggestions while typing, and send
//! filters that see the text before it is sent.
//!
//! The box owns its keys: Up and Down move through the suggestions, Tab or
//! Enter takes one and Escape hides them; Tab with no suggestions completes
//! a nick; the formatting keys wrap the selection in Markdown markers; and
//! typing while nothing else takes text brings the focus here.
//!
//! Registered token providers mark stretches of the text (a server emoji's
//! `:name:`) that the box draws as a picture and edits as one unit; the
//! text itself never changes.
//!
//! It looks like the web's: a raised, rounded box whose placeholder names
//! the conversation ("Message #general"), the registered buttons (GIF,
//! emoji, attach), then a send arrow that fills once there is text.

pub mod drafts;
pub mod emoticons;
pub mod expressions;
pub mod reply;
pub mod suggest;
pub mod uploads;

use gpui_kit::TestSupportExt;
use std::ops::Range;

use gpui_kit::component::input::{
    InlineToken, InlineTokenContext, InputContent, InputEvent, Textarea, TextareaState,
};
use gpui_kit::prelude::*;
use gpui_kit::*;
use ircord_client::commands::complete::{self, TabCycle};
use ircord_client::contract::ConversationKind;
use ircord_client::{features::plain_line_breaks, store::drafts::TEXT_LIMIT};

use crate::registry::{self, ComposerCtx, SendOutcome, Suggestion, SuggestionLook, TokenView};
use crate::route::Route;
use crate::session::{Session, SessionEvent};
use crate::theme::{self, Theme};
use crate::ui;
use crate::ui::focus::Reachable;
use crate::ui::icon::LucideIcon;

gpui_kit::actions!(
    composer,
    [
        /// Wrap the selection in `**`, sent as IRC bold.
        FormatBold,
        /// Wrap the selection in `*`, sent as IRC italic.
        FormatItalic,
        /// Wrap the selection in `__`, sent as IRC underline.
        FormatUnderline,
    ]
);

/// The key context the composer's keys apply in.
pub const KEY_CONTEXT: &str = "Composer";

/// Each composer's text and caret as of its last draw or key, for slots and
/// suggestion providers, which run while the composer is being updated and
/// so cannot read it.
#[derive(Default)]
struct Typed(std::collections::HashMap<EntityId, (String, usize)>);

impl Global for Typed {}

/// The text in `composer` and the caret in it (a byte offset).
pub fn typed(composer: &Entity<ComposerView>, cx: &App) -> (String, usize) {
    cx.try_global::<Typed>()
        .and_then(|t| t.0.get(&composer.entity_id()).cloned())
        .unwrap_or_default()
}

/// Bumped by [`retokenize`]; every composer works its tokens out again.
#[derive(Default)]
struct Tokens(u64);

impl Global for Tokens {}

/// What the token providers find has changed without the text changing (an
/// emoji catalog loaded): every composer works its tokens out again, so a
/// draft restored before the catalog turns into pictures.
pub fn retokenize(cx: &mut App) {
    let next = cx.try_global::<Tokens>().map_or(0, |t| t.0) + 1;
    cx.set_global(Tokens(next));
}

pub struct ComposerView {
    session: Entity<Session>,
    pub input: Entity<TextareaState>,
    /// The highlighted suggestion.
    selected: usize,
    /// The highlight was moved by hand, so Enter takes it instead of sending.
    picked: bool,
    /// The text and caret when Escape hid the suggestions; they stay hidden
    /// until either changes.
    dismissed: Option<(String, usize)>,
    tab: Option<TabCycle>,
    /// The last emoticon turned into emoji, which Backspace can undo.
    emoticon: Option<emoticons::Converted>,
    /// The text and caret after the last change, so a typed space can be told from a render.
    seen: (String, usize),
    /// Keeps the highlighted suggestion in view.
    scroll: ScrollHandle,
    /// The placeholder given to the box, which names the conversation.
    placeholder: SharedString,
    _subscriptions: Vec<Subscription>,
}

/// The tallest the suggestion list grows before it scrolls.
const SUGGESTIONS_HEIGHT: f32 = 280.;
const DRAFT_WARNING: isize = 400;

impl ComposerView {
    pub fn new(session: Entity<Session>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| {
            let mut state = TextareaState::new(window, cx).auto_grow(1, 10);
            state.set_submit_on_enter(true, cx);
            state
        });
        let this = cx.entity().downgrade();
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
                        this.convert_typed(window, cx);
                        state.update(cx, tokenize);
                        this.selected = 0;
                        this.picked = false;
                        this.tab = None;
                        this.scroll.scroll_to_item(0);
                        let has_text = !state.read(cx).value().is_empty();
                        let session = this.session.read(cx);
                        if let (Some(client), Some(id)) = (session.client(), &session.conversation)
                        {
                            client.typing(id, has_text);
                        }
                        cx.notify();
                    }
                    InputEvent::Focus | InputEvent::Blur => cx.notify(),
                    InputEvent::PressEnter { .. } => {}
                },
            ),
            cx.observe_global::<Tokens>(|this, cx| this.input.update(cx, tokenize)),
            cx.subscribe(&session, |_, _, event: &SessionEvent, cx| match event {
                SessionEvent::Selection | SessionEvent::Phase => cx.notify(),
                SessionEvent::Changed(changes) if changes.sidebar => cx.notify(),
                _ => {}
            }),
            cx.intercept_keystrokes(move |event, window, cx| {
                if let Some(this) = this.upgrade() {
                    this.update(cx, |this, cx| this.on_key(event, window, cx));
                }
            }),
        ];
        Self {
            session,
            input,
            selected: 0,
            picked: false,
            dismissed: None,
            tab: None,
            emoticon: None,
            seen: (String::new(), 0),
            scroll: ScrollHandle::new(),
            placeholder: SharedString::default(),
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
        // Text pasted on Windows keeps its `\r\n` line breaks. Count and send
        // the normalized text, as the web textarea does.
        let text = plain_line_breaks(&text);
        let blank = text.trim().is_empty();
        if text.encode_utf16().count() > TEXT_LIMIT
            && (!blank || uploads::Uploads::has_pending(cx, &ctx.conversation))
        {
            ui::toast::info(window, cx, t!("composer-too-long"));
            return;
        }
        if blank {
            self.after_submit(&ctx, "", window, cx);
            return;
        }
        self.remember(cx);
        let filters = registry::get(cx).composer.send_filters.to_vec();
        let mut text = text.into_owned();
        for filter in filters {
            match (filter.filter)(&ctx, text, window, cx) {
                SendOutcome::Continue(next) => text = next,
                SendOutcome::Sent(sent) => {
                    self.set_text("", window, cx);
                    self.after_submit(&ctx, &sent, window, cx);
                    return;
                }
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

    /// The caret, as a byte offset into [`Self::text`].
    pub fn caret(&self, cx: &App) -> usize {
        self.input.read(cx).cursor()
    }

    pub fn set_text(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.replace(text, None, window, cx);
    }

    /// Replace the text, select `selected` in it (or leave the caret where
    /// the box puts it), then work out the tokens: `set_value` reports no
    /// change, so the Change handler does not.
    fn replace(
        &mut self,
        text: &str,
        selected: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.input.update(cx, |s, cx| {
            s.set_value(text.to_string(), window, cx);
            if let Some(selected) = selected {
                s.set_selected_range(selected, cx);
            }
            tokenize(s, cx);
        });
        self.seen = (self.text(cx), self.caret(cx));
    }

    /// Replace the text and put the caret at `caret` (a byte offset).
    pub fn set_text_at(
        &mut self,
        text: &str,
        caret: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let caret = caret.min(text.len());
        self.replace(text, Some(caret..caret), window, cx);
        cx.notify();
    }

    /// Put a suggestion in place of the text it replaces, the caret after it.
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
        let caret = range.start + suggestion.insert.len();
        self.set_text_at(&next, caret, window, cx);
        self.input.update(cx, |s, cx| s.focus(window, cx));
        self.selected = 0;
        self.picked = false;
    }

    /// Wrap the selection (or nothing, at the caret) in `marker`, keeping it selected.
    pub fn wrap(&mut self, marker: &str, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.text(cx);
        let range = self.input.read(cx).selected_range();
        let (start, end) = (range.start.min(text.len()), range.end.min(text.len()));
        if !text.is_char_boundary(start) || !text.is_char_boundary(end) {
            return;
        }
        let next = format!(
            "{}{marker}{}{marker}{}",
            &text[..start],
            &text[start..end],
            &text[end..]
        );
        let (from, to) = (start + marker.len(), end + marker.len());
        self.replace(&next, Some(from..to), window, cx);
        cx.notify();
    }

    /// What the registered providers suggest for the text now; the first
    /// provider with suggestions wins.
    pub fn suggestions(&self, ctx: &ComposerCtx, cx: &App) -> Vec<Suggestion> {
        let text = self.text(cx);
        if text.is_empty() {
            return Vec::new();
        }
        if self.dismissed.as_ref() == Some(&(text.clone(), self.caret(cx))) {
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

    /// Keep [`typed`] current before slots and providers run.
    fn remember(&self, cx: &mut Context<Self>) {
        let now = (self.text(cx), self.caret(cx));
        let id = cx.entity_id();
        cx.default_global::<Typed>().0.insert(id, now);
    }

    fn focused(&self, window: &Window, cx: &App) -> bool {
        self.input.read(cx).focus_handle(cx).is_focused(window)
    }

    /// The composer's keys, before anything else sees them.
    fn on_key(&mut self, event: &KeystrokeEvent, window: &mut Window, cx: &mut Context<Self>) {
        let key = &event.keystroke;
        let m = key.modifiers;
        if !self.focused(window, cx) {
            self.type_to_compose(event, window, cx);
            return;
        }
        let Some(ctx) = self.ctx(cx) else {
            return;
        };
        self.remember(cx);
        if key.key == "backspace" && !m.modified() && self.restore_typed(window, cx) {
            cx.stop_propagation();
            return;
        }
        if key.key != "shift" {
            self.emoticon = None;
        }
        let suggestions = self.suggestions(&ctx, cx);
        let plain = !m.modified();
        if !suggestions.is_empty() {
            let count = suggestions.len();
            let selected = self.selected.min(count - 1);
            match key.key.as_str() {
                "up" | "down" if plain => {
                    let step = if key.key == "down" { 1 } else { count - 1 };
                    self.selected = (selected + step) % count;
                    self.picked = true;
                    self.scroll.scroll_to_item(self.selected);
                    cx.stop_propagation();
                    cx.notify();
                }
                "tab" if plain => {
                    self.apply(&suggestions[selected], window, cx);
                    cx.stop_propagation();
                }
                "enter" if plain => {
                    // A suggestion typed out in full: Enter sends it, unless picked.
                    let s = &suggestions[selected];
                    let text = self.text(cx);
                    let typed = text.get(s.replace.clone()).unwrap_or_default();
                    if self.picked || typed != s.insert.trim_end() {
                        self.apply(s, window, cx);
                        cx.stop_propagation();
                    }
                }
                "escape" if plain => {
                    self.dismissed = Some((self.text(cx), self.caret(cx)));
                    cx.stop_propagation();
                    cx.notify();
                }
                _ => {}
            }
            return;
        }
        if key.key == "tab" && plain && self.tab_complete(&ctx, window, cx) {
            cx.stop_propagation();
        }
    }

    /// Tab completes the nick before the caret, adding the `@`; Tab again
    /// cycles through the other matches.
    fn tab_complete(
        &mut self,
        ctx: &ComposerCtx,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let (text, caret) = (self.text(cx), self.caret(cx));
        let session = ctx.session.read(cx);
        let Some(client) = session.client() else {
            return false;
        };
        let w = client.read();
        let Some(conversation) = w.conversation(&ctx.conversation) else {
            return false;
        };
        let unignore = text.to_lowercase().starts_with("/unignore ");
        let ignored = w
            .documents
            .notification_settings
            .value()
            .and_then(|s| s.ignore.clone())
            .unwrap_or_default();
        let found = complete::tab(&text, caret, self.tab.take(), |word| {
            let word = word.to_lowercase();
            if unignore {
                ignored
                    .iter()
                    .filter(|m| m.to_lowercase().starts_with(&word))
                    .cloned()
                    .collect()
            } else {
                complete::members(&w, conversation, &word)
                    .into_iter()
                    .map(|m| m.nick.clone())
                    .collect()
            }
        });
        drop(w);
        let Some((next, caret, cycle)) = found else {
            return false;
        };
        self.set_text_at(&next, caret, window, cx);
        self.tab = Some(cycle);
        true
    }

    /// A printable key typed while nothing that takes text has the focus
    /// goes into the box.
    fn type_to_compose(
        &mut self,
        event: &KeystrokeEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = &event.keystroke;
        let m = key.modifiers;
        if m.control || m.alt || m.platform || m.function {
            return;
        }
        let Some(ch) = key.key_char.as_deref() else {
            return;
        };
        if ch.is_empty() || ch.chars().any(|c| c.is_control() || c.is_whitespace()) {
            return;
        }
        if event.context_stack.iter().any(|c| c.contains("Input")) {
            return;
        }
        let shown = {
            let session = self.session.read(cx);
            let joined = match (session.client(), &session.conversation) {
                (Some(client), Some(id)) => {
                    client.read().conversation(id).is_some_and(|c| c.joined)
                }
                _ => false,
            };
            joined && matches!(session.route(), Some(Route::Conversation(_)))
        };
        if !shown {
            return;
        }
        let ch = ch.to_string();
        self.input.update(cx, |s, cx| {
            s.focus(window, cx);
            let end = s.value().len();
            s.set_selected_range(end..end, cx);
            s.insert(ch, window, cx);
        });
        cx.stop_propagation();
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
        let root = div()
            .id("composer")
            .test_support()
            .key_context(KEY_CONTEXT)
            .on_action(cx.listener(|this, _: &FormatBold, window, cx| this.wrap("**", window, cx)))
            .on_action(cx.listener(|this, _: &FormatItalic, window, cx| this.wrap("*", window, cx)))
            .on_action(
                cx.listener(|this, _: &FormatUnderline, window, cx| this.wrap("__", window, cx)),
            )
            .px_4()
            .pb_3();
        let Some((name, true)) = target else {
            return root
                .text_sm()
                .text_color(theme.muted)
                .child(t!("composer-not-joined"));
        };
        let Some(ctx) = self.ctx(cx) else {
            return root;
        };
        let placeholder = SharedString::from(t!("composer-placeholder", target = name));
        if self.placeholder != placeholder {
            self.placeholder = placeholder.clone();
            self.input
                .update(cx, |s, cx| s.set_placeholder(placeholder, window, cx));
        }
        self.remember(cx);
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
        let selected = self.selected.min(suggestions.len().saturating_sub(1));
        let mut root = root.children(above);
        if !suggestions.is_empty() {
            root = root.child(self.suggestion_list(suggestions, selected, &theme, cx));
        }
        let draft = self.text(cx);
        let sendable =
            !draft.trim().is_empty() || uploads::Uploads::has_pending(cx, &ctx.conversation);
        // Count the draft the way the web composer counts its input.
        let remaining =
            TEXT_LIMIT as isize - plain_line_breaks(&draft).encode_utf16().count() as isize;
        let message_look = crate::message::layout::settings(cx);
        let font = crate::message::layout::message_font(&message_look, &theme);
        let focused = self.input.read(cx).focus_handle(cx).is_focused(window);
        let border = if focused {
            theme.control_border.blend(theme.accent.opacity(0.65))
        } else {
            theme.control_border.opacity(0.7)
        };
        root.child(
            div()
                .min_h(px(48.))
                .rounded(theme.radius_large)
                .border_1()
                .border_color(border)
                .bg(theme.input)
                .shadow_sm()
                .pl(px(14.))
                .pr(px(10.))
                .py(px(4.))
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
                                .token(draw_token)
                                .appearance(false)
                                .p_0()
                                .font_family(font)
                                .text_size(px(message_look.size as f32))
                                .line_height(relative(1.375))
                                .aria_label(t!("composer-label", target = name)),
                        ),
                )
                .child(
                    div()
                        .h(px(38.))
                        .flex_none()
                        .flex()
                        .items_center()
                        .gap_1()
                        .when(remaining < DRAFT_WARNING, |row| {
                            row.child(
                                div()
                                    .id("composer-remaining")
                                    .test_support()
                                    .role(Role::Status)
                                    .aria_label(SharedString::from(if remaining < 0 {
                                        t!("composer-characters-over", count = -remaining)
                                    } else {
                                        t!("composer-characters-left", count = remaining)
                                    }))
                                    .px_1()
                                    .text_xs()
                                    .text_color(theme.warning)
                                    .child(remaining.to_string()),
                            )
                        })
                        .children(buttons)
                        .when(sendable, |row| row.child(self.send_button(&theme, cx))),
                ),
        )
    }
}

impl ComposerView {
    /// The suggestions above the box, under the heading the first one
    /// carries.
    fn suggestion_list(
        &self,
        suggestions: Vec<Suggestion>,
        selected: usize,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let session = self.session.read(cx);
        let server = session.server().map(|s| s.url().clone());
        let client = session.client().cloned();
        let heading = suggestions.first().and_then(|s| s.heading.clone());
        let mut list = div()
            .id("composer-suggestions")
            .test_support()
            .role(Role::ListBox)
            .aria_label(
                heading
                    .clone()
                    .unwrap_or_else(|| t!("composer-suggestions").into()),
            )
            .mb_1()
            .p_1()
            .rounded(theme.radius_large)
            .border_1()
            .border_color(theme.border)
            .bg(theme.raised)
            .shadow_md()
            .max_h(px(SUGGESTIONS_HEIGHT))
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .flex()
            .flex_col();
        if let Some(heading) = heading {
            list = list.child(
                div()
                    .id("composer-suggestions-heading")
                    .test_support()
                    .px_2()
                    .pt_1()
                    .pb_0p5()
                    .text_xs()
                    .text_color(theme.muted)
                    .child(heading),
            );
        }
        for (i, suggestion) in suggestions.into_iter().enumerate() {
            let hover = theme.hover;
            let label = suggestion.label.clone();
            let detail = suggestion.detail.clone();
            let row = div()
                .id(ElementId::Name(
                    format!("suggestion-{}", suggestion.id).into(),
                ))
                .test_support()
                .role(Role::ListBoxOption)
                .aria_label(label.clone())
                .aria_selected(i == selected)
                .px_2()
                .py_1()
                .rounded_md()
                .cursor_pointer()
                .when(i == selected, |d| d.bg(hover))
                .hover(move |d| d.bg(hover));
            let row = match &suggestion.look {
                SuggestionLook::Inline => row
                    .flex()
                    .items_baseline()
                    .gap_2()
                    .child(div().flex_none().text_color(theme.text).child(label))
                    .when_some(detail, |d, detail| {
                        d.child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_color(theme.muted)
                                .text_sm()
                                .child(detail),
                        )
                    }),
                SuggestionLook::Stacked => row
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .font_family(theme.mono_font.clone())
                            .text_sm()
                            .text_color(theme.text)
                            .child(label),
                    )
                    .when_some(detail, |d, detail| {
                        d.child(div().text_xs().text_color(theme.muted).child(detail))
                    }),
                SuggestionLook::Person { member } => {
                    let picture = client.as_ref().and_then(|client| {
                        let w = client.read();
                        let m = w.members.get(member)?;
                        Some(ui::avatar::person(
                            ui::avatar::Person {
                                member: m,
                                name: &label,
                                server: server.as_ref(),
                                presence: true,
                                key: &format!("suggestion-{}", suggestion.id),
                            },
                            px(ui::avatar::TINY),
                            theme,
                        ))
                    });
                    let picture = picture.unwrap_or_else(|| {
                        ui::avatar::avatar(&label, None, px(ui::avatar::TINY), theme)
                    });
                    row.flex()
                        .items_center()
                        .gap_2()
                        .child(picture)
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_color(theme.text)
                                .child(label),
                        )
                        .when_some(detail, |d, detail| {
                            d.child(
                                div()
                                    .flex_none()
                                    .text_xs()
                                    .text_color(theme.muted)
                                    .child(detail),
                            )
                        })
                }
            };
            list = list.child(row.on_click(
                cx.listener(move |this, _, window, cx| this.apply(&suggestion, window, cx)),
            ));
        }
        list
    }

    /// The send arrow, shown when there is a message or an attachment to send.
    fn send_button(&self, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let label = SharedString::from(t!("composer-send"));
        let fill = linear_gradient(
            135.,
            linear_color_stop(theme.accent, 0.),
            linear_color_stop(theme.accent.blend(theme.danger.opacity(0.22)), 1.),
        );
        let hover = theme.accent;
        div()
            .id("composer-send")
            .test_support()
            .role(Role::Button)
            .reachable(theme)
            .aria_label(label.clone())
            .tooltip(ui::tooltip::text(label))
            .size(px(30.))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded_full()
            .cursor_pointer()
            .bg(fill)
            .hover(move |d| d.bg(hover))
            .child(ui::icon::icon(LucideIcon::Send, theme.on_accent))
            .on_click(cx.listener(|this, _, window, cx| {
                let text = this.text(cx);
                this.submit(text, window, cx);
            }))
    }
}

/// Mark what the token providers find in the box's text as tokens. A
/// stretch the caret or the selection's edge is inside stays text until the
/// next change, so a name being typed is not swallowed.
fn tokenize(input: &mut TextareaState, cx: &mut Context<TextareaState>) {
    let providers = registry::get(cx).composer.tokens.to_vec();
    if providers.is_empty() && input.tokens().is_empty() {
        return;
    }
    let text = input.value().to_string();
    let selected = input.selected_range();
    let cuts = |range: &Range<usize>, at: usize| range.start < at && at < range.end;
    let mut content = InputContent::new(text.clone());
    for provider in &providers {
        for found in (provider.find)(&text, cx) {
            let range = found.range;
            if cuts(&range, selected.start) || cuts(&range, selected.end) {
                continue;
            }
            let Some(part) = text.get(range.clone()) else {
                continue;
            };
            let id = format!("{}:{}", provider.id, found.key);
            let token = InlineToken::new(id, part.to_string());
            // Overlapping another token, or not on a character's edge.
            if let Ok(next) = content.clone().with_token(range, token) {
                content = next;
            }
        }
    }
    if content != input.content() {
        // Refused while an IME composes; the change that ends it comes back.
        let _ = input.set_tokens(content, cx);
    }
}

/// A token as its provider draws it, tinted while selected.
fn draw_token(token: &InlineTokenContext, window: &mut Window, cx: &mut App) -> AnyElement {
    let selected = theme::get(cx).selected;
    let id = token.token().id().clone();
    let (provider, key) = id.split_once(':').unwrap_or((id.as_ref(), ""));
    let view = TokenView {
        key: key.to_string().into(),
        text: token.token().text().clone(),
        line_height: token.line_height(),
    };
    let render = registry::get(cx)
        .composer
        .tokens
        .get(provider)
        .map(|p| p.render.clone());
    div()
        .id("composer-token")
        .test_support()
        .rounded_sm()
        .when(token.is_selected(), |d| d.bg(selected))
        .children(render.map(|render| render(&view, window, cx)))
        .into_any_element()
}

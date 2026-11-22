//! The compact slash-command form that takes the place of the message box.

use std::collections::{HashMap, HashSet};
use std::rc::{Rc, Weak};
use std::time::Duration;

use gpui_kit::TestSupportExt;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{Disableable, Sizable};
use gpui_kit::prelude::*;
use gpui_kit::*;
use ircord_client::Client;
use ircord_client::commands::catalog::{FieldKind, Spec, Suggest, matching, resolve};
use ircord_client::contract::{ChannelListing, ConversationKind, Member};
use ircord_client::store::drafts::TEXT_LIMIT;
use ircord_client::store::{Workspace, people};

use super::{CommandFinished, CommandOrigin, SendOutcome, View, arg_error_text, args, cmd};
use super::{
    field_hint, field_label, handler, ignore, place, problem_text, request, run, watch_settings,
};
use crate::composer::ComposerView;
use crate::features::shortcuts::MarkRead;
use crate::features::{drafts, klipy, message_actions, text};
use crate::registry::ComposerCtx;
use crate::route::Route;
use crate::session::{Session, SessionEvent};
use crate::ui::icon::IconName;
use crate::{theme, ui};
use ircord_client::commands::complete;
use ircord_client::features::Context as Allowed;
use ircord_client::store::Connection;

const MAX_SUGGESTIONS: usize = 12;
const KEY_CONTEXT: &str = "CommandEditor";

#[derive(Default)]
struct Editors(HashMap<EntityId, Entity<Editor>>);
impl Global for Editors {}

pub(crate) fn is_open(ctx: &ComposerCtx, cx: &App) -> bool {
    cx.try_global::<Editors>()
        .is_some_and(|editors| editors.0.contains_key(&ctx.composer.entity_id()))
}

pub(super) fn slot(ctx: &ComposerCtx, _: &mut Window, cx: &mut App) -> Option<AnyElement> {
    let editor = cx
        .try_global::<Editors>()?
        .0
        .get(&ctx.composer.entity_id())?;
    Some(editor.clone().into_any_element())
}

pub(super) fn key(event: &KeystrokeEvent, window: &mut Window, cx: &mut App) {
    let key = &event.keystroke;
    if key.modifiers.modified()
        || !matches!(key.key.as_str(), "up" | "down" | "tab")
        || !event
            .context_stack
            .iter()
            .any(|context| context.contains(KEY_CONTEXT))
    {
        return;
    }
    let editor = cx.try_global::<Editors>().and_then(|editors| {
        editors
            .0
            .values()
            .find(|editor| editor.read(cx).focused(window, cx).is_some())
            .cloned()
    });
    if let Some(editor) = editor {
        editor.update(cx, |editor, cx| editor.key(key.key.as_str(), window, cx));
    }
}

/// Enter guided mode for a command prefix or an accepted suggestion.
pub(crate) fn open(
    ctx: &ComposerCtx,
    name: &str,
    draft: &str,
    window: &mut Window,
    cx: &mut App,
) -> bool {
    let Some(spec) = resolve(name).filter(|spec| !spec.fields.is_empty()) else {
        return false;
    };
    let Some(client) = ctx.session.read(cx).client().cloned() else {
        return false;
    };
    let network = {
        let w = client.read();
        let context = Allowed::of(
            &w,
            &ctx.conversation,
            w.connection == Some(Connection::Live),
        );
        let Some(network) = context.network else {
            return false;
        };
        if !matching(spec.name, place(&context)).contains(&spec) {
            return false;
        }
        network.id.clone()
    };

    if spec.name == "status" {
        super::open_view(ctx, View::Status, String::new(), window, cx);
        return true;
    }
    if is_open(ctx, cx) {
        return true;
    }
    if spec
        .fields
        .iter()
        .any(|field| field.suggest == Some(Suggest::Ignored))
    {
        watch_settings(&client, cx);
    }
    klipy::dismiss(cx);
    let id = ctx.composer.entity_id();
    let editor = cx.new(|cx| Editor::new(ctx, &client, network, spec, draft, window, cx));
    cx.default_global::<Editors>().0.insert(id, editor);
    cx.refresh_windows();
    true
}

struct OptionRow {
    value: String,
    label: String,
    detail: String,
    member: Option<String>,
}

impl OptionRow {
    fn person(member: &Member) -> Self {
        Self {
            value: member.nick.clone(),
            label: people::name(member).to_string(),
            detail: format!("@{}", member.nick),
            member: Some(member.id.clone()),
        }
    }

    fn option(value: String, detail: String) -> Self {
        Self {
            label: value.clone(),
            value,
            detail,
            member: None,
        }
    }
}

struct Editor {
    composer: WeakEntity<ComposerView>,
    session: Entity<Session>,
    client: Weak<Client>,
    conversation: String,
    network: String,
    spec: &'static Spec,
    draft: String,
    inputs: Vec<Entity<InputState>>,
    directory: Vec<ChannelListing>,
    directory_round: usize,
    selected: usize,
    show_suggestions: bool,
    suggestions_scroll: ScrollHandle,
    fields_scroll: ScrollHandle,
    run_focus: FocusHandle,
    initial_focus: bool,
    busy: bool,
    error: Option<String>,
    _subscriptions: Vec<Subscription>,
}

impl Editor {
    fn new(
        ctx: &ComposerCtx,
        client: &Rc<Client>,
        network: String,
        spec: &'static Spec,
        draft: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut subscriptions = Vec::new();
        let inputs = spec
            .fields
            .iter()
            .enumerate()
            .map(|(i, field)| {
                let input = cx.new(|cx| {
                    InputState::new(window, cx)
                        .placeholder(field_label(field.name))
                        .masked(field.kind == FieldKind::Secret)
                });
                subscriptions.push(cx.subscribe_in(
                    &input,
                    window,
                    move |this, _, event, window, cx| match event {
                        InputEvent::Change => this.changed(i, cx),
                        InputEvent::PressEnter { .. } => this.enter(window, cx),
                        InputEvent::Focus => {
                            this.fields_scroll.scroll_to_item(i);
                            this.show_suggestions = false;
                            cx.notify();
                        }
                        InputEvent::Blur => cx.notify(),
                    },
                ));
                input
            })
            .collect();
        let owner = ctx.composer.entity_id();
        subscriptions.push(cx.observe_release(&ctx.composer, move |_, _, cx| {
            cx.default_global::<Editors>().0.remove(&owner);
        }));
        subscriptions.push(cx.subscribe(
            &ctx.session,
            |this, session, event: &SessionEvent, cx| {
                if matches!(event, SessionEvent::Selection | SessionEvent::Phase)
                    && (session.read(cx).conversation.as_deref() != Some(&this.conversation)
                        || !matches!(session.read(cx).route(), Some(Route::Conversation(_)))
                        || this.client(cx).is_none())
                {
                    cx.default_global::<Editors>()
                        .0
                        .remove(&this.composer.entity_id());
                    cx.refresh_windows();
                }
            },
        ));
        Self {
            composer: ctx.composer.downgrade(),
            session: ctx.session.clone(),
            client: Rc::downgrade(client),
            conversation: ctx.conversation.clone(),
            network,
            spec,
            draft: draft.to_string(),
            inputs,
            directory: Vec::new(),
            directory_round: 0,
            selected: 0,
            show_suggestions: false,
            suggestions_scroll: ScrollHandle::new(),
            fields_scroll: ScrollHandle::new(),
            run_focus: cx.focus_handle().tab_stop(true),
            initial_focus: true,
            busy: false,
            error: None,
            _subscriptions: subscriptions,
        }
    }

    fn client(&self, cx: &App) -> Option<Rc<Client>> {
        let active = self.session.read(cx).client()?;
        self.client
            .upgrade()
            .filter(|origin| Rc::ptr_eq(origin, active))
    }

    fn focused(&self, window: &Window, cx: &App) -> Option<usize> {
        self.inputs
            .iter()
            .position(|input| input.focus_handle(cx).is_focused(window))
    }

    fn changed(&mut self, index: usize, cx: &mut Context<Self>) {
        self.error = None;
        self.show_suggestions = true;
        self.selected = 0;
        self.suggestions_scroll.scroll_to_item(0);
        self.directory.clear();
        self.directory_round += 1;
        cx.notify();
        if self.spec.fields[index].suggest != Some(Suggest::Channel) {
            return;
        }
        let query = self.inputs[index]
            .read(cx)
            .value()
            .trim()
            .trim_start_matches('#')
            .to_string();
        let Some(client) = self.client(cx).filter(|_| !query.is_empty()) else {
            return;
        };
        let round = self.directory_round;
        let network = self.network.clone();
        let task = client.request(move |api| async move {
            tokio::time::sleep(Duration::from_millis(200)).await;
            api.channel_list(&network, &query).await
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(items)) = task.await {
                let _ = this.update(cx, |this, cx| {
                    if this.directory_round == round {
                        this.directory = items;
                        cx.notify();
                    }
                });
            }
        })
        .detach();
    }

    fn options(&self, index: usize, cx: &App) -> Vec<OptionRow> {
        if self.busy || !self.show_suggestions {
            return Vec::new();
        }
        let Some(client) = self.client(cx) else {
            return Vec::new();
        };
        let w = client.read();
        let query = self.inputs[index].read(cx).value();
        let query = query
            .trim()
            .trim_start_matches(['#', '@', ':'])
            .trim_end_matches(':');
        let Some(suggest) = self.spec.fields[index].suggest else {
            return Vec::new();
        };
        let Some(conversation) = w.conversation(&self.conversation) else {
            return Vec::new();
        };
        match suggest {
            Suggest::Nickname => complete::members(&w, conversation, query)
                .into_iter()
                .take(MAX_SUGGESTIONS)
                .map(OptionRow::person)
                .collect(),
            Suggest::Invitee => {
                let target = self.inputs[1].read(cx).value();
                let outside = if target.trim().is_empty() {
                    Some(conversation)
                        .filter(|c| c.kind == ConversationKind::Channel && c.party.is_none())
                } else {
                    complete::channel_named(&w, &self.network, target.trim())
                };
                complete::network_members(&w, &self.network, outside, query)
                    .into_iter()
                    .take(MAX_SUGGESTIONS)
                    .map(OptionRow::person)
                    .collect()
            }
            Suggest::Channel => self.channels(&w, query),
            Suggest::Ignored => {
                let q = query.to_lowercase();
                w.documents
                    .notification_settings
                    .value()
                    .and_then(|s| s.ignore.as_ref())
                    .into_iter()
                    .flatten()
                    .filter(|nick| nick.to_lowercase().contains(&q))
                    .take(MAX_SUGGESTIONS)
                    .map(|nick| OptionRow::option(nick.clone(), String::new()))
                    .collect()
            }
            // Status opens its own editor, which owns its emoji field.
            Suggest::Emoji => Vec::new(),
        }
    }

    fn channels(&self, w: &Workspace, query: &str) -> Vec<OptionRow> {
        let name = |n: &str| format!("#{}", n.trim_start_matches('#'));
        let joined = complete::channels(w, &self.network, query);
        let known: HashSet<_> = joined
            .iter()
            .map(|c| name(&c.name).to_lowercase())
            .collect();
        joined
            .iter()
            .map(|c| OptionRow::option(name(&c.name), c.topic.clone()))
            .chain(self.directory.iter().filter_map(|c| {
                let n = name(&c.name);
                (!known.contains(&n.to_lowercase())).then(|| OptionRow::option(n, c.topic.clone()))
            }))
            .take(MAX_SUGGESTIONS)
            .collect()
    }

    fn choose(
        &mut self,
        field: usize,
        row: &OptionRow,
        advance: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.inputs[field].update(cx, |input, cx| {
            input.set_value(row.value.clone(), window, cx)
        });
        self.show_suggestions = false;
        if advance {
            self.inputs
                .get(field + 1)
                .map_or_else(|| self.run_focus.clone(), |i| i.focus_handle(cx))
                .focus(window, cx);
        } else {
            self.inputs[field].focus_handle(cx).focus(window, cx);
        }
        cx.notify();
    }

    fn enter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(i) = self.focused(window, cx) {
            let options = self.options(i, cx);
            if let Some(row) = options.get(self.selected.min(options.len().saturating_sub(1))) {
                self.choose(i, row, false, window, cx);
                return;
            }
        }
        self.submit(window, cx);
    }

    fn key(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(field) = self.focused(window, cx) else {
            return;
        };
        let options = self.options(field, cx);
        if options.is_empty() {
            return;
        }
        match key {
            "down" | "up" => {
                let count = options.len();
                let step = if key == "down" { 1 } else { count - 1 };
                self.selected = (self.selected.min(count - 1) + step) % count;
                self.suggestions_scroll.scroll_to_item(self.selected);
                cx.notify();
            }
            "tab" => self.choose(
                field,
                &options[self.selected.min(options.len() - 1)],
                true,
                window,
                cx,
            ),
            _ => return,
        }
        cx.stop_propagation();
    }

    fn escape(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        cx.stop_propagation();
        if self
            .focused(window, cx)
            .is_some_and(|i| !self.options(i, cx).is_empty())
        {
            self.show_suggestions = false;
            cx.notify();
        } else {
            self.close(window, cx);
        }
    }

    fn close(&self, window: &mut Window, cx: &mut App) {
        cx.default_global::<Editors>()
            .0
            .remove(&self.composer.entity_id());
        if let Some(composer) = self.composer.upgrade() {
            window.defer(cx, move |window, cx| {
                let input = composer.read(cx).input.clone();
                input.focus_handle(cx).focus(window, cx);
            });
        }
        cx.refresh_windows();
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let Some(composer) = self.composer.upgrade() else {
            return;
        };
        let Some(client) = self.client(cx) else {
            return;
        };
        if self.session.read(cx).conversation.as_deref() != Some(&self.conversation) {
            return;
        }
        let values: Vec<_> = self
            .inputs
            .iter()
            .map(|i| i.read(cx).value().to_string())
            .collect();
        let line = match args::serialize(self.spec, &values) {
            Ok(line) => line,
            Err((index, error)) => {
                self.error = Some(arg_error_text(
                    error,
                    field_label(self.spec.fields[index].name),
                ));
                self.inputs[index].focus_handle(cx).focus(window, cx);
                cx.notify();
                return;
            }
        };
        if line.encode_utf16().count() > TEXT_LIMIT {
            self.error = Some(t!("composer-too-long").to_string());
            cx.notify();
            return;
        }
        let plan = cmd::plan(&client.read(), &self.conversation, &line);
        let plan = match plan {
            Ok(plan) => plan,
            Err(problem) => {
                self.error = Some(problem_text(&problem));
                cx.notify();
                return;
            }
        };
        let ctx = ComposerCtx {
            session: self.session.clone(),
            conversation: self.conversation.clone(),
            composer,
        };
        let finished = completion(&ctx, &client, &self.draft, cx);
        self.busy = true;
        self.error = None;
        self.show_suggestions = false;
        cx.notify();
        match plan {
            cmd::Plan::Text(line) => {
                if let Some(reason) = super::text_reason(&ctx, &line, cx).filter(super::blocking) {
                    self.failed(Some(message_actions::reason_text(&reason).to_string()), cx);
                } else {
                    client.send(&ctx.conversation, &text::wire_text(&line));
                    window.defer(cx, move |window, cx| finished(true, window, cx));
                }
            }
            cmd::Plan::Action(action) => {
                client.send(&ctx.conversation, &format!("/me {action}"));
                window.defer(cx, move |window, cx| finished(true, window, cx));
            }
            cmd::Plan::Open { view, args } => {
                let Some(open) = handler(view, cx) else {
                    self.failed(Some(t!("commands-not-available").to_string()), cx);
                    return;
                };
                let mut request = request(&ctx, args, cx);
                if view == View::Gif {
                    request.finished = Some(finished);
                    window.defer(cx, move |window, cx| open(&request, window, cx));
                } else {
                    window.defer(cx, move |window, cx| {
                        finished(true, window, cx);
                        open(&request, window, cx);
                    });
                }
            }
            cmd::Plan::Ignore { unignore, arg } => {
                if matches!(
                    ignore(&ctx, &client, unignore, &arg, window, cx),
                    SendOutcome::Handled
                ) {
                    window.defer(cx, move |window, cx| finished(true, window, cx));
                } else {
                    self.failed(Some(t!("commands-settings-loading").to_string()), cx);
                }
            }
            plan => run(
                &ctx,
                &client,
                plan,
                CommandOrigin::Guided(finished),
                window,
                cx,
            ),
        }
    }

    fn failed(&mut self, message: Option<String>, cx: &mut Context<Self>) {
        self.busy = false;
        self.error = message.or_else(|| Some(t!("commands-editor-failed").to_string()));
        cx.notify();
    }
}

fn completion(
    ctx: &ComposerCtx,
    client: &Rc<Client>,
    draft: &str,
    cx: &Context<Editor>,
) -> CommandFinished {
    let (session, conversation, owner) = (
        ctx.session.clone(),
        ctx.conversation.clone(),
        ctx.composer.entity_id(),
    );
    let client = Rc::downgrade(client);
    let draft = draft.to_string();
    let editor = cx.weak_entity();
    Rc::new(move |sent, window, cx| {
        let Some(active) = session.read(cx).client() else {
            return;
        };
        if client
            .upgrade()
            .is_none_or(|origin| !Rc::ptr_eq(&origin, active))
        {
            return;
        }
        let current = cx
            .try_global::<Editors>()
            .and_then(|editors| editors.0.get(&owner));
        let same_editor = current.is_some_and(|current| current.entity_id() == editor.entity_id());
        if sent
            && (same_editor
                || (current.is_none()
                    && session.read(cx).conversation.as_deref() != Some(&conversation)))
        {
            drafts::clear_if_matches(&session, &conversation, &draft, window, cx);
        }
        if same_editor {
            let _ = editor.update(cx, |this, cx| {
                if sent {
                    this.close(window, cx);
                } else {
                    this.failed(None, cx);
                }
            });
        }
    })
}

impl Render for Editor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if std::mem::take(&mut self.initial_focus) {
            self.inputs[0].focus_handle(cx).focus(window, cx);
        }
        let theme = theme::get(cx).clone();
        let field = self.focused(window, cx);
        let mut form = div()
            .id("command-editor")
            .test_support()
            .role(Role::Group)
            .aria_label(t!("commands-editor-label", command = self.spec.name))
            .key_context(KEY_CONTEXT)
            .on_action(cx.listener(|this, _: &MarkRead, window, cx| this.escape(window, cx)))
            .flex()
            .flex_col()
            .gap_1();
        if let Some(error) = &self.error {
            form = form.child(
                div()
                    .id("command-editor-error")
                    .test_support()
                    .role(Role::Alert)
                    .aria_label(error.clone())
                    .px_2()
                    .text_sm()
                    .text_color(theme.danger)
                    .child(error.clone()),
            );
        } else if let Some(index) = field {
            let options = self.options(index, cx);
            if options.is_empty() {
                let field = &self.spec.fields[index];
                let hint = field_hint(field.name);
                let requirement = if field.required {
                    t!("commands-required")
                } else {
                    t!("commands-optional")
                };
                form = form.child(
                    div()
                        .id("command-editor-hint")
                        .test_support()
                        .role(Role::Status)
                        .aria_label(format!("{hint} {requirement}"))
                        .px_2()
                        .flex()
                        .gap_2()
                        .text_sm()
                        .text_color(theme.muted)
                        .child(hint)
                        .child(div().text_color(theme.faint).child(requirement)),
                );
            } else {
                form = form.child(self.suggestions(index, options, &theme, cx));
            }
        }

        let fields = div()
            .id("command-editor-fields")
            .flex_1()
            .min_w_0()
            .flex()
            .items_center()
            .gap_1()
            .overflow_x_scroll()
            .track_scroll(&self.fields_scroll)
            .children(
                self.inputs
                    .iter()
                    .zip(self.spec.fields)
                    .map(|(input, field)| {
                        let label = field_label(field.name);
                        div().flex_1().min_w(px(105.)).child(
                            Input::new(input)
                                .id(ElementId::Name(
                                    format!("command-argument-{}", field.name).into(),
                                ))
                                .aria_label(label)
                                .small()
                                .disabled(self.busy),
                        )
                    }),
            );
        let run = if self.busy {
            t!("commands-editor-running")
        } else {
            t!("commands-editor-run")
        };
        form.child(
            div()
                .min_h(px(48.))
                .px_2()
                .py_1()
                .flex()
                .items_center()
                .gap_1()
                .rounded(theme.radius_large)
                .border_1()
                .border_color(theme.control_border)
                .bg(theme.input)
                .shadow_sm()
                .child(
                    div()
                        .flex_none()
                        .rounded_md()
                        .bg(theme.raised)
                        .px_2()
                        .py_1()
                        .text_sm()
                        .text_color(theme.accent)
                        .font_family(theme.mono_font.clone())
                        .child(format!("/{}", self.spec.name)),
                )
                .child(fields)
                .child(
                    Button::new("command-editor-run")
                        .primary()
                        .small()
                        .label(run)
                        .track_focus(&self.run_focus)
                        .disabled(self.busy)
                        .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx))),
                )
                .child(
                    Button::new("command-editor-cancel")
                        .ghost()
                        .small()
                        .icon(IconName::Close)
                        .accessibility_label(t!("commands-editor-cancel"))
                        .tooltip(t!("commands-editor-cancel"))
                        .on_click(cx.listener(|this, _, window, cx| this.close(window, cx))),
                ),
        )
    }
}

impl Editor {
    fn suggestions(
        &self,
        field: usize,
        options: Vec<OptionRow>,
        theme: &theme::Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let client = self.client(cx);
        let server = self
            .session
            .read(cx)
            .server()
            .map(|server| server.url().clone());
        let selected = self.selected.min(options.len() - 1);
        let hover = theme.hover;
        div()
            .id("command-editor-suggestions")
            .test_support()
            .role(Role::ListBox)
            .aria_label(t!("commands-editor-suggestions"))
            .flex()
            .flex_col()
            .max_h(px(160.))
            .overflow_y_scroll()
            .track_scroll(&self.suggestions_scroll)
            .p_1()
            .rounded(theme.radius_large)
            .bg(theme.raised)
            .shadow_md()
            .children(options.into_iter().enumerate().map(|(i, option)| {
                let picture = client.as_ref().and_then(|client| {
                    let member = option.member.as_deref()?;
                    let w = client.read();
                    let member = w.member(member, &self.network)?;
                    Some(ui::avatar::person(
                        ui::avatar::Person {
                            member,
                            name: &option.label,
                            server: server.as_ref(),
                            presence: true,
                            key: &format!("command-editor-person-{}", member.id),
                        },
                        px(ui::avatar::TINY),
                        theme,
                    ))
                });
                div()
                    .id(ElementId::Name(
                        format!("command-option-{}", option.value).into(),
                    ))
                    .test_support()
                    .role(Role::ListBoxOption)
                    .aria_label(option.label.clone())
                    .aria_selected(i == selected)
                    .flex()
                    .items_center()
                    .gap_2()
                    .min_h(px(32.))
                    .px_2()
                    .rounded_md()
                    .cursor_pointer()
                    .when(i == selected, |d| d.bg(hover))
                    .hover(move |d| d.bg(hover))
                    .children(picture)
                    .child(div().flex_none().text_sm().child(option.label.clone()))
                    .when(!option.detail.is_empty(), |d| {
                        d.child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_xs()
                                .text_color(theme.muted)
                                .child(option.detail.clone()),
                        )
                    })
                    .on_mouse_down(MouseButton::Left, |_, window, cx| {
                        window.prevent_default();
                        cx.stop_propagation();
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.choose(field, &option, false, window, cx)
                    }))
            }))
    }
}

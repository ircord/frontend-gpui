//! Slash commands, their arguments and autocomplete (package C1).
//!
//! What a command does is decided by `ircord_client::commands`; this file
//! runs it from the composer's send filters and words what happens. It
//! also shows, above the box, why you cannot send here, when the text is
//! too long, and the arguments of the command being typed.
//!
//! Commands that open a view (`/list`, `/server`, `/settings`, `/banlist`,
//! `/whois`, `/gif`, `/status`) call the handler the view's package
//! registers with [`on_view`]; until one does, they say so.

use std::collections::HashMap;
use std::rc::Rc;

use gpui_kit::TestSupportExt;
use gpui_kit::prelude::*;
use gpui_kit::*;
use ircord_client::Client;
use ircord_client::commands::args::{self, ArgError};
use ircord_client::commands::catalog::{self, Place, Suggest};
use ircord_client::commands::complete;
use ircord_client::commands::ignore::Notice;
use ircord_client::commands::{self as cmd, NickError, Plan, Problem, View};
use ircord_client::documents::NOTIFICATION_SETTINGS;
use ircord_client::features::{Context as Allowed, Reason};
use ircord_client::irc::channel_rights_for;
use ircord_client::store::{Connection, people};

use crate::composer::{self, FormatBold, FormatItalic, FormatUnderline};
use crate::features::message_actions::reason_text;
use crate::registry::{
    self, ComposerCtx, KeyDefault, SendFilter, SendOutcome, Slot, Suggestion, SuggestionLook,
    SuggestionProvider,
};
use crate::session::Session;
use crate::theme;
use crate::ui;

mod editor;

pub(crate) use editor::{is_open as editor_is_open, open as open_editor};

type CommandFinished = Rc<dyn Fn(bool, &mut Window, &mut App)>;

/// What a view-opening command hands the view's package.
#[derive(Clone)]
pub struct ViewRequest {
    pub session: Entity<Session>,
    /// Where the command was typed.
    pub conversation: String,
    pub network: String,
    /// What was typed after the command, trimmed.
    pub args: String,
    /// The original draft, including any whitespace removed in planning.
    pub draft: String,
    /// Report whether an asynchronous command succeeded to its editor.
    pub(super) finished: Option<CommandFinished>,
}

pub type ViewHandler = Rc<dyn Fn(&ViewRequest, &mut Window, &mut App)>;

#[derive(Default)]
struct Views(HashMap<View, ViewHandler>);

impl Global for Views {}

/// Handle a view-opening command; the package that draws the view calls
/// this from its `init`.
pub fn on_view(
    cx: &mut App,
    view: View,
    handler: impl Fn(&ViewRequest, &mut Window, &mut App) + 'static,
) {
    cx.default_global::<Views>()
        .0
        .insert(view, Rc::new(handler));
}

/// The client crates whose notification settings are watched, so `/ignore`
/// can read the ignore list.
#[derive(Default)]
struct Watched(Vec<std::rc::Weak<Client>>);

impl Global for Watched {}

/// How many people an argument's list shows.
const ARGUMENT_LIMIT: usize = 12;

/// `/kick bo` → `bob`: people for a nickname argument, in the same rows as
/// the @ list. `/invite` offers the people not yet in the channel it names,
/// or else the one you are in.
fn arguments(ctx: &ComposerCtx, text: &str, cx: &App) -> Vec<Suggestion> {
    let (_, caret) = crate::composer::typed(&ctx.composer, cx);
    let Some(arg) = complete::argument(text, caret) else {
        return Vec::new();
    };
    let session = ctx.session.read(cx);
    let Some(client) = session.client() else {
        return Vec::new();
    };
    let w = client.read();
    let Some(conversation) = w.conversation(&ctx.conversation) else {
        return Vec::new();
    };
    let found = match arg.suggest {
        Suggest::Invitee => {
            let target = match &arg.channel {
                Some(name) => complete::channel_named(&w, &conversation.network_id, name),
                None => Some(conversation).filter(|c| {
                    c.kind == ircord_client::contract::ConversationKind::Channel
                        && c.party.is_none()
                }),
            };
            complete::network_members(&w, &conversation.network_id, target, &arg.query)
        }
        _ => complete::members(&w, conversation, &arg.query),
    };
    let heading = SharedString::from(t!("composer-people", query = arg.query.as_str()));
    found
        .into_iter()
        .take(ARGUMENT_LIMIT)
        .map(|m| Suggestion {
            id: format!("person-{}", m.nick).into(),
            label: people::name(m).to_string().into(),
            detail: Some(format!("@{}", m.nick).into()),
            replace: arg.range.clone(),
            insert: format!("{} ", m.nick),
            heading: Some(heading.clone()),
            look: SuggestionLook::Person {
                member: m.id.clone(),
            },
        })
        .collect()
}

pub fn init(cx: &mut App) {
    cx.intercept_keystrokes(editor::key).detach();
    registry::register(cx, |r| {
        let c = &mut r.composer;
        c.input.add(Slot::new("command-editor", editor::slot));
        c.suggestions.add(SuggestionProvider {
            id: "commands".into(),
            order: 0,
            suggest: Rc::new(composer::suggest::commands),
        });
        c.suggestions.add(SuggestionProvider {
            id: "arguments".into(),
            order: 5,
            suggest: Rc::new(arguments),
        });
        c.suggestions.add(SuggestionProvider {
            id: "people".into(),
            order: 10,
            suggest: Rc::new(composer::suggest::people),
        });
        c.suggestions.add(SuggestionProvider {
            id: "channels".into(),
            order: 20,
            suggest: Rc::new(composer::suggest::channels),
        });
        c.send_filters.add(SendFilter {
            id: "commands".into(),
            // First: a command never reaches Markdown or the length check.
            order: 100,
            filter: Rc::new(command_filter),
        });
        c.send_filters.add(SendFilter {
            id: "commands-limits".into(),
            // After Markdown to IRC (T1), before the message is sent (A1, 900).
            order: 800,
            filter: Rc::new(limit_filter),
        });
        c.above
            .add(Slot::new("commands-guide", render_guide).order(20));
        let context = Some(composer::KEY_CONTEXT);
        r.keys.add(
            KeyDefault::new(
                "format-bold",
                t!("commands-key-bold"),
                if cfg!(target_os = "macos") {
                    "cmd-b"
                } else {
                    "ctrl-b"
                },
                context,
                FormatBold,
            )
            .order(10),
        );
        r.keys.add(
            KeyDefault::new(
                "format-italic",
                t!("commands-key-italic"),
                if cfg!(target_os = "macos") {
                    "cmd-i"
                } else {
                    "ctrl-i"
                },
                context,
                FormatItalic,
            )
            .order(11),
        );
        r.keys.add(
            KeyDefault::new(
                "format-underline",
                t!("commands-key-underline"),
                if cfg!(target_os = "macos") {
                    "cmd-u"
                } else {
                    "ctrl-u"
                },
                context,
                FormatUnderline,
            )
            .order(12),
        );
    });
}

// ---- Running commands ----

pub(crate) fn place(context: &Allowed<'_>) -> Place {
    Place {
        channel: context.conversation.is_some_and(|c| {
            c.kind == ircord_client::contract::ConversationKind::Channel && c.party.is_none()
        }),
        operator: channel_rights_for(context.own_modes(), context.conversation, context.network).op,
    }
}

fn command_filter(
    ctx: &ComposerCtx,
    text: String,
    window: &mut Window,
    cx: &mut App,
) -> SendOutcome {
    if !text.trim_start().starts_with('/') {
        return SendOutcome::Continue(text);
    }
    if let Some(spec) = args::exact(&text)
        && (spec.name == "status" || spec.fields.iter().any(|field| field.required))
        && open_editor(ctx, spec.name, &text, window, cx)
    {
        return SendOutcome::Keep;
    }
    let Some(client) = ctx.session.read(cx).client().cloned() else {
        return SendOutcome::Continue(text);
    };
    let planned = cmd::plan(&client.read(), &ctx.conversation, &text);
    match planned {
        Err(problem) => {
            ui::toast::info(window, cx, problem_text(&problem));
            SendOutcome::Keep
        }
        Ok(Plan::Text(text)) => SendOutcome::Continue(text),
        Ok(Plan::Action(action)) => {
            // Sent as typed: `/slap _jules_` must not turn into italics.
            client.send(&ctx.conversation, &format!("/me {action}"));
            SendOutcome::Handled
        }
        Ok(Plan::Open { view, args }) => {
            open_view(ctx, view, args, window, cx);
            if view == View::Gif {
                SendOutcome::Keep
            } else {
                SendOutcome::Handled
            }
        }
        Ok(Plan::Ignore { unignore, arg }) => ignore(ctx, &client, unignore, &arg, window, cx),
        Ok(plan) => {
            run(ctx, &client, plan, CommandOrigin::Typed(text), window, cx);
            SendOutcome::Handled
        }
    }
}

enum CommandOrigin {
    Typed(String),
    Guided(CommandFinished),
}

/// Run a plan that talks to the server; on failure say why and put the
/// command back in the box.
fn run(
    ctx: &ComposerCtx,
    client: &Rc<Client>,
    plan: Plan,
    origin: CommandOrigin,
    window: &mut Window,
    cx: &mut App,
) {
    let then = match &plan {
        Plan::Raw { then, .. } => *then,
        _ => None,
    };
    let task = cmd::execute(client, plan);
    let (ctx, client) = (ctx.clone(), client.clone());
    window
        .spawn(cx, async move |acx| {
            let result = task
                .await
                .map_err(|e| e.to_string())
                .and_then(|r| r.map_err(|e| e.to_string()));
            let _ = acx.update(|window, cx| {
                if !ctx
                    .session
                    .read(cx)
                    .client()
                    .is_some_and(|active| Rc::ptr_eq(active, &client))
                {
                    return;
                }
                match result {
                    Err(e) => match origin {
                        CommandOrigin::Typed(typed) => {
                            ui::toast::error(window, cx, e);
                            if ctx.session.read(cx).conversation.as_deref()
                                == Some(&ctx.conversation)
                            {
                                ctx.composer.update(cx, |c, cx| {
                                    if c.text(cx).is_empty() {
                                        c.set_text_at(&typed, typed.len(), window, cx);
                                    }
                                });
                            }
                        }
                        CommandOrigin::Guided(finished) => finished(false, window, cx),
                    },
                    Ok(done) => {
                        if let CommandOrigin::Guided(finished) = origin {
                            finished(true, window, cx);
                        }
                        if let Some(id) = &done.open {
                            ctx.session
                                .update(cx, |s, cx| s.select_conversation(id, cx));
                            if let Some(text) = &done.send {
                                client.send(id, text);
                            }
                        }
                        if let Some(view) = then {
                            open_view_quietly(&ctx, view, String::new(), window, cx);
                        }
                    }
                }
            });
        })
        .detach();
}

fn request(ctx: &ComposerCtx, args: String, cx: &App) -> ViewRequest {
    let network = ctx
        .session
        .read(cx)
        .client()
        .and_then(|c| {
            c.read()
                .conversation(&ctx.conversation)
                .map(|c| c.network_id.clone())
        })
        .unwrap_or_default();
    ViewRequest {
        session: ctx.session.clone(),
        conversation: ctx.conversation.clone(),
        network,
        args,
        draft: composer::typed(&ctx.composer, cx).0,
        finished: None,
    }
}

fn handler(view: View, cx: &App) -> Option<ViewHandler> {
    cx.try_global::<Views>()?.0.get(&view).cloned()
}

/// Open a view, or say it is not available yet.
fn open_view(ctx: &ComposerCtx, view: View, args: String, window: &mut Window, cx: &mut App) {
    match handler(view, cx) {
        Some(h) => h(&request(ctx, args, cx), window, cx),
        None => ui::toast::info(window, cx, t!("commands-not-available")),
    }
}

/// Open a view after a raw line (the server window after `/whois`), if
/// its package handles it; the line has gone out either way.
fn open_view_quietly(
    ctx: &ComposerCtx,
    view: View,
    args: String,
    window: &mut Window,
    cx: &mut App,
) {
    let found = handler(view, cx).or_else(|| handler(View::ServerWindow, cx));
    if let Some(h) = found {
        h(&request(ctx, args, cx), window, cx);
    }
}

fn ignore(
    ctx: &ComposerCtx,
    client: &Rc<Client>,
    unignore: bool,
    arg: &str,
    window: &mut Window,
    cx: &mut App,
) -> SendOutcome {
    let outcome = cmd::ignore_outcome(&client.read(), &ctx.conversation, unignore, arg);
    let Some(outcome) = outcome else {
        watch_settings(client, cx);
        ui::toast::info(window, cx, t!("commands-settings-loading"));
        return SendOutcome::Keep;
    };
    if let Some(list) = outcome.ignore {
        let mut next = client
            .read()
            .documents
            .notification_settings
            .value()
            .cloned()
            .unwrap_or_default();
        next.ignore = Some(list);
        client.edit_document(&NOTIFICATION_SETTINGS, next);
    }
    ui::toast::info(window, cx, notice_text(&outcome.notice));
    SendOutcome::Handled
}

/// Read the notification settings once per signed-in client.
fn watch_settings(client: &Rc<Client>, cx: &mut App) {
    let watched = cx.default_global::<Watched>();
    watched.0.retain(|w| w.strong_count() > 0);
    if watched
        .0
        .iter()
        .any(|w| w.upgrade().is_some_and(|w| Rc::ptr_eq(&w, client)))
    {
        return;
    }
    watched.0.push(Rc::downgrade(client));
    client.watch_document(&NOTIFICATION_SETTINGS);
}

// ---- Why you cannot send, and the length limits ----

/// Why sending here is refused outright: moderated, or a group that is not
/// active. A network that is down still queues the message in the outbox.
fn blocking(reason: &Reason) -> bool {
    matches!(reason, Reason::SendModerated | Reason::Party(_))
}

fn send_reason(ctx: &ComposerCtx, cx: &App) -> Option<Reason> {
    let session = ctx.session.read(cx);
    let w = session.client()?.read();
    Allowed::of(
        &w,
        &ctx.conversation,
        w.connection == Some(Connection::Live),
    )
    .send_reason()
}

fn text_reason(ctx: &ComposerCtx, text: &str, cx: &App) -> Option<Reason> {
    let session = ctx.session.read(cx);
    let w = session.client()?.read();
    let allowed = Allowed::of(&w, &ctx.conversation, true);
    match text.strip_prefix("/me ") {
        Some(action) => allowed.text_reason(action, true),
        None if text.starts_with('/') && !text.starts_with("//") => None,
        None => allowed.text_reason(text.strip_prefix('/').unwrap_or(text), false),
    }
}

fn limit_filter(ctx: &ComposerCtx, text: String, window: &mut Window, cx: &mut App) -> SendOutcome {
    if let Some(reason) = send_reason(ctx, cx).filter(blocking) {
        ui::toast::info(window, cx, reason_text(&reason));
        return SendOutcome::Keep;
    }
    if let Some(reason) = text_reason(ctx, &text, cx) {
        ui::toast::info(window, cx, reason_text(&reason));
        return SendOutcome::Keep;
    }
    SendOutcome::Continue(text)
}

// ---- The bar above the box ----

fn render_guide(ctx: &ComposerCtx, _window: &mut Window, cx: &mut App) -> Option<AnyElement> {
    if editor::is_open(ctx, cx) {
        return None;
    }
    let theme = theme::get(cx).clone();
    let (text, _) = composer::typed(&ctx.composer, cx);
    let mut column = div().flex().flex_col().gap_1().mb_1().text_sm();
    let mut any = false;
    // Joining is the composer's own message; other reasons show here.
    if let Some(reason) = send_reason(ctx, cx).filter(|r| *r != Reason::SendJoin) {
        any = true;
        let why = reason_text(&reason);
        column = column.child(
            div()
                .id("composer-send-reason")
                .test_support()
                .role(Role::Status)
                .aria_label(why.clone())
                .text_color(if blocking(&reason) {
                    theme.warning
                } else {
                    theme.muted
                })
                .child(why),
        );
    }
    // Measured as it will be sent: a Windows paste's `\r\n` goes out as `\n`.
    let sent = ircord_client::features::plain_line_breaks(&text);
    if let Some(reason) = text_reason(ctx, &sent, cx) {
        any = true;
        let why = reason_text(&reason);
        column = column.child(
            div()
                .id("composer-limit")
                .test_support()
                .role(Role::Status)
                .aria_label(why.clone())
                .text_color(theme.danger)
                .child(why),
        );
    }
    if let Some(guide) = args::guide(&text) {
        any = true;
        let mut syntax = div().flex().flex_wrap().gap_1().child(
            div()
                .text_color(theme.text)
                .font_weight(FontWeight::BOLD)
                .child(format!("/{}", guide.spec.name)),
        );
        for (i, field) in guide.spec.fields.iter().enumerate() {
            let label = field_label(field.name);
            let shown = if field.required {
                format!("<{label}>")
            } else {
                format!("[{label}]")
            };
            let current = guide.current == Some(i);
            syntax = syntax.child(
                div()
                    .px_1()
                    .rounded_sm()
                    .when(current, |d| d.bg(theme.selected).text_color(theme.text))
                    .when(!current, |d| d.text_color(theme.muted))
                    .child(shown),
            );
        }
        let (line, colour) = match guide.error {
            Some((i, error)) => (
                arg_error_text(error, field_label(guide.spec.fields[i].name)),
                theme.danger,
            ),
            None => match guide.field() {
                Some(field) => (
                    format!(
                        "{} \u{2014} {}",
                        if field.required {
                            t!("commands-required")
                        } else {
                            t!("commands-optional")
                        },
                        field_hint(field.name)
                    ),
                    theme.muted,
                ),
                None => (description(guide.spec.name).to_string(), theme.muted),
            },
        };
        let label = format!(
            "/{} {}: {line}",
            guide.spec.name,
            args_text(guide.spec.name)
        );
        column = column.child(
            div()
                .id("composer-guide")
                .test_support()
                .role(Role::Status)
                .aria_label(label)
                .px_2()
                .py_1()
                .rounded_md()
                .bg(theme.sidebar)
                .flex()
                .flex_col()
                .child(syntax)
                .child(div().text_color(colour).child(line)),
        );
    }
    any.then(|| column.into_any_element())
}

// ---- Text ----

/// What a command does, as suggestions show it.
pub fn description(name: &str) -> &'static str {
    match name {
        "me" => t!("commands-description-me"),
        "slap" => t!("commands-description-slap"),
        "msg" => t!("commands-description-msg"),
        "query" => t!("commands-description-query"),
        "join" => t!("commands-description-join"),
        "part" => t!("commands-description-part"),
        "nick" => t!("commands-description-nick"),
        "topic" => t!("commands-description-topic"),
        "away" => t!("commands-description-away"),
        "status" => t!("commands-description-status"),
        "afk" => t!("commands-description-afk"),
        "brb" => t!("commands-description-brb"),
        "back" => t!("commands-description-back"),
        "op" => t!("commands-description-op"),
        "deop" => t!("commands-description-deop"),
        "voice" => t!("commands-description-voice"),
        "devoice" => t!("commands-description-devoice"),
        "kick" => t!("commands-description-kick"),
        "ban" => t!("commands-description-ban"),
        "invite" => t!("commands-description-invite"),
        "mode" => t!("commands-description-mode"),
        "whois" => t!("commands-description-whois"),
        "ignore" => t!("commands-description-ignore"),
        "unignore" => t!("commands-description-unignore"),
        "list" => t!("commands-description-list"),
        "server" => t!("commands-description-server"),
        "settings" => t!("commands-description-settings"),
        "banlist" => t!("commands-description-banlist"),
        "gif" => t!("commands-description-gif"),
        "quote" => t!("commands-description-quote"),
        "attract" => t!("commands-description-attract"),
        _ => "",
    }
}

/// A command's arguments as a person reads them, `<nick> [reason]`.
pub fn args_text(name: &str) -> &'static str {
    match name {
        "me" => t!("commands-args-me"),
        "slap" | "query" | "op" | "deop" | "voice" | "devoice" | "ban" | "whois" | "unignore" => {
            t!("commands-args-nick")
        }
        "msg" => t!("commands-args-msg"),
        "join" => t!("commands-args-join"),
        "part" | "afk" | "brb" => t!("commands-args-reason"),
        "nick" => t!("commands-args-new-nick"),
        "topic" => t!("commands-args-topic"),
        "away" => t!("commands-args-away"),
        "status" => t!("commands-args-status"),
        "kick" => t!("commands-args-kick"),
        "invite" => t!("commands-args-invite"),
        "mode" => t!("commands-args-mode"),
        "ignore" => t!("commands-args-ignore"),
        "list" => t!("commands-args-list"),
        "gif" => t!("commands-args-gif"),
        "quote" => t!("commands-args-quote"),
        _ => "",
    }
}

fn field_label(name: &str) -> &'static str {
    match name {
        "action" => t!("commands-field-action"),
        "nickname" => t!("commands-field-nickname"),
        "message" => t!("commands-field-message"),
        "channel" => t!("commands-field-channel"),
        "key" => t!("commands-field-key"),
        "reason" => t!("commands-field-reason"),
        "topic" => t!("commands-field-topic"),
        "emoji" => t!("commands-field-emoji"),
        "duration" => t!("commands-field-duration"),
        "mask" => t!("commands-field-mask"),
        "modes" => t!("commands-field-modes"),
        "arguments" => t!("commands-field-arguments"),
        "filter" => t!("commands-field-filter"),
        "query" => t!("commands-field-query"),
        "line" => t!("commands-field-line"),
        _ => "",
    }
}

fn field_hint(name: &str) -> &'static str {
    match name {
        "action" => t!("commands-hint-action"),
        "nickname" => t!("commands-hint-nickname"),
        "message" => t!("commands-hint-message"),
        "channel" => t!("commands-hint-channel"),
        "key" => t!("commands-hint-key"),
        "reason" => t!("commands-hint-reason"),
        "topic" => t!("commands-hint-topic"),
        "emoji" => t!("commands-hint-emoji"),
        "duration" => t!("commands-hint-duration"),
        "mask" => t!("commands-hint-mask"),
        "modes" => t!("commands-hint-modes"),
        "arguments" => t!("commands-hint-arguments"),
        "filter" => t!("commands-hint-filter"),
        "query" => t!("commands-hint-query"),
        "line" => t!("commands-hint-line"),
        _ => "",
    }
}

fn arg_error_text(error: ArgError, field: &str) -> String {
    match error {
        ArgError::Required => t!("commands-error-required", field = field),
        ArgError::Control => t!("commands-error-control", field = field),
        ArgError::Token => t!("commands-error-token", field = field),
        ArgError::MaxBytes => t!("commands-error-max-bytes", field = field),
        ArgError::Order => t!("commands-error-order", field = field),
    }
}

/// Why a command did not run, as the person reads it.
pub fn problem_text(problem: &Problem) -> String {
    match problem {
        Problem::Unknown(command) => t!("commands-unknown", command = command),
        Problem::ChannelOnly(command) => t!("commands-channel-only", command = command),
        Problem::Usage(spec) => t!(
            "commands-usage",
            command = spec.name,
            args = args_text(spec.name)
        ),
        Problem::PersonMissing(name) => t!("commands-person-missing", name = name),
        Problem::Nickname(NickError::Required) => t!("commands-nick-required").into(),
        Problem::Nickname(NickError::Spaces) => t!("commands-nick-spaces").into(),
        Problem::Nickname(NickError::Syntax) => t!("commands-nick-syntax").into(),
        Problem::Nickname(NickError::TooLong(maximum)) => {
            t!("commands-nick-too-long", maximum = maximum)
        }
        Problem::NotConnected => t!("commands-not-connected").into(),
        Problem::AttractUnsupported => t!("commands-attract-unsupported").into(),
    }
}

fn notice_text(notice: &Notice) -> String {
    let max = ircord_client::commands::ignore::MAX_ENTRIES;
    let longest = ircord_client::commands::ignore::MAX_ENTRY;
    match notice {
        Notice::Usage { command } => t!("commands-ignore-usage", command = command),
        Notice::List(list) if list.len() == 1 => {
            t!("commands-ignore-list-one", names = list.join(", "))
        }
        Notice::List(list) => t!(
            "commands-ignore-list",
            count = list.len(),
            names = list.join(", ")
        ),
        Notice::Empty => t!("commands-ignore-empty").into(),
        Notice::TooLong => t!("commands-ignore-too-long", limit = longest),
        Notice::AlreadyIgnoring(target) => t!("commands-ignore-already", target = target),
        Notice::Yourself => t!("commands-ignore-self").into(),
        Notice::HiddenBy { target, masks } => t!(
            "commands-ignore-hidden-by",
            target = target,
            masks = masks.join(", ")
        ),
        Notice::Maximum => t!("commands-ignore-maximum", limit = max),
        Notice::Added(target) => t!("commands-ignore-added", target = target),
        Notice::Removed(target) => t!("commands-ignore-removed", target = target),
        Notice::Covered { target, masks } => t!(
            "commands-ignore-covered",
            target = target,
            masks = masks.join(", "),
            first = masks.first().cloned().unwrap_or_default()
        ),
        Notice::NotIgnoring(target) => t!("commands-ignore-not-ignoring", target = target),
    }
}

//! Message actions (package A1): the hover toolbar and right-click menu on a
//! message — reply, react, edit, copy text, copy link, earlier versions and
//! delete — plus Up in an empty box to edit your newest message, the reply
//! and edit bar above the box, failed sends put back into the box or
//! discarded, the server's outbox restored at start, and deleted messages
//! hidden from the timeline.
//!
//! An action the network does not offer at all is not shown; one it offers
//! but that is not possible for this message shows disabled with the reason.
//!
//! Other packages use [`react`] (E1's full emoji picker) and may replace the
//! `react` menu entry by registering an entry with the same ID.

use crate::menu_order;
use crate::ui::focus::Reachable;
use std::collections::HashMap;

use gpui_kit::component::Sizable;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::prelude::*;
use gpui_kit::*;
use ircord_client::client::random_id;
use ircord_client::contract::{self as c, ConversationKind, Message, MessageKind, PartyState};
use ircord_client::features::{Context as Allowed, MessageAction, Reason};
use ircord_client::format;
use ircord_client::store::{Changes, Connection, PendingSend, SendState, Workspace};

use crate::composer::ComposerView;
use crate::registry::{
    self, ComposerCtx, MenuItem, MessageCtx, RowFilter, SendFilter, SendOutcome, Slot,
};
use crate::session::{Session, SessionEvent};
use crate::timeline::rows::Row;
use crate::ui;
use crate::ui::icon::IconName;

/// The reactions offered first, as the web client offers them before any
/// recent emoji exist.
pub const QUICK_REACTIONS: [&str; 5] = ["👍", "❤️", "😂", "😮", "😢"];

/// What the message box is doing in a conversation besides writing a new
/// message.
#[derive(Clone, Debug, PartialEq)]
pub enum Mode {
    /// The next message replies to `message`.
    Reply {
        message: String,
        nick: String,
        text: String,
    },
    /// The box holds the text of `message`; sending saves the edit. `draft`
    /// is what the box held before, put back afterwards.
    Edit { message: String, draft: String },
}

#[derive(Default)]
struct Actions {
    session: Option<WeakEntity<Session>>,
    composer: Option<WeakEntity<ComposerView>>,
    /// Conversation → mode.
    modes: HashMap<String, Mode>,
}

impl Global for Actions {}

pub fn init(cx: &mut App) {
    cx.default_global::<Actions>();
    cx.observe_new::<Session>(|_, _, cx| {
        let weak = cx.weak_entity();
        cx.global_mut::<Actions>().session = Some(weak);
        cx.subscribe_self(|session: &mut Session, event: &SessionEvent, _| {
            if let SessionEvent::Changed(changes) = event
                && changes.reset
                && let Some(client) = session.client()
            {
                restore_outbox(client);
            }
        })
        .detach();
    })
    .detach();
    cx.intercept_keystrokes(on_key).detach();
    registry::register(cx, register);
}

fn register(r: &mut registry::Registry) {
    use MessageAction as A;
    let m = &mut r.message_menu;
    m.add(
        MenuItem::new(
            "reply",
            t!("message-actions-reply"),
            |ctx: &MessageCtx, window: &mut Window, cx: &mut App| {
                if let Some(why) = reason(ctx, A::Replies, cx) {
                    return ui::toast::info(window, cx, why);
                }
                start_reply(ctx, window, cx);
            },
        )
        .icon(IconName::Undo2)
        .order(menu_order::message::REPLY)
        .toolbar()
        .when(|ctx: &MessageCtx, cx: &App| offered(ctx, A::Replies, cx))
        .unavailable_when(|ctx: &MessageCtx, cx: &App| reason(ctx, A::Replies, cx)),
    );
    m.add(
        MenuItem::new(
            "react",
            t!("message-actions-react"),
            |ctx: &MessageCtx, window: &mut Window, cx: &mut App| {
                if let Some(why) = reason(ctx, A::Reactions, cx) {
                    return ui::toast::info(window, cx, why);
                }
                open_quick_reactions(ctx.clone(), window, cx);
            },
        )
        .icon(IconName::ThumbsUp)
        .order(menu_order::message::ADD_REACTION)
        .toolbar()
        .when(|ctx: &MessageCtx, cx: &App| offered(ctx, A::Reactions, cx))
        .unavailable_when(|ctx: &MessageCtx, cx: &App| reason(ctx, A::Reactions, cx)),
    );
    for (i, emoji) in QUICK_REACTIONS.iter().enumerate() {
        m.add(
            MenuItem::new(
                format!("react-{i}"),
                t!("message-actions-react-with", emoji = emoji),
                move |ctx: &MessageCtx, window: &mut Window, cx: &mut App| {
                    react(
                        &ctx.session,
                        &ctx.conversation,
                        &ctx.message,
                        emoji,
                        window,
                        cx,
                    )
                },
            )
            .order(menu_order::message::QUICK_REACTIONS + i as i32)
            .when(|ctx: &MessageCtx, cx: &App| {
                offered(ctx, A::Reactions, cx) && reason(ctx, A::Reactions, cx).is_none()
            }),
        );
    }
    m.add(
        MenuItem::new(
            "edit",
            t!("message-actions-edit"),
            |ctx: &MessageCtx, window: &mut Window, cx: &mut App| {
                if let Some(why) = reason(ctx, A::Edit, cx) {
                    return ui::toast::info(window, cx, why);
                }
                start_edit(&ctx.conversation, &ctx.message, window, cx);
            },
        )
        .icon(IconName::Replace)
        .order(menu_order::message::EDIT)
        .toolbar()
        .when(|ctx: &MessageCtx, cx: &App| offered(ctx, A::Edit, cx))
        .unavailable_when(|ctx: &MessageCtx, cx: &App| reason(ctx, A::Edit, cx)),
    );
    m.add(
        MenuItem::new(
            "copy-text",
            t!("message-actions-copy-text"),
            |ctx: &MessageCtx, window: &mut Window, cx: &mut App| {
                let Some(text) = with_message(ctx, cx, |_, m, _| format::parse(&m.text).text)
                else {
                    return;
                };
                cx.write_to_clipboard(ClipboardItem::new_string(text));
                ui::toast::success(window, cx, t!("message-actions-copied"));
            },
        )
        .icon(IconName::Copy)
        .order(menu_order::message::COPY_TEXT)
        .when(|ctx: &MessageCtx, cx: &App| with_message(ctx, cx, |_, _, _| ()).is_some()),
    );
    m.add(
        MenuItem::new(
            "copy-link",
            t!("message-actions-copy-link"),
            |ctx: &MessageCtx, window: &mut Window, cx: &mut App| {
                let Some(link) = message_link(ctx, cx) else {
                    return;
                };
                cx.write_to_clipboard(ClipboardItem::new_string(link));
                ui::toast::success(window, cx, t!("message-actions-link-copied"));
            },
        )
        .icon(IconName::ExternalLink)
        .order(menu_order::message::COPY_LINK)
        .when(|ctx: &MessageCtx, cx: &App| message_link(ctx, cx).is_some()),
    );
    m.add(
        MenuItem::new(
            "versions",
            t!("message-actions-versions"),
            |ctx: &MessageCtx, window: &mut Window, cx: &mut App| {
                if let Some(versions) = with_message(ctx, cx, |_, m, _| earlier_versions(m)) {
                    open_versions(ctx.message.clone(), versions, window, cx);
                }
            },
        )
        .icon(IconName::Undo)
        .order(menu_order::message::VERSIONS)
        .when(|ctx: &MessageCtx, cx: &App| {
            with_message(ctx, cx, |_, m, _| !earlier_versions(m).is_empty()).unwrap_or(false)
        }),
    );
    m.add(
        MenuItem::new(
            "delete",
            t!("message-actions-delete"),
            |ctx: &MessageCtx, window: &mut Window, cx: &mut App| {
                if let Some(why) = reason(ctx, A::Redact, cx) {
                    return ui::toast::info(window, cx, why);
                }
                confirm_delete(ctx.clone(), window, cx);
            },
        )
        .icon(IconName::Delete)
        .order(menu_order::message::DELETE)
        .danger()
        .when(|ctx: &MessageCtx, cx: &App| ctx.own && offered(ctx, A::Redact, cx))
        .unavailable_when(|ctx: &MessageCtx, cx: &App| reason(ctx, A::Redact, cx)),
    );

    r.composer.above.add(
        Slot::new("message-actions", |ctx: &ComposerCtx, window, cx| {
            cx.global_mut::<Actions>().composer = Some(ctx.composer.downgrade());
            crate::composer::reply::render(ctx, window, cx)
        })
        .order(10),
    );
    r.composer.send_filters.add(SendFilter {
        id: "message-actions".into(),
        // After commands, Markdown and length checks: this filter sends.
        order: 900,
        filter: std::rc::Rc::new(send_filter),
    });
    r.row_filters.add(RowFilter {
        id: "message-actions-deleted".into(),
        order: 0,
        keep: std::rc::Rc::new(|row, _, _| !matches!(row, Row::Message(m) if m.deleted)),
    });
}

// ---- What the workspace allows ----

/// Run `f` on the message `ctx` names, with what the network allows.
fn with_message<R>(
    ctx: &MessageCtx,
    cx: &App,
    f: impl FnOnce(&Workspace, &Message, &Allowed) -> R,
) -> Option<R> {
    let session = ctx.session.read(cx);
    let client = session.client()?;
    let w = client.read();
    let message = w.message(&ctx.conversation, &ctx.message)?;
    let allowed = Allowed::of(
        &w,
        &ctx.conversation,
        w.connection == Some(Connection::Live),
    );
    Some(f(&w, message, &allowed))
}

fn chat_kind(m: &Message) -> bool {
    matches!(
        m.kind,
        MessageKind::Message | MessageKind::Action | MessageKind::Notice
    ) && m.deleted != Some(true)
}

/// Whether the network offers `action` at all on this message, so the entry shows.
fn offered(ctx: &MessageCtx, action: MessageAction, cx: &App) -> bool {
    with_message(ctx, cx, |w, m, a| {
        let network = a.network.map(|n| &n.features);
        let supported = network.is_some_and(|f| match action {
            MessageAction::Replies => f.replies,
            MessageAction::Reactions => f.reactions,
            MessageAction::Redact => f.redact,
            MessageAction::Edit => f.edit,
        });
        let own = a.network.is_some_and(|n| w.is_self(&m.sender_id, &n.id));
        supported
            && chat_kind(m)
            && match action {
                MessageAction::Edit => {
                    own && matches!(m.kind, MessageKind::Message | MessageKind::Action)
                        && m.gif.is_none()
                        && m.attachment.is_none()
                        && m.attachments.as_ref().is_none_or(|a| a.is_empty())
                }
                _ => true,
            }
    })
    .unwrap_or(false)
}

/// Why `action` is not possible on this message now, as the person reads it.
fn reason(ctx: &MessageCtx, action: MessageAction, cx: &App) -> Option<SharedString> {
    with_message(ctx, cx, |_, m, a| a.action_reason(m, action))
        .flatten()
        .map(|r| reason_text(&r).into())
}

/// A reason in the person's language.
pub fn reason_text(reason: &Reason) -> String {
    use Reason::*;
    match *reason {
        SendReconnect => t!("message-actions-reason-send-reconnect").into(),
        SendJoin => t!("message-actions-reason-send-join").into(),
        SendModerated => t!("message-actions-reason-moderated").into(),
        ActionReconnect => t!("message-actions-reason-reconnect").into(),
        ActionJoin => t!("message-actions-reason-join").into(),
        ActionInvalidId => t!("message-actions-reason-invalid-id").into(),
        RedactOwnOnly => t!("message-actions-reason-delete-own").into(),
        EditOwnOnly => t!("message-actions-reason-edit-own").into(),
        EditNotice => t!("message-actions-reason-edit-notice").into(),
        EditMedia => t!("message-actions-reason-edit-media").into(),
        RepliesUnavailable => t!("message-actions-reason-replies").into(),
        ReactionsUnavailable => t!("message-actions-reason-reactions").into(),
        RedactUnsupported => t!("message-actions-reason-delete-unsupported").into(),
        EditUnsupported => t!("message-actions-reason-edit-unsupported").into(),
        EditAccountRequired => t!("message-actions-reason-edit-account").into(),
        EditOtherAccount => t!("message-actions-reason-edit-other-account").into(),
        RedactAccountRequired => t!("message-actions-reason-delete-account").into(),
        RedactOtherAccount => t!("message-actions-reason-delete-other-account").into(),
        TextLength { characters, bytes } => t!(
            "message-actions-reason-text-length",
            characters = characters,
            bytes = bytes
        ),
        TextLineLimit => t!("message-actions-reason-text-line-limit").into(),
        TextMultilineUnsupported { bytes } => {
            t!(
                "message-actions-reason-text-multiline-unsupported",
                bytes = bytes
            )
        }
        TextMultilineBytes { bytes } => {
            t!("message-actions-reason-text-multiline-bytes", bytes = bytes)
        }
        TextMultilineLines { lines } => {
            t!("message-actions-reason-text-multiline-lines", lines = lines)
        }
        Party(PartyState::Provisioning) => t!("message-actions-reason-party-provisioning").into(),
        Party(PartyState::Recovering) => t!("message-actions-reason-party-recovering").into(),
        Party(PartyState::Archived) => t!("message-actions-reason-party-archived").into(),
        Party(_) => t!("message-actions-reason-party-inactive").into(),
    }
}

// ---- The box ----

fn session(cx: &App) -> Option<Entity<Session>> {
    cx.try_global::<Actions>()?.session.as_ref()?.upgrade()
}

fn composer(cx: &App) -> Option<Entity<ComposerView>> {
    cx.try_global::<Actions>()?.composer.as_ref()?.upgrade()
}

/// What the box is doing in `conversation`, if anything special.
pub fn mode(conversation: &str, cx: &App) -> Option<Mode> {
    cx.try_global::<Actions>()?.modes.get(conversation).cloned()
}

fn set_mode(conversation: &str, mode: Option<Mode>, cx: &mut App) {
    let modes = &mut cx.global_mut::<Actions>().modes;
    match mode {
        Some(mode) => modes.insert(conversation.to_string(), mode),
        None => modes.remove(conversation),
    };
    cx.refresh_windows();
}

fn focus_box(window: &mut Window, cx: &mut App) {
    if let Some(composer) = composer(cx) {
        let input = composer.read(cx).input.clone();
        input.update(cx, |state, cx| state.focus(window, cx));
    }
}

fn set_box(text: &str, window: &mut Window, cx: &mut App) {
    if let Some(composer) = composer(cx) {
        composer.update(cx, |c, cx| c.set_text(text, window, cx));
        // Carry on typing after the text, not before it.
        let input = composer.read(cx).input.clone();
        let end = text.len();
        input.update(cx, |state, cx| state.set_selected_range(end..end, cx));
    }
}

/// What the message box holds now.
pub fn box_text(cx: &App) -> String {
    composer(cx)
        .map(|c| c.read(cx).text(cx))
        .unwrap_or_default()
}

/// Reply to the message `ctx` names with the next message sent.
pub fn start_reply(ctx: &MessageCtx, window: &mut Window, cx: &mut App) {
    let Some((nick, text)) = with_message(ctx, cx, |w, m, a| {
        let network = a.network.map_or("", |n| n.id.as_str());
        (
            w.display_name(&m.sender_id, network),
            format::parse(&m.text).text,
        )
    }) else {
        return;
    };
    if let Some(Mode::Edit { draft, .. }) = mode(&ctx.conversation, cx) {
        set_box(&draft, window, cx);
    }
    set_mode(
        &ctx.conversation,
        Some(Mode::Reply {
            message: ctx.message.clone(),
            nick,
            text,
        }),
        cx,
    );
    focus_box(window, cx);
}

/// Put the text of your message `message` in the box to edit it.
pub fn start_edit(conversation: &str, message: &str, window: &mut Window, cx: &mut App) {
    let Some(session) = session(cx) else { return };
    let text = {
        let s = session.read(cx);
        let Some(client) = s.client() else { return };
        let w = client.read();
        let Some(m) = w.message(conversation, message) else {
            return;
        };
        format::parse(&m.text).text
    };
    let draft = match mode(conversation, cx) {
        Some(Mode::Edit { draft, .. }) => draft,
        _ => box_text(cx),
    };
    set_mode(
        conversation,
        Some(Mode::Edit {
            message: message.to_string(),
            draft,
        }),
        cx,
    );
    set_box(&text, window, cx);
    focus_box(window, cx);
}

/// Stop replying or editing; an edit gives the box its earlier text back.
pub fn cancel(conversation: &str, window: &mut Window, cx: &mut App) {
    if let Some(Mode::Edit { draft, .. }) = mode(conversation, cx) {
        set_box(&draft, window, cx);
    }
    set_mode(conversation, None, cx);
}

/// Take the reply target as a file-only send begins.
pub fn take_reply(conversation: &str, cx: &mut App) -> Option<String> {
    let Mode::Reply { message, .. } = mode(conversation, cx)? else {
        return None;
    };
    set_mode(conversation, None, cx);
    Some(message)
}

/// Your newest message in `conversation` that you can edit now.
fn latest_editable(w: &Workspace, conversation: &str) -> Option<String> {
    let allowed = Allowed::of(w, conversation, w.connection == Some(Connection::Live));
    let network = allowed.network?.id.clone();
    w.timelines
        .get(conversation)?
        .entries
        .iter()
        .rev()
        .map(|e| &e.message)
        .find(|m| {
            w.is_self(&m.sender_id, &network)
                && matches!(m.kind, MessageKind::Message | MessageKind::Action)
                && allowed.action_reason(m, MessageAction::Edit).is_none()
        })
        .map(|m| m.id.clone())
}

/// Up in an empty box edits your newest message; Escape stops replying or editing.
fn on_key(event: &KeystrokeEvent, window: &mut Window, cx: &mut App) {
    let key = &event.keystroke;
    if key.modifiers.modified() || !matches!(key.key.as_str(), "up" | "escape") {
        return;
    }
    let (Some(composer), Some(session)) = (composer(cx), session(cx)) else {
        return;
    };
    let focused = composer
        .read(cx)
        .input
        .read(cx)
        .focus_handle(cx)
        .is_focused(window);
    let Some(conversation) = session.read(cx).conversation.clone() else {
        return;
    };
    if !focused {
        return;
    }
    match key.key.as_str() {
        "up" if composer.read(cx).text(cx).is_empty() => {
            let latest = session
                .read(cx)
                .client()
                .and_then(|client| latest_editable(&client.read(), &conversation));
            if let Some(message) = latest {
                start_edit(&conversation, &message, window, cx);
                cx.stop_propagation();
            }
        }
        "escape" if mode(&conversation, cx).is_some() => {
            cancel(&conversation, window, cx);
            cx.stop_propagation();
        }
        _ => {}
    }
}

/// Sends a reply, or saves an edit, in place of a plain send.
fn send_filter(ctx: &ComposerCtx, text: String, window: &mut Window, cx: &mut App) -> SendOutcome {
    match mode(&ctx.conversation, cx) {
        None => SendOutcome::Continue(text),
        Some(Mode::Reply { message, .. }) => {
            let Some(client) = ctx.session.read(cx).client().cloned() else {
                return SendOutcome::Keep;
            };
            set_mode(&ctx.conversation, None, cx);
            send_reply(&client, &ctx.conversation, &text, &message);
            SendOutcome::Sent(text)
        }
        Some(Mode::Edit { message, draft }) => {
            let text = text.trim().to_string();
            if text.is_empty() {
                return SendOutcome::Keep;
            }
            set_mode(&ctx.conversation, None, cx);
            save_edit(ctx, &message, text, window, cx);
            // The box is cleared once this filter returns; the draft comes back after.
            window.defer(cx, move |window, cx| set_box(&draft, window, cx));
            SendOutcome::Handled
        }
    }
}

fn send_reply(client: &ircord_client::Client, conversation: &str, text: &str, reply_to: &str) {
    let text = text.trim_end_matches(['\n', '\r']);
    if text.trim().is_empty() {
        return;
    }
    let (action, text) = match text.strip_prefix("/me ") {
        Some(rest) => (true, rest.to_string()),
        None => (
            false,
            text.strip_prefix("//")
                .map_or(text.to_string(), |t| format!("/{t}")),
        ),
    };
    let pending = PendingSend {
        client_id: random_id(),
        conversation_id: conversation.to_string(),
        text: text.clone(),
        action,
        state: SendState::Sending,
        created_ms: ircord_client::events::now_ms(),
        reply_to_id: Some(reply_to.to_string()),
        ..Default::default()
    };
    client.typing(conversation, false);
    client.change(|w| w.queue_send(pending.clone()));
    let body = c::MessageInputBody {
        client_id: pending.client_id.clone(),
        text,
        kind: Some(if action {
            c::MessageInputBodyKind::Action
        } else {
            c::MessageInputBodyKind::Message
        }),
        reply_to_id: Some(reply_to.to_string()),
        ..Default::default()
    };
    let (id, client_id) = (conversation.to_string(), pending.client_id);
    client.call(
        move |api| async move { api.send_message(&id, &body).await },
        move |w, result| match result {
            Ok(response) => w.send_answered(&response),
            Err(e) => w.fail_send(&client_id, &e.to_string()).unwrap_or_default(),
        },
    );
}

fn save_edit(ctx: &ComposerCtx, message: &str, text: String, window: &mut Window, cx: &mut App) {
    let Some(client) = ctx.session.read(cx).client().cloned() else {
        return;
    };
    let unchanged = client
        .read()
        .message(&ctx.conversation, message)
        .is_some_and(|m| m.text == text || format::parse(&m.text).text == text);
    if unchanged {
        return;
    }
    let (conversation, id) = (ctx.conversation.clone(), message.to_string());
    let sent = text.clone();
    let task =
        client.request(move |api| async move { api.edit_message(&conversation, &id, &sent).await });
    let handle = client.handle().clone();
    let (conversation, message) = (ctx.conversation.clone(), message.to_string());
    window
        .spawn(cx, async move |cx| {
            let result = task
                .await
                .map_err(|e| e.to_string())
                .and_then(|r| r.map_err(|e| e.to_string()));
            match result {
                Ok(answer) => handle.change(|w| w.message_answered(answer.message)),
                Err(e) => {
                    handle.change(|_| Changes {
                        notices: vec![e],
                        ..Default::default()
                    });
                    // Give the edit back, unless the box has moved on.
                    let _ = cx.update(|window, cx| {
                        if box_text(cx).is_empty() && mode(&conversation, cx).is_none() {
                            start_edit(&conversation, &message, window, cx);
                            set_box(&text, window, cx);
                        }
                    });
                }
            }
        })
        .detach();
}

// ---- Reactions ----

/// Add your reaction `emoji` to a message, or take it back.
pub fn react(
    session: &Entity<Session>,
    conversation: &str,
    message: &str,
    emoji: &str,
    window: &mut Window,
    cx: &mut App,
) {
    let ctx = MessageCtx {
        session: session.clone(),
        conversation: conversation.to_string(),
        message: message.to_string(),
        own: false,
    };
    if let Some(why) = reason(&ctx, MessageAction::Reactions, cx) {
        return ui::toast::info(window, cx, why);
    }
    let Some(client) = session.read(cx).client() else {
        return;
    };
    let (conversation, message, emoji) = (
        conversation.to_string(),
        message.to_string(),
        emoji.to_string(),
    );
    client.call(
        move |api| async move { api.toggle_reaction(&conversation, &message, &emoji).await },
        |w, result| match result {
            Ok(m) => w.message_answered(m),
            Err(e) => Changes {
                notices: vec![e.to_string()],
                ..Default::default()
            },
        },
    );
}

/// The emoji you reacted with on this message.
pub fn my_reactions(ctx: &MessageCtx, cx: &App) -> Vec<String> {
    with_message(ctx, cx, |w, m, a| {
        let network = a.network.map_or("", |n| n.id.as_str());
        m.reactions
            .iter()
            .filter(|r| r.member_ids.iter().any(|id| w.is_self(id, network)))
            .map(|r| r.emoji.clone())
            .collect()
    })
    .unwrap_or_default()
}

fn open_quick_reactions(ctx: MessageCtx, window: &mut Window, cx: &mut App) {
    ui::dialog::open(
        window,
        cx,
        t!("message-actions-react-title"),
        move |dialog, _, cx| {
            let theme = crate::theme::get(cx).clone();
            let mut row = div()
                .id("quick-reactions")
                .test_support()
                .role(Role::Toolbar)
                .aria_label(t!("message-actions-react-title"))
                .flex()
                .gap_2();
            for (i, emoji) in QUICK_REACTIONS.iter().enumerate() {
                let ctx = ctx.clone();
                row = row.child(
                    div()
                        .id(ElementId::Name(format!("quick-reaction-{i}").into()))
                        .test_support()
                        .role(Role::Button)
                        .reachable(&theme)
                        .aria_label(t!("message-actions-react-with", emoji = emoji))
                        .tooltip(ui::tooltip::text(t!(
                            "message-actions-react-with",
                            emoji = emoji
                        )))
                        .p_2()
                        .rounded_md()
                        .text_xl()
                        .cursor_pointer()
                        .hover({
                            let hover = theme.hover;
                            move |d| d.bg(hover)
                        })
                        .child(*emoji)
                        .on_click(move |_, window, cx| {
                            ui::dialog::close(window, cx);
                            react(
                                &ctx.session,
                                &ctx.conversation,
                                &ctx.message,
                                emoji,
                                window,
                                cx,
                            );
                        }),
                );
            }
            dialog.child(row)
        },
    );
}

// ---- Copy, versions, delete ----

/// A link to the message that opens it for other readers too: the shared
/// ID when the network gave the message one.
fn message_link(ctx: &MessageCtx, cx: &App) -> Option<String> {
    let origin = ctx.session.read(cx).server()?.origin();
    with_message(ctx, cx, |_, m, a| {
        let conversation = a.conversation?;
        let kind = if conversation.kind == ConversationKind::Channel && conversation.party.is_none()
        {
            "channel"
        } else {
            "dm"
        };
        let id = match m.upstream_id.as_deref() {
            Some(up) if !up.is_empty() && !up.starts_with('_') => {
                format!("{}:{up}", conversation.network_id)
            }
            _ => m.id.clone(),
        };
        let mut url = url::Url::parse(&origin).ok()?;
        url.path_segments_mut()
            .ok()?
            .clear()
            .push(kind)
            .push(&conversation.id);
        url.query_pairs_mut().append_pair("m", &id);
        Some(url.to_string())
    })
    .flatten()
}

/// Every version before the current text, oldest first: `(time, text, original)`.
fn earlier_versions(m: &Message) -> Vec<(String, String, bool)> {
    let edits = m.edits.as_deref().unwrap_or_default();
    if edits.is_empty() || m.deleted == Some(true) {
        return Vec::new();
    }
    let mut versions = vec![(
        m.created_at.clone(),
        m.original_text.clone().unwrap_or_default(),
        true,
    )];
    versions.extend(
        edits[..edits.len() - 1]
            .iter()
            .map(|e| (e.at.clone(), e.text.clone(), false)),
    );
    versions
}

fn local_time(at: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(at)
        .map(|t| {
            t.with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M")
                .to_string()
        })
        .unwrap_or_else(|_| at.to_string())
}

fn open_versions(
    message: String,
    versions: Vec<(String, String, bool)>,
    window: &mut Window,
    cx: &mut App,
) {
    ui::dialog::open(
        window,
        cx,
        t!("message-actions-versions-title"),
        move |dialog, _, cx| {
            let theme = crate::theme::get(cx).clone();
            let mut list = div()
                .id(ElementId::Name(format!("versions-{message}").into()))
                .test_support()
                .role(Role::List)
                .aria_label(t!("message-actions-versions-title"))
                .flex()
                .flex_col()
                .gap_2();
            for (i, (at, text, original)) in versions.iter().enumerate() {
                let when = if *original {
                    t!("message-actions-original-at", time = local_time(at))
                } else {
                    t!("message-actions-edited-at", time = local_time(at))
                };
                list = list.child(
                    div()
                        .id(ElementId::Name(format!("version-{i}").into()))
                        .test_support()
                        .role(Role::ListItem)
                        .aria_label(format::parse(text).text)
                        .pl_2()
                        .border_l_2()
                        .border_color(theme.selected)
                        .child(div().text_xs().text_color(theme.muted).child(when))
                        .child(div().text_color(theme.text).child(format::parse(text).text)),
                );
            }
            dialog.child(list)
        },
    );
}

fn confirm_delete(ctx: MessageCtx, window: &mut Window, cx: &mut App) {
    let files = with_message(&ctx, cx, |_, m, _| {
        m.attachment.is_some() || m.attachments.as_ref().is_some_and(|a| !a.is_empty())
    })
    .unwrap_or(false);
    ui::confirm::confirm(
        window,
        cx,
        ui::confirm::Confirm {
            title: t!("message-actions-delete-title").into(),
            body: if files {
                t!("message-actions-delete-attachment-body").into()
            } else {
                t!("message-actions-delete-body").into()
            },
            ok: t!("message-actions-delete-confirm").into(),
            danger: true,
        },
        move |_, cx| {
            let Some(client) = ctx.session.read(cx).client() else {
                return;
            };
            let (conversation, message) = (ctx.conversation.clone(), ctx.message.clone());
            client.call(
                move |api| async move {
                    api.delete_message(&conversation, &message, None, false)
                        .await
                },
                |w, result| match result {
                    Ok(answer) => w.message_answered(answer.message),
                    Err(e) => Changes {
                        notices: vec![e.to_string()],
                        ..Default::default()
                    },
                },
            );
        },
    );
}

// ---- Failed sends and the outbox ----

/// The buttons under a failed send in the timeline, as the web client's
/// delivery actions draw them: Retry, Move to draft (only for text the box
/// can hold: not a reply, a GIF or uploads), Copy text and Remove from
/// outbox (after asking).
pub fn failed_send_actions(p: &PendingSend) -> Vec<AnyElement> {
    let id = p.client_id.clone();
    let retry = {
        let id = id.clone();
        Button::new(ElementId::Name(format!("retry-{id}").into()))
            .small()
            .label(t!("message-actions-retry"))
            .on_click(move |_, _, cx| {
                if let Some(client) = session(cx).and_then(|s| s.read(cx).client().cloned()) {
                    client.retry(&id);
                }
            })
    };
    let to_draft = {
        let id = id.clone();
        Button::new(ElementId::Name(format!("failed-send-to-box-{id}").into()))
            .small()
            .ghost()
            .label(t!("message-actions-to-draft"))
            .on_click(move |_, window, cx| discard(&id, true, window, cx))
    };
    let copy = {
        let text = format::markdown::irc_to_markdown(&p.text);
        Button::new(ElementId::Name(format!("failed-send-copy-{id}").into()))
            .small()
            .ghost()
            .label(t!("message-actions-copy-send"))
            .on_click(move |_, window, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
                ui::toast::success(window, cx, t!("message-actions-send-copied"));
            })
    };
    let remove = Button::new(ElementId::Name(format!("failed-send-discard-{id}").into()))
        .small()
        .ghost()
        .label(t!("message-actions-remove-send"))
        .on_click(move |_, window, cx| {
            let id = id.clone();
            ui::confirm::confirm(
                window,
                cx,
                ui::confirm::Confirm {
                    title: t!("message-actions-remove-send-title").into(),
                    body: t!("message-actions-remove-send-body").into(),
                    ok: t!("message-actions-remove-send").into(),
                    danger: true,
                },
                move |window, cx| discard(&id, false, window, cx),
            );
        });
    let mut buttons = vec![retry.into_any_element()];
    if p.movable() {
        buttons.push(to_draft.into_any_element());
    }
    buttons.push(copy.into_any_element());
    buttons.push(remove.into_any_element());
    buttons
}

/// Give up on a failed send; with `to_box`, put its text back in the box.
pub fn discard(client_id: &str, to_box: bool, window: &mut Window, cx: &mut App) {
    let Some(session) = session(cx) else { return };
    let Some(client) = session.read(cx).client().cloned() else {
        return;
    };
    let Some(pending) = client.read().pending(client_id).cloned() else {
        return;
    };
    if to_box {
        // Back as typed, the way the web moves a failed send to the draft.
        let body = format::markdown::irc_to_markdown(&pending.text);
        let text = if pending.action {
            format!("/me {body}")
        } else if body.starts_with('/') {
            format!("/{body}")
        } else {
            body
        };
        let current = box_text(cx);
        let next = if current.is_empty() {
            text
        } else {
            format!("{current}\n{text}")
        };
        set_box(&next, window, cx);
        focus_box(window, cx);
    }
    client.change(|w| {
        w.discard_send(client_id)
            .map(|(_, c)| c)
            .unwrap_or_default()
    });
    let id = client_id.to_string();
    client.call(
        move |api| async move { api.discard_send(&id).await },
        |_, result| match result {
            // The server never kept it: nothing to discard there.
            Ok(()) | Err(ircord_client::api::ApiError::Http { status: 404, .. }) => {
                Changes::default()
            }
            Err(e) => Changes {
                notices: vec![e.to_string()],
                ..Default::default()
            },
        },
    );
}

/// Read the sends the server still holds and put them back in the outbox.
fn restore_outbox(client: &ircord_client::Client) {
    client.call(
        |api| async move { api.outbox_all().await },
        |w, result| match result {
            Ok(entries) => w.restore_outbox(entries),
            Err(e) => {
                log::warn!("outbox: {e}");
                Changes::default()
            }
        },
    );
}

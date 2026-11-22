//! Moderation (package CH2), as the web client's member menu and
//! `ModerationDialog.vue` do it: op, voice, kick (with a reason) and ban
//! from a member's menu when your rank allows; deleting someone else's
//! message as a channel operator, or as a server owner with a reason and the
//! choice to delete its uploaded files too; and deleting someone's recent
//! messages in a channel, counted first, then deleted by the server one at
//! a time while the app follows the job.

use std::rc::Rc;
use std::time::Duration;

use gpui_kit::TestSupportExt;
use gpui_kit::component::Disableable;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::prelude::*;
use gpui_kit::*;
use ircord_client::Client;
use ircord_client::channel_modes as modes;
use ircord_client::contract::{self as c, MessageKind};
use ircord_client::features::{Context as Allowed, MessageAction};
use ircord_client::store::Connection;

use crate::features::channel_settings::{choices, here, hint, send};
use crate::menu_order;
use crate::registry::{self, MemberCtx, MenuItem, MessageCtx};
use crate::session::Session;
use crate::ui::icon::IconName;
use crate::{theme, ui};

/// How often a recent-messages deletion is asked how far it got.
const FOLLOW: Duration = Duration::from_millis(1500);

pub fn init(cx: &mut App) {
    registry::register(cx, |r| {
        let m = &mut r.member_menu;
        m.add(
            MenuItem::new(
                "moderation-op",
                t!("channel-settings-menu-op"),
                |ctx: &MemberCtx, w, cx| status(ctx, 'o', true, w, cx),
            )
            .icon(IconName::Star)
            .order(menu_order::member::OP)
            .when(|ctx: &MemberCtx, cx| target(ctx, cx).is_some_and(|t| t.op && !t.has('@'))),
        );
        m.add(
            MenuItem::new(
                "moderation-deop",
                t!("channel-settings-menu-deop"),
                |ctx: &MemberCtx, w, cx| status(ctx, 'o', false, w, cx),
            )
            .icon(IconName::StarOff)
            .order(menu_order::member::OP)
            .when(|ctx: &MemberCtx, cx| target(ctx, cx).is_some_and(|t| t.op && t.has('@'))),
        );
        m.add(
            MenuItem::new(
                "moderation-voice",
                t!("channel-settings-menu-voice"),
                |ctx: &MemberCtx, w, cx| status(ctx, 'v', true, w, cx),
            )
            .icon(IconName::Mic)
            .order(menu_order::member::VOICE)
            .when(|ctx: &MemberCtx, cx| target(ctx, cx).is_some_and(|t| t.voice && !t.has('+'))),
        );
        m.add(
            MenuItem::new(
                "moderation-devoice",
                t!("channel-settings-menu-devoice"),
                |ctx: &MemberCtx, w, cx| status(ctx, 'v', false, w, cx),
            )
            .icon(IconName::Minus)
            .order(menu_order::member::VOICE)
            .when(|ctx: &MemberCtx, cx| target(ctx, cx).is_some_and(|t| t.voice && t.has('+'))),
        );
        m.add(
            MenuItem::new("moderation-kick", t!("channel-settings-menu-kick"), kick)
                .icon(IconName::Close)
                .danger()
                .order(menu_order::member::KICK)
                .when(|ctx: &MemberCtx, cx| target(ctx, cx).is_some_and(|t| t.kick)),
        );
        m.add(
            MenuItem::new("moderation-ban", t!("channel-settings-menu-ban"), ban)
                .icon(IconName::Ban)
                .danger()
                .order(menu_order::member::BAN)
                .when(|ctx: &MemberCtx, cx| target(ctx, cx).is_some_and(|t| t.ban)),
        );
        m.add(
            MenuItem::new(
                "moderation-delete-recent",
                t!("channel-settings-menu-delete-recent"),
                |ctx: &MemberCtx, window, cx| {
                    if let Some(t) = target(ctx, cx) {
                        open_recent(&ctx.session, t, window, cx);
                    }
                },
            )
            .icon(IconName::Delete)
            .danger()
            .order(menu_order::member::DELETE_RECENT)
            .when(|ctx: &MemberCtx, cx| target(ctx, cx).is_some_and(|t| t.delete)),
        );

        r.message_menu.add(
            MenuItem::new(
                "moderation-delete",
                t!("channel-settings-menu-delete-message"),
                |ctx: &MessageCtx, window: &mut Window, cx: &mut App| {
                    if let Some(why) = message_reason(ctx, cx) {
                        return ui::toast::info(window, cx, why);
                    }
                    delete_message(ctx.clone(), window, cx);
                },
            )
            .icon(IconName::Delete)
            .order(menu_order::message::DELETE_OTHERS)
            .danger()
            .when(|ctx: &MessageCtx, cx: &App| !ctx.own && offered(ctx, cx))
            .unavailable_when(message_reason),
        );
    });
}

// ---- The member menu ----

/// Someone else in a channel, and what you may do to them there.
#[derive(Clone)]
struct Target {
    conversation: String,
    channel: String,
    member: String,
    nick: String,
    /// Their channel modes (prefixes).
    modes: String,
    op: bool,
    voice: bool,
    kick: bool,
    ban: bool,
    /// You may delete their recent messages here.
    delete: bool,
    /// …as a server owner, who gives a reason.
    owner: bool,
}

impl Target {
    fn has(&self, prefix: char) -> bool {
        self.modes.contains(prefix)
    }
}

fn target(ctx: &MemberCtx, cx: &App) -> Option<Target> {
    let conversation = ctx.conversation.as_deref()?;
    let h = here(&ctx.session, conversation, cx)?;
    let client = ctx.session.read(cx).client()?;
    let w = client.read();
    let member = w.members.get(&ctx.member)?;
    if w.is_self(&ctx.member, h.network_id()) {
        return None;
    }
    let live = w.connection == Some(Connection::Live);
    let allowed = Allowed::of(&w, conversation, live);
    let redact = h.network.as_ref().is_some_and(|n| n.features.redact);
    let moderation = allowed.moderation();
    Some(Target {
        conversation: conversation.to_string(),
        channel: h.target(),
        member: ctx.member.clone(),
        nick: member.nick.clone(),
        modes: h
            .conversation
            .member_modes
            .get(&ctx.member)
            .cloned()
            .unwrap_or_default(),
        op: h.rights.op,
        voice: h.rights.voice,
        kick: h.rights.kick,
        ban: h.rights.ban,
        delete: redact && moderation.is_some(),
        owner: allowed.owner_moderation(),
    })
}

fn status(ctx: &MemberCtx, mode: char, give: bool, window: &mut Window, cx: &mut App) {
    let Some(t) = target(ctx, cx) else {
        return;
    };
    send(
        &ctx.session,
        &t.conversation,
        modes::status_line(&t.channel, mode, &t.nick, give),
        window,
        cx,
    );
}

fn kick(ctx: &MemberCtx, window: &mut Window, cx: &mut App) {
    let Some(t) = target(ctx, cx) else {
        return;
    };
    let session = ctx.session.clone();
    ui::prompt::prompt(
        window,
        cx,
        ui::prompt::Prompt {
            title: t!(
                "channel-settings-kick-title",
                nick = t.nick.clone(),
                channel = t.channel.clone()
            )
            .into(),
            label: t!("channel-settings-kick-reason").into(),
            initial: String::new(),
            ok: t!("channel-settings-kick").into(),
        },
        move |reason, window, cx| {
            send(
                &session,
                &t.conversation,
                modes::kick_line(&t.channel, &t.nick, &reason),
                window,
                cx,
            );
        },
    );
}

fn ban(ctx: &MemberCtx, window: &mut Window, cx: &mut App) {
    let Some(t) = target(ctx, cx) else {
        return;
    };
    let session = ctx.session.clone();
    ui::confirm::confirm(
        window,
        cx,
        ui::confirm::Confirm {
            title: t!(
                "channel-settings-ban-title",
                nick = t.nick.clone(),
                channel = t.channel.clone()
            )
            .into(),
            body: t!("channel-settings-ban-body", nick = t.nick.clone()).into(),
            ok: t!("channel-settings-ban").into(),
            danger: true,
        },
        move |window, cx| {
            send(
                &session,
                &t.conversation,
                modes::ban_line(&t.channel, &t.nick),
                window,
                cx,
            );
        },
    );
}

// ---- Deleting recent messages ----

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Range {
    Hour,
    Day,
    Week,
}

impl Range {
    fn wire(self) -> &'static str {
        match self {
            Range::Hour => "1h",
            Range::Day => "24h",
            Range::Week => "7d",
        }
    }

    fn body(self) -> c::DeleteRecentMessagesRequestRange {
        match self {
            Range::Hour => c::DeleteRecentMessagesRequestRange::V1h,
            Range::Day => c::DeleteRecentMessagesRequestRange::V24h,
            Range::Week => c::DeleteRecentMessagesRequestRange::V7d,
        }
    }
}

enum Count {
    Counting,
    Ready(c::RecentMessagesPreview),
    Failed(SharedString),
}

pub struct RecentView {
    session: Entity<Session>,
    target: Target,
    range: Range,
    round: u64,
    count: Count,
    reason: Entity<InputState>,
    starting: bool,
}

fn open_recent(session: &Entity<Session>, target: Target, window: &mut Window, cx: &mut App) {
    let title = t!("channel-settings-recent-title", nick = target.nick.clone());
    let view = cx.new(|cx| RecentView::new(session.clone(), target, window, cx));
    ui::dialog::open(window, cx, title, move |dialog, _, _| {
        dialog
            .w(px(480.))
            .on_ok(|_, _, _| false)
            .child(view.clone())
    });
}

impl RecentView {
    fn new(
        session: Entity<Session>,
        target: Target,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let reason = cx.new(|cx| InputState::new(window, cx));
        let mut view = Self {
            session,
            target,
            range: Range::Day,
            round: 0,
            count: Count::Counting,
            reason,
            starting: false,
        };
        view.count(cx);
        view
    }

    fn client(&self, cx: &App) -> Option<Rc<Client>> {
        self.session.read(cx).client().cloned()
    }

    /// Count the member's messages in the range, deleting nothing.
    fn count(&mut self, cx: &mut Context<Self>) {
        let Some(client) = self.client(cx) else {
            return;
        };
        self.round += 1;
        let round = self.round;
        self.count = Count::Counting;
        let (conversation, member, range) = (
            self.target.conversation.clone(),
            self.target.member.clone(),
            self.range,
        );
        let task = client.request(move |api| async move {
            api.recent_messages(&conversation, &member, range.wire())
                .await
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                if this.round != round {
                    return;
                }
                this.count = match result {
                    Ok(Ok(p)) => Count::Ready(p),
                    Ok(Err(e)) => Count::Failed(e.to_string().into()),
                    Err(_) => Count::Failed(t!("channel-settings-recent-count-failed").into()),
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn set_range(&mut self, range: Range, cx: &mut Context<Self>) {
        if range != self.range {
            self.range = range;
            self.count(cx);
        }
        cx.notify();
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Count::Ready(preview) = &self.count else {
            return;
        };
        if preview.count == 0 || self.starting {
            return;
        }
        let Some(client) = self.client(cx) else {
            return;
        };
        self.starting = true;
        cx.notify();
        let reason = self.reason.read(cx).value().trim().to_string();
        let body = c::DeleteRecentMessagesRequest {
            range: self.range.body(),
            reason: (self.target.owner && !reason.is_empty()).then_some(reason),
        };
        let (conversation, member, nick) = (
            self.target.conversation.clone(),
            self.target.member.clone(),
            self.target.nick.clone(),
        );
        let task = client.request(move |api| async move {
            api.delete_recent_messages(&conversation, &member, &body)
                .await
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| match result {
                Ok(Ok(job)) => {
                    ui::dialog::close(window, cx);
                    if job.total == 0 {
                        return ui::toast::info(
                            window,
                            cx,
                            t!("channel-settings-recent-nothing", nick = nick.clone()),
                        );
                    }
                    ui::toast::info(
                        window,
                        cx,
                        if job.total == 1 {
                            t!("channel-settings-recent-deleting-one", nick = nick.clone())
                        } else {
                            t!(
                                "channel-settings-recent-deleting",
                                count = job.total,
                                nick = nick.clone()
                            )
                        },
                    );
                    if let Some(client) = this.client(cx) {
                        follow(client, job.id, nick.clone(), window, cx);
                    }
                }
                Ok(Err(e)) => {
                    this.starting = false;
                    ui::toast::error(window, cx, e.to_string());
                    cx.notify();
                }
                Err(e) => {
                    this.starting = false;
                    ui::toast::error(window, cx, e.to_string());
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }
}

/// Ask the job how far it got until it is done, then say what happened.
fn follow(client: Rc<Client>, job: String, nick: String, window: &mut Window, cx: &mut App) {
    window
        .spawn(cx, async move |acx| {
            loop {
                let id = job.clone();
                // The wait runs on the client's runtime, beside the request.
                let result = client
                    .request(move |api| async move {
                        tokio::time::sleep(FOLLOW).await;
                        api.recent_messages_deletion(&id).await
                    })
                    .await;
                let job = match result {
                    Ok(Ok(job)) => job,
                    _ => {
                        let nick = nick.clone();
                        let _ = acx.update(|window, cx| {
                            ui::toast::error(
                                window,
                                cx,
                                t!("channel-settings-recent-lost", nick = nick),
                            )
                        });
                        return;
                    }
                };
                if job.state != c::PurgeJobState::Done {
                    continue;
                }
                let text = job_summary(&job, &nick);
                let _ = acx.update(|window, cx| ui::toast::success(window, cx, text));
                return;
            }
        })
        .detach();
}

/// What a finished deletion did, in one line.
fn job_summary(job: &c::PurgeJob, nick: &str) -> String {
    let mut parts = vec![if job.deleted == job.total {
        if job.deleted == 1 {
            t!("channel-settings-recent-deleted-one", nick = nick)
        } else {
            t!(
                "channel-settings-recent-deleted",
                count = job.deleted,
                nick = nick
            )
        }
    } else {
        t!(
            "channel-settings-recent-deleted-some",
            deleted = job.deleted,
            total = job.total,
            nick = nick
        )
    }];
    if job.too_old > 0 {
        parts.push(if job.too_old == 1 {
            t!("channel-settings-recent-too-old-one").to_string()
        } else {
            t!("channel-settings-recent-too-old", count = job.too_old)
        });
    }
    if job.failed > 0 {
        parts.push(if job.failed == 1 {
            t!("channel-settings-recent-failed-one").to_string()
        } else {
            t!("channel-settings-recent-failed", count = job.failed)
        });
    }
    parts.join(" ")
}

impl Render for RecentView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme::get(cx).clone();
        let this = cx.entity().downgrade();
        let range = choices(
            "channel-settings-recent-range",
            t!("channel-settings-recent-range"),
            vec![
                (Range::Hour, t!("channel-settings-recent-hour").into()),
                (Range::Day, t!("channel-settings-recent-day").into()),
                (Range::Week, t!("channel-settings-recent-week").into()),
            ],
            self.range,
            !self.starting,
            move |range, _, cx| {
                this.update(cx, |v, cx| v.set_range(range, cx)).ok();
            },
        );
        let (status, count): (SharedString, i64) = match &self.count {
            Count::Counting => (t!("channel-settings-recent-counting").into(), 0),
            Count::Failed(e) => (e.clone(), 0),
            Count::Ready(p) if p.count == 0 => (
                t!(
                    "channel-settings-recent-none",
                    nick = self.target.nick.clone()
                )
                .into(),
                0,
            ),
            Count::Ready(p) => (
                if p.count == 1 {
                    t!(
                        "channel-settings-recent-will-delete-one",
                        channel = self.target.channel.clone()
                    )
                } else {
                    t!(
                        "channel-settings-recent-will-delete",
                        count = p.count,
                        channel = self.target.channel.clone()
                    )
                }
                .into(),
                p.count,
            ),
        };
        let mut notes = Vec::new();
        if let Count::Ready(p) = &self.count {
            if p.count >= p.limit && p.limit > 0 {
                notes.push(t!("channel-settings-recent-limit", count = p.limit));
            }
            if p.partial == Some(true) {
                notes.push(t!("channel-settings-recent-partial").to_string());
            }
        }
        div()
            .id("channel-settings-recent")
            .test_support()
            .role(Role::Form)
            .aria_label(t!(
                "channel-settings-recent-title",
                nick = self.target.nick.clone()
            ))
            .flex()
            .flex_col()
            .gap_3()
            .child(ui::field::field(
                t!("channel-settings-recent-range"),
                range,
                &theme,
            ))
            .child(
                div()
                    .id("channel-settings-recent-count")
                    .test_support()
                    .role(Role::Status)
                    .aria_label(status.clone())
                    .text_sm()
                    .when(matches!(self.count, Count::Failed(_)), |d| {
                        d.text_color(theme.danger)
                    })
                    .child(status),
            )
            .children(notes.into_iter().map(|n| hint(n, &theme)))
            .when(self.target.owner, |d| {
                d.child(ui::field::field(
                    t!("channel-settings-delete-reason"),
                    Input::new(&self.reason)
                        .id("channel-settings-recent-reason")
                        .aria_label(t!("channel-settings-delete-reason")),
                    &theme,
                ))
                .child(hint(t!("channel-settings-delete-reason-about"), &theme))
                .child(hint(t!("channel-settings-delete-recorded"), &theme))
            })
            .child(hint(t!("channel-settings-recent-undo"), &theme))
            .child(
                div().flex().justify_end().child(
                    Button::new("channel-settings-recent-delete")
                        .danger()
                        .label(match count {
                            0 => t!("channel-settings-recent-delete-none").to_string(),
                            1 => t!("channel-settings-recent-delete-one").to_string(),
                            n => t!("channel-settings-recent-delete", count = n),
                        })
                        .disabled(count == 0 || self.starting)
                        .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx))),
                ),
            )
    }
}

// ---- Deleting someone else's message ----

fn with_message<R>(
    ctx: &MessageCtx,
    cx: &App,
    f: impl FnOnce(&c::Message, &Allowed) -> R,
) -> Option<R> {
    let client = ctx.session.read(cx).client()?;
    let w = client.read();
    let message = w.message(&ctx.conversation, &ctx.message)?;
    let allowed = Allowed::of(
        &w,
        &ctx.conversation,
        w.connection == Some(Connection::Live),
    );
    Some(f(message, &allowed))
}

/// Shown on other people's chat messages where you can moderate them.
fn offered(ctx: &MessageCtx, cx: &App) -> bool {
    with_message(ctx, cx, |m, a| {
        a.network.is_some_and(|n| n.features.redact)
            && matches!(
                m.kind,
                MessageKind::Message | MessageKind::Action | MessageKind::Notice
            )
            && m.deleted != Some(true)
            && a.moderation().is_some()
    })
    .unwrap_or(false)
}

fn message_reason(ctx: &MessageCtx, cx: &App) -> Option<SharedString> {
    with_message(ctx, cx, |m, a| a.action_reason(m, MessageAction::Redact))
        .flatten()
        .map(|r| crate::features::message_actions::reason_text(&r).into())
}

fn delete_message(ctx: MessageCtx, window: &mut Window, cx: &mut App) {
    let Some((nick, files, owner)) = with_message(&ctx, cx, |m, a| {
        (
            a.workspace
                .members
                .get(&m.sender_id)
                .map(|x| x.nick.clone())
                .unwrap_or_default(),
            m.attachment.is_some() || m.attachments.as_ref().is_some_and(|x| !x.is_empty()),
            a.owner_moderation(),
        )
    }) else {
        return;
    };
    let view = cx.new(|cx| DeleteView::new(ctx, nick.clone(), files, owner, window, cx));
    ui::dialog::open(
        window,
        cx,
        t!("channel-settings-delete-title", nick = nick),
        move |dialog, _, _| {
            dialog
                .w(px(460.))
                .on_ok(|_, _, _| false)
                .child(view.clone())
        },
    );
}

pub struct DeleteView {
    ctx: MessageCtx,
    files: bool,
    owner: bool,
    reason: Entity<InputState>,
    delete_files: bool,
    busy: bool,
}

impl DeleteView {
    fn new(
        ctx: MessageCtx,
        _nick: String,
        files: bool,
        owner: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            ctx,
            files,
            owner,
            reason: cx.new(|cx| InputState::new(window, cx)),
            delete_files: false,
            busy: false,
        }
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(client) = self.ctx.session.read(cx).client().cloned() else {
            return;
        };
        if self.busy {
            return;
        }
        self.busy = true;
        let reason = self.reason.read(cx).value().trim().to_string();
        let (conversation, message) = (self.ctx.conversation.clone(), self.ctx.message.clone());
        let (owner, files, delete_files) =
            (self.owner, self.files, self.delete_files && self.owner);
        let task = client.request(move |api| async move {
            let reason = owner.then_some(reason);
            api.delete_message(&conversation, &message, reason.as_deref(), delete_files)
                .await
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| {
                this.busy = false;
                match result {
                    Ok(Ok(answer)) => {
                        if let Some(client) = this.ctx.session.read(cx).client() {
                            client.change(|w| w.message_answered(answer.message));
                        }
                        ui::dialog::close(window, cx);
                        if files {
                            ui::toast::success(
                                window,
                                cx,
                                if delete_files {
                                    t!("channel-settings-delete-files-gone")
                                } else {
                                    t!("channel-settings-delete-files-kept")
                                },
                            );
                        }
                    }
                    Ok(Err(e)) => ui::toast::error(window, cx, e.to_string()),
                    Err(e) => ui::toast::error(window, cx, e.to_string()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

impl Render for DeleteView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme::get(cx).clone();
        let body = match (self.files, self.owner) {
            (true, true) => t!("channel-settings-delete-files-owner-body"),
            (true, false) => t!("channel-settings-delete-files-body"),
            _ => t!("channel-settings-delete-body"),
        };
        div()
            .id("channel-settings-delete")
            .test_support()
            .role(Role::Form)
            .aria_label(body)
            .flex()
            .flex_col()
            .gap_3()
            .child(div().text_sm().child(body))
            .when(self.owner, |d| {
                d.child(ui::field::field(
                    t!("channel-settings-delete-reason"),
                    Input::new(&self.reason)
                        .id("channel-settings-delete-reason")
                        .aria_label(t!("channel-settings-delete-reason")),
                    &theme,
                ))
                .child(hint(t!("channel-settings-delete-reason-about"), &theme))
                .when(self.files, |d| {
                    d.child(
                        Checkbox::new("channel-settings-delete-files")
                            .checked(self.delete_files)
                            .label(t!("channel-settings-delete-files"))
                            .accessibility_label(t!("channel-settings-delete-files"))
                            .on_click(cx.listener(|this, on: &bool, _, cx| {
                                this.delete_files = *on;
                                cx.notify();
                            })),
                    )
                })
                .child(hint(t!("channel-settings-delete-recorded"), &theme))
            })
            .child(
                div().flex().justify_end().child(
                    Button::new("channel-settings-delete-confirm")
                        .danger()
                        .label(t!("channel-settings-delete-confirm"))
                        .disabled(self.busy)
                        .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx))),
                ),
            )
    }
}

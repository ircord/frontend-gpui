//! Your status and away message (package P1).
//!
//! Your name at the foot of the sidebar opens your own menu: set or clear
//! a status (an emoji and a line of text that clears after a while), go
//! away or come back, and open your profile. The line under your name
//! shows your presence and status. `/status` without arguments uses the
//! composer editor, and a complete command updates the network profile.

use crate::menu_order;
use crate::ui::focus::Reachable;
use gpui_kit::TestSupportExt;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::prelude::*;
use gpui_kit::*;
use ircord_client::commands::{View, status as command};
use ircord_client::contract::{Member, MemberStatus, NetworkStatus, ProfileUpdate};
use ircord_client::store::people::{self, Expiry, StatusError};
use std::rc::Rc;

use crate::features::commands::{ViewRequest, on_view};
use crate::features::drafts;
use crate::message::emoji;
use crate::registry::{self, MemberCtx, MenuItem, SidebarCtx, Slot};
use crate::session::Session;
use crate::theme;
use crate::ui;
use crate::ui::icon::IconName;

pub fn init(cx: &mut App) {
    registry::register(cx, |r| {
        let menu = &mut r.self_menu;
        menu.add(
            MenuItem::new(
                "status-set",
                t!("people-status-set"),
                |ctx: &SidebarCtx, window, cx| {
                    if let Some(network) = ctx.network.clone() {
                        open_dialog(&ctx.session, &network, window, cx);
                    }
                },
            )
            .icon(IconName::Heart)
            .when(|ctx, _| ctx.network.is_some())
            .order(menu_order::own::SET_STATUS),
        );
        menu.add(
            MenuItem::new(
                "status-clear",
                t!("people-status-clear"),
                |ctx: &SidebarCtx, window, cx| {
                    if let Some(network) = ctx.network.clone() {
                        save(&ctx.session, &network, people::clear_status(), window, cx);
                    }
                },
            )
            .icon(IconName::CircleX)
            .when(|ctx, cx| me_status(ctx, cx).is_some_and(|(status, _)| status.is_some()))
            .order(menu_order::own::CLEAR_STATUS),
        );
        menu.add(
            MenuItem::new(
                "status-away",
                t!("people-status-go-away"),
                |ctx: &SidebarCtx, window, cx| set_away(ctx, true, window, cx),
            )
            .icon(IconName::Moon)
            .when(|ctx, cx| me_status(ctx, cx).is_some_and(|(_, away)| !away))
            .unavailable_when(not_connected)
            .order(menu_order::own::AWAY),
        );
        menu.add(
            MenuItem::new(
                "status-back",
                t!("people-status-come-back"),
                |ctx: &SidebarCtx, window, cx| set_away(ctx, false, window, cx),
            )
            .icon(IconName::Sun)
            .when(|ctx, cx| me_status(ctx, cx).is_some_and(|(_, away)| away))
            .unavailable_when(not_connected)
            .order(menu_order::own::AWAY),
        );
        menu.add(
            MenuItem::new(
                "status-profile",
                t!("people-status-profile"),
                |ctx: &SidebarCtx, window, cx| {
                    let member = ctx
                        .session
                        .read(cx)
                        .client()
                        .map(|c| people::me(&c.read(), ctx.network.as_deref()).id.clone());
                    if let Some(member) = member {
                        let card = MemberCtx {
                            session: ctx.session.clone(),
                            member,
                            conversation: None,
                        };
                        crate::features::profile::open(&card, window, cx);
                    }
                },
            )
            .icon(IconName::CircleUser)
            .when(|ctx, _| ctx.network.is_some())
            .order(menu_order::own::VIEW_PROFILE),
        );
        r.self_line.add(Slot::new("status-line", self_line));
    });
    on_view(cx, View::Status, run_command);
}

fn run_command(request: &ViewRequest, window: &mut Window, cx: &mut App) {
    let Some(command) = command::parse(&request.args, chrono::Utc::now()) else {
        return ui::toast::info(window, cx, t!("people-status-command-usage"));
    };
    if request.network.is_empty() {
        return ui::toast::info(window, cx, t!("people-status-command-network"));
    }
    let (update, notice) = match command {
        command::Command::Edit => {
            return open_dialog(&request.session, &request.network, window, cx);
        }
        command::Command::Clear => (people::clear_status(), t!("people-status-command-cleared")),
        command::Command::Set(status) => (status.into(), t!("people-status-command-set")),
    };
    let session = request.session.clone();
    let (conversation, draft) = (request.conversation.clone(), request.draft.clone());
    update_status(
        &request.session,
        &request.network,
        update,
        window,
        cx,
        move |result, window, cx| match result {
            Ok(()) => {
                drafts::clear_if_matches(&session, &conversation, &draft, window, cx);
                ui::toast::success(window, cx, notice);
            }
            Err(error) => ui::toast::error(window, cx, error),
        },
    );
}

/// Your status label on the sidebar's network and whether you are away.
fn me_status(ctx: &SidebarCtx, cx: &App) -> Option<(Option<String>, bool)> {
    ctx.network.as_ref()?;
    let session = ctx.session.read(cx);
    let w = session.client()?.read();
    let me = people::me(&w, ctx.network.as_deref());
    Some((
        people::status_label(me, chrono::Utc::now().timestamp_millis()),
        me.status == MemberStatus::Away,
    ))
}

fn not_connected(ctx: &SidebarCtx, cx: &App) -> Option<SharedString> {
    let session = ctx.session.read(cx);
    let w = session.client()?.read();
    let up = ctx
        .network
        .as_deref()
        .and_then(|n| w.network(n))
        .is_some_and(|n| n.status == NetworkStatus::Connected);
    (!up).then(|| t!("people-status-offline-reason").into())
}

/// Show the current status in the profile chip, with presence as the fallback.
fn self_line(ctx: &SidebarCtx, _window: &mut Window, cx: &mut App) -> Option<AnyElement> {
    let theme = theme::get(cx).clone();
    let session = ctx.session.read(cx);
    let w = session.client()?.read();
    let network = ctx.network.as_deref().and_then(|n| w.network(n));
    let me = people::me(&w, ctx.network.as_deref());
    let presence = match network.map(|n| n.status) {
        Some(NetworkStatus::Connected) if me.status == MemberStatus::Away => t!("people-away"),
        Some(NetworkStatus::Connected) => t!("people-online"),
        _ => t!("people-offline"),
    };
    let status = people::status_label(me, chrono::Utc::now().timestamp_millis());
    let line = match &status {
        Some(status) => t!("people-status-line", presence = presence, status = status),
        None => presence.to_string(),
    };
    let detail = status.as_ref().map(|_| detail(me, px(14.), cx));
    Some(
        div()
            .id("self-status")
            .test_support()
            .aria_label(SharedString::from(line.clone()))
            .text_xs()
            .line_height(px(16.))
            .text_color(theme.muted)
            .min_w_0()
            .child(detail.unwrap_or_else(|| div().truncate().child(presence).into_any_element()))
            .into_any_element(),
    )
}

/// The visible parts of a current status. Keep the emoji out of the text's
/// truncation area so a long status leaves the whole icon visible.
pub(crate) fn detail(member: &Member, glyph_size: Pixels, cx: &App) -> AnyElement {
    let theme = theme::get(cx);
    let glyph = member
        .status_emoji
        .as_deref()
        .filter(|glyph| !glyph.is_empty());
    let icon = glyph.map(|glyph| {
        let picture = glyph
            .strip_prefix(':')
            .and_then(|glyph| glyph.strip_suffix(':'))
            .and_then(|name| cx.try_global::<emoji::Pictures>()?.0.get(name).cloned());
        if let Some(url) = picture {
            div()
                .id("custom-status-emoji")
                .test_support()
                .size(glyph_size)
                .flex_none()
                .child(emoji::picture(url, theme.faint))
                .into_any_element()
        } else {
            div()
                .text_size(glyph_size)
                .flex_none()
                .child(glyph.to_owned())
                .into_any_element()
        }
    });
    div()
        .flex()
        .min_w_0()
        .min_h(glyph_size + px(2.))
        .line_height(glyph_size + px(2.))
        .items_center()
        .gap_1()
        .children(icon)
        .children(
            member
                .status_text
                .as_ref()
                .map(|text| div().min_w_0().truncate().child(text.clone())),
        )
        .into_any_element()
}

fn set_away(ctx: &SidebarCtx, away: bool, window: &mut Window, cx: &mut App) {
    let Some(network) = ctx.network.clone() else {
        return;
    };
    let Some(client) = ctx.session.read(cx).client().cloned() else {
        return;
    };
    let message = away.then(|| t!("people-status-away-message").to_string());
    let task = client
        .handle()
        .request(move |api| async move { api.set_away(&network, message.as_deref()).await });
    window
        .spawn(cx, async move |acx| {
            let result = task
                .await
                .map_err(|e| e.to_string())
                .and_then(|r| r.map_err(|e| e.to_string()));
            if let Err(e) = result {
                let _ = acx.update(|window, cx| ui::toast::error(window, cx, e));
            }
        })
        .detach();
}

/// Send a status change; the server's answer is your profile.
fn save(
    session: &Entity<Session>,
    network: &str,
    update: ProfileUpdate,
    window: &mut Window,
    cx: &mut App,
) {
    update_status(
        session,
        network,
        update,
        window,
        cx,
        |result, window, cx| match result {
            Ok(()) => ui::dialog::close(window, cx),
            Err(error) => ui::toast::error(window, cx, error),
        },
    );
}

pub(super) fn update_status(
    session: &Entity<Session>,
    network: &str,
    update: ProfileUpdate,
    window: &mut Window,
    cx: &mut App,
    finished: impl FnOnce(Result<(), String>, &mut Window, &mut App) + 'static,
) {
    let Some(client) = session.read(cx).client().cloned() else {
        return;
    };
    let network = network.to_string();
    let task = client
        .handle()
        .request(move |api| async move { api.update_network_profile(&network, &update).await });
    let session = session.clone();
    window
        .spawn(cx, async move |acx| {
            let result = task
                .await
                .map_err(|e| e.to_string())
                .and_then(|r| r.map_err(|e| e.to_string()));
            let _ = acx.update(|window, cx| {
                if !session
                    .read(cx)
                    .client()
                    .is_some_and(|active| Rc::ptr_eq(active, &client))
                {
                    return;
                }
                finished(
                    result.map(|member| client.change(|w| people::put_member(w, member))),
                    window,
                    cx,
                );
            });
        })
        .detach();
}

/// The status editor opened from the account menu.
fn open_dialog(session: &Entity<Session>, network: &str, window: &mut Window, cx: &mut App) {
    let Some(client) = session.read(cx).client().cloned() else {
        return;
    };
    let me = people::me(&client.read(), Some(network)).clone();
    let editor = cx.new(|cx| {
        let emoji = cx.new(|cx| {
            let mut s =
                InputState::new(window, cx).placeholder(t!("people-status-emoji-placeholder"));
            s.set_value(me.status_emoji.clone().unwrap_or_default(), window, cx);
            s
        });
        let text = cx.new(|cx| {
            let mut s =
                InputState::new(window, cx).placeholder(t!("people-status-text-placeholder"));
            s.set_value(me.status_text.clone().unwrap_or_default(), window, cx);
            s
        });
        StatusEditor {
            session: session.clone(),
            network: network.to_string(),
            emoji,
            text,
            expiry: Expiry::Keep,
            error: None,
        }
    });
    let input = editor.read(cx).text.clone();
    let focus = std::cell::Cell::new(true);
    ui::dialog::open(
        window,
        cx,
        t!("people-status-title"),
        move |dialog, window, cx| {
            if focus.replace(false) {
                input.focus_handle(cx).focus(window, cx);
            }
            dialog.w(px(420.)).child(editor.clone())
        },
    );
}

pub struct StatusEditor {
    session: Entity<Session>,
    network: String,
    pub emoji: Entity<InputState>,
    pub text: Entity<InputState>,
    expiry: Expiry,
    error: Option<SharedString>,
}

pub(super) fn expiry_label(expiry: Expiry, until: Option<&str>) -> String {
    match expiry {
        Expiry::Keep => match until.and_then(|u| chrono::DateTime::parse_from_rfc3339(u).ok()) {
            Some(at) => t!(
                "people-status-keep-until",
                when = at.with_timezone(&chrono::Local).format("%H:%M %d %b")
            ),
            None => t!("people-status-keep").to_string(),
        },
        Expiry::Never => t!("people-status-never").to_string(),
        Expiry::Minutes30 => t!("people-status-30m").to_string(),
        Expiry::Hour1 => t!("people-status-1h").to_string(),
        Expiry::Hours4 => t!("people-status-4h").to_string(),
        Expiry::Today => t!("people-status-today").to_string(),
        Expiry::Week => t!("people-status-week").to_string(),
    }
}

pub(super) fn error_text(error: StatusError) -> &'static str {
    match error {
        StatusError::Controls => t!("people-status-error-controls"),
        StatusError::TextTooLong => t!("people-status-error-text"),
        StatusError::EmojiTooLong => t!("people-status-error-emoji"),
    }
}

impl StatusEditor {
    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(client) = self.session.read(cx).client().cloned() else {
            return;
        };
        let me = people::me(&client.read(), Some(&self.network)).clone();
        let emoji = self.emoji.read(cx).value().to_string();
        let text = self.text.read(cx).value().to_string();
        match people::status_update(&me, &emoji, &text, self.expiry, chrono::Local::now()) {
            Err(e) => {
                self.error = Some(error_text(e).into());
                cx.notify();
            }
            Ok(None) => ui::dialog::close(window, cx),
            Ok(Some(update)) => save(&self.session, &self.network, update, window, cx),
        }
    }
}

impl Render for StatusEditor {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme::get(cx).clone();
        let until = self.session.read(cx).client().and_then(|client| {
            people::me(&client.read(), Some(&self.network))
                .status_expires_at
                .clone()
        });
        let mut choices = div()
            .id("status-expiry")
            .test_support()
            .role(Role::RadioGroup)
            .aria_label(t!("people-status-clear-after"))
            .flex()
            .flex_wrap()
            .gap_1();
        for expiry in Expiry::CHOICES {
            let selected = self.expiry == expiry;
            let label = expiry_label(expiry, until.as_deref());
            choices = choices.child(
                div()
                    .id(ElementId::Name(
                        format!("status-expiry-{}", expiry.key()).into(),
                    ))
                    .test_support()
                    .role(Role::RadioButton)
                    .reachable(&theme)
                    .aria_label(SharedString::from(label.clone()))
                    .aria_selected(selected)
                    .px_2()
                    .py_0p5()
                    .rounded_md()
                    .text_sm()
                    .cursor_pointer()
                    .when(selected, |d| d.bg(theme.accent).text_color(theme.on_accent))
                    .when(!selected, |d| d.bg(theme.accent.opacity(0.15)))
                    .child(label)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.expiry = expiry;
                        cx.notify();
                    })),
            );
        }
        let mut form = div()
            .id("status-editor")
            .test_support()
            .flex()
            .flex_col()
            .gap_3()
            .child(ui::field::field(
                t!("people-status-emoji"),
                Input::new(&self.emoji)
                    .id("status-emoji")
                    .aria_label(t!("people-status-emoji")),
                &theme,
            ))
            .child(ui::field::field(
                t!("people-status-text"),
                Input::new(&self.text)
                    .id("status-text")
                    .aria_label(t!("people-status-text")),
                &theme,
            ))
            .child(ui::field::field(
                t!("people-status-clear-after"),
                choices,
                &theme,
            ));
        if let Some(error) = self.error.clone() {
            form = form.child(
                div()
                    .id("status-error")
                    .test_support()
                    .role(Role::Alert)
                    .aria_label(error.clone())
                    .text_sm()
                    .text_color(theme.danger)
                    .child(error),
            );
        }
        let (session, network) = (self.session.clone(), self.network.clone());
        form.child(
            div()
                .flex()
                .justify_end()
                .gap_2()
                .child(
                    Button::new("status-clear-button")
                        .ghost()
                        .label(t!("people-status-clear"))
                        .on_click(move |_, window, cx| {
                            save(&session, &network, people::clear_status(), window, cx)
                        }),
                )
                .child(
                    Button::new("status-save")
                        .primary()
                        .label(t!("people-status-save"))
                        .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx))),
                ),
        )
    }
}

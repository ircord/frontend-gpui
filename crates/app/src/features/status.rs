//! Your status and away message (package P1).
//!
//! Your name at the foot of the sidebar opens your own menu: set or clear
//! a status (an emoji and a line of text that clears after a while), go
//! away or come back, and open your profile. The line under your name
//! shows your presence and status. `/status` opens the same editor.

use crate::ui::focus::Reachable;
use gpui_kit::TestSupportExt;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::prelude::*;
use gpui_kit::*;
use ircord_client::commands::View;
use ircord_client::contract::{MemberStatus, NetworkStatus, ProfileUpdate};
use ircord_client::store::people::{self, Expiry, StatusError};

use crate::features::commands::on_view;
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
                        open_editor(&ctx.session, &network, "", window, cx);
                    }
                },
            )
            .when(|ctx, _| ctx.network.is_some())
            .order(0),
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
            .when(|ctx, cx| me_status(ctx, cx).is_some_and(|(status, _)| status.is_some()))
            .order(1),
        );
        menu.add(
            MenuItem::new(
                "status-away",
                t!("people-status-go-away"),
                |ctx: &SidebarCtx, window, cx| set_away(ctx, true, window, cx),
            )
            .when(|ctx, cx| me_status(ctx, cx).is_some_and(|(_, away)| !away))
            .unavailable_when(not_connected)
            .order(10),
        );
        menu.add(
            MenuItem::new(
                "status-back",
                t!("people-status-come-back"),
                |ctx: &SidebarCtx, window, cx| set_away(ctx, false, window, cx),
            )
            .when(|ctx, cx| me_status(ctx, cx).is_some_and(|(_, away)| away))
            .unavailable_when(not_connected)
            .order(10),
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
            .order(5),
        );
        r.self_line.add(Slot::new("status-line", self_line));
    });
    on_view(cx, View::Status, |request, window, cx| {
        open_editor(
            &request.session,
            &request.network,
            &request.args,
            window,
            cx,
        );
    });
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

/// The line under your name: presence, then your status.
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
    let line = match people::status_label(me, chrono::Utc::now().timestamp_millis()) {
        Some(status) => t!("people-status-line", presence = presence, status = status),
        None => presence.to_string(),
    };
    Some(
        div()
            .id("self-status")
            .test_support()
            .aria_label(SharedString::from(line.clone()))
            .text_xs()
            .text_color(theme.muted)
            .truncate()
            .child(line)
            .into_any_element(),
    )
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
    let Some(client) = session.read(cx).client().cloned() else {
        return;
    };
    let network = network.to_string();
    let task = client
        .handle()
        .request(move |api| async move { api.update_network_profile(&network, &update).await });
    let client = client.clone();
    window
        .spawn(cx, async move |acx| {
            let result = task
                .await
                .map_err(|e| e.to_string())
                .and_then(|r| r.map_err(|e| e.to_string()));
            let _ = acx.update(|window, cx| match result {
                Ok(member) => {
                    client.change(|w| people::put_member(w, member));
                    ui::dialog::close(window, cx);
                }
                Err(e) => ui::toast::error(window, cx, e),
            });
        })
        .detach();
}

/// The status editor. `typed` prefills the text (what followed `/status`).
pub fn open_editor(
    session: &Entity<Session>,
    network: &str,
    typed: &str,
    window: &mut Window,
    cx: &mut App,
) {
    let Some(client) = session.read(cx).client().cloned() else {
        return;
    };
    let me = people::me(&client.read(), Some(network)).clone();
    let typed = typed.trim().to_string();
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
            let value = if typed.is_empty() {
                me.status_text.clone().unwrap_or_default()
            } else {
                typed.clone()
            };
            s.set_value(value, window, cx);
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

fn expiry_label(expiry: Expiry, cx: &App, editor: &StatusEditor) -> String {
    match expiry {
        Expiry::Keep => {
            let session = editor.session.read(cx);
            let until = session.client().and_then(|c| {
                people::me(&c.read(), Some(&editor.network))
                    .status_expires_at
                    .clone()
                    .filter(|s| !s.is_empty())
            });
            match until.and_then(|u| chrono::DateTime::parse_from_rfc3339(&u).ok()) {
                Some(at) => t!(
                    "people-status-keep-until",
                    when = at.with_timezone(&chrono::Local).format("%H:%M %d %b")
                ),
                None => t!("people-status-keep").to_string(),
            }
        }
        Expiry::Never => t!("people-status-never").to_string(),
        Expiry::Minutes30 => t!("people-status-30m").to_string(),
        Expiry::Hour1 => t!("people-status-1h").to_string(),
        Expiry::Hours4 => t!("people-status-4h").to_string(),
        Expiry::Today => t!("people-status-today").to_string(),
        Expiry::Week => t!("people-status-week").to_string(),
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
                self.error = Some(
                    match e {
                        StatusError::Controls => t!("people-status-error-controls"),
                        StatusError::TextTooLong => t!("people-status-error-text"),
                        StatusError::EmojiTooLong => t!("people-status-error-emoji"),
                    }
                    .into(),
                );
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
            let label = expiry_label(expiry, cx, self);
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

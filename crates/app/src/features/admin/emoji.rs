//! Administration → Emoji, after `AdminEmoji.vue`: by default everyone
//! signed in manages the shared emoji catalog; an owner can limit that to a
//! list of accounts. Owners always keep it.

use gpui_kit::TestSupportExt;
use gpui_kit::component::Disableable;
use gpui_kit::component::switch::Switch;
use gpui_kit::prelude::*;
use gpui_kit::*;
use ircord_client::contract as c;

use super::picker::Picker;
use super::{Kind, button, client, error_text, group, hint, notice};
use crate::session::Session;
use crate::theme;

pub struct EmojiPanel {
    session: Entity<Session>,
    picker: Entity<Picker>,
    state: Option<c::EmojiManagement>,
    error: Option<String>,
    notice: Option<String>,
    busy: bool,
}

impl EmojiPanel {
    pub fn new(session: Entity<Session>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let this = cx.weak_entity();
        let picker = cx.new(|cx| {
            Picker::new(
                session.clone(),
                "admin-emoji-add",
                t!("admin-emoji-add"),
                t!("admin-emoji-owner-note"),
                move |user, _, cx| {
                    this.update(cx, |this, cx| this.add(user, cx)).ok();
                },
                window,
                cx,
            )
        });
        let mut panel = Self {
            session,
            picker,
            state: None,
            error: None,
            notice: None,
            busy: false,
        };
        panel.load(cx);
        panel
    }

    pub fn load(&mut self, cx: &mut Context<Self>) {
        let Some(client) = client(&self.session, cx) else {
            return;
        };
        let task = client.request(|api| async move { api.emoji_management().await });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(Ok(state)) => {
                        this.state = Some(state);
                        this.error = None;
                    }
                    Ok(Err(e)) => {
                        super::refused(&client, &e, cx);
                        this.error = Some(error_text(&e));
                    }
                    Err(e) => this.error = Some(e.to_string()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn managers(&self) -> Vec<c::AdminUser> {
        self.state
            .as_ref()
            .map(|s| s.managers.clone())
            .unwrap_or_default()
    }

    fn save(&mut self, restricted: bool, managers: Vec<c::AdminUser>, cx: &mut Context<Self>) {
        let Some(client) = client(&self.session, cx) else {
            return;
        };
        self.busy = true;
        self.error = None;
        self.notice = None;
        self.picker.update(cx, |p, _| p.busy = true);
        let update = c::EmojiManagementUpdate {
            restricted,
            managers: managers.into_iter().map(|u| u.id).collect(),
        };
        let task =
            client.request(move |api| async move { api.save_emoji_management(&update).await });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                this.busy = false;
                this.picker.update(cx, |p, cx| {
                    p.busy = false;
                    cx.notify();
                });
                match result {
                    Ok(Ok(state)) => this.state = Some(state),
                    Ok(Err(e)) => {
                        super::refused(&client, &e, cx);
                        this.error = Some(error_text(&e));
                    }
                    Err(e) => this.error = Some(e.to_string()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn add(&mut self, user: c::AdminUser, cx: &mut Context<Self>) {
        let Some(state) = &self.state else {
            return;
        };
        let mut managers = self.managers();
        if managers.iter().any(|m| m.id == user.id) {
            self.notice = Some(t!("admin-emoji-already", username = user.username.as_str()));
            cx.notify();
            return;
        }
        let restricted = state.restricted;
        managers.push(user);
        self.save(restricted, managers, cx);
    }

    fn remove(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(state) = &self.state else {
            return;
        };
        let restricted = state.restricted;
        let managers = self.managers().into_iter().filter(|m| m.id != id).collect();
        self.save(restricted, managers, cx);
    }
}

impl Render for EmojiPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme::get(cx).clone();
        let mut body = div()
            .flex()
            .flex_col()
            .gap_6()
            .child(hint(t!("admin-emoji-intro"), &theme));
        if let Some(error) = &self.error {
            body = body.child(notice("admin-emoji-error", error.clone(), true, &theme));
        }
        let Some(state) = &self.state else {
            if self.error.is_none() {
                body = body.child(hint(t!("admin-loading"), &theme));
            }
            return body;
        };
        let restricted = state.restricted;
        let managers = state.managers.clone();
        body = body.child(
            div()
                .flex()
                .items_start()
                .justify_between()
                .gap_4()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(theme.text)
                                .child(t!("admin-emoji-limit")),
                        )
                        .child(hint(t!("admin-emoji-limit-help"), &theme)),
                )
                .child(
                    div().flex_none().child(
                        Switch::new("admin-emoji-restricted")
                            .checked(restricted)
                            .disabled(self.busy)
                            .accessibility_label(t!("admin-emoji-limit"))
                            .on_click(cx.listener(|this, on: &bool, _, cx| {
                                let managers = this.managers();
                                this.save(*on, managers, cx)
                            })),
                    ),
                ),
        );
        let mut list = div()
            .id("admin-emoji-managers")
            .test_support()
            .role(Role::List)
            .aria_label(t!("admin-emoji-managers"))
            .flex()
            .flex_col()
            .gap_2();
        for (i, user) in managers.iter().enumerate() {
            let id = user.id.clone();
            list = list.child(
                div()
                    .id(ElementId::Name(format!("admin-emoji-manager-{i}").into()))
                    .test_support()
                    .role(Role::ListItem)
                    .aria_label(user.username.clone())
                    .flex()
                    .items_center()
                    .gap_3()
                    .px_3p5()
                    .py_2p5()
                    .rounded_lg()
                    .bg(theme.raised)
                    .border_1()
                    .border_color(theme.border)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.text)
                            .child(user.username.clone()),
                    )
                    .child(button(
                        ElementId::Name(format!("admin-emoji-manager-{i}-remove").into()),
                        t!("admin-emoji-remove"),
                        Some(t!(
                            "admin-emoji-remove-manager",
                            username = user.username.as_str()
                        )),
                        Kind::Secondary,
                        !self.busy,
                        &theme,
                        cx.listener(move |this, _, _, cx| this.remove(&id, cx)),
                    )),
            );
        }
        let mut managers_body = div().flex().flex_col().gap_2().child(self.picker.clone());
        if let Some(text) = &self.notice {
            managers_body =
                managers_body.child(notice("admin-emoji-notice", text.clone(), false, &theme));
        }
        managers_body = if managers.is_empty() {
            managers_body.child(notice(
                "admin-emoji-none",
                t!("admin-emoji-none"),
                false,
                &theme,
            ))
        } else {
            managers_body.child(list)
        };
        body.child(group(
            "admin-emoji-group",
            t!("admin-emoji-managers"),
            managers_body,
            &theme,
        ))
    }
}

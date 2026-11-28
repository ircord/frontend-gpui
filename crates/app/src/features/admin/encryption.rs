//! Administration → Encryption, after `AdminEncryption.vue`: owners turn
//! experimental end-to-end encrypted direct messages on or off for this
//! server, effective at once. When the server's configuration fixes the mode
//! (`IRCORD_E2EE`), the backend reports it locked and refuses changes, so the
//! choice is shown read-only with the server's reason.

use gpui_kit::TestSupportExt;
use gpui_kit::prelude::*;
use gpui_kit::*;
use ircord_client::api::admin as a;
use ircord_client::contract as c;

use super::{Kind, button, client, error_text, group, hint, notice, when};
use crate::session::Session;
use crate::theme::{self, Theme};
use crate::ui;
use crate::ui::focus::Reachable;

pub struct EncryptionPanel {
    session: Entity<Session>,
    setting: Option<c::EncryptionSetting>,
    loading: bool,
    saving: bool,
    error: Option<String>,
    notice: Option<String>,
}

impl EncryptionPanel {
    pub fn new(session: Entity<Session>, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut panel = Self {
            session,
            setting: None,
            loading: false,
            saving: false,
            error: None,
            notice: None,
        };
        panel.load(cx);
        panel
    }

    pub fn load(&mut self, cx: &mut Context<Self>) {
        let Some(client) = client(&self.session, cx) else {
            return;
        };
        self.loading = true;
        self.error = None;
        let task = client.request(|api| async move { api.administration_encryption().await });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                this.loading = false;
                match result {
                    Ok(Ok(setting)) => this.setting = Some(setting),
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

    fn choose(&mut self, mode: c::EncryptionSettingMode, cx: &mut Context<Self>) {
        let Some(setting) = &self.setting else {
            return;
        };
        if setting.locked || self.saving || setting.mode == mode {
            return;
        }
        let Some(client) = client(&self.session, cx) else {
            return;
        };
        let update = c::EncryptionSettingUpdate {
            mode: match mode {
                c::EncryptionSettingMode::Experimental => {
                    c::EncryptionSettingUpdateMode::Experimental
                }
                _ => c::EncryptionSettingUpdateMode::Off,
            },
            reason: None,
        };
        self.saving = true;
        self.error = None;
        self.notice = None;
        let task =
            client.request(move |api| async move { api.save_encryption_setting(&update).await });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                this.saving = false;
                match result {
                    Ok(Ok(setting)) => {
                        this.notice = Some(
                            if setting.mode == c::EncryptionSettingMode::Experimental {
                                t!("admin-encryption-saved-on")
                            } else {
                                t!("admin-encryption-saved-off")
                            }
                            .to_string(),
                        );
                        this.setting = Some(setting);
                        // The capability arrives only with a snapshot: read
                        // one now so this client offers encryption (or stops)
                        // without waiting for a reconnect.
                        drop(client.call(
                            |api| async move { api.bootstrap().await },
                            |w, result| {
                                result
                                    .map(|s| a::take_encryption(w, &s))
                                    .unwrap_or_default()
                            },
                        ));
                    }
                    Ok(Err(e)) => {
                        super::refused(&client, &e, cx);
                        if a::conflict(&e) {
                            // Most likely the configuration fixed the mode
                            // since the page loaded: read it again, and the
                            // lock note explains the refusal.
                            this.reread_after_conflict(error_text(&e), cx);
                        } else {
                            this.error = Some(error_text(&e));
                        }
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

    /// After a 409, read the setting again; when it is now locked, say so
    /// instead of showing the refusal.
    fn reread_after_conflict(&mut self, error: String, cx: &mut Context<Self>) {
        let Some(client) = client(&self.session, cx) else {
            self.error = Some(error);
            return;
        };
        let task = client.request(|api| async move { api.administration_encryption().await });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(Ok(setting)) if setting.locked => {
                        this.notice = Some(t!("admin-encryption-now-locked").to_string());
                        this.setting = Some(setting);
                    }
                    Ok(Ok(setting)) => {
                        this.setting = Some(setting);
                        this.error = Some(error);
                    }
                    _ => this.error = Some(error),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

/// One choice of the mode; a disabled one looks faint and does nothing.
fn option(
    id: &'static str,
    label: &'static str,
    on: bool,
    enabled: bool,
    theme: &Theme,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let hover = theme.hover;
    div()
        .id(id)
        .test_support()
        .role(Role::RadioButton)
        .reachable(theme)
        .aria_label(label)
        .aria_selected(on)
        .px_3()
        .py_1()
        .rounded_md()
        .text_sm()
        .bg(if on { theme.accent } else { theme.input })
        .text_color(if on { theme.on_accent } else { theme.text })
        .when(!on, |d| d.border_1().border_color(theme.control_border))
        .child(label)
        .when(!enabled, |d| d.opacity(0.5))
        .when(enabled && !on, |d| {
            d.cursor_pointer()
                .hover(move |d| d.bg(hover))
                .on_click(on_click)
        })
        .into_any_element()
}

/// A line of the "What this changes" list.
fn point(text: &'static str, theme: &Theme) -> AnyElement {
    div()
        .flex()
        .items_start()
        .gap_2()
        .child(
            div()
                .mt(px(7.))
                .size(px(4.))
                .flex_none()
                .rounded_full()
                .bg(theme.muted),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_sm()
                .text_color(theme.text)
                .child(text),
        )
        .into_any_element()
}

impl Render for EncryptionPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme::get(cx).clone();
        let mut body = div()
            .flex()
            .flex_col()
            .gap_6()
            .child(hint(t!("admin-encryption-intro"), &theme));
        if let Some(error) = &self.error {
            body = body.child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(notice(
                        "admin-encryption-error",
                        error.clone(),
                        true,
                        &theme,
                    ))
                    .child(button(
                        "admin-encryption-reload",
                        t!("admin-encryption-reload"),
                        None,
                        Kind::Ghost,
                        !self.loading,
                        &theme,
                        cx.listener(|this, _, _, cx| this.load(cx)),
                    )),
            );
        }
        let Some(setting) = self.setting.clone() else {
            if self.error.is_none() {
                body = body.child(hint(t!("admin-encryption-loading"), &theme));
            }
            return body;
        };
        let enabled = !setting.locked && !self.saving;
        let mut choices = div()
            .id("admin-encryption-mode")
            .test_support()
            .role(Role::RadioGroup)
            .aria_label(t!("admin-encryption-mode"))
            .flex_none()
            .flex()
            .gap_1();
        for (id, label, mode) in [
            (
                "admin-encryption-off",
                t!("admin-encryption-off"),
                c::EncryptionSettingMode::Off,
            ),
            (
                "admin-encryption-experimental",
                t!("admin-encryption-experimental"),
                c::EncryptionSettingMode::Experimental,
            ),
        ] {
            choices = choices.child(option(
                id,
                label,
                setting.mode == mode,
                enabled,
                &theme,
                cx.listener(move |this, _, _, cx| this.choose(mode, cx)),
            ));
        }
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
                                .child(t!("admin-encryption-mode")),
                        )
                        .child(hint(t!("admin-encryption-mode-help"), &theme)),
                )
                .child(choices),
        );
        if setting.locked {
            let reason = setting
                .lock_reason
                .clone()
                .filter(|r| !r.is_empty())
                .unwrap_or_else(|| t!("admin-encryption-locked-fallback").to_string());
            body = body.child(
                div()
                    .id("admin-encryption-locked")
                    .test_support()
                    .role(Role::Note)
                    .aria_label(format!("{}: {reason}", t!("admin-encryption-locked")))
                    .flex()
                    .items_start()
                    .gap_2()
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .bg(theme.raised)
                    .border_1()
                    .border_color(theme.border)
                    .child(
                        div()
                            .mt(px(2.))
                            .flex_none()
                            .child(ui::icon::icon(ui::icon::LucideIcon::Lock, theme.muted)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap_0p5()
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(theme.text)
                                    .child(t!("admin-encryption-locked")),
                            )
                            .child(hint(reason, &theme)),
                    ),
            );
        }
        body = body.child(group(
            "admin-encryption-about",
            t!("admin-encryption-about"),
            div()
                .flex()
                .flex_col()
                .gap_2()
                .child(point(t!("admin-encryption-about-experimental"), &theme))
                .child(point(t!("admin-encryption-about-review"), &theme))
                .child(point(t!("admin-encryption-about-off"), &theme)),
            &theme,
        ));
        if let Some(text) = &self.notice {
            body = body.child(notice(
                "admin-encryption-notice",
                text.clone(),
                false,
                &theme,
            ));
        } else if let Some(at) = setting.updated_at.as_deref().filter(|_| !setting.locked) {
            let time = when(at);
            let text = match setting.updated_by.as_deref().filter(|b| !b.is_empty()) {
                Some(account) => t!(
                    "admin-encryption-last-changed",
                    account = account,
                    time = time.as_str()
                ),
                None => t!("admin-encryption-last-changed-at", time = time.as_str()),
            };
            body = body.child(notice("admin-encryption-changed", text, false, &theme));
        }
        body
    }
}

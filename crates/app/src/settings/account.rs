//! Settings → Account: who you are, the password, logging out, deleting the
//! account, and the devices signed in to it (package S0), as the web client's
//! `AccountSettings.vue` and `DeviceSessions.vue`.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use gpui_kit::TestSupportExt;
use gpui_kit::component::Disableable;
use gpui_kit::component::Sizable;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::*;
use gpui_kit::*;
use ircord_client::api::ApiError;
use ircord_client::contract as c;
use ircord_client::store::topic;

use super::{block, row};
use crate::registry::{self, SettingsCtx, SettingsGroup, SettingsRow, SettingsSection};
use crate::session::{Session, SessionEvent};
use crate::theme::{self, Theme};
use crate::ui;

pub fn init(cx: &mut App) {
    registry::register(cx, |r| {
        r.settings_sections.add(
            SettingsSection::new("account", t!("settings-account"), |ctx, window, cx| {
                let view = view(ctx, window, cx);
                view.update(cx, |v, _| v.ctx = Some(ctx.clone()));
                view.into_any_element()
            })
            .group(SettingsGroup::User)
            .order(20)
            .row(
                SettingsRow::new("password", t!("settings-account-password-label"))
                    .keywords(t!("settings-account-password-keywords")),
            )
            .row(
                SettingsRow::new("log-out", t!("settings-account-log-out"))
                    .description(t!("settings-account-log-out-description"))
                    .keywords(t!("settings-account-log-out-keywords")),
            )
            .row(
                SettingsRow::new("delete-account", t!("settings-account-delete"))
                    .description(t!("settings-account-delete-description"))
                    .keywords(t!("settings-account-delete-keywords")),
            )
            .row(
                SettingsRow::new("device-sessions", t!("settings-devices-label"))
                    .description(t!("settings-devices-description"))
                    .keywords(t!("settings-devices-keywords")),
            ),
        );
    });
}

/// The account view, kept while the session lasts.
struct Account(Entity<AccountView>);
impl Global for Account {}

fn view(ctx: &SettingsCtx, window: &mut Window, cx: &mut App) -> Entity<AccountView> {
    if let Some(Account(view)) = cx.try_global::<Account>()
        && view.read(cx).session == ctx.session
    {
        return view.clone();
    }
    let view = cx.new(|cx| AccountView::new(ctx.session.clone(), window, cx));
    cx.set_global(Account(view.clone()));
    view
}

/// Something read from the server.
enum Load<T> {
    Loading,
    Failed(String),
    Ready(T),
}

pub struct AccountView {
    session: Entity<Session>,
    /// The settings context it was last drawn in, for the landing row.
    ctx: Option<SettingsCtx>,
    account: Load<c::Account>,
    devices: Load<Vec<c::Device>>,
    current: Entity<InputState>,
    new: Entity<InputState>,
    changing: bool,
    password_error: Option<String>,
    password_saved: bool,
    deleting: bool,
    delete_error: Option<String>,
    /// Devices being signed out.
    signing_out: Vec<String>,
    device_errors: HashMap<String, String>,
    notice: Option<String>,
    _subscriptions: Vec<Subscription>,
}

impl AccountView {
    fn new(session: Entity<Session>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let current = cx.new(|cx| InputState::new(window, cx).masked(true));
        let new = cx.new(|cx| InputState::new(window, cx).masked(true));
        let subscriptions = vec![
            cx.subscribe_in(&new, window, |this: &mut Self, _, event, window, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    this.change_password(window, cx);
                }
            }),
            cx.subscribe_in(&current, window, |_, _, event, _, cx| {
                if let InputEvent::Change = event {
                    cx.notify();
                }
            }),
            cx.subscribe_in(&new, window, |_, _, event, _, cx| {
                if let InputEvent::Change = event {
                    cx.notify();
                }
            }),
            // Another device signed in or out: read the list again.
            cx.subscribe(&session, |this, _, event: &SessionEvent, cx| {
                if let SessionEvent::Changed(changes) = event
                    && changes.topics.contains_key(topic::DEVICES)
                {
                    this.load_devices(cx);
                }
            }),
        ];
        let mut view = Self {
            session,
            ctx: None,
            account: Load::Loading,
            devices: Load::Loading,
            current,
            new,
            changing: false,
            password_error: None,
            password_saved: false,
            deleting: false,
            delete_error: None,
            signing_out: Vec::new(),
            device_errors: HashMap::new(),
            notice: None,
            _subscriptions: subscriptions,
        };
        view.load_account(cx);
        view.load_devices(cx);
        view
    }

    fn client(&self, cx: &App) -> Option<std::rc::Rc<ircord_client::Client>> {
        self.session.read(cx).client().cloned()
    }

    fn load_account(&mut self, cx: &mut Context<Self>) {
        let Some(client) = self.client(cx) else {
            return;
        };
        self.account = Load::Loading;
        let task = client.request(|api| async move { api.account().await });
        cx.spawn(async move |this, cx| {
            let result = joined(task.await);
            let _ = this.update(cx, |this, cx| {
                this.account = match result {
                    Ok(Some(account)) => Load::Ready(account),
                    Ok(None) => Load::Failed(ApiError::Unauthorized.to_string()),
                    Err(e) => Load::Failed(e),
                };
                cx.notify();
            });
        })
        .detach();
    }

    fn load_devices(&mut self, cx: &mut Context<Self>) {
        let Some(client) = self.client(cx) else {
            return;
        };
        let task = client.request(|api| async move { api.devices().await });
        cx.spawn(async move |this, cx| {
            let result = joined(task.await);
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(devices) => this.devices = Load::Ready(devices),
                    // Keep a list already shown; say why only when there is none.
                    Err(e) if !matches!(this.devices, Load::Ready(_)) => {
                        this.devices = Load::Failed(e)
                    }
                    Err(e) => log::warn!("devices: {e}"),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn change_password(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let current = self.current.read(cx).value().to_string();
        let new = self.new.read(cx).value().to_string();
        if self.changing || current.is_empty() {
            return;
        }
        self.password_saved = false;
        if let Some(problem) = password_problem(&new) {
            self.password_error = Some(problem.into());
            cx.notify();
            return;
        }
        let Some(client) = self.client(cx) else {
            return;
        };
        self.changing = true;
        self.password_error = None;
        cx.notify();
        let task =
            client.request(move |api| async move { api.change_password(&current, &new).await });
        cx.spawn_in(window, async move |this, cx| {
            let result = joined(task.await);
            let _ = this.update_in(cx, |this, window, cx| {
                this.changing = false;
                match result {
                    Ok(()) => {
                        this.password_saved = true;
                        for field in [&this.current, &this.new] {
                            field.update(cx, |f, cx| f.set_value("", window, cx));
                        }
                    }
                    Err(e) => this.password_error = Some(e),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn log_out(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let session = self.session.clone();
        ui::confirm::confirm(
            window,
            cx,
            ui::confirm::Confirm {
                title: t!("settings-account-log-out-title").into(),
                body: t!("settings-account-log-out-body").into(),
                ok: t!("settings-account-log-out").into(),
                danger: true,
            },
            move |_, cx| session.update(cx, |s, cx| s.sign_out(cx)),
        );
    }

    fn ask_delete(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let password = cx.new(|cx| InputState::new(window, cx).masked(true));
        let this = cx.entity();
        let focus = std::cell::Cell::new(true);
        ui::dialog::open(
            window,
            cx,
            t!("settings-account-delete-title"),
            move |dialog, window, cx| {
                if focus.replace(false) {
                    password.focus_handle(cx).focus(window, cx);
                }
                let theme = theme::get(cx).clone();
                let (this, password2) = (this.clone(), password.clone());
                dialog
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_3()
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(theme.muted)
                                    .child(t!("settings-account-delete-body")),
                            )
                            .child(ui::field::field(
                                t!("settings-account-current-password"),
                                Input::new(&password)
                                    .id("delete-password")
                                    .aria_label(t!("settings-account-current-password")),
                                &theme,
                            )),
                    )
                    .footer(
                        div()
                            .flex()
                            .justify_end()
                            .gap_2()
                            .child(
                                Button::new("delete-cancel")
                                    .label(t!("core-cancel"))
                                    .on_click(|_, window, cx| ui::dialog::close(window, cx)),
                            )
                            .child(
                                Button::new("delete-confirm")
                                    .danger()
                                    .label(t!("settings-account-delete"))
                                    .on_click(move |_, window, cx| {
                                        let typed = password2.read(cx).value().to_string();
                                        if typed.is_empty() {
                                            return;
                                        }
                                        ui::dialog::close(window, cx);
                                        this.update(cx, |v, cx| v.delete(typed, cx));
                                    }),
                            ),
                    )
            },
        );
    }

    fn delete(&mut self, password: String, cx: &mut Context<Self>) {
        let Some(client) = self.client(cx) else {
            return;
        };
        self.deleting = true;
        self.delete_error = None;
        cx.notify();
        let task = client.request(move |api| async move { api.delete_account(&password).await });
        cx.spawn(async move |this, cx| {
            let result = joined(task.await);
            let _ = this.update(cx, |this, cx| {
                this.deleting = false;
                match result {
                    Ok(()) => this.session.update(cx, |s, cx| s.sign_out(cx)),
                    Err(e) => this.delete_error = Some(e),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn ask_sign_out(&mut self, device: c::Device, window: &mut Window, cx: &mut Context<Self>) {
        let name = device_name(&device);
        let confirm = if device.current {
            ui::confirm::Confirm {
                title: t!("settings-devices-sign-out-this-title").into(),
                body: t!("settings-devices-sign-out-this-body").into(),
                ok: t!("settings-devices-sign-out").into(),
                danger: true,
            }
        } else {
            ui::confirm::Confirm {
                title: t!("settings-devices-sign-out-other-title", name = name).into(),
                body: t!("settings-devices-sign-out-other-body").into(),
                ok: t!("settings-devices-sign-out-other-confirm").into(),
                danger: true,
            }
        };
        let this = cx.entity();
        ui::confirm::confirm(window, cx, confirm, move |_, cx| {
            this.update(cx, |v, cx| v.sign_out_device(device.clone(), cx))
        });
    }

    fn sign_out_device(&mut self, device: c::Device, cx: &mut Context<Self>) {
        let Some(client) = self.client(cx) else {
            return;
        };
        let name = device_name(&device);
        self.signing_out.push(device.id.clone());
        self.device_errors.remove(&device.id);
        self.notice = None;
        cx.notify();
        let id = device.id.clone();
        let task = client.request(move |api| async move { api.sign_out_device(&id).await });
        cx.spawn(async move |this, cx| {
            let result = joined(task.await);
            let _ = this.update(cx, |this, cx| {
                this.signing_out.retain(|d| d != &device.id);
                match result {
                    Ok(_) if device.current => {
                        return this.session.update(cx, |s, cx| s.sign_out(cx));
                    }
                    Ok(true) => this.notice = Some(t!("settings-devices-signed-out", name = name)),
                    Ok(false) => {
                        this.notice = Some(t!("settings-devices-already-signed-out", name = name))
                    }
                    Err(e) => {
                        this.device_errors.insert(
                            device.id.clone(),
                            t!("settings-devices-sign-out-failed", name = name, error = e),
                        );
                    }
                }
                this.load_devices(cx);
                cx.notify();
            });
        })
        .detach();
    }
}

/// A request's answer, with a lost task as its error text.
fn joined<T>(result: Result<Result<T, ApiError>, tokio::task::JoinError>) -> Result<T, String> {
    result
        .map_err(|e| e.to_string())
        .and_then(|r| r.map_err(|e| e.to_string()))
}

/// Why the server would refuse a new password, checked before sending.
pub fn password_problem(password: &str) -> Option<&'static str> {
    if password.chars().count() < 8 {
        Some(t!("settings-account-password-short"))
    } else if password.chars().any(char::is_whitespace) {
        Some(t!("settings-account-password-spaces"))
    } else {
        None
    }
}

/// "Firefox on Linux"; the device's own name when it has one. The server
/// does not know this app's user agent, so this device is "IRCord on Linux".
pub fn device_name(d: &c::Device) -> String {
    if !d.name.is_empty() {
        return d.name.clone();
    }
    let browser = if d.browser.is_empty() && d.current {
        t!("core-app-name")
    } else {
        d.browser.as_str()
    };
    match (browser.is_empty(), d.os.is_empty()) {
        (false, false) => t!(
            "settings-devices-browser-on-os",
            browser = browser,
            os = d.os
        ),
        (false, true) => browser.to_string(),
        (true, false) => d.os.clone(),
        (true, true) => t!("settings-devices-unknown-browser").into(),
    }
}

/// "just now", "5 min ago", "2 wk ago".
pub fn ago(iso: &str, now: DateTime<Utc>) -> String {
    let Ok(then) = DateTime::parse_from_rfc3339(iso) else {
        return t!("settings-devices-unknown").into();
    };
    let seconds = (now - then.with_timezone(&Utc)).num_seconds().max(0);
    const STEPS: [(i64, &str); 6] = [
        (60, "minutes"),
        (3600, "hours"),
        (86_400, "days"),
        (7 * 86_400, "weeks"),
        (30 * 86_400, "months"),
        (365 * 86_400, "years"),
    ];
    if seconds < 60 {
        return t!("settings-devices-just-now").into();
    }
    let (size, unit) = STEPS
        .iter()
        .rev()
        .find(|(size, _)| seconds >= *size)
        .copied()
        .unwrap_or(STEPS[0]);
    let n = seconds / size;
    match unit {
        "minutes" => t!("settings-devices-minutes-ago", n = n),
        "hours" => t!("settings-devices-hours-ago", n = n),
        "days" => t!("settings-devices-days-ago", n = n),
        "weeks" => t!("settings-devices-weeks-ago", n = n),
        "months" => t!("settings-devices-months-ago", n = n),
        _ => t!("settings-devices-years-ago", n = n),
    }
}

/// How recently the device showed signs of life.
pub fn presence(d: &c::Device, now: DateTime<Utc>) -> String {
    if d.active {
        return t!("settings-devices-active-now").into();
    }
    if d.status == c::DeviceStatus::Connected {
        return t!("settings-devices-connected-now").into();
    }
    match &d.last_seen_at {
        Some(at) => t!("settings-devices-last-seen", time = ago(at, now)),
        None => t!("settings-devices-last-seen-unknown").into(),
    }
}

/// "26 November 2026".
fn date(iso: &str) -> String {
    DateTime::parse_from_rfc3339(iso)
        .map(|d| d.format("%-d %B %Y").to_string())
        .unwrap_or_default()
}

/// The device's type; this app, which the server cannot tell from its user
/// agent, is a desktop.
fn form_factor(d: &c::Device) -> &'static str {
    match d.form_factor {
        c::DeviceFormFactor::UnknownValue | c::DeviceFormFactor::Unknown if d.current => {
            t!("settings-devices-type-desktop")
        }
        c::DeviceFormFactor::Desktop => t!("settings-devices-type-desktop"),
        c::DeviceFormFactor::Phone => t!("settings-devices-type-phone"),
        c::DeviceFormFactor::Tablet => t!("settings-devices-type-tablet"),
        _ => t!("settings-devices-type-unknown"),
    }
}

fn note<T: Into<SharedString>>(
    id: &'static str,
    text: T,
    color: Hsla,
    alert: bool,
) -> impl IntoElement + use<T> {
    let text: SharedString = text.into();
    div()
        .id(id)
        .role(if alert { Role::Alert } else { Role::Status })
        .aria_label(text.clone())
        .text_sm()
        .text_color(color)
        .child(text)
        .test_support()
}

impl Render for AccountView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme::get(cx).clone();
        let ctx = self.ctx.clone().unwrap_or(SettingsCtx {
            session: self.session.clone(),
            target: None,
        });
        div()
            .id("account-settings")
            .flex()
            .flex_col()
            .gap_4()
            .child(self.summary(&theme, cx))
            .child(self.password(&ctx, &theme, cx))
            .child(self.session_rows(&ctx, &theme, cx))
            .child(self.devices(&ctx, &theme, cx))
    }
}

impl AccountView {
    fn summary(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        match &self.account {
            Load::Loading => note(
                "account-summary",
                t!("settings-account-loading"),
                theme.muted,
                false,
            )
            .into_any_element(),
            Load::Failed(e) => div()
                .flex()
                .items_center()
                .gap_3()
                .child(note(
                    "account-summary",
                    t!("settings-account-load-failed", error = e),
                    theme.danger,
                    true,
                ))
                .child(
                    Button::new("account-retry")
                        .small()
                        .label(t!("settings-account-retry"))
                        .on_click(cx.listener(|this, _, _, cx| this.load_account(cx))),
                )
                .into_any_element(),
            Load::Ready(account) => {
                let text = t!(
                    "settings-account-summary",
                    created = date(&account.created_at),
                    expires = date(&account.expires_at)
                );
                div()
                    .id("account-summary")
                    .test_support()
                    .role(Role::Group)
                    .aria_label(SharedString::from(account.username.clone()))
                    .p_3()
                    .rounded_md()
                    .bg(theme.sidebar)
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(ui::icon::icon(ui::icon::IconName::User, theme.accent))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(theme.text)
                                    .child(account.username.clone()),
                            )
                            .child(div().text_sm().text_color(theme.muted).child(text)),
                    )
                    .into_any_element()
            }
        }
    }

    fn password(
        &self,
        ctx: &SettingsCtx,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let ready = !self.current.read(cx).value().is_empty()
            && self.new.read(cx).value().chars().count() >= 8
            && !self.changing;
        let mut form = div()
            .max_w(px(420.))
            .flex()
            .flex_col()
            .gap_2()
            .child(ui::field::field(
                t!("settings-account-current-password"),
                Input::new(&self.current)
                    .id("current-password")
                    .aria_label(t!("settings-account-current-password")),
                theme,
            ))
            .child(ui::field::field(
                t!("settings-account-new-password"),
                Input::new(&self.new)
                    .id("new-password")
                    .aria_label(t!("settings-account-new-password")),
                theme,
            ))
            .child(
                div()
                    .text_xs()
                    .text_color(theme.faint)
                    .child(t!("settings-account-new-password-help")),
            );
        if let Some(error) = &self.password_error {
            form = form.child(note("password-error", error.clone(), theme.danger, true));
        }
        if self.password_saved {
            form = form.child(note(
                "password-saved",
                t!("settings-account-password-changed"),
                theme.online,
                false,
            ));
        }
        form = form.child(
            div().flex().justify_end().child(
                Button::new("change-password")
                    .primary()
                    .small()
                    .label(if self.changing {
                        t!("settings-account-changing-password")
                    } else {
                        t!("settings-account-change-password")
                    })
                    .disabled(!ready)
                    .on_click(cx.listener(|this, _, window, cx| this.change_password(window, cx))),
            ),
        );
        block(
            ctx,
            "password",
            t!("settings-account-password"),
            form,
            theme,
        )
    }

    fn session_rows(&self, ctx: &SettingsCtx, theme: &Theme, cx: &mut Context<Self>) -> Div {
        let mut rows = div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.text)
                    .child(t!("settings-account-session")),
            )
            .child(row(
                ctx,
                "log-out",
                t!("settings-account-log-out"),
                Some(t!("settings-account-log-out-description").into()),
                Button::new("log-out-button")
                    .small()
                    .label(t!("settings-account-log-out"))
                    .on_click(cx.listener(|this, _, window, cx| this.log_out(window, cx))),
                theme,
            ))
            .child(row(
                ctx,
                "delete-account",
                t!("settings-account-delete"),
                Some(t!("settings-account-delete-description").into()),
                Button::new("delete-account-button")
                    .danger()
                    .small()
                    .label(if self.deleting {
                        t!("settings-account-deleting")
                    } else {
                        t!("settings-account-delete")
                    })
                    .disabled(self.deleting)
                    .on_click(cx.listener(|this, _, window, cx| this.ask_delete(window, cx))),
                theme,
            ));
        if let Some(error) = &self.delete_error {
            rows = rows.child(note("delete-error", error.clone(), theme.danger, true));
        }
        rows
    }

    fn devices(
        &self,
        ctx: &SettingsCtx,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let mut body = div().flex().flex_col().gap_2().child(
            div()
                .text_sm()
                .text_color(theme.muted)
                .child(t!("settings-devices-intro")),
        );
        if let Some(notice) = &self.notice {
            body = body.child(note("devices-notice", notice.clone(), theme.muted, false));
        }
        body = match &self.devices {
            Load::Loading => body.child(note(
                "devices-status",
                t!("settings-devices-loading"),
                theme.muted,
                false,
            )),
            Load::Failed(e) => body.child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(note(
                        "devices-status",
                        t!("settings-devices-load-failed", error = e),
                        theme.danger,
                        true,
                    ))
                    .child(
                        Button::new("devices-retry")
                            .small()
                            .label(t!("settings-account-retry"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.devices = Load::Loading;
                                this.load_devices(cx);
                                cx.notify();
                            })),
                    ),
            ),
            Load::Ready(devices) if devices.is_empty() => body.child(note(
                "devices-status",
                t!("settings-devices-none"),
                theme.muted,
                false,
            )),
            Load::Ready(devices) => {
                let now = Utc::now();
                let mut list = div()
                    .id("device-list")
                    .test_support()
                    .role(Role::List)
                    .aria_label(t!("settings-devices-label"))
                    .flex()
                    .flex_col()
                    .gap_1();
                for device in devices {
                    list = list.child(self.device_row(device, now, theme, cx));
                }
                body.child(list)
            }
        };
        block(ctx, "device-sessions", t!("settings-devices"), body, theme)
    }

    fn device_row(
        &self,
        device: &c::Device,
        now: DateTime<Utc>,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let name = device_name(device);
        let label: SharedString = if device.current {
            t!("settings-devices-current-row", name = name).into()
        } else {
            name.clone().into()
        };
        let browser = [device.browser.as_str(), device.os.as_str()]
            .iter()
            .filter(|s| !s.is_empty())
            .copied()
            .collect::<Vec<_>>()
            .join(" · ");
        let browser = if browser.is_empty() {
            t!("settings-devices-unknown").to_string()
        } else {
            browser
        };
        let details = format!(
            "{} · {}",
            presence(device, now),
            t!(
                "settings-devices-signed-in",
                time = ago(&device.signed_in_at, now)
            ),
        );
        let busy = self.signing_out.contains(&device.id);
        let id = device.id.clone();
        let sign_out_label: SharedString = if device.current {
            t!("settings-devices-sign-out").into()
        } else {
            t!("settings-devices-sign-out-named", name = name).into()
        };
        let d = device.clone();
        div()
            .id(ElementId::Name(format!("device-{id}").into()))
            .role(Role::ListItem)
            .aria_label(label)
            .px_3()
            .py_2()
            .rounded_md()
            .bg(theme.input)
            .flex()
            .items_center()
            .gap_3()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .items_center()
                            .child(
                                div()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(theme.text)
                                    .child(name),
                            )
                            .when(device.current, |d| {
                                d.child(
                                    div()
                                        .px_1p5()
                                        .rounded_md()
                                        .bg(theme.selected)
                                        .text_xs()
                                        .text_color(theme.text)
                                        .child(t!("settings-devices-this-device")),
                                )
                            }),
                    )
                    .child(div().text_sm().text_color(theme.muted).child(details))
                    .when(device.focal, |d| {
                        d.child(
                            div()
                                .text_sm()
                                .text_color(theme.muted)
                                .child(t!("settings-devices-selected")),
                        )
                    })
                    .child(div().text_sm().text_color(theme.faint).child(t!(
                        "settings-devices-details",
                        browser = browser,
                        kind = form_factor(device)
                    )))
                    .when_some(self.device_errors.get(&device.id), |d, error| {
                        d.child(
                            div()
                                .text_sm()
                                .text_color(theme.danger)
                                .child(error.clone()),
                        )
                    }),
            )
            .child(
                Button::new(ElementId::Name(format!("sign-out-{id}").into()))
                    .small()
                    .when(device.current, |b| b.danger())
                    .label(if busy {
                        t!("settings-devices-signing-out")
                    } else {
                        t!("settings-devices-sign-out")
                    })
                    .tooltip(sign_out_label)
                    .disabled(busy)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.ask_sign_out(d.clone(), window, cx)
                    })),
            )
            .test_support()
    }
}

#[cfg(test)]
mod tests {
    use super::{DateTime, Utc, ago, c, device_name, password_problem};

    fn device(browser: &str, os: &str, name: &str) -> c::Device {
        c::Device {
            browser: browser.into(),
            os: os.into(),
            name: name.into(),
            ..Default::default()
        }
    }

    #[test]
    fn device_names() {
        assert_eq!(
            device_name(&device("Firefox", "Linux", "")),
            "Firefox on Linux"
        );
        assert_eq!(device_name(&device("", "Android", "")), "Android");
        assert_eq!(device_name(&device("", "", "")), "Unknown browser");
        assert_eq!(device_name(&device("Firefox", "Linux", "Laptop")), "Laptop");
        let mut here = device("", "Linux", "");
        here.current = true;
        assert_eq!(device_name(&here), "IRCord on Linux");
    }

    #[test]
    fn relative_times() {
        let now = DateTime::parse_from_rfc3339("2026-11-21T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        assert_eq!(ago("2026-11-21T11:59:30Z", now), "just now");
        assert_eq!(ago("2026-11-21T11:55:00Z", now), "5 min ago");
        assert_eq!(ago("2026-11-21T09:00:00Z", now), "3 h ago");
        assert_eq!(ago("2026-11-07T12:00:00Z", now), "2 wk ago");
        assert_eq!(ago("2024-11-21T12:00:00Z", now), "2 yr ago");
        assert_eq!(ago("not a time", now), "Unknown");
    }

    #[test]
    fn passwords_are_checked_before_sending() {
        assert!(password_problem("short").is_some());
        assert!(password_problem("has a space").is_some());
        assert_eq!(password_problem("long-enough"), None);
    }
}

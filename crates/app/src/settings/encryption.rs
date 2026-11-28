//! Settings → Encryption (experimental): this device's end-to-end
//! encryption. A device set up shows the account's fingerprint, its devices
//! and, when it can approve others, a field for the approval code a new
//! device shows and how many wait; a new device shows that code, as text and
//! as a QR code, as the web client does. Starting fresh is here too, behind
//! a confirmation that says what follows. There is no recovery key: the page
//! says so.

use gpui_kit::TestSupportExt;
use std::collections::HashMap;

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{Disableable, Sizable};
use gpui_kit::prelude::*;
use gpui_kit::*;
use ircord_client::store::e2ee::{ApprovalFailure, Approving, DeviceState};

use super::{block, row};
use crate::features::encryption::{engine, qr_code};
use crate::registry::{self, SettingsCtx, SettingsGroup, SettingsRow, SettingsSection};
use crate::session::Session;
use crate::theme::{self, Theme};
use crate::ui::confirm::{Confirm, confirm};

pub fn init(cx: &mut App) {
    registry::register(cx, |r| {
        r.settings_sections.add(
            SettingsSection::new("encryption", t!("encryption-settings-title"), render)
                .group(SettingsGroup::User)
                .order(30)
                .keywords(t!("encryption-settings-keywords").split(", "))
                .row(
                    SettingsRow::new("encryption-device", t!("encryption-settings-device"))
                        .description(t!("encryption-settings-device-help"))
                        .keywords(t!("encryption-settings-device-keywords")),
                )
                .row(
                    SettingsRow::new("encryption-devices", t!("encryption-settings-devices"))
                        .keywords(t!("encryption-settings-devices-keywords")),
                )
                .row(
                    SettingsRow::new("encryption-fresh", t!("encryption-settings-fresh"))
                        .description(t!("encryption-settings-fresh-help"))
                        .keywords(t!("encryption-settings-fresh-keywords")),
                ),
        );
    });
}

fn render(ctx: &SettingsCtx, window: &mut Window, cx: &mut App) -> AnyElement {
    let theme = theme::get(cx).clone();
    let Some(client) = ctx.session.read(cx).client().cloned() else {
        return div().into_any_element();
    };
    if let Some(engine) = engine(&ctx.session, cx) {
        engine.poke();
    }
    let w = client.read();
    let offered = w.capabilities.encryption.is_some();
    let state = w.e2ee.device.clone();
    let fingerprint = w.e2ee.fingerprint.clone();
    let devices = w.e2ee.devices.clone();
    let waiting_approvals = w.e2ee.waiting_approvals;
    let approving = w.e2ee.approving.clone();
    let error = w.e2ee.errors.get("").cloned();
    drop(w);
    let mut page = div()
        .id("encryption-settings")
        .test_support()
        .flex()
        .flex_col()
        .gap_3()
        .child(
            div()
                .id("encryption-settings-experimental")
                .test_support()
                .role(Role::Note)
                .text_sm()
                .text_color(theme.warning)
                .child(t!("encryption-settings-experimental")),
        );
    if !offered {
        return page
            .child(
                div()
                    .id("encryption-settings-off")
                    .test_support()
                    .role(Role::Status)
                    .text_sm()
                    .text_color(theme.muted)
                    .child(t!("encryption-settings-off")),
            )
            .into_any_element();
    }
    page = page.child(row(
        ctx,
        "encryption-device",
        t!("encryption-settings-device"),
        Some(status_text(&state).into()),
        device_control(&ctx.session, &state, &theme),
        &theme,
    ));
    if let DeviceState::Waiting { qr, code } = &state {
        page = page.child(waiting(qr, code, &theme));
    }
    if let Some(e) = error {
        page = page.child(
            div()
                .id("encryption-settings-error")
                .test_support()
                .role(Role::Alert)
                .text_sm()
                .text_color(theme.danger)
                .child(t!("encryption-error", error = e)),
        );
    }
    if matches!(state, DeviceState::Ready { approver: true }) {
        page = page.child(approve_block(
            ctx,
            waiting_approvals,
            &approving,
            window,
            cx,
        ));
    }
    if let Some(f) = fingerprint {
        page = page.child(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .text_sm()
                        .text_color(theme.text)
                        .child(t!("encryption-settings-fingerprint")),
                )
                .child(
                    div()
                        .id("encryption-settings-fingerprint")
                        .test_support()
                        .role(Role::Note)
                        .aria_label(SharedString::from(f.clone()))
                        .font_family(theme.mono_font.clone())
                        .text_sm()
                        .text_color(theme.muted)
                        .child(f),
                ),
        );
    }
    page = page.child(
        div()
            .text_sm()
            .text_color(theme.muted)
            .child(t!("encryption-settings-keys-only")),
    );
    if state.ready() {
        page = page.child(block(
            ctx,
            "encryption-devices",
            t!("encryption-settings-devices"),
            device_list(&ctx.session, &devices, &theme),
            &theme,
        ));
        page = page.child(
            div()
                .text_xs()
                .text_color(theme.muted)
                .child(t!("encryption-settings-approver-only")),
        );
    }
    if matches!(state, DeviceState::Ready { .. }) {
        let session = ctx.session.clone();
        page = page.child(row(
            ctx,
            "encryption-fresh",
            t!("encryption-settings-fresh"),
            Some(t!("encryption-settings-fresh-help").into()),
            Button::new("encryption-settings-fresh-button")
                .small()
                .danger()
                .label(t!("encryption-settings-fresh"))
                .on_click(move |_, window, cx| ask_fresh(&session, window, cx)),
            &theme,
        ));
    }
    page.into_any_element()
}

fn status_text(state: &DeviceState) -> String {
    match state {
        DeviceState::Off => t!("encryption-settings-off").into(),
        DeviceState::Starting => t!("encryption-settings-starting").into(),
        DeviceState::Ready { approver: true } => t!("encryption-settings-ready-approver").into(),
        DeviceState::Ready { approver: false } => t!("encryption-settings-ready").into(),
        DeviceState::Waiting { .. } => t!("encryption-settings-waiting").into(),
        DeviceState::ResetElsewhere => t!("encryption-settings-reset").into(),
        DeviceState::OtherServer => t!("encryption-settings-other-server").into(),
        DeviceState::Failed(e) => t!("encryption-settings-failed", error = e),
    }
}

fn device_control(session: &Entity<Session>, state: &DeviceState, theme: &Theme) -> AnyElement {
    let session = session.clone();
    match state {
        DeviceState::ResetElsewhere | DeviceState::Failed(_) => {
            Button::new("encryption-settings-again")
                .small()
                .label(t!("encryption-settings-set-up-again"))
                .on_click(move |_, _, cx| {
                    if let Some(engine) = engine(&session, cx) {
                        engine.set_up_again();
                    }
                })
                .into_any_element()
        }
        DeviceState::Ready { .. } => div()
            .id("encryption-settings-state")
            .test_support()
            .role(Role::Status)
            .text_sm()
            .text_color(theme.online)
            .child(t!("encryption-settings-on"))
            .into_any_element(),
        _ => div().into_any_element(),
    }
}

/// What a new device shows while it waits: the QR code to scan and the
/// code to type on the approving device.
fn waiting(qr: &str, code: &str, theme: &Theme) -> impl IntoElement {
    div()
        .id("encryption-settings-waiting")
        .test_support()
        .flex()
        .gap_4()
        .items_start()
        .p_3()
        .rounded(theme.radius)
        .border_1()
        .border_color(theme.border)
        .child(qr_code(
            "encryption-settings-qr",
            qr,
            t!("encryption-settings-qr"),
            3.0,
        ))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap_2()
                .text_sm()
                .text_color(theme.text)
                .child(t!("encryption-settings-waiting-how"))
                .child(
                    div()
                        .text_color(theme.muted)
                        .child(t!("encryption-settings-waiting-qr")),
                )
                .child(
                    div()
                        .id("encryption-settings-code")
                        .test_support()
                        .role(Role::Status)
                        .aria_label(SharedString::from(code.to_string()))
                        .font_family(theme.mono_font.clone())
                        .text_xl()
                        .text_color(theme.text)
                        .child(code.to_string()),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.warning)
                        .child(t!("encryption-settings-waiting-secret")),
                )
                .child(
                    div()
                        .text_color(theme.muted)
                        .child(t!("encryption-settings-waiting-answer")),
                ),
        )
}

/// On a device that can approve others: the code field, how many devices
/// wait, and how the last approval went.
fn approve_block(
    ctx: &SettingsCtx,
    waiting: usize,
    approving: &Approving,
    window: &mut Window,
    cx: &mut App,
) -> impl IntoElement {
    let theme = theme::get(cx).clone();
    let form = form(&ctx.session, window, cx);
    let count = match waiting {
        0 => t!("encryption-settings-approve-none").to_string(),
        1 => t!("encryption-settings-approve-waiting-one").to_string(),
        n => t!("encryption-settings-approve-waiting", count = n).to_string(),
    };
    let outcome = match approving {
        Approving::Idle => None,
        Approving::Busy => Some((
            t!("encryption-settings-approve-busy").to_string(),
            theme.muted,
            Role::Status,
        )),
        Approving::Done => Some((
            t!("encryption-settings-approve-done").to_string(),
            theme.online,
            Role::Status,
        )),
        Approving::Failed(f) => Some((
            match f {
                ApprovalFailure::BadCode => t!("encryption-settings-approve-bad-code").to_string(),
                ApprovalFailure::NotFound => {
                    t!("encryption-settings-approve-not-found").to_string()
                }
                ApprovalFailure::Other(e) => {
                    t!("encryption-settings-approve-failed", error = e).to_string()
                }
            },
            theme.danger,
            Role::Alert,
        )),
    };
    div()
        .id("encryption-approve")
        .test_support()
        .role(Role::Group)
        .aria_label(t!("encryption-settings-approve-title"))
        .flex()
        .flex_col()
        .gap_2()
        .p_3()
        .rounded(theme.radius)
        .border_1()
        .border_color(theme.border)
        .child(
            div()
                .text_color(theme.text)
                .child(t!("encryption-settings-approve-title")),
        )
        .child(
            div()
                .text_sm()
                .text_color(theme.muted)
                .child(t!("encryption-settings-approve-help")),
        )
        .child(
            div()
                .id("encryption-approve-waiting")
                .test_support()
                .role(Role::Status)
                .aria_label(SharedString::from(count.clone()))
                .text_sm()
                .text_color(if waiting > 0 { theme.text } else { theme.muted })
                .child(count),
        )
        .child(form)
        .children(outcome.map(|(text, color, role)| {
            div()
                .id("encryption-approve-outcome")
                .test_support()
                .role(role)
                .aria_label(SharedString::from(text.clone()))
                .text_sm()
                .text_color(color)
                .child(text)
        }))
}

/// The approval code field, one per session.
struct ApproveForm {
    session: Entity<Session>,
    code: Entity<InputState>,
    _subscriptions: Vec<Subscription>,
}

impl ApproveForm {
    fn new(session: Entity<Session>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let code = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t!("encryption-settings-approve-placeholder"))
        });
        let subscriptions = vec![cx.subscribe_in(
            &code,
            window,
            |this: &mut Self, _, event, window, cx| match event {
                InputEvent::PressEnter { .. } => this.approve(window, cx),
                InputEvent::Change => cx.notify(),
                _ => {}
            },
        )];
        Self {
            session,
            code,
            _subscriptions: subscriptions,
        }
    }

    fn approve(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let typed = self.code.read(cx).value().to_string();
        if typed.trim().is_empty() {
            return;
        }
        if let Some(engine) = engine(&self.session, cx) {
            engine.approve(&typed);
        }
        self.code
            .update(cx, |field, cx| field.set_value("", window, cx));
    }
}

impl Render for ApproveForm {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let empty = self.code.read(cx).value().trim().is_empty();
        div()
            .flex()
            .gap_2()
            .items_center()
            .child(
                div().flex_1().min_w_0().child(
                    Input::new(&self.code)
                        .id("encryption-approve-code")
                        .aria_label(t!("encryption-settings-approve-placeholder"))
                        .small(),
                ),
            )
            .child(
                Button::new("encryption-approve-button")
                    .small()
                    .primary()
                    .label(t!("encryption-settings-approve"))
                    .disabled(empty)
                    .on_click(cx.listener(|this, _, window, cx| this.approve(window, cx))),
            )
    }
}

#[derive(Default)]
struct Forms(HashMap<EntityId, Entity<ApproveForm>>);
impl Global for Forms {}

fn form(session: &Entity<Session>, window: &mut Window, cx: &mut App) -> Entity<ApproveForm> {
    let key = session.entity_id();
    if let Some(form) = cx.default_global::<Forms>().0.get(&key) {
        return form.clone();
    }
    let form = cx.new(|cx| ApproveForm::new(session.clone(), window, cx));
    cx.default_global::<Forms>().0.insert(key, form.clone());
    form
}

fn device_list(
    session: &Entity<Session>,
    devices: &[ircord_client::store::e2ee::OwnDevice],
    theme: &Theme,
) -> impl IntoElement {
    div()
        .id("encryption-devices-list")
        .test_support()
        .role(Role::List)
        .flex()
        .flex_col()
        .gap_1()
        .children(devices.iter().map(|d| {
            let label = if d.label.is_empty() {
                t!("encryption-settings-unnamed").to_string()
            } else {
                d.label.clone()
            };
            let state = if d.this {
                t!("encryption-settings-device-this")
            } else if d.approved {
                t!("encryption-settings-device-approved")
            } else {
                t!("encryption-settings-device-unapproved")
            };
            let (session, id) = (session.clone(), d.id.clone());
            div()
                .id(SharedString::from(format!("encryption-device-{}", d.id)))
                .test_support()
                .role(Role::ListItem)
                .aria_label(SharedString::from(format!("{label}, {state}")))
                .flex()
                .items_center()
                .justify_between()
                .gap_2()
                .py_1()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .child(div().text_sm().text_color(theme.text).child(label))
                        .child(div().text_xs().text_color(theme.muted).child(state)),
                )
                .when(!d.this, |row| {
                    row.child(
                        Button::new(SharedString::from(format!(
                            "encryption-device-remove-{}",
                            d.id
                        )))
                        .small()
                        .label(t!("encryption-settings-remove"))
                        .on_click(move |_, _, cx| {
                            if let Some(engine) = engine(&session, cx) {
                                engine.remove_device(&id);
                            }
                        }),
                    )
                })
        }))
}

fn ask_fresh(session: &Entity<Session>, window: &mut Window, cx: &mut App) {
    let session = session.clone();
    confirm(
        window,
        cx,
        Confirm {
            title: t!("encryption-settings-fresh-title").into(),
            body: t!("encryption-settings-fresh-body").into(),
            ok: t!("encryption-settings-fresh-ok").into(),
            danger: true,
        },
        move |_, cx| {
            if let Some(engine) = engine(&session, cx) {
                engine.start_fresh();
            }
        },
    );
}

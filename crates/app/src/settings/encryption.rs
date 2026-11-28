//! Settings → Encryption (experimental): this device's end-to-end
//! encryption. A device set up shows the account's fingerprint, its devices
//! and requests from new ones to approve, each with the code both devices
//! show; a new device shows its QR code and, once another device answered,
//! the code to compare. Starting fresh is here too, behind a confirmation
//! that says what follows. There is no recovery key: the page says so.

use gpui_kit::TestSupportExt;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::{Disableable, Sizable};
use gpui_kit::prelude::*;
use gpui_kit::*;
use ircord_client::store::e2ee::DeviceState;

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

fn render(ctx: &SettingsCtx, _window: &mut Window, cx: &mut App) -> AnyElement {
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
    let requests = w.e2ee.requests.clone();
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
    if let DeviceState::Waiting {
        qr,
        code,
        confirmed,
    } = &state
    {
        page = page.child(waiting(
            &ctx.session,
            qr,
            code.as_deref(),
            *confirmed,
            &theme,
        ));
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
    for request in &requests {
        page = page.child(request_card(&ctx.session, request, &theme));
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

/// What a new device shows while it waits: the QR code, then the code.
fn waiting(
    session: &Entity<Session>,
    qr: &str,
    code: Option<&str>,
    confirmed: bool,
    theme: &Theme,
) -> impl IntoElement {
    let (s1, s2) = (session.clone(), session.clone());
    // Once the other device answered: the person confirms here too that the
    // codes match, or starts over.
    let answer = code.is_some().then(|| {
        if confirmed {
            div()
                .text_color(theme.muted)
                .child(t!("encryption-settings-waiting-approve"))
                .into_any_element()
        } else {
            div()
                .flex()
                .gap_2()
                .child(
                    Button::new("encryption-settings-codes-match")
                        .small()
                        .primary()
                        .label(t!("encryption-settings-codes-match"))
                        .on_click(move |_, _, cx| {
                            if let Some(engine) = engine(&s1, cx) {
                                engine.confirm_code(true);
                            }
                        }),
                )
                .child(
                    Button::new("encryption-settings-codes-differ")
                        .small()
                        .label(t!("encryption-settings-codes-differ"))
                        .on_click(move |_, _, cx| {
                            if let Some(engine) = engine(&s2, cx) {
                                engine.confirm_code(false);
                            }
                        }),
                )
                .into_any_element()
        }
    });
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
                .child(match code {
                    Some(code) => div()
                        .id("encryption-settings-code")
                        .test_support()
                        .role(Role::Status)
                        .aria_label(SharedString::from(code.to_string()))
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(t!("encryption-settings-waiting-code"))
                        .child(
                            div()
                                .font_family(theme.mono_font.clone())
                                .text_xl()
                                .text_color(theme.text)
                                .child(code.to_string()),
                        ),
                    None => div()
                        .id("encryption-settings-code")
                        .test_support()
                        .role(Role::Status)
                        .text_color(theme.muted)
                        .child(t!("encryption-settings-waiting-answer")),
                })
                .children(answer),
        )
}

fn request_card(
    session: &Entity<Session>,
    request: &ircord_client::store::e2ee::Request,
    theme: &Theme,
) -> impl IntoElement {
    let (s1, s2) = (session.clone(), session.clone());
    let (d1, d2) = (request.device.clone(), request.device.clone());
    // Approves only the code the person saw.
    let shown = request.code.clone();
    let label = if request.label.is_empty() {
        t!("encryption-settings-unnamed").to_string()
    } else {
        request.label.clone()
    };
    div()
        .id(SharedString::from(format!(
            "encryption-request-{}",
            request.device
        )))
        .test_support()
        .role(Role::Group)
        .aria_label(SharedString::from(t!(
            "encryption-settings-request",
            label = label.clone()
        )))
        .flex()
        .flex_col()
        .gap_2()
        .p_3()
        .rounded(theme.radius)
        .border_1()
        .border_color(theme.accent)
        .bg(theme.accent.opacity(0.08))
        .child(
            div()
                .text_color(theme.text)
                .child(t!("encryption-settings-request", label = label)),
        )
        .child(match &request.code {
            Some(code) => div()
                .flex()
                .flex_col()
                .gap_1()
                .text_sm()
                .text_color(theme.text)
                .child(t!("encryption-settings-request-code"))
                .child(
                    div()
                        .id("encryption-request-code")
                        .test_support()
                        .role(Role::Status)
                        .aria_label(SharedString::from(code.clone()))
                        .font_family(theme.mono_font.clone())
                        .text_xl()
                        .child(code.clone()),
                ),
            None => div()
                .text_sm()
                .text_color(theme.muted)
                .child(t!("encryption-settings-request-waiting")),
        })
        .child(
            div()
                .flex()
                .gap_2()
                .child(
                    Button::new("encryption-request-approve")
                        .small()
                        .primary()
                        .label(t!("encryption-settings-approve"))
                        .disabled(request.code.is_none())
                        .on_click(move |_, _, cx| {
                            if let Some(engine) = engine(&s1, cx) {
                                engine.answer(&d1, shown.as_deref(), true);
                            }
                        }),
                )
                .child(
                    Button::new("encryption-request-decline")
                        .small()
                        .label(t!("encryption-settings-decline"))
                        .on_click(move |_, _, cx| {
                            if let Some(engine) = engine(&s2, cx) {
                                engine.answer(&d2, None, false);
                            }
                        }),
                ),
        )
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

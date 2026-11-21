//! Settings → This device (package DK1): whether IRCord opens `irc://`
//! links, notifications on this device (S3's set-up, and a test), and the
//! unread mark on the tray icon. The web client keeps the first under
//! Networks; a desktop app's link handler belongs to the device.

use gpui_kit::TestSupportExt;
use gpui_kit::component::Disableable;
use gpui_kit::component::Sizable;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::switch::Switch;
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::notifications::{DeviceStatus, device_status};
use super::row;
use crate::features::desktop::{self, links::Handler};
use crate::registry::{self, SettingsCtx, SettingsGroup, SettingsRow, SettingsSection};
use crate::theme::{self, Theme};
use crate::tray;

pub fn init(cx: &mut App) {
    registry::register(cx, |r| {
        r.settings_sections.add(
            SettingsSection::new("device", t!("desktop-device-title"), render)
                .group(SettingsGroup::App)
                .order(35)
                .keywords(t!("desktop-device-keywords").split(", "))
                .row(
                    SettingsRow::new("desktop-irc-links", t!("desktop-links"))
                        .description(t!("desktop-links-help"))
                        .keywords(t!("desktop-links-keywords")),
                )
                .row(
                    SettingsRow::new("desktop-notifications", t!("desktop-notifications"))
                        .description(t!("desktop-notifications-help"))
                        .keywords(t!("desktop-notifications-keywords")),
                )
                .row(
                    SettingsRow::new("desktop-tray-mark", t!("desktop-tray-mark"))
                        .description(t!("desktop-tray-mark-help"))
                        .keywords(t!("desktop-tray-mark-keywords")),
                ),
        );
    });
}

/// Who opens `irc://` links, as last read from the system.
#[derive(Default)]
struct Links {
    handler: Option<Result<Handler, String>>,
    /// Reading or registering is under way.
    busy: bool,
    /// Why registering failed.
    error: Option<String>,
}
impl Global for Links {}

/// Read who opens `irc://` links, off the UI thread; with `register`, make
/// IRCord the handler first.
fn refresh(register: bool, cx: &mut App) {
    let links = cx.default_global::<Links>();
    if links.busy {
        return;
    }
    links.busy = true;
    if register {
        links.error = None;
    }
    let task = cx.background_spawn(async move {
        let registered = if register {
            desktop::links::register().map_err(|e| e.to_string())
        } else {
            Ok(())
        };
        (
            registered,
            desktop::links::handler().map_err(|e| e.to_string()),
        )
    });
    cx.spawn(async move |cx| {
        let (registered, handler) = task.await;
        cx.update(|cx| {
            let links = cx.global_mut::<Links>();
            links.busy = false;
            links.error = registered.err();
            links.handler = Some(handler);
            cx.refresh_windows();
        });
    })
    .detach();
}

fn render(ctx: &SettingsCtx, _window: &mut Window, cx: &mut App) -> AnyElement {
    if cx.default_global::<Links>().handler.is_none() {
        refresh(false, cx);
    }
    let theme = theme::get(cx).clone();
    div()
        .id("desktop-settings")
        .test_support()
        .flex()
        .flex_col()
        .gap_3()
        .child(row(
            ctx,
            "desktop-irc-links",
            t!("desktop-links"),
            Some(t!("desktop-links-help").into()),
            links_control(cx, &theme),
            &theme,
        ))
        .child(row(
            ctx,
            "desktop-notifications",
            t!("desktop-notifications"),
            Some(notification_status(ctx, cx).into()),
            notification_control(ctx),
            &theme,
        ))
        .child(row(
            ctx,
            "desktop-tray-mark",
            t!("desktop-tray-mark"),
            Some(if tray::installed(cx) {
                t!("desktop-tray-mark-help").into()
            } else {
                t!("desktop-tray-missing").into()
            }),
            tray_control(ctx, cx),
            &theme,
        ))
        .into_any_element()
}

fn links_control(cx: &App, theme: &Theme) -> impl IntoElement + use<> {
    let links = cx.global::<Links>();
    // The text, whether there is nothing to do, and whether IRCord opens them.
    let (state, settled, ours): (SharedString, bool, bool) = match &links.handler {
        _ if links.busy => (t!("desktop-links-checking").into(), false, false),
        None => (t!("desktop-links-checking").into(), false, false),
        Some(Ok(Handler::Ours)) => (t!("desktop-links-ours").into(), true, true),
        Some(Ok(Handler::Bundle)) => (t!("desktop-links-bundle").into(), true, true),
        Some(Ok(Handler::Unavailable)) => (t!("desktop-links-unavailable").into(), true, false),
        Some(Ok(Handler::Other(app))) => {
            (t!("desktop-links-other", app = app).into(), false, false)
        }
        Some(Ok(Handler::None)) => (t!("desktop-links-none").into(), false, false),
        Some(Err(e)) => (t!("desktop-links-unknown", error = e).into(), false, false),
    };
    let error = links
        .error
        .clone()
        .map(|e| SharedString::from(t!("desktop-links-failed", error = e)));
    // Narrow, so the label beside it keeps its room; long text wraps.
    let mut column = div()
        .w(px(240.))
        .flex()
        .flex_col()
        .items_end()
        .gap_1()
        .child(
            div()
                .w_full()
                .flex()
                .flex_col()
                .items_end()
                .gap_1()
                .child(
                    div()
                        .id("desktop-irc-links-state")
                        .test_support()
                        .role(Role::Status)
                        .aria_label(state.clone())
                        .text_sm()
                        .text_right()
                        .text_color(if ours { theme.online } else { theme.muted })
                        .child(state),
                )
                .when(!settled, |this| {
                    this.child(
                        Button::new("desktop-irc-links-use")
                            .small()
                            .primary()
                            .label(t!("desktop-links-use"))
                            .disabled(links.busy)
                            .on_click(|_, _, cx| refresh(true, cx)),
                    )
                }),
        );
    if let Some(error) = error {
        column = column.child(
            div()
                .id("desktop-irc-links-error")
                .test_support()
                .role(Role::Alert)
                .aria_label(error.clone())
                .text_sm()
                .text_right()
                .text_color(theme.danger)
                .child(error),
        );
    }
    column
}

/// What S3's device record says about notifications here.
fn notification_status(ctx: &SettingsCtx, cx: &App) -> &'static str {
    let status = ctx
        .session
        .read(cx)
        .client()
        .map(|c| device_status(&c.read().notify_settings));
    match status {
        Some(DeviceStatus::Open) => t!("desktop-notifications-on"),
        Some(DeviceStatus::Off) => t!("desktop-notifications-off"),
        _ => t!("desktop-notifications-checking"),
    }
}

fn notification_control(ctx: &SettingsCtx) -> impl IntoElement + use<> {
    let session = ctx.session.clone();
    div()
        .flex()
        .gap_2()
        .child(
            Button::new("desktop-notifications-test")
                .small()
                .label(t!("desktop-notifications-test"))
                .on_click(|_, _, cx| {
                    cx.show_system_notification(SystemNotification {
                        tag: "ircord-device-test".into(),
                        title: t!("core-app-name").into(),
                        body: t!("desktop-notifications-test-body").into(),
                        actions: Vec::new(),
                    });
                }),
        )
        .child(
            Button::new("desktop-notifications-set-up")
                .small()
                .label(t!("desktop-notifications-set-up"))
                .on_click(move |_, _, cx| {
                    super::open(&session, Some("notifications-device"), cx);
                }),
        )
}

fn tray_control(ctx: &SettingsCtx, cx: &App) -> impl IntoElement + use<> {
    let on = desktop::tray_mark_shown(cx);
    let session = ctx.session.clone();
    Switch::new("desktop-tray-mark-switch")
        .checked(on)
        .accessibility_label(t!("desktop-tray-mark"))
        .on_click(move |on, _, cx| {
            desktop::set_tray_mark(&session, *on, cx);
            super::saved(cx);
            cx.refresh_windows();
        })
}

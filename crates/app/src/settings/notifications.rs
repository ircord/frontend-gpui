//! Settings → Notifications (package S3), as the web client's
//! `NotificationSettings.vue`, `NotificationRouting.vue`,
//! `NotificationDevice.vue` and `DeviceNotificationPreferences.vue`:
//!
//! - This device: whether it shows notifications, a test notification, and
//!   its own choices (what it notifies about, its sound, previews).
//! - Across your devices: when the last-used device and then every device is
//!   alerted, and auto-away.
//! - Quiet hours, in this device's time zone.
//! - Notify me about: the channel and direct-message defaults, your own
//!   levels for conversations, and highlight words.
//! - Your other devices and their choices.
//!
//! The account-wide rules are the server's notification settings document
//! (edited through `ircord_client::store::notify_settings`); the device
//! choices are each device's record (`/auth/devices/{id}/notifications`).
//! The server routes every alert, so nothing here decides what shows; how an
//! alert shows on this device is `notify::plan`, and its chime plays through
//! `media::sound`.

use crate::ui::focus::Reachable;
use gpui_kit::TestSupportExt;
use gpui_kit::component::Disableable;
use gpui_kit::component::Sizable;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::switch::Switch;
use gpui_kit::prelude::*;
use gpui_kit::*;
use ircord_client::Client;
use ircord_client::api::notifications::DeviceSaveError;
use ircord_client::client::Handle;
use ircord_client::contract::{
    Device, DeviceNotificationsContent as Content, DeviceNotificationsDelivery as Delivery,
    DeviceNotificationsSound as Sound, Settings,
};
use ircord_client::documents::NOTIFICATION_SETTINGS;
use ircord_client::store::notify_settings::{self as ns, DeviceChange, Level, Unit, WordProblem};
use ircord_client::store::topic;
use std::rc::Rc;

use super::read_aloud::ReadAloudView;
use super::{block, row};
use crate::registry::{self, SettingsCtx, SettingsGroup, SettingsRow, SettingsSection};
use crate::session::{Session, SessionEvent};
use crate::theme::{self, Theme};

pub fn init(cx: &mut App) {
    // Read the devices and set this one up as soon as a client starts:
    // the server alerts no device that was never set up.
    cx.observe_new::<Session>(|_, _, cx| {
        cx.subscribe_self(|session: &mut Session, event: &SessionEvent, _| {
            if let SessionEvent::Changed(changes) = event
                && changes.reset
                && let Some(client) = session.client()
            {
                ns::keep(client);
            }
        })
        .detach();
    })
    .detach();
    registry::register(cx, |r| {
        let row = |id: &'static str, label: &'static str, description: Option<&'static str>| {
            let mut row = SettingsRow::new(id, label).keywords(t!("notifications-keywords"));
            if let Some(d) = description {
                row = row.description(d);
            }
            row
        };
        r.settings_sections.add(
            SettingsSection::new(
                "notifications",
                t!("notifications-title"),
                |ctx, window, cx| {
                    let view = view(ctx, window, cx);
                    view.update(cx, |v, _| v.ctx = Some(ctx.clone()));
                    view.into_any_element()
                },
            )
            .group(SettingsGroup::App)
            .order(30)
            .keywords(t!("notifications-keywords").split(", "))
            .row(
                SettingsRow::new("read-aloud", t!("speech-read-aloud"))
                    .description(t!("speech-service-help"))
                    .keywords(t!("speech-keywords")),
            )
            .row(row(
                "notifications-status",
                t!("notifications-device-title"),
                Some(t!("notifications-device-search")),
            ))
            .row(row(
                "notifications-device",
                t!("notifications-device-delivery"),
                None,
            ))
            .row(row(
                "notifications-test",
                t!("notifications-test"),
                Some(t!("notifications-test-help")),
            ))
            .row(row(
                "notification-sound",
                t!("notifications-device-choices"),
                Some(t!("notifications-device-choices-search")),
            ))
            .row(row(
                "notify-last-device",
                t!("notifications-last-device"),
                Some(t!("notifications-last-device-help")),
            ))
            .row(row(
                "notify-all-devices",
                t!("notifications-all-devices"),
                Some(t!("notifications-all-devices-help")),
            ))
            .row(row("notify-reset", t!("notifications-reset"), None))
            .row(
                row("auto-away", t!("notifications-auto-away"), None)
                    .keywords(t!("notifications-auto-away-keywords")),
            )
            .row(
                row(
                    "quiet-hours",
                    t!("notifications-quiet"),
                    Some(t!("notifications-quiet-search")),
                )
                .keywords(t!("notifications-quiet-keywords")),
            )
            .row(row(
                "channel-notifications",
                t!("notifications-channels"),
                Some(t!("notifications-channels-help")),
            ))
            .row(row(
                "direct-notifications",
                t!("notifications-direct"),
                None,
            ))
            .row(row(
                "conversation-levels",
                t!("notifications-own-levels"),
                Some(t!("notifications-own-levels-help")),
            ))
            .row(
                row(
                    "highlight-words",
                    t!("notifications-highlight"),
                    Some(t!("notifications-highlight-search")),
                )
                .keywords(t!("notifications-highlight-keywords")),
            )
            .row(row(
                "device-notifications",
                t!("notifications-other-devices"),
                Some(t!("notifications-other-devices-help")),
            )),
        );
    });
}

/// The view, kept while the session lasts.
struct Notifications(Entity<NotificationsView>);
impl Global for Notifications {}

fn view(ctx: &SettingsCtx, window: &mut Window, cx: &mut App) -> Entity<NotificationsView> {
    if let Some(Notifications(view)) = cx.try_global::<Notifications>()
        && view.read(cx).session == ctx.session
    {
        return view.clone();
    }
    let view = cx.new(|cx| NotificationsView::new(ctx.session.clone(), window, cx));
    cx.set_global(Notifications(view.clone()));
    view
}

/// A text field that edits one number or time in the document.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Field {
    LastDevice,
    AllDevices,
    Away,
    QuietStart,
    QuietEnd,
}

impl Field {
    const EVERY: [Field; 5] = [
        Field::LastDevice,
        Field::AllDevices,
        Field::Away,
        Field::QuietStart,
        Field::QuietEnd,
    ];

    fn id(self) -> &'static str {
        match self {
            Field::LastDevice => "notify-last-device-input",
            Field::AllDevices => "notify-all-devices-input",
            Field::Away => "auto-away-input",
            Field::QuietStart => "quiet-hours-start",
            Field::QuietEnd => "quiet-hours-end",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Field::LastDevice => t!("notifications-last-device"),
            Field::AllDevices => t!("notifications-all-devices"),
            Field::Away => t!("notifications-away-after"),
            Field::QuietStart => t!("notifications-quiet-from"),
            Field::QuietEnd => t!("notifications-quiet-until"),
        }
    }

    /// What the field shows for the stored settings.
    fn shown(self, s: &Settings) -> String {
        match self {
            Field::LastDevice => short_duration(s.last_device_delay),
            Field::AllDevices => short_duration(s.all_devices_delay),
            Field::Away => short_duration(s.auto_away_delay),
            Field::QuietStart => s.quiet_hours.start.clone(),
            Field::QuietEnd => s.quiet_hours.end.clone(),
        }
    }

    /// The settings with `text` typed into the field, or `None` when it
    /// does not read as a duration or a time.
    fn apply(self, s: &Settings, text: &str) -> Option<Settings> {
        let zone = ns::local_time_zone();
        Some(match self {
            Field::LastDevice => ns::set_last_device(s, ns::parse_duration(text)?),
            Field::AllDevices => ns::set_all_devices(s, ns::parse_duration(text)?),
            Field::Away => ns::set_auto_away_delay(s, ns::parse_duration(text)?),
            Field::QuietStart => ns::set_quiet(s, None, Some(&ns::parse_time(text)?), None, &zone),
            Field::QuietEnd => ns::set_quiet(s, None, None, Some(&ns::parse_time(text)?), &zone),
        })
    }
}

pub struct NotificationsView {
    session: Entity<Session>,
    /// The settings context it was last drawn in, for the landing row.
    ctx: Option<SettingsCtx>,
    fields: Vec<(Field, Entity<InputState>)>,
    /// A field whose text did not read, until it is typed into again.
    invalid: Option<Field>,
    word: Entity<InputState>,
    word_problem: Option<WordProblem>,
    /// The last test notification: whether it was shown, and what to say.
    tested: Option<bool>,
    read_aloud: Entity<ReadAloudView>,
    _subscriptions: Vec<Subscription>,
}

impl NotificationsView {
    fn new(session: Entity<Session>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut subscriptions = vec![cx.subscribe(&session, |_, _, event: &SessionEvent, cx| {
            if let SessionEvent::Changed(changes) = event
                && (changes.touches(ns::TOPIC) || changes.touches(topic::DOCUMENTS))
            {
                cx.notify();
            }
        })];
        let mut fields = Vec::new();
        for field in Field::EVERY {
            let input = cx.new(|cx| InputState::new(window, cx));
            subscriptions.push(cx.subscribe_in(
                &input,
                window,
                move |this: &mut Self, _, event, window, cx| match event {
                    InputEvent::PressEnter { .. } | InputEvent::Blur => {
                        this.commit(field, window, cx)
                    }
                    InputEvent::Change if this.invalid == Some(field) => {
                        this.invalid = None;
                        cx.notify();
                    }
                    _ => {}
                },
            ));
            fields.push((field, input));
        }
        let word = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t!("notifications-highlight-placeholder"))
        });
        subscriptions.push(cx.subscribe_in(
            &word,
            window,
            |this: &mut Self, _, event, window, cx| match event {
                InputEvent::PressEnter { .. } => this.add_word(window, cx),
                InputEvent::Change if this.word_problem.is_some() => {
                    this.word_problem = None;
                    cx.notify();
                }
                _ => {}
            },
        ));
        let read_aloud = cx.new(|cx| ReadAloudView::new(session.clone(), window, cx));
        Self {
            session,
            ctx: None,
            fields,
            invalid: None,
            word,
            word_problem: None,
            tested: None,
            read_aloud,
            _subscriptions: subscriptions,
        }
    }

    fn client(&self, cx: &App) -> Option<Rc<Client>> {
        self.session.read(cx).client().cloned()
    }

    fn input(&self, field: Field) -> &Entity<InputState> {
        &self.fields.iter().find(|(f, _)| *f == field).unwrap().1
    }

    fn commit(&mut self, field: Field, window: &mut Window, cx: &mut Context<Self>) {
        let Some(client) = self.client(cx) else {
            return;
        };
        let Some(current) = settings(&client) else {
            return;
        };
        let input = self.input(field).clone();
        let text = input.read(cx).value().to_string();
        if text == field.shown(&current) {
            return;
        }
        match field.apply(&current, &text) {
            Some(next) => {
                self.invalid = None;
                input.update(cx, |i, cx| i.set_value(field.shown(&next), window, cx));
                if next != current {
                    client.edit_document(&NOTIFICATION_SETTINGS, next);
                }
            }
            None => self.invalid = Some(field),
        }
        cx.notify();
    }

    fn add_word(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(client) = self.client(cx) else {
            return;
        };
        let Some(current) = settings(&client) else {
            return;
        };
        let text = self.word.read(cx).value().to_string();
        match ns::add_highlight(&current, &text) {
            Ok(next) => {
                self.word_problem = None;
                self.word.update(cx, |i, cx| i.set_value("", window, cx));
                client.edit_document(&NOTIFICATION_SETTINGS, next);
            }
            Err(problem) => self.word_problem = Some(problem),
        }
        cx.notify();
    }

    /// Show each field's stored value unless it is being typed into.
    fn sync_fields(&self, s: &Settings, window: &mut Window, cx: &mut Context<Self>) {
        for (field, input) in &self.fields {
            if self.invalid == Some(*field) || input.read(cx).focus_handle(cx).is_focused(window) {
                continue;
            }
            let shown = field.shown(s);
            if input.read(cx).value().as_ref() != shown {
                input.update(cx, |i, cx| i.set_value(shown, window, cx));
            }
        }
    }
}

/// The settings document, bounded, once read.
fn settings(client: &Client) -> Option<Settings> {
    client
        .read()
        .documents
        .notification_settings
        .value()
        .map(ns::sanitize)
}

/// Save an edit of the settings document, if it was read (a save replaces
/// the whole document).
fn edit(session: &Entity<Session>, cx: &App, f: impl FnOnce(&Settings) -> Settings) {
    let Some(client) = session.read(cx).client().cloned() else {
        return;
    };
    let Some(current) = settings(&client) else {
        return;
    };
    let next = f(&current);
    if next != current {
        client.edit_document(&NOTIFICATION_SETTINGS, next);
    }
}

fn unit(count: i64, unit: Unit) -> String {
    match (unit, count == 1) {
        (Unit::Hour, true) => t!("notifications-hour-one", count = count),
        (Unit::Hour, false) => t!("notifications-hour-other", count = count),
        (Unit::Minute, true) => t!("notifications-minute-one", count = count),
        (Unit::Minute, false) => t!("notifications-minute-other", count = count),
        (Unit::Second, true) => t!("notifications-second-one", count = count),
        (Unit::Second, false) => t!("notifications-second-other", count = count),
    }
}

/// `4 minutes 30 seconds`.
pub fn duration(seconds: i64) -> String {
    ns::duration_parts(seconds)
        .into_iter()
        .map(|(n, u)| unit(n, u))
        .collect::<Vec<_>>()
        .join(" ")
}

/// `4m 30s`, the way the fields show a duration and read one back.
pub fn short_duration(seconds: i64) -> String {
    ns::duration_parts(seconds)
        .into_iter()
        .map(|(n, u)| match u {
            Unit::Hour => format!("{n}h"),
            Unit::Minute => format!("{n}m"),
            Unit::Second => format!("{n}s"),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn level_label(level: Level) -> &'static str {
    match level {
        Level::All => t!("notifications-level-all"),
        Level::Mentions => t!("notifications-level-mentions"),
        Level::None => t!("notifications-level-none"),
    }
}

fn word_problem(problem: WordProblem) -> &'static str {
    match problem {
        WordProblem::Empty => t!("notifications-highlight-empty"),
        WordProblem::TooLong => t!("notifications-highlight-too-long"),
        WordProblem::Duplicate => t!("notifications-highlight-duplicate"),
        WordProblem::Full => t!("notifications-highlight-full"),
    }
}

/// A row of choices, one of them picked: a radio group named `label`, each
/// option `{id}-{key}`.
fn choice<T: Copy + PartialEq + 'static>(
    id: &str,
    label: impl Into<SharedString>,
    options: Vec<(T, &'static str, SharedString)>,
    picked: Option<T>,
    disabled: bool,
    theme: &Theme,
    on_pick: impl Fn(T, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let on_pick = Rc::new(on_pick);
    let mut group = div()
        .id(ElementId::Name(id.to_string().into()))
        .role(Role::RadioGroup)
        .aria_label(label.into())
        .flex()
        .flex_wrap()
        .gap_1()
        .p_0p5()
        .rounded_md()
        .bg(theme.input);
    for (value, key, text) in options {
        let selected = picked == Some(value);
        let hover = theme.hover;
        let on_pick = on_pick.clone();
        group = group.child(
            div()
                .id(ElementId::Name(format!("{id}-{key}").into()))
                .test_support()
                .role(Role::RadioButton)
                .reachable(theme)
                .aria_label(text.clone())
                .aria_selected(selected)
                .px_2()
                .py_0p5()
                .rounded_md()
                .text_sm()
                .when(selected, |d| d.bg(theme.accent).text_color(theme.on_accent))
                .when(!selected && !disabled, |d| {
                    d.text_color(theme.text)
                        .cursor_pointer()
                        .hover(move |d| d.bg(hover))
                })
                .when(disabled, |d| d.opacity(0.5).text_color(theme.muted))
                .child(text)
                .when(!disabled && !selected, |d| {
                    d.on_click(move |_, window, cx| on_pick(value, window, cx))
                }),
        );
    }
    group
}

fn levels(include_inherit: bool) -> Vec<(Option<Level>, &'static str, SharedString)> {
    let mut list = Vec::new();
    if include_inherit {
        list.push((None, "inherit", t!("notifications-content-inherit").into()));
    }
    list.extend(
        Level::EVERY
            .into_iter()
            .map(|l| (Some(l), l.as_str(), level_label(l).into())),
    );
    list
}

fn content_level(content: Content) -> Option<Level> {
    match content {
        Content::All => Some(Level::All),
        Content::Mentions => Some(Level::Mentions),
        Content::None => Some(Level::None),
        _ => None,
    }
}

fn level_content(level: Option<Level>) -> Content {
    match level {
        Some(Level::All) => Content::All,
        Some(Level::Mentions) => Content::Mentions,
        Some(Level::None) => Content::None,
        None => Content::Inherit,
    }
}

fn delivery_label(delivery: Delivery) -> &'static str {
    match delivery {
        Delivery::Open => t!("notifications-delivery-open"),
        Delivery::Push => t!("notifications-delivery-push"),
        _ => t!("notifications-delivery-off"),
    }
}

fn note(text: impl Into<SharedString>, theme: &Theme) -> Div {
    div().text_sm().text_color(theme.muted).child(text.into())
}

fn group_heading(text: &'static str, theme: &Theme) -> Div {
    div()
        .pt_2()
        .text_xs()
        .font_weight(FontWeight::BOLD)
        .text_color(theme.faint)
        .child(text.to_uppercase())
}

/// What the status card says about this device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceStatus {
    /// The device list is being read, or this device is being set up.
    Checking,
    /// The server keeps no record of this device.
    Unknown,
    Off,
    Open,
}

pub fn device_status(state: &ns::State) -> DeviceStatus {
    match state.this_device() {
        None if state.devices.is_none() && state.error.is_none() => DeviceStatus::Checking,
        None => DeviceStatus::Unknown,
        Some(d) if !d.notifications.initialized => DeviceStatus::Checking,
        Some(d) => match d.notifications.delivery {
            Delivery::Open | Delivery::Push => DeviceStatus::Open,
            _ => DeviceStatus::Off,
        },
    }
}

impl Render for NotificationsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme::get(cx).clone();
        let Some(ctx) = self.ctx.clone() else {
            return div().id("notification-settings").into_any_element();
        };
        let Some(client) = self.client(cx) else {
            return div().id("notification-settings").into_any_element();
        };
        ns::keep(&client);
        crate::sidebar::layout::watch_notification_settings(Some(client.clone()), cx);
        let (state, loaded) = {
            let w = client.read();
            (w.notify_settings.clone(), settings(&client))
        };
        let mut page = div()
            .id("notification-settings")
            .test_support()
            .flex()
            .flex_col()
            .gap_3()
            .text_sm()
            .text_color(theme.text)
            .child(self.this_device(&ctx, &state, &theme, cx));
        match loaded {
            Some(s) => {
                self.sync_fields(&s, window, cx);
                page = page
                    .child(group_heading(t!("notifications-group-devices"), &theme))
                    .children(self.routing(&ctx, &s, &theme))
                    .child(group_heading(t!("notifications-group-quiet"), &theme))
                    .children(self.quiet(&ctx, &s, &theme))
                    .child(group_heading(t!("notifications-group-about"), &theme))
                    .children(self.about(&ctx, &s, &client, &theme, cx));
            }
            None => {
                page = page.child(
                    div()
                        .id("notification-settings-loading")
                        .test_support()
                        .role(Role::Status)
                        .aria_label(t!("notifications-loading"))
                        .child(note(t!("notifications-loading"), &theme)),
                );
            }
        }
        page.child(self.other_devices(&ctx, &state, Some(client.handle().clone()), &theme))
            .child(block(
                &ctx,
                "read-aloud",
                t!("speech-read-aloud"),
                self.read_aloud.clone(),
                &theme,
            ))
            .into_any_element()
    }
}

impl NotificationsView {
    fn this_device(
        &self,
        ctx: &SettingsCtx,
        state: &ns::State,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let status = device_status(state);
        let device = state.this_device().cloned();
        let busy = device
            .as_ref()
            .is_some_and(|d| state.saving.contains(&d.id));
        let (title, detail, colour) = match status {
            DeviceStatus::Checking => (
                t!("notifications-status-checking"),
                t!("notifications-status-checking-detail"),
                theme.faint,
            ),
            DeviceStatus::Unknown => (
                t!("notifications-status-unknown"),
                t!("notifications-status-unknown-detail"),
                theme.warning,
            ),
            DeviceStatus::Off => (
                t!("notifications-status-off"),
                t!("notifications-status-off-detail"),
                theme.faint,
            ),
            DeviceStatus::Open => (
                t!("notifications-status-on"),
                t!("notifications-status-on-detail"),
                theme.online,
            ),
        };
        let handle: Option<Handle> = self.client(cx).map(|c| c.handle().clone());
        let mut card = div().flex().flex_col().gap_2().child(
            div()
                .flex()
                .items_center()
                .gap_3()
                .child(div().size(px(10.)).rounded_full().bg(colour))
                .child(
                    div()
                        .id("notifications-status-summary")
                        .test_support()
                        .role(Role::Status)
                        .aria_label(SharedString::from(title))
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(theme.text)
                                .child(title),
                        )
                        .child(note(detail, theme)),
                )
                .when(status == DeviceStatus::Off, |d| {
                    let (handle, id) = (handle.clone(), device.as_ref().map(|d| d.id.clone()));
                    d.child(
                        Button::new("notifications-enable")
                            .primary()
                            .small()
                            .label(t!("notifications-enable"))
                            .disabled(busy)
                            .on_click(move |_, _, _| {
                                if let (Some(h), Some(id)) = (&handle, &id) {
                                    ns::save_device(
                                        h,
                                        id,
                                        DeviceChange {
                                            delivery: Some(Delivery::Open),
                                            ..Default::default()
                                        },
                                    );
                                }
                            }),
                    )
                }),
        );
        if let Some(device) = &device
            && device.notifications.initialized
        {
            let id = device.id.clone();
            let h = handle.clone();
            card = card.child(row(
                ctx,
                "notifications-device",
                t!("notifications-device-delivery"),
                Some(t!("notifications-device-delivery-help").into()),
                choice(
                    "notifications-device-choice",
                    t!("notifications-device-delivery"),
                    vec![
                        (Delivery::Off, "off", delivery_label(Delivery::Off).into()),
                        (
                            Delivery::Open,
                            "open",
                            delivery_label(Delivery::Open).into(),
                        ),
                    ],
                    Some(match device.notifications.delivery {
                        Delivery::Open | Delivery::Push => Delivery::Open,
                        _ => Delivery::Off,
                    }),
                    busy,
                    theme,
                    move |delivery, _, _| {
                        if let Some(h) = &h {
                            ns::save_device(
                                h,
                                &id,
                                DeviceChange {
                                    delivery: Some(delivery),
                                    ..Default::default()
                                },
                            );
                        }
                    },
                ),
                theme,
            ));
            if status == DeviceStatus::Open {
                card = card.child(row(
                    ctx,
                    "notifications-test",
                    t!("notifications-test"),
                    Some(t!("notifications-test-help").into()),
                    Button::new("notifications-send-test")
                        .small()
                        .label(t!("notifications-send-test"))
                        .on_click(cx.listener(|this, _, _, cx| {
                            crate::notify::send_test(cx);
                            this.tested = Some(true);
                            cx.notify();
                        })),
                    theme,
                ));
                if self.tested.is_some() {
                    card = card.child(
                        div()
                            .id("notifications-test-result")
                            .test_support()
                            .role(Role::Status)
                            .aria_label(t!("notifications-test-sent"))
                            .text_sm()
                            .text_color(theme.online)
                            .child(t!("notifications-test-sent")),
                    );
                }
            }
            card = card.child(block(
                ctx,
                "notification-sound",
                t!("notifications-device-choices"),
                device_choices("this-device", device, state, false, handle.clone(), theme),
                theme,
            ));
        }
        block(
            ctx,
            "notifications-status",
            t!("notifications-device-title"),
            card,
            theme,
        )
    }

    fn routing(&self, ctx: &SettingsCtx, s: &Settings, theme: &Theme) -> Vec<AnyElement> {
        let session = self.session.clone();
        let last_only = ns::last_only(s);
        let summary = if last_only > 0 {
            t!(
                "notifications-routing-last-only",
                duration = duration(last_only)
            )
        } else {
            t!("notifications-routing-at-once").to_string()
        };
        let defaults = ns::defaults();
        let away_on = s.auto_away_delay > 0;
        let away_session = self.session.clone();
        vec![
            row(
                ctx,
                "notify-last-device",
                t!("notifications-last-device"),
                Some(t!("notifications-last-device-help").into()),
                self.field(Field::LastDevice, theme),
                theme,
            )
            .into_any_element(),
            row(
                ctx,
                "notify-all-devices",
                t!("notifications-all-devices"),
                Some(t!("notifications-all-devices-help").into()),
                self.field(Field::AllDevices, theme),
                theme,
            )
            .into_any_element(),
            div()
                .id("routing-summary")
                .test_support()
                .role(Role::Note)
                .aria_label(SharedString::from(summary.clone()))
                .px_3()
                .child(note(
                    format!("{} {summary}", t!("notifications-routing-anywhere")),
                    theme,
                ))
                .into_any_element(),
            row(
                ctx,
                "notify-reset",
                t!("notifications-reset"),
                Some(
                    t!(
                        "notifications-reset-help",
                        last = duration(defaults.last_device_delay),
                        all = duration(defaults.all_devices_delay)
                    )
                    .into(),
                ),
                Button::new("notify-reset-button")
                    .small()
                    .label(t!("notifications-reset-button"))
                    .disabled(ns::routing_is_default(s))
                    .on_click(move |_, _, cx| edit(&session, cx, ns::reset_routing)),
                theme,
            )
            .into_any_element(),
            row(
                ctx,
                "auto-away",
                t!("notifications-auto-away"),
                Some(
                    if away_on {
                        t!(
                            "notifications-auto-away-on",
                            delay = duration(s.auto_away_delay)
                        )
                    } else {
                        t!("notifications-auto-away-off").to_string()
                    }
                    .into(),
                ),
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        Switch::new("auto-away-switch")
                            .checked(away_on)
                            .accessibility_label(t!("notifications-auto-away-switch"))
                            .on_click(move |on, _, cx| {
                                let on = *on;
                                edit(&away_session, cx, |s| ns::set_auto_away(s, on))
                            }),
                    )
                    .when(away_on, |d| d.child(self.field(Field::Away, theme))),
                theme,
            )
            .into_any_element(),
        ]
    }

    fn quiet(&self, ctx: &SettingsCtx, s: &Settings, theme: &Theme) -> Vec<AnyElement> {
        let on = s.quiet_hours.enabled;
        let session = self.session.clone();
        let mut rows = vec![
            row(
                ctx,
                "quiet-hours",
                t!("notifications-quiet"),
                Some(t!("notifications-quiet-help", zone = s.quiet_hours.time_zone).into()),
                Switch::new("quiet-hours-switch")
                    .checked(on)
                    .accessibility_label(t!("notifications-quiet"))
                    .on_click(move |on, _, cx| {
                        let (on, zone) = (*on, ns::local_time_zone());
                        edit(&session, cx, |s| {
                            ns::set_quiet(s, Some(on), None, None, &zone)
                        })
                    }),
                theme,
            )
            .into_any_element(),
        ];
        if on {
            rows.push(
                div()
                    .id("quiet-hours-times")
                    .test_support()
                    .role(Role::Group)
                    .aria_label(t!("notifications-quiet-times"))
                    .px_3()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(note(t!("notifications-quiet-from"), theme))
                    .child(self.field(Field::QuietStart, theme))
                    .child(note(t!("notifications-quiet-until"), theme))
                    .child(self.field(Field::QuietEnd, theme))
                    .into_any_element(),
            );
        }
        rows
    }

    fn about(
        &self,
        ctx: &SettingsCtx,
        s: &Settings,
        client: &Client,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let (channel, direct) = (self.session.clone(), self.session.clone());
        let own = ns::own_levels(s);
        let mut own_list = div()
            .id("conversation-levels-list")
            .test_support()
            .role(Role::List)
            .aria_label(t!("notifications-own-levels"))
            .flex()
            .flex_col()
            .gap_1();
        if own.is_empty() {
            own_list = own_list.child(note(t!("notifications-own-levels-empty"), theme));
        }
        {
            let w = client.read();
            for (conversation, level) in &own {
                let name = w
                    .conversation(conversation)
                    .map(|c| c.name.clone())
                    .unwrap_or_else(|| t!("notifications-own-levels-gone").to_string());
                let (pick, clear) = (self.session.clone(), self.session.clone());
                let (pick_id, clear_id) = (conversation.clone(), conversation.clone());
                own_list = own_list.child(
                    div()
                        .id(ElementId::Name(
                            format!("conversation-level-{conversation}").into(),
                        ))
                        .test_support()
                        .role(Role::ListItem)
                        .aria_label(SharedString::from(name.clone()))
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .child(div().text_color(theme.text).child(name.clone()))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(choice(
                                    &format!("conversation-level-{conversation}-choice"),
                                    t!("notifications-own-level-for", name = name),
                                    levels(false),
                                    Some(Some(*level)),
                                    false,
                                    theme,
                                    move |level, _, cx| {
                                        edit(&pick, cx, |s| ns::set_level(s, &pick_id, level))
                                    },
                                ))
                                .child(
                                    Button::new(ElementId::Name(
                                        format!("conversation-level-{conversation}-reset").into(),
                                    ))
                                    .small()
                                    .ghost()
                                    .label(t!("notifications-own-level-reset"))
                                    .on_click(
                                        move |_, _, cx| {
                                            edit(&clear, cx, |s| ns::set_level(s, &clear_id, None))
                                        },
                                    ),
                                ),
                        ),
                );
            }
        }

        let words = ns::highlight_words(s).to_vec();
        let mut chips = div().flex().flex_wrap().gap_1();
        for word in &words {
            let session = self.session.clone();
            let remove = word.clone();
            chips = chips.child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .pl_2()
                    .rounded_full()
                    .bg(theme.selected)
                    .text_color(theme.text)
                    .child(word.clone())
                    .child(crate::ui::icon::button(
                        ElementId::Name(format!("highlight-remove-{word}").into()),
                        crate::ui::icon::IconName::Close,
                        t!("notifications-highlight-remove", word = word),
                        theme,
                        move |_, _, cx| edit(&session, cx, |s| ns::remove_highlight(s, &remove)),
                    )),
            );
        }
        let privacy = self.session.clone();
        // S2 draws Privacy; link to it once it is there.
        let privacy_registered = registry::get(cx)
            .settings_sections
            .iter()
            .any(|s| s.id == "privacy");
        let highlight = div()
            .flex()
            .flex_col()
            .gap_2()
            .child(note(t!("notifications-highlight-help"), theme))
            .when(!words.is_empty(), |d| d.child(chips))
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(
                        div().flex_1().child(
                            Input::new(&self.word)
                                .id("highlight-input")
                                .aria_label(t!("notifications-highlight-input")),
                        ),
                    )
                    .child(
                        Button::new("highlight-add")
                            .small()
                            .label(t!("notifications-highlight-add"))
                            .on_click(cx.listener(|this, _, window, cx| this.add_word(window, cx))),
                    ),
            )
            .when_some(self.word_problem, |d, problem| {
                d.child(
                    div()
                        .id("highlight-problem")
                        .test_support()
                        .role(Role::Alert)
                        .aria_label(word_problem(problem))
                        .text_color(theme.danger)
                        .child(word_problem(problem)),
                )
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(note(t!("notifications-ignored-note"), theme))
                    .when(privacy_registered, |d| {
                        d.child(
                            Button::new("notifications-open-privacy")
                                .small()
                                .ghost()
                                .label(t!("notifications-open-privacy"))
                                .on_click(move |_, _, cx| {
                                    super::open(&privacy, Some("privacy"), cx)
                                }),
                        )
                    }),
            );

        vec![
            row(
                ctx,
                "channel-notifications",
                t!("notifications-channels"),
                Some(t!("notifications-channels-help").into()),
                choice(
                    "channel-notifications-choice",
                    t!("notifications-channels"),
                    levels(false),
                    Some(Some(Level::of_channel(s.channel_default))),
                    false,
                    theme,
                    move |level, _, cx| {
                        if let Some(level) = level {
                            edit(&channel, cx, |s| ns::set_channel_default(s, level))
                        }
                    },
                ),
                theme,
            )
            .into_any_element(),
            row(
                ctx,
                "direct-notifications",
                t!("notifications-direct"),
                None,
                choice(
                    "direct-notifications-choice",
                    t!("notifications-direct"),
                    levels(false),
                    Some(Some(Level::of_direct(s.direct_default))),
                    false,
                    theme,
                    move |level, _, cx| {
                        if let Some(level) = level {
                            edit(&direct, cx, |s| ns::set_direct_default(s, level))
                        }
                    },
                ),
                theme,
            )
            .into_any_element(),
            block(
                ctx,
                "conversation-levels",
                t!("notifications-own-levels"),
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(note(t!("notifications-own-levels-help"), theme))
                    .child(own_list),
                theme,
            )
            .into_any_element(),
            block(
                ctx,
                "highlight-words",
                t!("notifications-highlight"),
                highlight,
                theme,
            )
            .into_any_element(),
        ]
    }

    fn other_devices(
        &self,
        ctx: &SettingsCtx,
        state: &ns::State,
        handle: Option<Handle>,
        theme: &Theme,
    ) -> impl IntoElement + use<> {
        let others: Vec<Device> = state
            .devices
            .iter()
            .flatten()
            .filter(|d| !d.current)
            .cloned()
            .collect();
        let mut list = div()
            .id("device-notifications-list")
            .test_support()
            .role(Role::List)
            .aria_label(t!("notifications-other-devices"))
            .flex()
            .flex_col()
            .gap_3();
        if let Some(error) = &state.error {
            list = list.child(
                div()
                    .text_color(theme.danger)
                    .child(t!("notifications-devices-failed", error = error)),
            );
        } else if state.devices.is_none() {
            list = list.child(note(t!("notifications-devices-loading"), theme));
        } else if others.is_empty() {
            list = list.child(note(t!("notifications-other-devices-empty"), theme));
        }
        for device in &others {
            let name = crate::settings::account::device_name(device);
            list = list.child(
                div()
                    .id(ElementId::Name(format!("device-{}", device.id).into()))
                    .test_support()
                    .role(Role::ListItem)
                    .aria_label(SharedString::from(name.clone()))
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.text)
                            .child(name),
                    )
                    .child(device_choices(
                        &device.id,
                        device,
                        state,
                        true,
                        handle.clone(),
                        theme,
                    )),
            );
        }
        block(
            ctx,
            "device-notifications",
            t!("notifications-other-devices"),
            div()
                .flex()
                .flex_col()
                .gap_2()
                .child(note(t!("notifications-other-devices-help"), theme))
                .child(list),
            theme,
        )
    }

    fn field(&self, field: Field, theme: &Theme) -> impl IntoElement + use<> {
        let invalid = self.invalid == Some(field);
        div()
            .w(px(120.))
            .flex()
            .flex_col()
            .child(
                Input::new(self.input(field))
                    .id(field.id())
                    .aria_label(field.label()),
            )
            .when(invalid, |d| {
                let text = match field {
                    Field::QuietStart | Field::QuietEnd => t!("notifications-time-invalid"),
                    _ => t!("notifications-duration-invalid"),
                };
                d.child(
                    div()
                        .id(ElementId::Name(format!("{}-problem", field.id()).into()))
                        .test_support()
                        .role(Role::Alert)
                        .aria_label(text)
                        .text_xs()
                        .text_color(theme.danger)
                        .child(text),
                )
            })
    }
}

/// One device's own choices: what it notifies about, (for another device)
/// how it delivers, its sound and previews.
fn device_choices(
    key: &str,
    device: &Device,
    state: &ns::State,
    delivery: bool,
    handle: Option<Handle>,
    theme: &Theme,
) -> Div {
    let n = &device.notifications;
    let busy = state.saving.contains(&device.id);
    let save = {
        let id = device.id.clone();
        Rc::new(move |change: DeviceChange| {
            if let Some(h) = &handle {
                ns::save_device(h, &id, change);
            }
        })
    };
    let mut body = div().flex().flex_col().gap_2();
    if !n.initialized {
        return body.child(note(t!("notifications-device-not-set-up"), theme));
    }
    let labelled = |label: &'static str, control: AnyElement| {
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .justify_between()
            .gap_2()
            .child(div().text_color(theme.muted).child(label))
            .child(control)
    };
    let s = save.clone();
    body = body.child(labelled(
        t!("notifications-content"),
        choice(
            &format!("{key}-content"),
            t!("notifications-content"),
            levels(true),
            Some(content_level(n.content)),
            busy,
            theme,
            move |level, _, _| {
                s(DeviceChange {
                    content: Some(level_content(level)),
                    ..Default::default()
                })
            },
        )
        .into_any_element(),
    ));
    if delivery {
        let s = save.clone();
        body = body.child(labelled(
            t!("notifications-delivery"),
            choice(
                &format!("{key}-delivery"),
                t!("notifications-delivery"),
                [Delivery::Off, Delivery::Open, Delivery::Push]
                    .into_iter()
                    .map(|d| {
                        let key = match d {
                            Delivery::Open => "open",
                            Delivery::Push => "push",
                            _ => "off",
                        };
                        (d, key, delivery_label(d).into())
                    })
                    .collect(),
                Some(match n.delivery {
                    Delivery::Open => Delivery::Open,
                    Delivery::Push => Delivery::Push,
                    _ => Delivery::Off,
                }),
                busy,
                theme,
                move |delivery, _, _| {
                    s(DeviceChange {
                        delivery: Some(delivery),
                        ..Default::default()
                    })
                },
            )
            .into_any_element(),
        ));
    }
    let s = save.clone();
    let sound = choice(
        &format!("{key}-sound"),
        t!("notifications-sound"),
        vec![
            (Sound::Off, "off", t!("notifications-sound-off").into()),
            (
                Sound::Mentions,
                "mentions",
                level_label(Level::Mentions).into(),
            ),
            (Sound::All, "all", level_label(Level::All).into()),
        ],
        Some(match n.sound {
            Sound::Off => Sound::Off,
            Sound::All => Sound::All,
            _ => Sound::Mentions,
        }),
        busy,
        theme,
        move |sound, _, _| {
            s(DeviceChange {
                sound: Some(sound),
                ..Default::default()
            })
        },
    );
    // This device's chime is played here, so it can be tried here.
    let local = key == "this-device";
    body = body.child(labelled(
        t!("notifications-sound"),
        div()
            .flex()
            .items_center()
            .gap_2()
            .child(sound.into_any_element())
            .when(local, |row| {
                row.child(
                    Button::new("notifications-sound-test")
                        .small()
                        .label(t!("notifications-sound-test"))
                        .on_click(|_, _, cx| {
                            crate::media::sound::chime(ircord_platform::audio::Chime::Mention, cx);
                        })
                        .into_any_element(),
                )
            })
            .into_any_element(),
    ));
    let s = save.clone();
    body = body.child(labelled(
        t!("notifications-preview"),
        Switch::new(ElementId::Name(format!("{key}-preview").into()))
            .checked(n.preview)
            .disabled(busy)
            .accessibility_label(t!("notifications-preview"))
            .on_click(move |on, _, _| {
                s(DeviceChange {
                    preview: Some(*on),
                    ..Default::default()
                })
            })
            .into_any_element(),
    ));
    if let Some(error) = state.errors.get(&device.id) {
        let text = match error {
            DeviceSaveError::Changed => t!("notifications-device-changed").to_string(),
            DeviceSaveError::Failed(e) => t!("notifications-device-save-failed", error = e),
        };
        body = body.child(
            div()
                .id(ElementId::Name(format!("{key}-error").into()))
                .test_support()
                .role(Role::Alert)
                .aria_label(SharedString::from(text.clone()))
                .text_color(theme.danger)
                .child(text),
        );
    }
    body
}

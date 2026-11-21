//! Settings → Privacy (package S2), as the web client's
//! `PrivacySettings.vue`: whether NSFW media and links show (an account
//! preference; switching it off also clears the older per-network opt-ins),
//! and the people you ignore, a nick or a mask each, kept in the
//! notification settings with the rest of who reaches you. Nothing is
//! embedded here, so "ask before embeds" has no row.

use std::rc::{Rc, Weak};

use gpui_kit::TestSupportExt;
use gpui_kit::component::Disableable;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::switch::Switch;
use gpui_kit::prelude::*;
use gpui_kit::*;
use ircord_client::Client;
use ircord_client::documents::{NOTIFICATION_SETTINGS, PREFERENCES};
use ircord_client::store::{Workspace, topic};

use crate::registry::{self, SettingsCtx, SettingsRow, SettingsSection};
use crate::session::{Session, SessionEvent};
use crate::theme;

/// The web client's limit for one ignore entry.
const MAX_LEN: usize = 120;

pub fn init(cx: &mut App) {
    registry::register(cx, |r| {
        r.settings_sections.add(
            SettingsSection::new("privacy", t!("settings-messages-privacy-title"), render)
                .order(60)
                .keywords(t!("settings-messages-privacy-keywords").split(", "))
                .row(
                    SettingsRow::new("nsfw-content", t!("settings-messages-nsfw"))
                        .description(t!("settings-messages-nsfw-help"))
                        .keywords(t!("settings-messages-nsfw-keywords")),
                )
                .row(
                    SettingsRow::new("ignored-people", t!("settings-messages-ignored"))
                        .description(t!("settings-messages-ignored-help"))
                        .keywords(t!("settings-messages-ignored-keywords")),
                ),
        );
    });
}

/// NSFW content shows: the account-wide choice, or an older opt-in for one
/// network.
pub fn shows_nsfw(w: &Workspace) -> bool {
    w.documents
        .preferences
        .value()
        .and_then(|p| p.preferences.as_ref())
        .is_some_and(|p| {
            p.show_nsfw == Some(true)
                || p.nsfw_networks
                    .as_ref()
                    .is_some_and(|n| n.values().any(|on| *on))
        })
}

fn set_nsfw(client: &Client, on: bool) {
    let mut next = client
        .read()
        .documents
        .preferences
        .value()
        .cloned()
        .unwrap_or_default();
    let p = next.preferences.get_or_insert_default();
    p.show_nsfw = Some(on);
    p.nsfw_networks = Some(Default::default());
    client.edit_document(&PREFERENCES, next);
}

pub fn ignored(w: &Workspace) -> Vec<String> {
    w.documents
        .notification_settings
        .value()
        .and_then(|s| s.ignore.clone())
        .unwrap_or_default()
}

/// The ignore list with `entry` added (once, ignoring case) or removed.
pub fn with_ignored(list: &[String], entry: &str, add: bool) -> Vec<String> {
    let entry = entry.trim();
    let mut out: Vec<String> = list
        .iter()
        .filter(|x| !x.eq_ignore_ascii_case(entry))
        .cloned()
        .collect();
    if add && !entry.is_empty() {
        let at = list
            .iter()
            .position(|x| x.eq_ignore_ascii_case(entry))
            .unwrap_or(out.len());
        out.insert(at.min(out.len()), entry.to_string());
    }
    out
}

fn set_ignored(client: &Client, list: Vec<String>) {
    let Some(mut next) = client
        .read()
        .documents
        .notification_settings
        .value()
        .cloned()
    else {
        return;
    };
    next.ignore = Some(list);
    client.edit_document(&NOTIFICATION_SETTINGS, next);
}

/// The privacy view, kept while the session lasts for its field.
struct Privacy(Entity<PrivacyView>);
impl Global for Privacy {}

fn render(ctx: &SettingsCtx, window: &mut Window, cx: &mut App) -> AnyElement {
    let view = match cx.try_global::<Privacy>() {
        Some(Privacy(view)) if view.read(cx).session == ctx.session => view.clone(),
        _ => {
            let view = cx.new(|cx| PrivacyView::new(ctx.session.clone(), window, cx));
            cx.set_global(Privacy(view.clone()));
            view
        }
    };
    let theme = theme::get(cx).clone();
    let Some(client) = ctx.session.read(cx).client().cloned() else {
        return div().id("privacy-settings").into_any_element();
    };
    watch(&client, cx);
    let w = client.read();
    let loaded = w.documents.preferences.is_loaded();
    let nsfw = shows_nsfw(&w);
    let ignore_loaded = w.documents.notification_settings.is_loaded();
    drop(w);
    let switch_client = client.clone();
    div()
        .id("privacy-settings")
        .test_support()
        .flex()
        .flex_col()
        .gap_3()
        .child(super::row(
            ctx,
            "nsfw-content",
            t!("settings-messages-nsfw"),
            Some(t!("settings-messages-nsfw-help").into()),
            Switch::new("nsfw-content-switch")
                .checked(loaded && nsfw)
                .disabled(!loaded)
                .accessibility_label(t!("settings-messages-nsfw"))
                .on_click(move |on, _, _| set_nsfw(&switch_client, *on)),
            &theme,
        ))
        .child(super::block(
            ctx,
            "ignored-people",
            t!("settings-messages-ignored"),
            if ignore_loaded {
                view.into_any_element()
            } else {
                div()
                    .text_sm()
                    .text_color(theme.muted)
                    .child(t!("core-loading"))
                    .into_any_element()
            },
            &theme,
        ))
        .into_any_element()
}

/// The client whose documents are watched, so each client is watched once.
#[derive(Default)]
struct Watched(Option<Weak<Client>>);
impl Global for Watched {}

fn watch(client: &Rc<Client>, cx: &mut App) {
    let watched = cx.default_global::<Watched>();
    if watched
        .0
        .as_ref()
        .and_then(Weak::upgrade)
        .is_some_and(|c| Rc::ptr_eq(&c, client))
    {
        return;
    }
    watched.0 = Some(Rc::downgrade(client));
    client.watch_document(&PREFERENCES);
    client.watch_document(&NOTIFICATION_SETTINGS);
}

pub struct PrivacyView {
    session: Entity<Session>,
    input: Entity<InputState>,
    _subscriptions: Vec<Subscription>,
}

impl PrivacyView {
    fn new(session: Entity<Session>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t!("settings-messages-ignore-placeholder"))
        });
        let subscriptions = vec![
            cx.subscribe_in(&input, window, |this: &mut Self, _, event, window, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    this.add(window, cx);
                }
            }),
            cx.subscribe(&session, |_, _, event: &SessionEvent, cx| {
                if let SessionEvent::Changed(changes) = event
                    && changes.topics.contains_key(topic::DOCUMENTS)
                {
                    cx.notify();
                }
            }),
        ];
        Self {
            session,
            input,
            _subscriptions: subscriptions,
        }
    }

    fn client(&self, cx: &App) -> Option<Rc<Client>> {
        self.session.read(cx).client().cloned()
    }

    fn add(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.input.read(cx).value().trim().to_string();
        if text.is_empty() {
            return;
        }
        let text: String = text.chars().take(MAX_LEN).collect();
        let Some(client) = self.client(cx) else {
            return;
        };
        let list = ignored(&client.read());
        set_ignored(&client, with_ignored(&list, &text, true));
        self.input
            .update(cx, |input, cx| input.set_value("", window, cx));
        cx.notify();
    }
}

impl Render for PrivacyView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme::get(cx).clone();
        let list = self
            .client(cx)
            .map(|c| ignored(&c.read()))
            .unwrap_or_default();
        let mut chips = div()
            .id("ignored-list")
            .test_support()
            .role(Role::List)
            .aria_label(t!("settings-messages-ignored"))
            .flex()
            .flex_wrap()
            .gap_1();
        for (i, entry) in list.iter().enumerate() {
            let remove = t!("settings-messages-stop-ignoring", value = entry.as_str());
            let entry2 = entry.clone();
            let hover = theme.hover;
            chips = chips.child(
                div()
                    .id(ElementId::Name(format!("ignored-{i}").into()))
                    .test_support()
                    .role(Role::ListItem)
                    .aria_label(SharedString::from(entry.clone()))
                    .flex()
                    .items_center()
                    .gap_1()
                    .pl_2()
                    .pr_1()
                    .py_0p5()
                    .rounded_md()
                    .border_1()
                    .border_color(theme.rail)
                    .text_sm()
                    .text_color(theme.text)
                    .child(entry.clone())
                    .child(
                        div()
                            .id(ElementId::Name(format!("ignored-remove-{i}").into()))
                            .test_support()
                            .role(Role::Button)
                            .aria_label(SharedString::from(remove))
                            .px_1()
                            .rounded_sm()
                            .text_color(theme.muted)
                            .cursor_pointer()
                            .hover(move |d| d.bg(hover))
                            .child("×")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(client) = this.client(cx) {
                                    let list = ignored(&client.read());
                                    set_ignored(&client, with_ignored(&list, &entry2, false));
                                }
                                cx.notify();
                            })),
                    ),
            );
        }
        if list.is_empty() {
            chips = chips.child(
                div()
                    .text_sm()
                    .text_color(theme.faint)
                    .child(t!("settings-messages-ignored-empty")),
            );
        }
        let hover = theme.hover;
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .text_sm()
                    .text_color(theme.muted)
                    .child(t!("settings-messages-ignored-help")),
            )
            .child(chips)
            .child(
                div()
                    .flex()
                    .gap_2()
                    .items_center()
                    .child(
                        div().flex_1().child(
                            Input::new(&self.input)
                                .id("ignore-input")
                                .aria_label(t!("settings-messages-ignore-label")),
                        ),
                    )
                    .child(
                        div()
                            .id("ignore-add")
                            .test_support()
                            .role(Role::Button)
                            .aria_label(t!("settings-messages-ignore-add"))
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .text_sm()
                            .bg(theme.selected)
                            .text_color(theme.text)
                            .cursor_pointer()
                            .hover(move |d| d.bg(hover))
                            .child(t!("settings-messages-ignore-add"))
                            .on_click(cx.listener(|this, _, window, cx| this.add(window, cx))),
                    ),
            )
    }
}

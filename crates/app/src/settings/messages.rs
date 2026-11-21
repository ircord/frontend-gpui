//! Settings → Messages (package S2), as the web client's
//! `MessageSettings.vue`: the layout (cozy, compact, IRC), the message font,
//! text size and space between groups, the clock, coloured names, the
//! conversation to open on startup, channel activity (show, fold or hide
//! joins and parts), animated media, and a reset for the layout.
//!
//! The layout settings are the message keys of the appearance document
//! (`message::layout` reads them); startup and channel activity are account
//! preferences. Opening the startup channel happens here too: once per
//! sign-in, when the preferences first arrive.

use crate::ui::focus::Reachable;
use std::rc::{Rc, Weak};

use gpui_kit::TestSupportExt;
use gpui_kit::component::switch::Switch;
use gpui_kit::prelude::*;
use gpui_kit::*;
use ircord_client::Client;
use ircord_client::contract::{
    Appearance, AppearanceMessageFont, AppearanceTimeFormat, ClientPreferences,
    ClientPreferencesEvents, ConversationKind, Startup, StartupMode,
};
use ircord_client::documents::{APPEARANCE, PREFERENCES};
use ircord_client::store::{Workspace, topic};

use crate::message::layout::{self, Layout, Look, SIZE, SPACING, SPACING_STEP};
use crate::registry::{self, SettingsCtx, SettingsRow, SettingsSection};
use crate::route::Route;
use crate::session::{Session, SessionEvent};
use crate::theme::{self, Theme};

/// The message fonts in the web client's order.
pub const FONTS: [AppearanceMessageFont; 8] = [
    AppearanceMessageFont::Interface,
    AppearanceMessageFont::Atkinson,
    AppearanceMessageFont::Lexend,
    AppearanceMessageFont::Serif,
    AppearanceMessageFont::Literata,
    AppearanceMessageFont::JetbrainsMinusMono,
    AppearanceMessageFont::GeistMinusMono,
    AppearanceMessageFont::AtkinsonMinusMono,
];

pub fn init(cx: &mut App) {
    registry::register(cx, |r| {
        let row = |id: &'static str, label: &'static str, help: Option<&'static str>, words| {
            let mut row = SettingsRow::new(id, label).keywords(words);
            if let Some(help) = help {
                row = row.description(help);
            }
            row
        };
        r.settings_sections.add(
            SettingsSection::new("messages", t!("settings-messages-title"), render)
                .order(20)
                .keywords(t!("settings-messages-keywords").split(", "))
                .row(row(
                    "message-layout",
                    t!("settings-messages-layout"),
                    Some(t!("settings-messages-layout-help")),
                    t!("settings-messages-layout-keywords"),
                ))
                .row(row(
                    "message-font",
                    t!("settings-messages-font"),
                    None,
                    t!("settings-messages-font-keywords"),
                ))
                .row(row(
                    "text-size",
                    t!("settings-messages-size"),
                    Some(t!("settings-messages-size-help")),
                    t!("settings-messages-size-keywords"),
                ))
                .row(row(
                    "group-spacing",
                    t!("settings-messages-spacing"),
                    Some(t!("settings-messages-spacing-help")),
                    t!("settings-messages-spacing-keywords"),
                ))
                .row(row(
                    "clock",
                    t!("settings-messages-clock"),
                    None,
                    t!("settings-messages-clock-keywords"),
                ))
                .row(row(
                    "coloured-names",
                    t!("settings-messages-colours"),
                    Some(t!("settings-messages-colours-help")),
                    t!("settings-messages-colours-keywords"),
                ))
                .row(row(
                    "startup-conversation",
                    t!("settings-messages-startup"),
                    Some(t!("settings-messages-startup-help")),
                    t!("settings-messages-startup-keywords"),
                ))
                .row(row(
                    "channel-activity",
                    t!("settings-messages-activity"),
                    Some(t!("settings-messages-activity-help")),
                    t!("settings-messages-activity-keywords"),
                ))
                .row(row(
                    "animate-media",
                    t!("settings-messages-animate"),
                    Some(t!("settings-messages-animate-help")),
                    t!("settings-messages-animate-keywords"),
                ))
                .row(row(
                    "messages-reset",
                    t!("settings-messages-reset"),
                    None,
                    t!("settings-messages-reset-keywords"),
                )),
        );
    });
    // Open the startup channel once per sign-in, when the preferences arrive.
    cx.observe_new::<Session>(|_, _, cx| {
        cx.subscribe_self(|session: &mut Session, event: &SessionEvent, cx| {
            let SessionEvent::Changed(changes) = event else {
                return;
            };
            if !changes.reset && !changes.topics.contains_key(topic::DOCUMENTS) {
                return;
            }
            let Some(client) = session.client().cloned() else {
                return;
            };
            let started = cx.default_global::<Started>();
            if started
                .0
                .as_ref()
                .and_then(Weak::upgrade)
                .is_some_and(|c| Rc::ptr_eq(&c, &client))
            {
                return;
            }
            let w = client.read();
            if !w.documents.preferences.is_loaded() || w.networks.is_empty() {
                return;
            }
            let target = startup_target(&w);
            drop(w);
            // Nothing is shown before the first conversation is picked.
            let Some(route) = session.route() else {
                return;
            };
            cx.global_mut::<Started>().0 = Some(Rc::downgrade(&client));
            if let Some(id) = target
                && matches!(route, Route::Conversation(_))
                && session.conversation.as_deref() != Some(id.as_str())
            {
                session.select_conversation(&id, cx);
            }
        })
        .detach();
    })
    .detach();
}

/// The client the startup channel was opened for.
#[derive(Default)]
struct Started(Option<Weak<Client>>);
impl Global for Started {}

/// The channels startup can open, ordered by network and ID like the web
/// client's `startupChannels`.
pub fn startup_channels(w: &Workspace) -> Vec<(String, String)> {
    let mut out: Vec<_> = w
        .conversations
        .iter()
        .filter(|c| {
            c.kind == ConversationKind::Channel
                && c.party.is_none()
                && c.joined
                && c.kicked.is_none()
                && w.network(&c.network_id).is_some()
        })
        .map(|c| (c.network_id.clone(), c.id.clone(), c.name.clone()))
        .collect();
    out.sort();
    out.into_iter()
        .map(|(network, id, name)| {
            let network = w
                .network(&network)
                .map(|n| n.name.clone())
                .unwrap_or_default();
            let name = if name.starts_with(['#', '&']) {
                name
            } else {
                format!("#{name}")
            };
            (id, format!("{name} · {network}"))
        })
        .collect()
}

fn preferences(w: &Workspace) -> Option<&ClientPreferences> {
    w.documents.preferences.value()?.preferences.as_ref()
}

fn general(w: &Workspace) -> Option<String> {
    w.conversations
        .iter()
        .filter(|c| c.kind == ConversationKind::Channel && c.party.is_none())
        .find(|c| {
            c.name
                .trim_start_matches('#')
                .eq_ignore_ascii_case("general")
        })
        .map(|c| c.id.clone())
}

/// The startup choice: open a chosen channel, or `None` for the last one viewed.
pub fn startup_choice(w: &Workspace) -> Option<Option<String>> {
    let startup = preferences(w)?.startup.as_ref()?;
    (startup.mode == Some(StartupMode::Channel)).then(|| startup.channel_id.clone())
}

/// The channel to open at startup, when the preferences choose one that
/// still exists (`#general` when none was picked).
pub fn startup_target(w: &Workspace) -> Option<String> {
    let chosen = startup_choice(w)?;
    let channels = startup_channels(w);
    let id = chosen.or_else(|| general(w))?;
    channels.iter().any(|(c, _)| *c == id).then_some(id)
}

pub fn events(w: &Workspace) -> ClientPreferencesEvents {
    match preferences(w).and_then(|p| p.events) {
        Some(ClientPreferencesEvents::Show) => ClientPreferencesEvents::Show,
        Some(ClientPreferencesEvents::Hide) => ClientPreferencesEvents::Hide,
        _ => ClientPreferencesEvents::Collapse,
    }
}

/// Change the message settings: in the account's appearance with sync on,
/// on this device with it off (`theme::appearance::edit`), and lay the
/// timeline out again.
pub fn edit_appearance(cx: &mut App, f: impl FnOnce(&mut Appearance)) {
    if crate::theme::appearance::edit(cx, f) {
        crate::timeline::refresh(cx);
        cx.refresh_windows();
    }
}

/// Change the account's preferences.
pub fn edit_preferences(client: &Client, f: impl FnOnce(&mut ClientPreferences)) {
    let mut next = client
        .read()
        .documents
        .preferences
        .value()
        .cloned()
        .unwrap_or_default();
    f(next.preferences.get_or_insert_default());
    client.edit_document(&PREFERENCES, next);
}

/// Every message key back to the web client's default.
pub fn reset(a: &mut Appearance) {
    let d = Look::default();
    a.message_layout = Some(d.layout.wire());
    a.message_font = Some(d.font);
    a.message_size = Some(d.size);
    a.group_spacing = Some(d.spacing);
    a.time_format = Some(AppearanceTimeFormat::V24h);
    a.colored_names = Some(d.colored_names);
    a.animate_media = Some(d.animate_media);
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
    client.watch_document(&APPEARANCE);
    client.watch_document(&PREFERENCES);
}

/// One choice in a row of choices.
pub fn choice(
    id: String,
    label: impl Into<SharedString>,
    active: bool,
    theme: &Theme,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let label: SharedString = label.into();
    let hover = theme.hover;
    div()
        .id(ElementId::Name(id.into()))
        .role(Role::RadioButton)
        .aria_label(label.clone())
        .aria_toggled(if active {
            Toggled::True
        } else {
            Toggled::False
        })
        .px_2()
        .py_1()
        .rounded_md()
        .text_sm()
        .cursor_pointer()
        .when(active, |d| d.bg(theme.accent).text_color(theme.on_accent))
        .when(!active, |d| {
            d.bg(theme.selected)
                .text_color(theme.text)
                .hover(move |d| d.bg(hover))
        })
        .child(label)
        .on_click(on_click)
}

fn group(id: &str, label: impl Into<SharedString>) -> Stateful<Div> {
    div()
        .id(ElementId::Name(id.to_string().into()))
        .role(Role::RadioGroup)
        .aria_label(label.into())
        .flex()
        .flex_wrap()
        .gap_1()
}

/// − value +, for the text size and the space between groups.
fn stepper(
    id: &str,
    label: &'static str,
    value: i64,
    (min, max, step): (i64, i64, i64),
    theme: &Theme,
    set: impl Fn(i64, &mut App) + 'static,
) -> impl IntoElement {
    let set = Rc::new(set);
    let button = |suffix: &str, text: &'static str, to: i64, enabled: bool| {
        let set = set.clone();
        let hover = theme.hover;
        div()
            .id(ElementId::Name(format!("{id}-{suffix}").into()))
            .test_support()
            .role(Role::Button)
            .reachable(theme)
            .aria_label(SharedString::from(format!("{label} {text}")))
            .w(px(28.))
            .h(px(28.))
            .flex()
            .items_center()
            .justify_center()
            .rounded_md()
            .bg(theme.selected)
            .text_color(if enabled { theme.text } else { theme.faint })
            .when(enabled, |d| d.cursor_pointer().hover(move |d| d.bg(hover)))
            .child(text)
            .on_click(move |_, _, cx| {
                if enabled {
                    set(to, cx)
                }
            })
    };
    div()
        .flex()
        .items_center()
        .gap_2()
        .child(button("less", "−", (value - step).max(min), value > min))
        .child(
            div()
                .id(ElementId::Name(format!("{id}-value").into()))
                .test_support()
                .role(Role::Status)
                .aria_label(t!("settings-messages-pixels", value = value))
                .min_w(px(40.))
                .text_sm()
                .text_color(theme.muted)
                .child(t!("settings-messages-pixels", value = value)),
        )
        .child(button("more", "+", (value + step).min(max), value < max))
}

fn layout_label(l: Layout) -> (&'static str, &'static str) {
    match l {
        Layout::Cozy => (
            t!("settings-messages-layout-cozy"),
            t!("settings-messages-layout-cozy-note"),
        ),
        Layout::Compact => (
            t!("settings-messages-layout-compact"),
            t!("settings-messages-layout-compact-note"),
        ),
        Layout::Irc => (
            t!("settings-messages-layout-irc"),
            t!("settings-messages-layout-irc-note"),
        ),
    }
}

pub fn font_label(f: AppearanceMessageFont) -> &'static str {
    match f {
        AppearanceMessageFont::Atkinson => t!("settings-messages-font-atkinson"),
        AppearanceMessageFont::Lexend => t!("settings-messages-font-lexend"),
        AppearanceMessageFont::Serif => t!("settings-messages-font-serif"),
        AppearanceMessageFont::Literata => t!("settings-messages-font-literata"),
        AppearanceMessageFont::JetbrainsMinusMono => t!("settings-messages-font-jetbrains"),
        AppearanceMessageFont::GeistMinusMono => t!("settings-messages-font-geist"),
        AppearanceMessageFont::AtkinsonMinusMono => t!("settings-messages-font-atkinson-mono"),
        _ => t!("settings-messages-font-interface"),
    }
}

fn font_id(f: AppearanceMessageFont) -> &'static str {
    match f {
        AppearanceMessageFont::Atkinson => "atkinson",
        AppearanceMessageFont::Lexend => "lexend",
        AppearanceMessageFont::Serif => "serif",
        AppearanceMessageFont::Literata => "literata",
        AppearanceMessageFont::JetbrainsMinusMono => "jetbrains-mono",
        AppearanceMessageFont::GeistMinusMono => "geist-mono",
        AppearanceMessageFont::AtkinsonMinusMono => "atkinson-mono",
        _ => "interface",
    }
}

fn render(ctx: &SettingsCtx, _window: &mut Window, cx: &mut App) -> AnyElement {
    let theme = theme::get(cx).clone();
    let Some(client) = ctx.session.read(cx).client().cloned() else {
        return div().id("message-settings").into_any_element();
    };
    watch(&client, cx);
    let w = client.read();
    let page = div()
        .id("message-settings")
        .test_support()
        .flex()
        .flex_col()
        .gap_3();
    // Edits replace the whole document, so nothing is changed before it is read.
    let synced = crate::theme::appearance::sync(cx);
    if (synced && !w.documents.appearance.is_loaded()) || !w.documents.preferences.is_loaded() {
        return page
            .child(
                div()
                    .text_sm()
                    .text_color(theme.muted)
                    .child(t!("core-loading")),
            )
            .into_any_element();
    }
    let look = layout::settings(cx);
    let startup = startup_choice(&w);
    let channels = startup_channels(&w);
    let activity = events(&w);
    drop(w);

    // Layout.
    let mut layouts = group("message-layout-choices", t!("settings-messages-layout"));
    for l in Layout::ALL {
        let (name, note) = layout_label(l);
        let active = look.layout == l;
        let hover = theme.hover;
        layouts = layouts.child(
            div()
                .id(ElementId::Name(format!("message-layout-{}", l.id()).into()))
                .test_support()
                .role(Role::RadioButton)
                .reachable(&theme)
                .aria_label(SharedString::from(name))
                .aria_toggled(if active {
                    Toggled::True
                } else {
                    Toggled::False
                })
                .flex_1()
                .min_w(px(140.))
                .p_2()
                .rounded_md()
                .border_1()
                .border_color(if active { theme.accent } else { theme.rail })
                .bg(theme.main)
                .cursor_pointer()
                .hover(move |d| d.bg(hover))
                .flex()
                .flex_col()
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.text)
                        .child(name),
                )
                .child(div().text_sm().text_color(theme.muted).child(note))
                .on_click(move |_, _, cx| {
                    edit_appearance(cx, |a| a.message_layout = Some(l.wire()))
                }),
        );
    }

    // Font.
    let mut fonts = group("message-font-choices", t!("settings-messages-font"));
    for f in FONTS {
        let active = look.font == f;
        let hover = theme.hover;
        let sample = div()
            .text_color(theme.text)
            .child(t!("settings-messages-font-sample"));
        let sample = match layout::family(f) {
            Some(family) => sample.font_family(family),
            None => sample,
        };
        fonts = fonts.child(
            div()
                .id(ElementId::Name(
                    format!("message-font-{}", font_id(f)).into(),
                ))
                .test_support()
                .role(Role::RadioButton)
                .reachable(&theme)
                .aria_label(SharedString::from(font_label(f)))
                .aria_toggled(if active {
                    Toggled::True
                } else {
                    Toggled::False
                })
                .w(px(180.))
                .p_2()
                .rounded_md()
                .border_1()
                .border_color(if active { theme.accent } else { theme.rail })
                .bg(theme.main)
                .cursor_pointer()
                .hover(move |d| d.bg(hover))
                .flex()
                .flex_col()
                .gap_1()
                .child(sample)
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.muted)
                        .child(font_label(f)),
                )
                .on_click(move |_, _, cx| edit_appearance(cx, |a| a.message_font = Some(f))),
        );
    }

    let clock = {
        let mut g = group("message-clock", t!("settings-messages-clock"));
        for (twelve, id, label) in [
            (false, "24h", t!("settings-messages-clock-24")),
            (true, "12h", t!("settings-messages-clock-12")),
        ] {
            g = g.child(
                choice(
                    format!("message-clock-{id}"),
                    label,
                    look.clock_12h == twelve,
                    &theme,
                    move |_, _, cx| {
                        edit_appearance(cx, |a| {
                            a.time_format = Some(if twelve {
                                AppearanceTimeFormat::V12h
                            } else {
                                AppearanceTimeFormat::V24h
                            })
                        })
                    },
                )
                .test_support()
                .reachable(&theme),
            );
        }
        g.test_support()
    };

    // Startup.
    let mut startup_body = div().flex().flex_col().gap_2();
    {
        let mut g = group("startup-mode", t!("settings-messages-startup"));
        for (channel, id, label) in [
            (false, "last", t!("settings-messages-startup-last")),
            (true, "channel", t!("settings-messages-startup-channel")),
        ] {
            let client = client.clone();
            let first = channels.first().map(|(id, _)| id.clone());
            g = g.child(
                choice(
                    format!("startup-mode-{id}"),
                    label,
                    startup.is_some() == channel,
                    &theme,
                    move |_, _, _| {
                        let first = first.clone();
                        edit_preferences(&client, |p| {
                            let old = p.startup.clone().unwrap_or_default();
                            p.startup = Some(Startup {
                                mode: Some(if channel {
                                    StartupMode::Channel
                                } else {
                                    StartupMode::Last
                                }),
                                channel_id: old.channel_id.or(if channel { first } else { None }),
                            });
                        })
                    },
                )
                .test_support()
                .reachable(&theme),
            );
        }
        startup_body = startup_body.child(g.test_support());
    }
    if let Some(chosen) = &startup {
        let current = chosen.clone().or_else(|| {
            let w = client.read();
            general(&w)
        });
        if channels.is_empty() {
            startup_body = startup_body.child(
                div()
                    .text_sm()
                    .text_color(theme.muted)
                    .child(t!("settings-messages-startup-none")),
            );
        }
        let mut g = group("startup-channel", t!("settings-messages-startup-choose"));
        for (id, label) in &channels {
            let client = client.clone();
            let picked = id.clone();
            g = g.child(
                choice(
                    format!("startup-channel-{id}"),
                    label.clone(),
                    current.as_deref() == Some(id.as_str()),
                    &theme,
                    move |_, _, _| {
                        let picked = picked.clone();
                        edit_preferences(&client, |p| {
                            p.startup = Some(Startup {
                                mode: Some(StartupMode::Channel),
                                channel_id: Some(picked),
                            })
                        })
                    },
                )
                .test_support()
                .reachable(&theme),
            );
        }
        startup_body = startup_body.child(g.test_support());
    }

    // Channel activity.
    let mut activity_group = group("channel-activity-choices", t!("settings-messages-activity"));
    for (mode, id, label) in [
        (
            ClientPreferencesEvents::Show,
            "show",
            t!("settings-messages-activity-show"),
        ),
        (
            ClientPreferencesEvents::Collapse,
            "collapse",
            t!("settings-messages-activity-fold"),
        ),
        (
            ClientPreferencesEvents::Hide,
            "hide",
            t!("settings-messages-activity-hide"),
        ),
    ] {
        let client = client.clone();
        activity_group = activity_group.child(
            choice(
                format!("channel-activity-{id}"),
                label,
                activity == mode,
                &theme,
                move |_, _, _| edit_preferences(&client, |p| p.events = Some(mode)),
            )
            .test_support()
            .reachable(&theme),
        );
    }

    let hover = theme.hover;
    page.child(super::block(
        ctx,
        "message-layout",
        t!("settings-messages-layout"),
        layouts.test_support(),
        &theme,
    ))
    .child(super::block(
        ctx,
        "message-font",
        t!("settings-messages-font"),
        fonts.test_support(),
        &theme,
    ))
    .child(super::row(
        ctx,
        "text-size",
        t!("settings-messages-size"),
        Some(t!("settings-messages-size-help").into()),
        stepper(
            "message-size",
            t!("settings-messages-size"),
            look.size,
            (SIZE.0, SIZE.1, 1),
            &theme,
            |v, cx| edit_appearance(cx, |a| a.message_size = Some(v)),
        ),
        &theme,
    ))
    .child(super::row(
        ctx,
        "group-spacing",
        t!("settings-messages-spacing"),
        Some(t!("settings-messages-spacing-help").into()),
        stepper(
            "group-spacing",
            t!("settings-messages-spacing"),
            look.spacing,
            (SPACING.0, SPACING.1, SPACING_STEP),
            &theme,
            |v, cx| edit_appearance(cx, |a| a.group_spacing = Some(v)),
        ),
        &theme,
    ))
    .child(super::row(
        ctx,
        "clock",
        t!("settings-messages-clock"),
        None,
        clock,
        &theme,
    ))
    .child(super::row(
        ctx,
        "coloured-names",
        t!("settings-messages-colours"),
        Some(t!("settings-messages-colours-help").into()),
        Switch::new("coloured-names-switch")
            .checked(look.colored_names)
            .accessibility_label(t!("settings-messages-colours"))
            .on_click(|on, _, cx| edit_appearance(cx, |a| a.colored_names = Some(*on))),
        &theme,
    ))
    .child(super::block(
        ctx,
        "startup-conversation",
        t!("settings-messages-startup"),
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .text_sm()
                    .text_color(theme.muted)
                    .child(t!("settings-messages-startup-help")),
            )
            .child(startup_body),
        &theme,
    ))
    .child(super::row(
        ctx,
        "channel-activity",
        t!("settings-messages-activity"),
        Some(t!("settings-messages-activity-help").into()),
        activity_group.test_support(),
        &theme,
    ))
    .child(super::row(
        ctx,
        "animate-media",
        t!("settings-messages-animate"),
        Some(t!("settings-messages-animate-help").into()),
        Switch::new("animate-media-switch")
            .checked(look.animate_media)
            .accessibility_label(t!("settings-messages-animate"))
            .on_click(|on, _, cx| edit_appearance(cx, |a| a.animate_media = Some(*on))),
        &theme,
    ))
    .child(super::row(
        ctx,
        "messages-reset",
        t!("settings-messages-reset"),
        None,
        div()
            .id("messages-reset-button")
            .test_support()
            .role(Role::Button)
            .reachable(&theme)
            .aria_label(t!("settings-messages-reset"))
            .px_2()
            .py_1()
            .rounded_md()
            .text_sm()
            .bg(theme.selected)
            .text_color(theme.text)
            .cursor_pointer()
            .hover(move |d| d.bg(hover))
            .child(t!("settings-messages-reset"))
            .on_click(|_, _, cx| edit_appearance(cx, reset)),
        &theme,
    ))
    .into_any_element()
}

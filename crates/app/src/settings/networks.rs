//! Settings → Networks: the network list and form (package NW1), as the web
//! client's `NetworkSettingsList.vue`: every network with its address, nick
//! and connection, an edit and a connect button on each, Add network, and
//! whether channels you create are registered with the network's services
//! (the `registerChannels` preference). Opening `irc://` links is DK1's.

use std::rc::{Rc, Weak};

use gpui_kit::TestSupportExt;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::switch::Switch;
use gpui_kit::prelude::*;
use gpui_kit::*;
use ircord_client::Client;
use ircord_client::api::networks::Login;
use ircord_client::contract::NetworkStatus;
use ircord_client::documents::PREFERENCES;

use crate::features::network_form;
use crate::registry::{self, SettingsCtx, SettingsSection};
use crate::session::Session;
use crate::theme::{self, Theme};

pub fn init(cx: &mut App) {
    registry::register(cx, |r| {
        r.settings_sections.add(
            SettingsSection::new(
                "networks",
                t!("networks-settings-title"),
                |ctx, window, cx| render(ctx, window, cx).into_any_element(),
            )
            .order(40)
            .keywords(["irc", "server", "nick", "sasl", "nickserv", "connect"]),
        );
    });
}

/// The client whose preferences are watched, so each client is watched once.
#[derive(Default)]
struct Watched(Option<Weak<Client>>);
impl Global for Watched {}

fn watch_preferences(client: &Rc<Client>, cx: &mut App) {
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
}

/// Whether channels you create are registered; on unless turned off.
pub fn register_channels(client: &Client) -> bool {
    client
        .read()
        .documents
        .preferences
        .value()
        .and_then(|p| p.preferences.as_ref()?.register_channels)
        .unwrap_or(true)
}

fn set_register_channels(client: &Client, on: bool) {
    let mut next = client
        .read()
        .documents
        .preferences
        .value()
        .cloned()
        .unwrap_or_default();
    next.preferences.get_or_insert_default().register_channels = Some(on);
    client.edit_document(&PREFERENCES, next);
}

fn status_label(status: NetworkStatus) -> &'static str {
    match status {
        NetworkStatus::Connected => t!("networks-status-connected"),
        NetworkStatus::Connecting => t!("networks-status-connecting"),
        NetworkStatus::Error => t!("networks-status-error"),
        _ => t!("networks-status-disconnected"),
    }
}

fn status_colour(status: NetworkStatus, theme: &Theme) -> Hsla {
    match status {
        NetworkStatus::Connected => theme.online,
        NetworkStatus::Connecting => theme.away,
        NetworkStatus::Error => theme.danger,
        _ => theme.offline,
    }
}

fn render(ctx: &SettingsCtx, _window: &mut Window, cx: &mut App) -> impl IntoElement {
    let theme = theme::get(cx).clone();
    let session: Entity<Session> = ctx.session.clone();
    let Some(client) = session.read(cx).client().cloned() else {
        return div().id("network-settings");
    };
    watch_preferences(&client, cx);
    let networks = client.read().networks.clone();
    let mut list = div()
        .id("network-settings")
        .test_support()
        .role(Role::List)
        .aria_label(t!("networks-settings-title"))
        .flex()
        .flex_col()
        .gap_2();
    if networks.is_empty() {
        list = list.child(
            div()
                .text_sm()
                .text_color(theme.muted)
                .child(t!("networks-settings-empty")),
        );
    }
    for n in &networks {
        let address = format!(
            "{} · {}:{}{}",
            n.nick,
            n.host,
            n.port,
            if n.tls { " · TLS" } else { "" }
        );
        let login = Login::of(n.auth.method);
        let mut state = status_label(n.status).to_string();
        if login != Login::None {
            state.push_str(&format!(" · {}", login.wire()));
        }
        let connect_label = match n.status {
            NetworkStatus::Connected => t!("networks-menu-disconnect"),
            NetworkStatus::Connecting => t!("networks-menu-cancel-connect"),
            _ => t!("networks-menu-connect"),
        };
        let up = !matches!(
            n.status,
            NetworkStatus::Connected | NetworkStatus::Connecting
        );
        let (edit_session, edit_id) = (session.clone(), n.id.clone());
        let (connect_session, connect_id) = (session.clone(), n.id.clone());
        list = list.child(
            div()
                .id(ElementId::Name(format!("network-setting-{}", n.id).into()))
                .test_support()
                .role(Role::ListItem)
                .aria_label(SharedString::from(n.name.clone()))
                .p_3()
                .rounded_md()
                .bg(theme.sidebar)
                .flex()
                .items_center()
                .gap_3()
                .child(
                    div()
                        .size(px(36.))
                        .flex_none()
                        .rounded_md()
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(theme.selected)
                        .font_weight(FontWeight::BOLD)
                        .child(if n.initials.is_empty() {
                            n.name.chars().take(2).collect::<String>().to_uppercase()
                        } else {
                            n.initials.clone()
                        }),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(div().font_weight(FontWeight::BOLD).child(n.name.clone()))
                        .child(div().text_xs().text_color(theme.faint).child(address))
                        .child(
                            div()
                                .id(ElementId::Name(format!("network-status-{}", n.id).into()))
                                .test_support()
                                .role(Role::Status)
                                .aria_label(SharedString::from(state.clone()))
                                .flex()
                                .items_center()
                                .gap_1p5()
                                .text_xs()
                                .text_color(theme.muted)
                                .child(
                                    div()
                                        .size(px(7.))
                                        .rounded_full()
                                        .bg(status_colour(n.status, &theme)),
                                )
                                .child(state),
                        )
                        .children(
                            n.status_detail
                                .clone()
                                .filter(|d| n.status == NetworkStatus::Error && !d.is_empty())
                                .map(|d| div().text_xs().text_color(theme.danger).child(d)),
                        ),
                )
                .child(
                    Button::new(ElementId::Name(format!("network-edit-{}", n.id).into()))
                        .ghost()
                        .label(t!("networks-settings-edit"))
                        .on_click(move |_, window, cx| {
                            network_form::open_edit(&edit_session, &edit_id, window, cx)
                        }),
                )
                .child(
                    Button::new(ElementId::Name(format!("network-connect-{}", n.id).into()))
                        .ghost()
                        .label(connect_label)
                        .on_click(move |_, window, cx| {
                            network_form::connect(&connect_session, &connect_id, up, window, cx)
                        }),
                ),
        );
    }
    let add_session = session.clone();
    let register = register_channels(&client);
    let switch_client = client.clone();
    div()
        .id("network-settings-page")
        .flex()
        .flex_col()
        .gap_3()
        .text_sm()
        .text_color(theme.text)
        .child(list)
        .child(
            div().child(
                Button::new("network-add")
                    .label(t!("networks-menu-add"))
                    .on_click(move |_, window, cx| {
                        network_form::open_add(&add_session, window, cx)
                    }),
            ),
        )
        .child(
            div()
                .mt_4()
                .text_xs()
                .font_weight(FontWeight::BOLD)
                .text_color(theme.muted)
                .child(t!("networks-channels").to_uppercase()),
        )
        .child(
            div()
                .id("network-register-channels")
                .test_support()
                .role(Role::Group)
                .aria_label(t!("networks-register-channels"))
                .flex()
                .items_center()
                .gap_4()
                .child(
                    div()
                        .flex_1()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(t!("networks-register-channels")),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.muted)
                                .child(t!("networks-register-channels-help")),
                        ),
                )
                .child(
                    Switch::new("network-register-channels-switch")
                        .checked(register)
                        .accessibility_label(t!("networks-register-channels"))
                        .on_click(move |on, _, _| set_register_channels(&switch_client, *on)),
                ),
        )
}

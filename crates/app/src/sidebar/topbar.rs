//! The bar across the top of the workspace, as the web client's: back and
//! forward, one tab per network (its icon, name and a dot), "+" to add a
//! network; at the right, Settings and your own button, which opens your
//! menu (`Registry::self_menu`).
//!
//! A network tab's dot shows its state: a ring while it is not connected,
//! a red dot for mentions, a dark one for unread elsewhere, else a green
//! one. Right-clicking a tab opens the network menu.

use gpui_kit::TestSupportExt;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::menu::{ContextMenuExt, DropdownMenu};
use gpui_kit::prelude::*;
use gpui_kit::*;
use ircord_client::contract::{MemberStatus, NetworkStatus};
use ircord_client::store::people;
use ircord_client::store::sidebar_order::{self as order, Activity};

use super::{glyph, network_label};
use crate::menu_order;
use crate::registry::{self, MenuItem, NetworkCtx, SidebarCtx};
use crate::session::Session;
use crate::theme;
use crate::ui;
use crate::ui::focus::Reachable;
use crate::ui::icon::IconName;

/// The bar's height.
pub const HEIGHT: f32 = 48.;

/// Register your menu's entries for settings and signing out.
pub fn init(cx: &mut App) {
    registry::register(cx, |r| {
        let menu = &mut r.self_menu;
        let settings = |id: &'static str, label: &'static str, section: &'static str| {
            MenuItem::new(id, label, move |ctx: &SidebarCtx, _, cx| {
                crate::settings::open(&ctx.session, Some(section), cx)
            })
        };
        menu.add(
            settings("self-edit-profile", t!("sidebar-edit-profile"), "profile")
                .icon(IconName::User)
                .order(menu_order::own::EDIT_PROFILE),
        );
        menu.add(
            settings("self-preferences", t!("sidebar-preferences"), "appearance")
                .icon(IconName::Settings2)
                .order(menu_order::own::PREFERENCES),
        );
        menu.add(
            settings(
                "self-notifications",
                t!("sidebar-notifications"),
                "notifications",
            )
            .icon(IconName::Bell)
            .order(menu_order::own::NOTIFICATIONS),
        );
        menu.add(
            settings("self-account", t!("sidebar-account"), "account")
                .icon(IconName::Settings)
                .order(menu_order::own::ACCOUNT),
        );
        menu.add(
            MenuItem::new(
                "sign-out",
                t!("sidebar-sign-out"),
                |ctx: &SidebarCtx, _, cx| ctx.session.update(cx, |s, cx| s.sign_out(cx)),
            )
            .icon(IconName::CircleX)
            .danger()
            .order(menu_order::own::LOG_OUT),
        );
    });
}

/// The bar, for the session's client.
pub fn bar(session: &Entity<Session>, window: &mut Window, cx: &mut App) -> AnyElement {
    let theme = theme::get(cx).clone();
    let s = session.read(cx);
    let Some(client) = s.client().cloned() else {
        return div().into_any_element();
    };
    let selected = s.network.clone();
    let (can_back, can_forward) = (s.history.can_go_back(), s.history.can_go_forward());
    let self_ctx = SidebarCtx {
        session: session.clone(),
        network: selected.clone(),
    };
    // Your status, from a registered slot (P1), else your account.
    let self_line = registry::get(cx)
        .self_line
        .to_vec()
        .iter()
        .find_map(|slot| (slot.render)(&self_ctx, window, cx));
    let w = client.read();

    let nav = |id: &'static str, icon, label: &'static str, enabled: bool, back: bool| {
        let session = session.clone();
        div()
            .when(!enabled, |d| d.opacity(0.4))
            .child(ui::icon::button(
                id,
                icon,
                label,
                &theme,
                move |_, _, cx| {
                    session.update(cx, |s, cx| {
                        if back {
                            s.go_back(cx)
                        } else {
                            s.go_forward(cx)
                        }
                    })
                },
            ))
    };

    let mut tabs = div()
        .id("network-tabs")
        .test_support()
        .role(Role::TabList)
        .aria_label(t!("sidebar-networks"))
        .flex()
        .items_center()
        .gap_1()
        .min_w_0()
        .overflow_x_scroll();
    for network in &w.networks {
        let activity = order::network_activity(&w, &network.id);
        let active = selected.as_deref() == Some(network.id.as_str());
        let label = network_label(&network.name, network.status, activity);
        let tooltip = match network.status_detail.as_deref() {
            Some(detail) if !detail.is_empty() => t!(
                "sidebar-network-tooltip",
                network = network.name,
                status = detail
            ),
            _ => label.clone(),
        };
        let dot_id = |kind: &str| ElementId::Name(format!("network-{}-{kind}", network.id).into());
        let dot = div().rounded_full().border_2().border_color(theme.sidebar);
        let dot = if network.status != NetworkStatus::Connected {
            dot.id(dot_id("offline"))
                .test_support()
                .size(px(10.))
                .border_color(theme.warning)
                .bg(theme.sidebar)
        } else {
            match activity {
                Activity::Mention => dot
                    .id(dot_id("mention"))
                    .test_support()
                    .size(px(10.))
                    .bg(theme.danger),
                Activity::Unread if !active => dot
                    .id(dot_id("unread"))
                    .test_support()
                    .size(px(10.))
                    .bg(theme.text),
                _ => dot
                    .id(dot_id("online"))
                    .test_support()
                    .size(px(9.))
                    .bg(theme.online),
            }
        };
        let id = network.id.clone();
        let select = session.clone();
        let ctx = NetworkCtx {
            session: session.clone(),
            network: network.id.clone(),
        };
        let tab = div()
            .id(ElementId::Name(format!("network-{}", network.id).into()))
            .test_support()
            .role(Role::Tab)
            .reachable(&theme)
            .aria_label(SharedString::from(label))
            .aria_selected(active)
            .tooltip(ui::tooltip::text(tooltip))
            .h(px(32.))
            .flex_none()
            .pl_1()
            .pr_3()
            .rounded_full()
            .flex()
            .items_center()
            .gap_2()
            .text_sm()
            .cursor_pointer()
            .border_1()
            .when(active, |d| {
                d.bg(theme.selected)
                    .border_color(theme.border)
                    .text_color(theme.text)
                    .font_weight(FontWeight::SEMIBOLD)
            })
            .when(!active, |d| {
                d.border_color(gpui_kit::transparent_black())
                    .text_color(theme.muted)
                    .hover(|d| d.bg(theme.hover))
            })
            .child(
                div()
                    .relative()
                    .flex_none()
                    .child(glyph::network_icon(&w, network, px(24.)))
                    .child(div().absolute().bottom(px(-3.)).right(px(-3.)).child(dot)),
            )
            .child(div().truncate().max_w(px(160.)).child(network.name.clone()))
            .on_click(move |_, _, cx| select.update(cx, |s, cx| s.select_network(&id, cx)));
        tabs = tabs.child(tab.context_menu(move |menu, window, cx| {
            let items = registry::get(cx).network_menu.to_vec();
            let items = ui::context_menu::visible(&items, &ctx, cx);
            ui::context_menu::fill(menu, items, ctx.clone(), window, cx)
        }));
    }
    let add = {
        let session = session.clone();
        ui::icon::button(
            "add-network",
            IconName::Plus,
            t!("sidebar-add-network"),
            &theme,
            move |_, window, cx| crate::features::network_form::open_add(&session, window, cx),
        )
    };

    let settings = {
        let session = session.clone();
        div()
            .id("open-settings")
            .test_support()
            .role(Role::Button)
            .reachable(&theme)
            .aria_label(t!("sidebar-settings"))
            .h(px(32.))
            .px_3()
            .rounded_full()
            .flex()
            .items_center()
            .gap_1p5()
            .text_sm()
            .cursor_pointer()
            .text_color(theme.text)
            .border_1()
            .border_color(theme.border)
            .bg(theme.main)
            .hover(|d| d.bg(theme.hover))
            .child(ui::icon::icon(IconName::Settings2, theme.muted))
            .child(t!("sidebar-settings"))
            .on_click(move |_, _, cx| crate::settings::open(&session, None, cx))
    };

    let network = selected.as_deref().and_then(|id| w.network(id));
    let nick = network
        .map(|n| n.nick.clone())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| w.user.nick.clone());
    let me = people::me(&w, selected.as_deref());
    let presence = match network.map(|n| n.status) {
        Some(NetworkStatus::Connected) if me.status == MemberStatus::Away => theme.away,
        Some(NetworkStatus::Connected) => theme.online,
        _ => theme.offline,
    };
    let self_line = self_line.unwrap_or_else(|| {
        div()
            .text_xs()
            .text_color(theme.muted)
            .truncate()
            .child(w.user.account.clone())
            .into_any_element()
    });
    let picture = div()
        .relative()
        .flex_none()
        .child(
            ui::avatar::avatar(&nick, Some(theme.selected), px(24.), &theme)
                .text_xs()
                .text_color(theme.text),
        )
        .child(
            div()
                .absolute()
                .bottom(px(-2.))
                .right(px(-2.))
                .size(px(10.))
                .rounded_full()
                .border_2()
                .border_color(theme.main)
                .bg(presence),
        );
    let name = div()
        .flex()
        .items_center()
        .gap_2()
        .min_w_0()
        .child(picture)
        .child(
            div()
                .flex()
                .flex_col()
                .min_w_0()
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.text)
                        .truncate()
                        .child(nick),
                )
                .child(self_line),
        );
    drop(w);
    let me_button = Button::new("self-menu")
        .ghost()
        .accessibility_label(t!("sidebar-self-menu"))
        .tooltip(t!("sidebar-self-menu"))
        .max_w(px(220.))
        .h(px(40.))
        .justify_start()
        .child(name)
        .dropdown_menu_with_anchor(Anchor::TopRight, move |menu, window, cx| {
            let items = registry::get(cx).self_menu.to_vec();
            let items = ui::context_menu::visible(&items, &self_ctx, cx);
            ui::context_menu::fill(menu, items, self_ctx.clone(), window, cx)
        });

    div()
        .id("top-bar")
        .test_support()
        .role(Role::Navigation)
        .aria_label(t!("sidebar-top-bar"))
        .h(px(HEIGHT))
        .flex_none()
        .px_2()
        .flex()
        .items_center()
        .gap_1()
        .bg(theme.sidebar)
        .child(nav(
            "nav-back",
            IconName::ArrowLeft,
            t!("sidebar-back"),
            can_back,
            true,
        ))
        .child(nav(
            "nav-forward",
            IconName::ArrowRight,
            t!("sidebar-forward"),
            can_forward,
            false,
        ))
        .child(div().w(px(4.)))
        .child(tabs)
        .child(add)
        .child(div().flex_1())
        .child(settings)
        .child(me_button)
        .into_any_element()
}

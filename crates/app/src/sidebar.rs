//! The network rail and the conversation list. Redraws only when the
//! sidebar's data, the selection or the connection changes.

use gpui_kit::TestSupportExt;
use gpui_kit::prelude::*;
use gpui_kit::*;
use ircord_client::contract::{ConversationKind, NetworkStatus};

use crate::session::{Session, SessionEvent};
use crate::theme::{self, c};

pub struct SidebarView {
    session: Entity<Session>,
    _subscription: Subscription,
}

impl SidebarView {
    pub fn new(session: Entity<Session>, cx: &mut Context<Self>) -> Self {
        let subscription = cx.subscribe(&session, |_, _, event: &SessionEvent, cx| match event {
            SessionEvent::Changed(changes) if changes.sidebar || changes.reset => cx.notify(),
            SessionEvent::Selection | SessionEvent::Phase | SessionEvent::Connection => cx.notify(),
            _ => {}
        });
        Self {
            session,
            _subscription: subscription,
        }
    }
}

fn badge(count: i64, mention: bool) -> Div {
    let text = if count > 99 {
        "99+".to_string()
    } else {
        count.to_string()
    };
    div()
        .px_1p5()
        .min_w(px(18.))
        .h(px(18.))
        .rounded_full()
        .flex()
        .items_center()
        .justify_center()
        .text_xs()
        .font_weight(FontWeight::BOLD)
        .bg(c(if mention {
            theme::MENTION_BG
        } else {
            theme::SELECTED
        }))
        .text_color(c(0xffffff))
        .child(text)
}

impl Render for SidebarView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let session = self.session.read(cx);
        let Some(client) = session.client().cloned() else {
            return div().into_any_element();
        };
        let selected_network = session.network.clone();
        let selected = session.conversation.clone();
        let w = client.read();

        // The rail: one circle per network, with its unread state.
        let mut rail = div()
            .id("network-rail")
            .w(px(72.))
            .h_full()
            .flex()
            .flex_col()
            .items_center()
            .gap_2()
            .py_3()
            .bg(c(theme::RAIL))
            .overflow_y_scroll();
        for network in &w.networks {
            let (unread, mentions) = w.network_counts(&network.id);
            let active = selected_network.as_deref() == Some(network.id.as_str());
            let id = network.id.clone();
            let session = self.session.clone();
            let initials = if network.initials.is_empty() {
                network.name.chars().take(2).collect::<String>()
            } else {
                network.initials.clone()
            };
            let mut circle = div()
                .id(ElementId::Name(format!("network-{}", network.id).into()))
                .test_support()
                .role(Role::Button)
                .aria_label(SharedString::from(network.name.clone()))
                .size(px(48.))
                .rounded(if active { px(16.) } else { px(24.) })
                .flex()
                .items_center()
                .justify_center()
                .bg(c(if active { theme::ACCENT } else { theme::MAIN }))
                .text_color(c(theme::TEXT))
                .font_weight(FontWeight::BOLD)
                .cursor_pointer()
                .relative()
                .child(initials)
                .on_click(move |_, _, cx| session.update(cx, |s, cx| s.select_network(&id, cx)));
            if network.status != NetworkStatus::Connected {
                circle = circle.opacity(0.5);
            }
            if mentions > 0 {
                circle = circle.child(
                    div()
                        .absolute()
                        .bottom(px(-4.))
                        .right(px(-4.))
                        .child(badge(mentions, true)),
                );
            } else if unread > 0 && !active {
                circle = circle.child(
                    div()
                        .absolute()
                        .left(px(-12.))
                        .size(px(8.))
                        .rounded_full()
                        .bg(c(theme::TEXT)),
                );
            }
            rail = rail.child(circle);
        }

        // The conversation list for the selected network.
        let network = selected_network.as_deref().and_then(|id| w.network(id));
        let mut list = div()
            .id("conversation-list")
            .flex_1()
            .flex()
            .flex_col()
            .gap_0p5()
            .px_2()
            .py_2()
            .overflow_y_scroll();
        if let Some(network) = network {
            let mut section: Option<&str> = None;
            for conversation in w.sidebar(&network.id) {
                let heading = if conversation.favorite {
                    "Favourites"
                } else if conversation.kind == ConversationKind::Channel {
                    "Channels"
                } else {
                    "Direct messages"
                };
                if section != Some(heading) {
                    section = Some(heading);
                    list = list.child(
                        div()
                            .pt_3()
                            .pb_1()
                            .px_2()
                            .text_xs()
                            .font_weight(FontWeight::BOLD)
                            .text_color(c(theme::MUTED))
                            .child(heading.to_uppercase()),
                    );
                }
                let active = selected.as_deref() == Some(conversation.id.as_str());
                let unread = conversation.unread_count > 0;
                let prefix = if conversation.kind == ConversationKind::Channel {
                    "#"
                } else {
                    "@"
                };
                let id = conversation.id.clone();
                let session = self.session.clone();
                let mut row = div()
                    .id(ElementId::Name(
                        format!("conversation-{}", conversation.id).into(),
                    ))
                    .test_support()
                    .role(Role::Button)
                    .aria_label(SharedString::from(conversation.name.clone()))
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .cursor_pointer()
                    .text_color(c(if active || unread {
                        theme::TEXT
                    } else {
                        theme::MUTED
                    }))
                    .when(active, |d| d.bg(c(theme::SELECTED)))
                    .when(!active, |d| d.hover(|d| d.bg(c(theme::HOVER))))
                    .when(unread, |d| d.font_weight(FontWeight::SEMIBOLD))
                    .when(!conversation.joined, |d| d.opacity(0.6))
                    .child(div().text_color(c(theme::FAINT)).child(prefix))
                    .child(div().flex_1().truncate().child(conversation.name.clone()))
                    .on_click(move |_, _, cx| {
                        session.update(cx, |s, cx| s.select_conversation(&id, cx))
                    });
                if conversation.mention_count > 0 {
                    row = row.child(badge(conversation.mention_count, true));
                }
                list = list.child(row);
            }
        }

        let header = div()
            .h(px(48.))
            .px_4()
            .flex()
            .items_center()
            .border_b_1()
            .border_color(c(theme::RAIL))
            .font_weight(FontWeight::BOLD)
            .text_color(c(theme::TEXT))
            .child(network.map(|n| n.name.clone()).unwrap_or_default());

        let me = network
            .map(|n| n.nick.clone())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| w.user.nick.clone());
        let session = self.session.clone();
        let footer = div()
            .h(px(52.))
            .px_3()
            .flex()
            .items_center()
            .justify_between()
            .bg(c(0x232428))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::BOLD)
                            .text_color(c(theme::TEXT))
                            .child(me),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(c(theme::MUTED))
                            .child(w.user.account.clone()),
                    ),
            )
            .child(
                div()
                    .id("sign-out")
                    .test_support()
                    .role(Role::Button)
                    .aria_label("Sign out")
                    .text_xs()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .cursor_pointer()
                    .text_color(c(theme::MUTED))
                    .hover(|d| d.bg(c(theme::HOVER)).text_color(c(theme::TEXT)))
                    .child("Sign out")
                    .on_click(move |_, _, cx| session.update(cx, |s, cx| s.sign_out(cx))),
            );

        div()
            .h_full()
            .flex()
            .flex_row()
            .child(rail)
            .child(
                div()
                    .w(px(240.))
                    .h_full()
                    .flex()
                    .flex_col()
                    .bg(c(theme::SIDEBAR))
                    .child(header)
                    .child(list)
                    .child(footer),
            )
            .into_any_element()
    }
}

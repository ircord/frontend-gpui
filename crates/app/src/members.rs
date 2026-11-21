//! The member list with presence, grouped online, away, offline and ranked
//! by channel privilege within each group. Right-clicking a member opens
//! the registered member menu.

use gpui_kit::TestSupportExt;
use gpui_kit::component::menu::ContextMenuExt;
use gpui_kit::prelude::*;
use gpui_kit::*;
use ircord_client::contract::{Member, MemberStatus};

use crate::registry::{self, MemberCtx};
use crate::session::{Session, SessionEvent};
use crate::theme;
use crate::ui;

pub struct MembersView {
    session: Entity<Session>,
    _subscription: Subscription,
}

impl MembersView {
    pub fn new(session: Entity<Session>, cx: &mut Context<Self>) -> Self {
        let subscription = cx.subscribe(&session, |_, _, event: &SessionEvent, cx| match event {
            SessionEvent::Changed(changes)
                if changes.members || changes.reset || changes.sidebar =>
            {
                cx.notify()
            }
            SessionEvent::Selection | SessionEvent::Phase => cx.notify(),
            _ => {}
        });
        Self {
            session,
            _subscription: subscription,
        }
    }
}

/// Channel prefixes from ISUPPORT `PREFIX=(qaohv)~&@%+`, highest first.
fn prefixes(isupport: &std::collections::BTreeMap<String, String>) -> String {
    isupport
        .get("PREFIX")
        .and_then(|p| p.split_once(')').map(|(_, symbols)| symbols.to_string()))
        .unwrap_or_else(|| "~&@%+".to_string())
}

impl Render for MembersView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme::get(cx).clone();
        let session = self.session.read(cx);
        let (Some(client), Some(id)) = (session.client().cloned(), session.conversation.clone())
        else {
            return div().into_any_element();
        };
        let w = client.read();
        let Some(conversation) = w.conversation(&id) else {
            return div().into_any_element();
        };
        let order = w
            .network(&conversation.network_id)
            .map(|n| prefixes(&n.isupport))
            .unwrap_or_default();
        let mut members: Vec<(&Member, String)> = conversation
            .member_ids
            .iter()
            .filter_map(|mid| w.members.get(mid))
            .map(|m| {
                let modes = conversation
                    .member_modes
                    .get(&m.id)
                    .cloned()
                    .unwrap_or_default();
                let top = order
                    .chars()
                    .find(|p| modes.contains(*p))
                    .map(String::from)
                    .unwrap_or_default();
                (m, top)
            })
            .collect();
        let rank = |p: &str| {
            p.chars()
                .next()
                .and_then(|c| order.find(c))
                .unwrap_or(usize::MAX)
        };
        let status_rank = |s: MemberStatus| match s {
            MemberStatus::Online => 0,
            MemberStatus::Away => 1,
            _ => 2,
        };
        members.sort_by_key(|(m, p)| (status_rank(m.status), rank(p), m.nick.to_lowercase()));

        let mut panel = div()
            .id("member-list")
            .test_support()
            .size_full()
            .bg(theme.sidebar)
            .flex()
            .flex_col()
            .px_2()
            .py_3()
            .gap_0p5()
            .overflow_y_scroll();
        let mut group: Option<i32> = None;
        for (m, prefix) in members {
            let g = status_rank(m.status);
            if group != Some(g) {
                group = Some(g);
                let label = match g {
                    0 => t!("people-online"),
                    1 => t!("people-away"),
                    _ => t!("people-offline"),
                };
                panel = panel.child(
                    div()
                        .pt_2()
                        .px_2()
                        .text_xs()
                        .font_weight(FontWeight::BOLD)
                        .text_color(theme.muted)
                        .child(label.to_uppercase()),
                );
            }
            let dot = match m.status {
                MemberStatus::Online => theme.online,
                MemberStatus::Away => theme.away,
                _ => theme.offline,
            };
            let name = if m.display_name.is_empty() {
                m.nick.clone()
            } else {
                m.display_name.clone()
            };
            let ctx = MemberCtx {
                session: self.session.clone(),
                member: m.id.clone(),
                conversation: Some(id.clone()),
            };
            panel = panel.child(
                div()
                    .id(ElementId::Name(format!("member-{}", m.id).into()))
                    .test_support()
                    .role(Role::ListItem)
                    .aria_label(SharedString::from(name.clone()))
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .flex()
                    .items_center()
                    .gap_2()
                    .hover(|d| d.bg(theme.hover))
                    .when(m.status == MemberStatus::Offline, |d| d.opacity(0.5))
                    .child(div().size(px(10.)).rounded_full().bg(dot))
                    .child(div().text_color(theme.faint).text_sm().child(prefix))
                    .child(
                        div()
                            .flex_1()
                            .truncate()
                            .text_sm()
                            .text_color(theme.text)
                            .child(name),
                    )
                    .when(!m.status_text.clone().unwrap_or_default().is_empty(), |d| {
                        d.child(
                            div()
                                .text_xs()
                                .text_color(theme.muted)
                                .truncate()
                                .max_w(px(80.))
                                .child(m.status_text.clone().unwrap_or_default()),
                        )
                    })
                    .context_menu(move |menu, _, cx| {
                        let items = registry::get(cx).member_menu.to_vec();
                        let items = ui::context_menu::visible(&items, &ctx, cx);
                        ui::context_menu::fill(menu, items, ctx.clone())
                    }),
            );
        }
        panel.into_any_element()
    }
}

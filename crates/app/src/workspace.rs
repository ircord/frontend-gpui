//! The signed-in layout, as the web client's: the top bar with the network
//! tabs across the window, the sidebar under it, and beside it a card
//! holding the main pane (header, connection banner, registered banners,
//! the route) and the right-hand panel.
//!
//! The main pane shows the session's route: a conversation (header,
//! timeline, composer) is drawn here; any other route by the pane a feature
//! registered for it. The right-hand panel is the registered panel the
//! session names, or the member list beside a channel.

use crate::ui::focus::Reachable;
use gpui_kit::TestSupportExt;
use gpui_kit::component::Sizable;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::menu::DropdownMenu;
use gpui_kit::prelude::*;
use gpui_kit::*;
use ircord_client::contract::{ConversationKind, MemberStatus, NetworkStatus};
use ircord_client::store::{Connection, parties, people};

use crate::composer::ComposerView;
use crate::members::MembersView;
use crate::registry::{self, BannerCtx, ConversationCtx, HeaderCtx, PaneCtx, PanelCtx};
use crate::route::Route;
use crate::session::{Session, SessionEvent};
use crate::sidebar::SidebarView;
use crate::theme::{self, Theme};
use crate::timeline::TimelineView;
use crate::ui;
use crate::ui::icon::IconName;

pub struct WorkspaceView {
    session: Entity<Session>,
    pub sidebar: Entity<SidebarView>,
    pub timeline: Entity<TimelineView>,
    pub members: Entity<MembersView>,
    pub composer: Entity<ComposerView>,
    notice: Option<String>,
    _subscriptions: Vec<Subscription>,
}

/// What a conversation's header shows.
#[derive(Default)]
struct Header {
    title: String,
    topic: String,
    kind: ConversationKind,
    party: bool,
    favorite: bool,
    online: usize,
    /// The other person's presence, in a direct conversation.
    partner: Option<MemberStatus>,
}

/// What the banner says, and whether it is an error.
enum Banner {
    Warning(String),
    Info(String),
    Danger(String),
}

impl WorkspaceView {
    pub fn session(&self) -> &Entity<Session> {
        &self.session
    }

    pub fn new(session: Entity<Session>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let sidebar = cx.new(|cx| SidebarView::new(session.clone(), cx));
        let timeline = cx.new(|cx| TimelineView::new(session.clone(), cx));
        let members = cx.new(|cx| MembersView::new(session.clone(), cx));
        let composer = cx.new(|cx| ComposerView::new(session.clone(), window, cx));
        let subscriptions =
            vec![
                cx.subscribe(&session, |this, _, event: &SessionEvent, cx| match event {
                    SessionEvent::Notice(text) => {
                        this.notice = Some(text.clone());
                        cx.notify();
                    }
                    SessionEvent::Selection => {
                        this.notice = None;
                        cx.notify();
                    }
                    SessionEvent::Connection | SessionEvent::Phase | SessionEvent::Route => {
                        cx.notify()
                    }
                    SessionEvent::Changed(changes) if changes.sidebar => cx.notify(),
                    _ => {}
                }),
            ];
        Self {
            session,
            sidebar,
            timeline,
            members,
            composer,
            notice: None,
            _subscriptions: subscriptions,
        }
    }

    fn banner(&self, cx: &App) -> Option<Banner> {
        let session = self.session.read(cx);
        let client = session.client()?;
        let w = client.read();
        match &w.connection {
            Some(Connection::Connecting) => {
                return Some(Banner::Warning(t!("core-connecting").into()));
            }
            Some(Connection::Replaying) => {
                return Some(Banner::Info(t!("core-catching-up").into()));
            }
            Some(Connection::Offline { reason, retry_ms }) => {
                return Some(Banner::Danger(t!(
                    "core-offline",
                    reason = reason,
                    seconds = retry_ms.div_ceil(1000)
                )));
            }
            _ => {}
        }
        let network = session.network.as_deref().and_then(|id| w.network(id))?;
        match network.status {
            NetworkStatus::Connected => None,
            NetworkStatus::Connecting => Some(Banner::Warning(t!(
                "core-network-connecting",
                network = network.name
            ))),
            _ => Some(Banner::Danger(match &network.status_detail {
                Some(detail) if !detail.is_empty() => t!(
                    "core-network-down-detail",
                    network = network.name,
                    detail = detail
                ),
                _ => t!("core-network-down", network = network.name),
            })),
        }
    }

    /// A conversation's header: the favourite star, its name and full
    /// topic (a person's picture and name in a direct conversation); at the
    /// right the registered buttons, the member list toggle with the count
    /// online, and the conversation menu.
    fn conversation_header(
        &self,
        id: &str,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let session = self.session.read(cx);
        let members_hidden = crate::members::hidden(session);
        let found = session.client().and_then(|client| {
            let w = client.read();
            w.conversation(id).map(|conv| {
                let partner = (conv.kind != ConversationKind::Channel && conv.party.is_none())
                    .then(|| people::direct_partner(&w, &conv.id).map(|m| m.status))
                    .flatten();
                Header {
                    title: parties::title(&w, conv),
                    topic: ircord_client::format::parse(&conv.topic).text,
                    kind: conv.kind,
                    party: conv.party.is_some(),
                    favorite: conv.favorite,
                    online: people::online_count(&w, id),
                    partner,
                }
            })
        });
        let Header {
            title,
            topic,
            kind,
            party,
            favorite,
            online,
            partner,
        } = found.unwrap_or_default();
        let channel = kind == ConversationKind::Channel && !party;
        let prefix = if party {
            ""
        } else if kind == ConversationKind::Channel {
            "#"
        } else {
            "@"
        };
        let ctx = HeaderCtx {
            session: self.session.clone(),
            route: Route::Conversation(id.to_string()),
        };
        let buttons: Vec<AnyElement> = registry::get(cx)
            .header_buttons
            .to_vec()
            .iter()
            .filter_map(|slot| (slot.render)(&ctx, window, cx))
            .collect();
        let heading = format!("{prefix}{title}");
        let star = {
            let (session, conversation) = (self.session.clone(), id.to_string());
            let label: SharedString = if favorite {
                t!("core-header-unfavourite", name = heading).into()
            } else {
                t!("core-header-favourite", name = heading).into()
            };
            div()
                .id("header-favourite")
                .test_support()
                .role(Role::Button)
                .reachable(theme)
                .aria_label(label.clone())
                .aria_toggled(if favorite {
                    Toggled::True
                } else {
                    Toggled::False
                })
                .tooltip(ui::tooltip::text(label))
                .p_1()
                .rounded_md()
                .cursor_pointer()
                .hover(|d| d.bg(theme.hover))
                .child(ui::icon::icon(
                    if favorite {
                        IconName::StarFill
                    } else {
                        IconName::Star
                    },
                    if favorite { theme.warning } else { theme.muted },
                ))
                .on_click(move |_, _, cx| {
                    crate::sidebar::menus::set_favourite(&session, &conversation, !favorite, cx)
                })
        };
        let title_row = div()
            .id("conversation-title")
            .test_support()
            .role(Role::Heading)
            .aria_level(1)
            .aria_label(SharedString::from(heading.clone()))
            .flex_none()
            .flex()
            .items_center()
            .gap_1()
            .text_lg()
            .font_weight(FontWeight::BOLD)
            .text_color(theme.text);
        let title_row = match partner {
            Some(status) => title_row
                .child(
                    div()
                        .relative()
                        .mr_1()
                        .child(ui::avatar::avatar(&title, None, px(26.), theme).text_sm())
                        .child(
                            div()
                                .id("conversation-presence")
                                .test_support()
                                .absolute()
                                .bottom(px(-2.))
                                .right(px(-2.))
                                .size(px(10.))
                                .rounded_full()
                                .border_2()
                                .border_color(theme.main)
                                .bg(match status {
                                    MemberStatus::Online => theme.online,
                                    MemberStatus::Away => theme.away,
                                    _ => theme.offline,
                                }),
                        ),
                )
                .child(title),
            None => title_row
                .child(div().text_color(theme.faint).child(prefix))
                .child(title),
        };
        let members = channel.then(|| {
            let session = self.session.clone();
            let label = if members_hidden {
                t!("core-header-members-show", online = online)
            } else {
                t!("core-header-members-hide", online = online)
            };
            div()
                .id("members-toggle")
                .test_support()
                .role(Role::Button)
                .reachable(theme)
                .aria_label(SharedString::from(label.clone()))
                .aria_toggled(if members_hidden {
                    Toggled::False
                } else {
                    Toggled::True
                })
                .tooltip(ui::tooltip::text(label))
                .h(px(28.))
                .px_2()
                .rounded_full()
                .flex()
                .items_center()
                .gap_1()
                .cursor_pointer()
                .text_sm()
                .when(!members_hidden, |d| d.bg(theme.selected))
                .hover(|d| d.bg(theme.hover))
                .child(ui::icon::icon(IconName::User, theme.muted))
                .child(div().size(px(6.)).rounded_full().bg(theme.online))
                .child(div().text_color(theme.text).child(online.to_string()))
                .on_click(move |_, _, cx| crate::members::set_hidden(&session, !members_hidden, cx))
        });
        let menu_ctx = ConversationCtx {
            session: self.session.clone(),
            conversation: id.to_string(),
        };
        let more = Button::new("conversation-menu")
            .ghost()
            .small()
            .icon(IconName::Ellipsis)
            .accessibility_label(t!("core-header-more"))
            .tooltip(t!("core-header-more"))
            .dropdown_menu_with_anchor(Anchor::TopRight, move |menu, _, cx| {
                let client = menu_ctx.session.read(cx).client().cloned();
                crate::sidebar::layout::watch_notification_settings(client, cx);
                let items = registry::get(cx).conversation_menu.to_vec();
                let items = ui::context_menu::visible(&items, &menu_ctx, cx);
                ui::context_menu::fill(menu, items, menu_ctx.clone())
            });
        div()
            .id("conversation-header")
            .test_support()
            .role(Role::Banner)
            .aria_label(SharedString::from(heading))
            .h(px(52.))
            .flex_none()
            .pl_3()
            .pr_3()
            .flex()
            .items_center()
            .gap_2()
            .border_b_1()
            .border_color(theme.border)
            .when(!party, |d| d.child(star))
            .child(title_row)
            .child(
                div()
                    .id("conversation-topic")
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_sm()
                    .text_color(theme.muted)
                    .when(!topic.is_empty(), |d| {
                        d.tooltip(ui::tooltip::text(topic.clone())).child(topic)
                    }),
            )
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap_1()
                    .children(buttons)
                    .children(members)
                    .child(more),
            )
    }

    /// The registered pane for a route, or a placeholder when none is.
    fn route_pane(
        &self,
        route: &Route,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let pane = route
            .pane()
            .and_then(|kind| registry::get(cx).pane(&kind).cloned());
        match pane {
            Some(pane) => {
                let ctx = PaneCtx {
                    session: self.session.clone(),
                    route: route.clone(),
                };
                (pane.render)(&ctx, window, cx)
            }
            None => ui::empty::empty("pane-missing", t!("core-pane-missing"), None, theme)
                .into_any_element(),
        }
    }

    /// The right-hand panel: the one the session names, else the member
    /// list beside a channel.
    fn panel(
        &self,
        route: &Option<Route>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let session = self.session.read(cx);
        let conversation = route
            .as_ref()
            .and_then(|r| r.conversation())
            .map(str::to_string);
        if session.panel.as_deref() == Some(crate::session::NO_PANEL) {
            return None;
        }
        if let Some(id) = session.panel.clone()
            && let Some(panel) = registry::get(cx).panels.get(&id).cloned()
        {
            let ctx = PanelCtx {
                session: self.session.clone(),
                conversation,
            };
            return Some((panel.render)(&ctx, window, cx));
        }
        let is_channel = conversation.as_ref().is_some_and(|id| {
            session
                .client()
                .and_then(|c| c.read().conversation(id).map(|c| c.kind))
                .is_none_or(|kind| kind == ConversationKind::Channel)
        });
        is_channel.then(|| self.members.clone().into_any_element())
    }
}

impl Render for WorkspaceView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme::get(cx).clone();
        let route = self.session.read(cx).route();
        let mut main = div()
            .id("main-pane")
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .bg(theme.main);
        if let Some(Route::Conversation(id)) = &route {
            main = main.child(self.conversation_header(id, &theme, window, cx));
        }
        if let Some(banner) = self.banner(cx) {
            let (text, color) = match banner {
                Banner::Warning(text) => (text, theme.warning),
                Banner::Info(text) => (text, theme.accent),
                Banner::Danger(text) => (text, theme.danger),
            };
            main = main.child(
                div()
                    .id("connection-banner")
                    .test_support()
                    .role(Role::Status)
                    .aria_label(SharedString::from(text.clone()))
                    .px_4()
                    .py_1()
                    .text_sm()
                    .text_color(theme.on_accent)
                    .bg(color)
                    .child(text),
            );
        }
        let ctx = BannerCtx {
            session: self.session.clone(),
            route: route.clone(),
        };
        let banners: Vec<AnyElement> = registry::get(cx)
            .banners
            .to_vec()
            .iter()
            .filter_map(|slot| (slot.render)(&ctx, window, cx))
            .collect();
        main = main.children(banners);
        if let Some(notice) = &self.notice {
            main = main.child(
                div()
                    .id("notice")
                    .test_support()
                    .role(Role::Status)
                    .aria_label(SharedString::from(notice.clone()))
                    .px_4()
                    .py_1()
                    .text_sm()
                    .text_color(theme.warning)
                    .bg(theme.sidebar)
                    .child(notice.clone()),
            );
        }
        main = match &route {
            Some(Route::Conversation(_)) => main
                .child(self.timeline.clone())
                .child(self.composer.clone()),
            Some(route) => main.child(self.route_pane(route, &theme, window, cx)),
            None => main,
        };
        let panel = self
            .panel(&route, window, cx)
            .map(|panel| crate::sidebar::side_panel(panel, window, cx));
        let card = div()
            .id("workspace-card")
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_row()
            .overflow_hidden()
            .rounded(theme.radius_large)
            .border_1()
            .border_color(theme.border)
            .bg(theme.main)
            .child(main)
            .children(panel);
        div()
            .id("workspace")
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.sidebar)
            .text_color(theme.text)
            .child(crate::sidebar::topbar::bar(&self.session, window, cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_row()
                    .pb_2()
                    .pr_2()
                    .child(self.sidebar.clone())
                    .child(card),
            )
    }
}

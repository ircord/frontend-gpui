//! The signed-in layout: rail and sidebar, the connection banner and the
//! registered banners, the main pane, and the right-hand panel.
//!
//! The main pane shows the session's route: a conversation (header,
//! timeline, composer) is drawn here; any other route by the pane a feature
//! registered for it. The right-hand panel is the registered panel the
//! session names, or the member list beside a channel.

use gpui_kit::TestSupportExt;
use gpui_kit::prelude::*;
use gpui_kit::*;
use ircord_client::contract::{ConversationKind, NetworkStatus};
use ircord_client::store::{Connection, parties};

use crate::composer::ComposerView;
use crate::members::MembersView;
use crate::registry::{self, BannerCtx, HeaderCtx, PaneCtx, PanelCtx};
use crate::route::Route;
use crate::session::{Session, SessionEvent};
use crate::sidebar::SidebarView;
use crate::theme::{self, Theme};
use crate::timeline::TimelineView;
use crate::ui;

pub struct WorkspaceView {
    session: Entity<Session>,
    pub sidebar: Entity<SidebarView>,
    pub timeline: Entity<TimelineView>,
    pub members: Entity<MembersView>,
    pub composer: Entity<ComposerView>,
    notice: Option<String>,
    _subscriptions: Vec<Subscription>,
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

    /// A conversation's header.
    fn conversation_header(
        &self,
        id: &str,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let session = self.session.read(cx);
        let (title, topic, kind, party) = session
            .client()
            .and_then(|client| {
                let w = client.read();
                w.conversation(id).map(|conv| {
                    (
                        parties::title(&w, conv),
                        ircord_client::format::parse(&conv.topic).text,
                        conv.kind,
                        conv.party.is_some(),
                    )
                })
            })
            .unwrap_or((
                String::new(),
                String::new(),
                ConversationKind::Channel,
                false,
            ));
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
        div()
            .id("conversation-header")
            .h(px(48.))
            .flex_none()
            .px_4()
            .flex()
            .items_center()
            .gap_3()
            .border_b_1()
            .border_color(theme.rail)
            .child(
                div()
                    .flex()
                    .gap_1()
                    .font_weight(FontWeight::BOLD)
                    .text_color(theme.text)
                    .child(div().text_color(theme.faint).child(prefix))
                    .child(title),
            )
            .when(!topic.is_empty(), |d| {
                d.child(div().h(px(24.)).w(px(1.)).bg(theme.selected))
                    .child(
                        div()
                            .flex_1()
                            .truncate()
                            .text_sm()
                            .text_color(theme.muted)
                            .child(topic),
                    )
            })
            .when(!buttons.is_empty(), |d| {
                d.child(div().flex_1())
                    .child(div().flex().gap_1().children(buttons))
            })
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
        div()
            .id("workspace")
            .size_full()
            .flex()
            .flex_row()
            .text_color(theme.text)
            .child(self.sidebar.clone())
            .child(main)
            .children(panel)
    }
}

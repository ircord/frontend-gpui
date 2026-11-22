//! The signed-in layout, as the web client's: the top bar with the network
//! tabs across the window, the sidebar under it, and beside it a card
//! holding the main pane (header, connection banner, registered banners,
//! the route) and the right-hand panel.
//!
//! The main pane shows the session's route: a conversation (header,
//! timeline, composer) is drawn here; any other route by the pane a feature
//! registered for it. The right-hand panel is the registered panel the
//! session names, or the member list beside a channel.

use std::sync::Arc;

use crate::ui::focus::Reachable;
use gpui_kit::TestSupportExt;
use gpui_kit::base::FocusTrapElement;
use gpui_kit::component::Sizable;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::menu::DropdownMenu;
use gpui_kit::prelude::*;
use gpui_kit::*;
use ircord_client::contract::{ConversationKind, MemberStatus, NetworkStatus};
use ircord_client::store::{Connection, parties, people};

use crate::composer::ComposerView;
use crate::features::shortcuts::MarkRead;
use crate::members::{self, MembersView};
use crate::registry::{self, BannerCtx, ConversationCtx, HeaderCtx, PaneCtx, PanelCtx};
use crate::route::Route;
use crate::session::{NO_PANEL, Session, SessionEvent};
use crate::sidebar::SidebarView;
use crate::theme::background::{self, Wallpaper};
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
    wallpaper: Entity<Wallpaper>,
    notice: Option<String>,
    _subscriptions: Vec<Subscription>,
}

/// The narrow-window drawer is local to this window. Docking uses the saved
/// member-list preference instead.
struct MembersDrawer {
    conversation: Option<String>,
    scope: FocusHandle,
    close_button: FocusHandle,
    opener: Option<FocusHandle>,
}

impl MembersDrawer {
    fn close(&mut self, window: &mut Window, cx: &mut App) {
        self.conversation = None;
        if let Some(opener) = self.opener.take()
            && self.scope.contains_focused(window, cx)
        {
            opener.focus(window, cx);
        }
        window.refresh();
    }
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
            wallpaper: cx.new(|_| Wallpaper::default()),
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
        drawer: &Entity<MembersDrawer>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let session = self.session.read(cx);
        let compact = window.viewport_size().width < members::DRAWER_BREAKPOINT;
        let members_visible = if compact {
            drawer.read(cx).conversation.as_deref() == Some(id)
        } else {
            session.panel.is_none()
        };
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
            .min_w_0()
            .max_w(relative(0.55))
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
                .child(div().truncate().child(title)),
            None => title_row
                .child(div().text_color(theme.accent).child(prefix))
                .child(div().truncate().child(title)),
        };
        let members = channel.then(|| {
            let session = self.session.clone();
            let drawer = drawer.clone();
            let id = id.to_string();
            let label = if members_visible {
                t!("core-header-members-hide", online = online)
            } else {
                t!("core-header-members-show", online = online)
            };
            div()
                .id("members-toggle")
                .test_support()
                .role(Role::Button)
                .reachable(theme)
                .aria_label(SharedString::from(label.clone()))
                .aria_toggled(if members_visible {
                    Toggled::True
                } else {
                    Toggled::False
                })
                .tooltip(ui::tooltip::text(label))
                .h(px(32.))
                .px_2p5()
                .rounded_full()
                .flex()
                .items_center()
                .gap_1p5()
                .cursor_pointer()
                .text_xs()
                .bg(theme.raised.opacity(0.45))
                .when(members_visible, |d| {
                    d.bg(linear_gradient(
                        90.,
                        linear_color_stop(theme.raised.blend(theme.accent.opacity(0.22)), 0.),
                        linear_color_stop(theme.raised.blend(theme.accent.opacity(0.08)), 1.),
                    ))
                })
                .hover(|d| d.bg(theme.hover))
                .border_1()
                .border_color(if members_visible {
                    theme.accent.opacity(0.35)
                } else {
                    theme.border.opacity(0.55)
                })
                .child(ui::icon::icon(
                    ui::icon::LucideIcon::Users,
                    if members_visible {
                        theme.accent
                    } else {
                        theme.muted
                    },
                ))
                .child(div().size(px(6.)).rounded_full().bg(theme.online))
                .child(div().text_color(theme.text).child(online.to_string()))
                .on_click(move |_, window, cx| {
                    if !compact {
                        members::set_hidden(&session, members_visible, cx);
                        return;
                    }
                    if !members_visible {
                        session.update(cx, |session, cx| {
                            if session.panel.as_deref().is_some_and(|id| id != NO_PANEL) {
                                let hidden =
                                    crate::features::profile::saved_members_hidden(session);
                                session.set_panel(hidden.then(|| NO_PANEL.into()), cx);
                            }
                        });
                    }
                    drawer.update(cx, |drawer, cx| {
                        if members_visible {
                            drawer.close(window, cx);
                        } else {
                            drawer.conversation = Some(id.clone());
                            drawer.opener = window.focused(cx);
                            drawer.close_button.focus(window, cx);
                            window.refresh();
                        }
                    });
                })
        });
        let menu_ctx = ConversationCtx {
            session: self.session.clone(),
            conversation: id.to_string(),
        };
        let more = Button::new("conversation-menu")
            .ghost()
            .small()
            .rounded(px(16.))
            .size(px(32.))
            .bg(theme.raised.opacity(0.45))
            .border_1()
            .border_color(theme.border.opacity(0.55))
            .icon(IconName::Ellipsis)
            .accessibility_label(t!("core-header-more"))
            .tooltip(t!("core-header-more"))
            .dropdown_menu_with_anchor(Anchor::TopRight, move |menu, window, cx| {
                let client = menu_ctx.session.read(cx).client().cloned();
                crate::sidebar::layout::watch_notification_settings(client, cx);
                let items = registry::get(cx).conversation_menu.to_vec();
                let items = ui::context_menu::visible(&items, &menu_ctx, cx);
                ui::context_menu::fill(menu, items, menu_ctx.clone(), window, cx)
            });
        div()
            .id("conversation-header")
            .test_support()
            .role(Role::Banner)
            .aria_label(SharedString::from(heading))
            .relative()
            .h(rems(3.))
            .flex_none()
            .px_4()
            .flex()
            .items_center()
            .gap_2()
            .child(header_divider(theme))
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
                    .gap_1p5()
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
        dock_members: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let session = self.session.read(cx);
        let conversation = route
            .as_ref()
            .and_then(|r| r.conversation())
            .map(str::to_string);
        if members::hidden(session) {
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
        let is_channel = dock_members
            && conversation.as_ref().is_some_and(|id| {
                session
                    .client()
                    .and_then(|c| {
                        c.read()
                            .conversation(id)
                            .map(|c| c.kind == ConversationKind::Channel && c.party.is_none())
                    })
                    .unwrap_or(true)
            });
        is_channel.then(|| self.members.clone().into_any_element())
    }

    fn members_drawer(
        &self,
        state: &Entity<MembersDrawer>,
        theme: &Theme,
        window: &Window,
        cx: &App,
    ) -> impl IntoElement + use<> {
        let focus = state.read(cx);
        let search = Button::new("members-drawer-search")
            .ghost()
            .small()
            .h(px(32.))
            .flex_1()
            .justify_start()
            .bg(theme.input)
            .border_1()
            .border_color(theme.border)
            .icon(IconName::Search)
            .label(t!("sidebar-find"))
            .on_click({
                let state = state.clone();
                let session = self.session.clone();
                move |_, window, cx| {
                    state.update(cx, |drawer, cx| drawer.close(window, cx));
                    crate::features::search::open(&session, None, cx);
                }
            });
        let close_button = Button::new("hide-members")
            .ghost()
            .small()
            .icon(IconName::PanelRightClose)
            .track_focus(&focus.close_button)
            .accessibility_label(t!("people-members-hide"))
            .tooltip(t!("people-members-hide"))
            .on_click({
                let state = state.clone();
                move |_, window, cx| state.update(cx, |drawer, cx| drawer.close(window, cx))
            });
        let contents = div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .h(rems(3.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .child(search)
                    .child(close_button),
            )
            .child(div().flex_1().min_h_0().child(self.members.clone()))
            .into_any_element();
        let state = state.clone();
        right_drawer(
            ("members-drawer", "members-scrim", t!("people-members")),
            contents,
            &focus.scope,
            theme,
            window,
            move |window, cx| state.update(cx, |drawer, cx| drawer.close(window, cx)),
        )
    }
}

impl Render for WorkspaceView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme::get(cx).clone();
        let session = self.session.read(cx);
        let settings_open = session.route() == Some(Route::Settings);
        let route = session.workspace_route();
        let client = session.client().cloned();
        self.wallpaper.update(cx, |wallpaper, cx| {
            wallpaper.select(client.as_ref().zip(theme.background.as_ref()), window, cx);
        });
        let compact = window.viewport_size().width < members::DRAWER_BREAKPOINT;
        let drawer = window.use_keyed_state("member-drawer", cx, |_, cx| MembersDrawer {
            conversation: None,
            scope: cx.focus_handle(),
            close_button: cx.focus_handle().tab_stop(true),
            opener: None,
        });
        let conversation = route.as_ref().and_then(Route::conversation);
        let drawer_open = drawer.read(cx).conversation.is_some();
        let show_drawer = compact
            && drawer_open
            && drawer.read(cx).conversation.as_deref() == conversation
            && self
                .session
                .read(cx)
                .panel
                .as_deref()
                .is_none_or(|id| id == NO_PANEL);
        if drawer_open && !show_drawer {
            drawer.update(cx, |drawer, cx| drawer.close(window, cx));
        }
        let mut main = div()
            .id("main-pane")
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col();
        let header = match &route {
            Some(Route::Conversation(id)) => {
                Some(self.conversation_header(id, &theme, &drawer, window, cx))
            }
            _ => None,
        };
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
        let searching = conversation.is_some()
            && self.session.read(cx).panel.as_deref() == Some(crate::features::search::PANEL);
        let panel = self.panel(&route, !compact, window, cx);
        let (panel, search_drawer) = if compact && searching {
            (None, panel)
        } else {
            (
                panel.map(|panel| {
                    crate::sidebar::side_panel(panel, searching.then_some(340.), window, cx)
                }),
                None,
            )
        };
        let card_corners = Corners {
            top_left: theme.radius_large,
            top_right: theme.radius_large,
            ..Default::default()
        };
        let app_background = theme.app_background();
        let chat_background = theme
            .background
            .as_ref()
            .filter(|_| app_background.is_none());
        let card = div()
            .id("workspace-card")
            .relative()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .rounded_t(theme.radius_large)
            .border_1()
            .border_b_0()
            .border_color(theme.border.opacity(0.65))
            .bg(theme
                .main
                .opacity(app_background.map_or(1., |bg| f32::from(bg.dim) / 100.)))
            .shadow(frame_shadows(&theme))
            .when_some(chat_background, |card, bg| {
                card.child(background::layer(
                    self.wallpaper.clone(),
                    bg,
                    1. - f32::from(bg.dim) / 100.,
                    card_corners,
                ))
            })
            .when(
                theme.hibiki && (header.is_some() || route == Some(Route::Search)),
                |d| d.child(ambient_light(&theme, window, cx)),
            )
            .children(header)
            .child(div().flex_1().min_h_0().flex().child(main).children(panel));
        div()
            .id("workspace")
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.sidebar)
            .text_color(theme.text)
            .when_some(app_background, |workspace, bg| {
                workspace.child(background::layer(
                    self.wallpaper.clone(),
                    bg,
                    1. - f32::from(bg.panel_opacity) / 100.,
                    Corners::default(),
                ))
            })
            .child(crate::sidebar::topbar::frame(&self.session, window, cx))
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_row()
                    .pt(px(5.))
                    .pr_2()
                    .child(self.sidebar.clone())
                    .child(card)
                    .when(show_drawer, |body| {
                        body.child(self.members_drawer(&drawer, &theme, window, cx))
                    })
                    .when_some(search_drawer, |body, search| {
                        let session = self.session.clone();
                        body.child(right_drawer(
                            (
                                "conversation-search-drawer",
                                "conversation-search-scrim",
                                t!("search-in-conversation"),
                            ),
                            search,
                            &drawer.read(cx).scope,
                            &theme,
                            window,
                            move |window, cx| {
                                crate::features::search::toggle_panel(&session, false, window, cx)
                            },
                        ))
                    }),
            )
            .when(settings_open, |workspace| {
                let settings = self.route_pane(&Route::Settings, &theme, window, cx);
                let session = self.session.clone();
                let top = if cfg!(target_os = "macos") && !window.is_fullscreen() {
                    px(crate::MAC_TITLEBAR_HEIGHT)
                } else {
                    px(0.)
                };
                workspace.child(
                    div()
                        .absolute()
                        .top(top)
                        .left_0()
                        .right_0()
                        .bottom_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .p_6()
                        .child(
                            div()
                                .id("settings-scrim")
                                .test_support()
                                .absolute()
                                .inset_0()
                                .occlude()
                                .bg(black().opacity(0.55))
                                .on_click(move |_, _, cx| crate::settings::close(&session, cx)),
                        )
                        .child(
                            div()
                                .w_full()
                                .max_w(px(1052.))
                                .h_full()
                                .max_h(px(836.))
                                .occlude()
                                .rounded(theme.radius_large)
                                .overflow_hidden()
                                .bg(theme.main)
                                .border_1()
                                .border_color(theme.border.opacity(0.65))
                                .shadow(frame_shadows(&theme))
                                .child(settings),
                        ),
                )
            })
    }
}

fn right_drawer<D>(
    (id, scrim, label): (&'static str, &'static str, &'static str),
    contents: AnyElement,
    focus: &FocusHandle,
    theme: &Theme,
    window: &Window,
    dismiss: D,
) -> impl IntoElement + use<D>
where
    D: Fn(&mut Window, &mut App) + Clone + 'static,
{
    let escape = dismiss.clone();
    let panel = div()
        .id(id)
        .test_support()
        .role(Role::Dialog)
        .aria_label(label)
        .a11y_synthetic_children(|tree| tree.parent_node().set_modal())
        .absolute()
        .right_0()
        .top_0()
        .bottom_0()
        .w(px(300.).min(window.viewport_size().width * 0.85))
        .flex()
        .flex_col()
        .occlude()
        .bg(theme.main)
        .border_l_1()
        .border_color(theme.border.opacity(0.65))
        .shadow(frame_shadows(theme))
        .on_action(move |_: &MarkRead, window, cx| {
            cx.stop_propagation();
            escape(window, cx);
        })
        .child(contents)
        .focus_trap(id, focus);
    div()
        .absolute()
        .inset_0()
        .child(
            div()
                .id(scrim)
                .test_support()
                .absolute()
                .inset_0()
                .bg(black().opacity(0.5))
                .occlude()
                .on_click(move |_, window, cx| dismiss(window, cx)),
        )
        .child(panel)
}

fn header_divider(theme: &Theme) -> Div {
    let edge = linear_color_stop(theme.border.opacity(0.), 0.);
    let line = theme.border.opacity(0.45);
    let center = linear_color_stop(line, 1.);
    div()
        .absolute()
        .left_4()
        .right_4()
        .bottom_0()
        .h(px(1.))
        .flex()
        .child(
            div()
                .w(relative(0.2))
                .bg(linear_gradient(90., edge, center)),
        )
        .child(div().flex_1().bg(line))
        .child(
            div()
                .w(relative(0.2))
                .bg(linear_gradient(270., edge, center)),
        )
}

fn frame_shadows(theme: &Theme) -> Vec<BoxShadow> {
    let light_shadow = theme::hex(0x1e1432);
    let (shadow, outline) = if theme.dark {
        (black().opacity(0.35), white().opacity(0.04))
    } else {
        (light_shadow.opacity(0.16), light_shadow.opacity(0.06))
    };
    vec![
        BoxShadow {
            color: shadow,
            offset: point(px(0.), px(12.)),
            blur_radius: px(40.),
            spread_radius: px(0.),
            inset: false,
        },
        BoxShadow {
            color: outline,
            offset: point(px(0.), px(0.)),
            blur_radius: px(0.),
            spread_radius: px(1.),
            inset: false,
        },
    ]
}

#[derive(Clone, Copy, PartialEq)]
struct AmbientColors {
    main: Rgba,
    warm: Rgba,
    amber: Rgba,
}

const AMBIENT_BLEED: f32 = 8.;

fn ambient_light(theme: &Theme, window: &mut Window, cx: &mut App) -> impl IntoElement {
    let colors = AmbientColors {
        main: theme
            .main
            .opacity(if theme.background.is_some() { 0. } else { 1. })
            .into(),
        warm: theme
            .accent
            .blend(theme.danger.opacity(0.35))
            .opacity(if theme.dark { 0.18 } else { 0.10 })
            .into(),
        amber: theme
            .warning
            .opacity(if theme.dark { 0.11 } else { 0.06 })
            .into(),
    };
    let corners = Corners {
        top_left: theme.radius_large,
        top_right: theme.radius_large,
        ..Default::default()
    };
    let cached = window.use_keyed_state("workspace-ambient-light", cx, |_, _| {
        None::<(Size<Pixels>, AmbientColors, Arc<RenderImage>)>
    });
    canvas(
        move |bounds, window, cx| {
            if let Some((size, tint, texture)) = cached.read(cx)
                && *size == bounds.size
                && *tint == colors
            {
                return texture.clone();
            }
            let texture = render_ambient_light(bounds.size, colors);
            let old = cached.update(cx, |cache, _| {
                cache.replace((bounds.size, colors, texture.clone()))
            });
            if let Some((_, _, old)) = old {
                cx.drop_image(old, Some(window));
            }
            texture
        },
        move |bounds, image, window, _| {
            let _ = window.paint_image(
                bounds,
                bounds.dilate(px(AMBIENT_BLEED)),
                corners,
                image,
                0,
                false,
            );
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .w_full()
    .h(px(216.))
}

fn render_ambient_light(size: Size<Pixels>, colors: AmbientColors) -> Arc<RenderImage> {
    // Match the two ellipses in Nuxt's workspace-layout.css. Sampling every
    // four logical pixels is enough for the soft falloff.
    let (width, height) = (f32::from(size.width), f32::from(size.height));
    // Keep the visible edge inside the tile so the sampler never sees an
    // adjacent image in the atlas.
    let padded_width = width + 2. * AMBIENT_BLEED;
    let padded_height = height + 2. * AMBIENT_BLEED;
    let columns = (padded_width / 4.).ceil() as u32;
    let rows = (padded_height / 4.).ceil() as u32;
    let buffer = image::RgbaImage::from_fn(columns, rows, |i, j| {
        let x = padded_width * (i as f32 + 0.5) / columns as f32 - AMBIENT_BLEED;
        let y = padded_height * (j as f32 + 0.5) / rows as f32 - AMBIENT_BLEED;
        let warm = 1.
            - ((x - width * 0.08) / 489.6)
                .hypot((y + 30.) / 244.8)
                .min(1.);
        let amber = 1. - ((x - width * 0.36) / 378.).hypot((y + 20.) / 196.).min(1.);
        let color = overlay(
            overlay(colors.main, colors.amber.alpha(colors.amber.a * amber)),
            colors.warm.alpha(colors.warm.a * warm),
        );
        // GPUI's texture atlas expects BGRA.
        image::Rgba([
            (color.b * 255.).round() as u8,
            (color.g * 255.).round() as u8,
            (color.r * 255.).round() as u8,
            (color.a * 255.).round() as u8,
        ])
    });
    Arc::new(RenderImage::new(vec![image::Frame::new(buffer)]))
}

fn overlay(base: Rgba, top: Rgba) -> Rgba {
    let covered_base = base.a * (1. - top.a);
    let alpha = top.a + covered_base;
    if alpha == 0. {
        return base;
    }
    Rgba {
        r: (top.r * top.a + base.r * covered_base) / alpha,
        g: (top.g * top.a + base.g * covered_base) / alpha,
        b: (top.b * top.a + base.b * covered_base) / alpha,
        a: alpha,
    }
}

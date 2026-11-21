//! The network rail and the conversation list (package N1).
//!
//! - The rail shows each network's own icon, or a pattern drawn from its
//!   name, with a dot only when something needs a look: a ring while it is
//!   not connected, a large dot for mentions, a small one for unread.
//! - The list has Favourites, Channels and Direct messages, each collapsible
//!   per network on this device. Favourites and channels can be dragged into
//!   your own order (or moved from the menu), which follows the account;
//!   Sort A to Z forgets it. The first nine channels show their Ctrl+number.
//! - The list folds to an icon rail and back, and is as wide as you drag it;
//!   so is the right-hand panel ([`side_panel`]).
//! - Right-clicking a network or a conversation opens its registered menu;
//!   N1's own entries are in [`menus`].
//!
//! Redraws when the sidebar's data, the documents it reads, the selection,
//! the connection or the layout change.

pub mod glyph;
pub mod layout;
pub mod menus;

use gpui_kit::TestSupportExt;
use gpui_kit::component::menu::ContextMenuExt;
use gpui_kit::prelude::*;
use gpui_kit::*;
use ircord_client::contract::{Conversation, ConversationKind, NetworkStatus};
use ircord_client::store::sidebar_order::{
    self as order, Activity, Filter, PANEL_WIDTH, Place, SIDEBAR_WIDTH, Section,
};
use ircord_client::store::{Workspace, topic};

use crate::registry::{self, ConversationCtx, NetworkCtx};
use crate::session::{Session, SessionEvent};
use crate::theme::{self, Theme};
use crate::ui;
use crate::ui::icon::IconName;

/// The rail's width.
const RAIL: f32 = 72.;
/// The folded list's width.
const FOLDED: f32 = 56.;

/// Set up the layout state and register the menus. Called from
/// `features::shortcuts::init`.
pub fn init(cx: &mut App) {
    layout::init(cx);
    menus::register(cx);
}

/// A conversation being dragged to a new place in its section.
#[derive(Clone)]
pub struct DraggedConversation {
    pub id: String,
    pub network: String,
    pub section: Section,
    pub name: SharedString,
}

struct DragPreview(SharedString);

impl Render for DragPreview {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme::get(cx);
        div()
            .px_2()
            .py_1()
            .rounded_md()
            .bg(theme.selected)
            .text_color(theme.text)
            .text_sm()
            .child(self.0.clone())
    }
}

/// Dragging a panel's edge.
#[derive(Clone, Copy)]
struct ResizeList;
#[derive(Clone, Copy)]
struct ResizePanel;

struct Nothing;
impl Render for Nothing {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

/// An edge to drag, `id` for tests.
fn handle<T: Clone + 'static>(
    id: &'static str,
    label: SharedString,
    theme: &Theme,
    drag: T,
) -> impl IntoElement {
    div()
        .id(id)
        .test_support()
        .role(Role::Splitter)
        .aria_label(label)
        .w(px(5.))
        .h_full()
        .flex_none()
        .cursor_col_resize()
        .hover({
            let accent = theme.accent;
            move |d| d.bg(accent.opacity(0.4))
        })
        .on_drag(drag, |_, _, _, cx| cx.new(|_| Nothing))
}

/// The right-hand panel (the member list or a feature's panel) in its frame
/// beside the main pane, as wide as the person dragged it.
pub fn side_panel(panel: AnyElement, _window: &mut Window, cx: &mut App) -> AnyElement {
    let theme = theme::get(cx).clone();
    let width = layout::get(cx).panel_width;
    div()
        .id("side-panel")
        .w(px(width))
        .h_full()
        .flex_none()
        .flex()
        .flex_row()
        .on_drag_move::<ResizePanel>(|e, _, cx| {
            let width = f32::from(e.bounds.right() - e.event.position.x);
            layout::update(cx, |l| {
                l.panel_width = order::clamp_width(width, PANEL_WIDTH)
            });
        })
        .child(handle(
            "resize-panel",
            t!("sidebar-resize-panel").into(),
            &theme,
            ResizePanel,
        ))
        .child(div().flex_1().min_w_0().h_full().child(panel))
        .into_any_element()
}

pub struct SidebarView {
    session: Entity<Session>,
    /// Holds the keyboard when nothing else does, so the workspace keys
    /// (bound in the window's root context) work before anything is clicked.
    focus: FocusHandle,
    focused_once: bool,
    _subscription: Subscription,
}

impl SidebarView {
    pub fn new(session: Entity<Session>, cx: &mut Context<Self>) -> Self {
        let subscription = cx.subscribe(&session, |_, _, event: &SessionEvent, cx| match event {
            SessionEvent::Changed(changes)
                if changes.sidebar || changes.reset || changes.touches(topic::DOCUMENTS) =>
            {
                cx.notify()
            }
            SessionEvent::Selection
            | SessionEvent::Phase
            | SessionEvent::Connection
            | SessionEvent::Route => cx.notify(),
            _ => {}
        });
        let client = session.read(cx).client().cloned();
        layout::watch_documents(client, cx);
        Self {
            session,
            focus: cx.focus_handle(),
            focused_once: false,
            _subscription: subscription,
        }
    }
}

fn badge(count: i64, theme: &Theme) -> Div {
    let text = if count > 99 {
        t!("sidebar-badge-many").to_string()
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
        .bg(theme.mention)
        .text_color(theme.on_accent)
        .child(text)
}

/// What a screen reader says for a network tab: its name, then a problem
/// with the connection, then mentions or unread.
fn network_label(name: &str, status: NetworkStatus, activity: Activity) -> String {
    let mut parts = vec![name.to_string()];
    match status {
        NetworkStatus::Connected => {}
        NetworkStatus::Connecting => parts.push(t!("sidebar-network-connecting").into()),
        NetworkStatus::Error => parts.push(t!("sidebar-network-error").into()),
        _ => parts.push(t!("sidebar-network-disconnected").into()),
    }
    match activity {
        Activity::Mention => parts.push(t!("sidebar-network-mentions").into()),
        Activity::Unread => parts.push(t!("sidebar-network-unread").into()),
        Activity::Quiet => {}
    }
    parts.join(", ")
}

fn prefix(c: &Conversation) -> &'static str {
    if c.kind == ConversationKind::Channel {
        "#"
    } else {
        "@"
    }
}

/// The Ctrl+number label for the `n`th (1-based) channel.
fn number_label(n: usize) -> String {
    let modifier = if cfg!(target_os = "macos") {
        "⌘"
    } else {
        "Ctrl"
    };
    format!("{modifier} {n}")
}

impl SidebarView {
    fn rail(&self, w: &Workspace, selected: Option<&str>, theme: &Theme) -> AnyElement {
        let mut rail = div()
            .id("network-rail")
            .test_support()
            .role(Role::Navigation)
            .aria_label(t!("sidebar-networks"))
            .w(px(RAIL))
            .h_full()
            .flex_none()
            .flex()
            .flex_col()
            .items_center()
            .gap_2()
            .py_3()
            .bg(theme.rail)
            .overflow_y_scroll();
        for network in &w.networks {
            let activity = order::network_activity(w, &network.id);
            let active = selected == Some(network.id.as_str());
            let id = network.id.clone();
            let session = self.session.clone();
            let label = network_label(&network.name, network.status, activity);
            let dot = if network.status != NetworkStatus::Connected {
                Some(
                    div()
                        .id(ElementId::Name(
                            format!("network-{}-offline", network.id).into(),
                        ))
                        .test_support()
                        .size(px(12.))
                        .rounded_full()
                        .border_2()
                        .border_color(theme.warning)
                        .bg(theme.rail),
                )
            } else {
                match activity {
                    Activity::Mention => Some(
                        div()
                            .id(ElementId::Name(
                                format!("network-{}-mention", network.id).into(),
                            ))
                            .test_support()
                            .size(px(14.))
                            .rounded_full()
                            .border_2()
                            .border_color(theme.rail)
                            .bg(theme.danger),
                    ),
                    Activity::Unread if !active => Some(
                        div()
                            .id(ElementId::Name(
                                format!("network-{}-unread", network.id).into(),
                            ))
                            .test_support()
                            .size(px(12.))
                            .rounded_full()
                            .border_2()
                            .border_color(theme.rail)
                            .bg(theme.text),
                    ),
                    _ => None,
                }
            };
            let tooltip = match network.status_detail.as_deref() {
                Some(detail) if !detail.is_empty() => {
                    t!(
                        "sidebar-network-tooltip",
                        network = network.name,
                        status = detail
                    )
                }
                _ => label.clone(),
            };
            let tile = glyph::network_icon(w, network, px(34.));
            let ctx = NetworkCtx {
                session: self.session.clone(),
                network: network.id.clone(),
            };
            let tab = div()
                .id(ElementId::Name(format!("network-{}", network.id).into()))
                .test_support()
                .role(Role::Tab)
                .aria_label(SharedString::from(label))
                .aria_selected(active)
                .tooltip(ui::tooltip::text(tooltip))
                .size(px(48.))
                .flex_none()
                .rounded(px(14.))
                .flex()
                .items_center()
                .justify_center()
                .when(active, |d| d.bg(theme.selected))
                .when(!active, |d| d.hover(|d| d.bg(theme.hover)))
                .cursor_pointer()
                .relative()
                .child(tile)
                .children(dot.map(|d| div().absolute().bottom(px(4.)).right(px(4.)).child(d)))
                .on_click(move |_, _, cx| session.update(cx, |s, cx| s.select_network(&id, cx)));
            rail = rail.child(tab.context_menu(move |menu, _, cx| {
                let items = registry::get(cx).network_menu.to_vec();
                let items = ui::context_menu::visible(&items, &ctx, cx);
                ui::context_menu::fill(menu, items, ctx.clone())
            }));
        }
        rail.into_any_element()
    }

    fn conversation_row(
        &self,
        w: &Workspace,
        c: &Conversation,
        section: Section,
        number: Option<usize>,
        active: bool,
        theme: &Theme,
    ) -> AnyElement {
        let unread = c.unread_count > 0 || c.unread_from.is_some();
        let muted = order::muted(w, c);
        let id = c.id.clone();
        let session = self.session.clone();
        let mut row = div()
            .id(ElementId::Name(format!("conversation-{}", c.id).into()))
            .test_support()
            .role(Role::Button)
            .aria_label(SharedString::from(c.name.clone()))
            .aria_selected(active)
            .group("conversation-row")
            .px_2()
            .py_1()
            .rounded_md()
            .flex()
            .items_center()
            .gap_1p5()
            .cursor_pointer()
            .text_color(if active || (unread && !muted) {
                theme.text
            } else {
                theme.muted
            })
            .when(active, |d| d.bg(theme.selected))
            .when(!active, |d| d.hover(|d| d.bg(theme.hover)))
            .when(unread && !muted, |d| d.font_weight(FontWeight::SEMIBOLD))
            .when(!c.joined || muted, |d| d.opacity(0.6))
            .child(div().text_color(theme.faint).child(prefix(c)))
            .child(div().flex_1().truncate().child(c.name.clone()))
            .on_click(move |_, _, cx| session.update(cx, |s, cx| s.select_conversation(&id, cx)));
        if let Some(n) = number {
            row = row.aria_description(SharedString::from(number_label(n)));
        }
        if c.mention_count > 0 {
            row = row.child(badge(c.mention_count, theme));
        } else if let Some(n) = number {
            row = row.child(
                div()
                    .text_xs()
                    .text_color(theme.faint)
                    .invisible()
                    .group_hover("conversation-row", |s| s.visible())
                    .child(number_label(n)),
            );
        }
        if section.sortable() {
            let dragged = DraggedConversation {
                id: c.id.clone(),
                network: c.network_id.clone(),
                section,
                name: SharedString::from(format!("{}{}", prefix(c), c.name)),
            };
            let accent = theme.accent;
            let target = c.id.clone();
            let session = self.session.clone();
            row = row
                .on_drag(dragged, |d, _, _, cx| {
                    let name = d.name.clone();
                    cx.new(|_| DragPreview(name))
                })
                .drag_over::<DraggedConversation>(move |style, d, _, _| {
                    if d.section == section {
                        style.border_t_2().border_color(accent)
                    } else {
                        style
                    }
                })
                .on_drop(move |d: &DraggedConversation, _, cx| {
                    if d.section != section || d.id == target {
                        return;
                    }
                    // Down the list lands after the target, up lands before it.
                    let place = {
                        let s = session.read(cx);
                        let Some(client) = s.client() else {
                            return;
                        };
                        let ids = order::ids(&client.read(), &d.network, section);
                        let from = ids.iter().position(|i| *i == d.id);
                        let to = ids.iter().position(|i| *i == target);
                        if from < to {
                            Place::After
                        } else {
                            Place::Before
                        }
                    };
                    menus::move_to(&session, &d.network, section, &d.id, &target, place, cx);
                });
        }
        let ctx = ConversationCtx {
            session: self.session.clone(),
            conversation: c.id.clone(),
        };
        row.context_menu(move |menu, _, cx| {
            let client = ctx.session.read(cx).client().cloned();
            layout::watch_notification_settings(client, cx);
            let items = registry::get(cx).conversation_menu.to_vec();
            let items = ui::context_menu::visible(&items, &ctx, cx);
            ui::context_menu::fill(menu, items, ctx.clone())
        })
        .into_any_element()
    }

    fn section_heading(
        &self,
        network: &str,
        view: &order::SectionView,
        theme: &Theme,
    ) -> AnyElement {
        let section = view.section;
        let label = match section {
            Section::Favorites => t!("sidebar-favourites"),
            Section::Channels => t!("sidebar-channels"),
            Section::Directs => t!("sidebar-direct-messages"),
        };
        let state = if view.collapsed {
            t!("sidebar-section-collapsed", section = label)
        } else {
            t!("sidebar-section-expanded", section = label)
        };
        let key = order::section_key(network, section);
        let toggle = div()
            .id(ElementId::Name(format!("section-{}", section.id()).into()))
            .test_support()
            .role(Role::Button)
            .aria_label(SharedString::from(state))
            .aria_expanded(!view.collapsed)
            .flex_1()
            .min_w_0()
            .flex()
            .items_center()
            .gap_1()
            .cursor_pointer()
            .hover({
                let text = theme.text;
                move |d| d.text_color(text)
            })
            .child(ui::icon::icon(
                if view.collapsed {
                    IconName::ChevronRight
                } else {
                    IconName::ChevronDown
                },
                theme.faint,
            ))
            .child(div().truncate().child(label.to_uppercase()))
            .on_click(move |_, _, cx| {
                layout::update(cx, |l| {
                    l.collapsed = order::toggle_collapsed(&l.collapsed, &key)
                })
            });
        let mut heading = div()
            .pt_3()
            .pb_1()
            .px_1()
            .flex()
            .items_center()
            .gap_1()
            .text_xs()
            .font_weight(FontWeight::BOLD)
            .text_color(theme.muted)
            .child(toggle);
        if view.custom {
            let (session, network) = (self.session.clone(), network.to_string());
            heading = heading.child(ui::icon::button(
                ElementId::Name(format!("section-{}-sort", section.id()).into()),
                IconName::SortAscending,
                t!("sidebar-sort-section", section = label),
                theme,
                move |_, _, cx| menus::sort_by_name(&session, &network, section, cx),
            ));
        }
        match section {
            Section::Channels => {
                heading = heading.child(ui::icon::button(
                    "section-channels-join",
                    IconName::Plus,
                    t!("sidebar-join-channel"),
                    theme,
                    |_, window, cx| {
                        window
                            .dispatch_action(Box::new(crate::features::shortcuts::JoinChannel), cx)
                    },
                ));
            }
            Section::Directs => {
                heading = heading.child(ui::icon::button(
                    "section-directs-new",
                    IconName::Plus,
                    t!("sidebar-message-someone"),
                    theme,
                    |_, window, cx| crate::features::switcher::open(window, cx),
                ));
            }
            Section::Favorites => {}
        }
        heading.into_any_element()
    }

    fn list(
        &self,
        w: &Workspace,
        network: &str,
        selected: Option<&str>,
        layout: &order::Layout,
        theme: &Theme,
    ) -> AnyElement {
        let mut list = div()
            .id("conversation-list")
            .test_support()
            .role(Role::List)
            .aria_label(t!("sidebar-conversations"))
            .flex_1()
            .flex()
            .flex_col()
            .gap_0p5()
            .px_2()
            .py_1()
            .overflow_y_scroll();
        let numbers = order::numbered(w, network);
        let filter = Filter {
            active: selected,
            unread_only: layout.unread_only,
        };
        for view in order::sections(w, network, &layout.collapsed, filter) {
            list = list.child(self.section_heading(network, &view, theme));
            for c in &view.shown {
                let number = numbers.iter().position(|id| *id == c.id).map(|i| i + 1);
                let active = selected == Some(c.id.as_str());
                list = list.child(self.conversation_row(w, c, view.section, number, active, theme));
            }
            if view.section == Section::Directs && view.items.is_empty() && !view.collapsed {
                list = list.child(
                    div()
                        .id("sidebar-no-directs")
                        .px_2()
                        .py_1()
                        .text_xs()
                        .text_color(theme.faint)
                        .child(t!("sidebar-no-directs")),
                );
            }
        }
        list.into_any_element()
    }

    /// The list folded to icons: one per conversation that would show.
    fn folded(
        &self,
        w: &Workspace,
        network: &str,
        selected: Option<&str>,
        layout: &order::Layout,
        theme: &Theme,
    ) -> Div {
        let mut column = div()
            .id("folded-list")
            .test_support()
            .role(Role::List)
            .aria_label(t!("sidebar-conversations"))
            .flex_1()
            .flex()
            .flex_col()
            .items_center()
            .gap_1()
            .py_2()
            .overflow_y_scroll();
        let filter = Filter {
            active: selected,
            unread_only: layout.unread_only,
        };
        for view in order::sections(w, network, &layout.collapsed, filter) {
            for c in &view.shown {
                let active = selected == Some(c.id.as_str());
                let id = c.id.clone();
                let session = self.session.clone();
                let initial: String = c.name.chars().take(2).collect();
                let unread = c.unread_count > 0 && !order::muted(w, c);
                column = column.child(
                    div()
                        .id(ElementId::Name(format!("folded-{}", c.id).into()))
                        .test_support()
                        .role(Role::Button)
                        .aria_label(SharedString::from(c.name.clone()))
                        .tooltip(ui::tooltip::text(format!("{}{}", prefix(c), c.name)))
                        .size(px(38.))
                        .flex_none()
                        .rounded_md()
                        .relative()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_sm()
                        .cursor_pointer()
                        .text_color(if active || unread {
                            theme.text
                        } else {
                            theme.muted
                        })
                        .when(active, |d| d.bg(theme.selected))
                        .when(!active, |d| d.bg(theme.main).hover(|d| d.bg(theme.hover)))
                        .when(unread, |d| d.font_weight(FontWeight::BOLD))
                        .child(format!("{}{}", prefix(c), initial))
                        .when(c.mention_count > 0, |d| {
                            d.child(
                                div()
                                    .absolute()
                                    .top(px(-4.))
                                    .right(px(-4.))
                                    .child(badge(c.mention_count, theme)),
                            )
                        })
                        .on_click(move |_, _, cx| {
                            session.update(cx, |s, cx| s.select_conversation(&id, cx))
                        }),
                );
            }
        }
        let restore = ui::icon::button(
            "sidebar-restore",
            IconName::PanelLeftOpen,
            t!("sidebar-show-list"),
            theme,
            |_, _, cx| layout::update(cx, |l| l.show_list = true),
        );
        div()
            .w(px(FOLDED))
            .h_full()
            .flex_none()
            .flex()
            .flex_col()
            .items_center()
            .bg(theme.sidebar)
            .child(div().h(px(48.)).flex().items_center().child(restore))
            .child(column)
    }
}

impl Render for SidebarView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // The workspace takes the keyboard when it appears (the sign-in
        // form's field may still hold it, though it is gone), and whenever
        // nothing else has it.
        if !self.focused_once || window.focused(cx).is_none() {
            self.focused_once = true;
            self.focus.focus(window, cx);
        }
        let theme = theme::get(cx).clone();
        let layout = layout::get(cx);
        let session = self.session.read(cx);
        let Some(client) = session.client().cloned() else {
            return div().into_any_element();
        };
        let selected_network = session.network.clone();
        let selected = session.conversation.clone();
        let (can_back, can_forward) = (
            session.history.can_go_back(),
            session.history.can_go_forward(),
        );
        let w = client.read();
        let rail = self.rail(&w, selected_network.as_deref(), &theme);
        let network = selected_network.as_deref().and_then(|id| w.network(id));

        if !layout.show_list {
            let folded = network
                .map(|n| self.folded(&w, &n.id, selected.as_deref(), &layout, &theme))
                .unwrap_or_else(|| div().w(px(FOLDED)).h_full().bg(theme.sidebar));
            return div()
                .id("sidebar")
                .track_focus(&self.focus)
                .h_full()
                .flex()
                .flex_row()
                .child(rail)
                .child(folded)
                .into_any_element();
        }

        let session_entity = self.session.clone();
        let nav = |id: &'static str, icon, label: &'static str, enabled: bool, back: bool| {
            let session = session_entity.clone();
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
        let mut header = div()
            .h(px(48.))
            .flex_none()
            .pl_3()
            .pr_1()
            .flex()
            .items_center()
            .gap_0p5()
            .border_b_1()
            .border_color(theme.rail);
        if let Some(n) = network {
            let ctx = NetworkCtx {
                session: self.session.clone(),
                network: n.id.clone(),
            };
            header = header.child(
                div()
                    .id("network-header")
                    .test_support()
                    .role(Role::Heading)
                    .aria_label(SharedString::from(n.name.clone()))
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .font_weight(FontWeight::BOLD)
                    .text_color(theme.text)
                    .child(n.name.clone())
                    .context_menu(move |menu, _, cx| {
                        let items = registry::get(cx).network_menu.to_vec();
                        let items = ui::context_menu::visible(&items, &ctx, cx);
                        ui::context_menu::fill(menu, items, ctx.clone())
                    }),
            );
        } else {
            header = header.child(div().flex_1());
        }
        header = header
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
            .child(ui::icon::button(
                "sidebar-hide",
                IconName::PanelLeftClose,
                t!("sidebar-hide-list"),
                &theme,
                |_, _, cx| layout::update(cx, |l| l.show_list = false),
            ));

        // Find a conversation, the unread filter and mark all read.
        let unread_here = network.map_or(0, |n| order::unread_in(&w, Some(&n.id)).len());
        let mut tools = div()
            .px_2()
            .pt_2()
            .flex()
            .items_center()
            .gap_1()
            .child(
                div()
                    .id("sidebar-find")
                    .test_support()
                    .role(Role::Button)
                    .aria_label(t!("sidebar-find"))
                    .flex_1()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .cursor_pointer()
                    .bg(theme.main)
                    .text_sm()
                    .text_color(theme.faint)
                    .hover(|d| d.bg(theme.hover))
                    .child(ui::icon::icon(IconName::Search, theme.faint))
                    .child(t!("sidebar-find"))
                    .on_click(|_, window, cx| crate::features::switcher::open(window, cx)),
            )
            .child(
                div()
                    .id("sidebar-unread-only")
                    .test_support()
                    .role(Role::CheckBox)
                    .aria_label(t!("sidebar-unread-only"))
                    .aria_toggled(if layout.unread_only {
                        Toggled::True
                    } else {
                        Toggled::False
                    })
                    .tooltip(ui::tooltip::text(t!("sidebar-unread-only")))
                    .p_1()
                    .rounded_md()
                    .cursor_pointer()
                    .when(layout.unread_only, |d| d.bg(theme.selected))
                    .hover(|d| d.bg(theme.hover))
                    .child(ui::icon::icon(
                        IconName::Inbox,
                        if layout.unread_only {
                            theme.text
                        } else {
                            theme.muted
                        },
                    ))
                    .on_click(|_, _, cx| layout::update(cx, |l| l.unread_only = !l.unread_only)),
            );
        if let Some(n) = network.filter(|_| unread_here > 0) {
            let (session, id) = (self.session.clone(), n.id.clone());
            tools = tools.child(ui::icon::button(
                "sidebar-mark-network-read",
                IconName::Check,
                t!("sidebar-mark-network-read", count = unread_here),
                &theme,
                move |_, _, cx| {
                    menus::mark_network_read(&session, &id, cx);
                },
            ));
        }

        let list = match network {
            Some(n) => self.list(&w, &n.id, selected.as_deref(), &layout, &theme),
            None => div().id("conversation-list").flex_1().into_any_element(),
        };

        let me = network
            .map(|n| n.nick.clone())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| w.user.nick.clone());
        let session = self.session.clone();
        let footer = div()
            .h(px(52.))
            .flex_none()
            .px_3()
            .flex()
            .items_center()
            .justify_between()
            .bg(theme.footer)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .min_w_0()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::BOLD)
                            .text_color(theme.text)
                            .truncate()
                            .child(me),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted)
                            .truncate()
                            .child(w.user.account.clone()),
                    ),
            )
            .child(
                div()
                    .id("sign-out")
                    .test_support()
                    .role(Role::Button)
                    .aria_label(t!("sidebar-sign-out"))
                    .text_xs()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .cursor_pointer()
                    .text_color(theme.muted)
                    .hover(|d| d.bg(theme.hover).text_color(theme.text))
                    .child(t!("sidebar-sign-out"))
                    .on_click(move |_, _, cx| session.update(cx, |s, cx| s.sign_out(cx))),
            );

        div()
            .id("sidebar")
            .track_focus(&self.focus)
            .h_full()
            .flex()
            .flex_row()
            .on_drag_move::<ResizeList>(|e, _, cx| {
                let width = f32::from(e.event.position.x - e.bounds.left()) - RAIL;
                layout::update(cx, |l| {
                    l.sidebar_width = order::clamp_width(width, SIDEBAR_WIDTH)
                });
            })
            .child(rail)
            .child(
                div()
                    .w(px(layout.sidebar_width))
                    .h_full()
                    .flex_none()
                    .flex()
                    .flex_col()
                    .bg(theme.sidebar)
                    .child(header)
                    .child(tools)
                    .child(list)
                    .child(footer),
            )
            .child(handle(
                "resize-sidebar",
                t!("sidebar-resize-list").into(),
                &theme,
                ResizeList,
            ))
            .into_any_element()
    }
}

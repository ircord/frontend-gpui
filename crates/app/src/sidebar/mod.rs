//! The sidebar and the top bar (packages N1 and V3).
//!
//! - The top bar ([`topbar`]) has a tab per network with its own icon, or
//!   a pattern drawn from its name, and your own button.
//! - The sidebar opens with Search, then the pages features register
//!   (`Registry::sidebar_footer`: Inbox, Saved, Drafts, Links), the
//!   network's server window, and the unread filter.
//! - The list has Favourites, Channels and Direct messages, each collapsible
//!   per network on this device. Favourites and channels can be dragged into
//!   your own order (or moved from the menu), which follows the account;
//!   Sort A to Z forgets it. The first nine channels show their Ctrl+number.
//! - The list folds to an icon rail and back, and is as wide as you drag it;
//!   so is the right-hand panel ([`side_panel`]).
//! - Right-clicking a network or a conversation opens its registered menu;
//!   N1's own entries are in [`menus`]. Slots marked `at_bottom` (the
//!   connection indicator) sit under the list.
//!
//! Redraws when the sidebar's data, the documents it reads, the selection,
//! the connection or the layout change.

pub mod glyph;
pub mod layout;
pub mod menus;
pub mod topbar;

use std::{cell::Cell, rc::Rc};

use crate::ui::focus::Reachable;
use gpui_kit::TestSupportExt;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::menu::{ContextMenuExt, DropdownMenu};
use gpui_kit::component::{Selectable, Sizable};
use gpui_kit::prelude::*;
use gpui_kit::*;
use ircord_client::contract::{Conversation, ConversationKind, NetworkStatus};
use ircord_client::store::inbox;
use ircord_client::store::sidebar_order::{
    self as order, Activity, Filter, PANEL_WIDTH, Place, SIDEBAR_WIDTH, Section,
};
use ircord_client::store::{Workspace, parties, people, topic};

use crate::registry::{self, ConversationCtx, NetworkCtx, SidebarCtx};
use crate::route::Route;
use crate::session::{Session, SessionEvent};
use crate::theme::{self, Theme};
use crate::ui;
use crate::ui::icon::IconName;

/// The folded list's width.
const FOLDED: f32 = 56.;

/// Set up the layout state and register the menus. Called from
/// `features::shortcuts::init`.
pub fn init(cx: &mut App) {
    layout::init(cx);
    menus::register(cx);
    topbar::init(cx);
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

#[derive(Clone, Copy, PartialEq, Eq)]
enum ResizeEdge {
    Sidebar,
    Panel,
}

impl ResizeEdge {
    fn limits(self) -> (f32, f32, f32) {
        match self {
            Self::Sidebar => SIDEBAR_WIDTH,
            Self::Panel => PANEL_WIDTH,
        }
    }

    fn direction(self) -> f32 {
        match self {
            Self::Sidebar => 1.,
            Self::Panel => -1.,
        }
    }

    fn set_width(self, width: f32, cx: &mut App) {
        layout::update(cx, |layout| match self {
            Self::Sidebar => layout.sidebar_width = width,
            Self::Panel => layout.panel_width = width,
        });
    }
}

#[derive(Clone)]
struct ResizeDrag {
    edge: ResizeEdge,
    start: Rc<Cell<(Pixels, f32)>>,
}

struct Nothing;
impl Render for Nothing {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

/// The hitbox fills the gutter; only the center line lights up.
fn resize_handle(
    edge: ResizeEdge,
    width: f32,
    window: &mut Window,
    cx: &mut App,
) -> impl IntoElement {
    let (id, label, hit_width) = match edge {
        ResizeEdge::Sidebar => ("resize-sidebar", t!("sidebar-resize-list"), px(8.)),
        ResizeEdge::Panel => ("resize-panel", t!("sidebar-resize-panel"), px(6.)),
    };
    let theme = theme::get(cx).clone();
    let scale = theme.scale;
    let (min, max, default) = edge.limits();
    let start = window
        .use_keyed_state((id, 0_usize), cx, |_, _| {
            Rc::new(Cell::new((px(0.), width)))
        })
        .read(cx)
        .clone();
    let drag = ResizeDrag {
        edge,
        start: start.clone(),
    };
    div()
        .id(id)
        .test_support()
        .role(Role::Splitter)
        .aria_label(label)
        .aria_numeric_value(width.into())
        .aria_min_numeric_value(min.into())
        .aria_max_numeric_value(max.into())
        .aria_orientation(accesskit::Orientation::Vertical)
        .reachable(&theme)
        .group(id)
        .w(hit_width)
        .h_full()
        .flex_none()
        .flex()
        .justify_center()
        .cursor_col_resize()
        .child(
            div()
                .w(px(2.))
                .h_full()
                .bg(theme.accent.opacity(0.7))
                .opacity(0.)
                .group_hover(id, |line| line.opacity(1.))
                .group_drag_over::<ResizeDrag>(id, |line| line.opacity(1.)),
        )
        .on_mouse_down(MouseButton::Left, move |event, _, _| {
            start.set((event.position.x, width))
        })
        .on_drag(drag, |_, _, _, cx| cx.new(|_| Nothing))
        .on_drag_move::<ResizeDrag>(move |event, _, cx| {
            let drag = event.drag(cx);
            if drag.edge == edge {
                let (origin, width) = drag.start.get();
                let delta = f32::from(event.event.position.x - origin) / scale;
                edge.set_width(width + delta * edge.direction(), cx);
            }
        })
        .on_click(move |event, _, cx| {
            if event.click_count() == 2 {
                edge.set_width(default, cx);
            }
        })
        .on_key_down(move |event, _, cx| {
            let width = match event.keystroke.key.as_str() {
                "left" => width - 16. * edge.direction(),
                "right" => width + 16. * edge.direction(),
                "home" => min,
                "end" => max,
                _ => return,
            };
            cx.stop_propagation();
            edge.set_width(width, cx);
        })
}

/// The right-hand panel (the member list or a feature's panel) in its frame
/// beside the main pane, as wide as the person dragged it.
pub fn side_panel(
    panel: AnyElement,
    fixed_width: Option<f32>,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let theme = theme::get(cx).clone();
    let width = fixed_width.unwrap_or_else(|| layout::get(cx).panel_width);
    div()
        .id("side-panel")
        .w(px(width * theme.scale))
        .h_full()
        .flex_none()
        .flex()
        .flex_row()
        .border_l_1()
        .border_color(theme.border.opacity(0.45))
        .when(fixed_width.is_none(), |d| {
            d.child(resize_handle(ResizeEdge::Panel, width, window, cx))
        })
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
                if changes.sidebar
                    || changes.reset
                    || changes.touches(topic::DOCUMENTS)
                    || changes.touches(inbox::TOPIC) =>
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

/// A page in the block at the top of the sidebar (Inbox, Saved, Server):
/// its icon, its name, and a count at the right when there is one.
/// `accessible` is the name a screen reader says (with the count).
#[allow(clippy::too_many_arguments)]
pub fn page_row(
    id: impl Into<ElementId>,
    icon: IconName,
    label: impl Into<SharedString>,
    accessible: impl Into<SharedString>,
    count: Option<String>,
    active: bool,
    theme: &Theme,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let hover = theme.hover;
    div()
        .id(id.into())
        .test_support()
        .role(Role::Button)
        .reachable(theme)
        .aria_label(accessible.into())
        .aria_selected(active)
        .mx_2()
        .px_2()
        .py_1()
        .rounded_md()
        .flex()
        .items_center()
        .gap_2()
        .text_sm()
        .cursor_pointer()
        .text_color(if active { theme.text } else { theme.muted })
        .when(active, |d| d.bg(theme.selected))
        .hover(move |d| d.bg(hover))
        .child(ui::icon::icon(icon, theme.muted))
        .child(div().flex_1().truncate().child(label.into()))
        .when_some(count, |d, count| {
            d.child(
                div()
                    .px_1p5()
                    .rounded_full()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.accent)
                    .child(count),
            )
        })
        .on_click(on_click)
        .into_any_element()
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
    if c.party.is_some() {
        ""
    } else if c.kind == ConversationKind::Channel {
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
    /// One conversation's row. A direct conversation shows the other
    /// person as the web does: their picture with their presence, and their
    /// name rather than their nick.
    #[allow(clippy::too_many_arguments)]
    fn conversation_row(
        &self,
        w: &Workspace,
        c: &Conversation,
        section: Section,
        number: Option<usize>,
        active: bool,
        server: Option<&url::Url>,
        theme: &Theme,
    ) -> AnyElement {
        let unread = c.unread_count > 0 || c.unread_from.is_some();
        let muted = order::muted(w, c);
        let direct = c.kind == ConversationKind::Direct && c.party.is_none();
        let partner = direct.then(|| people::direct_partner(w, &c.id)).flatten();
        let name = match partner {
            Some(m) => people::name(m).to_string(),
            None => parties::title(w, c),
        };
        let size = px(ui::avatar::TINY);
        let marker = match partner {
            Some(m) => ui::avatar::person(
                ui::avatar::Person {
                    member: m,
                    name: &name,
                    server,
                    presence: true,
                    key: &format!("conversation-{}", c.id),
                },
                size,
                theme,
            )
            .into_any_element(),
            None if direct => ui::avatar::avatar(&name, None, size, theme).into_any_element(),
            None => div()
                // The prefix, with the channel's Ctrl+number raised beside it.
                .relative()
                .w(px(14.))
                .flex_none()
                .text_color(theme.faint)
                .child(prefix(c))
                .when_some(number, |d, n| {
                    d.child(
                        div()
                            .absolute()
                            .top(px(-5.))
                            .left(px(9.))
                            .text_size(px(9.))
                            .font_weight(FontWeight::NORMAL)
                            .child(n.to_string()),
                    )
                })
                .into_any_element(),
        };
        let id = c.id.clone();
        let session = self.session.clone();
        let mut row = div()
            .id(ElementId::Name(format!("conversation-{}", c.id).into()))
            .test_support()
            .role(Role::Button)
            .reachable(theme)
            .aria_label(SharedString::from(name.clone()))
            .aria_selected(active)
            .group("conversation-row")
            .px_2()
            .py(theme.row_padding)
            .text_sm()
            .rounded(theme.radius)
            .flex()
            .items_center()
            .gap_1p5()
            .cursor_pointer()
            .text_color(if active || (unread && !muted) {
                theme.text
            } else {
                theme.muted
            })
            .border_1()
            .when(active, |d| {
                let border = if theme.hibiki { 0.58 } else { 0.26 };
                let d = d
                    .bg(theme.selected)
                    .border_color(theme.accent.opacity(border))
                    .font_weight(FontWeight::SEMIBOLD);
                d.when(theme.hibiki, |d| {
                    d.bg(linear_gradient(
                        95.,
                        linear_color_stop(theme.sidebar.blend(theme.accent.opacity(0.52)), 0.),
                        linear_color_stop(theme.sidebar.blend(theme.accent.opacity(0.24)), 1.),
                    ))
                    .shadow(vec![BoxShadow {
                        color: theme.accent.opacity(0.20),
                        offset: point(px(0.), px(4.)),
                        blur_radius: px(14.),
                        spread_radius: px(0.),
                        inset: false,
                    }])
                })
            })
            .when(!active, |d| {
                d.border_color(gpui_kit::transparent_black())
                    .hover(|d| d.bg(theme.hover))
            })
            .when(unread && !muted, |d| d.font_weight(FontWeight::SEMIBOLD))
            .when(!c.joined || muted, |d| d.opacity(0.6))
            .child(marker)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(div().truncate().child(name.clone()))
                    .children(
                        partner
                            .and_then(|member| {
                                people::status_label(member, chrono::Utc::now().timestamp_millis())
                            })
                            .map(|status| {
                                div()
                                    .truncate()
                                    .text_xs()
                                    .text_color(theme.muted)
                                    .child(status)
                            }),
                    ),
            )
            .on_click(move |_, _, cx| session.update(cx, |s, cx| s.select_conversation(&id, cx)));
        if let Some(n) = number {
            row = row.aria_description(SharedString::from(number_label(n)));
        }
        if c.mention_count > 0 {
            row = row.child(badge(c.mention_count, theme));
        } else if unread && !muted && !active {
            row = row.child(
                div()
                    .id(ElementId::Name(
                        format!("conversation-{}-unread", c.id).into(),
                    ))
                    .test_support()
                    .size(px(6.))
                    .flex_none()
                    .rounded_full()
                    .bg(theme.text),
            );
        }
        if section.sortable() {
            let dragged = DraggedConversation {
                id: c.id.clone(),
                network: c.network_id.clone(),
                section,
                name: SharedString::from(format!("{}{}", prefix(c), name)),
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
        row.context_menu(move |menu, window, cx| {
            let client = ctx.session.read(cx).client().cloned();
            layout::watch_notification_settings(client, cx);
            let items = registry::get(cx).conversation_menu.to_vec();
            let items = ui::context_menu::visible(&items, &ctx, cx);
            ui::context_menu::fill(menu, items, ctx.clone(), window, cx)
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
            .reachable(theme)
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
            .child(div().truncate().child(label))
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
            .text_sm()
            .font_weight(FontWeight::MEDIUM)
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
        server: Option<&url::Url>,
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
                list = list.child(self.conversation_row(
                    w,
                    c,
                    view.section,
                    number,
                    active,
                    server,
                    theme,
                ));
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

    fn folded_tools(
        &self,
        network: &str,
        route: Option<&Route>,
        unread: i64,
        theme: &Theme,
    ) -> impl IntoElement + use<> {
        use ui::icon::LucideIcon as Icon;
        let tools = [
            (
                "folded-search",
                Icon::Search,
                t!("sidebar-find"),
                Route::Search,
            ),
            (
                "folded-inbox",
                Icon::Inbox,
                t!("inbox-saved-inbox"),
                Route::Inbox,
            ),
            (
                "folded-saved",
                Icon::Bookmark,
                t!("inbox-saved-saved"),
                Route::Saved,
            ),
            (
                "folded-drafts",
                Icon::Pencil,
                t!("drafts-title"),
                Route::Drafts,
            ),
            (
                "folded-links",
                Icon::Link,
                t!("links-view-title"),
                Route::Links(None),
            ),
            (
                "folded-server",
                Icon::SquareTerminal,
                t!("sidebar-server"),
                Route::ServerWindow(network.to_string()),
            ),
        ];
        div()
            .id("folded-pages")
            .test_support()
            .role(Role::Group)
            .aria_label(t!("sidebar-pages"))
            .flex()
            .flex_col()
            .items_center()
            .gap_1()
            .pb_2()
            .children(tools.map(|(id, icon, title, destination)| {
                let active = match (&destination, route) {
                    (Route::Links(_), Some(Route::Links(_))) => true,
                    _ => route == Some(&destination),
                };
                let fresh_inbox = destination == Route::Inbox && unread > 0;
                let label: SharedString = if fresh_inbox {
                    t!("inbox-saved-inbox-unread", count = unread).into()
                } else {
                    title.into()
                };
                let session = self.session.clone();
                let hover = theme.hover;
                div()
                    .id(id)
                    .test_support()
                    .role(Role::Button)
                    .reachable(theme)
                    .aria_label(label.clone())
                    .aria_selected(active)
                    .relative()
                    .size(px(32.))
                    .rounded(theme.radius)
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .child(ui::icon::icon(
                        icon,
                        if active { theme.text } else { theme.muted },
                    ))
                    .when(active, |b| b.bg(theme.selected))
                    .when(!active, |b| b.hover(move |b| b.bg(hover)))
                    .tooltip(ui::tooltip::text(label))
                    .when(fresh_inbox, |b| {
                        b.child(
                            div()
                                .absolute()
                                .top_1()
                                .right_1()
                                .size(px(6.))
                                .rounded_full()
                                .bg(theme.accent),
                        )
                    })
                    .on_click(move |_, _, cx| match &destination {
                        Route::Search => crate::features::search::open(&session, None, cx),
                        Route::ServerWindow(id) => {
                            crate::features::server_window::open(&session, id, cx)
                        }
                        _ => session.update(cx, |s, cx| s.navigate(destination.clone(), cx)),
                    })
            }))
    }

    fn folded_conversation(
        &self,
        w: &Workspace,
        c: &Conversation,
        number: Option<usize>,
        active: bool,
        server: Option<&url::Url>,
        theme: &Theme,
    ) -> AnyElement {
        let id = format!("folded-{}", c.id);
        let partner = (number.is_none() && c.party.is_none())
            .then(|| people::direct_partner(w, &c.id))
            .flatten();
        let name = partner.map_or_else(|| parties::title(w, c), |p| people::name(p).to_string());
        let size = px(ui::avatar::TINY);
        let body = match (number, partner) {
            (Some(number), _) => div().child(number.to_string()).into_any_element(),
            (_, Some(member)) => ui::avatar::person(
                ui::avatar::Person {
                    member,
                    name: &name,
                    server,
                    presence: true,
                    key: &id,
                },
                size,
                theme,
            )
            .into_any_element(),
            _ if c.party.is_some() => {
                ui::icon::icon(ui::icon::LucideIcon::Users, theme.muted).into_any_element()
            }
            _ => ui::avatar::avatar(&name, None, size, theme).into_any_element(),
        };
        let label = if number.is_some() {
            format!("{}{}", prefix(c), name)
        } else if c.party.is_some() {
            name
        } else {
            t!("sidebar-folded-direct", name = name)
        };
        let mention = c.mention_count > 0;
        let unread = c.unread_count > 0 && !order::muted(w, c);
        let ctx = ConversationCtx {
            session: self.session.clone(),
            conversation: c.id.clone(),
        };
        let target = ctx.clone();
        let mut details = Vec::new();
        if let Some(number) = number.filter(|n| *n <= order::NUMBERED) {
            details.push(number_label(number));
        }
        if c.mention_count == 1 {
            details.push(t!("sidebar-folded-one-mention").into());
        } else if mention {
            details.push(t!("sidebar-folded-mentions", count = c.mention_count));
        } else if unread {
            details.push(t!("sidebar-folded-unread").into());
        }
        div()
            .id(ElementId::Name(id.into()))
            .test_support()
            .role(Role::Button)
            .aria_label(SharedString::from(label.clone()))
            .aria_selected(active)
            .when(!details.is_empty(), |d| {
                d.aria_description(details.join(", "))
            })
            .reachable(theme)
            .tooltip(ui::tooltip::text(label))
            .size(px(32.))
            .flex_none()
            .rounded(theme.radius)
            .relative()
            .flex()
            .items_center()
            .justify_center()
            .text_sm()
            .cursor_pointer()
            .text_color(if active { theme.accent } else { theme.muted })
            .border_1()
            .border_color(if active {
                theme.accent.opacity(0.35)
            } else {
                transparent_black()
            })
            .when(active, |d| d.bg(theme.selected))
            .when(!active, |d| d.hover(|d| d.bg(theme.hover)))
            .child(body)
            .when(mention || unread, |d| {
                d.child(
                    div()
                        .absolute()
                        .top(px(2.))
                        .right(px(2.))
                        .size(px(6.))
                        .rounded_full()
                        .bg(if mention { theme.danger } else { theme.text }),
                )
            })
            .on_click(move |_, _, cx| {
                target
                    .session
                    .update(cx, |s, cx| s.select_conversation(&target.conversation, cx));
            })
            .context_menu(move |menu, window, cx| {
                let client = ctx.session.read(cx).client().cloned();
                layout::watch_notification_settings(client, cx);
                let items = registry::get(cx).conversation_menu.to_vec();
                let items = ui::context_menu::visible(&items, &ctx, cx);
                ui::context_menu::fill(menu, items, ctx.clone(), window, cx)
            })
            .into_any_element()
    }

    /// The folded tools above numbered channels and direct conversations.
    fn folded(
        &self,
        w: &Workspace,
        network: &str,
        server: Option<&url::Url>,
        theme: &Theme,
        route: Option<&Route>,
    ) -> Div {
        let selected = route.and_then(Route::conversation);
        let (channels, directs): (Vec<_>, Vec<_>) = order::ordered(w, network)
            .into_iter()
            .partition(|c| order::channel_wide(c));
        let active = |conversation: &Conversation| selected == Some(conversation.id.as_str());
        let column = div()
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
            .overflow_y_scroll()
            .child(self.folded_tools(network, route, w.inbox.unread.unwrap_or(0), theme))
            .children(channels.into_iter().enumerate().map(|(i, c)| {
                self.folded_conversation(w, c, Some(i + 1), active(c), server, theme)
            }))
            .when(!directs.is_empty(), |d| {
                d.child(
                    div()
                        .id("folded-directs")
                        .role(Role::Group)
                        .aria_label(t!("sidebar-direct-messages"))
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap_1()
                        .mt_1()
                        .pt_2()
                        .border_t_1()
                        .border_color(theme.border.opacity(0.5))
                        .children(directs.into_iter().map(|c| {
                            self.folded_conversation(w, c, None, active(c), server, theme)
                        })),
                )
            });
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
            .when(theme.app_background().is_none(), |d| d.bg(theme.sidebar))
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
            return div().w(px(8.)).into_any_element();
        };
        let selected_network = session.network.clone();
        let selected = session.conversation.clone();
        let route = session.route();
        let pictures_from = session.server().map(|s| s.url().clone());
        // Slots draw first: they may read the workspace too.
        let slot_ctx = SidebarCtx {
            session: self.session.clone(),
            network: selected_network.clone(),
        };
        let (mut pages, mut bottom) = (Vec::new(), Vec::new());
        for slot in registry::get(cx).sidebar_footer.to_vec() {
            if let Some(element) = (slot.render)(&slot_ctx, window, cx) {
                if slot.bottom {
                    bottom.push(element);
                } else {
                    pages.push(element);
                }
            }
        }
        let w = client.read();
        let network = selected_network.as_deref().and_then(|id| w.network(id));

        if !layout.show_list {
            let folded = network
                .map(|n| self.folded(&w, &n.id, pictures_from.as_ref(), &theme, route.as_ref()))
                .unwrap_or_else(|| div().w(px(FOLDED)).h_full().bg(theme.sidebar));
            return div()
                .id("sidebar")
                .track_focus(&self.focus)
                .h_full()
                .flex()
                .flex_row()
                .pr_2()
                .child(folded)
                .into_any_element();
        }

        // Search and folding the list.
        let searching = route == Some(crate::route::Route::Search);
        let session = self.session.clone();
        let search = div()
            .px_2()
            .pt_1()
            .pb_1()
            .flex()
            .items_center()
            .gap_1()
            .child(
                div()
                    .id("sidebar-find")
                    .test_support()
                    .role(Role::Button)
                    .reachable(&theme)
                    .aria_label(t!("sidebar-find"))
                    .aria_selected(searching)
                    .flex_1()
                    .px_2()
                    .py_1()
                    .rounded_full()
                    .flex()
                    .items_center()
                    .gap_2()
                    .cursor_pointer()
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.main)
                    .text_sm()
                    .text_color(theme.muted)
                    .when(searching, |d| {
                        d.bg(theme.selected)
                            .border_color(theme.accent.opacity(0.58))
                            .text_color(theme.text)
                            .font_weight(FontWeight::SEMIBOLD)
                    })
                    .when(!searching, |d| d.hover(|d| d.bg(theme.hover)))
                    .child(ui::icon::icon(IconName::Search, theme.muted))
                    .child(t!("sidebar-find"))
                    .on_click(move |_, _, cx| crate::features::search::open(&session, None, cx)),
            )
            .child(ui::icon::button(
                "sidebar-hide",
                IconName::PanelLeftClose,
                t!("sidebar-hide-list"),
                &theme,
                |_, _, cx| layout::update(cx, |l| l.show_list = false),
            ));

        // The network's server window, with the network menu beside it.
        let server = network.map(|n| {
            let active = route == Some(crate::route::Route::ServerWindow(n.id.clone()));
            let (session, id) = (self.session.clone(), n.id.clone());
            let ctx = NetworkCtx {
                session: self.session.clone(),
                network: n.id.clone(),
            };
            div()
                .relative()
                .child(page_row(
                    "sidebar-server",
                    IconName::SquareTerminal,
                    t!("sidebar-server"),
                    t!("sidebar-server"),
                    None,
                    active,
                    &theme,
                    move |_, _, cx| crate::features::server_window::open(&session, &id, cx),
                ))
                .child(
                    div().absolute().right(px(12.)).top(px(2.)).child(
                        Button::new("sidebar-network-menu")
                            .ghost()
                            .xsmall()
                            .icon(IconName::Ellipsis)
                            .accessibility_label(t!("sidebar-network-menu", network = n.name))
                            .tooltip(t!("sidebar-network-menu", network = n.name))
                            .dropdown_menu_with_anchor(
                                Anchor::TopRight,
                                move |menu, window, cx| {
                                    let items = registry::get(cx).network_menu.to_vec();
                                    let items = ui::context_menu::visible(&items, &ctx, cx);
                                    ui::context_menu::fill(menu, items, ctx.clone(), window, cx)
                                },
                            ),
                    ),
                )
        });

        // Unread only, and mark the network read.
        let unread_here = network.map_or(0, |n| order::unread_in(&w, Some(&n.id)).len());
        let mut filters = div()
            .px_2()
            .pt_2()
            .flex()
            .flex_col()
            .items_start()
            .gap_1()
            .child(
                div()
                    .id("sidebar-unread-only")
                    .test_support()
                    .role(Role::CheckBox)
                    .reachable(&theme)
                    .aria_label(t!("sidebar-unread-only"))
                    .aria_toggled(if layout.unread_only {
                        Toggled::True
                    } else {
                        Toggled::False
                    })
                    .px_2()
                    .py_0p5()
                    .rounded_full()
                    .flex()
                    .items_center()
                    .gap_1()
                    .text_xs()
                    .cursor_pointer()
                    .border_1()
                    .border_color(if layout.unread_only {
                        theme.accent
                    } else {
                        theme.border
                    })
                    .text_color(if layout.unread_only {
                        theme.text
                    } else {
                        theme.muted
                    })
                    .when(layout.unread_only, |d| d.bg(theme.selected))
                    .hover(|d| d.bg(theme.hover))
                    .child(ui::icon::icon(IconName::Bell, theme.muted))
                    .child(t!("sidebar-unread-only-chip"))
                    .on_click(|_, _, cx| layout::update(cx, |l| l.unread_only = !l.unread_only)),
            );
        if let Some(n) = network.filter(|_| unread_here > 0) {
            let (session, id) = (self.session.clone(), n.id.clone());
            filters = filters.child(
                div()
                    .id("sidebar-mark-network-read")
                    .test_support()
                    .role(Role::Button)
                    .reachable(&theme)
                    .aria_label(t!("sidebar-mark-network-read", count = unread_here))
                    .px_2()
                    .rounded_md()
                    .text_xs()
                    .cursor_pointer()
                    .text_color(theme.text)
                    .hover(|d| d.bg(theme.hover))
                    .child(t!("sidebar-mark-all-read"))
                    .on_click(move |_, _, cx| {
                        menus::mark_network_read(&session, &id, cx);
                    }),
            );
        }

        let list = match network {
            Some(n) => self.list(
                &w,
                &n.id,
                selected.as_deref(),
                &layout,
                pictures_from.as_ref(),
                &theme,
            ),
            None => div().id("conversation-list").flex_1().into_any_element(),
        };
        drop(w);

        div()
            .id("sidebar")
            .track_focus(&self.focus)
            .h_full()
            .flex()
            .flex_row()
            .child(
                div()
                    .w(px(layout.sidebar_width * theme.scale))
                    .h_full()
                    .flex_none()
                    .flex()
                    .flex_col()
                    .when(
                        theme.app_background().is_none() || window.viewport_size().width < px(900.),
                        |d| d.bg(theme.sidebar),
                    )
                    .child(search)
                    .child(
                        div()
                            .id("sidebar-pages")
                            .role(Role::Group)
                            .aria_label(t!("sidebar-pages"))
                            .flex()
                            .flex_col()
                            .gap_0p5()
                            .children(pages)
                            .children(server),
                    )
                    .child(filters)
                    .child(list)
                    .children(bottom),
            )
            .child(resize_handle(
                ResizeEdge::Sidebar,
                layout.sidebar_width,
                window,
                cx,
            ))
            .into_any_element()
    }
}

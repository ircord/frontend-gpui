//! The member list (package P1): grouped by highest channel prefix, then
//! online, away and offline, each heading with its count, as the web
//! client groups it. A filter field shows above long lists. Each row shows
//! the person's status under their name, a bot or phone marker, and is
//! struck through when you ignore them. A click opens their profile; a
//! right click opens the registered member menu. The list can be hidden
//! (see [`hidden`]). A list of [`VIRTUAL_FROM`] lines or more is drawn as a
//! virtual list, so only the rows on screen are built (package P2). The
//! grouping is kept until the roster, the selection or the filter changes.

use std::rc::Rc;

use gpui_kit::TestSupportExt;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::menu::ContextMenuExt;
use gpui_kit::prelude::*;
use gpui_kit::*;
use ircord_client::Client;
use ircord_client::contract::event::Document;
use ircord_client::contract::{MemberDevice, MemberRole, MemberStatus};
use ircord_client::store::Workspace;
use ircord_client::store::documents::document_id;
use ircord_client::store::people::{self, GroupKey};
use ircord_client::store::topic;
use url::Url;

use crate::registry::{self, MemberCtx};
use crate::session::{NO_PANEL, Session, SessionEvent};
use crate::theme::{self, Theme};
use crate::ui;
use crate::ui::icon::IconName;

/// Lists longer than this get a filter field, as on the web.
pub const FILTER_FROM: usize = 12;

/// Lists with this many lines or more are drawn as a virtual list.
pub const VIRTUAL_FROM: usize = 60;

/// One line of the list: a group's heading with its count, or a member with
/// the highest prefix shown before the name.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Line {
    Heading(GroupKey, usize),
    Member(String, String),
}

/// The lines for one conversation and filter, kept until the roster changes.
struct Listing {
    conversation: String,
    query: String,
    lines: Rc<Vec<Line>>,
}

/// What every line reads, read once per draw.
struct Frame {
    session: Entity<Session>,
    client: Rc<Client>,
    conversation: String,
    network: String,
    theme: Theme,
    colored_names: bool,
    server: Option<Url>,
    now: i64,
    ignore: Vec<String>,
}

pub struct MembersView {
    session: Entity<Session>,
    filter: Option<Entity<InputState>>,
    /// The conversation the filter was typed in; switching clears it.
    filtered_in: Option<String>,
    /// The lines of the list, until the roster, the selection or the filter
    /// changes.
    listing: Option<Listing>,
    /// The list's state when it is drawn as a virtual list.
    lines: ListState,
    _subscriptions: Vec<Subscription>,
}

impl MembersView {
    pub fn new(session: Entity<Session>, cx: &mut Context<Self>) -> Self {
        let subscription =
            cx.subscribe(&session, |this, _, event: &SessionEvent, cx| match event {
                SessionEvent::Changed(changes)
                    if changes.members
                        || changes.reset
                        || changes.sidebar
                        || changes.touches(people::TOPIC)
                        || changes.touches_id(
                            topic::DOCUMENTS,
                            document_id(Document::NotificationSettings),
                        ) =>
                {
                    this.listing = None;
                    cx.notify()
                }
                SessionEvent::Selection | SessionEvent::Phase => {
                    this.listing = None;
                    cx.notify()
                }
                _ => {}
            });
        Self {
            session,
            filter: None,
            filtered_in: None,
            listing: None,
            lines: ListState::new(0, ListAlignment::Top, px(400.)),
            _subscriptions: vec![subscription],
        }
    }

    /// The filter field, made on the first draw (it needs the window).
    fn filter(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<InputState> {
        if let Some(filter) = &self.filter {
            return filter.clone();
        }
        let filter =
            cx.new(|cx| InputState::new(window, cx).placeholder(t!("people-members-filter")));
        self._subscriptions
            .push(cx.subscribe(&filter, |_, _, event: &InputEvent, cx| {
                if let InputEvent::Change = event {
                    cx.notify();
                }
            }));
        self.filter = Some(filter.clone());
        filter
    }

    /// The lines for `conversation` under `query`: reused while the roster
    /// and the filter stay the same.
    fn lines(&mut self, w: &Workspace, conversation: &str, query: &str) -> Rc<Vec<Line>> {
        if let Some(listing) = &self.listing
            && listing.conversation == conversation
            && listing.query == query
        {
            return listing.lines.clone();
        }
        let mut lines = Vec::new();
        for group in people::member_groups(w, conversation, query) {
            lines.push(Line::Heading(group.key, group.members.len()));
            lines.extend(
                group
                    .members
                    .into_iter()
                    .map(|(id, prefix)| Line::Member(id, prefix)),
            );
        }
        let lines = Rc::new(lines);
        self.lines.splice(0..self.lines.item_count(), lines.len());
        self.listing = Some(Listing {
            conversation: conversation.to_string(),
            query: query.to_string(),
            lines: lines.clone(),
        });
        lines
    }
}

/// Whether the member list is hidden.
pub fn hidden(session: &Session) -> bool {
    session.panel.as_deref() == Some(NO_PANEL)
}

/// Hide the member list, or show it again.
pub fn set_hidden(session: &Entity<Session>, hide: bool, cx: &mut App) {
    session.update(cx, |s, cx| {
        let panel = hide.then(|| SharedString::from(NO_PANEL));
        if hidden(s) || hide {
            s.set_panel(panel, cx);
        }
    });
    crate::features::profile::remember_members_hidden(hide, cx);
}

impl Render for MembersView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme::get(cx).clone();
        let filter = self.filter(window, cx);
        let colored_names = crate::message::layout::settings(cx).colored_names;
        let server = self.session.read(cx).server().map(|s| s.url().clone());
        let session = self.session.read(cx);
        let (Some(client), Some(id)) = (session.client().cloned(), session.conversation.clone())
        else {
            return div().into_any_element();
        };
        if self.filtered_in.as_ref() != Some(&id) {
            self.filtered_in = Some(id.clone());
            filter.update(cx, |s, cx| s.set_value(String::new(), window, cx));
        }
        let query = filter.read(cx).value().to_string();
        let now = chrono::Utc::now().timestamp_millis();
        let (total, online, lines, frame) = {
            let w = client.read();
            let Some(conversation) = w.conversation(&id) else {
                return div().into_any_element();
            };
            let total = conversation.member_ids.len();
            let lines = self.lines(&w, &id, &query);
            let frame = Rc::new(Frame {
                session: self.session.clone(),
                client: client.clone(),
                conversation: id.clone(),
                network: conversation.network_id.clone(),
                theme: theme.clone(),
                colored_names,
                server,
                now,
                ignore: people::ignore_list(&w).unwrap_or_default(),
            });
            (total, people::online_count(&w, &id), lines, frame)
        };

        let session_entity = self.session.clone();
        let header = div()
            .flex()
            .items_center()
            .gap_1()
            .px_2()
            .pb_1()
            .child(
                div()
                    .id("member-count")
                    .test_support()
                    .role(Role::Status)
                    .aria_label(t!("people-members-count", online = online, total = total))
                    .flex_1()
                    .text_xs()
                    .text_color(theme.muted)
                    .child(t!("people-members-count", online = online, total = total)),
            )
            .child(ui::icon::button(
                "hide-members",
                IconName::PanelRightClose,
                t!("people-members-hide"),
                &theme,
                move |_, _, cx| set_hidden(&session_entity, true, cx),
            ));

        let virtual_list = lines.len() >= VIRTUAL_FROM;
        let mut panel = div()
            .id("member-list")
            .test_support()
            .role(Role::List)
            .aria_label(t!("people-members"))
            .size_full()
            .bg(theme.sidebar)
            .flex()
            .flex_col()
            .px_2()
            .py_3()
            .gap_0p5()
            .when(!virtual_list, |d| d.overflow_y_scroll())
            .child(header);
        if total > FILTER_FROM {
            panel = panel.child(
                div().px_1().pb_1().child(
                    Input::new(&filter)
                        .id("member-filter")
                        .aria_label(t!("people-members-filter")),
                ),
            );
        }
        if lines.is_empty() && !query.trim().is_empty() {
            panel = panel.child(
                div()
                    .id("member-no-matches")
                    .test_support()
                    .role(Role::Status)
                    .aria_label(t!("people-members-no-matches", filter = query.trim()))
                    .px_2()
                    .py_1()
                    .text_xs()
                    .text_color(theme.faint)
                    .child(t!("people-members-no-matches", filter = query.trim())),
            );
        }
        if virtual_list {
            let rows = lines.clone();
            let frame = frame.clone();
            panel = panel.child(
                list(self.lines.clone(), move |ix, _window, _cx| {
                    member_line(&frame, &rows[ix])
                })
                .flex_1()
                .min_h_0()
                .size_full(),
            );
        } else {
            for line in lines.iter() {
                panel = panel.child(member_line(&frame, line));
            }
        }
        panel.into_any_element()
    }
}

/// One line of the member list: a group's heading, or a member's row.
fn member_line(frame: &Frame, line: &Line) -> AnyElement {
    let theme = &frame.theme;
    match line {
        Line::Heading(key, count) => {
            let label = match *key {
                GroupKey::Prefix('~') => t!("people-group-owners").to_string(),
                GroupKey::Prefix('&') => t!("people-group-admins").to_string(),
                GroupKey::Prefix('@') => t!("people-group-operators").to_string(),
                GroupKey::Prefix('%') => t!("people-group-half-operators").to_string(),
                GroupKey::Prefix('+') => t!("people-group-voiced").to_string(),
                GroupKey::Prefix(p) => t!("people-group-prefix", prefix = p).to_string(),
                GroupKey::Online => t!("people-online").to_string(),
                GroupKey::Away => t!("people-away").to_string(),
                GroupKey::Offline => t!("people-offline").to_string(),
            };
            div()
                .pt_2()
                .px_2()
                .flex()
                .items_center()
                .justify_between()
                .text_xs()
                .font_weight(FontWeight::BOLD)
                .text_color(theme.muted)
                .child(label.to_uppercase())
                .child(
                    div()
                        .px_1p5()
                        .rounded_full()
                        .bg(theme.selected.opacity(0.6))
                        .text_size(px(10.))
                        .font_weight(FontWeight::NORMAL)
                        .text_color(theme.faint)
                        .child(count.to_string()),
                )
                .into_any_element()
        }
        Line::Member(mid, prefix) => {
            let w = frame.client.read();
            let Some(m) = w.members.get(mid) else {
                return div().into_any_element();
            };
            // Your own record can still carry the backend's placeholder
            // nick; the network knows what you are called there.
            let name = if w.is_self(&m.id, &frame.network) {
                people::self_name(&w, &frame.network).unwrap_or_else(|| people::name(m).to_string())
            } else {
                people::name(m).to_string()
            };
            let ignored = people::is_ignored(&frame.ignore, m);
            let status = people::status_label(m, frame.now);
            let bot = m.role == MemberRole::Bot;
            let device = match m.device {
                Some(MemberDevice::Phone) => Some(t!("people-on-phone")),
                Some(MemberDevice::Tablet) => Some(t!("people-on-tablet")),
                _ => None,
            };
            let mut states: Vec<String> = vec![match m.status {
                MemberStatus::Online => t!("people-online").to_string(),
                MemberStatus::Away => t!("people-away").to_string(),
                _ => t!("people-offline").to_string(),
            }];
            states.extend(status.clone());
            if bot {
                states.push(t!("people-bot").to_string());
            }
            states.extend(device.map(str::to_string));
            if ignored {
                states.push(t!("people-ignored").to_string());
            }
            let ctx = MemberCtx {
                session: frame.session.clone(),
                member: m.id.clone(),
                conversation: Some(frame.conversation.clone()),
            };
            let click = ctx.clone();
            let faded = m.status == MemberStatus::Offline || ignored;
            let name_color = if faded {
                theme.faint
            } else {
                ui::avatar::name_color(m, frame.colored_names, theme)
                    .map(theme::hex)
                    .unwrap_or(theme.text)
            };
            let picture = ui::avatar::person(
                ui::avatar::Person {
                    member: m,
                    name: &name,
                    server: frame.server.as_ref(),
                    presence: true,
                    key: &format!("member-{}", m.id),
                },
                px(ui::avatar::SMALL),
                theme,
            );
            div()
                .id(ElementId::Name(format!("member-{}", m.id).into()))
                .test_support()
                .role(Role::ListItem)
                .aria_label(SharedString::from(name.clone()))
                .aria_description(SharedString::from(states.join(", ")))
                .px_2()
                .py_1()
                .rounded_md()
                .flex()
                .items_center()
                .gap_2()
                .cursor_pointer()
                .hover(|d| d.bg(theme.hover))
                .child(picture.when(m.status == MemberStatus::Offline, |d| d.opacity(0.45)))
                .when(!prefix.is_empty(), |d| {
                    d.child(
                        div()
                            .mr(px(-6.))
                            .font_family(theme.mono_font.clone())
                            .font_weight(FontWeight::BOLD)
                            .text_color(theme.online)
                            .text_sm()
                            .child(prefix.clone()),
                    )
                })
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .truncate()
                                .text_sm()
                                .text_color(name_color)
                                .when(ignored, |d| d.line_through())
                                .child(name),
                        )
                        .children(status.map(|s| {
                            div()
                                .id(ElementId::Name(format!("member-status-{}", m.id).into()))
                                .test_support()
                                .aria_label(SharedString::from(s.clone()))
                                .text_xs()
                                .text_color(theme.muted)
                                .truncate()
                                .child(s)
                        })),
                )
                .when(ignored, |d| {
                    d.child(ui::icon::icon(IconName::EyeOff, theme.faint))
                })
                .children(device.map(|label| {
                    div()
                        .id(ElementId::Name(format!("member-device-{}", m.id).into()))
                        .test_support()
                        .aria_label(label)
                        .tooltip(ui::tooltip::text(label))
                        .text_xs()
                        .child("📱")
                }))
                .when(bot, |d| {
                    d.child(
                        div()
                            .id(ElementId::Name(format!("member-bot-{}", m.id).into()))
                            .test_support()
                            .aria_label(t!("people-bot"))
                            .px_1p5()
                            .rounded_full()
                            .bg(theme.selected)
                            .text_color(theme.accent)
                            .text_xs()
                            .font_weight(FontWeight::BOLD)
                            .child(t!("people-bot").to_uppercase()),
                    )
                })
                .on_click(move |_, window, cx| crate::features::profile::open(&click, window, cx))
                .context_menu(move |menu, window, cx| {
                    let items = registry::get(cx).member_menu.to_vec();
                    let items = ui::context_menu::visible(&items, &ctx, cx);
                    ui::context_menu::fill(menu, items, ctx.clone(), window, cx)
                })
                .into_any_element()
        }
    }
}

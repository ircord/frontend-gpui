//! The inbox page and its badge (package FD2).
//!
//! Your private attention inbox across every network: mentions, replies to
//! your messages and direct messages, newest first under day headings, in
//! six views (all, unread, mentions, replies, direct, dismissed). Opening an
//! item jumps to it in its conversation; dismissing one only hides it here.
//! Neither marks the conversation read. Its row at the top of the sidebar
//! opens it and shows the unread count. Follows the web client's
//! `InboxPage.vue`.
//!
//! The row helpers at the bottom are shared with `features::saved`.

use crate::ui::focus::Reachable;
use gpui_kit::TestSupportExt;
use gpui_kit::prelude::*;
use gpui_kit::*;
use ircord_client::contract::{
    ConversationKind, InboxItem, InboxItemKind, InboxItemReasonsItem, InboxItemStatus, Member,
};
use ircord_client::store::inbox::{self, Filter, List};
use ircord_client::store::{Connection, Workspace, parse_time, people};

use crate::features::shortcuts::MarkRead;
use crate::message::layout::Look;
use crate::ui::page::Hint;

use crate::registry::{self, Pane, SidebarCtx, Slot};
use crate::route::{PaneKind, Route};
use crate::session::{Session, SessionEvent};
use crate::theme::{self, Theme};
use crate::ui;
use crate::ui::icon::IconName;

pub fn init(cx: &mut App) {
    registry::register(cx, |r| {
        r.panes.add(Pane::new(PaneKind::Inbox, |ctx, _, cx| {
            view(&ctx.session, cx).into_any_element()
        }));
        r.sidebar_footer.add(
            Slot::new("inbox", |ctx: &SidebarCtx, _, cx| {
                Some(badge(&ctx.session, cx).into_any_element())
            })
            .order(-40),
        );
    });
}

/// Show the inbox.
pub fn open(session: &Entity<Session>, cx: &mut App) {
    session.update(cx, |s, cx| s.navigate(Route::Inbox, cx));
}

/// The label of a view's tab.
pub fn view_label(filter: Filter) -> &'static str {
    match filter {
        Filter::All => t!("inbox-saved-view-all"),
        Filter::Unread => t!("inbox-saved-view-unread"),
        Filter::Mentions => t!("inbox-saved-view-mentions"),
        Filter::Replies => t!("inbox-saved-view-replies"),
        Filter::Direct => t!("inbox-saved-view-direct"),
        Filter::Dismissed => t!("inbox-saved-view-dismissed"),
    }
}

fn empty_text(filter: Filter) -> &'static str {
    match filter {
        Filter::All => t!("inbox-saved-empty-all"),
        Filter::Unread => t!("inbox-saved-empty-unread"),
        Filter::Mentions => t!("inbox-saved-empty-mentions"),
        Filter::Replies => t!("inbox-saved-empty-replies"),
        Filter::Direct => t!("inbox-saved-empty-direct"),
        Filter::Dismissed => t!("inbox-saved-empty-dismissed"),
    }
}

fn reason_label(reason: InboxItemReasonsItem) -> Option<&'static str> {
    Some(match reason {
        InboxItemReasonsItem::Mention => t!("inbox-saved-reason-mention"),
        InboxItemReasonsItem::Reply => t!("inbox-saved-reason-reply"),
        InboxItemReasonsItem::Direct => t!("inbox-saved-reason-direct"),
        InboxItemReasonsItem::Unknown => return None,
    })
}

/// The row's edge colour follows its first reason: mentions match the
/// timeline's highlight.
fn reason_color(reason: Option<&InboxItemReasonsItem>, theme: &Theme) -> Hsla {
    match reason {
        Some(InboxItemReasonsItem::Reply) => theme.accent,
        Some(InboxItemReasonsItem::Direct) => theme.online,
        _ => theme.mention,
    }
}

fn reason_icon(reason: InboxItemReasonsItem) -> IconName {
    match reason {
        InboxItemReasonsItem::Reply => IconName::Undo,
        InboxItemReasonsItem::Direct => IconName::User,
        _ => IconName::Asterisk,
    }
}

// ---- The badge ----

/// The header button, kept for the session it shows.
struct Badge(Entity<BadgeView>);
impl Global for Badge {}

fn badge(session: &Entity<Session>, cx: &mut App) -> Entity<BadgeView> {
    if let Some(Badge(view)) = cx.try_global::<Badge>()
        && view.read(cx).session == *session
    {
        return view.clone();
    }
    let view = cx.new(|cx| BadgeView::new(session.clone(), cx));
    cx.set_global(Badge(view.clone()));
    view
}

pub struct BadgeView {
    session: Entity<Session>,
    _subscription: Subscription,
}

impl BadgeView {
    fn new(session: Entity<Session>, cx: &mut Context<Self>) -> Self {
        let subscription = cx.subscribe(&session, |_, _, event: &SessionEvent, cx| match event {
            SessionEvent::Changed(changes) if changes.touches(inbox::TOPIC) => cx.notify(),
            SessionEvent::Route | SessionEvent::Selection => cx.notify(),
            _ => {}
        });
        Self {
            session,
            _subscription: subscription,
        }
    }
}

impl Render for BadgeView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme::get(cx).clone();
        let session = self.session.read(cx);
        let active = session.route() == Some(Route::Inbox);
        let unread = match session.client() {
            Some(client) => {
                inbox::keep(client);
                client.read().inbox.unread.unwrap_or(0)
            }
            None => 0,
        };
        let label: SharedString = if unread > 0 {
            t!("inbox-saved-inbox-unread", count = unread).into()
        } else {
            t!("inbox-saved-inbox").into()
        };
        let count = (unread > 0).then(|| {
            if unread > 99 {
                "99+".to_string()
            } else {
                unread.to_string()
            }
        });
        let session = self.session.clone();
        crate::sidebar::page_row(
            "open-inbox",
            IconName::Inbox,
            t!("inbox-saved-inbox"),
            label,
            count,
            active,
            &theme,
            move |_, _, cx| open(&session, cx),
        )
    }
}

// ---- The page ----

/// The inbox page, kept across visits so the chosen view stays.
struct Page(Entity<InboxView>);
impl Global for Page {}

fn view(session: &Entity<Session>, cx: &mut App) -> Entity<InboxView> {
    if let Some(Page(view)) = cx.try_global::<Page>()
        && view.read(cx).session == *session
    {
        return view.clone();
    }
    let view = cx.new(|cx| InboxView::new(session.clone(), cx));
    cx.set_global(Page(view.clone()));
    view
}

pub struct InboxView {
    session: Entity<Session>,
    filter: Filter,
    /// The row the arrow keys moved to.
    selected: usize,
    focus: FocusHandle,
    /// Take the keyboard on the next draw (the page was just shown).
    take_focus: bool,
    _subscription: Subscription,
}

impl InboxView {
    fn new(session: Entity<Session>, cx: &mut Context<Self>) -> Self {
        let subscription =
            cx.subscribe(
                &session,
                |this, session, event: &SessionEvent, cx| match event {
                    SessionEvent::Changed(changes)
                        if changes.touches(inbox::TOPIC) || changes.sidebar =>
                    {
                        this.ensure_loaded(cx);
                        cx.notify();
                    }
                    SessionEvent::Connection | SessionEvent::Route => {
                        if matches!(event, SessionEvent::Route)
                            && session.read(cx).route() == Some(Route::Inbox)
                        {
                            this.take_focus = true;
                        }
                        this.ensure_loaded(cx);
                        cx.notify();
                    }
                    _ => {}
                },
            );
        let mut this = Self {
            session,
            filter: Filter::All,
            selected: 0,
            focus: cx.focus_handle(),
            take_focus: true,
            _subscription: subscription,
        };
        this.ensure_loaded(cx);
        this
    }

    pub fn filter(&self) -> Filter {
        self.filter
    }

    pub fn set_filter(&mut self, filter: Filter, cx: &mut Context<Self>) {
        self.filter = filter;
        self.selected = 0;
        self.ensure_loaded(cx);
        cx.notify();
    }

    /// Read the shown view if it is missing or out of date.
    fn ensure_loaded(&mut self, cx: &mut Context<Self>) {
        let session = self.session.read(cx);
        if session.route() != Some(Route::Inbox) {
            return;
        }
        let Some(client) = session.client() else {
            return;
        };
        if client.read().connection != Some(Connection::Live) {
            return;
        }
        if List::wanted(client.read().inbox.list(self.filter)) {
            inbox::load(client.handle(), self.filter);
        }
    }

    fn retry(&mut self, cx: &mut Context<Self>) {
        if let Some(client) = self.session.read(cx).client() {
            inbox::load(client.handle(), self.filter);
        }
    }

    fn load_more(&mut self, cx: &mut Context<Self>) {
        if let Some(client) = self.session.read(cx).client() {
            inbox::load_more(client.handle(), self.filter);
        }
    }

    /// The shown items: (message, conversation, dismissed, unavailable).
    fn shown(&self, cx: &App) -> Vec<(String, String, bool, bool)> {
        let Some(client) = self.session.read(cx).client() else {
            return Vec::new();
        };
        let w = client.read();
        w.inbox
            .list(self.filter)
            .filter(|l| l.loaded)
            .map(|l| {
                l.items
                    .iter()
                    .map(|i| {
                        (
                            i.message_id.clone(),
                            i.conversation_id.clone(),
                            i.dismissed,
                            i.status == InboxItemStatus::Unavailable,
                        )
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The keys the hint bar names: up and down choose, left and right
    /// change the view, Enter opens, E dismisses or restores, Escape leaves.
    fn on_key(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let k = &e.keystroke;
        if k.modifiers.modified() {
            return;
        }
        let items = self.shown(cx);
        let count = items.len();
        let current = items
            .get(self.selected.min(count.saturating_sub(1)))
            .cloned();
        match k.key.as_str() {
            "down" | "up" if count > 0 => {
                let step = if k.key == "down" { 1 } else { count - 1 };
                self.selected = (self.selected.min(count - 1) + step) % count;
                cx.notify();
            }
            "left" | "right" => {
                let views = Filter::ALL;
                let at = views.iter().position(|f| *f == self.filter).unwrap_or(0);
                let n = views.len();
                let next = if k.key == "right" {
                    (at + 1) % n
                } else {
                    (at + n - 1) % n
                };
                self.set_filter(views[next], cx);
            }
            // Enter on a focused button already clicks it.
            "enter" if self.focus.is_focused(window) => {
                let Some((message, conversation, _, gone)) = current else {
                    return;
                };
                if gone {
                    ui::toast::info(window, cx, t!("inbox-saved-closed"));
                } else {
                    crate::timeline::reveal(&self.session, &conversation, &message, window, cx);
                }
            }
            "e" => {
                let Some((message, _, dismissed, _)) = current else {
                    return;
                };
                set_dismissed(&self.session, &message, !dismissed, window, cx);
            }
            "escape" => ui::page::leave(&self.session, cx),
            _ => return,
        }
        cx.stop_propagation();
    }

    #[allow(clippy::too_many_arguments)]
    fn row(
        &self,
        w: &Workspace,
        item: &InboxItem,
        pending: bool,
        selected: bool,
        server: Option<&url::Url>,
        look: &Look,
        theme: &Theme,
    ) -> AnyElement {
        let id = item.message_id.clone();
        let available = item.status == InboxItemStatus::Available;
        let unread = item.unread && !item.dismissed;
        let gone = item.status == InboxItemStatus::Unavailable;
        let place = place(w, &item.conversation_id, gone);
        let network = network_name(w, &item.network_id);
        let who = sender(
            w,
            item.sender_id.as_deref().filter(|_| !gone),
            &item.network_id,
            server,
            look.colored_names,
            theme,
        );
        let text: SharedString = match item.status {
            InboxItemStatus::Available => item.excerpt.clone().unwrap_or_default().into(),
            InboxItemStatus::Deleted => t!("inbox-saved-deleted").into(),
            _ => t!("inbox-saved-unavailable").into(),
        };
        let label = t!(
            "inbox-saved-item",
            author = who.name.clone(),
            place = place.clone(),
            text = text.clone()
        );
        let (_, time) = day_and_time(&item.created_at, look.clock_12h);
        let edge = reason_color(item.reasons.first(), theme);

        let (session, conversation, message) = (
            self.session.clone(),
            item.conversation_id.clone(),
            item.message_id.clone(),
        );
        let open = move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
            if gone {
                return ui::toast::info(window, cx, t!("inbox-saved-closed"));
            }
            crate::timeline::reveal(&session, &conversation, &message, window, cx);
        };

        let dismissed = item.dismissed;
        let toggle_label = if dismissed {
            t!("inbox-saved-restore")
        } else {
            t!("inbox-saved-dismiss")
        };
        let session = self.session.clone();
        let toggle_id = id.clone();
        let toggle = ui::icon::button(
            ElementId::Name(format!("inbox-toggle-{id}").into()),
            if dismissed {
                IconName::Undo
            } else {
                IconName::Check
            },
            toggle_label,
            theme,
            move |_, window, cx| {
                cx.stop_propagation();
                if !pending {
                    set_dismissed(&session, &toggle_id, !dismissed, window, cx);
                }
            },
        );

        let mut location = Vec::new();
        if unread {
            location.push(
                div()
                    .id(ElementId::Name(format!("inbox-unread-{id}").into()))
                    .test_support()
                    .role(Role::Image)
                    .aria_label(t!("inbox-saved-unread"))
                    .size(px(7.))
                    .rounded_full()
                    .bg(theme.text)
                    .into_any_element(),
            );
        }
        location.push(
            div()
                .truncate()
                .when(unread, |d| d.text_color(theme.text))
                .child(format!("{place} · {network}"))
                .into_any_element(),
        );
        for (i, reason) in item.reasons.iter().enumerate() {
            let Some(r) = reason_label(*reason) else {
                continue;
            };
            location.push(
                div()
                    .id(ElementId::Name(format!("inbox-reason-{id}-{i}").into()))
                    .test_support()
                    .role(Role::Status)
                    .aria_label(r)
                    .flex()
                    .items_center()
                    .gap_1()
                    .px_1p5()
                    .rounded_sm()
                    .bg(theme.hover)
                    .text_size(px(11.))
                    .text_color(theme.muted)
                    .child(
                        gpui_kit::component::Icon::new(reason_icon(*reason))
                            .size(px(11.))
                            .text_color(theme.muted),
                    )
                    .child(r)
                    .into_any_element(),
            );
        }
        let excerpt = div()
            .text_color(if available { theme.body } else { theme.muted })
            .when(
                !available || item.kind == Some(InboxItemKind::Action),
                |d| d.italic(),
            )
            .child(text);

        div()
            .id(ElementId::Name(format!("inbox-row-{id}").into()))
            .relative()
            .flex()
            .items_start()
            .gap_1()
            .rounded_md()
            .when(selected, |d| d.bg(theme.selected))
            .when(!selected, |d| d.hover(|d| d.bg(theme.row_hover)))
            .child(
                div()
                    .absolute()
                    .left_0()
                    .top_3()
                    .bottom_3()
                    .w(px(3.))
                    .rounded_full()
                    .bg(edge)
                    .when(!unread, |d| d.opacity(0.35)),
            )
            .child(
                div()
                    .id(ElementId::Name(format!("inbox-item-{id}").into()))
                    .test_support()
                    .role(Role::Button)
                    .reachable(theme)
                    .aria_label(SharedString::from(label))
                    .aria_selected(selected)
                    .flex_1()
                    .min_w_0()
                    .cursor_pointer()
                    .on_click(open)
                    .child(result_line(
                        &format!("inbox-{id}"),
                        &who,
                        location,
                        time,
                        excerpt.into_any_element(),
                        theme,
                    )),
            )
            .child(div().mt_2().mr_2().child(toggle))
            .into_any_element()
    }
}

/// Dismiss an item, or restore it, saying so if that fails.
pub fn set_dismissed(
    session: &Entity<Session>,
    message: &str,
    dismissed: bool,
    window: &mut Window,
    cx: &mut App,
) {
    let Some(client) = session.read(cx).client().cloned() else {
        return;
    };
    if client.read().connection != Some(Connection::Live) {
        return ui::toast::info(window, cx, t!("inbox-saved-inbox-offline"));
    }
    let answer = inbox::set_dismissed(client.handle(), message, dismissed);
    window
        .spawn(cx, async move |cx| {
            let result = answer.await;
            cx.update(|window, cx| {
                if let Ok(Err(e)) = result {
                    let text = if dismissed {
                        t!("inbox-saved-dismiss-failed", error = e)
                    } else {
                        t!("inbox-saved-restore-failed", error = e)
                    };
                    ui::toast::error(window, cx, text);
                }
            })
            .ok();
        })
        .detach();
}

impl Render for InboxView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme::get(cx).clone();
        if std::mem::take(&mut self.take_focus) {
            self.focus.focus(window, cx);
        }
        let look = crate::message::layout::settings(cx);
        let server = server(&self.session, cx);
        let Some(client) = self.session.read(cx).client().cloned() else {
            return div().into_any_element();
        };
        let w = client.read();
        let state = &w.inbox;
        let unread = state.unread.unwrap_or(0);
        let meta: SharedString = if unread > 0 {
            t!("inbox-saved-unread-count", count = unread).into()
        } else {
            t!("inbox-saved-caught-up").into()
        };

        let tabs = div()
            .id("inbox-views")
            .test_support()
            .role(Role::TabList)
            .aria_label(t!("inbox-saved-views"))
            .flex()
            .flex_wrap()
            .flex_none()
            .max_w_full()
            .self_start()
            .gap_1()
            .p_1()
            .rounded_lg()
            .bg(theme.sidebar)
            .children(Filter::ALL.into_iter().map(|filter| {
                let label = view_label(filter);
                let selected = self.filter == filter;
                div()
                    .id(ElementId::Name(
                        format!("inbox-view-{}", filter.as_str()).into(),
                    ))
                    .test_support()
                    .role(Role::Tab)
                    .reachable(&theme)
                    .aria_label(label)
                    .aria_selected(selected)
                    .px_3()
                    .py_1()
                    .rounded_md()
                    .text_sm()
                    .cursor_pointer()
                    .flex()
                    .gap_1()
                    .text_color(if selected { theme.accent } else { theme.muted })
                    .when(selected, |d| d.bg(theme.selected))
                    .when(!selected, |d| d.hover(|d| d.bg(theme.hover)))
                    .child(label)
                    .when(filter == Filter::Unread && unread > 0, |d| {
                        d.child(div().text_color(theme.faint).child(unread.to_string()))
                    })
                    .on_click(cx.listener(move |this, _, _, cx| this.set_filter(filter, cx)))
            }));

        let list = state.list(self.filter);
        let body: AnyElement = match list {
            Some(l) if !l.loaded && l.error.is_some() => div()
                .id("inbox-failed")
                .test_support()
                .role(Role::Alert)
                .aria_label(SharedString::from(t!(
                    "inbox-saved-inbox-failed",
                    error = l.error.clone().unwrap_or_default()
                )))
                .p_8()
                .flex()
                .flex_col()
                .items_center()
                .gap_2()
                .text_color(theme.muted)
                .child(t!(
                    "inbox-saved-inbox-failed",
                    error = l.error.clone().unwrap_or_default()
                ))
                .child(link_button(
                    "inbox-retry",
                    t!("inbox-saved-try-again"),
                    &theme,
                    cx.listener(|this, _, _, cx| this.retry(cx)),
                ))
                .into_any_element(),
            Some(l) if l.loaded && l.items.is_empty() => {
                let text = if l.next.is_some() {
                    t!("inbox-saved-nothing-on-page")
                } else {
                    empty_text(self.filter)
                };
                ui::empty::with_icon("inbox-empty", Some(IconName::Inbox), text, None, &theme)
                    .into_any_element()
            }
            Some(l) if l.loaded => {
                let selected = self.selected.min(l.items.len().saturating_sub(1));
                let mut groups: Vec<(String, Vec<AnyElement>)> = Vec::new();
                for (i, item) in l.items.iter().enumerate() {
                    let (day, _) = day_and_time(&item.created_at, look.clock_12h);
                    let row = self.row(
                        &w,
                        item,
                        state.pending.contains(&item.message_id),
                        i == selected,
                        server.as_ref(),
                        &look,
                        &theme,
                    );
                    match groups.last_mut() {
                        Some((last, rows)) if *last == day => rows.push(row),
                        _ => groups.push((day, vec![row])),
                    }
                }
                let more = more(
                    "inbox-load-more",
                    t!("inbox-saved-load-older"),
                    l.next.is_some(),
                    l.loading_more,
                    l.more_error.as_deref(),
                    &theme,
                    cx.listener(|this, _, _, cx| this.load_more(cx)),
                );
                div()
                    .id("inbox-list")
                    .test_support()
                    .role(Role::List)
                    .aria_label(SharedString::from(t!(
                        "inbox-saved-list",
                        view = view_label(self.filter)
                    )))
                    .flex()
                    .flex_col()
                    .gap_3()
                    .children(groups.into_iter().map(|(day, rows)| {
                        div()
                            .flex()
                            .flex_col()
                            .gap_0p5()
                            .child(day_heading(day, &theme))
                            .children(rows)
                    }))
                    .children(more)
                    .into_any_element()
            }
            _ => loading("inbox-loading", t!("inbox-saved-inbox-loading"), &theme),
        };
        drop(w);

        let page_body = div().px_4().pb_4().child(body);
        let session = self.session.clone();
        let page = ui::page::Page::new("inbox-page", t!("inbox-saved-inbox"), page_body)
            .description(t!("inbox-saved-inbox-description"))
            .toolbar(div().flex().child(tabs))
            .meta("inbox-meta", meta)
            .close(t!("inbox-saved-return-label"), move |_, _, cx| {
                ui::page::leave(&session, cx)
            })
            .hints(vec![
                Hint::new(
                    &[t!("inbox-saved-key-up"), t!("inbox-saved-key-down")],
                    t!("inbox-saved-key-choose"),
                ),
                Hint::new(
                    &[t!("inbox-saved-key-left"), t!("inbox-saved-key-right")],
                    t!("inbox-saved-key-views"),
                ),
                Hint::new(&[t!("inbox-saved-key-enter")], t!("inbox-saved-key-open")),
                Hint::new(&[t!("inbox-saved-key-e")], t!("inbox-saved-key-dismiss")),
                Hint::new(
                    &[t!("inbox-saved-key-escape")],
                    t!("inbox-saved-key-return"),
                ),
            ])
            .render(&theme);
        div()
            .id("inbox-keys")
            .track_focus(&self.focus)
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .on_key_down(cx.listener(Self::on_key))
            // Escape is bound to Mark read, which means nothing here.
            .capture_action(cx.listener(|this, _: &MarkRead, _, cx| {
                ui::page::leave(&this.session, cx);
                cx.stop_propagation();
            }))
            .child(page)
            .into_any_element()
    }
}

// ---- Shared by the inbox and the saved messages ----

/// Where a message is: `#channel` or `@nick`, or why it is unavailable.
pub(crate) fn place(w: &Workspace, conversation: &str, unavailable: bool) -> String {
    match w.conversation(conversation) {
        Some(c) if !unavailable => match c.kind {
            ConversationKind::Direct => format!("@{}", c.name),
            _ => format!("#{}", c.name.trim_start_matches('#')),
        },
        _ => t!("inbox-saved-unavailable-conversation").to_string(),
    }
}

pub(crate) fn network_name(w: &Workspace, network: &str) -> String {
    w.network(network)
        .map(|n| n.name.clone())
        .unwrap_or_else(|| t!("inbox-saved-network-removed").to_string())
}

/// The sender's display name, else their nick.
pub(crate) fn author(w: &Workspace, sender: &str, network: &str) -> String {
    if w.is_self(sender, network) {
        let user = &w.user;
        return if user.display_name.is_empty() {
            user.nick.clone()
        } else {
            user.display_name.clone()
        };
    }
    match w.members.get(sender) {
        Some(m) if !m.display_name.is_empty() => m.display_name.clone(),
        Some(m) if !m.nick.is_empty() => m.nick.clone(),
        _ => t!("inbox-saved-someone").to_string(),
    }
}

/// The local day label ("Today", "Yesterday", a date) and time of an
/// RFC 3339 time, as the web's `useTimeFormat`.
pub(crate) fn day_and_time(at: &str, clock_12h: bool) -> (String, String) {
    use chrono::TimeZone;
    match chrono::Local.timestamp_millis_opt(parse_time(at)).single() {
        Some(local) if !at.is_empty() => {
            let today = chrono::Local::now().date_naive();
            (
                day_label(local.date_naive(), today),
                local
                    .format(if clock_12h { "%-I:%M %p" } else { "%H:%M" })
                    .to_string(),
            )
        }
        _ => (String::new(), String::new()),
    }
}

fn day_label(day: chrono::NaiveDate, today: chrono::NaiveDate) -> String {
    use chrono::Datelike;
    if day == today {
        t!("inbox-saved-today").into()
    } else if today.pred_opt() == Some(day) {
        t!("inbox-saved-yesterday").into()
    } else if day.year() == today.year() {
        day.format("%A, %B %-d").to_string()
    } else {
        day.format("%A, %B %-d, %Y").to_string()
    }
}

/// A day's label over its rows, in small capitals as on the web.
pub(crate) fn day_heading(day: String, theme: &Theme) -> impl IntoElement + use<> {
    div()
        .id(ElementId::Name(format!("day-{day}").into()))
        .test_support()
        .role(Role::Heading)
        .aria_label(SharedString::from(day.clone()))
        .h(px(32.))
        .px_3()
        .flex()
        .items_center()
        .text_xs()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(theme.muted)
        .child(day.to_uppercase())
}

/// Who sent a listed message: their member for the picture, the name the
/// list shows and its colour.
pub(crate) struct Sender {
    pub member: Option<Member>,
    pub name: String,
    pub color: Option<Hsla>,
    /// The server pictures may load from.
    pub server: Option<url::Url>,
}

/// The sender `id` on `network`, or nobody.
pub(crate) fn sender(
    w: &Workspace,
    id: Option<&str>,
    network: &str,
    server: Option<&url::Url>,
    colored_names: bool,
    theme: &Theme,
) -> Sender {
    let name = id.map(|s| author(w, s, network)).unwrap_or_default();
    let member = id.and_then(|s| people::person(w, s, network)).cloned();
    let color = member
        .as_ref()
        .and_then(|m| ui::avatar::name_color(m, colored_names, theme))
        .map(theme::hex);
    Sender {
        member,
        name,
        color,
        server: server.cloned(),
    }
}

/// The server signed in to, where pictures may load from.
pub(crate) fn server(session: &Entity<Session>, cx: &App) -> Option<url::Url> {
    session.read(cx).server().map(|s| s.url().clone())
}

/// One message in a list (inbox, saved, search), as the web's
/// `MessageResult.vue`: the sender's picture in a 40 px gutter, then their
/// coloured name, `location` and the time on one line and up to three lines
/// of `text` under it. `key` names the picture's test ID.
pub(crate) fn result_line(
    key: &str,
    who: &Sender,
    location: Vec<AnyElement>,
    time: String,
    text: AnyElement,
    theme: &Theme,
) -> Div {
    let size = px(40.);
    let picture = match &who.member {
        Some(m) => ui::avatar::person(
            ui::avatar::Person {
                member: m,
                name: &who.name,
                server: who.server.as_ref(),
                presence: false,
                key,
            },
            size,
            theme,
        ),
        None => ui::avatar::avatar(
            if who.name.is_empty() { "?" } else { &who.name },
            None,
            size,
            theme,
        ),
    };
    div()
        .w_full()
        .px_3()
        .py_2p5()
        .flex()
        .items_start()
        .gap(px(14.))
        .child(div().mt_0p5().child(picture))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap_0p5()
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap_x_2()
                        .gap_y_0p5()
                        .min_w_0()
                        .text_xs()
                        .text_color(theme.muted)
                        .when(!who.name.is_empty(), |d| {
                            d.child(
                                div()
                                    .id(ElementId::Name(format!("author-{key}").into()))
                                    .test_support()
                                    .truncate()
                                    .text_size(px(15.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(who.color.unwrap_or(theme.text))
                                    .child(who.name.clone()),
                            )
                        })
                        .children(location)
                        .child(
                            div()
                                .ml_auto()
                                .pl_2()
                                .flex_none()
                                .text_color(theme.faint)
                                .child(time),
                        ),
                )
                .child(div().text_sm().line_clamp(3).child(text)),
        )
}

pub(crate) fn loading(id: &'static str, label: &'static str, theme: &Theme) -> AnyElement {
    div()
        .id(id)
        .test_support()
        .role(Role::Status)
        .aria_label(label)
        .p_8()
        .flex()
        .justify_center()
        .text_sm()
        .text_color(theme.muted)
        .child(label)
        .into_any_element()
}

pub(crate) fn link_button<F: Fn(&ClickEvent, &mut Window, &mut App) + 'static>(
    id: &'static str,
    label: &'static str,
    theme: &Theme,
    on_click: F,
) -> impl IntoElement + use<F> {
    div()
        .id(id)
        .test_support()
        .role(Role::Button)
        .reachable(theme)
        .aria_label(label)
        .px_2()
        .py_1()
        .rounded_md()
        .text_sm()
        .cursor_pointer()
        .text_color(theme.link)
        .hover(|d| d.bg(theme.hover))
        .child(label)
        .on_click(on_click)
}

/// The row under a list that loads older items, or says why it could not.
pub(crate) fn more(
    id: &'static str,
    label: &'static str,
    has_more: bool,
    loading: bool,
    error: Option<&str>,
    theme: &Theme,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Option<AnyElement> {
    if !has_more {
        return None;
    }
    let row = div().flex().flex_col().items_center().gap_1().py_2();
    Some(if loading {
        row.child(
            div()
                .text_sm()
                .text_color(theme.muted)
                .child(t!("inbox-saved-loading-more")),
        )
        .into_any_element()
    } else {
        row.when_some(error, |d, e| {
            d.child(
                div()
                    .text_sm()
                    .text_color(theme.danger)
                    .child(t!("inbox-saved-more-failed", error = e)),
            )
        })
        .child(link_button(id, label, theme, on_click))
        .into_any_element()
    })
}

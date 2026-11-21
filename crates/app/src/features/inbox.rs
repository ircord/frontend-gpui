//! The inbox page and its badge (package FD2).
//!
//! Your private attention inbox across every network: mentions, replies to
//! your messages and direct messages, newest first under day headings, in
//! six views (all, unread, mentions, replies, direct, dismissed). Opening an
//! item jumps to it in its conversation; dismissing one only hides it here.
//! Neither marks the conversation read. The button in the conversation
//! header opens it and shows the unread count. Follows the web client's
//! `InboxPage.vue`.
//!
//! The row helpers at the bottom are shared with `features::saved`.

use crate::ui::focus::Reachable;
use gpui_kit::TestSupportExt;
use gpui_kit::prelude::*;
use gpui_kit::*;
use ircord_client::contract::{
    ConversationKind, InboxItem, InboxItemKind, InboxItemReasonsItem, InboxItemStatus,
};
use ircord_client::store::inbox::{self, Filter, List};
use ircord_client::store::{Connection, Workspace, parse_time};

use crate::registry::{self, HeaderCtx, Pane, Slot};
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
        r.header_buttons.add(
            Slot::new("inbox", |ctx: &HeaderCtx, _, cx| {
                Some(badge(&ctx.session, cx).into_any_element())
            })
            .order(80),
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
        let subscription = cx.subscribe(&session, |_, _, event: &SessionEvent, cx| {
            if let SessionEvent::Changed(changes) = event
                && changes.touches(inbox::TOPIC)
            {
                cx.notify();
            }
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
        let unread = match self.session.read(cx).client() {
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
        let session = self.session.clone();
        div()
            .relative()
            .child(ui::icon::button(
                "open-inbox",
                IconName::Inbox,
                label,
                &theme,
                move |_, _, cx| open(&session, cx),
            ))
            .when(unread > 0, |d| {
                d.child(
                    div()
                        .absolute()
                        .top(px(-4.))
                        .right(px(-4.))
                        .min_w(px(16.))
                        .h(px(16.))
                        .px_1()
                        .rounded_full()
                        .bg(theme.danger)
                        .text_color(theme.on_accent)
                        .text_xs()
                        .font_weight(FontWeight::BOLD)
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(if unread > 99 {
                            "99+".to_string()
                        } else {
                            unread.to_string()
                        }),
                )
            })
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
    _subscription: Subscription,
}

impl InboxView {
    fn new(session: Entity<Session>, cx: &mut Context<Self>) -> Self {
        let subscription =
            cx.subscribe(&session, |this, _, event: &SessionEvent, cx| match event {
                SessionEvent::Changed(changes)
                    if changes.touches(inbox::TOPIC) || changes.sidebar =>
                {
                    this.ensure_loaded(cx);
                    cx.notify();
                }
                SessionEvent::Connection | SessionEvent::Route => {
                    this.ensure_loaded(cx);
                    cx.notify();
                }
                _ => {}
            });
        let mut this = Self {
            session,
            filter: Filter::All,
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

    fn row(&self, w: &Workspace, item: &InboxItem, pending: bool, theme: &Theme) -> AnyElement {
        let id = item.message_id.clone();
        let available = item.status == InboxItemStatus::Available;
        let unread = item.unread && !item.dismissed;
        let place = place(
            w,
            &item.conversation_id,
            item.status == InboxItemStatus::Unavailable,
        );
        let network = network_name(w, &item.network_id);
        let author = item
            .sender_id
            .as_deref()
            .map(|s| author(w, s, &item.network_id))
            .unwrap_or_default();
        let text: SharedString = match item.status {
            InboxItemStatus::Available => item.excerpt.clone().unwrap_or_default().into(),
            InboxItemStatus::Deleted => t!("inbox-saved-deleted").into(),
            _ => t!("inbox-saved-unavailable").into(),
        };
        let label = t!(
            "inbox-saved-item",
            author = author.clone(),
            place = place.clone(),
            text = text.clone()
        );
        let (_, time) = day_and_time(&item.created_at);
        let reasons: Vec<&'static str> = item
            .reasons
            .iter()
            .copied()
            .filter_map(reason_label)
            .collect();
        let edge = reason_color(item.reasons.first(), theme);

        let (session, conversation, message) = (
            self.session.clone(),
            item.conversation_id.clone(),
            item.message_id.clone(),
        );
        let gone = item.status == InboxItemStatus::Unavailable;
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

        div()
            .id(ElementId::Name(format!("inbox-row-{id}").into()))
            .flex()
            .items_start()
            .gap_2()
            .rounded_md()
            .hover(|d| d.bg(theme.row_hover))
            .child(
                div()
                    .w(px(3.))
                    .mt_2()
                    .h(px(32.))
                    .rounded_full()
                    .bg(edge)
                    .when(!unread, |d| d.opacity(0.35)),
            )
            .child(
                div()
                    .id(ElementId::Name(format!("inbox-item-{id}").into()))
                    .test_support()
                    .role(Role::Button)
                    .reachable(&theme)
                    .aria_label(SharedString::from(label))
                    .flex_1()
                    .min_w_0()
                    .py_1p5()
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .cursor_pointer()
                    .on_click(open)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1p5()
                            .text_xs()
                            .text_color(if unread { theme.text } else { theme.muted })
                            .when(unread, |d| {
                                d.child(
                                    div()
                                        .id(ElementId::Name(format!("inbox-unread-{id}").into()))
                                        .test_support()
                                        .role(Role::Image)
                                        .aria_label(t!("inbox-saved-unread"))
                                        .size(px(7.))
                                        .rounded_full()
                                        .bg(theme.text),
                                )
                            })
                            .child(div().truncate().child(format!("{place} · {network}")))
                            .children(reasons.into_iter().enumerate().map(|(i, r)| {
                                div()
                                    .id(ElementId::Name(format!("inbox-reason-{id}-{i}").into()))
                                    .test_support()
                                    .role(Role::Status)
                                    .aria_label(r)
                                    .px_1p5()
                                    .rounded_sm()
                                    .bg(theme.hover)
                                    .text_color(theme.muted)
                                    .child(r)
                            }))
                            .child(div().flex_1())
                            .child(div().text_color(theme.faint).child(time)),
                    )
                    .child(
                        div()
                            .flex()
                            .gap_1()
                            .text_sm()
                            .min_w_0()
                            .when(!author.is_empty() && !gone, |d| {
                                d.child(
                                    div()
                                        .flex_none()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(theme.text)
                                        .child(author.clone()),
                                )
                            })
                            .child(
                                div()
                                    .truncate()
                                    .text_color(if available { theme.text } else { theme.muted })
                                    .when(
                                        !available || item.kind == Some(InboxItemKind::Action),
                                        |d| d.italic(),
                                    )
                                    .child(text),
                            ),
                    ),
            )
            .child(div().pt_1().child(toggle))
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
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme::get(cx).clone();
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
            .gap_1()
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
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .text_sm()
                    .cursor_pointer()
                    .flex()
                    .gap_1()
                    .text_color(if selected { theme.text } else { theme.muted })
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
                ui::empty::empty("inbox-empty", text, None, &theme).into_any_element()
            }
            Some(l) if l.loaded => {
                let mut groups: Vec<(String, Vec<AnyElement>)> = Vec::new();
                for item in &l.items {
                    let (day, _) = day_and_time(&item.created_at);
                    let row = self.row(&w, item, state.pending.contains(&item.message_id), &theme);
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

        let page_body = div()
            .px_4()
            .py_3()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .text_sm()
                    .text_color(theme.muted)
                    .child(t!("inbox-saved-inbox-description")),
            )
            .child(tabs)
            .child(body);
        ui::page::Page::new("inbox-page", t!("inbox-saved-inbox"), page_body)
            .action(
                div()
                    .id("inbox-meta")
                    .test_support()
                    .role(Role::Status)
                    .aria_label(meta.clone())
                    .text_sm()
                    .text_color(theme.muted)
                    .child(meta),
            )
            .render(&theme)
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

/// The local day heading and time of an RFC 3339 time.
pub(crate) fn day_and_time(at: &str) -> (String, String) {
    use chrono::TimeZone;
    match chrono::Local.timestamp_millis_opt(parse_time(at)).single() {
        Some(local) if !at.is_empty() => (
            local.format("%A, %-d %B %Y").to_string(),
            local.format("%H:%M").to_string(),
        ),
        _ => (String::new(), String::new()),
    }
}

pub(crate) fn day_heading(day: String, theme: &Theme) -> impl IntoElement + use<> {
    div()
        .id(ElementId::Name(format!("day-{day}").into()))
        .test_support()
        .role(Role::Heading)
        .aria_label(SharedString::from(day.clone()))
        .h(px(28.))
        .px_3()
        .flex()
        .items_center()
        .text_xs()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(theme.muted)
        .child(day)
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
        .reachable(&theme)
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

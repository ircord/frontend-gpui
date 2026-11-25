//! Search (package FD1), as the web client has it:
//!
//! - The search page ([`Route::Search`]): one list of conversations, people
//!   and server windows by name, then messages by text. Filters (`from:`,
//!   `mentions:`, `in:`, `has:`, `before:`, `after:`, `on:`, `author:`) are
//!   typed into the field or chosen in the filters dialog, and show as
//!   removable chips. Recent searches are kept on this device, per account.
//! - Open the same page from a conversation with its In filter filled in.
//! - Server history (the bouncer's or IRC server's own search) as a source
//!   when exactly one conversation is searched.
//!
//! Filters, requests, sources and recent searches are in
//! `ircord_client::search_filter`; this file draws them.

use crate::ui::focus::Reachable;
use std::time::Duration;

use chrono::{Local, NaiveDate};
use gpui_kit::TestSupportExt;
use gpui_kit::component::IndexPath;
use gpui_kit::component::Sizable;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::calendar::{Calendar, CalendarEvent, CalendarState, Date, Matcher};
use gpui_kit::component::combobox::{Combobox, ComboboxState};
use gpui_kit::component::date_picker::{DatePickerEvent, DatePickerState};
use gpui_kit::component::input::{
    Backspace, Escape, Input, InputEvent, InputState, MoveDown, MoveUp,
};
use gpui_kit::component::popover::Popover;
use gpui_kit::component::searchable_list::{SearchableListItem, SearchableVec};
use gpui_kit::prelude::*;
use gpui_kit::*;
use ircord_client::commands::{self as cmd, Plan};
use ircord_client::contract::{self as c, ConversationKind};
use ircord_client::format;
use ircord_client::search_filter::{
    self as sf, Author, Chip, Coverage, Filters, Has, Key, Query, RecentSearch, Source,
};
use ircord_client::store::Workspace;

use crate::features::{inbox, switcher};
use crate::registry::{self, HeaderCtx, KeyDefault, Pane, Slot};
use crate::route::{PaneKind, Route};
use crate::session::{Session, SessionEvent};
use crate::sidebar::layout;
use crate::theme::{self, Theme};
use crate::ui;
use crate::ui::icon::{IconName, LucideIcon};
use crate::ui::page::Hint;
use crate::ui::query_field::{
    self, AcceptSuggestion, SUGGESTION_KEYS, Selection, SelectionScroll as ResultScroll,
};

gpui_kit::actions!(ircord_search, [OpenSearch, SearchConversation]);

/// Typing settles for this long before a search runs.
const DEBOUNCE: Duration = Duration::from_millis(220);
/// Conversations listed by name at most, with and without text typed.
const CONVERSATIONS: usize = 12;
const CONVERSATIONS_MATCHED: usize = 5;
const PEOPLE: usize = 4;
const SERVERS: usize = 4;
const OFFERS: usize = 8;

const MAC: bool = cfg!(target_os = "macos");

pub fn init(cx: &mut App) {
    cx.bind_keys([KeyBinding::new(
        "tab",
        AcceptSuggestion,
        Some(SUGGESTION_KEYS),
    )]);
    registry::register(cx, |r| {
        r.panes.add(Pane::new(PaneKind::Search, |ctx, window, cx| {
            page_view(&ctx.session, window, cx).into_any_element()
        }));
        r.header_buttons.add(
            Slot::new("search-conversation", |ctx: &HeaderCtx, _, cx| {
                let conversation = ctx.route.conversation()?.to_string();
                let session = ctx.session.clone();
                let theme = theme::get(cx);
                Some(
                    Button::new("search-conversation")
                        .ghost()
                        .small()
                        .rounded(px(16.))
                        .size(px(32.))
                        .bg(theme.raised.opacity(0.45))
                        .border_1()
                        .border_color(theme.border.opacity(0.55))
                        .icon(IconName::Search)
                        .accessibility_label(t!("search-in-conversation"))
                        .tooltip(t!("search-in-conversation"))
                        .on_click(move |_, _, cx| {
                            open_conversation(&session, &conversation, cx);
                        })
                        .into_any_element(),
                )
            })
            .order(90),
        );
        r.keys.add(
            KeyDefault::new(
                "search-messages",
                t!("search-key-page"),
                if MAC { "cmd-shift-f" } else { "ctrl-shift-f" },
                Some("Root"),
                OpenSearch,
            )
            .order(5),
        );
        r.keys.add(
            KeyDefault::new(
                "search-conversation",
                t!("search-key-conversation"),
                // Ctrl+F alone is the text field's own find, so it never reaches
                // here while typing.
                if MAC { "cmd-alt-f" } else { "ctrl-alt-f" },
                Some("Root"),
                SearchConversation,
            )
            .order(6),
        );
    });
    cx.on_action(|_: &OpenSearch, cx| {
        if let Some(session) = layout::session(cx) {
            open(&session, None, cx);
        }
    });
    cx.on_action(|_: &SearchConversation, cx| {
        if let Some(session) = layout::session(cx)
            && let Some(Route::Conversation(id)) = session.read(cx).route()
        {
            open_conversation(&session, &id, cx);
        }
    });
}

/// Open the search page, with `query` typed in when given (the switcher's
/// "search messages for …").
pub fn open(session: &Entity<Session>, query: Option<&str>, cx: &mut App) {
    if let Some(query) = query {
        cx.set_global(PendingSearch(Some(SearchTarget::Text(query.to_string()))));
    }
    if let Some(PageGlobal(view)) = cx.try_global::<PageGlobal>().cloned()
        && view.read(cx).session == *session
    {
        view.update(cx, |page, cx| {
            page.focus = true;
            cx.notify();
        });
    }
    session.update(cx, |s, cx| s.navigate(Route::Search, cx));
}

fn open_conversation(session: &Entity<Session>, id: &str, cx: &mut App) {
    cx.set_global(PendingSearch(Some(SearchTarget::Conversation(
        id.to_string(),
    ))));
    open(session, None, cx);
}

// ---- What a search finds and how it is running ----

#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    /// Not enough to search yet.
    Idle,
    Searching,
    Failed(String),
    Done,
}

#[derive(Clone, Debug, PartialEq)]
pub enum More {
    Idle,
    Loading,
    Failed(String),
}

/// One running search: what is typed and chosen, and what the server found.
pub struct SearchModel {
    session: Entity<Session>,
    pub filters: Filters,
    /// What was asked for; server history applies to one conversation only.
    pub source: Source,
    /// The text typed so far, without a filter token being written.
    pending: String,
    /// The text the results are for.
    pub text: String,
    pub results: Vec<c::SearchResult>,
    pub cursor: Option<String>,
    pub coverage: Option<Coverage>,
    pub bounded: bool,
    pub status: Status,
    pub more: More,
    generation: u64,
    request: Option<tokio::task::AbortHandle>,
}

impl SearchModel {
    fn new(session: Entity<Session>) -> Self {
        Self {
            session,
            filters: Filters::default(),
            source: Source::Retained,
            pending: String::new(),
            text: String::new(),
            results: Vec::new(),
            cursor: None,
            coverage: None,
            bounded: false,
            status: Status::Idle,
            more: More::Idle,
            generation: 0,
            request: None,
        }
    }

    /// Whether what is typed and chosen is a search.
    pub fn searches(&self) -> bool {
        sf::can_search(&self.pending, &self.filters)
    }

    /// Whether the results are not yet for what is typed.
    pub fn searching(&self) -> bool {
        self.pending.trim() != self.text.trim() || self.status == Status::Searching
    }

    /// Whether server history can be asked: one conversation is searched.
    pub fn source_allowed(&self) -> bool {
        self.filters.in_.len() == 1
    }

    fn set_text(&mut self, typed: &str, cx: &mut Context<Self>) {
        if typed == self.pending {
            return;
        }
        self.pending = typed.to_string();
        self.generation += 1;
        self.cancel_request();
        let generation = self.generation;
        let runtime = self.session.read(cx).runtime.clone();
        let settle = runtime.spawn(async { tokio::time::sleep(DEBOUNCE).await });
        cx.spawn(async move |this, cx| {
            let _ = settle.await;
            this.update(cx, |m, cx| {
                if m.generation == generation {
                    m.run(cx);
                }
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// Search for exactly this text now (a recent search run again).
    fn set_search(&mut self, text: &str, filters: Filters, cx: &mut Context<Self>) {
        self.pending = text.to_string();
        self.filters = filters;
        self.fix_source();
        self.run(cx);
    }

    fn set_filters(&mut self, filters: Filters, cx: &mut Context<Self>) {
        self.filters = filters;
        self.fix_source();
        self.run(cx);
    }

    fn set_source(&mut self, source: Source, cx: &mut Context<Self>) {
        self.source = source;
        self.fix_source();
        self.run(cx);
    }

    /// Server history goes back off once it no longer applies.
    fn fix_source(&mut self) {
        if !self.source_allowed() {
            self.source = Source::Retained;
        }
    }

    fn run(&mut self, cx: &mut Context<Self>) {
        self.cancel_request();
        self.generation += 1;
        self.text = self.pending.clone();
        self.cursor = None;
        self.more = More::Idle;
        if !self.searches() {
            self.results.clear();
            self.coverage = None;
            self.bounded = false;
            self.status = Status::Idle;
            cx.notify();
            return;
        }
        let Some(client) = self.session.read(cx).client().cloned() else {
            return;
        };
        self.status = Status::Searching;
        let query = Query::new(self.text.clone(), self.filters.clone(), self.source);
        let generation = self.generation;
        let request = client.request(move |api| async move { api.search(&query).await });
        self.request = Some(request.abort_handle());
        cx.spawn(async move |this, cx| {
            let result = request.await;
            this.update(cx, |m, cx| {
                if m.generation != generation {
                    return;
                }
                match result {
                    Ok(Ok(page)) => {
                        m.coverage = sf::coverage(&page);
                        m.bounded = sf::is_bounded(&page);
                        m.cursor = page.next_cursor.filter(|c| !c.is_empty());
                        m.results = page.items;
                        m.status = Status::Done;
                    }
                    Ok(Err(e)) => m.status = Status::Failed(e.to_string()),
                    Err(e) => m.status = Status::Failed(e.to_string()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn retry(&mut self, cx: &mut Context<Self>) {
        self.run(cx);
    }

    fn cancel_request(&mut self) {
        if let Some(request) = self.request.take() {
            request.abort();
        }
    }

    fn load_more(&mut self, cx: &mut Context<Self>) {
        let Some(cursor) = self.cursor.clone() else {
            return;
        };
        if self.more == More::Loading {
            return;
        }
        let Some(client) = self.session.read(cx).client().cloned() else {
            return;
        };
        self.more = More::Loading;
        let mut query = Query::new(self.text.clone(), self.filters.clone(), self.source);
        query.cursor = Some(cursor);
        let generation = self.generation;
        let request = client.request(move |api| async move { api.search(&query).await });
        self.request = Some(request.abort_handle());
        cx.spawn(async move |this, cx| {
            let result = request.await;
            this.update(cx, |m, cx| {
                if m.generation != generation {
                    return;
                }
                match result {
                    Ok(Ok(page)) => {
                        for item in page.items {
                            // A repeated message keeps its last place.
                            m.results.retain(|r| r.message.id != item.message.id);
                            m.results.push(item);
                        }
                        m.cursor = page.next_cursor.filter(|c| !c.is_empty());
                        m.more = More::Idle;
                    }
                    Ok(Err(e)) => m.more = More::Failed(e.to_string()),
                    Err(e) => m.more = More::Failed(e.to_string()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// The count to announce and show once a search settles.
    fn count_text(&self) -> String {
        let n = self.results.len();
        if self.bounded {
            t!("search-bounded-count", count = n)
        } else if self.cursor.is_some() {
            t!("search-message-count-more", count = n)
        } else if n == 1 {
            t!("search-message-count-one").to_string()
        } else {
            t!("search-message-count", count = n)
        }
    }
}

// ---- Names, labels and what a typed filter can be ----

fn conversation_label(c: &c::Conversation) -> String {
    match c.kind {
        ConversationKind::Channel => format!("#{}", c.name),
        _ => format!("@{}", c.name),
    }
}

fn member_name(m: &c::Member) -> String {
    if m.display_name.is_empty() {
        m.nick.clone()
    } else {
        m.display_name.clone()
    }
}

/// Everyone to choose from, by name, without yourself.
fn people(w: &Workspace) -> Vec<&c::Member> {
    let mut people: Vec<&c::Member> = w
        .members
        .values()
        .filter(|m| m.id != w.user.id && !w.networks.iter().any(|n| w.is_self(&m.id, &n.id)))
        .collect();
    people.sort_by_key(|m| member_name(m).to_lowercase());
    people.dedup_by_key(|m| m.id.clone());
    people
}

fn kind_label(kind: Has) -> &'static str {
    match kind {
        Has::Link => t!("search-kind-link"),
        Has::Image => t!("search-kind-image"),
        Has::Gif => t!("search-kind-gif"),
        Has::Video => t!("search-kind-video"),
        Has::File => t!("search-kind-file"),
    }
}

fn author_label(author: Author) -> &'static str {
    match author {
        Author::User => t!("search-person"),
        Author::Bot => t!("search-bot"),
    }
}

/// What a chip shows after its key.
fn chip_label(w: &Workspace, chip: &Chip) -> String {
    match chip {
        Chip::From(id) | Chip::Mentions(id) => w
            .members
            .get(id)
            .or_else(|| (id == &w.user.id).then_some(&w.user))
            .map(member_name)
            .unwrap_or_else(|| t!("search-someone").to_string()),
        Chip::In(id) => w
            .conversation(id)
            .map(conversation_label)
            .unwrap_or_else(|| t!("search-closed-conversation").to_string()),
        Chip::Has(kind) => kind_label(*kind).to_string(),
        Chip::Author(author) => author_label(*author).to_string(),
        Chip::Before(day) | Chip::After(day) | Chip::On(day) => day.clone(),
    }
}

fn today() -> NaiveDate {
    Local::now().date_naive()
}

/// A day typed as a word or as `YYYY-MM-DD`.
fn typed_day(value: &str, today: NaiveDate) -> Option<String> {
    let value = value.trim().to_lowercase();
    if sf::is_day(&value) {
        return Some(value);
    }
    let named = [
        (t!("search-today").to_lowercase(), 0),
        (t!("search-yesterday").to_lowercase(), 1),
    ];
    named
        .iter()
        .find(|(word, _)| *word == value)
        .map(|(_, back)| sf::days_before(today, *back))
}

fn date_chip(key: Key, day: String) -> Option<Chip> {
    match key {
        Key::Before => Some(Chip::Before(day)),
        Key::After => Some(Chip::After(day)),
        Key::On => Some(Chip::On(day)),
        _ => None,
    }
}

/// The chip a finished `key:value` token names exactly, if any.
fn resolve(w: &Workspace, key: Key, value: &str, today: NaiveDate) -> Option<Chip> {
    let bare = value.trim_start_matches(['@', '#']);
    match key {
        Key::From | Key::Mentions => {
            let m = people(w)
                .into_iter()
                .chain((key == Key::From).then_some(&w.user))
                .find(|m| m.nick.eq_ignore_ascii_case(bare))?;
            Some(if key == Key::From {
                Chip::From(m.id.clone())
            } else {
                Chip::Mentions(m.id.clone())
            })
        }
        Key::In => w
            .conversations
            .iter()
            .find(|c| c.name.eq_ignore_ascii_case(bare))
            .map(|c| Chip::In(c.id.clone())),
        Key::Has => Has::parse(bare)
            .or_else(|| {
                Has::ALL
                    .into_iter()
                    .find(|h| kind_label(*h).eq_ignore_ascii_case(bare))
            })
            .map(Chip::Has),
        Key::Author => Author::parse(bare).map(Chip::Author),
        Key::Before | Key::After | Key::On => date_chip(key, typed_day(value, today)?),
    }
}

/// A choice offered for the token being typed.
#[derive(Clone, Debug)]
struct Offer {
    chip: Chip,
    label: String,
    person: Option<c::Member>,
    secondary: Option<String>,
}

/// What the token being typed could be, best first.
fn offers(w: &Workspace, key: Key, value: &str, today: NaiveDate) -> Vec<Offer> {
    let needle = value.trim_start_matches(['@', '#']).to_lowercase();
    let matches = |name: &str| needle.is_empty() || name.to_lowercase().contains(&needle);
    let mut offers: Vec<Offer> = match key {
        Key::From | Key::Mentions => (key == Key::From)
            .then_some(&w.user)
            .into_iter()
            .chain(people(w))
            .filter(|m| matches(&m.nick) || matches(&member_name(m)))
            .map(|m| Offer {
                chip: if key == Key::From {
                    Chip::From(m.id.clone())
                } else {
                    Chip::Mentions(m.id.clone())
                },
                label: if m.id == w.user.id {
                    t!("search-yourself", name = member_name(m))
                } else {
                    member_name(m)
                },
                secondary: Some(format!("@{}", m.nick)),
                person: Some(m.clone()),
            })
            .collect(),
        Key::In => w
            .conversations
            .iter()
            .filter(|c| matches(&c.name))
            .map(|c| Offer {
                chip: Chip::In(c.id.clone()),
                label: conversation_label(c),
                person: None,
                secondary: w.network(&c.network_id).map(|n| n.name.clone()),
            })
            .collect(),
        Key::Has => Has::ALL
            .into_iter()
            .filter(|h| matches(h.as_str()) || matches(kind_label(*h)))
            .map(|h| Offer {
                chip: Chip::Has(h),
                label: kind_label(h).to_string(),
                person: None,
                secondary: None,
            })
            .collect(),
        Key::Author => [Author::User, Author::Bot]
            .into_iter()
            .filter(|a| matches(a.as_str()) || matches(author_label(*a)))
            .map(|a| Offer {
                chip: Chip::Author(a),
                label: author_label(a).to_string(),
                person: None,
                secondary: None,
            })
            .collect(),
        Key::Before | Key::After | Key::On => {
            let mut days: Vec<(String, String)> = query_field::date_presets()
                .into_iter()
                .filter(|(name, _)| matches(name))
                .map(|(name, back)| {
                    let day = sf::days_before(today, back);
                    (name.to_string(), day)
                })
                .collect();
            if sf::is_day(value) {
                days.insert(0, (value.to_string(), value.to_string()));
            }
            days.into_iter()
                .filter_map(|(label, day)| {
                    Some(Offer {
                        chip: date_chip(key, day.clone())?,
                        secondary: (label != day).then_some(day),
                        label,
                        person: None,
                    })
                })
                .collect()
        }
    };
    if let Some(exact) = resolve(w, key, value, today)
        && let Some(index) = offers.iter().position(|offer| offer.chip == exact)
    {
        offers[..=index].rotate_right(1);
    }
    offers
}

fn coverage_text(coverage: Coverage) -> &'static str {
    use c::SearchSourceReason as R;
    match coverage {
        Coverage::Retained => t!("search-coverage-retained"),
        Coverage::Upstream => t!("search-coverage-upstream"),
        Coverage::Fallback(reason) => match reason {
            R::UnsupportedMinusScope => t!("search-fallback-unsupported-scope"),
            R::UnsupportedMinusFilters => t!("search-fallback-unsupported-filters"),
            R::CapabilityMinusUnavailable => t!("search-fallback-capability-unavailable"),
            R::UpstreamMinusUnavailable => t!("search-fallback-upstream-unavailable"),
            R::Continuation => t!("search-fallback-continuation"),
            R::Unknown => t!("search-coverage-retained"),
        },
    }
}

// ---- Recent searches on this device ----

fn account(session: &Session) -> Option<String> {
    session.client().map(|c| c.read().user.id.clone())
}

fn load_recent(session: &Session) -> Vec<RecentSearch> {
    account(session)
        .map(|account| sf::load_recent(&session.config, &account))
        .unwrap_or_default()
}

fn save_recent(session: &Session, list: &[RecentSearch]) {
    let Some(account) = account(session) else {
        return;
    };
    if let Err(e) = sf::save_recent(&session.config, &account, list) {
        log::warn!("save recent searches: {e}");
    }
}

// ---- Shared pieces of view ----

/// Open a result in the timeline, or say why it cannot be.
fn open_result(
    session: &Entity<Session>,
    result: &c::SearchResult,
    window: &mut Window,
    cx: &mut App,
) {
    if !sf::is_jumpable(result) {
        return ui::toast::info(window, cx, t!("search-result-not-jumpable"));
    }
    crate::timeline::reveal(
        session,
        &result.conversation.id,
        &result.message.id,
        window,
        cx,
    );
}

fn list_label(text: impl Into<SharedString>, theme: &Theme) -> Div {
    div()
        .px_3()
        .pt_3()
        .pb_1()
        .text_xs()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(theme.muted)
        .child(text.into())
}

/// A small text button.
fn text_button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    theme: &Theme,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let label: SharedString = label.into();
    div()
        .id(id)
        .role(Role::Button)
        .aria_label(label.clone())
        .px_2()
        .py_1()
        .rounded_md()
        .cursor_pointer()
        .text_sm()
        .bg(theme.input)
        .text_color(theme.text)
        .hover({
            let hover = theme.hover;
            move |d| d.bg(hover)
        })
        .child(label)
        .on_click(on_click)
}

fn segment(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    on: bool,
    theme: &Theme,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let label: SharedString = label.into();
    div()
        .id(id)
        .role(Role::RadioButton)
        .aria_label(label.clone())
        .aria_toggled(if on { Toggled::True } else { Toggled::False })
        .px_2p5()
        .py_1p5()
        .rounded(theme.radius)
        .cursor_pointer()
        .text_xs()
        .when(on, |d| d.bg(theme.raised).text_color(theme.text))
        .when(!on, |d| {
            let hover = theme.hover;
            d.text_color(theme.muted).hover(move |d| d.bg(hover))
        })
        .child(label)
        .on_click(on_click)
}

fn segments(label: &'static str, theme: &Theme) -> Stateful<Div> {
    div()
        .id(ElementId::Name(label.into()))
        .role(Role::RadioGroup)
        .aria_label(label)
        .flex()
        .flex_wrap()
        .gap_0p5()
        .p_0p5()
        .bg(theme.sidebar)
        .border_1()
        .border_color(theme.border)
        .rounded(theme.radius)
}

/// The server history switch, disabled with the reason when it cannot apply.
fn source_switch(model: &Entity<SearchModel>, theme: &Theme, cx: &App) -> Div {
    let m = model.read(cx);
    let allowed = m.source_allowed();
    let on = allowed && m.source == Source::Upstream;
    let model = model.clone();
    div()
        .px_3()
        .py_1()
        .flex()
        .flex_wrap()
        .items_center()
        .gap_2()
        .text_xs()
        .text_color(theme.muted)
        .when(!allowed, |d| {
            d.child(t!("search-server-history-needs-scope"))
        })
        .child(
            div()
                .id("search-server-history")
                .test_support()
                .role(Role::Switch)
                .reachable(theme)
                .aria_label(t!("search-server-history"))
                .aria_toggled(if on { Toggled::True } else { Toggled::False })
                .flex()
                .items_center()
                .gap_1p5()
                .when(allowed, |d| d.cursor_pointer())
                .when(!allowed, |d| d.opacity(0.5))
                .child(
                    div()
                        .w(px(26.))
                        .h(px(14.))
                        .rounded_full()
                        .bg(if on { theme.accent } else { theme.selected })
                        .flex()
                        .when(on, |d| d.justify_end())
                        .child(div().size(px(14.)).rounded_full().bg(theme.text)),
                )
                .child(t!("search-server-history"))
                .when(allowed, |d| {
                    d.on_click(move |_, _, cx| {
                        model.update(cx, |m, cx| {
                            let next = if m.source == Source::Upstream {
                                Source::Retained
                            } else {
                                Source::Upstream
                            };
                            m.set_source(next, cx)
                        })
                    })
                }),
        )
}

/// The removable chips for what is chosen.
fn chips_row(
    prefix: &'static str,
    model: &Entity<SearchModel>,
    theme: &Theme,
    cx: &App,
) -> Option<Div> {
    let session = model.read(cx).session.read(cx);
    let client = session.client()?.clone();
    let w = client.read();
    let chips = model.read(cx).filters.chips();
    if chips.is_empty() {
        return None;
    }
    let mut row = div().flex().flex_wrap().gap_1();
    for (i, chip) in chips.into_iter().enumerate() {
        let value = chip_label(&w, &chip);
        let key = chip.key().as_str();
        let label = t!("search-remove-filter", filter = key, value = value.clone());
        let model = model.clone();
        row = row.child(query_field::chip(
            format!("{prefix}-chip-{i}"),
            label,
            key,
            value,
            theme,
            move |_, _, cx| {
                model.update(cx, |m, cx| {
                    let next = m.filters.without(&chip);
                    m.set_filters(next, cx)
                })
            },
        ));
    }
    Some(row)
}

/// One message result, as the web's `MessageResult.vue`: the sender's
/// picture, their coloured name, where and when, then the text.
#[allow(clippy::too_many_arguments)]
fn result_row(
    id: String,
    w: &Workspace,
    result: &c::SearchResult,
    selected: bool,
    server: Option<&url::Url>,
    colored_names: bool,
    theme: &Theme,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let network = &result.conversation.network_id;
    let sender = &result.message.sender_id;
    let known = w.is_self(sender, network) || w.member(sender, network).is_some();
    let member = known.then(|| w.display_name(sender, network));
    let author = sf::result_author(result, member).unwrap_or_else(|| t!("search-someone").into());
    let place = w
        .conversation(&result.conversation.id)
        .map(conversation_label)
        .unwrap_or_else(|| conversation_label(&result.conversation));
    let (time, _) = inbox::day_and_time(&result.message.created_at, false);
    let text = format::strip(&result.message.text);
    let jumpable = sf::is_jumpable(result);
    let mut label = t!(
        "search-result",
        author = author.clone(),
        place = place.clone(),
        time = time.clone(),
        text = text.clone()
    );
    if !jumpable {
        label = format!("{label}. {}", t!("search-result-not-jumpable"));
    }
    let mut who = inbox::sender(
        w,
        known.then_some(sender.as_str()),
        network,
        server,
        colored_names,
        theme,
    );
    who.name = author;
    let mut location = vec![div().truncate().child(place).into_any_element()];
    if !jumpable {
        location.push(
            div()
                .px_1p5()
                .rounded_sm()
                .bg(theme.input)
                .text_color(theme.muted)
                .child(t!("search-result-remote-badge"))
                .into_any_element(),
        );
    }
    div()
        .id(ElementId::Name(id.clone().into()))
        .role(Role::ListBoxOption)
        .aria_label(SharedString::from(label))
        .aria_selected(selected)
        .mx_2()
        .rounded_md()
        .when(jumpable, |d| d.cursor_pointer())
        .when(selected, |d| d.bg(theme.selected))
        .when(!selected, |d| {
            let hover = theme.row_hover;
            d.hover(move |d| d.bg(hover))
        })
        .child(inbox::result_line(
            &id,
            &who,
            location,
            time,
            div().text_color(theme.body).child(text).into_any_element(),
            theme,
        ))
        .on_click(on_click)
}

/// The messages part: status, results and "load older".
#[allow(clippy::too_many_arguments)]
fn messages_part(
    prefix: &'static str,
    model: &Entity<SearchModel>,
    session: &Entity<Session>,
    selected: Option<usize>,
    scroll: &ResultScroll,
    offset: usize,
    empty_text: String,
    theme: &Theme,
    cx: &App,
) -> Div {
    let m = model.read(cx);
    let mut part = div().flex().flex_col().gap_0p5();
    if m.searching() {
        return part.child(
            div()
                .id(ElementId::Name(format!("{prefix}-searching").into()))
                .test_support()
                .role(Role::Status)
                .aria_label(t!("search-searching"))
                .px_3()
                .py_2()
                .text_sm()
                .text_color(theme.muted)
                .child(t!("search-searching")),
        );
    }
    if let Status::Failed(error) = &m.status {
        let model = model.clone();
        let text = t!("search-failed", error = error.clone());
        return part.child(
            div()
                .id(ElementId::Name(format!("{prefix}-failed").into()))
                .test_support()
                .role(Role::Alert)
                .aria_label(SharedString::from(text.clone()))
                .px_3()
                .py_2()
                .flex()
                .flex_col()
                .gap_2()
                .text_sm()
                .text_color(theme.danger)
                .child(text)
                .child(
                    text_button(
                        ElementId::Name(format!("{prefix}-retry").into()),
                        t!("search-try-again"),
                        theme,
                        move |_, _, cx| model.update(cx, |m, cx| m.retry(cx)),
                    )
                    .test_support()
                    .reachable(theme),
                ),
        );
    }
    if m.status != Status::Done {
        return part;
    }
    let count = if m.results.is_empty() {
        empty_text
    } else {
        m.count_text()
    };
    part = part.child(
        div()
            .id(ElementId::Name(format!("{prefix}-count").into()))
            .test_support()
            .role(Role::Status)
            .aria_label(SharedString::from(count.clone()))
            .px_3()
            .py_1()
            .text_xs()
            .text_color(theme.muted)
            .child(count),
    );
    if let Some(coverage) = m.coverage {
        let note = coverage_text(coverage);
        part = part.child(
            div()
                .id(ElementId::Name(format!("{prefix}-coverage").into()))
                .test_support()
                .role(Role::Note)
                .aria_label(note)
                .px_3()
                .pb_1()
                .text_xs()
                .text_color(theme.muted)
                .child(note),
        );
    }
    let Some(client) = session.read(cx).client().cloned() else {
        return part;
    };
    let look = crate::message::layout::settings(cx);
    let server = inbox::server(session, cx);
    let w = client.read();
    for (i, result) in m.results.iter().enumerate() {
        let session = session.clone();
        let model = model.clone();
        let r = result.clone();
        let selected = selected == Some(offset + i);
        part = part.child(
            result_row(
                format!("{prefix}-result-{i}"),
                &w,
                result,
                selected,
                server.as_ref(),
                look.colored_names,
                theme,
                move |_, window, cx| {
                    model.update(cx, |_, cx| cx.emit(Opened));
                    open_result(&session, &r, window, cx)
                },
            )
            .when_some(scroll.reveal_row(selected), |row, reveal| {
                row.relative().child(reveal)
            })
            .test_support(),
        );
    }
    if let More::Failed(error) = &m.more {
        let text = t!("search-load-more-failed", error = error.clone());
        part = part.child(
            div()
                .id(ElementId::Name(format!("{prefix}-more-failed").into()))
                .test_support()
                .role(Role::Alert)
                .aria_label(SharedString::from(text.clone()))
                .px_3()
                .text_sm()
                .text_color(theme.danger)
                .child(text),
        );
    }
    if m.cursor.is_some() {
        let loading = m.more == More::Loading;
        let model = model.clone();
        part = part.child(
            div().px_3().py_2().child(
                text_button(
                    ElementId::Name(format!("{prefix}-load-older").into()),
                    if loading {
                        t!("search-loading-older")
                    } else {
                        t!("search-load-older")
                    },
                    theme,
                    move |_, _, cx| model.update(cx, |m, cx| m.load_more(cx)),
                )
                .test_support()
                .reachable(theme),
            ),
        );
    }
    part
}

/// A result was opened (the page remembers the search).
pub struct Opened;
impl EventEmitter<Opened> for SearchModel {}

#[derive(Default)]
struct FilterSelection {
    selection: Selection,
}

impl FilterSelection {
    fn choices(
        &self,
        input: &Entity<InputState>,
        model: &Entity<SearchModel>,
        cx: &App,
    ) -> Option<(sf::Token, Vec<Offer>)> {
        if self.selection.dismissed {
            return None;
        }
        let token = sf::active_token(&input.read(cx).value())?;
        let model = model.read(cx);
        let client = model.session.read(cx).client()?;
        let chosen = model.filters.chips();
        let choices = offers(&client.read(), token.key, &token.value, today())
            .into_iter()
            .filter(|offer| !chosen.contains(&offer.chip))
            .take(OFFERS)
            .collect();
        Some((token, choices))
    }

    fn step(&mut self, count: usize, backward: bool) {
        self.selection.step(count, backward);
    }

    fn accept(
        &self,
        input: &Entity<InputState>,
        model: &Entity<SearchModel>,
        window: &mut Window,
        cx: &mut App,
    ) -> bool {
        let Some((_, choices)) = self.choices(input, model, cx) else {
            return false;
        };
        let Some(offer) = choices.get(self.selection.index(choices.len())) else {
            return true;
        };
        apply_offer(input, model, offer.chip.clone(), window, cx);
        true
    }
}

/// Offers float below the field without moving the search results.
#[allow(clippy::too_many_arguments)]
fn offers_row(
    prefix: &'static str,
    input: &Entity<InputState>,
    model: &Entity<SearchModel>,
    choices: Option<(sf::Token, Vec<Offer>)>,
    selection: &FilterSelection,
    calendar: &Entity<DatePickerState>,
    theme: &Theme,
    cx: &App,
) -> Option<Deferred> {
    let (token, offers) = choices?;
    let server = model
        .read(cx)
        .session
        .read(cx)
        .server()
        .map(|server| server.url());
    let heading = match token.key {
        Key::From => t!("search-from-label"),
        Key::Mentions => t!("search-mentions-label"),
        Key::In => t!("search-in-label"),
        Key::Has => t!("search-has-label"),
        Key::On => t!("search-on-label"),
        Key::Before => t!("search-suggest-before"),
        Key::After => t!("search-suggest-after"),
        Key::Author => t!("search-filters-author-type"),
    };
    let count = offers.len();
    let rows = offers
        .into_iter()
        .enumerate()
        .map(|(i, offer)| {
            let (input, model) = (input.clone(), model.clone());
            let chip = offer.chip.clone();
            let leading = offer
                .person
                .as_ref()
                .map(|person| {
                    ui::avatar::person(
                        ui::avatar::Person {
                            member: person,
                            name: &offer.label,
                            server,
                            presence: false,
                            key: &format!("{prefix}-offer-{i}"),
                        },
                        px(24.),
                        theme,
                    )
                    .into_any_element()
                })
                .or_else(|| {
                    let icon = match offer.chip {
                        Chip::In(_) if offer.label.starts_with('@') => LucideIcon::MessageCircle,
                        Chip::In(_) => LucideIcon::Hash,
                        Chip::Has(Has::Link) => LucideIcon::Link,
                        Chip::Has(Has::Image) => LucideIcon::Image,
                        Chip::Has(Has::Gif) => LucideIcon::Clapperboard,
                        Chip::Has(Has::Video) => LucideIcon::Video,
                        Chip::Has(Has::File) => {
                            return Some(
                                ui::icon::icon(IconName::File, theme.muted).into_any_element(),
                            );
                        }
                        Chip::On(_) | Chip::Before(_) | Chip::After(_) => {
                            return Some(
                                ui::icon::icon(IconName::Calendar, theme.muted).into_any_element(),
                            );
                        }
                        _ => return None,
                    };
                    Some(ui::icon::icon(icon, theme.muted).into_any_element())
                });
            query_field::Suggestion {
                title: offer.label,
                detail: offer.secondary,
                leading,
            }
            .render(
                prefix,
                i,
                count,
                &selection.selection,
                theme,
                move |_, window, cx| apply_offer(&input, &model, chip.clone(), window, cx),
            )
            .into_any_element()
        })
        .collect();
    let footer = matches!(token.key, Key::On | Key::Before | Key::After)
        .then(|| query_field::calendar(calendar, theme).into_any_element());
    Some(query_field::suggestions(
        prefix,
        heading,
        rows,
        t!("search-no-suggestion", query = token.value),
        footer,
        &selection.selection,
        theme,
    ))
}

fn pick_search_day(
    day: NaiveDate,
    input: &Entity<InputState>,
    model: &Entity<SearchModel>,
    window: &mut Window,
    cx: &mut App,
) {
    if let Some(token) = sf::active_token(&input.read(cx).value())
        && let Some(chip) = date_chip(token.key, day.format("%Y-%m-%d").to_string())
    {
        apply_offer(input, model, chip, window, cx);
    }
}

/// Start a filter at the end of the field, replacing any unfinished filter.
fn start_filter(input: &Entity<InputState>, key: Key, window: &mut Window, cx: &mut App) {
    let raw = input.read(cx).value().to_string();
    let raw = match sf::active_token(&raw) {
        Some(token) => sf::without_token(&raw, &token).to_string(),
        None => raw,
    };
    let space = if raw.is_empty() || raw.ends_with(' ') {
        ""
    } else {
        " "
    };
    let next = format!("{raw}{space}{}:", key.as_str());
    input.update(cx, |i, cx| i.set_value(next, window, cx));
    input.focus_handle(cx).focus(window, cx);
}

/// Turn the token being typed into `chip`.
fn apply_offer(
    input: &Entity<InputState>,
    model: &Entity<SearchModel>,
    chip: Chip,
    window: &mut Window,
    cx: &mut App,
) {
    let raw = input.read(cx).value().to_string();
    let rest = match sf::active_token(&raw).or_else(|| sf::finished_token(&raw)) {
        Some(token) => sf::without_token(&raw, &token).to_string(),
        None => raw,
    };
    model.update(cx, |m, cx| {
        let next = m.filters.with(chip);
        m.set_filters(next, cx)
    });
    input.update(cx, |i, cx| i.set_value(rest, window, cx));
    input.focus_handle(cx).focus(window, cx);
}

/// Backspace in an empty field removes the last chip. Returns whether it did.
fn remove_last_chip(input: &Entity<InputState>, model: &Entity<SearchModel>, cx: &mut App) -> bool {
    if !input.read(cx).value().is_empty() {
        return false;
    }
    let Some(last) = model.read(cx).filters.chips().pop() else {
        return false;
    };
    model.update(cx, |m, cx| {
        let next = m.filters.without(&last);
        m.set_filters(next, cx)
    });
    true
}

/// What the field changed to: a finished token that names something
/// becomes a chip; the rest is the text to search.
fn field_changed(
    input: &Entity<InputState>,
    model: &Entity<SearchModel>,
    window: &mut Window,
    cx: &mut App,
) {
    let raw = input.read(cx).value().to_string();
    if let Some(token) = sf::finished_token(&raw) {
        let chip = {
            let session = model.read(cx).session.read(cx);
            session
                .client()
                .and_then(|client| resolve(&client.read(), token.key, &token.value, today()))
        };
        if let Some(chip) = chip {
            return apply_offer(input, model, chip, window, cx);
        }
    }
    let typed = sf::typed_text(&raw).to_string();
    model.update(cx, |m, cx| m.set_text(&typed, cx));
}

// ---- The search page ----

enum SearchTarget {
    Text(String),
    Conversation(String),
}

/// A search handed to the page before it shows.
#[derive(Default)]
struct PendingSearch(Option<SearchTarget>);
impl Global for PendingSearch {}

/// The one page view, kept across visits so a search stays.
#[derive(Clone)]
struct PageGlobal(Entity<SearchPage>);
impl Global for PageGlobal {}

fn page_view(session: &Entity<Session>, window: &mut Window, cx: &mut App) -> Entity<SearchPage> {
    let view = match cx.try_global::<PageGlobal>() {
        Some(PageGlobal(view)) if view.read(cx).session == *session => view.clone(),
        _ => {
            let view = cx.new(|cx| SearchPage::new(session.clone(), window, cx));
            cx.set_global(PageGlobal(view.clone()));
            view
        }
    };
    if let Some(target) = cx.default_global::<PendingSearch>().0.take() {
        view.update(cx, |v, cx| {
            let (query, filters) = match target {
                SearchTarget::Text(query) => (query, v.model.read(cx).filters.clone()),
                SearchTarget::Conversation(id) => {
                    v.model.update(cx, |m, _| m.source = Source::Retained);
                    (String::new(), Filters::default().scoped(&id))
                }
            };
            v.selected = 0;
            v.result_scroll.reset();
            v.filter_selection = FilterSelection::default();
            v.error = None;
            v.input
                .update(cx, |i, cx| i.set_value(query.clone(), window, cx));
            v.focus = true;
            v.model.update(cx, |m, cx| {
                m.set_search(sf::typed_text(&query), filters, cx)
            });
        });
    }
    view
}

/// One thing the page's list can open.
#[derive(Clone, Debug, PartialEq)]
enum Item {
    Conversation(String),
    Server(String),
    Person(String),
    Nickname(String),
    Message(usize),
}

pub struct SearchPage {
    session: Entity<Session>,
    model: Entity<SearchModel>,
    input: Entity<InputState>,
    selected: usize,
    result_scroll: ResultScroll,
    filter_selection: FilterSelection,
    query_calendar: Entity<DatePickerState>,
    recent: Vec<RecentSearch>,
    focus: bool,
    error: Option<String>,
    _subscriptions: Vec<Subscription>,
}

impl SearchPage {
    fn new(session: Entity<Session>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let model = cx.new(|_| SearchModel::new(session.clone()));
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(t!("search-placeholder")));
        let query_calendar = query_field::calendar_state(window, cx);
        let date = cx.subscribe_in(
            &query_calendar,
            window,
            |this: &mut Self, _, event: &DatePickerEvent, window, cx| {
                let DatePickerEvent::Change(date) = event;
                if let Some(day) = date.start() {
                    pick_search_day(day.date(), &this.input, &this.model, window, cx);
                }
            },
        );
        let field = cx.subscribe_in(
            &input,
            window,
            |this: &mut Self, input, event: &InputEvent, window, cx| match event {
                InputEvent::Change => {
                    field_changed(input, &this.model, window, cx);
                    this.selected = 0;
                    this.result_scroll.reset();
                    this.filter_selection = FilterSelection::default();
                    cx.notify();
                }
                InputEvent::PressEnter { .. } => this.enter(window, cx),
                _ => {}
            },
        );
        let changed = cx.observe(&model, |this: &mut Self, _, cx| {
            this.selected = this.selected.min(this.items(cx).len().saturating_sub(1));
            cx.notify();
        });
        let opened = cx.subscribe(&model, |this: &mut Self, _, _: &Opened, cx| {
            this.remember(cx)
        });
        let workspace = cx.subscribe(&session, |_, _, event: &SessionEvent, cx| {
            if matches!(event, SessionEvent::Changed(_)) {
                cx.notify();
            }
        });
        let recent = load_recent(session.read(cx));
        Self {
            session,
            model,
            input,
            selected: 0,
            result_scroll: ResultScroll::default(),
            filter_selection: FilterSelection::default(),
            query_calendar,
            recent,
            focus: true,
            error: None,
            _subscriptions: vec![field, changed, opened, workspace, date],
        }
    }

    fn typed(&self, cx: &App) -> String {
        sf::typed_text(&self.input.read(cx).value()).to_string()
    }

    fn needle(&self, cx: &App) -> String {
        self.typed(cx)
            .to_lowercase()
            .trim_start_matches(['#', '@'])
            .to_string()
    }

    /// Conversations, server windows and people by name (only while no
    /// filter is chosen), then messages.
    fn items(&self, cx: &App) -> Vec<Item> {
        let mut items = Vec::new();
        let m = self.model.read(cx);
        if m.filters.is_empty()
            && let Some(client) = self.session.read(cx).client()
        {
            let w = client.read();
            let needle = self.needle(cx);
            items.extend(
                conversations(&w, &needle, &switcher::recent_conversations(cx))
                    .into_iter()
                    .map(Item::Conversation),
            );
            if !needle.is_empty() {
                let window_name = t!("search-server-window").to_lowercase();
                items.extend(
                    w.networks
                        .iter()
                        .filter(|n| {
                            format!("{window_name} {}", n.name.to_lowercase()).contains(&needle)
                        })
                        .take(SERVERS)
                        .map(|n| Item::Server(n.id.clone())),
                );
                let found: Vec<&c::Member> = people(&w)
                    .into_iter()
                    .filter(|p| {
                        p.nick.to_lowercase().contains(&needle)
                            || member_name(p).to_lowercase().contains(&needle)
                    })
                    .take(PEOPLE)
                    .collect();
                let typed = self.typed(cx);
                let typed = typed.trim_start_matches('@');
                let known = found.iter().any(|p| p.nick.eq_ignore_ascii_case(typed))
                    || people(&w)
                        .iter()
                        .any(|p| p.nick.eq_ignore_ascii_case(typed));
                items.extend(found.into_iter().map(|p| Item::Person(p.id.clone())));
                let network = self.session.read(cx).network.clone();
                let limit = network
                    .as_deref()
                    .and_then(|n| w.network(n))
                    .map_or(30, |n| cmd::run::nickname_limit(&n.isupport));
                if !known
                    && network.is_some()
                    && !typed.is_empty()
                    && cmd::run::nickname_error(typed, limit).is_none()
                {
                    items.push(Item::Nickname(typed.to_string()));
                }
            }
        }
        if m.searches() && !m.searching() && m.status == Status::Done {
            items.extend((0..m.results.len()).map(Item::Message));
        }
        items
    }

    fn enter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self
            .filter_selection
            .accept(&self.input, &self.model, window, cx)
        {
            return;
        }
        let items = self.items(cx);
        if let Some(item) = items.get(self.selected).cloned() {
            self.choose(item, window, cx);
        }
    }

    fn choose(&mut self, item: Item, window: &mut Window, cx: &mut Context<Self>) {
        let session = self.session.clone();
        match item {
            Item::Conversation(id) => session.update(cx, |s, cx| s.select_conversation(&id, cx)),
            Item::Server(id) => session.update(cx, |s, cx| s.navigate(Route::ServerWindow(id), cx)),
            Item::Person(id) => self.direct(Some(id), None, window, cx),
            Item::Nickname(nick) => self.direct(None, Some(nick), window, cx),
            Item::Message(i) => {
                let result = self.model.read(cx).results.get(i).cloned();
                if let Some(result) = result {
                    self.remember(cx);
                    open_result(&session, &result, window, cx);
                }
            }
        }
    }

    /// Open a direct conversation with someone, by member or by nickname.
    fn direct(
        &mut self,
        member: Option<String>,
        nickname: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let session = self.session.read(cx);
        let Some(client) = session.client().cloned() else {
            return;
        };
        let network = member
            .as_deref()
            .and_then(|id| {
                let w = client.read();
                w.conversations
                    .iter()
                    .find(|c| c.member_ids.iter().any(|m| m == id))
                    .map(|c| c.network_id.clone())
            })
            .or_else(|| session.network.clone());
        let Some(network) = network else {
            return;
        };
        let task = cmd::execute(
            &client,
            Plan::Direct {
                network,
                member,
                nickname,
                text: None,
            },
        );
        self.error = None;
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(Ok(done)) => {
                        if let Some(id) = done.open {
                            this.session
                                .update(cx, |s, cx| s.select_conversation(&id, cx));
                        }
                    }
                    Ok(Err(e)) => {
                        this.error = Some(t!("search-open-person-failed", error = e.to_string()))
                    }
                    Err(e) => {
                        this.error = Some(t!("search-open-person-failed", error = e.to_string()))
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn remember(&mut self, cx: &mut Context<Self>) {
        let m = self.model.read(cx);
        if !sf::can_search(&m.text, &m.filters) {
            return;
        }
        let entry = RecentSearch {
            text: m.text.clone(),
            filters: m.filters.clone(),
        };
        self.recent = sf::remember(&self.recent, entry);
        save_recent(self.session.read(cx), &self.recent);
        cx.notify();
    }

    fn forget(&mut self, cx: &mut Context<Self>) {
        self.recent.clear();
        save_recent(self.session.read(cx), &self.recent);
        cx.notify();
    }

    fn recall(&mut self, entry: RecentSearch, window: &mut Window, cx: &mut Context<Self>) {
        self.input
            .update(cx, |i, cx| i.set_value(entry.text.clone(), window, cx));
        self.model
            .update(cx, |m, cx| m.set_search(&entry.text, entry.filters, cx));
        self.input.focus_handle(cx).focus(window, cx);
    }

    fn open_filters(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        open_filters(&self.model, window, cx);
    }

    fn render_list(&self, theme: &Theme, cx: &App) -> Div {
        let items = self.items(cx);
        let selected = (!items.is_empty()).then_some(self.selected);
        let m = self.model.read(cx);
        let mut list = div().flex().flex_col().gap_0p5().pb_4();
        let Some(client) = self.session.read(cx).client().cloned() else {
            return list;
        };
        let w = client.read();
        let needle = self.needle(cx);
        let raw = self.input.read(cx).value().to_string();
        if raw.is_empty() && m.filters.is_empty() && !self.recent.is_empty() {
            list = list.child(self.render_recent(&w, theme, cx));
        }
        let mut index = 0;
        if m.filters.is_empty() {
            let conversations: Vec<&Item> = items
                .iter()
                .filter(|i| matches!(i, Item::Conversation(_) | Item::Server(_)))
                .collect();
            if !conversations.is_empty() || needle.is_empty() {
                list = list.child(list_label(
                    if needle.is_empty() {
                        t!("search-jump-to")
                    } else {
                        t!("search-conversations")
                    },
                    theme,
                ));
            }
            for item in conversations {
                list = list.child(self.name_row(item, index, selected, &w, theme, cx));
                index += 1;
            }
            let found: Vec<&Item> = items
                .iter()
                .filter(|i| matches!(i, Item::Person(_) | Item::Nickname(_)))
                .collect();
            if !found.is_empty() {
                list = list.child(list_label(t!("search-people"), theme));
            }
            for item in found {
                list = list.child(self.name_row(item, index, selected, &w, theme, cx));
                index += 1;
            }
            if index == 0 && !needle.is_empty() && !m.searches() {
                list = list.child(ui::empty::empty(
                    "search-nothing",
                    t!("search-no-conversations", query = self.typed(cx)),
                    None,
                    theme,
                ));
            }
        }
        if m.searches() {
            list = list
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .flex_wrap()
                        .child(list_label(t!("search-messages"), theme))
                        .child(source_switch(&self.model, theme, cx)),
                )
                .child(messages_part(
                    "search",
                    &self.model,
                    &self.session,
                    selected,
                    &self.result_scroll,
                    index,
                    empty_text(&m.text, &m.filters),
                    theme,
                    cx,
                ));
        }
        if let Some(error) = &self.error {
            list = list.child(
                div()
                    .id("search-error")
                    .test_support()
                    .role(Role::Alert)
                    .aria_label(SharedString::from(error.clone()))
                    .px_3()
                    .py_2()
                    .text_sm()
                    .text_color(theme.danger)
                    .child(error.clone()),
            );
        }
        list
    }

    fn name_row(
        &self,
        item: &Item,
        index: usize,
        selected: Option<usize>,
        w: &Workspace,
        theme: &Theme,
        cx: &App,
    ) -> AnyElement {
        let (icon, title, detail, badge) = match item {
            Item::Conversation(id) => {
                let Some(c) = w.conversation(id) else {
                    return div().into_any_element();
                };
                let network = w
                    .network(&c.network_id)
                    .map(|n| n.name.clone())
                    .unwrap_or_default();
                let kind = if c.kind == ConversationKind::Channel {
                    t!("search-channel")
                } else {
                    t!("search-direct-message")
                };
                let badge = if c.mention_count > 0 {
                    Some(c.mention_count.to_string())
                } else {
                    (c.unread_count > 0).then(|| "•".to_string())
                };
                (
                    (c.kind != ConversationKind::Channel).then_some(IconName::User),
                    conversation_label(c),
                    t!("search-conversation-detail", network = network, kind = kind),
                    badge,
                )
            }
            Item::Server(id) => (
                Some(IconName::SquareTerminal),
                t!("search-server-window").to_string(),
                w.network(id).map(|n| n.name.clone()).unwrap_or_default(),
                None,
            ),
            Item::Person(id) => {
                let Some(p) = w.members.get(id) else {
                    return div().into_any_element();
                };
                (
                    Some(IconName::User),
                    member_name(p),
                    format!("@{}", p.nick),
                    None,
                )
            }
            Item::Nickname(nick) => (Some(IconName::User), nick.clone(), format!("@{nick}"), None),
            Item::Message(_) => return div().into_any_element(),
        };
        let is_selected = selected == Some(index);
        let person = match item {
            Item::Conversation(id) => ircord_client::store::people::direct_partner(w, id),
            Item::Person(id) => w.members.get(id),
            _ => None,
        };
        let picture = person.map(|person| {
            ui::avatar::person(
                ui::avatar::Person {
                    member: person,
                    name: &member_name(person),
                    server: self.session.read(cx).server().map(|server| server.url()),
                    presence: false,
                    key: &format!("search-item-{index}"),
                },
                px(28.),
                theme,
            )
        });
        let channel_icon = icon.is_none();
        let label = match item {
            Item::Person(_) | Item::Nickname(_) => {
                t!("search-message-person", name = title.clone())
            }
            _ => format!("{title}, {detail}"),
        };
        let item = item.clone();
        let view = PageGlobalHandle::get(cx);
        div()
            .id(ElementId::Name(format!("search-item-{index}").into()))
            .test_support()
            .role(Role::ListBoxOption)
            .aria_label(SharedString::from(label))
            .aria_selected(is_selected)
            .mx_2()
            .px_2()
            .py_1()
            .rounded_md()
            .flex()
            .items_center()
            .gap_2()
            .cursor_pointer()
            .when(is_selected, |d| d.bg(theme.selected))
            .when(!is_selected, |d| {
                let hover = theme.row_hover;
                d.hover(move |d| d.bg(hover))
            })
            .child(
                div()
                    .w(px(28.))
                    .children(picture)
                    .when(person.is_none(), |d| {
                        d.children(icon.map(|icon| ui::icon::icon(icon, theme.muted)))
                            .when(channel_icon, |d| d.text_color(theme.muted).child("#"))
                    }),
            )
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
                            .text_color(theme.text)
                            .child(title),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_xs()
                            .text_color(theme.faint)
                            .child(detail),
                    ),
            )
            .children(badge.map(|b| {
                div()
                    .px_1p5()
                    .rounded_full()
                    .text_xs()
                    .bg(theme.mention)
                    .text_color(theme.on_accent)
                    .child(b)
            }))
            .when(is_selected, |row| {
                row.child(div().text_xs().text_color(theme.muted).child("↵"))
            })
            .on_click(move |_, window, cx| {
                if let Some(view) = view.clone() {
                    let item = item.clone();
                    view.update(cx, |v, cx| v.choose(item, window, cx));
                }
            })
            .when_some(self.result_scroll.reveal_row(is_selected), |row, reveal| {
                row.relative().child(reveal)
            })
            .into_any_element()
    }

    fn render_recent(&self, w: &Workspace, theme: &Theme, cx: &App) -> Div {
        let view = PageGlobalHandle::get(cx);
        let mut chips = div().px_3().pb_2().flex().flex_wrap().gap_1p5();
        for (i, entry) in self.recent.iter().enumerate() {
            let mut parts: Vec<String> = entry
                .filters
                .chips()
                .iter()
                .map(|chip| format!("{}:{}", chip.key().as_str(), chip_label(w, chip)))
                .collect();
            if !entry.text.is_empty() {
                parts.push(entry.text.clone());
            }
            let shown = parts.join(" ");
            let entry = entry.clone();
            let view = view.clone();
            chips = chips.child(
                text_button(
                    ElementId::Name(format!("search-recent-{i}").into()),
                    shown.clone(),
                    theme,
                    move |_, window, cx| {
                        if let Some(view) = view.clone() {
                            let entry = entry.clone();
                            view.update(cx, |v, cx| v.recall(entry, window, cx));
                        }
                    },
                )
                .test_support()
                .reachable(theme)
                .aria_label(SharedString::from(t!("search-recent", search = shown))),
            );
        }
        let clear = text_button("search-clear-recent", t!("search-clear"), theme, {
            let view = view.clone();
            move |_, _, cx| {
                if let Some(view) = view.clone() {
                    view.update(cx, |v, cx| v.forget(cx));
                }
            }
        })
        .test_support()
        .reachable(theme)
        .aria_label(t!("search-clear-recent"));
        div()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .pr_3()
                    .child(list_label(t!("search-recent-searches"), theme))
                    .child(clear),
            )
            .child(chips)
    }
}

/// The page view from inside its own rows' handlers.
struct PageGlobalHandle;
impl PageGlobalHandle {
    fn get(cx: &App) -> Option<Entity<SearchPage>> {
        cx.try_global::<PageGlobal>().map(|g| g.0.clone())
    }
}

/// Rank by name match, unread urgency, and the last visit, then by name.
fn conversations(w: &Workspace, needle: &str, recent: &[String]) -> Vec<String> {
    let urgency = |c: &c::Conversation| {
        if c.mention_count > 0 {
            0
        } else if c.kind == ConversationKind::Direct && c.unread_count > 0 {
            1
        } else if c.unread_count > 0 {
            2
        } else {
            3
        }
    };
    let score = |c: &c::Conversation| {
        let name = c.name.to_lowercase();
        if needle.is_empty() || name == needle {
            Some(0)
        } else if name.starts_with(needle) {
            Some(1)
        } else if name.contains(needle) {
            Some(2)
        } else {
            None
        }
    };
    let mut found: Vec<_> = w
        .conversations
        .iter()
        .filter_map(|c| {
            let visit = recent
                .iter()
                .position(|id| id == &c.id)
                .unwrap_or(usize::MAX);
            Some((
                score(c)?,
                urgency(c),
                visit,
                c.name.to_lowercase(),
                c.id.clone(),
            ))
        })
        .collect();
    found.sort();
    let limit = if needle.is_empty() {
        CONVERSATIONS
    } else {
        CONVERSATIONS_MATCHED
    };
    found
        .into_iter()
        .take(limit)
        .map(|(_, _, _, _, id)| id)
        .collect()
}

/// What an empty result says.
fn empty_text(text: &str, filters: &Filters) -> String {
    match text.is_empty() {
        false if !filters.is_empty() => {
            t!("search-no-message-query-filtered", query = text)
        }
        false => t!("search-no-message-query", query = text),
        true => t!("search-no-message-filters").to_string(),
    }
}

impl Render for SearchPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme::get(cx).clone();
        if std::mem::take(&mut self.focus) {
            self.input.focus_handle(cx).focus(window, cx);
        }
        let count = self.items(cx).len();
        let choices = self.filter_selection.choices(&self.input, &self.model, cx);
        let choices_open = choices.is_some();
        let choice_count = choices.as_ref().map_or(0, |(_, offers)| offers.len());
        let filters = self.model.read(cx).filters.count();
        let has_query = !self.input.read(cx).value().is_empty() || filters > 0;
        let filters_label = if filters > 0 {
            t!("search-filters-on", count = filters)
        } else {
            t!("search-filters").to_string()
        };
        let quick = [
            (Key::From, t!("search-from-short"), t!("search-from-label")),
            (
                Key::Mentions,
                t!("search-mentions-short"),
                t!("search-mentions-label"),
            ),
            (Key::In, t!("search-in-short"), t!("search-in-label")),
            (Key::Has, t!("search-has-short"), t!("search-has-label")),
            (Key::On, t!("search-on-short"), t!("search-on-label")),
        ];
        let mut pills = div()
            .id("search-quick-filters")
            .test_support()
            .role(Role::Group)
            .aria_label(t!("search-quick-filters"))
            .flex()
            .flex_wrap()
            .gap_1p5();
        for (key, short, label) in quick {
            pills = pills.child(query_field::shortcut(
                ElementId::Name(format!("search-quick-{}", key.as_str()).into()),
                short,
                label,
                None,
                &theme,
                cx.listener(move |this, _, window, cx| start_filter(&this.input, key, window, cx)),
            ));
        }
        let toolbar = div()
            .id("search-page-root")
            .flex()
            .flex_col()
            .gap_2()
            .border_b_1()
            .border_color(theme.border)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .id("search-query")
                            .relative()
                            .flex_1()
                            .min_w_0()
                            .when(choice_count > 0, |field| field.key_context(SUGGESTION_KEYS))
                            .child(
                                query_field::input(
                                    "search-input",
                                    &self.input,
                                    t!("search-field"),
                                    chips_row("search", &self.model, &theme, cx),
                                    &theme,
                                )
                                .when(has_query, |field| {
                                    field.suffix(ui::icon::button(
                                        "search-clear-query",
                                        IconName::Close,
                                        t!("search-clear"),
                                        &theme,
                                        cx.listener(|this, _, window, cx| {
                                            this.model.update(cx, |m, cx| {
                                                m.set_search("", Filters::default(), cx)
                                            });
                                            this.input.update(cx, |input, cx| {
                                                input.set_value("", window, cx)
                                            });
                                            this.input.focus_handle(cx).focus(window, cx);
                                        }),
                                    ))
                                }),
                            )
                            .children(offers_row(
                                "search",
                                &self.input,
                                &self.model,
                                choices,
                                &self.filter_selection,
                                &self.query_calendar,
                                &theme,
                                cx,
                            )),
                    )
                    .child(
                        Button::new("search-filters-button")
                            .outline()
                            .flex_none()
                            .h(px(38. * theme.scale))
                            .bg(theme.raised)
                            .border_color(theme.border)
                            .icon(IconName::Settings2)
                            .label(t!("search-filters"))
                            .accessibility_label(filters_label)
                            .when(filters > 0, |button| {
                                button.child(
                                    div()
                                        .px_1p5()
                                        .rounded_full()
                                        .bg(theme.selected)
                                        .text_xs()
                                        .text_color(theme.accent)
                                        .child(filters.to_string()),
                                )
                            })
                            .on_click(
                                cx.listener(|this, _, window, cx| this.open_filters(window, cx)),
                            ),
                    ),
            )
            .child(pills);
        let list = div()
            .id("search-results")
            .test_support()
            .role(Role::ListBox)
            .aria_label(t!("search-results"))
            .child(self.render_list(&theme, cx));
        self.result_scroll.reveal = false;
        let toolbar = toolbar
            .capture_action(cx.listener(move |this, _: &MoveDown, window, cx| {
                if choice_count > 0 && this.input.focus_handle(cx).is_focused(window) {
                    this.filter_selection.step(choice_count, false);
                    cx.stop_propagation();
                    cx.notify();
                } else if count > 0 {
                    this.selected = (this.selected + 1) % count;
                    this.result_scroll.reveal = true;
                    cx.stop_propagation();
                    cx.notify();
                }
            }))
            .capture_action(cx.listener(move |this, _: &MoveUp, window, cx| {
                if choice_count > 0 && this.input.focus_handle(cx).is_focused(window) {
                    this.filter_selection.step(choice_count, true);
                    cx.stop_propagation();
                    cx.notify();
                } else if count > 0 {
                    this.selected = (this.selected + count - 1) % count;
                    this.result_scroll.reveal = true;
                    cx.stop_propagation();
                    cx.notify();
                }
            }))
            .capture_action(cx.listener(|this, _: &Backspace, _, cx| {
                if remove_last_chip(&this.input, &this.model, cx) {
                    cx.stop_propagation();
                }
            }))
            .capture_action(cx.listener(|this, _: &AcceptSuggestion, window, cx| {
                if !this
                    .filter_selection
                    .accept(&this.input, &this.model, window, cx)
                {
                    window.focus_next(cx);
                }
                cx.stop_propagation();
            }))
            .capture_action(cx.listener(move |this, _: &Escape, window, cx| {
                if choices_open && this.input.focus_handle(cx).is_focused(window) {
                    this.filter_selection.selection.dismissed = true;
                    cx.notify();
                } else {
                    ui::page::leave(&this.session, cx);
                    window.refresh();
                }
                cx.stop_propagation();
            }));
        let session = self.session.clone();
        ui::page::Page::new("search-page", t!("search-title"), list)
            .description(t!("search-description"))
            .inherit_background()
            .toolbar(toolbar)
            .body_scroll(&self.result_scroll.handle)
            .close(t!("search-return"), move |_, _, cx| {
                ui::page::leave(&session, cx)
            })
            .hints(vec![
                Hint::new(
                    &[t!("search-key-up"), t!("search-key-down")],
                    t!("search-key-choose"),
                ),
                Hint::new(&[t!("search-key-enter")], t!("search-key-open")),
                Hint::new(
                    &[t!("search-key-backspace")],
                    t!("search-key-remove-filter"),
                ),
                Hint::new(&[t!("search-key-escape")], t!("search-key-return")),
            ])
            .render(&theme)
    }
}

// ---- The filters dialog ----

#[derive(Clone, Copy, PartialEq)]
enum When {
    Any,
    On,
    Between,
}

#[derive(Clone, Copy, PartialEq)]
enum DateField {
    On,
    After,
    Before,
}

struct FiltersDialog {
    model: Entity<SearchModel>,
    draft: Filters,
    menus: Vec<FilterMenu>,
    when: When,
    on: Entity<InputState>,
    after: Entity<InputState>,
    before: Entity<InputState>,
    calendar: Option<DateField>,
    calendar_state: Entity<CalendarState>,
    _calendar_subscription: Subscription,
    error: Option<String>,
}

#[derive(Clone)]
struct FilterChoice {
    chip: Chip,
    title: SharedString,
    detail: Option<SharedString>,
}

impl SearchableListItem for FilterChoice {
    type Value = Chip;

    fn title(&self) -> SharedString {
        self.title.clone()
    }

    fn value(&self) -> &Self::Value {
        &self.chip
    }

    fn matches(&self, query: &str) -> bool {
        let query = query.to_lowercase();
        self.title.to_lowercase().contains(&query)
            || self
                .detail
                .as_ref()
                .is_some_and(|detail| detail.to_lowercase().contains(&query))
    }

    fn render(&self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        div()
            .id(ElementId::Name(
                format!(
                    "search-filter-{}-{}",
                    self.chip.key().as_str(),
                    self.chip.value()
                )
                .into(),
            ))
            .test_support()
            .flex()
            .w_full()
            .items_center()
            .gap_2()
            .child(div().min_w_0().truncate().child(self.title.clone()))
            .children(self.detail.as_ref().map(|detail| {
                div()
                    .text_xs()
                    .text_color(theme::get(cx).muted)
                    .child(detail.clone())
            }))
    }
}

struct FilterMenu {
    key: Key,
    state: Entity<ComboboxState<SearchableVec<FilterChoice>>>,
}

impl FilterMenu {
    fn new(
        key: Key,
        workspace: Option<&Workspace>,
        draft: &[Chip],
        window: &mut Window,
        cx: &mut App,
    ) -> Self {
        let simple = |chip, title: String| FilterChoice {
            chip,
            title: title.into(),
            detail: None,
        };
        let mut choices: Vec<_> = match key {
            Key::From | Key::Mentions => workspace
                .into_iter()
                .flat_map(people)
                .map(|member| {
                    let chip = if key == Key::From {
                        Chip::From(member.id.clone())
                    } else {
                        Chip::Mentions(member.id.clone())
                    };
                    FilterChoice {
                        chip,
                        title: member_name(member).into(),
                        detail: Some(format!("@{}", member.nick).into()),
                    }
                })
                .collect(),
            Key::In => workspace
                .into_iter()
                .flat_map(|w| &w.conversations)
                .map(|c| simple(Chip::In(c.id.clone()), conversation_label(c)))
                .collect(),
            Key::Has => Has::ALL
                .into_iter()
                .map(|kind| simple(Chip::Has(kind), kind_label(kind).to_string()))
                .collect(),
            _ => Vec::new(),
        };
        // Keep a previously chosen value if its person or conversation is
        // temporarily absent from the current workspace.
        for chip in draft.iter().filter(|chip| chip.key() == key) {
            if !choices.iter().any(|choice| &choice.chip == chip) {
                let title =
                    workspace.map_or_else(|| chip.value().to_string(), |w| chip_label(w, chip));
                choices.push(simple(chip.clone(), title));
            }
        }
        let selected = choices
            .iter()
            .enumerate()
            .filter(|(_, item)| draft.contains(&item.chip))
            .map(|(index, _)| IndexPath::new(index))
            .collect();
        let state = cx.new(|cx| {
            ComboboxState::new(SearchableVec::new(choices), selected, window, cx)
                .multiple(true)
                .searchable(key != Key::Has)
        });
        Self { key, state }
    }

    fn render(&self, theme: &Theme) -> impl IntoElement {
        let (title, hint, placeholder, search) = match self.key {
            Key::From => (
                t!("search-filters-from"),
                t!("search-filters-from-help"),
                t!("search-filters-anyone"),
                t!("search-filters-find-person"),
            ),
            Key::Mentions => (
                t!("search-filters-mentions"),
                t!("search-filters-mentions-help"),
                t!("search-filters-any-mentions"),
                t!("search-filters-find-person"),
            ),
            Key::In => (
                t!("search-filters-in"),
                t!("search-filters-in-help"),
                t!("search-filters-everywhere"),
                t!("search-filters-find-conversation"),
            ),
            Key::Has => (
                t!("search-filters-has"),
                t!("search-filters-has-help"),
                t!("search-filters-any-content"),
                "",
            ),
            _ => unreachable!("a multiselect filter has one of four keys"),
        };
        field(title, theme)
            .gap_2()
            .child(
                div()
                    .id(ElementId::Name(
                        format!("search-filter-menu-{}", self.key.as_str()).into(),
                    ))
                    .test_support()
                    .role(Role::Group)
                    .aria_label(title)
                    .h(px(38. * theme.scale))
                    .child(
                        Combobox::new(&self.state)
                            .w_full()
                            .h_full()
                            .bg(theme.raised)
                            .border_color(theme.border)
                            .placeholder(placeholder)
                            .search_placeholder(search)
                            .menu_max_h(px(220.))
                            .cleanable(true),
                    ),
            )
            .child(div().text_xs().text_color(theme.muted).child(hint))
    }
}

/// Choose every filter at once. Apply changes them together; closing the
/// dialog leaves the search as it was.
fn open_filters(model: &Entity<SearchModel>, window: &mut Window, cx: &mut App) {
    let draft = model.read(cx).filters.clone();
    let chips = draft.chips();
    let client = model.read(cx).session.read(cx).client().cloned();
    let workspace = client.as_ref().map(|client| client.read());
    let menus = [Key::From, Key::Has, Key::Mentions, Key::In]
        .into_iter()
        .map(|key| FilterMenu::new(key, workspace.as_deref(), &chips, window, cx))
        .collect();
    drop(workspace);
    let day_input = |value: &Option<String>, placeholder, window: &mut Window, cx: &mut App| {
        let value = value.clone().unwrap_or_default();
        cx.new(|cx| {
            let mut state = InputState::new(window, cx).placeholder(placeholder);
            state.set_value(value, window, cx);
            state
        })
    };
    let on = day_input(&draft.on, t!("search-filters-sent-on"), window, cx);
    let after = day_input(&draft.after, t!("search-filters-sent-after"), window, cx);
    let before = day_input(&draft.before, t!("search-filters-sent-before"), window, cx);
    let when = if draft.on.is_some() {
        When::On
    } else if draft.before.is_some() || draft.after.is_some() {
        When::Between
    } else {
        When::Any
    };
    let calendar_state = cx.new(|cx| CalendarState::new(window, cx));
    let view = cx.new(|cx| {
        let subscription = cx.subscribe_in(
            &calendar_state,
            window,
            |this: &mut FiltersDialog, _, event: &CalendarEvent, window, cx| {
                if let (Some(field), CalendarEvent::Selected(Date::Single(Some(day)))) =
                    (this.calendar, event)
                {
                    this.date_input(field).update(cx, |input, cx| {
                        input.set_value(day.format("%Y-%m-%d").to_string(), window, cx)
                    });
                    this.calendar = None;
                    this.error = None;
                    cx.notify();
                }
            },
        );
        FiltersDialog {
            model: model.clone(),
            draft,
            menus,
            when,
            on,
            after,
            before,
            calendar: None,
            calendar_state,
            _calendar_subscription: subscription,
            error: None,
        }
    });
    ui::dialog::open(
        window,
        cx,
        t!("search-filters-title"),
        move |dialog, window, cx| {
            let theme = theme::get(cx);
            dialog
                .centered()
                .w(px(480. * theme.scale))
                .px_5()
                .py_4()
                .bg(theme.main)
                .border_color(theme.border)
                .on_ok(|_, _, _| false)
                .child(view.clone())
                .footer(filters_footer(&view, window, theme))
        },
    );
}

impl FiltersDialog {
    fn date_input(&self, field: DateField) -> &Entity<InputState> {
        match field {
            DateField::On => &self.on,
            DateField::After => &self.after,
            DateField::Before => &self.before,
        }
    }

    fn open_calendar(&mut self, field: DateField, window: &mut Window, cx: &mut Context<Self>) {
        let selected = sf::parse_day(self.date_input(field).read(cx).value().trim());
        let (min, max) = self.calendar_bounds(field, cx);
        self.calendar_state.update(cx, |state, cx| {
            state.set_disabled_matcher_shared(None);
            state.set_date(
                Date::Single(Some(selected.unwrap_or_else(today))),
                window,
                cx,
            );
            state.set_date(Date::Single(selected), window, cx);
            state.set_disabled_matcher(Matcher::interval(min, Some(max)), window, cx);
        });
        self.calendar = Some(field);
        cx.notify();
    }

    fn calendar_bounds(&self, field: DateField, cx: &App) -> (Option<NaiveDate>, NaiveDate) {
        let parsed = |input: &Entity<InputState>| sf::parse_day(input.read(cx).value().trim());
        match field {
            DateField::On => (None, today()),
            DateField::After => (
                None,
                parsed(&self.before).unwrap_or_else(today).min(today()),
            ),
            DateField::Before => (parsed(&self.after), today()),
        }
    }

    fn apply(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut result = self.draft.clone();
        let existing = result.chips();
        for menu in &self.menus {
            for chip in existing.iter().filter(|chip| chip.key() == menu.key) {
                result = result.without(chip);
            }
            for chip in menu.state.read(cx).selected_values() {
                result = result.with(chip);
            }
        }
        let day = |input: &Entity<InputState>, cx: &App| {
            let v = input.read(cx).value().trim().to_string();
            (!v.is_empty()).then_some(v)
        };
        let (on, after, before) = (
            day(&self.on, cx),
            day(&self.after, cx),
            day(&self.before, cx),
        );
        result.on = None;
        result.after = None;
        result.before = None;
        match self.when {
            When::Any => {}
            When::On => result.on = on,
            When::Between => {
                result.after = after;
                result.before = before;
            }
        }
        let dates = [&result.on, &result.after, &result.before];
        let today = today();
        self.error = if dates
            .iter()
            .any(|day| day.as_deref().is_some_and(|day| !sf::is_day(day)))
        {
            Some(t!(
                "search-filters-bad-day",
                example = today.format("%Y-%m-%d").to_string()
            ))
        } else if dates.iter().any(|day| {
            day.as_deref()
                .and_then(sf::parse_day)
                .is_some_and(|day| day > today)
        }) {
            Some(t!("search-filters-future-day").to_string())
        } else if let (Some(after), Some(before)) = (&result.after, &result.before)
            && after > before
        {
            Some(t!("search-filters-date-order").to_string())
        } else {
            None
        };
        if self.error.is_some() {
            cx.notify();
            return;
        }
        self.model.update(cx, |m, cx| m.set_filters(result, cx));
        ui::dialog::close(window, cx);
    }

    fn clear(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.draft = Filters::default();
        self.when = When::Any;
        self.calendar = None;
        self.error = None;
        for menu in &self.menus {
            menu.state
                .update(cx, |state, cx| state.set_selected_values(&[], window, cx));
        }
        for input in [&self.on, &self.after, &self.before] {
            input.update(cx, |i, cx| i.set_value("", window, cx));
        }
        cx.notify();
    }
}

fn date_field(
    field: DateField,
    input: &Entity<InputState>,
    calendar: Option<DateField>,
    calendar_state: &Entity<CalendarState>,
    cx: &mut Context<FiltersDialog>,
) -> Div {
    let (id, label) = match field {
        DateField::On => ("on", t!("search-filters-sent-on")),
        DateField::After => ("after", t!("search-filters-sent-after")),
        DateField::Before => ("before", t!("search-filters-sent-before")),
    };
    div()
        .flex()
        .items_center()
        .gap_1()
        .min_w_0()
        .child(
            div().flex_1().min_w_0().child(
                Input::new(input)
                    .id(ElementId::Name(format!("search-filter-{id}").into()))
                    .aria_label(label),
            ),
        )
        .child({
            let state = calendar_state.clone();
            let focus = state.read(cx).focus_handle.clone();
            let view = cx.entity().downgrade();
            Popover::new(ElementId::Name(
                format!("search-calendar-popover-{id}").into(),
            ))
            .open(calendar == Some(field))
            .anchor(Anchor::BottomRight)
            .track_focus(&focus)
            .trigger(
                Button::new(ElementId::Name(format!("search-calendar-{id}").into()))
                    .ghost()
                    .small()
                    .icon(IconName::Calendar)
                    .accessibility_label(t!("search-calendar-open", field = label)),
            )
            .on_open_change(move |open, window, cx| {
                let _ = view.update(cx, |this, cx| {
                    if *open {
                        this.open_calendar(field, window, cx);
                    } else if this.calendar == Some(field) {
                        this.calendar = None;
                        cx.notify();
                    }
                });
            })
            .content(move |_, _, _| Calendar::new(&state))
        })
}

fn field(title: &'static str, theme: &Theme) -> Div {
    div().flex().flex_col().gap_2().child(
        div()
            .text_sm()
            .font_weight(FontWeight::MEDIUM)
            .text_color(theme.text)
            .child(title),
    )
}

impl Render for FiltersDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme::get(cx).clone();
        let mut body = div()
            .id("search-filters-dialog")
            .test_support()
            .role(Role::Group)
            .aria_label(t!("search-filters-title"))
            .flex()
            .flex_col()
            .gap_3()
            .pb_2()
            .children(self.menus.iter().map(|menu| menu.render(&theme)));
        let mut when = segments(t!("search-filters-date"), &theme);
        for (value, id, label) in [
            (When::Any, "any", t!("search-filters-any-time")),
            (When::On, "on", t!("search-filters-on-day")),
            (When::Between, "between", t!("search-filters-between")),
        ] {
            when = when.child(
                segment(
                    ElementId::Name(format!("search-filter-when-{id}").into()),
                    label,
                    self.when == value,
                    &theme,
                    cx.listener(move |this, _, _, cx| {
                        this.when = value;
                        this.calendar = None;
                        cx.notify();
                    }),
                )
                .test_support()
                .reachable(&theme),
            );
        }
        let mut date = field(t!("search-filters-date"), &theme).child(when);
        match self.when {
            When::Any => {}
            When::On => {
                date = date.child(date_field(
                    DateField::On,
                    &self.on,
                    self.calendar,
                    &self.calendar_state,
                    cx,
                ))
            }
            When::Between => {
                date = date.child(
                    div()
                        .flex()
                        .gap_2()
                        .child(div().flex_1().min_w_0().child(date_field(
                            DateField::After,
                            &self.after,
                            self.calendar,
                            &self.calendar_state,
                            cx,
                        )))
                        .child(div().flex_1().min_w_0().child(date_field(
                            DateField::Before,
                            &self.before,
                            self.calendar,
                            &self.calendar_state,
                            cx,
                        ))),
                )
            }
        }
        let mut authors = segments(t!("search-filters-author-type"), &theme);
        for (value, id, label) in [
            (None, "any", t!("search-filters-anyone")),
            (Some(Author::User), "user", t!("search-people")),
            (Some(Author::Bot), "bot", t!("search-bots")),
        ] {
            authors = authors.child(
                segment(
                    ElementId::Name(format!("search-filter-author-{id}").into()),
                    label,
                    self.draft.author == value,
                    &theme,
                    cx.listener(move |this, _, _, cx| {
                        this.draft.author = value;
                        cx.notify();
                    }),
                )
                .test_support()
                .reachable(&theme),
            );
        }
        body = body
            .child(date)
            .child(field(t!("search-filters-author-type"), &theme).child(authors));
        if let Some(error) = &self.error {
            body = body.child(
                div()
                    .id("search-filters-error")
                    .test_support()
                    .role(Role::Alert)
                    .aria_label(SharedString::from(error.clone()))
                    .text_sm()
                    .text_color(theme.danger)
                    .child(error.clone()),
            );
        }
        body
    }
}

fn filters_footer(view: &Entity<FiltersDialog>, window: &Window, theme: &Theme) -> Div {
    div()
        .flex()
        .justify_end()
        .items_center()
        .gap_2()
        .border_t_1()
        .border_color(theme.border)
        .pt_3()
        .child(
            Button::new("search-filters-clear")
                .ghost()
                .label(t!("search-filters-clear"))
                .h(px(38. * theme.scale))
                .mr_auto()
                .on_click(window.listener_for(view, |this, _, window, cx| this.clear(window, cx))),
        )
        .child(
            Button::new("search-filters-cancel")
                .outline()
                .label(t!("core-cancel"))
                .h(px(38. * theme.scale))
                .bg(theme.raised)
                .border_color(theme.border)
                .on_click(|_, window, cx| ui::dialog::close(window, cx)),
        )
        .child(
            Button::new("search-filters-apply")
                .primary()
                .label(t!("search-filters-apply"))
                .h(px(38. * theme.scale))
                .on_click(window.listener_for(view, |this, _, window, cx| this.apply(window, cx))),
        )
}

#[cfg(test)]
mod tests {
    use chrono::NaiveDate;
    use ircord_client::search_filter::{Author, Chip, Has, Key};
    use ircord_client::store::Workspace;
    use ircord_client::testing::SnapshotBuilder;

    use super::{conversations, offers, resolve};

    fn workspace() -> Workspace {
        SnapshotBuilder::new()
            .network("n1", "Home")
            .channel("c-general", "general")
            .channel("c-random", "random")
            .member("m-bob", "bob")
            .member("m-eve", "eve")
            .workspace()
    }

    fn day() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 11, 21).unwrap()
    }

    #[test]
    fn finished_tokens_resolve_to_what_they_name() {
        let w = workspace();
        assert_eq!(
            resolve(&w, Key::From, "@Bob", day()),
            Some(Chip::From("m-bob".into()))
        );
        assert_eq!(
            resolve(&w, Key::Mentions, "eve", day()),
            Some(Chip::Mentions("m-eve".into()))
        );
        assert_eq!(resolve(&w, Key::From, "nobody", day()), None);
        assert_eq!(
            resolve(&w, Key::In, "#general", day()),
            Some(Chip::In("c-general".into()))
        );
        assert_eq!(
            resolve(&w, Key::Has, "GIF", day()),
            Some(Chip::Has(Has::Gif))
        );
        assert_eq!(
            resolve(&w, Key::On, "yesterday", day()),
            Some(Chip::On("2026-11-20".into()))
        );
        assert_eq!(
            resolve(&w, Key::Before, "2026-10-01", day()),
            Some(Chip::Before("2026-10-01".into()))
        );
        assert_eq!(resolve(&w, Key::After, "soon", day()), None);
        assert_eq!(
            resolve(&w, Key::Author, "bot", day()),
            Some(Chip::Author(Author::Bot))
        );
    }

    #[test]
    fn offers_match_what_is_typed() {
        let w = workspace();
        let people = offers(&w, Key::From, "b", day());
        assert_eq!(people.len(), 1);
        assert_eq!(people[0].chip, Chip::From("m-bob".into()));
        assert_eq!(offers(&w, Key::Mentions, "", day()).len(), 2);
        let places = offers(&w, Key::In, "#rand", day());
        assert_eq!(places[0].chip, Chip::In("c-random".into()));
        assert_eq!(
            offers(&w, Key::Has, "im", day())[0].chip,
            Chip::Has(Has::Image)
        );
        let days = offers(&w, Key::After, "", day());
        assert_eq!(days.len(), 4);
        assert_eq!(days[2].chip, Chip::After("2026-11-14".into()));
        let typed = offers(&w, Key::On, "2026-01-05", day());
        assert_eq!(typed[0].chip, Chip::On("2026-01-05".into()));
    }

    #[test]
    fn from_offers_self_but_mentions_do_not() {
        let w = workspace();
        let from = offers(&w, Key::From, "", day());
        assert_eq!(from[0].chip, Chip::From(w.user.id.clone()));
        assert_eq!(from[0].secondary.as_deref(), Some("@me"));
        assert!(
            !offers(&w, Key::Mentions, "", day())
                .iter()
                .any(|offer| offer.chip == Chip::Mentions(w.user.id.clone()))
        );
        assert_eq!(
            resolve(&w, Key::From, "me", day()),
            Some(Chip::From(w.user.id.clone()))
        );
    }

    #[test]
    fn conversations_rank_exact_then_prefix_then_substring() {
        let w = SnapshotBuilder::new()
            .network("n1", "Home")
            .channel("c1", "rust-help")
            .channel("c2", "rust")
            .channel("c3", "trusty")
            .channel("c4", "other")
            .workspace();
        let recent = ["c3".into(), "c1".into()];
        assert_eq!(conversations(&w, "rust", &recent), ["c2", "c1", "c3"]);
        assert_eq!(conversations(&w, "", &recent), ["c3", "c1", "c4", "c2"]);
    }
}

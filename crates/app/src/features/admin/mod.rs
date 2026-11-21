//! Administration (package AD1), after `AdministrationDialog.vue` in the web
//! client: a page for IRCord server owners, not a settings section. It has
//! five sections, each in its own file beside this one:
//!
//! - Overview (`status.rs`): the backend's health, IRCord's totals, the IRC
//!   server's, and recent Administration actions.
//! - Database (`database.rs`): space, configuration and connection waits.
//! - Users (`users.rs`): every account; add and remove owners.
//! - Welcome (`welcome.rs`): the channel guide new members pick from, and
//!   channel operators' requests to be listed in it.
//! - Emoji (`emoji.rs`): who may manage the emoji catalog.
//!
//! Only owners see the entry (a row under the conversation list, your own
//! menu, a key). Whether you are one is asked of the server
//! (`/administration/access`) when the client connects and again a minute
//! later at most, and at once when the server refuses a request, which it
//! does to anyone who stopped being an owner; the page then says so.
//! Marking someone as a bot lives with the people menus (`profile.rs`).

mod database;
mod emoji;
mod picker;
mod status;
mod users;
mod welcome;

use crate::ui::focus::Reachable;
use std::rc::{Rc, Weak};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gpui_kit::TestSupportExt;
use gpui_kit::prelude::*;
use gpui_kit::*;
use ircord_client::Client;
use ircord_client::api::ApiError;
use ircord_client::store::{Changes, Connection};

use crate::registry::{self, KeyDefault, MenuItem, Pane, PaneCtx, SidebarCtx, Slot};
use crate::route::{PaneKind, Route};
use crate::session::{Session, SessionEvent};
use crate::sidebar::layout;
use crate::theme::{self, Theme};
use crate::ui;
use crate::ui::icon::IconName;

pub use database::DatabasePanel;
pub use emoji::EmojiPanel;
pub use status::StatusPanel;
pub use users::UsersPanel;
pub use welcome::WelcomePanel;

gpui_kit::actions!(
    admin,
    [
        /// Open Administration (server owners only).
        OpenAdministration,
    ]
);

/// The page's ID in [`Route::Page`].
pub const PAGE: &str = "administration";

/// How long an answer to "am I an owner?" holds before it is asked again.
const FRESH: Duration = Duration::from_secs(60);

pub fn init(cx: &mut App) {
    cx.on_action(|_: &OpenAdministration, cx| {
        if let Some(session) = layout::session(cx)
            && is_owner(&session, cx)
        {
            open(&session, cx);
        }
    });
    registry::register(cx, |r| {
        r.panes.add(Pane::new(
            PaneKind::Page(PAGE.into()),
            |ctx: &PaneCtx, window, cx| view(ctx, window, cx).into_any_element(),
        ));
        r.sidebar_footer.add(
            Slot::new("administration", |ctx: &SidebarCtx, _, cx| {
                check(&ctx.session, cx);
                is_owner(&ctx.session, cx).then(|| sidebar_row(&ctx.session, cx))
            })
            .order(-10),
        );
        r.self_menu.add(
            MenuItem::new(
                "administration",
                t!("admin-title"),
                |ctx: &SidebarCtx, _, cx| open(&ctx.session, cx),
            )
            .icon(IconName::Settings)
            .order(80)
            .when(|ctx: &SidebarCtx, cx: &App| is_owner(&ctx.session, cx)),
        );
        r.keys.add(KeyDefault::new(
            "administration",
            t!("admin-key"),
            "",
            Some("Root"),
            OpenAdministration,
        ));
    });
}

/// Show Administration.
pub fn open(session: &Entity<Session>, cx: &mut App) {
    session.update(cx, |s, cx| s.navigate(Route::Page(PAGE.into()), cx));
}

fn showing(session: &Entity<Session>, cx: &App) -> bool {
    session.read(cx).route() == Some(Route::Page(PAGE.into()))
}

// ---- Am I an owner? ----

/// What the server last said about ownership, shared with the request that
/// asks, which runs on the client's own runtime.
#[derive(Default)]
struct Answer {
    owner: Option<bool>,
    checking: bool,
    checked: Option<Instant>,
}

struct Known {
    client: Weak<Client>,
    answer: Arc<Mutex<Answer>>,
}

/// What each client's server said about ownership.
#[derive(Default)]
struct Access(Vec<Known>);
impl Global for Access {}

fn answer(cx: &mut App, client: &Rc<Client>) -> Arc<Mutex<Answer>> {
    let access = cx.default_global::<Access>();
    access.0.retain(|k| k.client.strong_count() > 0);
    if let Some(k) = access
        .0
        .iter()
        .find(|k| k.client.upgrade().is_some_and(|c| Rc::ptr_eq(&c, client)))
    {
        return k.answer.clone();
    }
    let answer = Arc::new(Mutex::new(Answer::default()));
    access.0.push(Known {
        client: Rc::downgrade(client),
        answer: answer.clone(),
    });
    answer
}

fn owner_of(client: &Rc<Client>, cx: &App) -> Option<bool> {
    cx.try_global::<Access>()?
        .0
        .iter()
        .find(|k| k.client.upgrade().is_some_and(|c| Rc::ptr_eq(&c, client)))
        .and_then(|k| k.answer.lock().ok()?.owner)
}

/// Whether the signed-in account is a server owner, as last heard.
pub fn is_owner(session: &Entity<Session>, cx: &App) -> bool {
    session
        .read(cx)
        .client()
        .and_then(|c| owner_of(c, cx))
        .unwrap_or(false)
}

/// Ask the server whether the account is an owner, unless an answer is
/// fresh or a question is out. The answer arrives as a sidebar change, so
/// the entry and the page redraw; no app task waits on the network.
fn check(session: &Entity<Session>, cx: &mut App) {
    let Some(client) = session.read(cx).client().cloned() else {
        return;
    };
    if client.read().connection != Some(Connection::Live) {
        return;
    }
    let shared = answer(cx, &client);
    {
        let Ok(mut a) = shared.lock() else {
            return;
        };
        if a.checking || a.checked.is_some_and(|at| at.elapsed() < FRESH) {
            return;
        }
        a.checking = true;
    }
    drop(client.call(
        |api| async move { api.administration_access().await },
        move |_, result| {
            let Ok(mut a) = shared.lock() else {
                return Changes::default();
            };
            a.checking = false;
            a.checked = Some(Instant::now());
            let before = a.owner;
            match result {
                Ok(access) => a.owner = Some(access.owner),
                // Signed out or not an owner (any more).
                Err(ApiError::Unauthorized) => a.owner = Some(false),
                Err(e) if ircord_client::api::admin::refused(&e) => a.owner = Some(false),
                // Unreachable: keep the last answer.
                Err(_) => {}
            }
            Changes {
                sidebar: a.owner != before,
                ..Changes::default()
            }
        },
    ));
}

/// The server refused a request: ask again now whether the account is
/// still an owner.
pub(crate) fn refused(client: &Rc<Client>, error: &ApiError, cx: &mut App) {
    if ircord_client::api::admin::refused(error) {
        recheck(client, cx);
    }
}

/// Ask again now whether the account is an owner (its ownership changed).
pub(crate) fn recheck(client: &Rc<Client>, cx: &mut App) {
    if let Ok(mut a) = answer(cx, client).lock() {
        a.checked = None;
    }
    if let Some(session) = layout::session(cx) {
        check(&session, cx);
    }
}

fn sidebar_row(session: &Entity<Session>, cx: &App) -> AnyElement {
    let theme = theme::get(cx);
    let active = showing(session, cx);
    let session = session.clone();
    let hover = theme.hover;
    div()
        .id("sidebar-administration")
        .test_support()
        .role(Role::Button)
        .reachable(theme)
        .aria_label(t!("admin-title"))
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
        .child(ui::icon::icon(IconName::Settings, theme.muted))
        .child(div().flex_1().truncate().child(t!("admin-title")))
        .on_click(move |_, _, cx| open(&session, cx))
        .into_any_element()
}

// ---- The page ----

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Section {
    Status,
    Database,
    Users,
    Welcome,
    Emoji,
}

impl Section {
    const ALL: [Section; 5] = [
        Section::Status,
        Section::Database,
        Section::Users,
        Section::Welcome,
        Section::Emoji,
    ];

    fn key(self) -> &'static str {
        match self {
            Section::Status => "status",
            Section::Database => "database",
            Section::Users => "users",
            Section::Welcome => "welcome",
            Section::Emoji => "emoji",
        }
    }

    fn title(self) -> &'static str {
        match self {
            Section::Status => t!("admin-section-status"),
            Section::Database => t!("admin-section-database"),
            Section::Users => t!("admin-section-users"),
            Section::Welcome => t!("admin-section-welcome"),
            Section::Emoji => t!("admin-section-emoji"),
        }
    }
}

/// The page, kept across visits so the section stays.
struct Page(Entity<AdminView>);
impl Global for Page {}

fn view(ctx: &PaneCtx, window: &mut Window, cx: &mut App) -> Entity<AdminView> {
    match cx.try_global::<Page>() {
        Some(Page(view)) if view.read(cx).session == ctx.session => view.clone(),
        _ => {
            let view = cx.new(|cx| AdminView::new(ctx.session.clone(), window, cx));
            cx.set_global(Page(view.clone()));
            view
        }
    }
}

pub struct AdminView {
    session: Entity<Session>,
    section: Section,
    status: Option<Entity<StatusPanel>>,
    database: Option<Entity<DatabasePanel>>,
    users: Option<Entity<UsersPanel>>,
    welcome: Option<Entity<WelcomePanel>>,
    emoji: Option<Entity<EmojiPanel>>,
    showing: bool,
    _subscriptions: Vec<Subscription>,
}

impl AdminView {
    fn new(session: Entity<Session>, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        let subscriptions = vec![cx.subscribe(&session, |this, session, event, cx| {
            if let SessionEvent::Changed(changes) = event
                && changes.sidebar
            {
                // Ownership may have been answered.
                cx.notify();
            }
            if let SessionEvent::Route = event {
                let now = showing(&session, cx);
                if now && !this.showing {
                    // Read the open section again on every visit.
                    this.reload(cx);
                }
                this.showing = now;
            }
        })];
        Self {
            session,
            section: Section::Status,
            status: None,
            database: None,
            users: None,
            welcome: None,
            emoji: None,
            showing: true,
            _subscriptions: subscriptions,
        }
    }

    pub fn section(&self) -> Section {
        self.section
    }

    fn choose(&mut self, section: Section, cx: &mut Context<Self>) {
        if self.section != section {
            self.section = section;
            self.reload(cx);
        }
    }

    /// Read the open section's panel again, if it exists yet.
    fn reload(&mut self, cx: &mut Context<Self>) {
        match self.section {
            Section::Status => {
                if let Some(p) = &self.status {
                    p.update(cx, |p, cx| p.load(cx));
                }
            }
            Section::Database => {
                if let Some(p) = &self.database {
                    p.update(cx, |p, cx| p.load(cx));
                }
            }
            Section::Users => {
                if let Some(p) = &self.users {
                    p.update(cx, |p, cx| p.load(false, cx));
                }
            }
            Section::Welcome => {}
            Section::Emoji => {
                if let Some(p) = &self.emoji {
                    p.update(cx, |p, cx| p.load(cx));
                }
            }
        }
        cx.notify();
    }

    fn panel(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let session = self.session.clone();
        macro_rules! panel {
            ($slot:ident, $ty:ty) => {
                self.$slot
                    .get_or_insert_with(|| cx.new(|cx| <$ty>::new(session, window, cx)))
                    .clone()
                    .into_any_element()
            };
        }
        match self.section {
            Section::Status => panel!(status, StatusPanel),
            Section::Database => panel!(database, DatabasePanel),
            Section::Users => panel!(users, UsersPanel),
            Section::Welcome => panel!(welcome, WelcomePanel),
            Section::Emoji => panel!(emoji, EmojiPanel),
        }
    }
}

impl Render for AdminView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme::get(cx).clone();
        let known = self.session.read(cx).client().and_then(|c| owner_of(c, cx));
        if known != Some(true) {
            let body = if known == Some(false) {
                ui::empty::empty(
                    "admin-not-owner",
                    t!("admin-not-owner"),
                    Some(t!("admin-not-owner-hint").into()),
                    &theme,
                )
                .into_any_element()
            } else {
                ui::empty::empty("admin-checking", t!("admin-checking"), None, &theme)
                    .into_any_element()
            };
            return ui::page::Page::new("admin-page", t!("admin-title"), body)
                .render(&theme)
                .into_any_element();
        }

        let mut nav = div()
            .id("admin-sections")
            .test_support()
            .role(Role::TabList)
            .aria_label(t!("admin-title"))
            .w(px(180.))
            .flex_none()
            .p_2()
            .flex()
            .flex_col()
            .gap_0p5()
            .border_r_1()
            .border_color(theme.border)
            .child(
                div()
                    .px_2()
                    .pb_1()
                    .text_xs()
                    .font_weight(FontWeight::BOLD)
                    .text_color(theme.faint)
                    .child(t!("admin-server").to_uppercase()),
            );
        for section in Section::ALL {
            let active = section == self.section;
            nav = nav.child(
                div()
                    .id(ElementId::Name(
                        format!("admin-tab-{}", section.key()).into(),
                    ))
                    .test_support()
                    .role(Role::Tab)
                    .reachable(&theme)
                    .aria_label(section.title())
                    .aria_selected(active)
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .cursor_pointer()
                    .text_sm()
                    .text_color(if active { theme.text } else { theme.muted })
                    .when(active, |d| d.bg(theme.selected))
                    .hover({
                        let hover = theme.hover;
                        move |d| d.bg(hover)
                    })
                    .child(section.title())
                    .on_click(cx.listener(move |this, _, _, cx| this.choose(section, cx))),
            );
        }
        let panel = self.panel(window, cx);
        let body = div().size_full().flex().child(nav).child(
            div()
                .id("admin-content")
                .test_support()
                .h_full()
                .overflow_y_scroll()
                .flex_1()
                .min_w_0()
                .px_6()
                .py_5()
                .flex()
                .flex_col()
                .gap_4()
                .child(
                    div()
                        .text_lg()
                        .font_weight(FontWeight::BOLD)
                        .text_color(theme.text)
                        .child(self.section.title()),
                )
                .child(panel),
        );
        ui::page::Page::new("admin-page", t!("admin-title"), body)
            .render(&theme)
            .into_any_element()
    }
}

// ---- Shared pieces of the sections ----

/// The client to call with, if the session has one.
fn client(session: &Entity<Session>, cx: &App) -> Option<Rc<Client>> {
    session.read(cx).client().cloned()
}

/// A pause (after typing) on the client's runtime, which the headless
/// tests run in real time, unlike GPUI's timers.
fn pause(session: &Entity<Session>, cx: &App, length: Duration) -> tokio::task::JoinHandle<()> {
    session
        .read(cx)
        .runtime
        .spawn(async move { tokio::time::sleep(length).await })
}

/// A titled group of a section, with `body` under the title.
fn group(
    id: &str,
    title: impl Into<SharedString>,
    body: impl IntoElement,
    theme: &Theme,
) -> AnyElement {
    let title: SharedString = title.into();
    div()
        .id(ElementId::Name(id.to_string().into()))
        .test_support()
        .role(Role::Group)
        .aria_label(title.clone())
        .flex()
        .flex_col()
        .gap_2()
        .child(
            div()
                .text_xs()
                .font_weight(FontWeight::BOLD)
                .text_color(theme.muted)
                .child(title.to_uppercase()),
        )
        .child(body)
        .into_any_element()
}

/// One fact in a tile: a label over its value. Its accessible name is
/// "label: value", so a test or a screen reader reads both.
fn tile(id: String, label: &str, value: String, help: Option<String>, theme: &Theme) -> AnyElement {
    div()
        .id(ElementId::Name(id.into()))
        .test_support()
        .role(Role::Group)
        .aria_label(format!("{label}: {value}"))
        .min_w(px(150.))
        .flex_1()
        .px_3()
        .py_2()
        .rounded_md()
        .bg(theme.raised)
        .border_1()
        .border_color(theme.border)
        .flex()
        .flex_col()
        .gap_0p5()
        .child(
            div()
                .text_xs()
                .text_color(theme.muted)
                .child(label.to_string()),
        )
        .child(
            div()
                .text_base()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.text)
                .child(value),
        )
        .when_some(help, |d, help| {
            d.child(div().text_xs().text_color(theme.faint).child(help))
        })
        .into_any_element()
}

/// Tiles in rows that wrap.
fn tiles(children: impl IntoIterator<Item = AnyElement>) -> Div {
    div().flex().flex_wrap().gap_2().children(children)
}

/// A line of help under a control or a group.
fn hint(text: impl Into<SharedString>, theme: &Theme) -> Div {
    div().text_sm().text_color(theme.muted).child(text.into())
}

/// A message about the last request, read out when it appears.
fn notice(
    id: &'static str,
    text: impl Into<SharedString>,
    danger: bool,
    theme: &Theme,
) -> AnyElement {
    let text: SharedString = text.into();
    div()
        .id(id)
        .test_support()
        .role(if danger { Role::Alert } else { Role::Status })
        .aria_label(text.clone())
        .text_sm()
        .text_color(if danger { theme.danger } else { theme.muted })
        .child(text)
        .into_any_element()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Primary,
    Secondary,
    Ghost,
}

/// A text button. Disabled buttons look faint and do nothing.
fn button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    aria: Option<String>,
    kind: Kind,
    enabled: bool,
    theme: &Theme,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let label: SharedString = label.into();
    let (bg, fg) = match kind {
        Kind::Primary => (theme.accent, theme.on_accent),
        Kind::Secondary => (theme.input, theme.text),
        Kind::Ghost => (gpui_kit::transparent_black(), theme.muted),
    };
    let hover = theme.hover;
    div()
        .id(id.into())
        .test_support()
        .role(Role::Button)
        .reachable(theme)
        .aria_label(
            aria.map(SharedString::from)
                .unwrap_or_else(|| label.clone()),
        )
        .flex_none()
        .px_3()
        .py_1()
        .rounded_md()
        .text_sm()
        .bg(bg)
        .text_color(fg)
        .when(kind == Kind::Secondary, |d| {
            d.border_1().border_color(theme.control_border)
        })
        .child(label)
        .when(!enabled, |d| d.opacity(0.5))
        .when(enabled, |d| {
            d.cursor_pointer()
                .when(kind != Kind::Primary, |d| d.hover(move |d| d.bg(hover)))
                .on_click(on_click)
        })
        .into_any_element()
}

/// A request's error as a person reads it.
fn error_text(error: &ApiError) -> String {
    error.to_string()
}

/// The day of an RFC 3339 time, in local time: "21 November 2026".
fn day(at: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(at)
        .map(|t| {
            t.with_timezone(&chrono::Local)
                .format("%-d %B %Y")
                .to_string()
        })
        .unwrap_or_default()
}

/// The day and time of an RFC 3339 time, in local time.
fn when(at: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(at)
        .map(|t| {
            t.with_timezone(&chrono::Local)
                .format("%-d %B %Y %H:%M")
                .to_string()
        })
        .unwrap_or_default()
}

/// "2 days 3 hours 4 minutes", leaving out leading zero parts.
fn uptime(seconds: i64) -> String {
    let u = ircord_client::api::admin::uptime(seconds);
    let part = |n: i64, one: &'static str, many: fn(i64) -> String| {
        if n == 1 { one.to_string() } else { many(n) }
    };
    let mut parts = Vec::new();
    if u.days > 0 {
        parts.push(part(u.days, t!("admin-days-one"), |n| {
            t!("admin-days", count = n)
        }));
    }
    if u.hours > 0 {
        parts.push(part(u.hours, t!("admin-hours-one"), |n| {
            t!("admin-hours", count = n)
        }));
    }
    parts.push(part(u.minutes, t!("admin-minutes-one"), |n| {
        t!("admin-minutes", count = n)
    }));
    parts.join(" ")
}

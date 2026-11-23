//! The timeline: a virtual list of rows that follows the newest message,
//! pages older and newer history before the reader reaches an edge, and
//! redraws only when its own conversation changes. Message rows are drawn
//! by `crate::message`; registered row filters can hide rows.
//!
//! Reading (package R1): jumping to a message ([`reveal`], also how a
//! reply's quote goes to the original) or to a date, the "New messages"
//! line with a key to jump to it, the jump-to-present pill, marking a
//! message unread, folded join and part runs, rows to retry history that
//! failed to load, marked mentions, and exporting a conversation's log.
//!
//! Scrolling (package P6): a jump to a message puts it in the middle of the
//! view ([`TimelineView::jump_to_row`]), and jump to present glides to the
//! newest message unless reduced motion is on ([`glide`]).

pub mod announce;
pub mod attract;
pub mod export;
mod glide;
pub mod rows;

use std::time::{Duration, Instant};

use crate::menu_order;
use crate::ui::focus::Reachable;
use std::collections::{BTreeSet, HashSet};
use std::rc::Rc;

use gpui_kit::TestSupportExt;
use gpui_kit::prelude::*;
use gpui_kit::*;
use ircord_client::Client;
use ircord_client::api::ApiError;
use ircord_client::client::PAGE_SIZE;
use ircord_client::contract::Message;
use ircord_client::store::timeline::{AROUND_SIZE, NEWER_SIZE, TOPIC};
use ircord_client::store::{Changes, SendState, Workspace, people, topic};

use crate::message::{self, MessageView};
use crate::registry::{self, ConversationCtx, KeyDefault, MenuItem, MessageCtx};
use crate::route::Route;
use crate::session::{Session, SessionEvent};
use crate::theme::{self, Theme};
use crate::ui;
use crate::ui::icon::IconName;
use rows::Row;

/// Rows from an edge at which the next page is fetched, so it is there
/// before the reader arrives.
const PAGE_AHEAD: usize = 12;
/// How many older pages a jump to a date reads at most.
const DATE_PAGES: usize = 20;

actions!(
    timeline,
    [
        /// Scroll to the "New messages" line.
        JumpToUnread,
        /// Scroll to the newest message, leaving a window into older history.
        JumpToPresent
    ]
);

/// An announcement in the catalog's words.
fn announcement(say: announce::Say) -> String {
    let who = |who: String| {
        if who.is_empty() {
            t!("audit-someone").to_string()
        } else {
            who
        }
    };
    let count = |count: usize| {
        if count == 1 {
            t!("audit-new-message").to_string()
        } else {
            t!("audit-new-messages", count = count).to_string()
        }
    };
    match say {
        announce::Say::Latest { who: w, text } => {
            t!("audit-latest-single", who = who(w), text = text).to_string()
        }
        announce::Say::Several {
            count: n,
            who: w,
            text,
        } => t!(
            "audit-latest-multiple",
            count = count(n),
            who = who(w),
            text = text
        )
        .to_string(),
        announce::Say::Count(n) => count(n),
    }
}

/// The timeline view on screen, for the keys.
#[derive(Default)]
struct Reading {
    view: Option<WeakEntity<TimelineView>>,
}

impl Global for Reading {}

/// Rebuild the timeline's rows now, as after a change to how they look that
/// no workspace change announces (an appearance kept on this device).
pub fn refresh(cx: &mut App) {
    let view = cx.try_global::<Reading>().and_then(|r| r.view.clone());
    if let Some(view) = view.and_then(|v| v.upgrade()) {
        view.update(cx, |view, cx| view.rebuild(cx));
    }
}

/// Register the timeline's menu entries and keys (package R1).
pub fn init(cx: &mut App) {
    cx.default_global::<Reading>();
    cx.on_action(|_: &JumpToUnread, cx| {
        with_view(cx, |v, window, cx| v.jump_to_unread(window, cx))
    });
    cx.on_action(|_: &JumpToPresent, cx| with_view(cx, |v, _, cx| v.jump_to_present(cx)));
    registry::register(cx, |r| {
        r.message_menu.add(
            MenuItem::new(
                "mark-unread",
                t!("timeline-mark-unread"),
                |ctx: &MessageCtx, window: &mut Window, cx: &mut App| {
                    mark_unread(&ctx.session, &ctx.conversation, &ctx.message, window, cx)
                },
            )
            .icon(IconName::EyeOff)
            .order(menu_order::message::MARK_UNREAD),
        );
        r.conversation_menu.add(
            MenuItem::new(
                "jump-to-date",
                t!("timeline-jump-date"),
                |ctx: &ConversationCtx, window: &mut Window, cx: &mut App| {
                    ask_date(&ctx.session, &ctx.conversation, window, cx)
                },
            )
            .icon(IconName::Calendar)
            .order(menu_order::conversation::JUMP_TO_DATE),
        );
        r.conversation_menu.add(
            MenuItem::new(
                "export-log",
                t!("timeline-export"),
                |ctx: &ConversationCtx, window: &mut Window, cx: &mut App| {
                    export_log(&ctx.session, &ctx.conversation, window, cx)
                },
            )
            .icon(IconName::FileText)
            .order(menu_order::conversation::EXPORT_LOG),
        );
        r.keys.add(
            KeyDefault::new(
                "jump-to-unread",
                t!("timeline-key-jump-unread"),
                "shift-pageup",
                None,
                JumpToUnread,
            )
            .order(40),
        );
        r.keys.add(
            KeyDefault::new(
                "jump-to-present",
                t!("timeline-key-jump-present"),
                "shift-pagedown",
                None,
                JumpToPresent,
            )
            .order(41),
        );
    });
}

fn with_view(
    cx: &mut App,
    f: impl FnOnce(&mut TimelineView, &mut Window, &mut Context<TimelineView>) + 'static,
) {
    let Some(view) = cx
        .try_global::<Reading>()
        .and_then(|r| r.view.clone())
        .and_then(|v| v.upgrade())
    else {
        return;
    };
    let Some(window) = cx.active_window() else {
        return;
    };
    window
        .update(cx, |_, window, cx| {
            view.update(cx, |v, cx| f(v, window, cx))
        })
        .ok();
}

fn client_of(session: &Entity<Session>, cx: &App) -> Option<Rc<Client>> {
    session.read(cx).client().cloned()
}

fn timeline_changed(conversation: &str) -> Changes {
    Changes {
        timelines: BTreeSet::from([conversation.to_string()]),
        ..Default::default()
    }
}

/// Show `conversation` in the main pane unless it already shows.
fn show(session: &Entity<Session>, conversation: &str, cx: &mut App) {
    let shown = session.read(cx).route() == Some(Route::Conversation(conversation.to_string()));
    if !shown {
        session.update(cx, |s, cx| {
            s.navigate(Route::Conversation(conversation.to_string()), cx)
        });
    }
}

/// Show `message` in `conversation`: open the conversation, load the history
/// around the message if it is not loaded, scroll to it and mark it. Other
/// views (a reply's quote, search, the inbox) jump through this.
pub fn reveal(
    session: &Entity<Session>,
    conversation: &str,
    message: &str,
    window: &mut Window,
    cx: &mut App,
) {
    let Some(client) = client_of(session, cx) else {
        return;
    };
    if client.read().conversation(conversation).is_none() {
        return ui::toast::info(window, cx, t!("timeline-conversation-closed"));
    }
    show(session, conversation, cx);
    let held = client
        .read()
        .timelines
        .get(conversation)
        .is_some_and(|t| t.find(message).is_some());
    if held {
        client.change(|w| w.focus(conversation, message));
        return;
    }
    let (id, target) = (conversation.to_string(), message.to_string());
    let read = client
        .request(move |api| async move { api.messages_around(&id, &target, AROUND_SIZE).await });
    let (id, target) = (conversation.to_string(), message.to_string());
    window
        .spawn(cx, async move |cx| {
            let result = read.await;
            cx.update(|window, cx| match result {
                Ok(Ok(page)) if page.items.iter().any(|m| m.id == target) => {
                    client.change(|w| w.open_around(&id, &target, page));
                }
                Ok(Ok(_)) | Ok(Err(ApiError::Http { status: 404, .. })) => {
                    ui::toast::info(window, cx, t!("timeline-message-gone"))
                }
                Ok(Err(e)) => ui::toast::error(window, cx, e.to_string()),
                Err(e) => ui::toast::error(window, cx, e.to_string()),
            })
            .ok();
        })
        .detach();
}

/// Make `message` and everything after it unread, on every device.
pub fn mark_unread(
    session: &Entity<Session>,
    conversation: &str,
    message: &str,
    _window: &mut Window,
    cx: &mut App,
) {
    let Some(client) = client_of(session, cx) else {
        return;
    };
    client.change(|w| w.mark_unread_from(conversation, message));
    let (id, target) = (conversation.to_string(), message.to_string());
    client.call(
        move |api| async move { api.mark_unread(&id, &target).await },
        |_, result| match result {
            Ok(()) => Changes::default(),
            Err(e) => Changes {
                notices: vec![e.to_string()],
                ..Default::default()
            },
        },
    );
}

/// Load the page before the oldest message held, then call `then`.
fn page_older(client: &Rc<Client>, conversation: &str) -> Option<tokio::task::JoinHandle<()>> {
    let mut cursor = None;
    client.change(|w| {
        cursor = w.begin_older(conversation);
        match cursor {
            Some(_) => timeline_changed(conversation),
            None => Changes::default(),
        }
    });
    let cursor = cursor?;
    let (id, id2) = (conversation.to_string(), conversation.to_string());
    Some(client.call(
        move |api| async move { api.messages(&id, Some(&cursor), PAGE_SIZE).await },
        move |w, result| {
            let mut changes = w.load_older(&id2, result.as_ref().ok().cloned());
            if let Err(e) = result {
                changes.notices.push(e.to_string());
            }
            changes
        },
    ))
}

/// Load the page after the newest message held, in a window into older
/// history.
fn page_newer(client: &Rc<Client>, conversation: &str) {
    let mut boundary = None;
    client.change(|w| {
        boundary = w.begin_newer(conversation);
        match boundary {
            Some(_) => timeline_changed(conversation),
            None => Changes::default(),
        }
    });
    let Some(boundary) = boundary else {
        return;
    };
    let (id, id2) = (conversation.to_string(), conversation.to_string());
    let (b, b2) = (boundary.clone(), boundary);
    client.call(
        move |api| async move { api.messages_around(&id, &b, NEWER_SIZE).await },
        move |w, result| w.load_newer(&id2, &b2, result.map_err(|e| e.to_string())),
    );
}

/// Read the newest page again and leave a window into older history.
fn present(client: &Rc<Client>, conversation: &str) {
    let (id, id2) = (conversation.to_string(), conversation.to_string());
    client.call(
        move |api| async move { api.messages(&id, None, PAGE_SIZE).await },
        move |w, result| match result {
            Ok(page) => w.return_to_present(&id2, page),
            Err(e) => Changes {
                notices: vec![e.to_string()],
                ..Default::default()
            },
        },
    );
}

fn conversation_label(w: &Workspace, conversation: &str) -> (String, String) {
    let Some(c) = w.conversation(conversation) else {
        return (String::new(), String::new());
    };
    let network = w
        .network(&c.network_id)
        .map(|n| n.name.clone())
        .unwrap_or_default();
    let name = match c.kind {
        ircord_client::contract::ConversationKind::Channel if !c.name.starts_with('#') => {
            format!("#{}", c.name)
        }
        _ => c.name.clone(),
    };
    (name, network)
}

/// Ask for a date, then show the first message from that day on.
pub fn ask_date(session: &Entity<Session>, conversation: &str, window: &mut Window, cx: &mut App) {
    let Some(client) = client_of(session, cx) else {
        return;
    };
    let (name, _) = conversation_label(&client.read(), conversation);
    let (session, id) = (session.clone(), conversation.to_string());
    ui::prompt::prompt(
        window,
        cx,
        ui::prompt::Prompt {
            title: t!("timeline-jump-date-title", conversation = name).into(),
            label: t!("timeline-jump-date-label").into(),
            initial: chrono::Local::now().format("%Y-%m-%d").to_string(),
            ok: t!("timeline-jump-date-ok").into(),
        },
        move |text, window, cx| jump_to_date(&session, &id, text.trim(), window, cx),
    );
}

/// Load older history until it reaches `date` (`YYYY-MM-DD`, local), then
/// land on the first message from that day on.
pub fn jump_to_date(
    session: &Entity<Session>,
    conversation: &str,
    date: &str,
    window: &mut Window,
    cx: &mut App,
) {
    use chrono::TimeZone;
    let target = chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d")
        .ok()
        .and_then(|d| d.and_hms_opt(0, 0, 0))
        .and_then(|t| chrono::Local.from_local_datetime(&t).earliest())
        .map(|t| t.timestamp_millis());
    let Some(target) = target else {
        return ui::toast::info(window, cx, t!("timeline-invalid-date"));
    };
    let Some(client) = client_of(session, cx) else {
        return;
    };
    show(session, conversation, cx);
    let id = conversation.to_string();
    window
        .spawn(cx, async move |cx| {
            // Wait for the newest page if the conversation is just opening.
            for _ in 0..200 {
                let loaded = client.read().timelines.get(&id).is_some_and(|t| t.loaded);
                if loaded {
                    break;
                }
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(25))
                    .await;
            }
            for _ in 0..DATE_PAGES {
                let reached = client.read().timelines.get(&id).is_none_or(|t| {
                    t.older.is_none() || t.entries.first().is_some_and(|e| e.at <= target)
                });
                if reached {
                    break;
                }
                let Some(task) = page_older(&client, &id) else {
                    break;
                };
                if task.await.is_err() {
                    break;
                }
            }
            let (first, short) = {
                let w = client.read();
                let t = w.timelines.get(&id);
                let first = t.and_then(|t| {
                    t.entries
                        .iter()
                        .find(|e| e.at >= target)
                        .map(|e| e.message.id.clone())
                });
                let short = t.is_some_and(|t| {
                    t.older.is_some() && t.entries.first().is_some_and(|e| e.at > target)
                });
                (first, short)
            };
            cx.update(|window, cx| match first {
                None => ui::toast::info(window, cx, t!("timeline-nothing-on-date")),
                Some(message) => {
                    if short {
                        ui::toast::info(window, cx, t!("timeline-oldest-loaded"));
                    }
                    client.change(|w| w.focus(&id, &message));
                }
            })
            .ok();
        })
        .detach();
}

/// Ask where to save, then write the conversation's whole history as a log.
pub fn export_log(
    session: &Entity<Session>,
    conversation: &str,
    window: &mut Window,
    cx: &mut App,
) {
    let Some(client) = client_of(session, cx) else {
        return;
    };
    let (name, network) = conversation_label(&client.read(), conversation);
    let dir = std::env::home_dir()
        .map(|h| {
            let d = h.join("Downloads");
            if d.is_dir() { d } else { h }
        })
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    let target = cx.prompt_for_new_path(&dir, Some(&export::file_name(&network, &name, &today)));
    let id = conversation.to_string();
    window
        .spawn(cx, async move |cx| {
            let Ok(Ok(Some(path))) = target.await else {
                return;
            };
            cx.update(|window, cx| ui::toast::info(window, cx, t!("timeline-export-preparing")))
                .ok();
            let mut pages: Vec<Vec<Message>> = Vec::new();
            let mut before: Option<String> = None;
            let mut total = 0;
            let mut failed = None;
            loop {
                let (id2, cursor) = (id.clone(), before.clone());
                let read = client.request(move |api| async move {
                    api.messages(&id2, cursor.as_deref(), 100).await
                });
                match read.await {
                    Ok(Ok(page)) => {
                        total += page.items.len();
                        let empty = page.items.is_empty();
                        pages.push(page.items);
                        before = page.next_cursor;
                        if empty || before.is_none() || total >= export::MAX_EXPORT {
                            break;
                        }
                    }
                    Ok(Err(e)) => {
                        failed = Some(e.to_string());
                        break;
                    }
                    Err(e) => {
                        failed = Some(e.to_string());
                        break;
                    }
                }
            }
            let result = match failed {
                Some(e) => Err(e),
                None => {
                    let mut messages: Vec<Message> = pages.into_iter().rev().flatten().collect();
                    messages.sort_by_key(|m| ircord_client::store::parse_time(&m.created_at));
                    let w = client.read();
                    let network_id = w
                        .conversation(&id)
                        .map(|c| c.network_id.clone())
                        .unwrap_or_default();
                    let heading = t!(
                        "timeline-export-heading",
                        conversation = name.clone(),
                        network = network.clone(),
                        time = chrono::Local::now().format("%Y-%m-%d %H:%M").to_string()
                    );
                    let text = export::format_log(&w, &messages, &network_id, &heading);
                    std::fs::write(&path, text)
                        .map(|_| messages.len())
                        .map_err(|e| e.to_string())
                }
            };
            cx.update(|window, cx| match result {
                Ok(count) => ui::toast::success(
                    window,
                    cx,
                    t!(
                        "timeline-exported",
                        count = count,
                        path = path.display().to_string()
                    ),
                ),
                Err(e) => ui::toast::error(window, cx, t!("timeline-export-failed", reason = e)),
            })
            .ok();
        })
        .detach();
}

pub struct TimelineView {
    session: Entity<Session>,
    conversation: Option<String>,
    rows: Rc<Vec<Row>>,
    keys: Vec<String>,
    list: ListState,
    /// The message under the pointer, which shows the hover toolbar.
    hovered: Option<String>,
    /// Folded runs of events the person opened.
    expanded: HashSet<String>,
    /// The last jump scrolled to (`Workspace::reading.focus_seq`).
    jumped: u64,
    /// The newest row is in view.
    at_end: bool,
    /// Shows a window into older history.
    detached: bool,
    /// The message settings the rows were measured with (`layout::look`).
    look: u64,
    /// New messages for screen readers (Q1).
    announcer: announce::Announcer,
    /// The live region's text.
    announcement: SharedString,
    /// An announcement waits for the pace.
    announce_waiting: bool,
    /// A jump's row, to centre once the frame that lays it out is drawn.
    centre: Option<usize>,
    /// The centring frame is already scheduled.
    centre_queued: bool,
    /// A glide to the newest message, while it runs.
    glide: Option<Glide>,
    /// The id of the last glide started.
    glides: u64,
    /// Parsed bodies, reused by the next rebuild.
    bodies: rows::Bodies,
    /// `/attract`, while it plays on the open conversation.
    attract: Option<Playing>,
    /// The id of the last attract started.
    attracts: u64,
    _subscription: Subscription,
}

/// One glide to the newest message. `id` tells its frames apart, so a frame
/// left over from a glide that was stopped does nothing.
struct Glide {
    id: u64,
    started: Instant,
    last: Instant,
}

/// One `/attract` playing. `id` tells its frames apart from those of an
/// attract it replaced.
struct Playing {
    id: u64,
    target: attract::Target,
    started: Instant,
}

/// The time between two frames of a glide: about 60 a second.
const GLIDE_FRAME: Duration = Duration::from_millis(16);

impl TimelineView {
    pub fn new(session: Entity<Session>, cx: &mut Context<Self>) -> Self {
        let subscription =
            cx.subscribe(&session, |this, _, event: &SessionEvent, cx| match event {
                SessionEvent::Changed(changes) => {
                    let open = this.conversation.clone();
                    let reading = changes.topics.contains_key(TOPIC);
                    // The preferences and appearance say how rows look.
                    let documents = changes
                        .topics
                        .contains_key(ircord_client::store::topic::DOCUMENTS);
                    if changes.reset
                        || reading
                        || documents
                        || open
                            .as_ref()
                            .is_some_and(|id| changes.timelines.contains(id))
                    {
                        this.rebuild(cx);
                    } else if open.as_ref().is_some_and(|id| changes.typing.contains(id)) {
                        // The typing line reads the store when it draws; the
                        // rows do not change.
                        cx.notify();
                    }
                    let attracted = open
                        .as_ref()
                        .is_some_and(|id| changes.touches_id(topic::ATTRACT, id));
                    if attracted {
                        this.start_attract(cx);
                    }
                }
                SessionEvent::Selection | SessionEvent::Phase => this.rebuild(cx),
                _ => {}
            });
        cx.global_mut::<Reading>().view = Some(cx.entity().downgrade());
        let list = ListState::new(0, ListAlignment::Bottom, px(600.));
        list.set_follow_mode(FollowMode::Tail);
        let mut view = Self {
            session,
            conversation: None,
            rows: Rc::default(),
            keys: Vec::new(),
            list,
            hovered: None,
            expanded: HashSet::new(),
            jumped: 0,
            at_end: true,
            detached: false,
            look: 0,
            announcer: announce::Announcer::default(),
            announcement: SharedString::default(),
            announce_waiting: false,
            centre: None,
            centre_queued: false,
            glide: None,
            glides: 0,
            bodies: rows::Bodies::default(),
            attract: None,
            attracts: 0,
            _subscription: subscription,
        };
        view.install_scroll_handler(cx);
        view.rebuild(cx);
        view
    }

    fn install_scroll_handler(&mut self, cx: &mut Context<Self>) {
        let this = cx.entity().downgrade();
        self.list.set_scroll_handler(move |event, _window, cx| {
            let (range, count) = (event.visible_range.clone(), event.count);
            if let Some(this) = this.upgrade() {
                // The list reports the range from before this wheel tick, so
                // "newest in view" is read from the list once it has moved.
                let later = this.clone();
                cx.defer(move |cx| later.update(cx, |this, cx| this.sync_at_end(cx)));
                this.update(cx, |this, cx| this.scrolled(range, count, cx));
            }
        });
    }

    /// Whether the newest row is in view, read from the list's own position.
    fn sync_at_end(&mut self, cx: &mut Context<Self>) {
        let at_end = self.gap_to_end() <= glide::ARRIVED;
        if at_end != self.at_end {
            self.at_end = at_end;
            cx.notify();
        }
    }

    /// Fetch the next page before the reader reaches an edge, and keep the
    /// jump pill up to date.
    fn scrolled(&mut self, range: std::ops::Range<usize>, count: usize, cx: &mut Context<Self>) {
        if count == 0 {
            return;
        }
        let session = self.session.read(cx);
        let (Some(client), Some(id)) = (session.client().cloned(), self.conversation.clone())
        else {
            return;
        };
        if range.start <= PAGE_AHEAD {
            let can = client
                .read()
                .timelines
                .get(&id)
                .is_some_and(|t| t.sync_error.is_none());
            if can {
                page_older(&client, &id);
            }
        }
        if count.saturating_sub(range.end) <= PAGE_AHEAD {
            page_newer(&client, &id);
        }
    }

    /// Recompute the rows and splice the list so scroll position survives.
    pub fn rebuild(&mut self, cx: &mut Context<Self>) {
        let session = self.session.read(cx);
        let conversation = session.conversation.clone();
        let client = session.client().cloned();
        let filters = registry::get(cx).row_filters.to_vec();
        let switched = conversation != self.conversation;
        let mut target = None;
        let mut detached = false;
        let mut seen = Vec::new();
        // The message settings shown (`layout::look`).
        let look = message::layout::look(cx);
        let rows = match (&client, &conversation) {
            (Some(client), Some(id)) => {
                let mut line = None;
                client.change(|w| {
                    if switched {
                        w.begin_visit(id);
                    }
                    line = w.unread_line(id);
                    Changes::default()
                });
                let w = client.read();
                detached = w.timelines.get(id).is_some_and(|t| t.has_newer);
                seen = announce::seen(&w, id);
                let focused = w.focused(id).map(str::to_string);
                if w.reading.focus_seq != self.jumped {
                    target = focused.clone().map(|m| (m, w.reading.focus_seq));
                }
                let mut rows = rows::build_with(
                    &w,
                    id,
                    rows::Options {
                        unread: line.as_deref(),
                        focused: focused.as_deref(),
                        expanded: Some(&self.expanded),
                        ..Default::default()
                    },
                    &mut self.bodies,
                );
                if !filters.is_empty() {
                    rows.retain(|row| filters.iter().all(|f| (f.keep)(row, &w, cx)));
                }
                rows
            }
            _ => Vec::new(),
        };
        let keys: Vec<String> = rows.iter().map(Row::key).collect();
        if switched || detached != self.detached {
            self.glide = None;
            self.centre = None;
        }
        if switched {
            self.conversation = conversation;
            self.attract = None;
            self.hovered = None;
            self.expanded.clear();
            self.list.reset(rows.len());
            self.list.set_follow_mode(FollowMode::Tail);
            self.list.scroll_to_end();
            self.at_end = true;
        } else if look != self.look {
            // Every row may have a new height.
            self.list.splice(0..self.keys.len(), keys.len());
        } else if let Some((range, count)) = rows::splice(&self.keys, &keys) {
            self.list.splice(range, count);
        }
        self.look = look;
        if detached != self.detached {
            self.detached = detached;
            if detached {
                self.list.set_follow_mode(FollowMode::Normal);
            } else {
                self.list.set_follow_mode(FollowMode::Tail);
                self.list.scroll_to_end();
                self.at_end = true;
            }
        }
        if let Some((message, seq)) = target
            && let Some(ix) = rows::index_of(&rows, &message)
        {
            self.jumped = seq;
            self.jump_to_row(ix, cx);
        }
        self.keys = keys;
        self.rows = Rc::new(rows);
        if switched {
            self.announcement = SharedString::default();
        }
        let following = self.at_end && !self.detached;
        if self
            .announcer
            .observe(self.conversation.as_deref(), &seen, following)
        {
            self.announce(cx);
        }
        cx.notify();
    }

    /// Say what is new now, or once the pace allows it.
    fn announce(&mut self, cx: &mut Context<Self>) {
        let now = std::time::Instant::now();
        if let Some(say) = self.announcer.take(now) {
            self.announcement = announcement(say).into();
            cx.notify();
            return;
        }
        let wait = self.announcer.wait(now);
        if self.announce_waiting || wait.is_zero() {
            return;
        }
        self.announce_waiting = true;
        // GPUI timers do not run in the headless tests; the session's
        // runtime does.
        let runtime = self.session.read(cx).runtime.clone();
        let paced = runtime.spawn(async move { tokio::time::sleep(wait).await });
        cx.spawn(async move |this, cx| {
            let _ = paced.await;
            this.update(cx, |this, cx| {
                this.announce_waiting = false;
                this.announce(cx);
            })
            .ok();
        })
        .detach();
    }

    /// The live region's text, for tests.
    pub fn announcement(&self) -> &str {
        &self.announcement
    }

    pub fn row_count(&self) -> usize {
        self.rows.len()
    }

    /// The rows shown, for tests.
    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    /// The jump pill shows: a window into older history, or scrolled up.
    pub fn shows_jump(&self) -> bool {
        self.detached || !self.at_end
    }

    /// Scroll to the "New messages" line, or say there is none.
    pub fn jump_to_unread(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.rows.iter().position(|r| *r == Row::Unread) {
            Some(ix) => self.jump_to_row(ix, cx),
            None => ui::toast::info(window, cx, t!("timeline-nothing-unread")),
        }
    }

    /// Scroll so the row at `ix` is in the middle of the view. The list first
    /// reveals the row, so it is laid out; the next frame then centres it
    /// ([`Self::centre_now`]). Near the end of the history the list stops at
    /// the bottom, as it cannot scroll past its last row.
    fn jump_to_row(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.glide = None;
        self.list.set_follow_mode(FollowMode::Normal);
        self.list.scroll_to_reveal_item(ix);
        self.centre = Some(ix);
        self.at_end = false;
        cx.notify();
    }

    /// Centre the pending jump's row. Runs once the frame that laid out the
    /// reveal is drawn, so the row's bounds are current. A row that is not
    /// laid out stays where the reveal put it.
    fn centre_now(&mut self, cx: &mut Context<Self>) {
        self.centre_queued = false;
        let Some(ix) = self.centre.take() else {
            return;
        };
        let Some(row) = self.list.bounds_for_item(ix) else {
            return;
        };
        let view = self.list.viewport_bounds();
        self.list.scroll_by(row.center().y - view.center().y);
        cx.notify();
    }

    /// Scroll to the newest message; a window into older history reads the
    /// newest page first. Scrolled up, the view glides there unless reduced
    /// motion is on.
    pub fn jump_to_present(&mut self, cx: &mut Context<Self>) {
        let session = self.session.read(cx);
        let (Some(client), Some(id)) = (session.client().cloned(), self.conversation.clone())
        else {
            return;
        };
        client.change(|w| w.clear_focus());
        if self.detached {
            present(&client, &id);
        } else if self.at_end || theme::get(cx).reduced_motion {
            self.glide = None;
            self.list.set_follow_mode(FollowMode::Tail);
            self.list.scroll_to_end();
            self.at_end = true;
            cx.notify();
        } else {
            self.start_glide(cx);
        }
    }

    /// Start a glide: the list stops following while it runs, and each frame
    /// moves a share of the way to the end ([`glide::step`]).
    fn start_glide(&mut self, cx: &mut Context<Self>) {
        self.list.set_follow_mode(FollowMode::Normal);
        self.glides += 1;
        let now = Instant::now();
        self.glide = Some(Glide {
            id: self.glides,
            started: now,
            last: now,
        });
        self.glide_frame(cx);
    }

    /// One frame of the glide, then the next after [`GLIDE_FRAME`] on the
    /// session's runtime (GPUI timers do not run in the headless tests).
    fn glide_frame(&mut self, cx: &mut Context<Self>) {
        let Some(glide) = self.glide.as_mut() else {
            return;
        };
        let (id, now) = (glide.id, Instant::now());
        let elapsed = now - glide.started;
        let since_last = now - glide.last;
        glide.last = now;
        let gap = self.gap_to_end();
        if glide::finished(gap, elapsed) {
            self.glide = None;
            self.list.set_follow_mode(FollowMode::Tail);
            self.at_end = true;
            cx.notify();
            return;
        }
        self.list.scroll_by(px(glide::step(gap, since_last)));
        cx.notify();
        let runtime = self.session.read(cx).runtime.clone();
        let paced = runtime.spawn(async { tokio::time::sleep(GLIDE_FRAME).await });
        cx.spawn(async move |this, cx| {
            let _ = paced.await;
            this.update(cx, |this, cx| {
                if this.glide.as_ref().is_some_and(|g| g.id == id) {
                    this.glide_frame(cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// Play the newest `/attract` of the open conversation: it sparkles on
    /// the message it points at for [`attract::ATTRACT_MS`], and never moves
    /// the scroll position. One that arrives while another plays restarts it.
    fn start_attract(&mut self, cx: &mut Context<Self>) {
        let (Some(conversation), Some(client)) = (
            self.conversation.clone(),
            self.session.read(cx).client().cloned(),
        ) else {
            return;
        };
        let Some(sent) = client.read().reading.attract(&conversation).cloned() else {
            return;
        };
        let Some(target) = attract::resolve(&self.rows, &sent.message_id, &sent.member_id) else {
            return;
        };
        self.attracts += 1;
        self.attract = Some(Playing {
            id: self.attracts,
            target,
            started: Instant::now(),
        });
        self.attract_frame(cx);
    }

    /// Draw the attract once, then again after a frame (or, with reduced
    /// motion, once it is over): on the session's runtime, as GPUI's timers
    /// do not run in the headless tests.
    fn attract_frame(&mut self, cx: &mut Context<Self>) {
        let Some(playing) = self.attract.as_ref() else {
            return;
        };
        let (id, elapsed) = (playing.id, playing.started.elapsed());
        cx.notify();
        if attract::over(elapsed) {
            self.attract = None;
            return;
        }
        let wait = if theme::get(cx).reduced_motion {
            Duration::from_millis(attract::ATTRACT_MS).saturating_sub(elapsed)
        } else {
            attract::FRAME
        };
        let runtime = self.session.read(cx).runtime.clone();
        let paced = runtime.spawn(async move { tokio::time::sleep(wait).await });
        cx.spawn(async move |this, cx| {
            let _ = paced.await;
            this.update(cx, |this, cx| {
                if this.attract.as_ref().is_some_and(|p| p.id == id) {
                    this.attract_frame(cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// How far the end of the history is below the view, in pixels.
    fn gap_to_end(&self) -> f32 {
        let end = self.list.max_offset_for_scrollbar().y;
        let at = -self.list.scroll_px_offset_for_scrollbar().y;
        f32::from(end - at)
    }

    fn retry(&mut self, newer: bool, cx: &mut Context<Self>) {
        let session = self.session.read(cx);
        let (Some(client), Some(id)) = (session.client().cloned(), self.conversation.clone())
        else {
            return;
        };
        if newer {
            client.change(|w| {
                if let Some(t) = w.timelines.get_mut(&id) {
                    t.newer_failed = false;
                }
                Changes::default()
            });
            page_newer(&client, &id);
            return;
        }
        let empty = client
            .read()
            .timelines
            .get(&id)
            .is_none_or(|t| t.entries.is_empty() || t.older.is_none());
        client.change(|w| {
            if let Some(t) = w.timelines.get_mut(&id) {
                t.sync_error = None;
            }
            timeline_changed(&id)
        });
        if empty {
            client.refresh(&id);
        } else {
            page_older(&client, &id);
        }
    }

    fn toggle_fold(&mut self, id: &str, cx: &mut Context<Self>) {
        if !self.expanded.remove(id) {
            self.expanded.insert(id.to_string());
        }
        self.rebuild(cx);
    }

    fn render_row(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(row) = self.rows.get(ix).cloned() else {
            return div().into_any_element();
        };
        let theme = theme::get(cx).clone();
        match row {
            Row::Start(intro) => {
                let slots = registry::get(cx).timeline_start.to_vec();
                let extra: Vec<AnyElement> = match &self.conversation {
                    Some(id) => {
                        let ctx = ConversationCtx {
                            session: self.session.clone(),
                            conversation: id.clone(),
                        };
                        slots
                            .iter()
                            .filter_map(|slot| (slot.render)(&ctx, window, cx))
                            .collect()
                    }
                    None => Vec::new(),
                };
                render_intro(&intro, extra, &theme)
            }
            Row::Loading => div()
                .px_4()
                .py_3()
                .text_color(theme.muted)
                .text_sm()
                .child(t!("core-loading"))
                .into_any_element(),
            // A centred pill between two rules, as the web draws it.
            Row::Day(day) => div()
                .id(ElementId::Name(format!("day-{day}").into()))
                .test_support()
                .role(Role::Group)
                .aria_label(day.clone())
                .w_full()
                .px_4()
                .pt_4()
                .pb_1()
                .flex()
                .items_center()
                .gap(px(10.))
                .child(div().flex_1().h(px(1.)).bg(theme.border))
                .child(
                    div()
                        .flex_none()
                        .px(px(10.))
                        .py(px(2.))
                        .rounded_full()
                        .bg(theme.raised)
                        .border_1()
                        .border_color(theme.border)
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.faint)
                        .whitespace_nowrap()
                        .child(day),
                )
                .child(div().flex_1().h(px(1.)).bg(theme.border))
                .into_any_element(),
            Row::Retry => self.render_retry(false, &theme, cx),
            Row::NewerFailed => self.render_retry(true, &theme, cx),
            Row::NewerLoading => div()
                .px_4()
                .py_3()
                .text_color(theme.muted)
                .text_sm()
                .child(t!("timeline-newer-loading"))
                .into_any_element(),
            Row::Unread => div()
                .id("unread-line")
                .test_support()
                .role(Role::Group)
                .aria_label(t!("timeline-unread-line"))
                .px_4()
                .py_1()
                .flex()
                .items_center()
                .gap_2()
                .child(div().flex_1().h(px(1.)).bg(theme.danger))
                .child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.danger)
                        .child(t!("timeline-unread-line")),
                )
                .into_any_element(),
            Row::Fold(f) => self.render_fold(&f, &theme, cx),
            Row::Event(e) => div()
                .px_4()
                .py_0p5()
                .pl(px(28.))
                .flex()
                .gap_2()
                .text_size(px(13.))
                .line_height(px(20.))
                .text_color(theme.body)
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(theme.faint)
                        .min_w(px(34.))
                        .flex_none()
                        .whitespace_nowrap()
                        .child(e.time.clone()),
                )
                .child(e.text.clone())
                .into_any_element(),
            Row::Pending(p) => self.render_pending(&p, &theme, cx),
            Row::Message(m) => {
                let Some(conversation) = self.conversation.clone() else {
                    return div().into_any_element();
                };
                let id = m.id.clone();
                let hovered = self.hovered.as_deref() == Some(id.as_str());
                let reduced = theme.reduced_motion;
                let attract = self
                    .attract
                    .as_ref()
                    .and_then(|p| attract::fx(&p.target, &m.id, p.started.elapsed(), reduced));
                let this = cx.entity().downgrade();
                message::render(
                    MessageView {
                        row: m.clone(),
                        ctx: MessageCtx {
                            session: self.session.clone(),
                            conversation,
                            message: m.id.clone(),
                            own: m.own,
                        },
                        hovered,
                        attract,
                        on_hover: Rc::new(move |over, _, cx| {
                            if let Some(this) = this.upgrade() {
                                this.update(cx, |this, cx| this.set_hovered(&id, over, cx));
                            }
                        }),
                    },
                    &theme,
                    window,
                    cx,
                )
            }
        }
    }

    fn render_retry(&self, newer: bool, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let (id, text) = if newer {
            ("retry-newer", t!("timeline-newer-failed"))
        } else {
            ("retry-older", t!("timeline-older-failed"))
        };
        let this = cx.entity().downgrade();
        div()
            .id(if newer {
                "retry-newer-row"
            } else {
                "retry-older-row"
            })
            .role(Role::Status)
            .px_4()
            .py_3()
            .flex()
            .justify_center()
            .items_center()
            .gap_3()
            .text_sm()
            .text_color(theme.muted)
            .child(text)
            .child(
                div()
                    .id(id)
                    .test_support()
                    .role(Role::Button)
                    .reachable(theme)
                    .aria_label(t!("timeline-try-again"))
                    .px_2()
                    .py_0p5()
                    .rounded_md()
                    .bg(theme.selected)
                    .text_color(theme.text)
                    .cursor_pointer()
                    .child(t!("timeline-try-again"))
                    .on_click(move |_, _, cx| {
                        if let Some(this) = this.upgrade() {
                            this.update(cx, |this, cx| this.retry(newer, cx));
                        }
                    }),
            )
            .into_any_element()
    }

    fn render_fold(&self, f: &rows::FoldRow, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let this = cx.entity().downgrade();
        let id = f.id.clone();
        let label = if f.expanded {
            t!("timeline-fold-hide")
        } else {
            t!("timeline-fold-show")
        };
        let mut column = div().flex().flex_col().child(
            div()
                .flex()
                .gap_2()
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.faint)
                        .min_w(px(40.))
                        .flex_none()
                        .whitespace_nowrap()
                        .child(f.time.clone()),
                )
                .child(
                    div()
                        .id(ElementId::Name(format!("fold-{}", f.id).into()))
                        .test_support()
                        .role(Role::Button)
                        .reachable(theme)
                        .aria_label(format!("{} · {label}", f.summary))
                        .aria_expanded(f.expanded)
                        .cursor_pointer()
                        .flex()
                        .items_center()
                        .gap_1p5()
                        .hover({
                            let text = theme.text;
                            move |d| d.text_color(text)
                        })
                        .child(ui::icon::icon(IconName::User, theme.faint))
                        .child(f.summary.clone())
                        .child(ui::icon::icon(
                            if f.expanded {
                                IconName::ChevronDown
                            } else {
                                IconName::ChevronRight
                            },
                            theme.faint,
                        ))
                        .on_click(move |_, _, cx| {
                            if let Some(this) = this.upgrade() {
                                this.update(cx, |this, cx| this.toggle_fold(&id, cx));
                            }
                        }),
                ),
        );
        if f.expanded {
            for e in &f.events {
                column = column.child(
                    div()
                        .flex()
                        .gap_2()
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.faint)
                                .min_w(px(40.))
                                .flex_none()
                                .whitespace_nowrap()
                                .child(e.time.clone()),
                        )
                        .child(e.text.clone()),
                );
            }
        }
        div()
            .px_4()
            .py_0p5()
            .pl(px(72.))
            .text_sm()
            .text_color(theme.muted)
            .child(column)
            .into_any_element()
    }

    fn render_jump(&self, theme: &Theme, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.shows_jump() || self.conversation.is_none() {
            return None;
        }
        let label = if self.detached {
            t!("timeline-viewing-older")
        } else {
            t!("timeline-jump-present")
        };
        let this = cx.entity().downgrade();
        Some(
            div()
                .absolute()
                .bottom(px(28.))
                .left_0()
                .right_0()
                .flex()
                .justify_center()
                .child(
                    div()
                        .id("jump-present")
                        .test_support()
                        .role(Role::Button)
                        .reachable(theme)
                        .aria_label(label)
                        .px_3()
                        .py_1()
                        .rounded_full()
                        .bg(theme.accent)
                        .text_color(theme.on_accent)
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .cursor_pointer()
                        .child(label)
                        .on_click(move |_, _, cx| {
                            if let Some(this) = this.upgrade() {
                                this.update(cx, |this, cx| this.jump_to_present(cx));
                            }
                        }),
                )
                .into_any_element(),
        )
    }

    fn set_hovered(&mut self, id: &str, over: bool, cx: &mut Context<Self>) {
        let next = if over {
            Some(id.to_string())
        } else if self.hovered.as_deref() == Some(id) {
            None
        } else {
            return;
        };
        if next != self.hovered {
            self.hovered = next;
            cx.notify();
        }
    }

    /// A send the server has not confirmed, drawn as the web client draws it:
    /// a message row of yours (with your name, picture and time unless it
    /// continues your message above), its text faded while it is on its
    /// way and red once it failed, with the reason and the buttons to retry,
    /// move it to the draft, copy it or remove it underneath. It stays at
    /// the end of the conversation, after what the server has stored, until
    /// it is sent or removed.
    fn render_pending(&self, row: &rows::PendingRow, theme: &Theme, cx: &App) -> AnyElement {
        use crate::message::layout::Layout;
        let p = &row.send;
        let failed = matches!(p.state, SendState::Failed(_));
        let look = crate::message::layout::settings(cx);
        let compact = look.layout == Layout::Compact;
        let irc = look.layout == Layout::Irc;
        let avatar = if compact { 24. } else { 40. };
        // As read, without the IRC formatting codes the send filters added.
        let plain = ircord_client::format::parse(&p.text).text;
        let text = if p.action {
            format!("* {plain}")
        } else {
            plain
        };
        let label = rows::pending_label(p).to_string();
        let mut status = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_x_2()
            .gap_y_1()
            .pt_1()
            .text_sm()
            .child(
                div()
                    .id(ElementId::Name(
                        format!("pending-status-{}", p.client_id).into(),
                    ))
                    .test_support()
                    .role(Role::Status)
                    .aria_label(label.clone())
                    .text_color(if failed { theme.danger } else { theme.muted })
                    .child(label),
            );
        if failed {
            status = status.children(crate::features::message_actions::failed_send_actions(p));
        }
        let body = div()
            .text_size(px(look.size as f32))
            .line_height(relative(1.375))
            .font_family(crate::message::layout::message_font(&look, theme))
            .text_color(if failed { theme.danger } else { theme.text })
            .when(!failed, |d| d.opacity(0.55))
            .child(text.clone());
        let frame = div()
            .id(ElementId::Name(format!("pending-{}", p.client_id).into()))
            .test_support()
            .role(Role::Group)
            .aria_label(text)
            .w_full()
            .px_4();
        let (me, name, nick, server) = {
            let session = self.session.read(cx);
            let server = session.server().map(|s| s.url().clone());
            match session.client() {
                Some(client) => {
                    let w = client.read();
                    let network = w
                        .conversation(&p.conversation_id)
                        .map(|c| c.network_id.clone())
                        .unwrap_or_default();
                    let nick = people::self_nick(&w, &network);
                    let name = people::self_name(&w, &network).unwrap_or_else(|| nick.clone());
                    (people::me(&w, Some(&network)).clone(), name, nick, server)
                }
                None => (Default::default(), String::new(), String::new(), server),
            }
        };
        let color = ui::avatar::name_color(&me, look.colored_names, theme)
            .map(theme::hex)
            .unwrap_or(theme.text);
        if irc {
            // One line, as the IRC layout draws a message: the time, your
            // nick in its column, the text; the status under the text.
            let (before, after) = if p.action { ("* ", "") } else { ("<", ">") };
            let line = div()
                .w_full()
                .flex()
                .items_baseline()
                .gap_2p5()
                .child(
                    div()
                        .flex_none()
                        .text_size(px(look.size as f32 * 0.85))
                        .text_color(theme.faint)
                        .child(row.time.clone()),
                )
                .child(
                    div()
                        .w(px(128.))
                        .flex_none()
                        .flex()
                        .justify_end()
                        .overflow_hidden()
                        .child(div().text_color(theme.faint).child(before))
                        .child(
                            div()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(color)
                                .truncate()
                                .child(nick),
                        )
                        .child(div().text_color(theme.faint).child(after)),
                )
                .child(div().flex_1().min_w_0().child(body));
            return frame
                .py(px(1.))
                .flex()
                .flex_col()
                .child(line)
                .child(div().pl(px(138.)).child(status))
                .into_any_element();
        }
        let mut column = div().flex_1().min_w_0().flex().flex_col();
        let gutter = if row.header {
            column = column.child(
                div()
                    .flex()
                    .items_baseline()
                    .gap_2()
                    .line_height(px(21.))
                    .child(
                        div()
                            .id(ElementId::Name(
                                format!("pending-sender-{}", p.client_id).into(),
                            ))
                            .test_support()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_size(px(if compact { 14. } else { 15. }))
                            .text_color(color)
                            .child(name.clone()),
                    )
                    .child(
                        div()
                            .id(ElementId::Name(
                                format!("pending-time-{}", p.client_id).into(),
                            ))
                            .test_support()
                            .text_size(px(12.))
                            .text_color(theme.faint)
                            .child(row.time.clone()),
                    ),
            );
            ui::avatar::person(
                ui::avatar::Person {
                    member: &me,
                    name: &name,
                    server: server.as_ref(),
                    presence: false,
                    key: &format!("pending-{}", p.client_id),
                },
                px(avatar),
                theme,
            )
            .into_any_element()
        } else {
            div().w(px(avatar)).flex_none().into_any_element()
        };
        column = column.child(body).child(status);
        let space = if compact {
            look.spacing as f32 / 2.
        } else {
            look.spacing as f32
        };
        frame
            .py(px(1.))
            .when(row.header, |d| d.pt(px(space.max(2.))))
            .flex()
            .when(compact, |d| d.gap_2p5())
            .when(!compact, |d| d.gap_3p5())
            .child(gutter)
            .child(column)
            .into_any_element()
    }
}

/// The typing line under the timeline.
/// The start of the history, as the web draws it: for a channel a `#` disc,
/// its name as a heading, "This is the beginning of #general." and the
/// topic; for a direct message the registered intro (their picture, name and
/// bio) and the line under it.
fn render_intro(intro: &rows::Intro, extra: Vec<AnyElement>, theme: &Theme) -> AnyElement {
    use rows::IntroKind;
    let mut column = div()
        .id("timeline-intro")
        .test_support()
        .role(Role::Group)
        .aria_label(intro.text.clone())
        .px_4()
        .pt_8()
        .pb_4()
        .flex()
        .flex_col()
        .gap_1()
        .children(extra);
    if matches!(intro.kind, IntroKind::Channel | IntroKind::Group) {
        // The app embeds gpui-kit's default icons, which have no `#`: the
        // channel's glyph is the character itself.
        let glyph = if intro.kind == IntroKind::Channel {
            div()
                .text_2xl()
                .font_weight(FontWeight::BOLD)
                .text_color(theme.accent)
                .child("#")
                .into_any_element()
        } else {
            ui::icon::icon(IconName::User, theme.accent).into_any_element()
        };
        column = column
            .child(
                div()
                    .mb_2()
                    .size(px(56.))
                    .flex_none()
                    .rounded_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(theme.accent.opacity(0.16))
                    .border_1()
                    .border_color(theme.accent.opacity(0.35))
                    .child(glyph),
            )
            .child(
                div()
                    .id("timeline-intro-title")
                    .test_support()
                    .role(Role::Heading)
                    .aria_label(intro.title.clone())
                    .text_2xl()
                    .font_weight(FontWeight::BOLD)
                    .text_color(theme.text)
                    .child(intro.title.clone()),
            );
    }
    column = column.child(
        div()
            .text_sm()
            .text_color(theme.muted)
            .child(intro.text.clone()),
    );
    if !intro.topic.is_empty() {
        column = column.child(
            div()
                .text_sm()
                .text_color(theme.muted)
                .child(intro.topic.clone()),
        );
    }
    column.into_any_element()
}

pub fn typing_text(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [one] => t!("timeline-typing-one", name = one),
        [a, b] => t!("timeline-typing-two", first = a, second = b),
        _ => t!("timeline-typing-many").to_string(),
    }
}

impl Render for TimelineView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.centre.is_some() && !self.centre_queued {
            // After this frame the list has laid out the reveal (`jump_to_row`).
            self.centre_queued = true;
            let view = cx.entity().downgrade();
            window.on_next_frame(move |_, cx| {
                view.update(cx, |this, cx| this.centre_now(cx)).ok();
            });
        }
        let theme = theme::get(cx).clone();
        let session = self.session.read(cx);
        let typing = match (session.client(), &self.conversation) {
            (Some(client), Some(id)) => {
                client.read().typing_in(id, ircord_client::events::now_ms())
            }
            _ => Vec::new(),
        };
        let jump = self.render_jump(&theme, cx);
        div()
            .id("timeline")
            .test_support()
            .role(Role::Log)
            .aria_label(t!("audit-timeline"))
            .relative()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(
                list(
                    self.list.clone(),
                    cx.processor(|this, ix, window, cx| this.render_row(ix, window, cx)),
                )
                .flex_1()
                .size_full(),
            )
            .child(
                div()
                    .id("typing")
                    .h(px(20.))
                    .px_4()
                    .text_xs()
                    .text_color(theme.muted)
                    .child(typing_text(&typing)),
            )
            .children(jump)
            .child(
                // The one live region for new messages (`announce`).
                div()
                    .id("timeline-announcement")
                    .test_support()
                    .role(Role::Status)
                    .aria_label(self.announcement.clone())
                    .a11y_synthetic_children(|tree| {
                        tree.parent_node().set_live(accesskit::Live::Polite);
                    })
                    .absolute()
                    .size_0()
                    .overflow_hidden(),
            )
    }
}

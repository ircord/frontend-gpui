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

pub mod export;
pub mod rows;

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
use ircord_client::store::{Changes, SendState, Workspace};

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
            .order(70),
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
            .order(60),
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
            .order(61),
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
    _subscription: Subscription,
}

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
                        || open.as_ref().is_some_and(|id| {
                            changes.timelines.contains(id) || changes.typing.contains(id)
                        })
                    {
                        this.rebuild(cx);
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
                this.update(cx, |this, cx| this.scrolled(range, count, cx));
            }
        });
    }

    /// Fetch the next page before the reader reaches an edge, and keep the
    /// jump pill up to date.
    fn scrolled(&mut self, range: std::ops::Range<usize>, count: usize, cx: &mut Context<Self>) {
        if count == 0 {
            return;
        }
        let at_end = range.end >= count;
        if at_end != self.at_end {
            self.at_end = at_end;
            cx.notify();
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
                let focused = w.focused(id).map(str::to_string);
                if w.reading.focus_seq != self.jumped {
                    target = focused.clone().map(|m| (m, w.reading.focus_seq));
                }
                let mut rows = rows::build(
                    &w,
                    id,
                    rows::Options {
                        unread: line.as_deref(),
                        focused: focused.as_deref(),
                        expanded: Some(&self.expanded),
                    },
                );
                if !filters.is_empty() {
                    rows.retain(|row| filters.iter().all(|f| (f.keep)(row, &w, cx)));
                }
                rows
            }
            _ => Vec::new(),
        };
        let keys: Vec<String> = rows.iter().map(Row::key).collect();
        if switched {
            self.conversation = conversation;
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
            self.list.set_follow_mode(FollowMode::Normal);
            self.list.scroll_to_reveal_item(ix);
            self.at_end = false;
        }
        self.keys = keys;
        self.rows = Rc::new(rows);
        cx.notify();
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
            Some(ix) => {
                self.list.set_follow_mode(FollowMode::Normal);
                self.list.scroll_to_reveal_item(ix);
                self.at_end = false;
                cx.notify();
            }
            None => ui::toast::info(window, cx, t!("timeline-nothing-unread")),
        }
    }

    /// Scroll to the newest message; a window into older history reads the
    /// newest page first.
    pub fn jump_to_present(&mut self, cx: &mut Context<Self>) {
        let session = self.session.read(cx);
        let (Some(client), Some(id)) = (session.client().cloned(), self.conversation.clone())
        else {
            return;
        };
        client.change(|w| w.clear_focus());
        if self.detached {
            present(&client, &id);
        } else {
            self.list.set_follow_mode(FollowMode::Tail);
            self.list.scroll_to_end();
            self.at_end = true;
            cx.notify();
        }
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
            Row::Start => {
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
                div()
                    .px_4()
                    .pt_8()
                    .pb_4()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .children(extra)
                    .child(
                        div()
                            .text_color(theme.muted)
                            .text_sm()
                            .child(t!("timeline-start")),
                    )
                    .into_any_element()
            }
            Row::Loading => div()
                .px_4()
                .py_3()
                .text_color(theme.muted)
                .text_sm()
                .child(t!("core-loading"))
                .into_any_element(),
            Row::Day(day) => div()
                .px_4()
                .py_2()
                .flex()
                .items_center()
                .gap_2()
                .child(div().flex_1().h(px(1.)).bg(theme.selected))
                .child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.muted)
                        .child(day),
                )
                .child(div().flex_1().h(px(1.)).bg(theme.selected))
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
                .pl(px(72.))
                .flex()
                .gap_2()
                .text_sm()
                .text_color(theme.muted)
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.faint)
                        .min_w(px(40.))
                        .flex_none()
                        .whitespace_nowrap()
                        .child(e.time.clone()),
                )
                .child(e.text.clone())
                .into_any_element(),
            Row::Pending(p) => self.render_pending(&p, &theme),
            Row::Message(m) => {
                let Some(conversation) = self.conversation.clone() else {
                    return div().into_any_element();
                };
                let id = m.id.clone();
                let hovered = self.hovered.as_deref() == Some(id.as_str());
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
                        .aria_label(format!("{} · {label}", f.summary))
                        .aria_expanded(f.expanded)
                        .cursor_pointer()
                        .hover({
                            let text = theme.text;
                            move |d| d.text_color(text)
                        })
                        .child(format!(
                            "{} {}",
                            if f.expanded { "▾" } else { "▸" },
                            f.summary
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

    fn render_pending(&self, p: &ircord_client::store::PendingSend, theme: &Theme) -> AnyElement {
        let failed = matches!(p.state, SendState::Failed(_));
        let mut row = div()
            .id(ElementId::Name(format!("pending-{}", p.client_id).into()))
            .test_support()
            .px_4()
            .py_0p5()
            .pl(px(72.))
            .flex()
            .flex_col()
            .child(
                div()
                    .text_color(if failed { theme.danger } else { theme.faint })
                    .child(if p.action {
                        format!("* {}", p.text)
                    } else {
                        p.text.clone()
                    }),
            );
        let label = rows::pending_label(p).to_string();
        let mut status = div()
            .flex()
            .gap_2()
            .text_xs()
            .text_color(theme.muted)
            .child(if failed {
                t!("timeline-not-sent", reason = label)
            } else {
                label
            });
        if failed {
            let session = self.session.clone();
            let client_id = p.client_id.clone();
            status = status.child(
                div()
                    .id(ElementId::Name(format!("retry-{}", p.client_id).into()))
                    .test_support()
                    .role(Role::Button)
                    .aria_label(t!("timeline-retry"))
                    .text_color(theme.link)
                    .cursor_pointer()
                    .child(t!("timeline-retry"))
                    .on_click(move |_, _, cx| {
                        if let Some(client) = session.read(cx).client() {
                            client.retry(&client_id);
                        }
                    }),
            );
        }
        row = row.child(status);
        row.into_any_element()
    }
}

/// The typing line under the timeline.
pub fn typing_text(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [one] => t!("timeline-typing-one", name = one),
        [a, b] => t!("timeline-typing-two", first = a, second = b),
        _ => t!("timeline-typing-many").to_string(),
    }
}

impl Render for TimelineView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
    }
}

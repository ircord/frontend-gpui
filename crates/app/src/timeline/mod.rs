//! The timeline: a virtual list of rows that follows the newest message,
//! loads older history when scrolled to the top, and redraws only when its
//! own conversation changes. Message rows are drawn by `crate::message`;
//! registered row filters can hide rows.

pub mod rows;

use std::rc::Rc;

use gpui_kit::TestSupportExt;
use gpui_kit::prelude::*;
use gpui_kit::*;
use ircord_client::store::SendState;

use crate::message::{self, MessageView};
use crate::registry::{self, MessageCtx};
use crate::session::{Session, SessionEvent};
use crate::theme::{self, Theme};
use rows::Row;

/// Register the timeline's menu entries and keys (package R1).
pub fn init(_cx: &mut App) {}

/// Show `message` in `conversation`: open the conversation, load the history
/// around the message if it is not loaded, scroll to it and mark it. Other
/// views (a reply's quote, search, the inbox) jump through this.
pub fn reveal(
    _session: &Entity<Session>,
    _conversation: &str,
    _message: &str,
    _window: &mut Window,
    _cx: &mut App,
) {
}

pub struct TimelineView {
    session: Entity<Session>,
    conversation: Option<String>,
    rows: Rc<Vec<Row>>,
    keys: Vec<String>,
    list: ListState,
    /// The message under the pointer, which shows the hover toolbar.
    hovered: Option<String>,
    _subscription: Subscription,
}

impl TimelineView {
    pub fn new(session: Entity<Session>, cx: &mut Context<Self>) -> Self {
        let subscription =
            cx.subscribe(&session, |this, _, event: &SessionEvent, cx| match event {
                SessionEvent::Changed(changes) => {
                    let open = this.conversation.clone();
                    if changes.reset
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
        let list = ListState::new(0, ListAlignment::Bottom, px(600.));
        list.set_follow_mode(FollowMode::Tail);
        let mut view = Self {
            session,
            conversation: None,
            rows: Rc::default(),
            keys: Vec::new(),
            list,
            hovered: None,
            _subscription: subscription,
        };
        view.install_scroll_handler(cx);
        view.rebuild(cx);
        view
    }

    fn install_scroll_handler(&mut self, cx: &mut Context<Self>) {
        let this = cx.entity().downgrade();
        self.list.set_scroll_handler(move |event, _window, cx| {
            if event.visible_range.start == 0
                && event.count > 0
                && let Some(this) = this.upgrade()
            {
                this.update(cx, |this, cx| this.load_older(cx));
            }
        });
    }

    fn load_older(&mut self, cx: &mut Context<Self>) {
        let session = self.session.read(cx);
        if let (Some(client), Some(id)) = (session.client(), &self.conversation) {
            client.load_older(id);
        }
    }

    /// Recompute the rows and splice the list so scroll position survives.
    pub fn rebuild(&mut self, cx: &mut Context<Self>) {
        let session = self.session.read(cx);
        let conversation = session.conversation.clone();
        let filters = registry::get(cx).row_filters.to_vec();
        let rows = match (session.client(), &conversation) {
            (Some(client), Some(id)) => {
                let w = client.read();
                let mut rows = rows::build(&w, id);
                if !filters.is_empty() {
                    rows.retain(|row| filters.iter().all(|f| (f.keep)(row, &w, cx)));
                }
                rows
            }
            _ => Vec::new(),
        };
        let keys: Vec<String> = rows.iter().map(Row::key).collect();
        if conversation != self.conversation {
            self.conversation = conversation;
            self.hovered = None;
            self.list.reset(rows.len());
            self.list.set_follow_mode(FollowMode::Tail);
            self.list.scroll_to_end();
        } else if let Some((range, count)) = rows::splice(&self.keys, &keys) {
            self.list.splice(range, count);
        }
        self.keys = keys;
        self.rows = Rc::new(rows);
        cx.notify();
    }

    pub fn row_count(&self) -> usize {
        self.rows.len()
    }

    fn render_row(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(row) = self.rows.get(ix).cloned() else {
            return div().into_any_element();
        };
        let theme = theme::get(cx).clone();
        match row {
            Row::Start => div()
                .px_4()
                .pt_8()
                .pb_4()
                .text_color(theme.muted)
                .text_sm()
                .child(t!("timeline-start"))
                .into_any_element(),
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
                        .w(px(40.))
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
        div()
            .id("timeline")
            .test_support()
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
    }
}

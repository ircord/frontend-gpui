//! The signed-in layout: rail and sidebar, the conversation header with the
//! connection banner, the timeline and composer, and the member list.

use gpui_kit::TestSupportExt;
use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::prelude::*;
use gpui_kit::*;
use ircord_client::contract::{ConversationKind, NetworkStatus};
use ircord_client::store::Connection;

use crate::members::MembersView;
use crate::session::{Session, SessionEvent};
use crate::sidebar::SidebarView;
use crate::theme::{self, c};
use crate::timeline::TimelineView;

pub struct WorkspaceView {
    session: Entity<Session>,
    pub sidebar: Entity<SidebarView>,
    pub timeline: Entity<TimelineView>,
    pub members: Entity<MembersView>,
    pub composer: Entity<TextareaState>,
    notice: Option<String>,
    _subscriptions: Vec<Subscription>,
}

impl WorkspaceView {
    pub fn new(session: Entity<Session>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let sidebar = cx.new(|cx| SidebarView::new(session.clone(), cx));
        let timeline = cx.new(|cx| TimelineView::new(session.clone(), cx));
        let members = cx.new(|cx| MembersView::new(session.clone(), cx));
        let composer = cx.new(|cx| {
            let mut state = TextareaState::new(window, cx)
                .auto_grow(1, 10)
                .placeholder("Message");
            state.set_submit_on_enter(true, cx);
            state
        });
        let subscriptions = vec![
            cx.subscribe_in(
                &composer,
                window,
                |this, state, event: &InputEvent, window, cx| match event {
                    InputEvent::PressEnter { shift: false, .. } => {
                        let text = state.read(cx).value().to_string();
                        if !text.trim().is_empty() {
                            this.session.update(cx, |s, _| s.send(&text));
                            state.update(cx, |s, cx| s.set_value("", window, cx));
                        }
                    }
                    InputEvent::Change => {
                        let has_text = !state.read(cx).value().is_empty();
                        let session = this.session.read(cx);
                        if let (Some(client), Some(id)) = (session.client(), &session.conversation)
                        {
                            client.typing(id, has_text);
                        }
                    }
                    _ => {}
                },
            ),
            cx.subscribe(&session, |this, _, event: &SessionEvent, cx| match event {
                SessionEvent::Notice(text) => {
                    this.notice = Some(text.clone());
                    cx.notify();
                }
                SessionEvent::Selection => {
                    this.notice = None;
                    cx.notify();
                }
                SessionEvent::Connection | SessionEvent::Phase => cx.notify(),
                SessionEvent::Changed(changes) if changes.sidebar => cx.notify(),
                _ => {}
            }),
        ];
        Self {
            session,
            sidebar,
            timeline,
            members,
            composer,
            notice: None,
            _subscriptions: subscriptions,
        }
    }

    fn banner(&self, cx: &App) -> Option<(String, u32)> {
        let session = self.session.read(cx);
        let client = session.client()?;
        let w = client.read();
        match &w.connection {
            Some(Connection::Connecting) => return Some(("Connecting…".into(), theme::WARNING)),
            Some(Connection::Replaying) => {
                return Some(("Catching up on messages…".into(), theme::ACCENT));
            }
            Some(Connection::Offline { reason, retry_ms }) => {
                return Some((
                    format!(
                        "Disconnected: {reason}. Trying again in {} s.",
                        retry_ms.div_ceil(1000)
                    ),
                    theme::DANGER,
                ));
            }
            _ => {}
        }
        let network = session.network.as_deref().and_then(|id| w.network(id))?;
        match network.status {
            NetworkStatus::Connected => None,
            NetworkStatus::Connecting => {
                Some((format!("Connecting to {}…", network.name), theme::WARNING))
            }
            _ => Some((
                match &network.status_detail {
                    Some(detail) if !detail.is_empty() => {
                        format!("{} is disconnected: {detail}", network.name)
                    }
                    _ => format!("{} is disconnected.", network.name),
                },
                theme::DANGER,
            )),
        }
    }
}

impl Render for WorkspaceView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let session = self.session.read(cx);
        let client = session.client().cloned();
        let (title, topic, kind, can_send) = match (&client, &session.conversation) {
            (Some(client), Some(id)) => {
                let w = client.read();
                match w.conversation(id) {
                    Some(conv) => (
                        conv.name.clone(),
                        ircord_client::format::parse(&conv.topic).text,
                        conv.kind,
                        conv.joined,
                    ),
                    None => (
                        String::new(),
                        String::new(),
                        ConversationKind::Channel,
                        false,
                    ),
                }
            }
            _ => (
                String::new(),
                String::new(),
                ConversationKind::Channel,
                false,
            ),
        };
        let prefix = if kind == ConversationKind::Channel {
            "#"
        } else {
            "@"
        };
        let header = div()
            .h(px(48.))
            .px_4()
            .flex()
            .items_center()
            .gap_3()
            .border_b_1()
            .border_color(c(theme::RAIL))
            .child(
                div()
                    .flex()
                    .gap_1()
                    .font_weight(FontWeight::BOLD)
                    .text_color(c(theme::TEXT))
                    .child(div().text_color(c(theme::FAINT)).child(prefix))
                    .child(title.clone()),
            )
            .when(!topic.is_empty(), |d| {
                d.child(div().h(px(24.)).w(px(1.)).bg(c(theme::SELECTED)))
                    .child(
                        div()
                            .flex_1()
                            .truncate()
                            .text_sm()
                            .text_color(c(theme::MUTED))
                            .child(topic),
                    )
            });

        let mut main = div()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .bg(c(theme::MAIN))
            .child(header);
        if let Some((text, color)) = self.banner(cx) {
            main = main.child(
                div()
                    .id("connection-banner")
                    .test_support()
                    .role(Role::Status)
                    .aria_label(SharedString::from(text.clone()))
                    .px_4()
                    .py_1()
                    .text_sm()
                    .text_color(c(0xffffff))
                    .bg(c(color))
                    .child(text),
            );
        }
        if let Some(notice) = &self.notice {
            main = main.child(
                div()
                    .id("notice")
                    .px_4()
                    .py_1()
                    .text_sm()
                    .text_color(c(theme::WARNING))
                    .bg(c(theme::SIDEBAR))
                    .child(notice.clone()),
            );
        }
        main = main.child(self.timeline.clone());
        let composer = if can_send {
            div()
                .px_4()
                .pb_4()
                .child(
                    div().rounded_lg().bg(c(theme::INPUT)).px_2().py_1().child(
                        Textarea::new(&self.composer)
                            .appearance(false)
                            .aria_label(SharedString::from(format!("Message {prefix}{title}"))),
                    ),
                )
                .into_any_element()
        } else {
            div()
                .px_4()
                .pb_4()
                .text_sm()
                .text_color(c(theme::MUTED))
                .child("You are not in this conversation.")
                .into_any_element()
        };
        main = main.child(div().id("composer").test_support().child(composer));
        let _ = window;
        div()
            .id("workspace")
            .size_full()
            .flex()
            .flex_row()
            .text_color(c(theme::TEXT))
            .child(self.sidebar.clone())
            .child(main)
            .when(kind == ConversationKind::Channel, |d| {
                d.child(self.members.clone())
            })
    }
}

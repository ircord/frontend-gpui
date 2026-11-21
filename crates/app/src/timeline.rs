//! The timeline: a virtual list of rows that follows the newest message,
//! loads older history when scrolled to the top, and redraws only when its
//! own conversation changes.

use std::ops::Range;
use std::rc::Rc;

use gpui_kit::TestSupportExt;
use gpui_kit::base::SelectableText;
use gpui_kit::prelude::*;
use gpui_kit::*;
use ircord_client::format::{Color as IrcColor, Formatted};
use ircord_client::store::SendState;

use crate::rows::{self, Image, MessageRow, Row};
use crate::session::{Session, SessionEvent};
use crate::theme::{self, c};

pub struct TimelineView {
    session: Entity<Session>,
    conversation: Option<String>,
    rows: Rc<Vec<Row>>,
    keys: Vec<String>,
    list: ListState,
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
    fn rebuild(&mut self, cx: &mut Context<Self>) {
        let session = self.session.read(cx);
        let conversation = session.conversation.clone();
        let rows = match (session.client(), &conversation) {
            (Some(client), Some(id)) => rows::build(&client.read(), id),
            _ => Vec::new(),
        };
        let keys: Vec<String> = rows.iter().map(Row::key).collect();
        if conversation != self.conversation {
            self.conversation = conversation;
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

    fn render_row(
        &mut self,
        ix: usize,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(row) = self.rows.get(ix).cloned() else {
            return div().into_any_element();
        };
        match row {
            Row::Start => div()
                .px_4()
                .pt_8()
                .pb_4()
                .text_color(c(theme::MUTED))
                .text_sm()
                .child("This is the beginning of the history kept for this conversation.")
                .into_any_element(),
            Row::Loading => div()
                .px_4()
                .py_3()
                .text_color(c(theme::MUTED))
                .text_sm()
                .child("Loading…")
                .into_any_element(),
            Row::Day(day) => div()
                .px_4()
                .py_2()
                .flex()
                .items_center()
                .gap_2()
                .child(div().flex_1().h(px(1.)).bg(c(theme::SELECTED)))
                .child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(c(theme::MUTED))
                        .child(day),
                )
                .child(div().flex_1().h(px(1.)).bg(c(theme::SELECTED)))
                .into_any_element(),
            Row::Event(e) => div()
                .px_4()
                .py_0p5()
                .pl(px(72.))
                .flex()
                .gap_2()
                .text_sm()
                .text_color(c(theme::MUTED))
                .child(
                    div()
                        .text_xs()
                        .text_color(c(theme::FAINT))
                        .w(px(40.))
                        .child(e.time.clone()),
                )
                .child(e.text.clone())
                .into_any_element(),
            Row::Pending(p) => {
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
                            .text_color(c(if failed { theme::DANGER } else { theme::FAINT }))
                            .child(if p.action {
                                format!("* {}", p.text)
                            } else {
                                p.text.clone()
                            }),
                    );
                let label = rows::pending_label(&p).to_string();
                let mut status = div()
                    .flex()
                    .gap_2()
                    .text_xs()
                    .text_color(c(theme::MUTED))
                    .child(if failed {
                        format!("Not sent: {label}")
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
                            .aria_label("Retry")
                            .text_color(c(theme::LINK))
                            .cursor_pointer()
                            .child("Retry")
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
            Row::Message(m) => self.render_message(&m, cx),
        }
    }

    fn render_message(&self, m: &MessageRow, _cx: &mut Context<Self>) -> AnyElement {
        let name_color = m.sender_color.map(c).unwrap_or(c(theme::TEXT));
        let mut column = div().flex_1().min_w_0().flex().flex_col().gap_0p5();
        if m.header && !m.action {
            column = column.child(
                div()
                    .flex()
                    .items_baseline()
                    .gap_2()
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(name_color)
                            .child(m.sender.clone()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(c(theme::FAINT))
                            .child(m.time.clone()),
                    ),
            );
        }
        if let Some((nick, text)) = &m.reply {
            let quoted = ircord_client::format::parse(text).text;
            column = column.child(
                div()
                    .text_xs()
                    .text_color(c(theme::MUTED))
                    .truncate()
                    .child(format!("↪ {nick}: {quoted}")),
            );
        }
        column = column.child(self.body(m));
        if !m.reactions.is_empty() {
            let mut chips = div().flex().flex_wrap().gap_1().pt_0p5();
            for (emoji, count) in &m.reactions {
                chips = chips.child(
                    div()
                        .px_1p5()
                        .py_0p5()
                        .rounded_md()
                        .bg(c(theme::SIDEBAR))
                        .text_xs()
                        .text_color(c(theme::TEXT))
                        .child(format!("{emoji} {count}")),
                );
            }
            column = column.child(chips);
        }
        for (i, image) in m.images.iter().enumerate() {
            column = column.child(render_image(&m.id, i, image));
        }
        for (i, link) in m.links.iter().enumerate() {
            let url = link.url.clone();
            column = column.child(
                div()
                    .id(ElementId::Name(format!("link-{}-{i}", m.id).into()))
                    .test_support()
                    .role(Role::Link)
                    .aria_label(SharedString::from(link.url.clone()))
                    .max_w(px(480.))
                    .mt_1()
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .border_l_4()
                    .border_color(c(if link.video {
                        theme::DANGER
                    } else {
                        theme::ACCENT
                    }))
                    .bg(c(theme::SIDEBAR))
                    .cursor_pointer()
                    .hover(|d| d.bg(c(theme::HOVER)))
                    .child(div().text_sm().text_color(c(theme::LINK)).truncate().child(
                        if link.video {
                            format!("▶ {}", link.title)
                        } else {
                            link.title.clone()
                        },
                    ))
                    .child(
                        div()
                            .text_xs()
                            .text_color(c(theme::MUTED))
                            .child(link.detail.clone()),
                    )
                    .on_click(move |_, _, cx| cx.open_url(&url)),
            );
        }

        let gutter = if m.header && !m.action {
            let initial = m
                .sender
                .chars()
                .next()
                .unwrap_or('?')
                .to_uppercase()
                .to_string();
            div()
                .size(px(40.))
                .rounded_full()
                .flex()
                .items_center()
                .justify_center()
                .bg(m.sender_color.map(c).unwrap_or(c(theme::ACCENT)))
                .text_color(c(0xffffff))
                .font_weight(FontWeight::BOLD)
                .child(initial)
        } else {
            div()
                .w(px(40.))
                .text_xs()
                .text_color(c(theme::FAINT))
                .pt_0p5()
                .child(if m.action {
                    m.time.clone()
                } else {
                    String::new()
                })
        };
        div()
            .id(ElementId::Name(format!("message-{}", m.id).into()))
            .test_support()
            .px_4()
            .when(m.header, |d| d.pt_2())
            .py_0p5()
            .flex()
            .gap_4()
            .hover(|d| d.bg(c(0x2e3035)))
            .child(gutter)
            .child(column)
            .into_any_element()
    }

    fn body(&self, m: &MessageRow) -> AnyElement {
        if m.deleted {
            return div()
                .italic()
                .text_color(c(theme::FAINT))
                .child("This message was deleted.")
                .into_any_element();
        }
        let mut text = String::new();
        let mut highlights = Vec::new();
        if m.action {
            text.push_str("* ");
            text.push_str(&m.sender);
            text.push(' ');
            highlights.push((
                0..text.len() - 1,
                HighlightStyle {
                    font_weight: Some(FontWeight::SEMIBOLD),
                    color: m.sender_color.map(c),
                    ..Default::default()
                },
            ));
        }
        let offset = text.len();
        text.push_str(&m.body.text);
        highlights.extend(
            body_highlights(&m.body)
                .into_iter()
                .map(|(r, s)| (r.start + offset..r.end + offset, s)),
        );
        if m.edited {
            let start = text.len();
            text.push_str(" (edited)");
            highlights.push((
                start..text.len(),
                HighlightStyle {
                    color: Some(c(theme::FAINT)),
                    ..Default::default()
                },
            ));
        }
        let color = if m.notice {
            c(theme::MUTED)
        } else {
            c(theme::TEXT)
        };
        div()
            .text_color(color)
            .when(m.action, |d| d.italic())
            .child(
                SelectableText::new(ElementId::Name(format!("text-{}", m.id).into()), text)
                    .highlights(highlights),
            )
            .into_any_element()
    }
}

fn irc_color(color: IrcColor) -> Hsla {
    match color {
        IrcColor::Rgb(rgb) => c(rgb),
        IrcColor::ThemeText => c(theme::TEXT),
        IrcColor::ThemeBackground => c(theme::MAIN),
    }
}

/// Formatting runs and links as non-overlapping highlights, in order.
pub fn body_highlights(f: &Formatted) -> Vec<(Range<usize>, HighlightStyle)> {
    let mut cuts: Vec<usize> = vec![0, f.text.len()];
    for (r, _) in &f.runs {
        cuts.extend([r.start, r.end]);
    }
    for (r, _) in &f.links {
        cuts.extend([r.start, r.end]);
    }
    cuts.sort_unstable();
    cuts.dedup();
    let mut out = Vec::new();
    for pair in cuts.windows(2) {
        let (start, end) = (pair[0], pair[1]);
        if start >= end {
            continue;
        }
        let mut style = HighlightStyle::default();
        let mut styled = false;
        if let Some((_, s)) = f
            .runs
            .iter()
            .find(|(r, _)| r.start <= start && end <= r.end)
        {
            styled = true;
            if s.bold {
                style.font_weight = Some(FontWeight::BOLD);
            }
            if s.italic {
                style.font_style = Some(FontStyle::Italic);
            }
            if s.underline {
                style.underline = Some(UnderlineStyle {
                    thickness: px(1.),
                    ..Default::default()
                });
            }
            if s.strike {
                style.strikethrough = Some(StrikethroughStyle {
                    thickness: px(1.),
                    ..Default::default()
                });
            }
            style.color = s.fg.map(irc_color);
            style.background_color = s.bg.map(irc_color).or(s.mono.then(|| c(theme::RAIL)));
        }
        if f.links
            .iter()
            .any(|(r, _)| r.start <= start && end <= r.end)
        {
            styled = true;
            style.color = Some(c(theme::LINK));
            style.underline = Some(UnderlineStyle {
                thickness: px(1.),
                ..Default::default()
            });
        }
        if styled {
            out.push((start..end, style));
        }
    }
    out
}

fn render_image(message: &str, i: usize, image: &Image) -> impl IntoElement {
    let (max_w, max_h) = (400.0_f32, 300.0_f32);
    let (w, h) = if image.width > 0 && image.height > 0 {
        let scale = (max_w / image.width as f32)
            .min(max_h / image.height as f32)
            .min(1.0);
        (image.width as f32 * scale, image.height as f32 * scale)
    } else {
        (max_w, max_h * 0.66)
    };
    let url = image.url.clone();
    div()
        .id(ElementId::Name(format!("image-{message}-{i}").into()))
        .role(Role::Image)
        .aria_label(SharedString::from(if image.alt.is_empty() {
            image.url.clone()
        } else {
            image.alt.clone()
        }))
        .mt_1()
        .w(px(w))
        .h(px(h))
        .rounded_md()
        .overflow_hidden()
        .bg(c(theme::SIDEBAR))
        .cursor_pointer()
        .child(
            img(SharedUri::from(image.url.clone()))
                .size_full()
                .object_fit(ObjectFit::Contain),
        )
        .on_click(move |_, _, cx| cx.open_url(&url))
}

impl Render for TimelineView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let session = self.session.read(cx);
        let typing = match (session.client(), &self.conversation) {
            (Some(client), Some(id)) => {
                client.read().typing_in(id, ircord_client::events::now_ms())
            }
            _ => Vec::new(),
        };
        let typing = match typing.as_slice() {
            [] => String::new(),
            [one] => format!("{one} is typing…"),
            [a, b] => format!("{a} and {b} are typing…"),
            _ => "Several people are typing…".to_string(),
        };
        div()
            .id("timeline")
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
                    .text_color(c(theme::MUTED))
                    .child(typing),
            )
    }
}

//! How a message row is laid out (package S2): the frame around the body,
//! reactions, attachments and link cards, and the clock its times use.

use chrono::{DateTime, Local};
use gpui_kit::TestSupportExt;
use gpui_kit::component::menu::ContextMenuExt;
use gpui_kit::prelude::*;
use gpui_kit::*;
use ircord_client::store::Workspace;

use super::{MessageView, attachments, body, links, reactions, toolbar};
use crate::registry::{self, MemberCtx};
use crate::theme::{self, Theme};
use crate::ui;

/// A message time as the clock setting shows it.
pub fn time_label(_w: &Workspace, at: &DateTime<Local>) -> String {
    at.format("%H:%M").to_string()
}

/// A fingerprint of what the timeline's rows depend on from the message
/// settings: when it changes, the timeline measures every row again.
pub fn look(_cx: &App) -> u64 {
    0
}

pub fn render(view: MessageView, theme: &Theme, window: &mut Window, cx: &mut App) -> AnyElement {
    let m = &view.row;
    let name_color = m.sender_color.map(theme::hex).unwrap_or(theme.text);
    // A click on the name or picture opens the person (`Registry::open_person`).
    let open_person = registry::get(cx).open_person.clone().map(|open| {
        let ctx = MemberCtx {
            session: view.ctx.session.clone(),
            member: m.sender_id.clone(),
            conversation: Some(view.ctx.conversation.clone()),
        };
        move |_: &ClickEvent, window: &mut Window, cx: &mut App| open(&ctx, window, cx)
    });
    let person = |el: Div, id: String, label: String| -> AnyElement {
        match open_person.clone() {
            Some(open) => el
                .id(ElementId::Name(id.into()))
                .test_support()
                .role(Role::Link)
                .aria_label(SharedString::from(label))
                .cursor_pointer()
                .on_click(open)
                .into_any_element(),
            None => el.into_any_element(),
        }
    };
    let mut column = div().flex_1().min_w_0().flex().flex_col().gap_0p5();
    if m.header && !m.action {
        column = column.child(
            div()
                .flex()
                .items_baseline()
                .gap_2()
                .child(person(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(name_color)
                        .hover(|d| d.underline())
                        .child(m.sender.clone()),
                    format!("sender-{}", m.id),
                    m.sender.clone(),
                ))
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.faint)
                        .child(m.time.clone()),
                ),
        );
    }
    if let Some((nick, text)) = &m.reply {
        let quoted = ircord_client::format::parse(text).text;
        let line = t!("core-message-reply-line", nick = nick, text = quoted);
        let hover_text = theme.text;
        let mut reply = div()
            .id(ElementId::Name(format!("reply-{}", m.id).into()))
            .test_support()
            .text_xs()
            .text_color(theme.muted)
            .truncate()
            .child(line.clone());
        // Clicking the quote goes to the original (`timeline::reveal`).
        if let Some(original) = m.reply_id.clone() {
            let ctx = view.ctx.clone();
            reply = reply
                .role(Role::Link)
                .aria_label(line)
                .cursor_pointer()
                .hover(move |d| d.text_color(hover_text))
                .on_click(move |_, window, cx| {
                    crate::timeline::reveal(&ctx.session, &ctx.conversation, &original, window, cx)
                });
        }
        column = column.child(reply);
    }
    let inline = registry::get(cx).inline_renderers.to_vec();
    // A message of uploads only has no text left to draw.
    if !(m.body.text.trim().is_empty() && !m.attachments.is_empty()) {
        column = column.child(body::render(m, &view.ctx, theme, &inline, window, cx));
    }
    column = column
        .children(reactions::render(m, &view.ctx, theme, window, cx))
        .children(attachments::render(m, &view.ctx, theme, window, cx))
        .children(links::render(m, &view.ctx, theme, window, cx));

    let gutter = if m.header && !m.action {
        person(
            ui::avatar::avatar(&m.sender, m.sender_color.map(theme::hex), px(40.), theme),
            format!("sender-avatar-{}", m.id),
            m.sender.clone(),
        )
    } else {
        div()
            .w(px(40.))
            .flex_none()
            .text_xs()
            .text_color(theme.faint)
            .pt_0p5()
            .child(if m.action {
                m.time.clone()
            } else {
                String::new()
            })
            .into_any_element()
    };

    let on_hover = view.on_hover.clone();
    let row_hover = theme.row_hover;
    let mut frame = div()
        .id(ElementId::Name(format!("message-{}", m.id).into()))
        .test_support()
        .relative()
        .w_full()
        .px_4()
        .when(m.header, |d| d.pt_2())
        .py_0p5()
        .flex()
        .gap_4()
        .when(m.mention || m.focused, |d| {
            let mark = if m.focused {
                theme.accent
            } else {
                theme.mention
            };
            d.bg(mark.opacity(0.12)).border_l_2().border_color(mark)
        })
        .hover(move |d| d.bg(row_hover))
        .on_hover(move |over: &bool, window, cx| on_hover(*over, window, cx))
        .child(gutter)
        .child(column);
    if view.hovered {
        frame = frame.child(toolbar(&view.ctx, theme, window, cx));
    }
    let ctx = view.ctx.clone();
    frame
        .context_menu(move |menu, _, cx| {
            let items = registry::get(cx).message_menu.to_vec();
            let items = ui::context_menu::visible(&items, &ctx, cx);
            ui::context_menu::fill(menu, items, ctx.clone())
        })
        .into_any_element()
}

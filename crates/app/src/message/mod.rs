//! One message row: the frame (avatar, name and time, reply line) around
//! the body, reactions, attachments and link cards, each in its own file.
//! The frame also hosts the registered message menu (right click) and the
//! hover toolbar (registered items marked `toolbar`).

pub mod attachments;
pub mod body;
pub mod emoji;
pub mod layout;
pub mod links;
pub mod reactions;

use std::rc::Rc;

use gpui_kit::TestSupportExt;
use gpui_kit::component::menu::ContextMenuExt;
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::registry::{self, MessageCtx};
use crate::theme::{self, Theme};
use crate::timeline::rows::MessageRow;
use crate::ui;

pub struct MessageView {
    pub row: Rc<MessageRow>,
    pub ctx: MessageCtx,
    /// Under the pointer: show the toolbar.
    pub hovered: bool,
    #[allow(clippy::type_complexity)]
    pub on_hover: Rc<dyn Fn(bool, &mut Window, &mut App)>,
}

pub fn render(view: MessageView, theme: &Theme, window: &mut Window, cx: &mut App) -> AnyElement {
    let m = &view.row;
    let name_color = m.sender_color.map(theme::hex).unwrap_or(theme.text);
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
                        .text_color(theme.faint)
                        .child(m.time.clone()),
                ),
        );
    }
    if let Some((nick, text)) = &m.reply {
        let quoted = ircord_client::format::parse(text).text;
        column = column.child(div().text_xs().text_color(theme.muted).truncate().child(t!(
            "core-message-reply-line",
            nick = nick,
            text = quoted
        )));
    }
    let inline = registry::get(cx).inline_renderers.to_vec();
    // A message of uploads only has no text left to draw.
    if !(m.body.text.trim().is_empty() && !m.attachments.is_empty()) {
        column = column.child(body::render(m, theme, &inline));
    }
    column = column
        .children(reactions::render(m, theme))
        .children(attachments::render(m, &view.ctx, theme, window, cx))
        .children(links::render(m, theme));

    let gutter = if m.header && !m.action {
        ui::avatar::avatar(&m.sender, m.sender_color.map(theme::hex), px(40.), theme)
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
    };

    let on_hover = view.on_hover.clone();
    let row_hover = theme.row_hover;
    let mut frame = div()
        .id(ElementId::Name(format!("message-{}", m.id).into()))
        .test_support()
        .relative()
        .px_4()
        .when(m.header, |d| d.pt_2())
        .py_0p5()
        .flex()
        .gap_4()
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

/// The registered toolbar actions over the message under the pointer.
fn toolbar(ctx: &MessageCtx, theme: &Theme, _window: &mut Window, cx: &mut App) -> AnyElement {
    let items = registry::get(cx).message_menu.to_vec();
    let items: Vec<_> = ui::context_menu::visible(&items, ctx, cx)
        .into_iter()
        .filter(|i| i.toolbar)
        .collect();
    if items.is_empty() {
        return div().into_any_element();
    }
    let mut bar = div()
        .id("message-toolbar")
        .test_support()
        .role(Role::Toolbar)
        .aria_label(t!("core-message-toolbar"))
        .absolute()
        .top(px(-12.))
        .right(px(16.))
        .flex()
        .gap_0p5()
        .p_0p5()
        .rounded_md()
        .border_1()
        .border_color(theme.rail)
        .bg(theme.sidebar);
    for item in items {
        let (run, ctx) = (item.run.clone(), ctx.clone());
        let icon = item.icon.clone().unwrap_or(ui::icon::IconName::Ellipsis);
        bar = bar.child(ui::icon::button(
            ElementId::Name(format!("message-toolbar-{}", item.id).into()),
            icon,
            item.label.clone(),
            theme,
            move |_, window, cx| run(&ctx, window, cx),
        ));
    }
    bar.into_any_element()
}

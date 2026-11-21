//! Link cards: files that are not pictures, and links in the text. Each
//! opens in the browser; videos and players are marked as such.

use gpui_kit::TestSupportExt;
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::registry::MessageCtx;
use crate::theme::Theme;
use crate::timeline::rows::MessageRow;

pub fn render(
    m: &MessageRow,
    _ctx: &MessageCtx,
    theme: &Theme,
    _window: &mut Window,
    _cx: &mut App,
) -> Vec<AnyElement> {
    let mut out = Vec::new();
    for (i, link) in m.links.iter().enumerate() {
        let url = link.url.clone();
        let hover = theme.hover;
        out.push(
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
                .border_color(if link.video {
                    theme.danger
                } else {
                    theme.accent
                })
                .bg(theme.sidebar)
                .cursor_pointer()
                .hover(move |d| d.bg(hover))
                .child(
                    div()
                        .text_sm()
                        .text_color(theme.link)
                        .truncate()
                        .child(if link.video {
                            t!("core-message-link-video-title", title = link.title)
                        } else {
                            link.title.clone()
                        }),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted)
                        .child(link.detail.clone()),
                )
                .on_click(move |_, _, cx| cx.open_url(&url))
                .into_any_element(),
        );
    }
    out
}

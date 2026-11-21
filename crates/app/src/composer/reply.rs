//! The bar above the box (package A1): "Replying to …" or "Editing your
//! message" with a cancel button, and each failed send with buttons to put
//! it back in the box or discard it.

use gpui_kit::TestSupportExt;
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::features::message_actions::{self, Mode};
use crate::registry::ComposerCtx;
use crate::theme;
use crate::ui;
use crate::ui::icon::IconName;

pub fn render(ctx: &ComposerCtx, _window: &mut Window, cx: &mut App) -> Option<AnyElement> {
    let theme = theme::get(cx).clone();
    let mode = message_actions::mode(&ctx.conversation, cx);
    let failed = message_actions::failed_sends(&ctx.conversation, cx);
    if mode.is_none() && failed.is_empty() {
        return None;
    }
    let mut column = div().flex().flex_col().gap_1().mb_1();
    for p in failed {
        let preview = if p.action {
            format!("* {}", p.text)
        } else {
            p.text.clone()
        };
        let (to_box, discard) = (p.client_id.clone(), p.client_id.clone());
        column = column.child(
            div()
                .id(ElementId::Name(
                    format!("failed-send-{}", p.client_id).into(),
                ))
                .test_support()
                .role(Role::Group)
                .aria_label(t!("message-actions-not-sent", text = preview))
                .px_2()
                .py_1()
                .rounded_md()
                .bg(theme.sidebar)
                .flex()
                .items_center()
                .gap_2()
                .text_sm()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_color(theme.danger)
                        .child(t!("message-actions-not-sent", text = preview)),
                )
                .child(link_button(
                    format!("failed-send-to-box-{}", p.client_id),
                    t!("message-actions-to-box"),
                    &theme,
                    move |window, cx| message_actions::discard(&to_box, true, window, cx),
                ))
                .child(link_button(
                    format!("failed-send-discard-{}", p.client_id),
                    t!("message-actions-discard"),
                    &theme,
                    move |window, cx| message_actions::discard(&discard, false, window, cx),
                )),
        );
    }
    if let Some(mode) = mode {
        let label = match &mode {
            Mode::Reply { nick, .. } => t!("message-actions-replying", nick = nick),
            Mode::Edit { .. } => t!("message-actions-editing").to_string(),
        };
        let quoted = match &mode {
            Mode::Reply { text, .. } => Some(text.clone()),
            Mode::Edit { .. } => None,
        };
        let conversation = ctx.conversation.clone();
        column = column.child(
            div()
                .id("composer-mode")
                .test_support()
                .role(Role::Status)
                .aria_label(label.clone())
                .px_2()
                .py_1()
                .rounded_md()
                .bg(theme.sidebar)
                .flex()
                .items_center()
                .gap_2()
                .text_sm()
                .child(
                    div()
                        .flex_none()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.text)
                        .child(label),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_color(theme.muted)
                        .child(quoted.unwrap_or_else(|| t!("message-actions-escape-hint").into())),
                )
                .child(ui::icon::button(
                    "composer-mode-cancel",
                    IconName::Close,
                    t!("message-actions-cancel"),
                    &theme,
                    move |_, window, cx| message_actions::cancel(&conversation, window, cx),
                )),
        );
    }
    Some(column.into_any_element())
}

fn link_button(
    id: String,
    label: &'static str,
    theme: &theme::Theme,
    run: impl Fn(&mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id(ElementId::Name(id.into()))
        .test_support()
        .role(Role::Button)
        .aria_label(label)
        .flex_none()
        .text_color(theme.link)
        .cursor_pointer()
        .child(label)
        .on_click(move |_, window, cx| run(window, cx))
}

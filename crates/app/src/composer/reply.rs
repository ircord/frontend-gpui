//! The bar above the box (package A1): "Replying to …" or "Editing your
//! message" with a cancel button. Failed sends are not listed here: they
//! stay in the timeline with their own buttons, as in the web client.

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
    let mode = message_actions::mode(&ctx.conversation, cx)?;
    let label = match &mode {
        Mode::Reply { nick, .. } => t!("message-actions-replying", nick = nick),
        Mode::Edit { .. } => t!("message-actions-editing").to_string(),
    };
    let quoted = match &mode {
        Mode::Reply { text, .. } => Some(text.clone()),
        Mode::Edit { .. } => None,
    };
    let conversation = ctx.conversation.clone();
    Some(
        div()
            .id("composer-mode")
            .test_support()
            .role(Role::Status)
            .aria_label(label.clone())
            .px_2()
            .py_1()
            .mb_1()
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
            ))
            .into_any_element(),
    )
}

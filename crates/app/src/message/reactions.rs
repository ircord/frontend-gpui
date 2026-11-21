//! The reaction chips under a message. They get the message's context, so
//! a chip toggles your reaction (package A1); yours are marked.

use crate::ui::focus::Reachable;
use gpui_kit::TestSupportExt;
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::features::message_actions;
use crate::registry::MessageCtx;
use crate::theme::Theme;
use crate::timeline::rows::MessageRow;

pub fn render(
    m: &MessageRow,
    ctx: &MessageCtx,
    theme: &Theme,
    _window: &mut Window,
    cx: &mut App,
) -> Option<AnyElement> {
    if m.reactions.is_empty() {
        return None;
    }
    let mine = message_actions::my_reactions(ctx, cx);
    let mut chips = div()
        .id(ElementId::Name(format!("reactions-{}", m.id).into()))
        .test_support()
        .role(Role::Group)
        .aria_label(t!("message-actions-reactions"))
        .flex()
        .flex_wrap()
        .gap_1()
        .pt_0p5();
    for (emoji, count) in &m.reactions {
        let own = mine.contains(emoji);
        let (ctx, react) = (ctx.clone(), emoji.clone());
        chips = chips.child(
            div()
                .id(ElementId::Name(format!("reaction-{}-{emoji}", m.id).into()))
                .test_support()
                .role(Role::Button)
                .reachable(theme)
                .aria_label(t!(
                    "message-actions-reaction-label",
                    emoji = emoji,
                    count = count
                ))
                .aria_selected(own)
                .px_1p5()
                .py_0p5()
                .rounded_md()
                .border_1()
                .border_color(if own { theme.accent } else { theme.sidebar })
                .bg(if own { theme.selected } else { theme.sidebar })
                .text_xs()
                .text_color(theme.text)
                .cursor_pointer()
                .hover({
                    let hover = theme.hover;
                    move |d| d.bg(hover)
                })
                .child(t!(
                    "message-actions-reaction-chip",
                    emoji = emoji,
                    count = count
                ))
                .on_click(move |_, window, cx| {
                    message_actions::react(
                        &ctx.session,
                        &ctx.conversation,
                        &ctx.message,
                        &react,
                        window,
                        cx,
                    )
                }),
        );
    }
    Some(chips.into_any_element())
}

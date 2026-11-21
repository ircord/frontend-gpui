//! The reaction chips under a message.

use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::theme::Theme;
use crate::timeline::rows::MessageRow;

pub fn render(m: &MessageRow, theme: &Theme) -> Option<AnyElement> {
    if m.reactions.is_empty() {
        return None;
    }
    let mut chips = div().flex().flex_wrap().gap_1().pt_0p5();
    for (emoji, count) in &m.reactions {
        chips = chips.child(
            div()
                .px_1p5()
                .py_0p5()
                .rounded_md()
                .bg(theme.sidebar)
                .text_xs()
                .text_color(theme.text)
                .child(format!("{emoji} {count}")),
        );
    }
    Some(chips.into_any_element())
}

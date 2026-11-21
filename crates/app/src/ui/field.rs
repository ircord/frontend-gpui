//! A form field: a small capitalised label over its control.

use gpui_kit::prelude::*;
use gpui_kit::*;

pub fn field(
    label: impl Into<SharedString>,
    control: impl IntoElement,
    theme: &crate::theme::Theme,
) -> Div {
    let label: SharedString = label.into();
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .text_xs()
                .font_weight(FontWeight::BOLD)
                .text_color(theme.muted)
                .child(label.to_uppercase()),
        )
        .child(control)
}

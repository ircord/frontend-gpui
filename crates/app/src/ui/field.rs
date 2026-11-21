//! A form field: a small label in sentence case, medium weight, over its
//! control, as the web's forms.

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
                .text_sm()
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme.text)
                .child(label),
        )
        .child(control)
}

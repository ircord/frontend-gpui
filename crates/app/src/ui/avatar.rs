//! A person's round initial (pictures come with profiles, package P1).

use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::theme::Theme;

/// A disc `size` across with the first letter of `name` on `color`.
pub fn avatar(name: &str, color: Option<Hsla>, size: Pixels, theme: &Theme) -> Div {
    let initial = name
        .chars()
        .next()
        .unwrap_or('?')
        .to_uppercase()
        .to_string();
    div()
        .size(size)
        .flex_none()
        .rounded_full()
        .flex()
        .items_center()
        .justify_center()
        .bg(color.unwrap_or(theme.accent))
        .text_color(theme.on_accent)
        .font_weight(FontWeight::BOLD)
        .child(initial)
}

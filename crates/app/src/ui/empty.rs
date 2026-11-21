//! What a page shows when it has nothing to show, as the web's
//! `EmptyState.vue`: an optional icon in a tinted tile, the headline and a
//! line under it.

use gpui_kit::TestSupportExt;
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::theme::Theme;
use crate::ui::icon::IconName;

pub fn empty(
    id: impl Into<ElementId>,
    title: impl Into<SharedString>,
    body: Option<SharedString>,
    theme: &Theme,
) -> impl IntoElement {
    with_icon(id, None, title, body, theme)
}

/// `empty` with `icon` in an accent-tinted tile above the headline.
pub fn with_icon(
    id: impl Into<ElementId>,
    icon: Option<IconName>,
    title: impl Into<SharedString>,
    body: Option<SharedString>,
    theme: &Theme,
) -> impl IntoElement {
    let title: SharedString = title.into();
    let tile = icon.map(|icon| {
        let accent = theme.accent;
        div()
            .mb_1()
            .p_4()
            .rounded_2xl()
            .border_1()
            .border_color(accent.opacity(0.22))
            .bg(accent.opacity(0.12))
            .child(
                gpui_kit::component::Icon::new(icon)
                    .size(px(26.))
                    .text_color(accent),
            )
    });
    div()
        .id(id)
        .test_support()
        .role(Role::Status)
        .aria_label(title.clone())
        .size_full()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap_2()
        .p_8()
        .children(tile)
        .child(
            div()
                .text_lg()
                .font_weight(FontWeight::BOLD)
                .text_color(theme.text)
                .text_center()
                .child(title),
        )
        .when_some(body, |d, body| {
            d.child(
                div()
                    .max_w(px(460.))
                    .text_sm()
                    .text_center()
                    .text_color(theme.muted)
                    .child(body),
            )
        })
}

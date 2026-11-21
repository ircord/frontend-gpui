//! What a page shows when it has nothing to show.

use gpui_kit::TestSupportExt;
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::theme::Theme;

pub fn empty(
    id: impl Into<ElementId>,
    title: impl Into<SharedString>,
    body: Option<SharedString>,
    theme: &Theme,
) -> impl IntoElement {
    let title: SharedString = title.into();
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
        .child(
            div()
                .text_lg()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.text)
                .child(title),
        )
        .when_some(body, |d, body| {
            d.child(div().text_sm().text_color(theme.muted).child(body))
        })
}

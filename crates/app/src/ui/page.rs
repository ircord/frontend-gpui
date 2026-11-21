//! The frame of a full page in the main pane (search, inbox, settings): a
//! header with the title and actions over a scrolling body.

use gpui_kit::TestSupportExt;
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::theme::Theme;

pub struct Page {
    pub id: SharedString,
    pub title: SharedString,
    pub actions: Vec<AnyElement>,
    pub body: AnyElement,
}

impl Page {
    pub fn new(
        id: impl Into<SharedString>,
        title: impl Into<SharedString>,
        body: impl IntoElement,
    ) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            actions: Vec::new(),
            body: body.into_any_element(),
        }
    }

    pub fn action(mut self, action: impl IntoElement) -> Self {
        self.actions.push(action.into_any_element());
        self
    }

    pub fn render(self, theme: &Theme) -> impl IntoElement + use<> {
        let header = div()
            .h(px(48.))
            .flex_none()
            .px_4()
            .flex()
            .items_center()
            .justify_between()
            .border_b_1()
            .border_color(theme.rail)
            .child(
                div()
                    .font_weight(FontWeight::BOLD)
                    .text_color(theme.text)
                    .child(self.title.clone()),
            )
            .child(div().flex().gap_1().children(self.actions));
        div()
            .id(ElementId::Name(self.id.clone()))
            .test_support()
            .role(Role::Region)
            .aria_label(self.title)
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .bg(theme.main)
            .child(header)
            .child(
                div()
                    .id(ElementId::Name(format!("{}-body", self.id).into()))
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(self.body),
            )
    }
}

//! The frame of a full page in the main pane (search, inbox, settings): a
//! header with the title and actions over a scrolling body.
//!
//! A page with a description gets the web client's large-title header
//! (`WorkspacePage.vue`): the title large, the description under it, a count
//! chip and a close button at the right, and a bar of key hints along the
//! bottom. A page without one keeps the compact header strip.

use gpui_kit::TestSupportExt;
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::route::Route;
use crate::session::Session;
use crate::theme::Theme;
use crate::ui::focus::Reachable;
use crate::ui::icon::IconName;

/// One hint in the bar along the bottom: keys, then what they do.
pub struct Hint {
    pub keys: Vec<SharedString>,
    pub text: SharedString,
}

impl Hint {
    pub fn new(keys: &[&'static str], text: &'static str) -> Self {
        Self {
            keys: keys.iter().map(|k| SharedString::from(*k)).collect(),
            text: text.into(),
        }
    }
}

type OnClose = Box<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

pub struct Page {
    pub id: SharedString,
    pub title: SharedString,
    pub description: Option<SharedString>,
    /// The count chip beside the close button, with its test ID.
    pub meta: Option<(SharedString, SharedString)>,
    pub actions: Vec<AnyElement>,
    pub close: Option<(SharedString, OnClose)>,
    pub hints: Vec<Hint>,
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
            description: None,
            meta: None,
            actions: Vec::new(),
            close: None,
            hints: Vec::new(),
            body: body.into_any_element(),
        }
    }

    pub fn action(mut self, action: impl IntoElement) -> Self {
        self.actions.push(action.into_any_element());
        self
    }

    /// The line under the title; gives the page the large-title header.
    pub fn description(mut self, text: impl Into<SharedString>) -> Self {
        self.description = Some(text.into());
        self
    }

    /// The count chip, drawn with the status role and `id`.
    pub fn meta(mut self, id: impl Into<SharedString>, text: impl Into<SharedString>) -> Self {
        self.meta = Some((id.into(), text.into()));
        self
    }

    /// The round close button at the top right, named `label`; its test ID
    /// is `{id}-close`.
    pub fn close(
        mut self,
        label: impl Into<SharedString>,
        on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.close = Some((label.into(), Box::new(on_click)));
        self
    }

    /// The key hints along the bottom. Name only keys the page handles.
    pub fn hints(mut self, hints: Vec<Hint>) -> Self {
        self.hints = hints;
        self
    }

    pub fn render(self, theme: &Theme) -> impl IntoElement + use<> {
        let large = self.description.is_some();
        let meta = self.meta.map(|(id, text)| {
            div()
                .id(ElementId::Name(id))
                .test_support()
                .role(Role::Status)
                .aria_label(text.clone())
                .flex_none()
                .px_3()
                .py_1()
                .rounded_full()
                .border_1()
                .border_color(theme.border)
                .bg(theme.sidebar)
                .text_xs()
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme.muted)
                .child(text)
                .into_any_element()
        });
        let close = self.close.map(|(label, on_click)| {
            let (hover, text) = (theme.hover, theme.text);
            div()
                .id(ElementId::Name(format!("{}-close", self.id).into()))
                .test_support()
                .role(Role::Button)
                .reachable(theme)
                .aria_label(label.clone())
                .tooltip(super::tooltip::text(label))
                .flex_none()
                .size(px(30.))
                .rounded_full()
                .flex()
                .items_center()
                .justify_center()
                .border_1()
                .border_color(theme.border)
                .bg(theme.sidebar)
                .cursor_pointer()
                .text_color(theme.muted)
                .hover(move |d| d.bg(hover).text_color(text))
                .child(super::icon::icon(IconName::Close, theme.muted))
                .on_click(on_click)
                .into_any_element()
        });
        let side = div()
            .flex()
            .flex_none()
            .items_center()
            .gap_1p5()
            .children(meta)
            .children(self.actions)
            .children(close);
        let header = if large {
            div()
                .flex_none()
                .px_4()
                .pt_5()
                .pb_4()
                .flex()
                .items_start()
                .gap_3()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap_1p5()
                        .child(
                            div()
                                .id(ElementId::Name(format!("{}-title", self.id).into()))
                                .test_support()
                                .role(Role::Heading)
                                .aria_label(self.title.clone())
                                .truncate()
                                .text_size(px(30.))
                                .line_height(relative(1.1))
                                .font_weight(FontWeight::EXTRA_BOLD)
                                .text_color(theme.text)
                                .child(self.title.clone()),
                        )
                        .children(self.description.map(|d| {
                            div()
                                .text_sm()
                                .line_clamp(2)
                                .text_color(theme.muted)
                                .child(d)
                        })),
                )
                .child(side)
        } else {
            div()
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
                .child(side)
        };
        let footer = (!self.hints.is_empty())
            .then(|| hint_bar(format!("{}-hints", self.id), &self.hints, theme));
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
            .children(footer)
    }
}

/// The bar of key hints, as the web's `WorkspaceFooter`: each key in a
/// small framed cap, then what it does.
pub fn hint_bar(id: String, hints: &[Hint], theme: &Theme) -> impl IntoElement + use<> {
    let text: Vec<String> = hints
        .iter()
        .map(|h| format!("{} {}", h.keys.join(" "), h.text))
        .collect();
    div()
        .id(ElementId::Name(id.into()))
        .test_support()
        .role(Role::Note)
        .aria_label(SharedString::from(text.join(", ")))
        .flex_none()
        .px_4()
        .py_2p5()
        .flex()
        .flex_wrap()
        .items_center()
        .gap_x_5()
        .gap_y_2()
        .border_t_1()
        .border_color(theme.border)
        .text_xs()
        .text_color(theme.muted)
        .children(hints.iter().map(|h| {
            div()
                .flex()
                .items_center()
                .gap_1()
                .children(h.keys.iter().map(|k| key_cap(k.clone(), theme)))
                .child(div().ml_0p5().child(h.text.clone()))
        }))
}

fn key_cap(key: SharedString, theme: &Theme) -> impl IntoElement {
    div()
        .h(px(18.))
        .min_w(px(18.))
        .px(px(5.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(4.))
        .border_1()
        .border_color(theme.border)
        .bg(theme.raised)
        .text_size(px(11.))
        .text_color(theme.muted)
        .child(key)
}

/// Leave a page for the conversation that was open, as the web's close
/// button and Escape do; with none open, step back.
pub fn leave(session: &Entity<Session>, cx: &mut App) {
    session.update(cx, |s, cx| match s.conversation.clone() {
        Some(id) => s.navigate(Route::Conversation(id), cx),
        None => s.go_back(cx),
    });
}

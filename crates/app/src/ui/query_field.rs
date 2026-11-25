//! Shared tagged-query controls for Search and Links. The page owns token
//! parsing and decides how to apply a suggestion.

use chrono::NaiveDate;
use gpui_kit::TestSupportExt;
use gpui_kit::component::calendar::Matcher;
use gpui_kit::component::date_picker::{DatePicker, DatePickerState};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::{Sizable, Size};
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::theme::Theme;
use crate::ui::{focus::Reachable, icon};

pub(crate) const SUGGESTION_KEYS: &str = "IrcordQuerySuggestions";
gpui_kit::actions!(ircord_query, [AcceptSuggestion]);

pub(crate) fn input(
    id: &'static str,
    state: &Entity<InputState>,
    label: impl Into<SharedString>,
    chips: Option<Div>,
    theme: &Theme,
) -> Input {
    Input::new(state)
        .id(id)
        .aria_label(label)
        .with_size(Size::Large)
        .bg(theme.raised)
        .border_color(theme.border)
        .prefix(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(icon::icon(icon::IconName::Search, theme.muted))
                .children(chips),
        )
}

pub(crate) fn chip(
    id: String,
    label: String,
    key: &str,
    value: String,
    theme: &Theme,
    remove: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id(ElementId::Name(id.into()))
        .test_support()
        .role(Role::Button)
        .reachable(theme)
        .aria_label(SharedString::from(label))
        .flex()
        .items_center()
        .gap_1()
        .px_2()
        .py_0p5()
        .rounded(theme.radius)
        .cursor_pointer()
        .bg(theme.selected)
        .text_sm()
        .child(div().text_color(theme.accent).child(format!("{key}:")))
        .child(div().text_color(theme.text).child(value))
        .child(icon::icon(icon::IconName::Close, theme.muted))
        .on_click(remove)
}

pub(crate) fn shortcut(
    id: impl Into<ElementId>,
    label: &'static str,
    accessible_name: &'static str,
    leading: Option<AnyElement>,
    theme: &Theme,
    start: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id(id)
        .test_support()
        .role(Role::Button)
        .reachable(theme)
        .aria_label(accessible_name)
        .h(px(28. * theme.scale))
        .px_3()
        .rounded_full()
        .border_1()
        .border_color(theme.border.opacity(0.55))
        .bg(theme.raised.opacity(0.45))
        .hover({
            let (hover, text) = (theme.hover, theme.text);
            move |d| d.bg(hover).text_color(text)
        })
        .cursor_pointer()
        .flex()
        .items_center()
        .gap_1p5()
        .text_xs()
        .text_color(theme.muted)
        .children(leading)
        .child(label)
        .on_click(start)
}

#[derive(Default)]
pub(crate) struct Selection {
    index: usize,
    pub(crate) dismissed: bool,
    scroll: SelectionScroll,
}

impl Selection {
    pub(crate) fn index(&self, count: usize) -> usize {
        self.index.min(count.saturating_sub(1))
    }

    pub(crate) fn step(&mut self, count: usize, backward: bool) {
        let offset = if backward { count - 1 } else { 1 };
        self.index = (self.index(count) + offset) % count;
        self.scroll.reveal = true;
    }
}

#[derive(Default)]
pub(crate) struct SelectionScroll {
    pub(crate) handle: ScrollHandle,
    pub(crate) reveal: bool,
}

impl SelectionScroll {
    pub(crate) fn reset(&mut self) {
        self.reveal = false;
        self.handle.set_offset(Point::default());
    }

    pub(crate) fn reveal_row(&self, selected: bool) -> Option<impl IntoElement + use<>> {
        if !self.reveal || !selected {
            return None;
        }
        let handle = self.handle.clone();
        Some(
            canvas(
                move |row, window, _| {
                    let viewport = handle.bounds();
                    let old = handle.offset();
                    let delta =
                        if row.top() < viewport.top() || row.size.height > viewport.size.height {
                            viewport.top() - row.top()
                        } else {
                            (viewport.bottom() - row.bottom()).min(px(0.))
                        };
                    let y = (old.y + delta).clamp(-handle.max_offset().y, px(0.));
                    if y != old.y {
                        handle.set_offset(point(old.x, y));
                        // The current frame already laid out the viewport.
                        window.on_next_frame(|window, _| window.refresh());
                    }
                },
                |_, _, _, _| {},
            )
            .absolute()
            .top_0()
            .left_0()
            .size_full(),
        )
    }
}

pub(crate) struct Suggestion {
    pub(crate) title: String,
    pub(crate) detail: Option<String>,
    pub(crate) leading: Option<AnyElement>,
}

impl Suggestion {
    pub(crate) fn render(
        self,
        prefix: &str,
        index: usize,
        count: usize,
        selection: &Selection,
        theme: &Theme,
        choose: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> impl IntoElement {
        let selected = index == selection.index(count);
        let name = match &self.detail {
            Some(detail) => format!("{} {detail}", self.title),
            None => self.title.clone(),
        };
        div()
            .id(ElementId::Name(format!("{prefix}-offer-{index}").into()))
            .test_support()
            .role(Role::ListBoxOption)
            .reachable(theme)
            .aria_label(SharedString::from(name))
            .aria_selected(selected)
            .flex()
            .items_center()
            .gap_2p5()
            .min_h(px(36. * theme.scale))
            .px_2()
            .py_1p5()
            .rounded(theme.radius)
            .cursor_pointer()
            .when(selected, |d| d.bg(theme.hover))
            .hover({
                let hover = theme.hover;
                move |d| d.bg(hover)
            })
            .children(self.leading)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .truncate()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme.text)
                            .child(self.title),
                    )
                    .children(self.detail.map(|detail| {
                        div()
                            .truncate()
                            .text_xs()
                            .text_color(theme.muted)
                            .child(detail)
                    })),
            )
            .on_click(choose)
            .when_some(selection.scroll.reveal_row(selected), |row, reveal| {
                row.relative().child(reveal)
            })
    }
}

pub(crate) fn suggestions(
    prefix: &str,
    heading: &'static str,
    rows: Vec<AnyElement>,
    empty: String,
    footer: Option<AnyElement>,
    selection: &Selection,
    theme: &Theme,
) -> Deferred {
    let no_rows = rows.is_empty();
    deferred(
        div()
            .absolute()
            .top(relative(1.))
            .mt(px(5. * theme.scale))
            .left_0()
            .w_full()
            .max_w(px(440. * theme.scale))
            .occlude()
            .p_1p5()
            .bg(theme.rail)
            .border_1()
            .border_color(theme.border)
            .rounded(theme.radius)
            .shadow_lg()
            .child(
                div()
                    .px_2()
                    .pt_1()
                    .pb_1p5()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.muted)
                    .child(heading),
            )
            .child(
                div()
                    .id(ElementId::Name(format!("{prefix}-offers").into()))
                    .test_support()
                    .role(Role::ListBox)
                    .aria_label(heading)
                    .max_h(px(320. * theme.scale))
                    .overflow_y_scroll()
                    .track_scroll(&selection.scroll.handle)
                    .when(no_rows, |d| {
                        d.child(div().p_2().text_sm().text_color(theme.muted).child(empty))
                    })
                    .children(rows),
            )
            .children(footer),
    )
}

pub(crate) fn calendar_state(window: &mut Window, cx: &mut App) -> Entity<DatePickerState> {
    cx.new(|cx| {
        DatePickerState::new(window, cx)
            .date_format("%-d / %-m / %Y")
            .disabled_matcher(Matcher::interval(None, Some(today())))
    })
}

pub(crate) fn calendar(state: &Entity<DatePickerState>, theme: &Theme) -> impl IntoElement {
    div()
        .mt_1()
        .border_t_1()
        .border_color(theme.border)
        .px_2()
        .pt_2()
        .pb_1()
        .flex()
        .items_center()
        .gap_2()
        .child(
            div()
                .flex_none()
                .text_xs()
                .text_color(theme.muted)
                .child(t!("search-pick-day")),
        )
        .child(
            div()
                .id("query-calendar")
                .test_support()
                .role(Role::Group)
                .aria_label(t!("search-pick-day"))
                .flex_1()
                .min_w_0()
                .child(
                    DatePicker::new(state)
                        .placeholder(today().format("%-d / %-m / %Y").to_string()),
                ),
        )
}

pub(crate) fn today() -> NaiveDate {
    chrono::Local::now().date_naive()
}

pub(crate) fn date_presets() -> [(&'static str, u64); 4] {
    [
        (t!("search-today"), 0),
        (t!("search-yesterday"), 1),
        (t!("search-week-ago"), 7),
        (t!("search-month-ago"), 30),
    ]
}

//! A text tooltip: `.tooltip(ui::tooltip::text("Reply"))` on any element
//! with an ID.

use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::*;

pub fn text(text: impl Into<SharedString>) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    let text: SharedString = text.into();
    move |window, cx| Tooltip::new(text.clone()).build(window, cx)
}

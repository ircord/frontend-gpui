//! Fill a gpui-kit popup menu from registered menu items.

use gpui_kit::TestSupportExt;
use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::registry::MenuItem;

/// The visible items for `ctx`, in order, each with its reason if it is
/// unavailable for `ctx`.
pub fn visible<C>(items: &[MenuItem<C>], ctx: &C, cx: &App) -> Vec<MenuItem<C>> {
    items
        .iter()
        .filter(|item| (item.visible)(ctx, cx))
        .map(|item| {
            let mut item = item.clone();
            item.reason = item.unavailable.as_ref().and_then(|why| why(ctx, cx));
            item
        })
        .collect()
}

/// Add `items` to `menu`. Each item's accessible name is its label, so a test
/// finds it with `find_by_label`. An unavailable item is disabled and shows
/// its reason under the label.
pub fn fill<C: Clone + 'static>(mut menu: PopupMenu, items: Vec<MenuItem<C>>, ctx: C) -> PopupMenu {
    for item in items {
        if let Some(reason) = item.reason.clone() {
            let label = item.label.clone();
            let id = ElementId::Name(format!("menu-unavailable-{}", item.id).into());
            let reason_id = ElementId::Name(format!("menu-unavailable-reason-{}", item.id).into());
            let mut entry = PopupMenuItem::element(move |_, cx| {
                let theme = crate::theme::get(cx);
                div()
                    .id(id.clone())
                    .test_support()
                    .role(Role::MenuItem)
                    .aria_label(label.clone())
                    .aria_description(reason.clone())
                    .flex()
                    .flex_col()
                    .child(div().text_color(theme.faint).child(label.clone()))
                    .child(
                        div()
                            .id(reason_id.clone())
                            .test_support()
                            .aria_label(reason.clone())
                            .text_xs()
                            .text_color(theme.faint)
                            .max_w(px(280.))
                            .child(reason.clone()),
                    )
            })
            .disabled(true);
            if let Some(icon) = item.icon.clone() {
                entry = entry.icon(icon);
            }
            menu = menu.item(entry);
            continue;
        }
        let run = item.run.clone();
        let ctx = ctx.clone();
        let mut entry = PopupMenuItem::new(item.label.clone())
            .on_click(move |_, window, cx| run(&ctx, window, cx));
        if let Some(icon) = item.icon.clone() {
            entry = entry.icon(icon);
        }
        menu = menu.item(entry);
    }
    menu
}

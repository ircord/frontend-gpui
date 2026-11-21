//! Fill a gpui-kit popup menu from registered menu items.

use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};
use gpui_kit::*;

use crate::registry::MenuItem;

/// The visible items for `ctx`, in order.
pub fn visible<C>(items: &[MenuItem<C>], ctx: &C, cx: &App) -> Vec<MenuItem<C>> {
    items
        .iter()
        .filter(|item| (item.visible)(ctx, cx))
        .cloned()
        .collect()
}

/// Add `items` to `menu`. Each item's accessible name is its label, so a test
/// finds it with `find_by_label`.
pub fn fill<C: Clone + 'static>(mut menu: PopupMenu, items: Vec<MenuItem<C>>, ctx: C) -> PopupMenu {
    for item in items {
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

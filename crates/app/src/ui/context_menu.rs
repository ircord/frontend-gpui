//! Fill a gpui-kit popup menu from registered menu items, drawn as the web's
//! menus: a title row naming what the menu is for, one icon column (gpui-kit
//! keeps a slot for every row once any row has an icon), a separator between
//! groups and destructive rows in the danger colour.
//!
//! Items group by their order's hundreds (`0..100`, `100..200`, …): a menu
//! draws a separator where the hundreds change, and before the first
//! destructive row that follows an ordinary one. The orders, and so the
//! web's sequence of every menu, are in `crate::menu_order`.
//!
//! An entry registered [`MenuItem::under`] another goes into that entry's
//! submenu, grouped the same way.

use gpui_kit::TestSupportExt;
use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};
use gpui_kit::component::{Icon, Side, Sizable};
use gpui_kit::prelude::*;
use gpui_kit::*;
use ircord_client::contract::ConversationKind;
use ircord_client::store::people;

use crate::registry::{ConversationCtx, MemberCtx, MenuItem, MessageCtx, NetworkCtx, SidebarCtx};

/// What a menu's title row says about its target, if anything.
pub trait MenuTitle {
    fn menu_title(&self, cx: &App) -> Option<SharedString>;
}

impl MenuTitle for ConversationCtx {
    fn menu_title(&self, cx: &App) -> Option<SharedString> {
        let client = self.session.read(cx).client()?.clone();
        let w = client.read();
        let c = w.conversation(&self.conversation)?;
        Some(match c.kind {
            ConversationKind::Channel if !c.name.starts_with('#') => format!("#{}", c.name).into(),
            ConversationKind::Channel => c.name.clone().into(),
            _ => w.display_name(&c.name, &c.network_id).into(),
        })
    }
}

impl MenuTitle for NetworkCtx {
    fn menu_title(&self, cx: &App) -> Option<SharedString> {
        let client = self.session.read(cx).client()?.clone();
        let w = client.read();
        w.network(&self.network).map(|n| n.name.clone().into())
    }
}

impl MenuTitle for MemberCtx {
    fn menu_title(&self, cx: &App) -> Option<SharedString> {
        let client = self.session.read(cx).client()?.clone();
        let w = client.read();
        let network = self
            .conversation
            .as_deref()
            .and_then(|c| w.conversation(c))
            .map(|c| c.network_id.clone());
        let networks: Vec<String> = match network {
            Some(n) => vec![n],
            None => w.networks.iter().map(|n| n.id.clone()).collect(),
        };
        networks.iter().find_map(|n| {
            let m = w.member(&self.member, n)?;
            if w.is_self(&m.id, n) {
                people::self_name(&w, n).or_else(|| Some(people::name(m).to_string()))
            } else {
                Some(people::name(m).to_string())
            }
            .map(SharedString::from)
        })
    }
}

impl MenuTitle for MessageCtx {
    /// The author's nick, as the web's message menu names it.
    fn menu_title(&self, cx: &App) -> Option<SharedString> {
        let client = self.session.read(cx).client()?.clone();
        let w = client.read();
        let network = w.conversation(&self.conversation)?.network_id.clone();
        let message = w.message(&self.conversation, &self.message)?;
        let author = people::person(&w, &message.sender_id, &network)?;
        Some(author.nick.clone().into())
    }
}

impl MenuTitle for SidebarCtx {
    /// Your own menu opens under your name and picture: no title.
    fn menu_title(&self, _: &App) -> Option<SharedString> {
        None
    }
}

/// The visible items for `ctx`, in order, each with its reason if it is
/// unavailable for `ctx`.
pub fn visible<C>(items: &[MenuItem<C>], ctx: &C, cx: &App) -> Vec<MenuItem<C>> {
    items
        .iter()
        .filter(|item| (item.visible)(ctx, cx))
        .map(|item| {
            let mut item = item.clone();
            item.reason = item.unavailable.as_ref().and_then(|why| why(ctx, cx));
            if let Some(label) = &item.label_for {
                item.label = label(ctx, cx);
            }
            item.checked = item.checked_when.as_ref().is_some_and(|c| c(ctx, cx));
            item.disabled = item.disabled_when.as_ref().is_some_and(|d| d(ctx, cx));
            item
        })
        .collect()
}

/// Add `items` to `menu` under the title `ctx` gives. Each item's
/// accessible name is its label, so a test finds it with `find_by_label`.
/// An unavailable item is disabled and shows its reason under the label; a
/// destructive one is drawn in the danger colour. Entries under another
/// open from it as a submenu.
pub fn fill<C: Clone + MenuTitle + 'static>(
    mut menu: PopupMenu,
    items: Vec<MenuItem<C>>,
    ctx: C,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    if items.is_empty() {
        return menu;
    }
    // The title row: not a menu item, so arrow keys pass over it.
    if let Some(title) = ctx.menu_title(cx) {
        menu = menu.item(PopupMenuItem::label(title)).separator();
    }
    let (top, nested): (Vec<_>, Vec<_>) = items.into_iter().partition(|i| i.parent.is_none());
    rows(menu, top, nested, ctx, window, cx)
}

/// The rows of one menu level, with separators, opening a submenu for each
/// entry that has entries under it.
fn rows<C: Clone + 'static>(
    mut menu: PopupMenu,
    items: Vec<MenuItem<C>>,
    nested: Vec<MenuItem<C>>,
    ctx: C,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let viewport = window.viewport_size();
    let scale = crate::theme::get(cx).scale;
    let width = (viewport.width - px(16.)).max(px(1.));
    menu = menu
        .min_w(px(224. * scale).min(width))
        .max_w(px(288. * scale).min(width))
        .max_h((viewport.height - px(16.)).max(px(1.)))
        .scrollable(true)
        .check_side(Side::Right);
    let breaks = separators(&items);
    for (item, separate) in items.into_iter().zip(breaks) {
        if separate {
            menu = menu.separator();
        }
        let under: Vec<MenuItem<C>> = nested
            .iter()
            .filter(|n| n.parent.as_ref() == Some(&item.id))
            .cloned()
            .collect();
        if under.is_empty() {
            menu = menu.item(entry(item, ctx.clone(), cx));
            continue;
        }
        let (ctx, rest) = (ctx.clone(), nested.clone());
        let icon = item.icon.clone().map(Icon::new);
        menu = menu.submenu_with_icon(
            icon,
            item.label.clone(),
            window,
            cx,
            move |m, window, cx| rows(m, under.clone(), rest.clone(), ctx.clone(), window, cx),
        );
    }
    menu
}

/// Where a menu draws separators: before each item whose group differs from
/// the one above it. An item's group is its order's hundreds; a destructive
/// row after an ordinary one starts a group of its own.
pub fn separators<C>(items: &[MenuItem<C>]) -> Vec<bool> {
    let mut before = Vec::with_capacity(items.len());
    for (i, item) in items.iter().enumerate() {
        let Some(prev) = i.checked_sub(1).map(|p| &items[p]) else {
            before.push(false);
            continue;
        };
        let new_group = prev.order.div_euclid(100) != item.order.div_euclid(100);
        before.push(new_group || (item.danger && !prev.danger));
    }
    before
}

fn entry<C: Clone + 'static>(item: MenuItem<C>, ctx: C, cx: &App) -> PopupMenuItem {
    if item.heading {
        return PopupMenuItem::label(item.label.clone());
    }
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
        return entry;
    }
    let run = item.run.clone();
    if item.danger {
        // Drawn by hand so the label and its icon take the danger colour.
        let label = item.label.clone();
        let id = ElementId::Name(format!("menu-danger-{}", item.id).into());
        let mut entry = PopupMenuItem::element(move |_, cx| {
            let theme = crate::theme::get(cx);
            div()
                .id(id.clone())
                .test_support()
                .role(Role::MenuItem)
                .aria_label(label.clone())
                .text_color(theme.danger)
                .child(label.clone())
        })
        .on_click(move |_, window, cx| run(&ctx, window, cx));
        if let Some(icon) = item.icon.clone() {
            entry = entry.icon(
                Icon::new(icon)
                    .xsmall()
                    .text_color(crate::theme::get(cx).danger),
            );
        }
        return entry;
    }
    let mut entry = PopupMenuItem::new(item.label.clone())
        .checked(item.checked)
        .disabled(item.disabled)
        .on_click(move |_, window, cx| run(&ctx, window, cx));
    if let Some(icon) = item.icon.clone() {
        entry = entry.icon(icon);
    }
    entry
}

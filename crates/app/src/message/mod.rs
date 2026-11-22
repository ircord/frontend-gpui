//! One message row: the frame (avatar, name and time, reply line) around
//! the body, reactions, attachments and link cards, each in its own file.
//! The frame also hosts the registered message menu (right click) and the
//! hover toolbar (registered items marked `toolbar`).

pub mod attachments;
pub mod body;
pub mod emoji;
pub mod layout;
pub mod links;
mod offline_pin;
pub mod reactions;

use std::rc::Rc;

use gpui_kit::TestSupportExt;
use gpui_kit::component::Sizable;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::menu::DropdownMenu;
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::registry::{self, MessageCtx};
use crate::theme::Theme;
use crate::timeline::rows::MessageRow;
use crate::ui;

pub struct MessageView {
    pub row: Rc<MessageRow>,
    pub ctx: MessageCtx,
    /// Under the pointer: show the toolbar.
    pub hovered: bool,
    #[allow(clippy::type_complexity)]
    pub on_hover: Rc<dyn Fn(bool, &mut Window, &mut App)>,
}

/// Draw one message row, laid out as the message settings say
/// ([`layout::render`]).
pub fn render(view: MessageView, theme: &Theme, window: &mut Window, cx: &mut App) -> AnyElement {
    layout::render(view, theme, window, cx)
}

/// The message whose "Message actions" menu is open, if any.
struct MenuOpen(Option<String>);
impl Global for MenuOpen {}

/// Whether the toolbar's menu is open on this message.
pub(crate) fn menu_open(ctx: &MessageCtx, cx: &App) -> bool {
    cx.try_global::<MenuOpen>().and_then(|m| m.0.as_ref()) == Some(&ctx.message)
}

/// The registered toolbar actions over the message under the pointer.
pub(crate) fn toolbar(
    ctx: &MessageCtx,
    theme: &Theme,
    _window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let items = registry::get(cx).message_menu.to_vec();
    let items: Vec<_> = ui::context_menu::visible(&items, ctx, cx)
        .into_iter()
        .filter(|i| i.toolbar && i.reason.is_none())
        .collect();
    let mut bar = div()
        .id("message-toolbar")
        .test_support()
        .role(Role::Toolbar)
        .aria_label(t!("core-message-toolbar"))
        .absolute()
        .top(px(-12.))
        .right(px(16.))
        .flex()
        .gap_0p5()
        .p_0p5()
        .rounded_full()
        .border_1()
        .border_color(theme.border.opacity(0.6))
        .bg(theme.raised)
        .shadow_sm();
    for item in items {
        let (run, ctx) = (item.run.clone(), ctx.clone());
        let icon = item.icon.clone().unwrap_or(ui::icon::IconName::Ellipsis);
        bar = bar.child(ui::icon::button(
            ElementId::Name(format!("message-toolbar-{}", item.id).into()),
            icon,
            item.label.clone(),
            theme,
            move |_, window, cx| run(&ctx, window, cx),
        ));
    }
    // The whole message menu, as the web's "Message actions" button.
    let menu_ctx = ctx.clone();
    bar = bar.child(
        Button::new("message-actions-menu")
            .ghost()
            .xsmall()
            .icon(ui::icon::IconName::Ellipsis)
            .accessibility_label(t!("core-message-toolbar"))
            .tooltip(t!("core-message-toolbar"))
            .dropdown_menu_with_anchor(Anchor::TopRight, move |menu, window, cx| {
                // The toolbar stays while its menu is open, though the
                // pointer leaves the row for the menu.
                let id = menu_ctx.message.clone();
                cx.set_global(MenuOpen(Some(id.clone())));
                cx.on_release(move |_, cx| {
                    if cx.try_global::<MenuOpen>().and_then(|m| m.0.as_ref()) == Some(&id) {
                        cx.set_global(MenuOpen(None));
                        cx.refresh_windows();
                    }
                })
                .detach();
                let items = registry::get(cx).message_menu.to_vec();
                let items = ui::context_menu::visible(&items, &menu_ctx, cx);
                ui::context_menu::fill(menu, items, menu_ctx.clone(), window, cx)
            }),
    );
    bar.into_any_element()
}

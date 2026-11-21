//! One message row: the frame (avatar, name and time, reply line) around
//! the body, reactions, attachments and link cards, each in its own file.
//! The frame also hosts the registered message menu (right click) and the
//! hover toolbar (registered items marked `toolbar`).

pub mod attachments;
pub mod body;
pub mod emoji;
pub mod layout;
pub mod links;
pub mod reactions;

use std::rc::Rc;

use gpui_kit::TestSupportExt;
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
    if items.is_empty() {
        return div().into_any_element();
    }
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
        .rounded_md()
        .border_1()
        .border_color(theme.rail)
        .bg(theme.sidebar);
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
    bar.into_any_element()
}

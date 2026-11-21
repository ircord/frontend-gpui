//! "Are you sure?" before something that cannot be undone.

use std::rc::Rc;

use gpui_kit::component::WindowExt;
use gpui_kit::component::button::ButtonVariant;
use gpui_kit::*;

pub struct Confirm {
    pub title: SharedString,
    pub body: SharedString,
    /// The confirming button's text, such as "Delete".
    pub ok: SharedString,
    /// Whether confirming destroys something.
    pub danger: bool,
}

/// Ask, and run `on_ok` only if the person confirms.
pub fn confirm(
    window: &mut Window,
    cx: &mut App,
    confirm: Confirm,
    on_ok: impl Fn(&mut Window, &mut App) + 'static,
) {
    let on_ok = Rc::new(on_ok);
    window.open_alert_dialog(cx, move |alert, _, _| {
        let on_ok = on_ok.clone();
        let alert = alert
            .title(confirm.title.clone())
            .description(confirm.body.clone())
            .ok_text(confirm.ok.clone())
            .cancel_text(crate::t!("core-cancel"))
            .show_cancel(true)
            .on_ok(move |_, window, cx| {
                on_ok(window, cx);
                true
            });
        if confirm.danger {
            alert.ok_variant(ButtonVariant::Danger)
        } else {
            alert
        }
    });
}

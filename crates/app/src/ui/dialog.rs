//! A dialog over the window.

use gpui_kit::component::WindowExt;
use gpui_kit::component::dialog::Dialog;
use gpui_kit::*;

/// Open a dialog titled `title`; `build` fills it in. Like gpui-kit's
/// builder it may run again whenever the dialog redraws, so create entities
/// (inputs) before calling this, not inside `build`.
pub fn open(
    window: &mut Window,
    cx: &mut App,
    title: impl Into<SharedString>,
    build: impl Fn(Dialog, &mut Window, &mut App) -> Dialog + 'static,
) {
    let title = title.into();
    window.open_dialog(cx, move |dialog, window, cx| {
        build(dialog.title(title.clone()), window, cx)
    });
}

/// Close the topmost dialog.
pub fn close(window: &mut Window, cx: &mut App) {
    window.close_dialog(cx);
}

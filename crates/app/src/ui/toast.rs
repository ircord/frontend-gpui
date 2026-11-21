//! Short messages in the corner of the window.

use gpui_kit::component::WindowExt;
use gpui_kit::component::notification::Notification;
use gpui_kit::*;

pub fn info(window: &mut Window, cx: &mut App, text: impl Into<SharedString>) {
    window.push_notification(Notification::info(text), cx);
}

pub fn success(window: &mut Window, cx: &mut App, text: impl Into<SharedString>) {
    window.push_notification(Notification::success(text), cx);
}

pub fn error(window: &mut Window, cx: &mut App, text: impl Into<SharedString>) {
    window.push_notification(Notification::error(text), cx);
}

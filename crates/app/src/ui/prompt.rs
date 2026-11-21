//! Ask for one line of text (a new name, a reason).

use std::rc::Rc;

use gpui_kit::component::WindowExt;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::prelude::*;
use gpui_kit::*;

pub struct Prompt {
    pub title: SharedString,
    /// The field's label, also its accessible name.
    pub label: SharedString,
    pub initial: String,
    pub ok: SharedString,
}

/// Show the prompt; `on_submit` gets the text when the person confirms.
pub fn prompt(
    window: &mut Window,
    cx: &mut App,
    prompt: Prompt,
    on_submit: impl Fn(String, &mut Window, &mut App) + 'static,
) {
    let input = cx.new(|cx| {
        let mut state = InputState::new(window, cx);
        state.set_value(prompt.initial.clone(), window, cx);
        state
    });
    let on_submit = Rc::new(on_submit);
    let focus = std::cell::Cell::new(true);
    window.open_alert_dialog(cx, move |dialog, window, cx| {
        if focus.replace(false) {
            input.focus_handle(cx).focus(window, cx);
        }
        let (input2, on_submit) = (input.clone(), on_submit.clone());
        dialog
            .title(prompt.title.clone())
            .description(super::field::field(
                prompt.label.clone(),
                Input::new(&input)
                    .id("prompt-input")
                    .aria_label(prompt.label.clone()),
                crate::theme::get(cx),
            ))
            .ok_text(prompt.ok.clone())
            .cancel_text(crate::t!("core-cancel"))
            .show_cancel(true)
            .on_ok(move |_, window, cx| {
                let text = input2.read(&*cx).value().to_string();
                on_submit(text, window, cx);
                true
            })
    });
}

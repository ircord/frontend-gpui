//! Studio base schemes and custom palettes, edited one mode at a time.

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::color_picker::{ColorPicker, ColorPickerEvent, ColorPickerState};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::menu::{DropdownMenu, PopupMenuItem};
use gpui_kit::component::{Colorize, Disableable, Sizable};
use gpui_kit::prelude::*;
use gpui_kit::*;
use ircord_client::contract::Theme as UserTheme;
use ircord_client::store::appearance::{self as look, Mode, Seed, Seeds};

use super::{
    Editor, draft_fields, draft_input_changed, fill_seeds, option, refresh_studio, say,
    scheme_label, switch_preview_mode,
};
use crate::theme::{self, Theme};
use crate::ui::focus::Reachable;

#[derive(Clone)]
pub(super) struct Field {
    pub(super) seed: Seed,
    input: Entity<InputState>,
    picker: Entity<ColorPickerState>,
}

pub(super) fn fields(window: &mut Window, cx: &mut App) -> Vec<Field> {
    Seed::INTERFACE
        .into_iter()
        .chain(Seed::PALETTE)
        .map(|seed| Field {
            seed,
            input: cx.new(|cx| {
                InputState::new(window, cx).placeholder(t!("appearance-accent-custom-placeholder"))
            }),
            picker: cx.new(|cx| ColorPickerState::new(window, cx)),
        })
        .collect()
}

impl Field {
    pub(super) fn value(&self, cx: &App) -> Option<u32> {
        parse(&self.input.read(cx).value())
    }

    pub(super) fn fill(&self, seeds: &Seeds, window: &mut Window, cx: &mut App) {
        let color = self.seed.get(seeds);
        self.input.update(cx, |input, cx| {
            input.set_value(look::to_hex(color), window, cx)
        });
        self.picker.update(cx, |picker, cx| {
            picker.set_value(theme::hex(color), window, cx)
        });
    }

    pub(super) fn subscribe(&self, window: &Window, cx: &mut Context<Editor>) -> [Subscription; 2] {
        let input = self.input.clone();
        let picker = self.picker.clone();
        [
            cx.subscribe_in(
                &self.picker,
                window,
                move |_, _, event: &ColorPickerEvent, window, cx| {
                    if let ColorPickerEvent::Change(Some(value)) = event {
                        let hex = Hsla { a: 1., ..*value }.to_hex();
                        input.update(cx, |input, cx| input.set_value(hex, window, cx));
                        draft_input_changed(&InputEvent::Change, cx);
                    }
                },
            ),
            cx.subscribe_in(
                &self.input,
                window,
                move |_, input, event: &InputEvent, window, cx| {
                    if matches!(event, InputEvent::Change)
                        && let Some(color) = parse(&input.read(cx).value())
                    {
                        picker.update(cx, |picker, cx| {
                            picker.set_value(theme::hex(color), window, cx)
                        });
                    }
                    draft_input_changed(event, cx);
                },
            ),
        ]
    }

    fn render(&self) -> impl IntoElement + use<> {
        let (key, label) = labels(self.seed);
        div()
            .flex()
            .items_center()
            .gap_2()
            .child(div().min_w_0().flex_1().text_sm().child(label))
            .child(
                div().w(px(96.)).child(
                    Input::new(&self.input)
                        .id(SharedString::from(format!("appearance-seed-{key}")))
                        .aria_label(label),
                ),
            )
            .child(
                div()
                    .id(SharedString::from(format!("appearance-seed-picker-{key}")))
                    .test_support()
                    .child(
                        ColorPicker::new(&self.picker)
                            .small()
                            .accessibility_label(t!("appearance-seed-choose", color = label)),
                    ),
            )
    }
}

fn labels(seed: Seed) -> (&'static str, &'static str) {
    match seed {
        Seed::Canvas => ("canvas", t!("appearance-seed-canvas")),
        Seed::Surface => ("surface", t!("appearance-seed-surface")),
        Seed::Overlay => ("overlay", t!("appearance-seed-overlay")),
        Seed::Ink => ("ink", t!("appearance-seed-ink")),
        Seed::Muted => ("muted", t!("appearance-seed-muted")),
        Seed::Accent => ("accent", t!("appearance-seed-accent")),
        Seed::Red => ("red", t!("appearance-seed-red")),
        Seed::Orange => ("orange", t!("appearance-seed-orange")),
        Seed::Yellow => ("yellow", t!("appearance-seed-yellow")),
        Seed::Green => ("green", t!("appearance-seed-green")),
        Seed::Cyan => ("cyan", t!("appearance-seed-cyan")),
        Seed::Blue => ("blue", t!("appearance-seed-blue")),
        Seed::Purple => ("purple", t!("appearance-seed-purple")),
        Seed::Pink => ("pink", t!("appearance-seed-pink")),
    }
}

fn parse(text: &str) -> Option<u32> {
    let text = text.trim();
    if text.starts_with('#') {
        look::parse_hex(text)
    } else {
        look::parse_hex(&format!("#{text}"))
    }
}

pub(super) fn render(
    editor: &Entity<Editor>,
    draft: &UserTheme,
    mode: Mode,
    theme: &Theme,
    cx: &App,
) -> impl IntoElement + use<> {
    let base = look::theme_base(draft, mode);
    let state = editor.read(cx);
    let busy = state.busy;
    let choose = editor.clone();
    let customize = editor.clone();
    let reset = editor.clone();
    let mut form = div()
        .flex()
        .flex_col()
        .gap_3()
        .child(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(div().text_sm().child(t!("appearance-studio-preview-mode")))
                .child(
                    div()
                        .id("theme-studio-mode")
                        .test_support()
                        .role(Role::RadioGroup)
                        .aria_label(t!("appearance-studio-preview-mode"))
                        .flex()
                        .gap_1()
                        .children(
                            [
                                (Mode::Dark, "theme-studio-dark", t!("appearance-mode-dark")),
                                (
                                    Mode::Light,
                                    "theme-studio-light",
                                    t!("appearance-mode-light"),
                                ),
                            ]
                            .map(|(choice, id, label)| {
                                let editor = editor.clone();
                                option(id, label, mode == choice, theme)
                                    .test_support()
                                    .reachable(theme)
                                    .on_click(move |_, window, cx| {
                                        switch_preview_mode(&editor, choice, window, cx)
                                    })
                            }),
                        ),
                ),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(div().text_sm().child(t!("appearance-studio-base")))
                .child(
                    Button::new("theme-studio-base")
                        .w_full()
                        .justify_start()
                        .dropdown_caret(true)
                        .label(scheme_label(base))
                        .accessibility_label(t!("appearance-studio-base"))
                        .dropdown_menu_with_anchor(Anchor::TopLeft, move |menu, _, _| {
                            look::schemes().filter(|scheme| scheme.mode == mode).fold(
                                menu.max_h(px(290.)),
                                |menu, scheme| {
                                    let editor = choose.clone();
                                    menu.item(
                                        PopupMenuItem::new(scheme_label(scheme))
                                            .checked(scheme.id == base.id)
                                            .on_click(move |_, window, cx| {
                                                change(&editor, window, cx, |draft, _| {
                                                    look::set_theme_base(draft, scheme)
                                                })
                                            }),
                                    )
                                },
                            )
                        }),
                ),
        );
    if look::theme_seeds(draft, mode).is_none() {
        return form
            .child(div().text_sm().text_color(theme.muted).child(t!(
                "appearance-studio-base-only",
                scheme = scheme_label(base)
            )))
            .child(
                Button::new("theme-studio-customize")
                    .outline()
                    .disabled(busy)
                    .label(t!("appearance-studio-customize"))
                    .on_click(move |_, window, cx| {
                        change(&customize, window, cx, |draft, mode| {
                            let seed = look::theme_base(draft, mode).seeds;
                            look::set_theme_seeds(draft, mode, Some(seed));
                        })
                    }),
            )
            .into_any_element();
    }
    form = form
        .child(
            div().flex().flex_col().gap_2().children(
                state
                    .seeds
                    .iter()
                    .filter(|field| Seed::INTERFACE.contains(&field.seed))
                    .map(Field::render),
            ),
        )
        .child(
            div()
                .text_sm()
                .font_weight(FontWeight::SEMIBOLD)
                .child(t!("appearance-studio-palette")),
        )
        .child(
            div().flex().flex_col().gap_2().children(
                state
                    .seeds
                    .iter()
                    .filter(|field| Seed::PALETTE.contains(&field.seed))
                    .map(Field::render),
            ),
        );
    if let Ok(current) = draft_fields(state, draft, mode, cx) {
        let seeds = look::drawn_seeds(&current, mode);
        let tokens = look::tokens(&seeds, mode, &[], None);
        if tokens.text != seeds.ink || tokens.muted != seeds.muted || tokens.accent != seeds.accent
        {
            form = form.child(
                div()
                    .text_sm()
                    .text_color(theme.muted)
                    .child(t!("appearance-studio-nudged")),
            );
        }
    }
    form.child(
        Button::new("theme-studio-reset-colors")
            .outline()
            .disabled(busy)
            .label(t!("appearance-studio-reset-colors"))
            .on_click(move |_, window, cx| {
                let state = reset.read(cx);
                if state.busy {
                    return;
                }
                if let Some((mut draft, mode)) = state.editing.clone() {
                    look::set_theme_seeds(&mut draft, mode, None);
                    set(&reset, draft, mode, window, cx);
                }
            }),
    )
    .into_any_element()
}

fn change(
    editor: &Entity<Editor>,
    window: &mut Window,
    cx: &mut App,
    change: impl FnOnce(&mut UserTheme, Mode),
) {
    let state = editor.read(cx);
    if state.busy {
        return;
    }
    let Some((draft, mode)) = &state.editing else {
        return;
    };
    let mode = *mode;
    match draft_fields(state, draft, mode, cx) {
        Ok(mut draft) => {
            change(&mut draft, mode);
            set(editor, draft, mode, window, cx);
        }
        Err(message) => say(editor, message, true, cx),
    }
}

fn set(editor: &Entity<Editor>, draft: UserTheme, mode: Mode, window: &mut Window, cx: &mut App) {
    fill_seeds(editor, &draft, mode, window, cx);
    editor.update(cx, |editor, _| {
        editor.editing = Some((draft, mode));
        editor.notice = None;
    });
    refresh_studio(editor, cx);
}

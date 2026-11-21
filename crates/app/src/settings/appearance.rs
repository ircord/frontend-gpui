//! Settings → Appearance (package S1), as the web client's
//! `AppearanceSettings.vue` and theme library: sync across devices, light or
//! dark, the colour scheme and its variant, the accent, the interface font,
//! scale, sidebar density, corners and motion, your themes (new, use, edit
//! colours, export, import, delete) and reset.
//!
//! Every change goes through `theme::appearance::edit`, which saves to the
//! account with sync on and on this device with it off, and redraws at once.

use crate::ui::focus::Reachable;
use std::path::PathBuf;
use std::rc::Rc;

use gpui_kit::TestSupportExt;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::switch::Switch;
use gpui_kit::prelude::*;
use gpui_kit::*;
use ircord_client::contract::{
    Appearance, AppearanceAccent, AppearanceCorners, AppearanceDensity, AppearanceFont,
    AppearanceMode, AppearanceMotion, Theme as UserTheme,
};
use ircord_client::store::appearance::{self as look, FAMILIES, ImportError, Look, Mode, Seed};

use crate::registry::{self, SettingsCtx, SettingsGroup, SettingsRow, SettingsSection};
use crate::session::Session;
use crate::settings::{block, row};
use crate::theme::{self, Theme, appearance};
use crate::ui;

pub fn init(cx: &mut App) {
    registry::register(cx, |r| {
        let rows: [(&str, &'static str, &'static str, &'static str); 11] = [
            (
                "appearance-sync",
                t!("appearance-sync"),
                t!("appearance-sync-help"),
                t!("appearance-sync-keywords"),
            ),
            (
                "appearance-mode",
                t!("appearance-mode"),
                t!("appearance-mode-help"),
                t!("appearance-mode-keywords"),
            ),
            (
                "appearance-scheme",
                t!("appearance-scheme"),
                t!("appearance-scheme-help"),
                t!("appearance-scheme-keywords"),
            ),
            (
                "appearance-accent",
                t!("appearance-accent"),
                t!("appearance-accent-help"),
                t!("appearance-accent-keywords"),
            ),
            (
                "appearance-font",
                t!("appearance-font"),
                t!("appearance-font-help"),
                t!("appearance-font-keywords"),
            ),
            (
                "appearance-scale",
                t!("appearance-scale"),
                t!("appearance-scale-help"),
                t!("appearance-scale-keywords"),
            ),
            (
                "appearance-density",
                t!("appearance-density"),
                t!("appearance-density-help"),
                t!("appearance-density-keywords"),
            ),
            (
                "appearance-corners",
                t!("appearance-corners"),
                t!("appearance-corners-help"),
                t!("appearance-corners-keywords"),
            ),
            (
                "appearance-motion",
                t!("appearance-motion"),
                t!("appearance-motion-help"),
                t!("appearance-motion-keywords"),
            ),
            (
                "appearance-themes",
                t!("appearance-themes"),
                t!("appearance-themes-help"),
                t!("appearance-themes-keywords"),
            ),
            (
                "appearance-reset",
                t!("appearance-reset"),
                t!("appearance-reset-help"),
                t!("appearance-reset-keywords"),
            ),
        ];
        let mut section =
            SettingsSection::new("appearance", t!("appearance-title"), |ctx, window, cx| {
                render(ctx, window, cx).into_any_element()
            })
            .group(SettingsGroup::App)
            .order(10)
            .keywords(t!("appearance-keywords").split(", "));
        for (id, label, description, keywords) in rows {
            section = section.row(
                SettingsRow::new(id, label)
                    .description(description)
                    .keywords(keywords),
            );
        }
        r.settings_sections.add(section);
    });
}

// ---- State kept across renders ----

/// The text fields and the theme being edited, kept while the session lasts.
struct Editor {
    session: Entity<Session>,
    accent: Entity<InputState>,
    name: Entity<InputState>,
    import: Entity<InputState>,
    seeds: Vec<(Seed, Entity<InputState>)>,
    /// The theme whose colours are being edited, and the mode being edited.
    editing: Option<(UserTheme, Mode)>,
    /// What the last action said, and whether it was a problem.
    notice: Option<(SharedString, bool)>,
    busy: bool,
}

struct Editors(Entity<Editor>);
impl Global for Editors {}

fn editor(ctx: &SettingsCtx, window: &mut Window, cx: &mut App) -> Entity<Editor> {
    if let Some(Editors(e)) = cx.try_global::<Editors>()
        && e.read(cx).session == ctx.session
    {
        return e.clone();
    }
    let input = |placeholder: &'static str, window: &mut Window, cx: &mut App| {
        cx.new(|cx| InputState::new(window, cx).placeholder(placeholder))
    };
    let accent = input(t!("appearance-accent-custom-placeholder"), window, cx);
    let name = input(t!("appearance-themes-new-name"), window, cx);
    let import = input(t!("appearance-themes-import-paste"), window, cx);
    let seeds = Seed::ALL
        .iter()
        .map(|s| {
            (
                *s,
                input(t!("appearance-accent-custom-placeholder"), window, cx),
            )
        })
        .collect();
    let e = cx.new(|_| Editor {
        session: ctx.session.clone(),
        accent,
        name,
        import,
        seeds,
        editing: None,
        notice: None,
        busy: false,
    });
    cx.set_global(Editors(e.clone()));
    e
}

fn say(editor: &Entity<Editor>, text: impl Into<SharedString>, problem: bool, cx: &mut App) {
    let text = text.into();
    editor.update(cx, |e, _| e.notice = Some((text, problem)));
    cx.refresh_windows();
}

// ---- Rendering ----

fn render(ctx: &SettingsCtx, window: &mut Window, cx: &mut App) -> impl IntoElement {
    let theme = theme::get(cx).clone();
    let editor = editor(ctx, window, cx);
    let current = appearance::look(cx);
    let compiled = appearance::compiled(cx);
    let sync = appearance::sync(cx);
    let mode = compiled.mode;

    div()
        .id("appearance-settings")
        .test_support()
        .role(Role::Group)
        .aria_label(t!("appearance-title"))
        .flex()
        .flex_col()
        .gap_4()
        .child(row(
            ctx,
            "appearance-sync",
            t!("appearance-sync"),
            Some(
                if sync {
                    t!("appearance-sync-help")
                } else {
                    t!("appearance-sync-local")
                }
                .into(),
            ),
            Switch::new("appearance-sync-switch")
                .checked(sync)
                .accessibility_label(t!("appearance-sync"))
                .on_click(|on, _, cx| appearance::set_sync(cx, *on)),
            &theme,
        ))
        .child(row(
            ctx,
            "appearance-mode",
            t!("appearance-mode"),
            Some(t!("appearance-mode-help").into()),
            choices(
                "appearance-mode",
                &[
                    (
                        AppearanceMode::System,
                        "system",
                        t!("appearance-mode-system"),
                    ),
                    (AppearanceMode::Dark, "dark", t!("appearance-mode-dark")),
                    (AppearanceMode::Light, "light", t!("appearance-mode-light")),
                ],
                current.mode,
                |a, m| a.mode = Some(m),
                &theme,
            ),
            &theme,
        ))
        .child(block(
            ctx,
            "appearance-scheme",
            t!("appearance-scheme"),
            schemes(&current, mode, compiled.theme.is_some(), &theme),
            &theme,
        ))
        .child(block(
            ctx,
            "appearance-accent",
            t!("appearance-accent"),
            accents(&current, &editor, &theme, cx),
            &theme,
        ))
        .child(block(
            ctx,
            "appearance-font",
            t!("appearance-font"),
            fonts(&current, &theme),
            &theme,
        ))
        .child(row(
            ctx,
            "appearance-scale",
            t!("appearance-scale"),
            Some(t!("appearance-scale-help").into()),
            scale(current.scale, &theme),
            &theme,
        ))
        .child(row(
            ctx,
            "appearance-density",
            t!("appearance-density"),
            Some(t!("appearance-density-help").into()),
            choices(
                "appearance-density",
                &[
                    (
                        AppearanceDensity::Comfortable,
                        "comfortable",
                        t!("appearance-density-comfortable"),
                    ),
                    (
                        AppearanceDensity::Compact,
                        "compact",
                        t!("appearance-density-compact"),
                    ),
                ],
                current.density,
                |a, d| a.density = Some(d),
                &theme,
            ),
            &theme,
        ))
        .child(row(
            ctx,
            "appearance-corners",
            t!("appearance-corners"),
            Some(t!("appearance-corners-help").into()),
            choices(
                "appearance-corners",
                &[
                    (
                        AppearanceCorners::Rounded,
                        "rounded",
                        t!("appearance-corners-rounded"),
                    ),
                    (
                        AppearanceCorners::Soft,
                        "soft",
                        t!("appearance-corners-soft"),
                    ),
                    (
                        AppearanceCorners::Square,
                        "square",
                        t!("appearance-corners-square"),
                    ),
                ],
                current.corners,
                |a, c| a.corners = Some(c),
                &theme,
            ),
            &theme,
        ))
        .child(row(
            ctx,
            "appearance-motion",
            t!("appearance-motion"),
            Some(t!("appearance-motion-help").into()),
            choices(
                "appearance-motion",
                &[
                    (
                        AppearanceMotion::System,
                        "system",
                        t!("appearance-motion-system"),
                    ),
                    (AppearanceMotion::Full, "full", t!("appearance-motion-full")),
                    (
                        AppearanceMotion::Reduced,
                        "reduced",
                        t!("appearance-motion-reduced"),
                    ),
                ],
                current.motion,
                |a, m| a.motion = Some(m),
                &theme,
            ),
            &theme,
        ))
        .child(block(
            ctx,
            "appearance-themes",
            t!("appearance-themes"),
            themes_block(&current, mode, &editor, &theme, cx),
            &theme,
        ))
        .child(row(
            ctx,
            "appearance-reset",
            t!("appearance-reset"),
            Some(t!("appearance-reset-help").into()),
            button("appearance-reset-button", t!("appearance-reset"), &theme)
                .reachable(&theme)
                .on_click(|_, window, cx| confirm_reset(window, cx)),
            &theme,
        ))
}

/// A plain button drawn with the theme's tokens.
fn button(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    theme: &Theme,
) -> Stateful<Div> {
    let id: SharedString = id.into();
    let label: SharedString = label.into();
    div()
        .id(ElementId::Name(id))
        .role(Role::Button)
        .aria_label(label.clone())
        .px_3()
        .py_1()
        .rounded(theme.radius)
        .border_1()
        .border_color(theme.control_border)
        .bg(theme.raised)
        .text_sm()
        .text_color(theme.text)
        .cursor_pointer()
        .hover(|s| s.bg(theme.hover))
        .child(label)
}

/// One option among several; the chosen one is filled with the accent.
fn option(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    selected: bool,
    theme: &Theme,
) -> Stateful<Div> {
    let id: SharedString = id.into();
    let label: SharedString = label.into();
    div()
        .id(ElementId::Name(id))
        .role(Role::RadioButton)
        .aria_label(label.clone())
        .aria_selected(selected)
        .px_3()
        .py_1()
        .rounded(theme.radius)
        .border_1()
        .text_sm()
        .cursor_pointer()
        .when(selected, |d| {
            d.bg(theme.accent)
                .border_color(theme.accent)
                .text_color(theme.on_accent)
        })
        .when(!selected, |d| {
            d.bg(theme.raised)
                .border_color(theme.control_border)
                .text_color(theme.text)
                .hover(|s| s.bg(theme.hover))
        })
        .child(label)
}

/// A row of options for one setting; picking one edits the appearance.
fn choices<T: Copy + PartialEq + 'static>(
    prefix: &'static str,
    options: &[(T, &'static str, &'static str)],
    selected: T,
    set: fn(&mut Appearance, T),
    theme: &Theme,
) -> impl IntoElement + use<T> {
    let mut list = div()
        .id(ElementId::Name(format!("{prefix}-choices").into()))
        .test_support()
        .role(Role::RadioGroup)
        .flex()
        .flex_wrap()
        .gap_1();
    for (value, key, label) in options {
        let value = *value;
        list = list.child(
            option(format!("{prefix}-{key}"), *label, value == selected, theme)
                .test_support()
                .reachable(theme)
                .on_click(move |_, _, cx| {
                    appearance::edit(cx, |a| set(a, value));
                }),
        );
    }
    list
}

fn family_description(id: &str) -> Option<&'static str> {
    Some(match id {
        "ircord" => t!("appearance-family-ircord"),
        "hibiki" => t!("appearance-family-hibiki"),
        "midnight" => t!("appearance-family-midnight"),
        "catppuccin" => t!("appearance-family-catppuccin"),
        "tokyo-night" => t!("appearance-family-tokyo-night"),
        "nord" => t!("appearance-family-nord"),
        "rose-pine" => t!("appearance-family-rose-pine"),
        "gruvbox" => t!("appearance-family-gruvbox"),
        "dracula" => t!("appearance-family-dracula"),
        "solarized" => t!("appearance-family-solarized"),
        "everforest" => t!("appearance-family-everforest"),
        "kanagawa" => t!("appearance-family-kanagawa"),
        "github" => t!("appearance-family-github"),
        _ => return None,
    })
}

fn swatch(color: u32, theme: &Theme) -> Div {
    div()
        .size(px(14.))
        .rounded_full()
        .border_1()
        .border_color(theme.border)
        .bg(theme::hex(color))
}

/// Pick a family: its variant in each mode it has, keeping the chosen one
/// when it already belongs to the family.
fn pick_family(family: &'static look::Family, cx: &mut App) {
    let current = appearance::look(cx);
    appearance::edit(cx, |a| {
        for mode in [Mode::Dark, Mode::Light] {
            let chosen = current.scheme(mode);
            if chosen.family == family.id {
                continue;
            }
            if let Some(variant) = family.variants.iter().find(|s| s.mode == mode) {
                match mode {
                    Mode::Dark => a.scheme_dark = Some(variant.id.into()),
                    Mode::Light => a.scheme_light = Some(variant.id.into()),
                }
            }
        }
        a.theme = None;
    });
}

fn schemes(
    current: &Look,
    mode: Mode,
    theme_in_use: bool,
    theme: &Theme,
) -> impl IntoElement + use<> {
    let chosen = current.scheme(mode);
    let mut grid = div()
        .id("appearance-schemes")
        .test_support()
        .role(Role::RadioGroup)
        .aria_label(t!("appearance-scheme"))
        .flex()
        .flex_wrap()
        .gap_2();
    for family in FAMILIES {
        let shown = family
            .variants
            .iter()
            .find(|s| s.mode == mode)
            .unwrap_or(&family.variants[0]);
        let selected = !theme_in_use && chosen.family == family.id;
        let only = match (
            family.variants.iter().any(|s| s.mode == Mode::Dark),
            family.variants.iter().any(|s| s.mode == Mode::Light),
        ) {
            (true, false) => Some(t!("appearance-scheme-only-dark")),
            (false, true) => Some(t!("appearance-scheme-only-light")),
            _ => None,
        };
        let seeds = shown.seeds;
        grid = grid.child(
            div()
                .id(ElementId::Name(
                    format!("appearance-scheme-{}", family.id).into(),
                ))
                .test_support()
                .role(Role::RadioButton)
                .reachable(theme)
                .aria_label(SharedString::from(t!(
                    "appearance-scheme-pick",
                    name = family.name
                )))
                .aria_selected(selected)
                .w(px(180.))
                .p_2()
                .rounded(theme.radius)
                .border_2()
                .border_color(if selected { theme.accent } else { theme.border })
                .bg(theme::hex(seeds.surface))
                .cursor_pointer()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .flex()
                        .gap_1()
                        .child(swatch(seeds.canvas, theme))
                        .child(swatch(seeds.overlay, theme))
                        .child(swatch(seeds.accent, theme))
                        .child(swatch(seeds.palette.green, theme))
                        .child(swatch(seeds.palette.red, theme)),
                )
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme::hex(seeds.ink))
                        .child(family.name),
                )
                .when_some(family_description(family.id), |d, text| {
                    d.child(
                        div()
                            .text_xs()
                            .text_color(theme::hex(seeds.muted))
                            .child(text),
                    )
                })
                .when_some(only, |d, text| {
                    d.child(
                        div()
                            .text_xs()
                            .text_color(theme::hex(seeds.muted))
                            .child(text),
                    )
                })
                .on_click(move |_, _, cx| pick_family(family, cx)),
        );
    }
    let variants: Vec<_> = look::family(chosen.family)
        .map(|f| f.variants.iter().filter(|s| s.mode == mode).collect())
        .unwrap_or_default();
    div()
        .flex()
        .flex_col()
        .gap_2()
        .child(
            div()
                .text_sm()
                .text_color(theme.muted)
                .child(t!("appearance-scheme-help")),
        )
        .child(grid)
        .when(variants.len() > 1 && !theme_in_use, |d| {
            let mut list = div()
                .id("appearance-variants")
                .test_support()
                .role(Role::RadioGroup)
                .aria_label(t!("appearance-scheme-variant"))
                .flex()
                .flex_wrap()
                .items_center()
                .gap_1()
                .child(
                    div()
                        .text_sm()
                        .text_color(theme.muted)
                        .mr_2()
                        .child(t!("appearance-scheme-variant")),
                );
            for variant in variants {
                let id = variant.id;
                list = list.child(
                    option(
                        format!("appearance-variant-{id}"),
                        variant.name,
                        variant.id == chosen.id,
                        theme,
                    )
                    .test_support()
                    .reachable(theme)
                    .on_click(move |_, _, cx| {
                        appearance::edit(cx, |a| {
                            match mode {
                                Mode::Dark => a.scheme_dark = Some(id.into()),
                                Mode::Light => a.scheme_light = Some(id.into()),
                            }
                            a.theme = None;
                        });
                    }),
                );
            }
            d.child(list)
        })
        .when(!chosen.source.is_empty() && !theme_in_use, |d| {
            d.child(
                div()
                    .text_xs()
                    .text_color(theme.faint)
                    .child(t!("appearance-scheme-source", source = chosen.source)),
            )
        })
}

fn accent_name(accent: AppearanceAccent) -> &'static str {
    match accent {
        AppearanceAccent::Lavender => t!("appearance-accent-lavender"),
        AppearanceAccent::Indigo => t!("appearance-accent-indigo"),
        AppearanceAccent::Blue => t!("appearance-accent-blue"),
        AppearanceAccent::Sky => t!("appearance-accent-sky"),
        AppearanceAccent::Teal => t!("appearance-accent-teal"),
        AppearanceAccent::Emerald => t!("appearance-accent-emerald"),
        AppearanceAccent::Lime => t!("appearance-accent-lime"),
        AppearanceAccent::Amber => t!("appearance-accent-amber"),
        AppearanceAccent::Orange => t!("appearance-accent-orange"),
        AppearanceAccent::Rose => t!("appearance-accent-rose"),
        AppearanceAccent::Pink => t!("appearance-accent-pink"),
        AppearanceAccent::Fuchsia => t!("appearance-accent-fuchsia"),
        AppearanceAccent::Custom => t!("appearance-accent-custom"),
        _ => t!("appearance-accent-scheme"),
    }
}

fn accent_key(accent: AppearanceAccent) -> String {
    look::key(&accent)
}

fn accents(
    current: &Look,
    editor: &Entity<Editor>,
    theme: &Theme,
    cx: &App,
) -> impl IntoElement + use<> {
    let compiled_accent = theme.accent;
    let mut list = div()
        .id("appearance-accents")
        .test_support()
        .role(Role::RadioGroup)
        .aria_label(t!("appearance-accent"))
        .flex()
        .flex_wrap()
        .items_center()
        .gap_1p5();
    let presets = std::iter::once((AppearanceAccent::Scheme, None))
        .chain(look::ACCENTS.iter().map(|(a, c)| (*a, Some(*c))));
    for (accent, color) in presets {
        let selected = current.accent == accent;
        let fill = color.map(theme::hex).unwrap_or(compiled_accent);
        list = list.child(
            div()
                .id(ElementId::Name(
                    format!("appearance-accent-{}", accent_key(accent)).into(),
                ))
                .test_support()
                .role(Role::RadioButton)
                .reachable(theme)
                .aria_label(accent_name(accent))
                .aria_selected(selected)
                .size(px(24.))
                .rounded_full()
                .bg(fill)
                .border_2()
                .border_color(if selected { theme.text } else { theme.border })
                .cursor_pointer()
                .when(accent == AppearanceAccent::Scheme, |d| {
                    d.flex()
                        .items_center()
                        .justify_center()
                        .text_xs()
                        .text_color(theme.on_accent)
                        .child("S")
                })
                .on_click(move |_, _, cx| {
                    appearance::edit(cx, |a| a.accent = Some(accent));
                }),
        );
    }
    let input = editor.read(cx).accent.clone();
    let editor = editor.clone();
    let custom_selected = current.accent == AppearanceAccent::Custom;
    div()
        .flex()
        .flex_col()
        .gap_2()
        .child(
            div()
                .text_sm()
                .text_color(theme.muted)
                .child(t!("appearance-accent-help")),
        )
        .child(list)
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .text_sm()
                        .text_color(if custom_selected {
                            theme.text
                        } else {
                            theme.muted
                        })
                        .child(t!("appearance-accent-custom")),
                )
                .child(
                    div().w(px(120.)).child(
                        Input::new(&input)
                            .id("appearance-accent-custom-input")
                            .aria_label(t!("appearance-accent-custom")),
                    ),
                )
                .child(
                    button(
                        "appearance-accent-custom-apply",
                        t!("appearance-accent-custom-apply"),
                        theme,
                    )
                    .test_support()
                    .reachable(theme)
                    .on_click(move |_, _, cx| {
                        let text = input.read(cx).value().trim().to_string();
                        let text = if text.starts_with('#') {
                            text
                        } else {
                            format!("#{text}")
                        };
                        match look::parse_hex(&text) {
                            Some(color) => {
                                appearance::edit(cx, |a| {
                                    a.accent = Some(AppearanceAccent::Custom);
                                    a.accent_custom = Some(look::to_hex(color));
                                });
                                editor.update(cx, |e, _| e.notice = None);
                            }
                            None => say(&editor, t!("appearance-accent-custom-invalid"), true, cx),
                        }
                    }),
                ),
        )
}

fn font_name(font: AppearanceFont) -> &'static str {
    match font {
        AppearanceFont::Inter => t!("appearance-font-inter"),
        AppearanceFont::IbmMinusPlex => t!("appearance-font-ibm-plex"),
        AppearanceFont::Manrope => t!("appearance-font-manrope"),
        AppearanceFont::Nunito => t!("appearance-font-nunito"),
        AppearanceFont::Atkinson => t!("appearance-font-atkinson"),
        AppearanceFont::SourceMinusSans => t!("appearance-font-source-sans"),
        AppearanceFont::System => t!("appearance-font-system"),
        _ => t!("appearance-font-geist"),
    }
}

fn fonts(current: &Look, theme: &Theme) -> impl IntoElement + use<> {
    let mut list = div()
        .id("appearance-fonts")
        .test_support()
        .role(Role::RadioGroup)
        .aria_label(t!("appearance-font"))
        .flex()
        .flex_wrap()
        .gap_1();
    for font in look::FONTS {
        let font = *font;
        let key = look::key(&font);
        let family: SharedString = look::font_family(font).unwrap_or(".SystemUIFont").into();
        list = list.child(
            option(
                format!("appearance-font-{key}"),
                font_name(font),
                current.font == font,
                theme,
            )
            .font_family(family)
            .test_support()
            .reachable(theme)
            .on_click(move |_, _, cx| {
                appearance::edit(cx, |a| a.font = Some(font));
            }),
        );
    }
    div()
        .flex()
        .flex_col()
        .gap_2()
        .child(
            div()
                .text_sm()
                .text_color(theme.muted)
                .child(t!("appearance-font-help")),
        )
        .child(list)
}

fn scale(value: i64, theme: &Theme) -> impl IntoElement + use<> {
    let step = |delta: i64| {
        move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
            let next = (value + delta).clamp(*look::SCALE.start(), *look::SCALE.end());
            appearance::edit(cx, |a| a.interface_scale = Some(next));
        }
    };
    div()
        .flex()
        .items_center()
        .gap_2()
        .child(
            button("appearance-scale-down", "−", theme)
                .aria_label(t!("appearance-scale-down"))
                .test_support()
                .on_click(step(-5)),
        )
        .child(
            div()
                .id("appearance-scale-value")
                .test_support()
                .role(Role::Status)
                .aria_label(SharedString::from(t!(
                    "appearance-scale-value",
                    percent = value
                )))
                .w(px(56.))
                .text_center()
                .text_color(theme.text)
                .child(t!("appearance-scale-value", percent = value)),
        )
        .child(
            button("appearance-scale-up", "+", theme)
                .aria_label(t!("appearance-scale-up"))
                .test_support()
                .on_click(step(5)),
        )
}

// ---- Your themes ----

fn themes_block(
    current: &Look,
    mode: Mode,
    editor: &Entity<Editor>,
    theme: &Theme,
    cx: &App,
) -> impl IntoElement + use<> {
    let signed_in = editor.read(cx).session.read(cx).client().is_some();
    let library = appearance::library(cx);
    let e = editor.read(cx);
    let notice = e.notice.clone();
    let editing = e.editing.clone();
    let busy = e.busy;
    let name_input = e.name.clone();
    let import_input = e.import.clone();
    let seeds = e.seeds.clone();
    let mut body = div()
        .id("appearance-theme-library")
        .test_support()
        .role(Role::List)
        .aria_label(t!("appearance-themes"))
        .flex()
        .flex_col()
        .gap_2()
        .child(
            div()
                .text_sm()
                .text_color(theme.muted)
                .child(t!("appearance-themes-help")),
        );
    if let Some((text, problem)) = notice {
        body = body.child(
            div()
                .id("appearance-themes-notice")
                .test_support()
                .role(if problem { Role::Alert } else { Role::Status })
                .aria_label(text.clone())
                .text_sm()
                .text_color(if problem { theme.danger } else { theme.online })
                .child(text),
        );
    }
    if !signed_in {
        return body.child(
            div()
                .text_sm()
                .text_color(theme.muted)
                .child(t!("appearance-themes-signed-out")),
        );
    }
    let Some(library) = library else {
        return body.child(
            div()
                .text_sm()
                .text_color(theme.muted)
                .child(t!("appearance-themes-loading")),
        );
    };
    if library.themes.is_empty() {
        body = body.child(
            div()
                .text_sm()
                .text_color(theme.muted)
                .child(t!("appearance-themes-empty")),
        );
    }
    for user_theme in &library.themes {
        body = body.child(theme_row(user_theme, current, mode, editor, theme));
    }
    if let Some((user_theme, editing_mode)) = editing {
        body = body.child(seed_editor(
            &user_theme,
            editing_mode,
            &seeds,
            editor,
            theme,
            cx,
        ));
    }
    let full = library.themes.len() >= look::MAX_THEMES;
    if full {
        body = body.child(
            div()
                .text_sm()
                .text_color(theme.warning)
                .child(t!("appearance-themes-full")),
        );
    }
    let new_editor = editor.clone();
    let paste_editor = editor.clone();
    let file_editor = editor.clone();
    let current = current.clone();
    body.child(
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_2()
            .child(
                div().w(px(180.)).child(
                    Input::new(&name_input)
                        .id("appearance-themes-name")
                        .aria_label(t!("appearance-themes-name")),
                ),
            )
            .when(!full && !busy, |d| {
                d.child(
                    button("appearance-themes-new", t!("appearance-themes-new"), theme)
                        .test_support()
                        .reachable(theme)
                        .on_click(move |_, _, cx| {
                            let name = name_input.read(cx).value().to_string();
                            let name = if name.trim().is_empty() {
                                t!("appearance-themes-new-name").to_string()
                            } else {
                                name
                            };
                            let made = look::new_theme(&current, &name, made_with(), &now());
                            save(&new_editor, made, None, true, None, cx);
                        }),
                )
            }),
    )
    .when(!full && !busy, |d| {
        d.child(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap_2()
                .child(
                    div().w(px(260.)).child(
                        Input::new(&import_input)
                            .id("appearance-themes-paste")
                            .aria_label(t!("appearance-themes-import-paste")),
                    ),
                )
                .child(
                    button(
                        "appearance-themes-import-paste",
                        t!("appearance-themes-import-paste"),
                        theme,
                    )
                    .test_support()
                    .reachable(theme)
                    .on_click(move |_, _, cx| {
                        let pasted = import_input.read(cx).value().to_string();
                        let text = if pasted.trim().is_empty() {
                            cx.read_from_clipboard().and_then(|c| c.text())
                        } else {
                            Some(pasted)
                        };
                        match text.filter(|t| !t.trim().is_empty()) {
                            Some(text) => import(&paste_editor, &text, cx),
                            None => say(
                                &paste_editor,
                                t!("appearance-themes-import-empty"),
                                true,
                                cx,
                            ),
                        }
                    }),
                )
                .child(
                    button(
                        "appearance-themes-import-file",
                        t!("appearance-themes-import-file"),
                        theme,
                    )
                    .test_support()
                    .reachable(theme)
                    .on_click(move |_, window, cx| import_file(&file_editor, window, cx)),
                ),
        )
    })
}

fn theme_row(
    user_theme: &UserTheme,
    current: &Look,
    mode: Mode,
    editor: &Entity<Editor>,
    theme: &Theme,
) -> impl IntoElement + use<> {
    let id = user_theme.id.clone().unwrap_or_default();
    let name: SharedString = user_theme.name.clone().unwrap_or_default().into();
    let in_use = current.theme.as_deref() == Some(id.as_str());
    let supported = look::theme_supported(user_theme);
    let seeds = look::drawn_seeds(user_theme, mode);
    let key = |what: &str| format!("appearance-theme-{what}-{id}");
    let use_id = id.clone();
    let edit_theme = user_theme.clone();
    let edit_editor = editor.clone();
    let export_theme = user_theme.clone();
    let copy_theme = user_theme.clone();
    let copy_editor = editor.clone();
    let delete_editor = editor.clone();
    let delete_id = id.clone();
    let delete_name = name.clone();
    div()
        .id(ElementId::Name(key("row").into()))
        .test_support()
        .role(Role::ListItem)
        .aria_label(name.clone())
        .flex()
        .flex_wrap()
        .items_center()
        .gap_2()
        .p_2()
        .rounded(theme.radius)
        .border_1()
        .border_color(if in_use { theme.accent } else { theme.border })
        .child(
            div()
                .flex()
                .gap_1()
                .child(swatch(seeds.canvas, theme))
                .child(swatch(seeds.surface, theme))
                .child(swatch(seeds.accent, theme)),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(120.))
                .text_color(theme.text)
                .child(name.clone()),
        )
        .when(in_use, |d| {
            d.child(
                div()
                    .text_xs()
                    .text_color(theme.accent)
                    .child(t!("appearance-themes-in-use")),
            )
        })
        .when(!supported, |d| {
            d.child(
                div()
                    .text_xs()
                    .text_color(theme.warning)
                    .child(t!("appearance-themes-newer")),
            )
        })
        .when(supported && !in_use, |d| {
            d.child(
                button(key("use"), t!("appearance-themes-use"), theme)
                    .aria_label(SharedString::from(t!(
                        "appearance-themes-use-named",
                        name = name
                    )))
                    .test_support()
                    .on_click(move |_, _, cx| {
                        let id = use_id.clone();
                        appearance::edit(cx, |a| a.theme = Some(id));
                    }),
            )
        })
        .when(in_use, |d| {
            d.child(
                button(key("stop"), t!("appearance-themes-stop"), theme)
                    .test_support()
                    .reachable(theme)
                    .on_click(|_, _, cx| {
                        appearance::edit(cx, |a| a.theme = None);
                    }),
            )
        })
        .when(supported, |d| {
            d.child(
                button(key("edit"), t!("appearance-themes-edit"), theme)
                    .aria_label(SharedString::from(t!(
                        "appearance-themes-edit-named",
                        name = name
                    )))
                    .test_support()
                    .on_click(move |_, _, cx| start_editing(&edit_editor, &edit_theme, mode, cx)),
            )
        })
        .child(
            button(key("export"), t!("appearance-themes-export"), theme)
                .aria_label(SharedString::from(t!(
                    "appearance-themes-export-named",
                    name = name
                )))
                .test_support()
                .on_click(move |_, window, cx| export_file(&export_theme, window, cx)),
        )
        .child(
            button(key("copy"), t!("appearance-themes-copy"), theme)
                .aria_label(SharedString::from(t!(
                    "appearance-themes-copy-named",
                    name = name
                )))
                .test_support()
                .on_click(move |_, _, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(look::export_theme(
                        &copy_theme,
                    )));
                    let name = copy_theme.name.clone().unwrap_or_default();
                    say(
                        &copy_editor,
                        t!("appearance-themes-copied", name = name),
                        false,
                        cx,
                    );
                }),
        )
        .child(
            button(key("delete"), t!("appearance-themes-delete"), theme)
                .aria_label(SharedString::from(t!(
                    "appearance-themes-delete-named",
                    name = name
                )))
                .text_color(theme.danger)
                .test_support()
                .on_click(move |_, window, cx| {
                    confirm_delete(&delete_editor, &delete_id, &delete_name, in_use, window, cx)
                }),
        )
}

fn seed_label(seed: Seed) -> &'static str {
    match seed {
        Seed::Canvas => t!("appearance-seed-canvas"),
        Seed::Surface => t!("appearance-seed-surface"),
        Seed::Overlay => t!("appearance-seed-overlay"),
        Seed::Ink => t!("appearance-seed-ink"),
        Seed::Muted => t!("appearance-seed-muted"),
        Seed::Accent => t!("appearance-seed-accent"),
    }
}

fn seed_key(seed: Seed) -> &'static str {
    match seed {
        Seed::Canvas => "canvas",
        Seed::Surface => "surface",
        Seed::Overlay => "overlay",
        Seed::Ink => "ink",
        Seed::Muted => "muted",
        Seed::Accent => "accent",
    }
}

fn start_editing(editor: &Entity<Editor>, user_theme: &UserTheme, mode: Mode, cx: &mut App) {
    let seeds = look::drawn_seeds(user_theme, mode);
    let inputs = editor.read(cx).seeds.clone();
    let windows = cx.windows();
    for (seed, input) in inputs {
        let value = look::to_hex(seed.get(&seeds));
        // Setting a field's text needs a window; any of the app's will do.
        if let Some(window) = windows.first() {
            let _ = window.update(cx, |_, window, cx| {
                input.update(cx, |s, cx| s.set_value(value, window, cx));
            });
        }
    }
    editor.update(cx, |e, _| {
        e.editing = Some((user_theme.clone(), mode));
        e.notice = None;
    });
    cx.refresh_windows();
}

fn seed_editor(
    user_theme: &UserTheme,
    mode: Mode,
    inputs: &[(Seed, Entity<InputState>)],
    editor: &Entity<Editor>,
    theme: &Theme,
    _cx: &App,
) -> impl IntoElement + use<> {
    let name = user_theme.name.clone().unwrap_or_default();
    let mode_name = match mode {
        Mode::Dark => t!("appearance-themes-editing-dark"),
        Mode::Light => t!("appearance-themes-editing-light"),
    };
    let mut form = div()
        .id("appearance-theme-editor")
        .test_support()
        .role(Role::Form)
        .aria_label(SharedString::from(t!(
            "appearance-themes-editing",
            name = name,
            mode = mode_name
        )))
        .flex()
        .flex_col()
        .gap_2()
        .p_3()
        .rounded(theme.radius_large)
        .bg(theme.panel)
        .child(
            div()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.text)
                .child(t!(
                    "appearance-themes-editing",
                    name = name,
                    mode = mode_name
                )),
        );
    for (seed, input) in inputs {
        form = form.child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .w(px(120.))
                        .text_sm()
                        .text_color(theme.text)
                        .child(seed_label(*seed)),
                )
                .child(
                    div().w(px(120.)).child(
                        Input::new(input)
                            .id(SharedString::from(format!(
                                "appearance-seed-{}",
                                seed_key(*seed)
                            )))
                            .aria_label(seed_label(*seed)),
                    ),
                ),
        );
    }
    let save_editor = editor.clone();
    let cancel_editor = editor.clone();
    let edited = user_theme.clone();
    let inputs: Vec<_> = inputs.to_vec();
    form.child(
        div()
            .flex()
            .gap_2()
            .child(
                button("appearance-theme-save", t!("appearance-themes-save"), theme)
                    .test_support()
                    .reachable(theme)
                    .on_click(move |_, _, cx| {
                        let mut next = edited.clone();
                        for (seed, input) in &inputs {
                            let text = input.read(cx).value().trim().to_string();
                            let text = if text.starts_with('#') {
                                text
                            } else {
                                format!("#{text}")
                            };
                            let Some(color) = look::parse_hex(&text) else {
                                say(
                                    &save_editor,
                                    t!("appearance-accent-custom-invalid"),
                                    true,
                                    cx,
                                );
                                return;
                            };
                            look::set_seed(&mut next, mode, *seed, color);
                        }
                        let expected = edited.updated_at.clone();
                        next.made_with = Some(made_with().into());
                        save(&save_editor, next, expected, false, None, cx);
                    }),
            )
            .child(
                button(
                    "appearance-theme-cancel",
                    t!("appearance-themes-cancel"),
                    theme,
                )
                .test_support()
                .reachable(theme)
                .on_click(move |_, _, cx| {
                    cancel_editor.update(cx, |e, _| e.editing = None);
                    cx.refresh_windows();
                }),
            ),
    )
}

fn made_with() -> &'static str {
    concat!("IRCord desktop ", env!("CARGO_PKG_VERSION"))
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// Save `user_theme` to the account; `then_use` applies it once stored.
fn save(
    editor: &Entity<Editor>,
    user_theme: UserTheme,
    expected: Option<String>,
    then_use: bool,
    done: Option<String>,
    cx: &mut App,
) {
    let Some(client) = editor.read(cx).session.read(cx).client().cloned() else {
        return;
    };
    editor.update(cx, |e, _| e.busy = true);
    let id = user_theme.id.clone();
    let task = client.request(move |api| async move {
        api.save_theme(&user_theme, expected.as_deref(), false)
            .await
    });
    let editor = editor.clone();
    cx.spawn(async move |cx| {
        let result = task.await;
        cx.update(|cx| {
            editor.update(cx, |e, _| e.busy = false);
            match result {
                Ok(Ok(themes)) => {
                    appearance::set_library(themes, cx);
                    editor.update(cx, |e, _| {
                        e.editing = None;
                        e.notice = done.map(|text| (text.into(), false));
                    });
                    if then_use {
                        appearance::edit(cx, |a| a.theme = id);
                    }
                    appearance::apply(cx);
                }
                Ok(Err(ircord_client::api::ApiError::Http { status: 412, .. })) => {
                    editor.update(cx, |e, _| e.editing = None);
                    say(&editor, t!("appearance-themes-conflict"), true, cx);
                    client.watch_document(&ircord_client::documents::THEMES);
                }
                Ok(Err(e)) => say(&editor, t!("appearance-themes-failed", error = e), true, cx),
                Err(e) => say(&editor, t!("appearance-themes-failed", error = e), true, cx),
            }
            cx.refresh_windows();
        });
    })
    .detach();
}

fn import_error(e: ImportError) -> &'static str {
    match e {
        ImportError::TooLarge => t!("appearance-themes-import-too-large"),
        ImportError::NotJson => t!("appearance-themes-import-not-json"),
        ImportError::NotTheme => t!("appearance-themes-import-not-theme"),
        ImportError::Newer => t!("appearance-themes-import-newer"),
        ImportError::CssTooLong => t!("appearance-themes-import-css-too-long"),
    }
}

/// Add an exported theme to the library, unused.
fn import(editor: &Entity<Editor>, text: &str, cx: &mut App) {
    match look::import_theme(text, made_with(), &now()) {
        Ok(user_theme) => {
            let name = user_theme.name.clone().unwrap_or_default();
            let note = if look::has_css(&user_theme) {
                t!("appearance-themes-css-kept", name = name)
            } else {
                t!("appearance-themes-imported", name = name)
            };
            save(editor, user_theme, None, false, Some(note), cx);
            let input = editor.read(cx).import.clone();
            if let Some(window) = cx.windows().first() {
                let _ = window.update(cx, |_, window, cx| {
                    input.update(cx, |s, cx| s.set_value("", window, cx));
                });
            }
        }
        Err(e) => say(editor, import_error(e), true, cx),
    }
}

fn import_file(editor: &Entity<Editor>, window: &mut Window, cx: &mut App) {
    let picked = cx.prompt_for_paths(PathPromptOptions {
        files: true,
        directories: false,
        multiple: false,
        prompt: Some(t!("appearance-themes-import-file").into()),
    });
    let editor = editor.clone();
    window
        .spawn(cx, async move |cx| {
            let Ok(Ok(Some(paths))) = picked.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let text = std::fs::metadata(&path)
                .ok()
                .filter(|m| m.len() as usize <= look::MAX_IMPORT)
                .and_then(|_| std::fs::read_to_string(&path).ok());
            cx.update(|_, cx| match text {
                Some(text) => import(&editor, &text, cx),
                None => say(&editor, t!("appearance-themes-import-too-large"), true, cx),
            })
            .ok();
        })
        .detach();
}

fn export_file(user_theme: &UserTheme, window: &mut Window, cx: &mut App) {
    let dir = std::env::home_dir()
        .map(|h| {
            let d = h.join("Downloads");
            if d.is_dir() { d } else { h }
        })
        .unwrap_or_else(|| PathBuf::from("."));
    let target = cx.prompt_for_new_path(&dir, Some(&look::theme_file_name(user_theme)));
    let text = look::export_theme(user_theme);
    let name = user_theme.name.clone().unwrap_or_default();
    window
        .spawn(cx, async move |cx| {
            let Ok(Ok(Some(path))) = target.await else {
                return;
            };
            let result = std::fs::write(&path, text);
            cx.update(|window, cx| match result {
                Ok(()) => {
                    ui::toast::success(window, cx, t!("appearance-themes-exported", name = name))
                }
                Err(e) => ui::toast::error(window, cx, t!("appearance-themes-failed", error = e)),
            })
            .ok();
        })
        .detach();
}

fn confirm_delete(
    editor: &Entity<Editor>,
    id: &str,
    name: &SharedString,
    in_use: bool,
    window: &mut Window,
    cx: &mut App,
) {
    let editor = editor.clone();
    let id = Rc::new(id.to_string());
    ui::confirm::confirm(
        window,
        cx,
        ui::confirm::Confirm {
            title: t!("appearance-themes-delete-title", name = name).into(),
            body: if in_use {
                t!("appearance-themes-delete-body-active")
            } else {
                t!("appearance-themes-delete-body")
            }
            .into(),
            ok: t!("appearance-themes-delete").into(),
            danger: true,
        },
        move |_, cx| delete(&editor, &id, in_use, cx),
    );
}

fn delete(editor: &Entity<Editor>, id: &str, in_use: bool, cx: &mut App) {
    let Some(client) = editor.read(cx).session.read(cx).client().cloned() else {
        return;
    };
    if in_use {
        appearance::edit(cx, |a| a.theme = None);
    }
    let id = id.to_string();
    let task = client.request(move |api| async move { api.delete_theme(&id).await });
    let editor = editor.clone();
    cx.spawn(async move |cx| {
        let result = task.await;
        cx.update(|cx| match result {
            Ok(Ok(themes)) => {
                appearance::set_library(themes, cx);
                editor.update(cx, |e, _| e.editing = None);
                cx.refresh_windows();
            }
            Ok(Err(e)) => say(&editor, t!("appearance-themes-failed", error = e), true, cx),
            Err(e) => say(&editor, t!("appearance-themes-failed", error = e), true, cx),
        });
    })
    .detach();
}

fn confirm_reset(window: &mut Window, cx: &mut App) {
    ui::confirm::confirm(
        window,
        cx,
        ui::confirm::Confirm {
            title: t!("appearance-reset-title").into(),
            body: t!("appearance-reset-body").into(),
            ok: t!("appearance-reset-ok").into(),
            danger: false,
        },
        |_, cx| {
            appearance::edit(cx, look::reset_look);
        },
    );
}

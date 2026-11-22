//! Settings → Appearance (package S1), as the web client's
//! `AppearanceSettings.vue` and theme library: sync across devices, light or
//! dark, the colour scheme and its variant, the accent, the interface font,
//! scale, sidebar density, corners and motion, your themes (new, use, edit
//! colours and backgrounds, export, import, delete) and reset.
//!
//! Appearance preferences use `theme::appearance::edit`. Personal theme edits
//! stay in their draft until saved to the account.

mod background;
mod colors;

use crate::ui::focus::Reachable;
use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;

use gpui_kit::TestSupportExt;
use gpui_kit::component::Disableable;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::menu::{DropdownMenu, PopupMenuItem};
use gpui_kit::component::switch::Switch;
use gpui_kit::prelude::*;
use gpui_kit::*;
use ircord_client::contract::{
    Appearance, AppearanceAccent, AppearanceCorners, AppearanceDensity, AppearanceFont,
    AppearanceMode, AppearanceMotion, Theme as UserTheme,
};
use ircord_client::store::appearance::{self as look, Compiled, FAMILIES, ImportError, Look, Mode};
use ircord_client::store::theme_css::CssVariables;

use crate::registry::{self, SettingsCtx, SettingsGroup, SettingsRow, SettingsSection};
use crate::session::{Session, SessionEvent};
use crate::settings::messages;
use crate::settings::{block, row};
use crate::theme::{self, Theme, appearance};
use crate::ui;

pub fn init(cx: &mut App) {
    registry::register(cx, |r| {
        let rows: [(&str, &'static str, &'static str, &'static str); 12] = [
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
                "appearance-scheme-variants",
                t!("appearance-scheme-variants"),
                t!("appearance-scheme-variants-help"),
                t!("appearance-scheme-variants-keywords"),
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
    css: Entity<TextareaState>,
    seeds: Vec<colors::Field>,
    /// The unsaved theme and its editing mode.
    editing: Option<(UserTheme, Mode)>,
    tab: ThemeTab,
    studio_open: bool,
    collapsed: bool,
    compare: bool,
    new_theme: bool,
    focus: FocusHandle,
    background_preview: Entity<crate::theme::background::Wallpaper>,
    edit_generation: u64,
    /// What the last action said, and whether it was a problem.
    notice: Option<(SharedString, bool)>,
    busy: bool,
    _session: Subscription,
    _inputs: Vec<Subscription>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ThemeTab {
    Colors,
    Background,
    Css,
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
    let css = cx.new(|cx| TextareaState::new(window, cx).rows(6));
    let seeds = colors::fields(window, cx);
    let background_preview = cx.new(|_| crate::theme::background::Wallpaper::default());
    let e = cx.new(|cx: &mut Context<Editor>| {
        let mut inputs: Vec<_> = seeds
            .iter()
            .flat_map(|field| field.subscribe(window, cx))
            .collect();
        inputs.push(cx.subscribe(&name, |_, _, event, cx| draft_input_changed(event, cx)));
        inputs.push(cx.subscribe(&css, |_, _, event, cx| draft_input_changed(event, cx)));
        Editor {
            session: ctx.session.clone(),
            accent,
            name,
            import,
            css,
            seeds,
            editing: None,
            tab: ThemeTab::Colors,
            studio_open: false,
            collapsed: false,
            compare: false,
            new_theme: false,
            focus: cx.focus_handle(),
            background_preview,
            edit_generation: 0,
            notice: None,
            busy: false,
            _session: cx.subscribe(&ctx.session, |editor, _, event: &SessionEvent, cx| {
                if matches!(event, SessionEvent::Phase) {
                    editor
                        .background_preview
                        .update(cx, |preview, cx| preview.clear(None, cx));
                    editor.studio_open = false;
                    editor.editing = None;
                    editor.edit_generation = editor.edit_generation.wrapping_add(1);
                    editor.notice = None;
                    editor.busy = false;
                }
            }),
            _inputs: inputs,
        }
    });
    cx.set_global(Editors(e.clone()));
    e
}

fn draft_input_changed(event: &InputEvent, cx: &mut Context<Editor>) {
    if matches!(event, InputEvent::Change) {
        let editor = cx.entity();
        cx.defer(move |cx| refresh_studio(&editor, cx));
    }
}

fn say(editor: &Entity<Editor>, text: impl Into<SharedString>, problem: bool, cx: &mut App) {
    let text = text.into();
    editor.update(cx, |e, _| e.notice = Some((text, problem)));
    cx.refresh_windows();
}

// ---- Rendering ----

pub(super) fn sync_in_header(window: &Window) -> bool {
    window.viewport_size().width > px(700.)
}

fn sync_switch(sync: bool) -> Switch {
    Switch::new("appearance-sync-switch")
        .checked(sync)
        .accessibility_label(t!("appearance-sync"))
        .on_click(|on, _, cx| appearance::set_sync(cx, *on))
}

fn sync_help(sync: bool) -> &'static str {
    if sync {
        t!("appearance-sync-help")
    } else {
        t!("appearance-sync-local")
    }
}

pub(super) fn sync_header(
    theme: &Theme,
    sync: bool,
    highlighted: bool,
) -> impl IntoElement + use<> {
    let icon = if sync {
        ui::icon::LucideIcon::Cloud
    } else {
        ui::icon::LucideIcon::CloudOff
    };
    div()
        .id("setting-appearance-sync")
        .test_support()
        .role(Role::Group)
        .aria_label(t!("appearance-sync"))
        .tooltip(ui::tooltip::text(sync_help(sync)))
        .flex()
        .items_center()
        .gap_2()
        .pl_3()
        .pr_1p5()
        .py_1()
        .rounded_full()
        .bg(theme.raised.opacity(0.45))
        .border_1()
        .border_color(if highlighted {
            theme.accent
        } else {
            theme.border.opacity(0.55)
        })
        .text_xs()
        .text_color(theme.muted)
        .child(ui::icon::icon(icon, theme.muted))
        .child(t!("appearance-sync-short"))
        .child(sync_switch(sync))
}

fn render(ctx: &SettingsCtx, window: &mut Window, cx: &mut App) -> impl IntoElement {
    let theme = theme::get(cx).clone();
    let editor = editor(ctx, window, cx);
    let current = appearance::look(cx);
    let compiled = appearance::compiled(cx);
    let sync = appearance::sync(cx);
    let compact = !sync_in_header(window);
    let mode = compiled.mode;

    div()
        .id("appearance-settings")
        .test_support()
        .role(Role::Group)
        .aria_label(t!("appearance-title"))
        .flex()
        .flex_col()
        .gap_4()
        .when(!compact && !sync, |d| {
            d.child(
                div()
                    .id("appearance-local-only")
                    .test_support()
                    .role(Role::Status)
                    .aria_label(t!("appearance-sync-local"))
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_xs()
                    .text_color(theme.muted)
                    .child(ui::icon::icon(ui::icon::LucideIcon::CloudOff, theme.muted))
                    .child(t!("appearance-sync-local")),
            )
        })
        .child(super::preview(&theme, crate::message::layout::settings(cx)))
        .when(compact, |d| {
            d.child(row(
                ctx,
                "appearance-sync",
                t!("appearance-sync"),
                Some(sync_help(sync).into()),
                sync_switch(sync),
                &theme,
            ))
        })
        .child(row(
            ctx,
            "appearance-mode",
            t!("appearance-mode"),
            Some(t!("appearance-mode-help").into()),
            mode_control(current.mode, &theme),
            &theme,
        ))
        .child(block(
            ctx,
            "appearance-scheme",
            t!("appearance-scheme"),
            schemes(&current, mode, compiled.theme.is_some(), &theme, window, cx),
            &theme,
        ))
        .child(block(
            ctx,
            "appearance-scheme-variants",
            t!("appearance-scheme-variants"),
            scheme_variants(&current, mode, compiled.theme.is_some()),
            &theme,
        ))
        .child(block(
            ctx,
            "appearance-accent",
            t!("appearance-accent"),
            under(
                accents(&current, &editor, &theme, cx),
                &["accent"],
                &compiled,
                &theme,
            ),
            &theme,
        ))
        .child(block(
            ctx,
            "appearance-font",
            t!("appearance-font"),
            under(
                fonts(&current, &theme, window, cx),
                &["font-ui"],
                &compiled,
                &theme,
            ),
            &theme,
        ))
        .child(under(
            row(
                ctx,
                "appearance-scale",
                t!("appearance-scale"),
                Some(t!("appearance-scale-help").into()),
                scale(current.scale, &theme, window, cx),
                &theme,
            ),
            &["interface-scale"],
            &compiled,
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
        .child(under(
            row(
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
            ),
            &["radius", "radius-large"],
            &compiled,
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

/// A row or block with a caption under it when the theme's CSS sets one of
/// `names` (the variables the row shows).
fn under(
    content: impl IntoElement,
    names: &[&'static str],
    compiled: &Compiled,
    theme: &Theme,
) -> Div {
    let applied = compiled
        .css
        .as_ref()
        .map(CssVariables::applied)
        .unwrap_or_default();
    div().flex().flex_col().gap_2().child(content).when(
        applied.iter().any(|a| names.contains(a)),
        |d| {
            d.child(
                div()
                    .text_sm()
                    .text_color(theme.muted)
                    .child(t!("appearance-set-by-css")),
            )
        },
    )
}

/// What the theme in use's CSS set and what this app left out, under the
/// theme's row.
fn css_report(id: &str, name: &str, css: &CssVariables, theme: &Theme) -> Div {
    let mut report = div()
        .flex()
        .flex_col()
        .gap_1()
        .text_sm()
        .text_color(theme.muted);
    let applied = css.applied().len();
    if applied > 0 {
        let text: SharedString = t!(
            "appearance-themes-css-applied",
            name = name,
            count = applied
        )
        .into();
        report = report.child(
            div()
                .id(ElementId::Name(format!("{id}-applied").into()))
                .test_support()
                .role(Role::Status)
                .aria_label(text.clone())
                .child(text),
        );
    }
    if css.skipped_rules > 0 || !css.unread.is_empty() {
        let text: SharedString = t!(
            "appearance-themes-css-skipped",
            rules = css.skipped_rules,
            values = css.unread.len()
        )
        .into();
        report = report.child(
            div()
                .id(ElementId::Name(format!("{id}-skipped").into()))
                .test_support()
                .role(Role::Status)
                .aria_label(text.clone())
                .child(text),
        );
    }
    report
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

fn mode_control(current: AppearanceMode, theme: &Theme) -> impl IntoElement + use<> {
    use ui::icon::LucideIcon as Icon;
    let modes = [
        (
            AppearanceMode::System,
            "system",
            t!("appearance-mode-system"),
            Icon::Monitor,
        ),
        (
            AppearanceMode::Dark,
            "dark",
            t!("appearance-mode-dark"),
            Icon::Moon,
        ),
        (
            AppearanceMode::Light,
            "light",
            t!("appearance-mode-light"),
            Icon::Sun,
        ),
    ];
    div()
        .id("appearance-mode-choices")
        .test_support()
        .role(Role::RadioGroup)
        .aria_label(t!("appearance-mode"))
        .flex()
        .gap_0p5()
        .p_0p5()
        .rounded(theme.radius)
        .border_1()
        .border_color(theme.border)
        .bg(theme.raised)
        .children(modes.map(|(value, key, text, icon)| {
            let selected = current == value;
            let hover = theme.hover;
            div()
                .id(ElementId::Name(format!("appearance-mode-{key}").into()))
                .test_support()
                .role(Role::RadioButton)
                .aria_label(text)
                .aria_selected(selected)
                .reachable(theme)
                .flex()
                .items_center()
                .gap_1p5()
                .px_2()
                .py_1()
                .rounded(theme.radius)
                .text_sm()
                .text_color(if selected { theme.text } else { theme.muted })
                .when(selected, |d| d.bg(theme.main).shadow_sm())
                .when(!selected, |d| d.hover(move |s| s.bg(hover)))
                .cursor_pointer()
                .child(ui::icon::icon(
                    icon,
                    if selected { theme.accent } else { theme.muted },
                ))
                .child(text)
                .on_click(move |_, _, cx| {
                    appearance::edit(cx, |a| a.mode = Some(value));
                })
        }))
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

/// A scheme drawn small, as the web's theme cards: its rail with two dots
/// beside a panel of text lines and an accent bar.
fn miniature(seeds: &look::Seeds) -> Div {
    let bar = |w: f32, color: u32| {
        div()
            .h(px(3.))
            .w(relative(w))
            .rounded_full()
            .bg(theme::hex(color))
    };
    div()
        .h(px(64.))
        .p_1p5()
        .flex()
        .gap_1p5()
        .rounded_md()
        .bg(theme::hex(seeds.canvas))
        .child(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .size(px(9.))
                        .rounded_full()
                        .bg(theme::hex(seeds.accent)),
                )
                .child(
                    div()
                        .size(px(9.))
                        .rounded_full()
                        .bg(theme::hex(seeds.overlay)),
                ),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .p_1p5()
                .flex()
                .flex_col()
                .gap_1p5()
                .rounded_sm()
                .bg(theme::hex(seeds.surface))
                .child(bar(0.6, seeds.ink))
                .child(bar(1., seeds.muted))
                .child(
                    div()
                        .h(px(6.))
                        .w(relative(0.4))
                        .rounded_full()
                        .bg(theme::hex(seeds.accent)),
                ),
        )
}

/// Draw equal-width cards, adding a column when another 150 px card fits.
fn card_grid(
    id: &'static str,
    label: &'static str,
    cards: Vec<AnyElement>,
    window: &mut Window,
    cx: &mut App,
) -> impl IntoElement + use<> {
    const MIN_WIDTH: f32 = 150.;
    const GAP: f32 = 10.;
    let columns = window
        .use_keyed_state((id, 0_usize), cx, |_, _| Rc::new(Cell::new(1u16)))
        .read(cx)
        .clone();
    let count = cards.len().max(1) as u16;
    div()
        .relative()
        .w_full()
        .child(
            div()
                .id(id)
                .test_support()
                .role(Role::RadioGroup)
                .aria_label(label)
                .w_full()
                .grid()
                .grid_cols(columns.get())
                .gap(px(GAP))
                .children(cards),
        )
        .child(
            canvas(
                move |bounds, window, _| {
                    let available = f32::from(bounds.size.width);
                    let fit = ((available + GAP) / (MIN_WIDTH + GAP)) as u16;
                    let next = fit.clamp(1, count);
                    if columns.replace(next) != next {
                        window.refresh();
                    }
                },
                |_, _, _, _| {},
            )
            .absolute()
            .inset_0()
            .size_full(),
        )
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
    window: &mut Window,
    cx: &mut App,
) -> impl IntoElement + use<> {
    let chosen = current.scheme(mode);
    let mut cards = Vec::new();
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
        cards.push(
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
                .min_w_0()
                .p_1p5()
                .rounded(theme.radius_large)
                .border_2()
                .border_color(if selected { theme.accent } else { theme.border })
                .bg(theme.main)
                .cursor_pointer()
                .flex()
                .flex_col()
                .gap_1()
                .child(miniature(&seeds))
                .child(
                    div()
                        .px_1()
                        .flex()
                        .items_center()
                        .gap_1()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.text)
                        .child(family.name)
                        .when(selected, |d| {
                            d.child(ui::icon::icon(ui::icon::IconName::Check, theme.muted))
                        }),
                )
                .when_some(family_description(family.id), |d, text| {
                    d.child(div().px_1().text_xs().text_color(theme.muted).child(text))
                })
                .when_some(only, |d, text| {
                    d.child(div().px_1().text_xs().text_color(theme.muted).child(text))
                })
                .on_click(move |_, _, cx| pick_family(family, cx))
                .into_any_element(),
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
                .child(t!("appearance-scheme-help")),
        )
        .child(card_grid(
            "appearance-schemes",
            t!("appearance-scheme"),
            cards,
            window,
            cx,
        ))
}

fn scheme_label(scheme: &look::Scheme) -> SharedString {
    let family = look::family(scheme.family).map(|f| f.name);
    match family {
        Some(family) if family != scheme.name => format!("{family} {}", scheme.name).into(),
        _ => scheme.name.into(),
    }
}

fn scheme_variants(current: &Look, mode: Mode, theme_in_use: bool) -> impl IntoElement + use<> {
    let source = current.scheme(mode).source;
    let controls = [
        (
            Mode::Dark,
            "appearance-dark-variant",
            t!("appearance-scheme-dark-variant"),
        ),
        (
            Mode::Light,
            "appearance-light-variant",
            t!("appearance-scheme-light-variant"),
        ),
    ];
    div()
        .flex()
        .flex_wrap()
        .items_end()
        .gap_3()
        .children(controls.map(|(mode, element_id, label)| {
            let active = current.scheme(mode);
            div()
                .w(px(220.))
                .max_w_full()
                .flex()
                .flex_col()
                .gap_1()
                .child(div().text_sm().child(label))
                .child(
                    Button::new(element_id)
                        .justify_start()
                        .w_full()
                        .label(scheme_label(active))
                        .accessibility_label(label)
                        .dropdown_caret(true)
                        .dropdown_menu_with_anchor(Anchor::TopLeft, move |menu, _, _| {
                            look::schemes().filter(|scheme| scheme.mode == mode).fold(
                                menu.max_h(px(290.)),
                                |menu, scheme| {
                                    menu.item(
                                        PopupMenuItem::new(scheme_label(scheme))
                                            .checked(scheme.id == active.id)
                                            .on_click(move |_, _, cx| {
                                                appearance::edit(cx, |a| match mode {
                                                    Mode::Dark => {
                                                        a.scheme_dark = Some(scheme.id.into())
                                                    }
                                                    Mode::Light => {
                                                        a.scheme_light = Some(scheme.id.into())
                                                    }
                                                });
                                            }),
                                    )
                                },
                            )
                        }),
                )
        }))
        .when(!theme_in_use && !source.is_empty(), |d| {
            d.child(
                Button::new("appearance-scheme-source")
                    .link()
                    .label(t!("appearance-scheme-source"))
                    .on_click(move |_, _, cx| cx.open_url(source)),
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

fn font_labels(font: AppearanceFont) -> (&'static str, &'static str) {
    match font {
        AppearanceFont::Inter => (
            t!("appearance-font-inter"),
            t!("appearance-font-inter-note"),
        ),
        AppearanceFont::IbmMinusPlex => (
            t!("appearance-font-ibm-plex"),
            t!("appearance-font-ibm-plex-note"),
        ),
        AppearanceFont::Manrope => (
            t!("appearance-font-manrope"),
            t!("appearance-font-manrope-note"),
        ),
        AppearanceFont::Nunito => (
            t!("appearance-font-nunito"),
            t!("appearance-font-nunito-note"),
        ),
        AppearanceFont::Atkinson => (
            t!("appearance-font-atkinson"),
            t!("appearance-font-atkinson-note"),
        ),
        AppearanceFont::SourceMinusSans => (
            t!("appearance-font-source-sans"),
            t!("appearance-font-source-sans-note"),
        ),
        AppearanceFont::System => (
            t!("appearance-font-system"),
            t!("appearance-font-system-note"),
        ),
        _ => (
            t!("appearance-font-geist"),
            t!("appearance-font-geist-note"),
        ),
    }
}

fn fonts(
    current: &Look,
    theme: &Theme,
    window: &mut Window,
    cx: &mut App,
) -> impl IntoElement + use<> {
    let mut cards = Vec::new();
    for font in look::FONTS {
        let font = *font;
        let key = look::key(&font);
        let family: SharedString = look::font_family(font).unwrap_or(".SystemUIFont").into();
        let selected = current.font == font;
        let hover = theme.hover;
        let (name, description) = font_labels(font);
        cards.push(
            div()
                .id(ElementId::Name(format!("appearance-font-{key}").into()))
                .test_support()
                .role(Role::RadioButton)
                .aria_label(name)
                .aria_selected(selected)
                .reachable(theme)
                .min_w_0()
                .flex()
                .flex_col()
                .gap_0p5()
                .py_2()
                .px_2p5()
                .font_family(family)
                .rounded(theme.radius_large)
                .border_2()
                .border_color(if selected { theme.accent } else { theme.border })
                .bg(theme.main)
                .cursor_pointer()
                .hover(move |s| s.bg(hover))
                .child(
                    div()
                        .text_size(px(26.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child("Aa"),
                )
                .child(div().font_weight(FontWeight::SEMIBOLD).child(name))
                .child(div().text_xs().text_color(theme.muted).child(description))
                .on_click(move |_, _, cx| {
                    appearance::edit(cx, |a| a.font = Some(font));
                })
                .into_any_element(),
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
        .child(card_grid(
            "appearance-fonts",
            t!("appearance-font"),
            cards,
            window,
            cx,
        ))
}

/// The interface scale as a slider in steps of 5 %, with its value beside it.
fn scale(value: i64, theme: &Theme, window: &mut Window, cx: &mut App) -> impl IntoElement + use<> {
    let range = (*look::SCALE.start(), *look::SCALE.end(), 5);
    let (track, shown) = messages::number_slider(
        "appearance-scale",
        t!("appearance-scale"),
        value,
        range,
        |a, v| a.interface_scale = Some(v),
        window,
        cx,
    );
    div().flex().items_center().gap_3().child(track).child(
        div()
            .id("appearance-scale-value")
            .test_support()
            .role(Role::Status)
            .aria_label(SharedString::from(t!(
                "appearance-scale-value",
                percent = shown
            )))
            .w(px(56.))
            .text_center()
            .text_color(theme.text)
            .child(t!("appearance-scale-value", percent = shown)),
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
    let compiled = appearance::compiled(cx);
    let e = editor.read(cx);
    let notice = e.notice.clone();
    let busy = e.busy;
    let import_input = e.import.clone();
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
    if let Some((text, problem)) = notice.filter(|_| !e.studio_open) {
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
    let server = appearance::server_allows_css(cx);
    body = body.child(
        div()
            .flex()
            .items_center()
            .justify_between()
            .gap_3()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme.text)
                            .child(t!("appearance-themes-css-switch")),
                    )
                    .child(div().text_sm().text_color(theme.muted).child(if server {
                        t!("appearance-themes-css-switch-help")
                    } else {
                        t!("appearance-themes-css-server-off")
                    })),
            )
            .child(
                Switch::new("appearance-themes-css-switch")
                    .checked(library.custom_css)
                    .disabled(!server)
                    .accessibility_label(t!("appearance-themes-css-switch"))
                    .on_click(|on: &bool, _, cx| appearance::set_custom_css(cx, *on)),
            ),
    );
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
        if let Some(css) = compiled
            .css
            .as_ref()
            .filter(|_| compiled.theme == user_theme.id)
        {
            let name = user_theme.name.clone().unwrap_or_default();
            body = body.child(css_report("appearance-themes-css", &name, css, theme));
        }
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
    let base = library
        .themes
        .iter()
        .find(|theme| theme.id == current.theme)
        .and_then(|theme| theme.base.clone());
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
            .when(!full && !busy, |d| {
                d.child(
                    button("appearance-themes-new", t!("appearance-themes-new"), theme)
                        .test_support()
                        .reachable(theme)
                        .on_click(move |_, window, cx| {
                            let mut made = look::new_theme(
                                &current,
                                t!("appearance-themes-new-name"),
                                made_with(),
                                &now(),
                            );
                            if let Some(base) = &base {
                                made.base = Some(base.clone());
                            }
                            open_studio(&new_editor, &made, mode, true, window, cx);
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
    let use_editor = editor.clone();
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
                        if appearance::trial::choose(&use_id, cx) {
                            let session = use_editor.read(cx).session.clone();
                            crate::settings::close(&session, cx);
                        }
                    }),
            )
        })
        .when(in_use, |d| {
            d.child(
                button(key("stop"), t!("appearance-themes-stop"), theme)
                    .test_support()
                    .reachable(theme)
                    .on_click(|_, _, cx| {
                        appearance::trial::use_built_in(cx);
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
                    .on_click(move |_, window, cx| {
                        open_studio(&edit_editor, &edit_theme, mode, false, window, cx)
                    }),
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

fn open_studio(
    editor: &Entity<Editor>,
    user_theme: &UserTheme,
    mode: Mode,
    new_theme: bool,
    window: &mut Window,
    cx: &mut App,
) {
    if editor.read(cx).busy {
        return;
    }
    let state = editor.read(cx);
    let resume = !new_theme
        && state
            .editing
            .as_ref()
            .is_some_and(|(draft, _)| draft.id == user_theme.id);
    let session = state.session.clone();
    let focus = state.focus.clone();
    if !resume {
        let css = state.css.clone();
        let name = state.name.clone();
        fill_seeds(editor, user_theme, mode, window, cx);
        css.update(cx, |s, cx| {
            s.set_value(user_theme.css.clone().unwrap_or_default(), window, cx)
        });
        name.update(cx, |s, cx| {
            s.set_value(user_theme.name.clone().unwrap_or_default(), window, cx)
        });
    }
    editor.update(cx, |e, _| {
        if !resume {
            e.editing = Some((user_theme.clone(), mode));
            e.tab = ThemeTab::Colors;
            e.new_theme = new_theme;
            e.edit_generation = e.edit_generation.wrapping_add(1);
            e.notice = None;
        }
        e.studio_open = true;
        e.collapsed = false;
        e.compare = false;
    });
    crate::settings::close(&session, cx);
    refresh_studio(editor, cx);
    window.on_next_frame(move |window, cx| focus.focus(window, cx));
}

fn fill_seeds(
    editor: &Entity<Editor>,
    theme: &UserTheme,
    mode: Mode,
    window: &mut Window,
    cx: &mut App,
) {
    let seeds = look::drawn_seeds(theme, mode);
    for field in editor.read(cx).seeds.clone() {
        field.fill(&seeds, window, cx);
    }
}

fn draft_fields(
    editor: &Editor,
    base: &UserTheme,
    mode: Mode,
    cx: &App,
) -> Result<UserTheme, &'static str> {
    let mut draft = base.clone();
    let shown = look::drawn_seeds(base, mode);
    if look::theme_seeds(base, mode).is_some() {
        let mut colors = shown;
        for field in &editor.seeds {
            let color = field
                .value(cx)
                .ok_or_else(|| t!("appearance-accent-custom-invalid"))?;
            field.seed.set(&mut colors, color);
        }
        if colors != shown {
            look::set_theme_seeds(&mut draft, mode, Some(colors));
        }
    }
    let css = editor.css.read(cx).value();
    if css.len() > look::MAX_THEME_CSS {
        return Err(t!("appearance-themes-import-css-too-long"));
    }
    if &*css != base.css.as_deref().unwrap_or_default() {
        draft.css = Some(css.to_string());
    }
    let name = editor.name.read(cx).value();
    let name = name.trim();
    if name.is_empty() || name.chars().count() > look::MAX_THEME_NAME {
        return Err(t!("appearance-studio-name-error"));
    }
    draft.name = Some(name.to_owned());
    Ok(draft)
}

fn refresh_studio(editor: &Entity<Editor>, cx: &mut App) {
    let state = editor.read(cx);
    if state.studio_open
        && let Some((base, mode)) = &state.editing
    {
        let theme = draft_fields(state, base, *mode, cx).unwrap_or_else(|_| base.clone());
        appearance::set_studio(
            Some(appearance::StudioPreview {
                theme,
                mode: *mode,
                compare: state.compare,
            }),
            cx,
        );
    }
}

fn hide_studio(editor: &Entity<Editor>, discard: bool, cx: &mut App) {
    editor.update(cx, |editor, cx| {
        editor.studio_open = false;
        editor
            .background_preview
            .update(cx, |preview, cx| preview.clear(None, cx));
        if discard {
            editor.editing = None;
            editor.notice = None;
            editor.edit_generation = editor.edit_generation.wrapping_add(1);
        }
    });
    appearance::set_studio(None, cx);
}

fn switch_preview_mode(editor: &Entity<Editor>, mode: Mode, window: &mut Window, cx: &mut App) {
    let state = editor.read(cx);
    let Some((base, current)) = &state.editing else {
        return;
    };
    if *current == mode {
        return;
    }
    match draft_fields(state, base, *current, cx) {
        Ok(draft) => {
            fill_seeds(editor, &draft, mode, window, cx);
            editor.update(cx, |editor, _| {
                editor.editing = Some((draft, mode));
                editor.notice = None;
            });
            refresh_studio(editor, cx);
        }
        Err(message) => say(editor, message, true, cx),
    }
}

pub(crate) const STUDIO_WIDTH: f32 = 360.;

pub(crate) fn studio_view(
    session: &Entity<Session>,
    window: &mut Window,
    cx: &mut App,
) -> Option<(AnyElement, bool)> {
    let editor = cx.try_global::<Editors>()?.0.clone();
    let state = editor.read(cx);
    if state.session != *session || !state.studio_open {
        return None;
    }
    let (draft, mode) = state.editing.clone()?;
    let docked = window.viewport_size().width > px(700.);
    let collapsed = !docked && state.collapsed;
    let focus = state.focus.clone();
    let busy = state.busy;
    let safe = Theme::from_compiled(&look::compile(&appearance::look(cx), mode, &[], false));
    let collapse = editor.clone();
    let dismiss = editor.clone();
    let panel = div()
        .id("theme-studio")
        .test_support()
        .role(Role::Group)
        .aria_label(t!("appearance-studio-title"))
        .track_focus(&focus)
        .flex()
        .flex_col()
        .min_h_0()
        .occlude()
        .bg(safe.panel)
        .text_color(safe.text)
        .when(docked, |panel| {
            panel
                .w(px(STUDIO_WIDTH))
                .h_full()
                .flex_none()
                .border_l_1()
                .border_color(safe.border)
        })
        .when(!docked, |panel| {
            panel
                .absolute()
                .left_0()
                .right_0()
                .bottom_0()
                .rounded_t(safe.radius_large)
                .overflow_hidden()
                .border_t_1()
                .border_color(safe.border)
        })
        .when(!docked && !collapsed, |panel| panel.h(relative(0.58)))
        .on_action(move |_: &crate::features::shortcuts::MarkRead, _, cx| {
            cx.stop_propagation();
            if !dismiss.read(cx).busy {
                hide_studio(&dismiss, true, cx);
            }
        })
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap_2()
                .px_3()
                .py_2()
                .border_b_1()
                .border_color(safe.border)
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(t!("appearance-studio-title")),
                )
                .when(!docked, |header| {
                    header.child(
                        Button::new("theme-studio-collapse")
                            .ghost()
                            .disabled(busy)
                            .label(if collapsed {
                                t!("appearance-studio-expand")
                            } else {
                                t!("appearance-studio-collapse")
                            })
                            .on_click(move |_, _, cx| {
                                collapse.update(cx, |editor, _| editor.collapsed = !collapsed);
                                cx.refresh_windows();
                            }),
                    )
                }),
        )
        .when(!collapsed, |panel| {
            panel.child(theme_editor(&draft, mode, &editor, &safe, window, cx))
        });
    Some((panel.into_any_element(), docked))
}

fn theme_editor(
    user_theme: &UserTheme,
    mode: Mode,
    editor: &Entity<Editor>,
    theme: &Theme,
    window: &mut Window,
    cx: &mut App,
) -> impl IntoElement + use<> {
    let state = editor.read(cx);
    let tab = state.tab;
    let busy = state.busy;
    let compare = state.compare;
    let name = state.name.clone();
    let mut form = div()
        .id("theme-studio-content")
        .test_support()
        .role(Role::Form)
        .aria_label(t!("appearance-studio-title"))
        .flex_1()
        .min_h_0()
        .overflow_y_scroll()
        .flex()
        .flex_col()
        .gap_2()
        .p_3()
        .child(div().text_sm().child(t!("appearance-themes-name")))
        .child(
            Input::new(&name)
                .id("appearance-themes-name")
                .aria_label(t!("appearance-themes-name")),
        );
    let tabs = div()
        .id("appearance-theme-tabs")
        .test_support()
        .role(Role::RadioGroup)
        .aria_label(t!("appearance-themes-edit"))
        .flex()
        .flex_wrap()
        .gap_2()
        .children(
            [
                (ThemeTab::Colors, "colors", t!("appearance-themes-colors")),
                (
                    ThemeTab::Background,
                    "background",
                    t!("appearance-themes-background"),
                ),
                (ThemeTab::Css, "css", t!("appearance-themes-css-tab")),
            ]
            .map(|(choice, key, label)| {
                let editor = editor.clone();
                option(
                    format!("appearance-theme-tab-{key}"),
                    label,
                    choice == tab,
                    theme,
                )
                .test_support()
                .reachable(theme)
                .on_click(move |_, _, cx| {
                    editor.update(cx, |e, _| e.tab = choice);
                    cx.refresh_windows();
                })
            }),
        );
    form = form.child(tabs);
    if tab == ThemeTab::Background {
        form = form.child(background::render(
            editor, user_theme, mode, theme, window, cx,
        ));
    } else {
        let preview = editor.read(cx).background_preview.clone();
        preview.update(cx, |view, cx| view.clear(Some(window), cx));
    }
    if tab == ThemeTab::Css {
        form = form.child(css_editor(editor, user_theme, mode, theme, cx));
    } else if tab == ThemeTab::Colors {
        form = form.child(colors::render(editor, user_theme, mode, theme, cx));
    }
    if let Some((text, problem)) = editor.read(cx).notice.clone() {
        form = form.child(
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
    let save_editor = editor.clone();
    let cancel_editor = editor.clone();
    let compare_editor = editor.clone();
    let edited = user_theme.clone();
    div()
        .id("appearance-theme-editor")
        .flex()
        .flex_col()
        .flex_1()
        .min_h_0()
        .child(form)
        .child(
            div()
                .flex()
                .flex_wrap()
                .justify_between()
                .items_center()
                .gap_2()
                .p_3()
                .border_t_1()
                .border_color(theme.border)
                .child(
                    option(
                        "theme-studio-compare",
                        t!("appearance-studio-compare"),
                        compare,
                        theme,
                    )
                    .role(Role::Button)
                    .test_support()
                    .reachable(theme)
                    .on_click(move |_, _, cx| {
                        compare_editor.update(cx, |editor, _| editor.compare = !compare);
                        refresh_studio(&compare_editor, cx);
                    }),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            Button::new("appearance-theme-save")
                                .label(t!("appearance-themes-save"))
                                .primary()
                                .disabled(busy)
                                .on_click(move |_, _, cx| {
                                    match draft_fields(save_editor.read(cx), &edited, mode, cx) {
                                        Ok(mut draft) => {
                                            draft.made_with = Some(made_with().into());
                                            save_edit(&save_editor, draft, cx);
                                        }
                                        Err(message) => say(&save_editor, message, true, cx),
                                    }
                                }),
                        )
                        .child(
                            Button::new("appearance-theme-cancel")
                                .label(t!("appearance-themes-cancel"))
                                .ghost()
                                .disabled(busy)
                                .on_click(move |_, _, cx| {
                                    hide_studio(&cancel_editor, true, cx);
                                }),
                        ),
                ),
        )
}

fn css_editor(
    editor: &Entity<Editor>,
    draft: &UserTheme,
    mode: Mode,
    theme: &Theme,
    cx: &App,
) -> Div {
    let input = &editor.read(cx).css;
    let text = input.read(cx).value();
    let size = text.len();
    let mut form = div()
        .flex()
        .flex_col()
        .gap_2()
        .child(
            div()
                .id("appearance-theme-css-editor")
                .test_support()
                .child(
                    Textarea::new(input)
                        .aria_label(t!("appearance-themes-css-editor"))
                        .font_family(theme.mono_font.clone()),
                ),
        )
        .child(
            div()
                .text_sm()
                .text_color(if size > look::MAX_THEME_CSS {
                    theme.danger
                } else {
                    theme.muted
                })
                .child(t!(
                    "appearance-themes-css-size",
                    used = size,
                    limit = look::MAX_THEME_CSS
                )),
        );
    if !appearance::server_allows_css(cx) {
        form = form.child(
            div()
                .text_sm()
                .text_color(theme.warning)
                .child(t!("appearance-themes-css-server-off")),
        );
    } else if !appearance::css_allowed(cx) {
        form = form.child(
            div()
                .text_sm()
                .text_color(theme.muted)
                .child(t!("appearance-themes-css-account-off")),
        );
    }
    if size <= look::MAX_THEME_CSS && !text.trim().is_empty() {
        let mut base = appearance::look(cx);
        base.theme.clone_from(&draft.id);
        let mut preview = draft.clone();
        preview.css = Some(text.to_string());
        let compiled = look::compile_with_css(&base, mode, &[preview], false, true);
        if let Some(css) = &compiled.css {
            form = form.child(css_report(
                "appearance-draft-css",
                draft.name.as_deref().unwrap_or_default(),
                css,
                theme,
            ));
        }
    }
    form.child(
        div()
            .text_sm()
            .text_color(theme.muted)
            .child(t!("appearance-themes-css-switch-help")),
    )
}

fn save_edit(editor: &Entity<Editor>, draft: UserTheme, cx: &mut App) {
    if editor.read(cx).busy {
        return;
    }
    if editor.read(cx).new_theme {
        save(editor, draft, None, true, None, cx);
        return;
    }
    let expected = draft.updated_at.clone();
    let preview = draft.clone();
    let keep_editor = editor.clone();
    if appearance::trial::preview_edit(
        &draft,
        move |cx| {
            let expected = preview.updated_at.clone();
            save(&keep_editor, preview, expected, false, None, cx);
        },
        cx,
    ) {
        hide_studio(editor, false, cx);
    } else {
        save(editor, draft, expected, false, None, cx);
    }
}

fn made_with() -> &'static str {
    concat!("IRCord GPUI ", env!("CARGO_PKG_VERSION"))
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
    if editor.read(cx).busy {
        return;
    }
    editor.update(cx, |e, _| e.busy = true);
    let id = user_theme.id.clone();
    let saves_draft = editor
        .read(cx)
        .editing
        .as_ref()
        .is_some_and(|(draft, _)| draft.id == id);
    let task = client.request(move |api| async move {
        api.save_theme(&user_theme, expected.as_deref(), false)
            .await
    });
    let editor = editor.clone();
    cx.spawn(async move |cx| {
        let result = task.await;
        cx.update(|cx| {
            if !editor
                .read(cx)
                .session
                .read(cx)
                .client()
                .is_some_and(|now| Rc::ptr_eq(now, &client))
            {
                return;
            }
            editor.update(cx, |e, _| {
                e.busy = false;
                if saves_draft && !matches!(&result, Ok(Ok(_))) {
                    e.studio_open = true;
                }
            });
            match result {
                Ok(Ok(themes)) => {
                    appearance::set_library(themes, cx);
                    if saves_draft {
                        hide_studio(&editor, true, cx);
                    }
                    editor.update(cx, |e, _| {
                        e.notice = done.map(|text| (text.into(), false));
                    });
                    if then_use && let Some(id) = id {
                        appearance::trial::choose(&id, cx);
                    }
                    appearance::apply(cx);
                }
                Ok(Err(ircord_client::api::ApiError::Http { status: 412, .. })) => {
                    say(&editor, t!("appearance-themes-conflict"), true, cx);
                    client.watch_document(&ircord_client::documents::THEMES);
                }
                Ok(Err(e)) => say(&editor, t!("appearance-themes-failed", error = e), true, cx),
                Err(e) => say(&editor, t!("appearance-themes-failed", error = e), true, cx),
            }
            refresh_studio(&editor, cx);
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
            let note = t!("appearance-themes-imported", name = name);
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

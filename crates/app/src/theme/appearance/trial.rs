//! A device-local preview for personal themes that supply custom CSS.

use std::time::{Duration, Instant};

use gpui_kit::prelude::*;
use gpui_kit::*;
use ircord_client::contract::Theme as UserTheme;
use ircord_client::store::appearance::{self as theme_data, Look};

use super::super::Theme;
use super::{apply, css_allowed, edit, look, system_mode, themes};
use crate::ui::focus::Reachable;

const TRIAL_TIME: Duration = Duration::from_secs(20);
const REFRESH_TIME: Duration = Duration::from_secs(1);

#[derive(Default)]
pub(super) struct State {
    paused: bool,
    running: Option<Running>,
    generation: u64,
}

struct Running {
    theme: UserTheme,
    kept_id: Option<String>,
    deadline: Instant,
    _ticker: Task<()>,
}

impl State {
    pub(super) fn cancel(&mut self) {
        self.running = None;
    }

    /// Discard a trial when the account choice, library entry or CSS policy changes.
    pub(super) fn validate(&mut self, look: &Look, themes: &[UserTheme], css: bool) -> bool {
        let invalid = self.running.as_ref().is_some_and(|trial| {
            !css || trial.kept_id != look.theme || !themes.contains(&trial.theme)
        });
        if invalid {
            self.cancel();
        }
        invalid
    }

    pub(super) fn select(&self, look: &mut Look) {
        if self.paused {
            look.theme = None;
        } else if let Some(trial) = &self.running {
            look.theme.clone_from(&trial.theme.id);
        }
    }
}

/// Select a saved personal theme, returning true when the workspace should
/// make room for its keep-or-revert prompt.
pub(crate) fn choose(id: &str, cx: &mut App) -> bool {
    let Some(theme) = themes(cx)
        .into_iter()
        .find(|theme| theme.id.as_deref() == Some(id) && theme_data::theme_supported(theme))
    else {
        return false;
    };
    let kept_id = look(cx).theme;
    let use_trial = css_allowed(cx)
        && theme
            .css
            .as_deref()
            .is_some_and(|css| !css.trim().is_empty())
        && kept_id != theme.id;
    {
        let state = &mut cx.global_mut::<super::State>().preview;
        state.cancel();
        state.paused = false;
    }
    if use_trial {
        start(theme, kept_id, cx);
    } else {
        edit(cx, |appearance| appearance.theme = theme.id);
        repaint(cx);
    }
    use_trial
}

fn start(theme: UserTheme, kept_id: Option<String>, cx: &mut App) {
    let executor = cx.background_executor().clone();
    let deadline = executor.now() + TRIAL_TIME;
    let state = &mut cx.global_mut::<super::State>().preview;
    state.generation = state.generation.wrapping_add(1);
    let generation = state.generation;
    let ticker = cx.spawn(async move |cx| {
        loop {
            let remaining = deadline.saturating_duration_since(executor.now());
            if remaining.is_zero() {
                cx.update(|cx| {
                    let state = &cx.global::<super::State>().preview;
                    if state.generation == generation && state.running.is_some() {
                        revert(cx);
                    }
                });
                break;
            }
            executor.timer(remaining.min(REFRESH_TIME)).await;
            cx.update(|cx| cx.refresh_windows());
        }
    });
    cx.global_mut::<super::State>().preview.running = Some(Running {
        theme,
        kept_id,
        deadline,
        _ticker: ticker,
    });
    repaint(cx);
}

fn repaint(cx: &mut App) {
    apply(cx);
    cx.refresh_windows();
}

fn keep(cx: &mut App) {
    let now = cx.background_executor().now();
    let kept = look(cx).theme;
    let library = themes(cx);
    let css = css_allowed(cx);
    let trial = cx.global_mut::<super::State>().preview.running.take();
    if let Some(trial) = trial
        && now < trial.deadline
        && css
        && kept == trial.kept_id
        && library.contains(&trial.theme)
    {
        edit(cx, |appearance| appearance.theme = trial.theme.id);
    }
    repaint(cx);
}

fn revert(cx: &mut App) {
    cx.global_mut::<super::State>().preview.cancel();
    repaint(cx);
}

/// Return to the built-in scheme and forget the account's current selection.
pub(crate) fn use_built_in(cx: &mut App) {
    let state = &mut cx.global_mut::<super::State>().preview;
    state.cancel();
    state.paused = false;
    edit(cx, |a| a.theme = None);
    repaint(cx);
}

/// Run with the personal theme suspended until the user resumes it.
pub(crate) fn safe_start(cx: &mut App) {
    let state = &mut cx.global_mut::<super::State>().preview;
    state.cancel();
    state.paused = true;
    repaint(cx);
}

fn resume(cx: &mut App) {
    cx.global_mut::<super::State>().preview.paused = false;
    repaint(cx);
}

pub(super) fn toggle_pause(cx: &mut App) {
    let state = &cx.global::<super::State>().preview;
    if state.paused {
        resume(cx);
    } else if state.running.is_some() || super::compiled(cx).theme.is_some() {
        safe_start(cx);
    }
}

pub(crate) fn banner(window: &Window, cx: &App) -> Option<AnyElement> {
    let state = &cx.global::<super::State>().preview;
    if state.running.is_none() && !state.paused {
        return None;
    }
    let mut base = look(cx);
    let mode = base.resolve_mode(system_mode(cx));
    let keep_id = base.theme.take();
    let safe = Theme::from_compiled(&theme_data::compile(&base, mode, &[], false));
    let (message, first, second): (_, _, Option<_>) = if let Some(trial) = &state.running {
        let remaining = trial
            .deadline
            .saturating_duration_since(cx.background_executor().now());
        let seconds = remaining.as_secs() + u64::from(remaining.subsec_nanos() > 0);
        (
            t!("appearance-trial-countdown", seconds = seconds),
            control(
                "theme-trial-keep",
                t!("appearance-trial-keep"),
                true,
                keep,
                &safe,
            ),
            Some(control(
                "theme-trial-revert",
                t!("appearance-trial-revert"),
                false,
                revert,
                &safe,
            )),
        )
    } else {
        let id = keep_id?;
        if !themes(cx).iter().any(|theme| {
            theme.id.as_deref() == Some(id.as_str()) && theme_data::theme_supported(theme)
        }) {
            return None;
        }
        (
            t!("appearance-trial-paused").into(),
            control(
                "theme-trial-resume",
                t!("appearance-trial-resume"),
                true,
                resume,
                &safe,
            ),
            None,
        )
    };
    let top = if cfg!(target_os = "macos") && !window.is_fullscreen() {
        crate::MAC_TITLEBAR_HEIGHT + 12.
    } else {
        12.
    };
    Some(
        div()
            .absolute()
            .top(px(top))
            .left_0()
            .right_0()
            .px(px(12.))
            .flex()
            .justify_center()
            .child(
                div()
                    .id("theme-trial-banner")
                    .test_support()
                    .role(Role::Group)
                    .aria_label(t!("appearance-trial-title"))
                    .occlude()
                    .max_w_full()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(8.))
                    .p(px(8.))
                    .pl(px(12.))
                    .rounded(px(14.))
                    .bg(safe.panel)
                    .shadow(vec![BoxShadow {
                        color: black().opacity(0.35),
                        offset: point(px(0.), px(12.)),
                        blur_radius: px(40.),
                        spread_radius: px(0.),
                        inset: false,
                    }])
                    .font_family(".SystemUIFont")
                    .text_size(px(13.))
                    .text_color(safe.text)
                    .child(div().font_weight(FontWeight::SEMIBOLD).child(message))
                    .child(div().flex().gap(px(6.)).child(first).children(second)),
            )
            .into_any_element(),
    )
}

fn control(
    id: &'static str,
    label: &'static str,
    primary: bool,
    callback: fn(&mut App),
    theme: &Theme,
) -> AnyElement {
    div()
        .id(id)
        .test_support()
        .role(Role::Button)
        .aria_label(label)
        .reachable(theme)
        .h(px(30.))
        .px(px(12.))
        .flex()
        .items_center()
        .rounded(px(8.))
        .cursor_pointer()
        .bg(if primary { theme.accent } else { theme.raised })
        .text_color(if primary { theme.on_accent } else { theme.text })
        .hover(|d| {
            d.bg(if primary {
                theme.accent.opacity(0.9)
            } else {
                theme.hover
            })
        })
        .on_click(move |_, _, cx| callback(cx))
        .child(label)
        .into_any_element()
}

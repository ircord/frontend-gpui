//! A theme change fades: over [`DURATION`] every colour token moves from the
//! old value to the new one. Fonts, corners, scale and the other non-colour
//! tokens change at the start. The motion is a pure function of time
//! ([`progress`], [`mix`]) so it is tested without a window; [`start`] drives
//! the frames from the session's runtime (GPUI timers do not run in the
//! headless tests) and only while a fade is running.
//!
//! With reduced motion, or with no session runtime to pace the frames, the
//! new theme is applied at once, as it was before the fade existed.

use std::time::{Duration, Instant};

use gpui_kit::{App, Global, Hsla, Rgba};

use super::Theme;

/// How long the colours take to arrive.
pub const DURATION: Duration = Duration::from_millis(200);
/// The pause between frames.
const FRAME: Duration = Duration::from_millis(16);

/// How far the fade has come, 0 to 1, eased out: quick at first, then settling.
pub fn progress(elapsed: Duration) -> f32 {
    let t = (elapsed.as_secs_f32() / DURATION.as_secs_f32()).clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

/// Whether the fade is over.
pub fn finished(elapsed: Duration) -> bool {
    elapsed >= DURATION
}

/// A colour `t` of the way from `from` to `to`, mixed in sRGB with its alpha.
pub fn colour(from: Hsla, to: Hsla, t: f32) -> Hsla {
    // The ends are the colours themselves, not a round trip through sRGB.
    if t <= 0.0 {
        return from;
    }
    if t >= 1.0 {
        return to;
    }
    let (a, b): (Rgba, Rgba) = (from.into(), to.into());
    let mix = |x: f32, y: f32| x + (y - x) * t;
    Hsla::from(Rgba {
        r: mix(a.r, b.r),
        g: mix(a.g, b.g),
        b: mix(a.b, b.b),
        a: mix(a.a, b.a),
    })
}

/// The theme `t` of the way from `from` to `to`: colours are mixed, every
/// other token is `to`'s.
pub fn mix(from: &Theme, to: &Theme, t: f32) -> Theme {
    let c = |f: Hsla, o: Hsla| colour(f, o, t);
    let mut theme = to.clone();
    theme.rail = c(from.rail, to.rail);
    theme.sidebar = c(from.sidebar, to.sidebar);
    theme.footer = c(from.footer, to.footer);
    theme.main = c(from.main, to.main);
    theme.input = c(from.input, to.input);
    theme.hover = c(from.hover, to.hover);
    theme.row_hover = c(from.row_hover, to.row_hover);
    theme.selected = c(from.selected, to.selected);
    theme.text = c(from.text, to.text);
    theme.muted = c(from.muted, to.muted);
    theme.faint = c(from.faint, to.faint);
    theme.accent = c(from.accent, to.accent);
    theme.on_accent = c(from.on_accent, to.on_accent);
    theme.link = c(from.link, to.link);
    theme.danger = c(from.danger, to.danger);
    theme.warning = c(from.warning, to.warning);
    theme.online = c(from.online, to.online);
    theme.away = c(from.away, to.away);
    theme.offline = c(from.offline, to.offline);
    theme.mention = c(from.mention, to.mention);
    theme.code = c(from.code, to.code);
    theme.panel = c(from.panel, to.panel);
    theme.raised = c(from.raised, to.raised);
    theme.border = c(from.border, to.border);
    theme.control_border = c(from.control_border, to.control_border);
    theme.body = c(from.body, to.body);
    theme
}

/// The fade in progress: the theme it ends on, which what is expensive to
/// redraw (diagrams, the ambient light) reads at once.
pub(super) struct Running {
    pub to: Theme,
    id: u64,
}

#[derive(Default)]
struct Fades {
    running: Option<Running>,
    count: u64,
}

impl Global for Fades {}

/// The theme the window settles on: the fade's end while one runs, the active
/// tokens otherwise.
pub fn settled(cx: &App) -> &Theme {
    cx.try_global::<Fades>()
        .and_then(|f| f.running.as_ref())
        .map_or_else(|| super::get(cx), |r| &r.to)
}

/// Whether a fade is running.
pub fn active(cx: &App) -> bool {
    cx.try_global::<Fades>()
        .is_some_and(|f| f.running.is_some())
}

/// Stop any fade; the active tokens stay as they are.
pub(super) fn cancel(cx: &mut App) {
    if cx.has_global::<Fades>() {
        cx.global_mut::<Fades>().running = None;
    }
}

/// Fade from the active tokens to `to`, drawing each frame through `show`.
/// Returns false, leaving everything to the caller, when the change should
/// snap: reduced motion, nothing open to show it, or no runtime to pace it.
pub(super) fn start(
    to: Theme,
    runtime: Option<std::sync::Arc<tokio::runtime::Runtime>>,
    show: fn(Theme, &mut App),
    cx: &mut App,
) -> bool {
    // A change that arrives mid-fade takes over: the old fade stops, and the
    // new one starts from the colours shown now.
    cancel(cx);
    let from = super::get(cx).clone();
    let Some(runtime) = runtime else {
        return false;
    };
    if to.reduced_motion || from.reduced_motion || cx.windows().is_empty() || from == to {
        return false;
    }
    if !cx.has_global::<Fades>() {
        cx.set_global(Fades::default());
    }
    let fades = cx.global_mut::<Fades>();
    fades.count += 1;
    let id = fades.count;
    fades.running = Some(Running { to: to.clone(), id });
    let started = Instant::now();
    frame(from, to, started, id, runtime, show, cx);
    true
}

fn frame(
    from: Theme,
    to: Theme,
    started: Instant,
    id: u64,
    runtime: std::sync::Arc<tokio::runtime::Runtime>,
    show: fn(Theme, &mut App),
    cx: &mut App,
) {
    if cx
        .try_global::<Fades>()
        .and_then(|f| f.running.as_ref())
        .is_none_or(|r| r.id != id)
    {
        return;
    }
    let elapsed = started.elapsed();
    if finished(elapsed) {
        cancel(cx);
        show(to, cx);
        return;
    }
    show(mix(&from, &to, progress(elapsed)), cx);
    let paced = runtime.spawn(async { tokio::time::sleep(FRAME).await });
    cx.spawn(async move |cx| {
        let _ = paced.await;
        cx.update(|cx| frame(from, to, started, id, runtime, show, cx))
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::hex;

    #[test]
    fn progress_runs_from_zero_to_one_and_only_forwards() {
        assert_eq!(progress(Duration::ZERO), 0.0);
        assert_eq!(progress(DURATION), 1.0);
        assert_eq!(progress(DURATION * 3), 1.0);
        let mut last = 0.0;
        for ms in (0..=200).step_by(8) {
            let p = progress(Duration::from_millis(ms));
            assert!(p >= last, "went back at {ms} ms");
            last = p;
        }
    }

    #[test]
    fn progress_eases_out() {
        assert!(progress(Duration::from_millis(100)) > 0.5);
    }

    #[test]
    fn a_fade_is_over_after_its_duration() {
        assert!(!finished(Duration::from_millis(199)));
        assert!(finished(DURATION));
    }

    #[test]
    fn colours_meet_at_both_ends_and_the_middle() {
        let (black, white) = (hex(0x000000), hex(0xffffff));
        assert_eq!(colour(black, white, 0.0), black);
        assert_eq!(colour(black, white, 1.0), white);
        let mid: Rgba = colour(black, white, 0.5).into();
        assert!((mid.r - 0.5).abs() < 0.01 && (mid.b - 0.5).abs() < 0.01);
    }

    #[test]
    fn alpha_fades_too() {
        let a = colour(hex(0xff0000).opacity(0.0), hex(0xff0000).opacity(1.0), 0.5);
        assert!((a.a - 0.5).abs() < 0.01);
    }

    #[test]
    fn mixing_moves_colours_and_takes_the_rest_from_the_end() {
        let (dark, light) = (Theme::dark(), Theme::light());
        assert_eq!(mix(&dark, &light, 0.0).main, dark.main);
        assert_eq!(mix(&dark, &light, 1.0), light);
        let half = mix(&dark, &light, 0.5);
        assert_ne!(half.main, dark.main);
        assert_ne!(half.main, light.main);
        assert_eq!(half.dark, light.dark);
        assert_eq!(half.radius, light.radius);
        assert_eq!(half.font, light.font);
    }
}

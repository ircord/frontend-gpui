//! The video player (package M5): a video attachment or a link's video
//! plays in the message, in a box whose size is fixed before anything
//! loads, with the shared transport bar under it; Enlarge opens it in a
//! large viewer. One thing plays at a time (`media::claim`).
//!
//! It is offered through `registry.playables` and answers only where
//! [`available`] says this machine plays video (on Linux: the
//! `ircord-video` helper and GStreamer). Otherwise, and for a video that
//! cannot play, the person gets the card that opens the browser.

use std::collections::HashSet;
use std::rc::Rc;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use gpui_kit::TestSupportExt;
use gpui_kit::component::WindowExt;
use gpui_kit::prelude::*;
use gpui_kit::*;
use ircord_platform::video::{self, State, Video};
use tokio::runtime::Runtime;

use crate::features::viewer::{self, Media};
use crate::media::controls::{self, Controls, Transport};
use crate::media::{self, video_view::VideoView};
use crate::registry::{self, PlayableCtx, PlayableKind, Slot};
use crate::session::{Session, SessionEvent};
use crate::theme::{self, Theme};
use crate::ui;
use crate::ui::focus::Reachable;
use crate::ui::icon::IconName;

/// The box a video is drawn in, at most this large.
const BOX_W: f32 = 400.;
const BOX_H: f32 = 300.;
/// The box's height when the video's size is not known (16:9).
const UNKNOWN_H: f32 = 225.;
/// The row under the picture: the name idle, the transport bar playing.
const BAR_H: f32 = 40.;
/// How often the bar learns where the player is.
const TICK: Duration = Duration::from_millis(250);

/// How a video is opened: `video::open` unless a test installs a fake.
pub type Open = Rc<dyn Fn(&str, u32, u32) -> Result<Box<dyn Video>, String>>;

/// Opens the backend; the default is [`ircord_platform::video::open`].
pub struct Opener(pub Open);
impl Global for Opener {}

/// Whether video plays here, set by tests; without it, what
/// [`ircord_platform::video::available`] says, asked once at start.
pub struct Available(pub bool);
impl Global for Available {}

/// Keys whose video failed to play: they show the card that opens the
/// browser until played again.
#[derive(Default)]
struct Failed(HashSet<String>);
impl Global for Failed {}

enum Place {
    Inline {
        view: Entity<VideoView>,
        controls: Entity<Controls>,
    },
    /// The large viewer's dialog holds it; it goes when the dialog closes.
    Large {
        large: WeakEntity<Large>,
        view: Entity<VideoView>,
        controls: Entity<Controls>,
    },
}

struct Current {
    key: String,
    conversation: String,
    window: AnyWindowHandle,
    place: Place,
    muted: bool,
    /// The state the box was last drawn for, so a change redraws it.
    shown: State,
    _tick: Task<()>,
}

/// The one video playing, if any.
#[derive(Default)]
struct Playing(Option<Current>);
impl Global for Playing {}

pub fn init(cx: &mut App) {
    cx.default_global::<Playing>();
    cx.default_global::<Failed>();
    // Asking runs the helper once (GStreamer can take seconds the first
    // time), so it runs on a thread of its own, outside GPUI.
    std::thread::Builder::new()
        .name("video-probe".into())
        .spawn(|| {
            let _ = PROBED.set(video::available());
        })
        .ok();
    registry::register(cx, |r| {
        r.playables.add(Slot::new("video", render));
    });
    // Another video or a sound started: this one stops.
    cx.observe_global::<media::NowPlaying>(|cx| {
        let ours = playing(cx);
        if ours.is_some() && media::holder(cx).as_deref() != ours.as_deref() {
            stop(true, cx);
        }
    })
    .detach();
    // Another conversation opened: the video in this one stops.
    cx.observe_new::<Session>(|_, _, cx| {
        cx.subscribe_self(|session: &mut Session, event: &SessionEvent, cx| {
            if let SessionEvent::Selection = event {
                let to = session.conversation.clone();
                let elsewhere = cx
                    .try_global::<Playing>()
                    .and_then(|p| p.0.as_ref())
                    .is_some_and(|c| to.as_deref() != Some(c.conversation.as_str()));
                if elsewhere {
                    stop(true, cx);
                }
            }
        })
        .detach();
    })
    .detach();
}

/// What [`ircord_platform::video::available`] said, once it has.
static PROBED: OnceLock<bool> = OnceLock::new();

/// Whether this machine plays video: [`Available`] when set, else the
/// probe started by [`init`]. Until the probe answers, videos show their
/// card; the next redraw after it does shows the player.
pub fn available(cx: &App) -> bool {
    match cx.try_global::<Available>() {
        Some(a) => a.0,
        None => PROBED.get().copied().unwrap_or(false),
    }
}

/// The key of the video playing, inline or in the large viewer.
pub fn playing(cx: &App) -> Option<String> {
    cx.try_global::<Playing>()
        .and_then(|p| p.0.as_ref())
        .map(|c| c.key.clone())
}

/// `width` by `height` fitted into `max_w` by `max_h`; 16:9 when the size
/// is not known.
pub fn fit(width: u32, height: u32, max_w: f32, max_h: f32) -> (f32, f32) {
    let (w, h) = if width == 0 || height == 0 {
        (16., 9.)
    } else {
        (width as f32, height as f32)
    };
    let scale = (max_w / w).min(max_h / h);
    ((w * scale).round(), (h * scale).round())
}

/// The box a video is drawn in inside a message.
pub fn box_size(media: &Media) -> (f32, f32) {
    if media.width == 0 || media.height == 0 {
        return (BOX_W, UNKNOWN_H);
    }
    fit(media.width, media.height, BOX_W, BOX_H)
}

fn open(url: &str, width: u32, height: u32, cx: &App) -> Result<Box<dyn Video>, String> {
    match cx.try_global::<Opener>().map(|o| o.0.clone()) {
        Some(open) => open(url, width, height),
        None => video::open(url, width, height),
    }
}

fn current<'a>(key: &str, cx: &'a App) -> Option<&'a Current> {
    cx.try_global::<Playing>()
        .and_then(|p| p.0.as_ref())
        .filter(|c| c.key == key)
}

/// The view drawing `key`'s video and the bar under it, while it plays.
fn parts(key: &str, cx: &App) -> Option<(Entity<VideoView>, Entity<Controls>)> {
    match &current(key, cx)?.place {
        Place::Inline { view, controls } | Place::Large { view, controls, .. } => {
            Some((view.clone(), controls.clone()))
        }
    }
}

/// Stop the video playing. `close` also closes the large viewer showing it.
fn stop(close: bool, cx: &mut App) {
    let Some(current) = cx.default_global::<Playing>().0.take() else {
        return;
    };
    media::release(&current.key, cx);
    if close
        && let Place::Large { large, .. } = &current.place
        && large.upgrade().is_some()
    {
        let window = current.window;
        cx.defer(move |cx| {
            window.update(cx, |_, w, cx| w.close_dialog(cx)).ok();
        });
    }
    drop(current);
    cx.refresh_windows();
}

/// `key` cannot play: drop its player and show the card instead.
fn fail(key: &str, why: &str, cx: &mut App) {
    log::warn!("video {key} cannot play: {why}");
    if playing(cx).as_deref() == Some(key) {
        stop(true, cx);
    }
    cx.default_global::<Failed>().0.insert(key.to_string());
    cx.refresh_windows();
}

fn failed(key: &str, cx: &App) -> bool {
    cx.try_global::<Failed>().is_some_and(|f| f.0.contains(key))
}

/// Play, pause, or play again from the start once ended.
fn toggle(key: &str, cx: &mut App) {
    let Some((view, _)) = parts(key, cx) else {
        return;
    };
    {
        let v = view.read(cx).video();
        match v.state() {
            State::Playing | State::Loading => v.pause(),
            State::Ended => {
                v.seek(Duration::ZERO);
                v.play();
            }
            State::Paused | State::Failed => v.play(),
        }
    }
    // A playing view asks for its next picture each frame.
    view.update(cx, |_, cx| cx.notify());
    tick(key, cx);
}

fn mute(key: &str, cx: &mut App) {
    let Some((view, _)) = parts(key, cx) else {
        return;
    };
    let Some(c) = cx.default_global::<Playing>().0.as_mut() else {
        return;
    };
    c.muted = !c.muted;
    let muted = c.muted;
    view.read(cx).video().set_muted(muted);
    tick(key, cx);
}

fn seek(key: &str, to: Duration, cx: &mut App) {
    if let Some((view, _)) = parts(key, cx) {
        view.read(cx).video().seek(to);
        tick(key, cx);
    }
}

/// Tell the bar where the player is; drop the player when it failed or its
/// large viewer closed. Returns whether `key` still plays.
fn tick(key: &str, cx: &mut App) -> bool {
    let Some(c) = current(key, cx) else {
        return false;
    };
    let (window, muted, shown) = (c.window, c.muted, c.shown);
    let large = matches!(c.place, Place::Large { .. });
    let Some((view, controls)) = parts(key, cx) else {
        return false;
    };
    let (state, position, duration) = {
        let v = view.read(cx).video();
        (v.state(), v.position(), v.duration())
    };
    if state == State::Failed {
        fail(key, "the player failed", cx);
        return false;
    }
    let transport = Transport {
        playing: state == State::Playing,
        loading: state == State::Loading,
        position,
        duration,
        muted,
    };
    // `None`: the window is busy (drawing a frame); try again next tick.
    let open = window
        .update(cx, |_, window, cx| {
            controls.update(cx, |bar, cx| bar.set(transport, window, cx));
            if state != shown {
                window.refresh();
            }
            !large || window.has_active_dialog(cx)
        })
        .ok();
    match open {
        None => return true,
        Some(false) => {
            stop(false, cx);
            return false;
        }
        Some(true) => {}
    }
    if let Some(c) = cx.default_global::<Playing>().0.as_mut() {
        c.shown = state;
    }
    if matches!(state, State::Playing | State::Loading) {
        view.update(cx, |_, cx| cx.notify());
    }
    true
}

/// Every [`TICK`] while `key` plays. The sleep runs on the session's
/// runtime: GPUI timers never fire in the headless tests.
fn ticker(key: String, runtime: Arc<Runtime>, cx: &mut App) -> Task<()> {
    cx.spawn(async move |cx| {
        loop {
            let _ = runtime
                .spawn(async { tokio::time::sleep(TICK).await })
                .await;
            if !cx.update(|cx| tick(&key, cx)) {
                break;
            }
        }
    })
}

/// The transport bar for `key`'s player, its element IDs starting `id`.
fn bar(key: &str, id: String, window: &mut Window, cx: &mut App) -> Entity<Controls> {
    let (a, b, c) = (key.to_string(), key.to_string(), key.to_string());
    cx.new(|cx| {
        Controls::new(
            id,
            Rc::new(move |_, cx| toggle(&a, cx)),
            Rc::new(move |at, _, cx| seek(&b, at, cx)),
            Rc::new(move |_, cx| mute(&c, cx)),
            window,
            cx,
        )
    })
}

/// Play `ctx`'s video in its box.
fn play(ctx: &PlayableCtx, window: &mut Window, cx: &mut App) {
    stop(true, cx);
    cx.default_global::<Failed>().0.remove(&ctx.key);
    let (w, h) = box_size(&ctx.media);
    let scale = window.scale_factor();
    let video = match open(
        &ctx.media.url,
        (w * scale).round() as u32,
        (h * scale).round() as u32,
        cx,
    ) {
        Ok(video) => video,
        Err(why) => return fail(&ctx.key, &why, cx),
    };
    video.play();
    let view = cx.new(|cx| VideoView::new(video, cx));
    let controls = bar(&ctx.key, ctx.key.clone(), window, cx);
    let runtime = ctx.session.read(cx).runtime.clone();
    let tick = ticker(ctx.key.clone(), runtime, cx);
    cx.default_global::<Playing>().0 = Some(Current {
        key: ctx.key.clone(),
        conversation: ctx.conversation.clone(),
        window: window.window_handle(),
        place: Place::Inline { view, controls },
        muted: false,
        shown: State::Loading,
        _tick: tick,
    });
    media::claim(&ctx.key, cx);
}

/// The large viewer: the video at the size of a dialog filling the window.
pub struct Large {
    key: String,
    size: (f32, f32),
    view: Entity<VideoView>,
    controls: Entity<Controls>,
    focus: FocusHandle,
}

impl Focusable for Large {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

/// Open the large viewer on `ctx`'s video, where the inline one was.
fn enlarge(ctx: &PlayableCtx, window: &mut Window, cx: &mut App) {
    let (at, muted) = match (parts(&ctx.key, cx), current(&ctx.key, cx)) {
        (Some((view, _)), Some(c)) => (view.read(cx).video().position(), c.muted),
        _ => (Duration::ZERO, false),
    };
    stop(true, cx);
    let viewport = window.viewport_size();
    let width = (viewport.width - px(64.)).max(px(320.));
    let stage = fit(
        ctx.media.width,
        ctx.media.height,
        f32::from(width) - 32.,
        (f32::from(viewport.height) - 220.).max(200.),
    );
    let scale = window.scale_factor();
    let video = match open(
        &ctx.media.url,
        (stage.0 * scale).round() as u32,
        (stage.1 * scale).round() as u32,
        cx,
    ) {
        Ok(video) => video,
        Err(why) => return fail(&ctx.key, &why, cx),
    };
    video.set_muted(muted);
    video.seek(at);
    video.play();
    let view = cx.new(|cx| VideoView::new(video, cx));
    let controls = bar(&ctx.key, format!("{}-large", ctx.key), window, cx);
    let key = ctx.key.clone();
    let large = cx.new(|cx| {
        cx.on_release(|this: &mut Large, cx| closed(&this.key, cx))
            .detach();
        Large {
            key: key.clone(),
            size: stage,
            view: view.clone(),
            controls: controls.clone(),
            focus: cx.focus_handle(),
        }
    });
    let runtime = ctx.session.read(cx).runtime.clone();
    let tick = ticker(key.clone(), runtime, cx);
    cx.default_global::<Playing>().0 = Some(Current {
        key: key.clone(),
        conversation: ctx.conversation.clone(),
        window: window.window_handle(),
        place: Place::Large {
            large: large.downgrade(),
            view,
            controls,
        },
        muted,
        shown: State::Loading,
        _tick: tick,
    });
    media::claim(&key, cx);
    let focus = large.read(cx).focus.clone();
    window.open_dialog(cx, move |dialog, _, _| {
        let key = key.clone();
        dialog
            .width(width)
            .title(t!("video-viewer-title"))
            .on_close(move |_, _, cx| closed(&key, cx))
            .child(large.clone())
    });
    window.focus(&focus, cx);
}

/// The large viewer closed: its video stops.
fn closed(key: &str, cx: &mut App) {
    if current(key, cx).is_some_and(|c| matches!(c.place, Place::Large { .. })) {
        stop(false, cx);
    }
}

impl Render for Large {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme::get(cx).clone();
        let key = self.key.clone();
        let (w, h) = self.size;
        let ended = self.view.read(cx).video().state() == State::Ended;
        div()
            .id(ElementId::Name(format!("{key}-viewer").into()))
            .test_support()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, e: &KeyDownEvent, _, cx| {
                if e.keystroke.key == "space" {
                    let key = this.key.clone();
                    cx.defer(move |cx| toggle(&key, cx));
                    cx.stop_propagation();
                }
            }))
            .flex()
            .flex_col()
            .items_center()
            .gap_2()
            .child(
                div()
                    .relative()
                    .w(px(w))
                    .h(px(h))
                    .child(self.view.clone())
                    .when(ended, |d| {
                        let k = key.clone();
                        d.child(play_button(
                            format!("{key}-large-again"),
                            t!("video-play-again").to_string(),
                            &theme,
                            move |_, _, cx| toggle(&k, cx),
                        ))
                    }),
            )
            .child(div().w(px(w)).child(self.controls.clone()))
    }
}

/// The round play button over the middle of a picture.
fn play_button(
    id: String,
    label: String,
    theme: &Theme,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .absolute()
        .inset_0()
        .flex()
        .items_center()
        .justify_center()
        .child(
            div()
                .id(ElementId::Name(id.into()))
                .test_support()
                .role(Role::Button)
                .reachable(theme)
                .aria_label(SharedString::from(label.clone()))
                .tooltip(ui::tooltip::text(label))
                .size(px(48.))
                .rounded_full()
                .bg(theme.accent)
                .cursor_pointer()
                .flex()
                .items_center()
                .justify_center()
                .child(ui::icon::icon(IconName::Play, theme.on_accent))
                .on_click(on_click),
        )
}

/// What a video in a message draws: its box, or `None` for the card.
fn render(ctx: &PlayableCtx, _window: &mut Window, cx: &mut App) -> Option<AnyElement> {
    if ctx.kind != PlayableKind::Video || !available(cx) {
        return None;
    }
    let theme = theme::get(cx).clone();
    let key = ctx.key.clone();
    let (w, h) = box_size(&ctx.media);
    let name = ctx.media.file_name();
    let inline = match current(&key, cx).map(|c| &c.place) {
        Some(Place::Inline { view, controls }) => Some((view.clone(), controls.clone())),
        _ => None,
    };
    let stage = div()
        .id(ElementId::Name(format!("{key}-stage").into()))
        .relative()
        .w(px(w))
        .h(px(h))
        .flex_none()
        .overflow_hidden()
        .bg(gpui_kit::black());
    let row = div()
        .w_full()
        .h(px(BAR_H))
        .flex_none()
        .px_2()
        .flex()
        .items_center()
        .gap_2()
        .overflow_hidden();
    let name_text = div()
        .flex_1()
        .min_w_0()
        .text_sm()
        .text_color(theme.text)
        .truncate()
        .child(name.clone());
    let (stage, row) = if failed(&key, cx) {
        (
            stage.child(
                div()
                    .size_full()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap_2()
                    .bg(theme.sidebar)
                    .child(ui::icon::icon(IconName::Play, theme.muted))
                    .child(
                        div()
                            .id(ElementId::Name(format!("{key}-cannot-play").into()))
                            .test_support()
                            .aria_label(SharedString::from(t!("video-cannot-play")))
                            .text_sm()
                            .text_color(theme.muted)
                            .child(t!("video-cannot-play")),
                    ),
            ),
            row.child(name_text).child(fallback(ctx, &theme, cx)),
        )
    } else if let Some((view, controls)) = inline {
        let ended = view.read(cx).video().state() == State::Ended;
        let (again, big, page) = (key.clone(), ctx.clone(), ctx.page.clone());
        (
            stage.child(view).when(ended, |d| {
                d.child(play_button(
                    format!("{key}-play-video"),
                    t!("video-play-again").to_string(),
                    &theme,
                    move |_, _, cx| toggle(&again, cx),
                ))
            }),
            row.child(div().flex_1().min_w_0().child(controls))
                .child(ui::icon::button(
                    ElementId::Name(format!("{key}-enlarge").into()),
                    IconName::Maximize,
                    t!("video-enlarge"),
                    &theme,
                    move |_, window, cx| enlarge(&big, window, cx),
                ))
                .child(ui::icon::button(
                    ElementId::Name(format!("{key}-browser").into()),
                    IconName::ExternalLink,
                    t!("video-open-in-browser"),
                    &theme,
                    move |_, _, cx| cx.open_url(&page),
                )),
        )
    } else {
        let start = ctx.clone();
        let length = ctx
            .duration
            .filter(|d| d.is_finite() && *d > 0.)
            .map(|d| controls::clock(Duration::from_secs_f64(d)));
        (
            stage
                .when_some(ctx.poster.clone(), |d, poster| {
                    d.child(
                        img(SharedUri::from(poster))
                            .size_full()
                            .object_fit(ObjectFit::Contain),
                    )
                })
                .child(play_button(
                    format!("{key}-play-video"),
                    t!("video-play", name = name.clone()).to_string(),
                    &theme,
                    move |_, window, cx| play(&start, window, cx),
                )),
            row.child(name_text).when_some(length, |d, length| {
                d.child(
                    div()
                        .id(ElementId::Name(format!("{key}-length").into()))
                        .test_support()
                        .flex_none()
                        .text_xs()
                        .text_color(theme.muted)
                        .child(length),
                )
            }),
        )
    };
    Some(
        div()
            .id(ElementId::Name(format!("{key}-video").into()))
            .test_support()
            .role(Role::Group)
            .aria_label(SharedString::from(name))
            .mt_1()
            .w(px(w))
            .flex()
            .flex_col()
            .rounded_md()
            .overflow_hidden()
            .border_1()
            .border_color(theme.rail)
            .bg(theme.sidebar)
            .child(stage)
            .child(row)
            .into_any_element(),
    )
}

/// What a video that cannot play offers instead: an attachment's download,
/// copy and open buttons, as its file card has, or a link's page.
fn fallback(ctx: &PlayableCtx, theme: &Theme, cx: &App) -> AnyElement {
    if ctx.page == ctx.media.url {
        let runtime = ctx.session.read(cx).runtime.clone();
        return viewer::actions(&ctx.key, &ctx.media, runtime, theme).into_any_element();
    }
    let page = ctx.page.clone();
    ui::icon::button(
        ElementId::Name(format!("{}-browser", ctx.key).into()),
        IconName::ExternalLink,
        t!("video-open-in-browser"),
        theme,
        move |_, _, cx| cx.open_url(&page),
    )
    .into_any_element()
}

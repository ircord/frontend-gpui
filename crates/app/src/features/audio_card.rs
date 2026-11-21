//! The audio card: a sound in a message plays in place (package M3).
//!
//! The card is a registered playable for `PlayableKind::Audio`. Pressing play
//! fetches the sound, decodes it into a `Track` that the one global
//! [`Playing`] holds, and drives the shared transport bar. Only one sound or
//! video plays at a time: the card [`media::claim`]s when it starts, and stops
//! when another key takes [`NowPlaying`].

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::TestSupportExt;
use gpui_kit::prelude::*;
use gpui_kit::*;
use ircord_platform::audio::{Output, Track};
use tokio::runtime::Runtime;

use crate::features::viewer::{self, Media};
use crate::media::controls::{Action, Controls, Seek, Transport};
use crate::media::{self, NowPlaying, sound};
use crate::registry::{self, PlayableCtx, PlayableKind, Slot};
use crate::session::SessionEvent;
use crate::theme;

/// The largest sound fetched to play; a larger one shows the open button.
const MAX_BYTES: usize = 64 << 20;
/// How often a playing card refreshes its bar.
const TICK: Duration = Duration::from_millis(250);
/// The card's size, the same in every state.
const WIDTH: f32 = 440.;
const HEIGHT: f32 = 64.;

const AUDIO_TYPES: [&str; 7] = [
    "audio/mpeg",
    "audio/mp4",
    "audio/aac",
    "audio/ogg",
    "audio/flac",
    "audio/wav",
    "audio/x-wav",
];
const AUDIO_EXTENSIONS: [&str; 6] = ["mp3", "m4a", "aac", "ogg", "flac", "wav"];

pub fn init(cx: &mut App) {
    registry::register(cx, |r| r.playables.add(Slot::new("audio-card", offer)));
}

/// The cards by their key, kept so a card keeps its state between frames.
#[derive(Default)]
struct Cards(HashMap<String, Entity<AudioCard>>);
impl Global for Cards {}

/// What the network task brings back: the status, the content type and the
/// body of the sound, or why it did not come.
type Fetched = Result<Result<(u16, Option<String>, Vec<u8>), String>, tokio::task::JoinError>;

/// The one track playing now, with the key of the card that plays it.
#[derive(Default)]
struct Playing(Option<(String, Track)>);
impl Global for Playing {}

/// The track held for `key`, if that card is the one playing.
fn playing_track<'a>(key: &str, cx: &'a App) -> Option<&'a Track> {
    cx.try_global::<Playing>()?
        .0
        .as_ref()
        .filter(|(k, _)| k == key)
        .map(|(_, track)| track)
}

/// Let go of `key`'s track. Dropping it stops the sound.
fn drop_track(key: &str, cx: &mut App) {
    let playing = cx.default_global::<Playing>();
    if playing.0.as_ref().is_some_and(|(k, _)| k == key) {
        playing.0 = None;
    }
}

/// Whether the card plays this file; otherwise the file card shows. `.opus`
/// is left to the browser. Other audio plays when its type or its extension
/// is one that the decoder reads.
pub fn plays_here(media: &Media) -> bool {
    let ext = extension(&media.url);
    if ext == "opus" {
        return false;
    }
    AUDIO_TYPES.contains(&media.content_type.as_str()) || AUDIO_EXTENSIONS.contains(&ext.as_str())
}

/// The lower-case extension of the file in `url`, without the query.
fn extension(url: &str) -> String {
    let path = url.split(['?', '#']).next().unwrap_or("");
    let file = path.rsplit('/').next().unwrap_or("");
    file.rsplit_once('.')
        .map_or(String::new(), |(_, ext)| ext.to_ascii_lowercase())
}

/// The registered playable: the card for this sound, made on first sight.
fn offer(ctx: &PlayableCtx, window: &mut Window, cx: &mut App) -> Option<AnyElement> {
    if ctx.kind != PlayableKind::Audio || !plays_here(&ctx.media) {
        return None;
    }
    let existing = cx
        .try_global::<Cards>()
        .and_then(|cards| cards.0.get(&ctx.key))
        .cloned();
    let card = match existing {
        Some(card) => card,
        None => {
            let card = new_card(ctx, window, cx);
            cx.default_global::<Cards>()
                .0
                .insert(ctx.key.clone(), card.clone());
            card
        }
    };
    Some(card.into_any_element())
}

fn new_card(ctx: &PlayableCtx, window: &mut Window, cx: &mut App) -> Entity<AudioCard> {
    let session = ctx.session.clone();
    let runtime = session.read(cx).runtime.clone();
    let key = ctx.key.clone();
    let card = cx.new(|cx| AudioCard {
        key: key.clone(),
        media: ctx.media.clone(),
        runtime,
        state: State::Idle,
        length: None,
        muted: false,
        ticking: false,
        shown: Transport::default(),
        controls: None,
        _subscriptions: vec![
            cx.subscribe(
                &session,
                |card: &mut AudioCard, _, event: &SessionEvent, cx| {
                    if matches!(event, SessionEvent::Selection) {
                        card.stop(cx);
                    }
                },
            ),
            cx.observe_global::<NowPlaying>(|card, cx| {
                if media::holder(cx).as_deref() != Some(card.key.as_str()) {
                    card.halt(cx);
                }
            }),
        ],
    });
    // The bar's callbacks reach the card through a weak handle: the bar is
    // made after the card, so it cannot be captured by the card itself.
    let weak = card.downgrade();
    let (on_toggle, on_seek, on_mute) = (weak.clone(), weak.clone(), weak);
    let toggle: Action = Rc::new(move |_: &mut Window, cx: &mut App| {
        on_toggle.update(cx, |card, cx| card.toggle(cx)).ok();
    });
    let seek: Seek = Rc::new(move |at: Duration, _: &mut Window, cx: &mut App| {
        on_seek.update(cx, |card, cx| card.seek(at, cx)).ok();
    });
    let mute: Action = Rc::new(move |_: &mut Window, cx: &mut App| {
        on_mute.update(cx, |card, cx| card.mute(cx)).ok();
    });
    let bar = cx.new(|cx| Controls::new(key, toggle, seek, mute, window, cx));
    card.update(cx, |card, _| card.controls = Some(bar));
    card
}

/// Why a sound did not play.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Failure {
    NoDevice,
    TooLarge,
    CannotPlay,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum State {
    Idle,
    Loading,
    Ready,
    Failed(Failure),
}

pub struct AudioCard {
    key: String,
    media: Media,
    runtime: Arc<Runtime>,
    state: State,
    /// The length, once the sound is decoded.
    length: Option<Duration>,
    muted: bool,
    /// A refresh loop is running while the sound plays.
    ticking: bool,
    /// What the bar showed last, so it is pushed only on a change.
    shown: Transport,
    controls: Option<Entity<Controls>>,
    _subscriptions: Vec<Subscription>,
}

impl AudioCard {
    /// Play or pause; with no sound loaded, fetch and start it.
    fn toggle(&mut self, cx: &mut Context<Self>) {
        match playing_track(&self.key, cx).map(|t| t.paused()) {
            Some(true) => {
                if let Some(track) = playing_track(&self.key, cx) {
                    track.play();
                }
                self.tick(cx);
            }
            Some(false) => {
                if let Some(track) = playing_track(&self.key, cx) {
                    track.pause();
                }
            }
            None if self.state != State::Loading => self.start(cx),
            None => {}
        }
        cx.notify();
    }

    /// Claim the sound, then fetch and decode it on the network runtime.
    fn start(&mut self, cx: &mut Context<Self>) {
        let Some(output) = sound::output(cx) else {
            return self.fail(Failure::NoDevice, cx);
        };
        media::claim(&self.key, cx);
        self.state = State::Loading;
        cx.notify();
        let url = self.media.url.clone();
        let fetch = self
            .runtime
            .spawn(async move { ircord_client::api::fetch(&url, MAX_BYTES).await });
        cx.spawn(async move |this, cx| {
            let fetched = fetch.await;
            this.update(cx, |card, cx| card.loaded(fetched, &output, cx))
                .ok();
        })
        .detach();
    }

    fn loaded(&mut self, fetched: Fetched, output: &Output, cx: &mut Context<Self>) {
        if media::holder(cx).as_deref() != Some(self.key.as_str()) {
            // Another card or a video took the sound while this one loaded.
            self.state = State::Idle;
            cx.notify();
            return;
        }
        let bytes = match fetched {
            Ok(Ok((status, _, bytes))) if (200..300).contains(&status) => bytes,
            Ok(Err(e)) if e.contains("larger than") => {
                return self.fail(Failure::TooLarge, cx);
            }
            _ => {
                log::warn!("a sound did not load");
                return self.fail(Failure::CannotPlay, cx);
            }
        };
        let track = match output.track(bytes) {
            Ok(track) => track,
            Err(e) => {
                log::warn!("a sound did not decode: {e}");
                return self.fail(Failure::CannotPlay, cx);
            }
        };
        track.set_volume(if self.muted { 0.0 } else { 1.0 });
        self.length = track.duration();
        track.play();
        cx.default_global::<Playing>().0 = Some((self.key.clone(), track));
        self.state = State::Ready;
        self.tick(cx);
        cx.notify();
    }

    fn fail(&mut self, reason: Failure, cx: &mut Context<Self>) {
        drop_track(&self.key, cx);
        media::release(&self.key, cx);
        self.state = State::Failed(reason);
        cx.notify();
    }

    fn seek(&mut self, at: Duration, cx: &mut Context<Self>) {
        if let Some(Err(e)) = playing_track(&self.key, cx).map(|track| track.seek(at)) {
            log::warn!("a sound did not seek: {e}");
        }
        cx.notify();
    }

    fn mute(&mut self, cx: &mut Context<Self>) {
        self.muted = !self.muted;
        if let Some(track) = playing_track(&self.key, cx) {
            track.set_volume(if self.muted { 0.0 } else { 1.0 });
        }
        cx.notify();
    }

    /// Stop because the person moved to another conversation: back to idle.
    fn stop(&mut self, cx: &mut Context<Self>) {
        self.halt(cx);
        media::release(&self.key, cx);
    }

    /// Drop the track because another key took the sound.
    fn halt(&mut self, cx: &mut Context<Self>) {
        drop_track(&self.key, cx);
        if matches!(self.state, State::Ready | State::Loading) {
            self.state = State::Idle;
        }
        cx.notify();
    }

    /// Refresh the bar every `TICK` while the sound plays. The loop ends when
    /// the sound pauses, ends or is dropped.
    fn tick(&mut self, cx: &mut Context<Self>) {
        if self.ticking {
            return;
        }
        self.ticking = true;
        let runtime = self.runtime.clone();
        cx.spawn(async move |this, cx| {
            loop {
                let _ = runtime
                    .spawn(async { tokio::time::sleep(TICK).await })
                    .await;
                let more = this
                    .update(cx, |card, cx| card.refresh(cx))
                    .unwrap_or(false);
                if !more {
                    break;
                }
            }
        })
        .detach();
    }

    /// One refresh. Whether the loop goes on.
    fn refresh(&mut self, cx: &mut Context<Self>) -> bool {
        let Some((ended, paused)) = playing_track(&self.key, cx).map(|t| (t.ended(), t.paused()))
        else {
            // Stopped elsewhere: the card is idle at 0:00.
            self.ticking = false;
            self.halt(cx);
            return false;
        };
        if ended {
            let key = self.key.clone();
            drop_track(&key, cx);
            media::release(&key, cx);
            self.ticking = false;
            self.state = State::Idle;
            cx.notify();
            return false;
        }
        cx.notify();
        if paused {
            self.ticking = false;
        }
        !paused
    }

    /// Where the sound is now, for the bar.
    fn transport(&self, cx: &App) -> Transport {
        let (position, playing) = match playing_track(&self.key, cx) {
            Some(track) => (track.position(), !track.paused()),
            None => (Duration::ZERO, false),
        };
        Transport {
            playing,
            loading: self.state == State::Loading,
            position,
            duration: self.length,
            muted: self.muted,
        }
    }

    /// Hand the bar what the sound is doing, when that has changed.
    fn push_transport(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(bar) = self.controls.clone() else {
            return;
        };
        let transport = self.transport(cx);
        if transport != self.shown {
            self.shown = transport.clone();
            bar.update(cx, |bar, cx| bar.set(transport, window, cx));
        }
    }
}

impl Render for AudioCard {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme::get(cx).clone();
        self.push_transport(window, cx);
        let name = self.media.file_name();
        let label = t!("audio-card-label", name = name.clone()).to_string();
        let body = match self.state {
            State::Failed(reason) => {
                let text = match reason {
                    Failure::NoDevice => t!("audio-card-no-device"),
                    Failure::TooLarge => t!("audio-card-too-large"),
                    Failure::CannotPlay => t!("audio-card-cannot-play"),
                }
                .to_string();
                div()
                    .id(ElementId::Name(format!("{}-failure", self.key).into()))
                    .test_support()
                    .aria_label(SharedString::from(text.clone()))
                    .text_xs()
                    .text_color(theme.danger)
                    .child(text)
                    .into_any_element()
            }
            _ => match &self.controls {
                Some(bar) => bar.clone().into_any_element(),
                None => div().into_any_element(),
            },
        };
        div()
            .id(ElementId::Name(format!("{}-card", self.key).into()))
            .test_support()
            .role(Role::Group)
            .aria_label(SharedString::from(label))
            .w(px(WIDTH))
            .h(px(HEIGHT))
            .flex()
            .flex_col()
            .justify_between()
            .px_2()
            .py_1()
            .rounded_md()
            .border_1()
            .border_color(theme.border)
            .bg(theme.sidebar)
            .overflow_hidden()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .w_full()
                    .min_w_0()
                    .child(div().flex_1().min_w_0().truncate().text_sm().child(name))
                    .child(viewer::actions(
                        &self.key,
                        &self.media,
                        self.runtime.clone(),
                        &theme,
                    )),
            )
            .child(body)
    }
}

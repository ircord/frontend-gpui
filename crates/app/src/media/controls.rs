//! The transport bar the audio card and the video player share: play or
//! pause, the time, a seek slider and mute. The player owns the sound or
//! video; it tells the bar where it is with [`Controls::set`] and hears
//! back through three callbacks.

use std::rc::Rc;
use std::time::Duration;

use gpui_kit::TestSupportExt;
use gpui_kit::component::slider::{Slider, SliderEvent, SliderState, SliderValue};
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::theme;
use crate::ui;
use crate::ui::icon::IconName;

/// Where a player is.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Transport {
    pub playing: bool,
    pub loading: bool,
    pub position: Duration,
    pub duration: Option<Duration>,
    pub muted: bool,
}

pub type Action = Rc<dyn Fn(&mut Window, &mut App)>;
pub type Seek = Rc<dyn Fn(Duration, &mut Window, &mut App)>;

/// The bar. Element IDs start with its key: `{key}-play`, `{key}-time`,
/// `{key}-seek` and `{key}-mute`.
pub struct Controls {
    key: SharedString,
    slider: Entity<SliderState>,
    transport: Transport,
    /// Between a slider change and its release: the person is dragging, so
    /// the player's position does not move the thumb.
    dragging: bool,
    on_toggle: Action,
    on_mute: Action,
    _seek: Subscription,
}

impl Controls {
    pub fn new(
        key: impl Into<SharedString>,
        on_toggle: Action,
        on_seek: Seek,
        on_mute: Action,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // The thumb is a fraction of the length, so the length can change.
        let slider = cx.new(|_| SliderState::new().min(0.0).max(1.0).step(0.001));
        let seek = cx.subscribe_in(
            &slider,
            window,
            move |this, _, event, window, cx| match event {
                SliderEvent::Change(_) => this.dragging = true,
                SliderEvent::Release(value) => {
                    this.dragging = false;
                    if let Some(length) = this.transport.duration {
                        let at = length.mul_f32(fraction(*value));
                        on_seek(at, window, cx);
                    }
                }
            },
        );
        Self {
            key: key.into(),
            slider,
            transport: Transport::default(),
            dragging: false,
            on_toggle,
            on_mute,
            _seek: seek,
        }
    }

    /// Where the player is now. The thumb follows, except while dragged.
    pub fn set(&mut self, transport: Transport, window: &mut Window, cx: &mut Context<Self>) {
        if !self.dragging {
            let at = match transport.duration {
                Some(length) if !length.is_zero() => {
                    (transport.position.as_secs_f32() / length.as_secs_f32()).clamp(0.0, 1.0)
                }
                _ => 0.0,
            };
            self.slider.update(cx, |s, cx| s.set_value(at, window, cx));
        }
        self.transport = transport;
        cx.notify();
    }

    pub fn transport(&self) -> &Transport {
        &self.transport
    }

    /// The seek slider's state, for tests.
    pub fn slider(&self) -> &Entity<SliderState> {
        &self.slider
    }
}

fn fraction(value: SliderValue) -> f32 {
    value.end().clamp(0.0, 1.0)
}

/// A time as a clock: `0:05`, `1:05`, `1:02:05` (hours only when needed).
pub fn clock(at: Duration) -> String {
    let s = at.as_secs();
    let (h, m, s) = (s / 3600, s / 60 % 60, s % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// A length as a clock, or `--:--` when it is not known.
pub fn length(of: Option<Duration>) -> String {
    of.map_or_else(|| "--:--".to_string(), clock)
}

/// What the bar shows for the time: `0:12 / 3:45`.
pub fn time_text(t: &Transport) -> String {
    format!("{} / {}", clock(t.position), length(t.duration))
}

impl Render for Controls {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme::get(cx).clone();
        let key = self.key.clone();
        let t = &self.transport;
        let (icon, label) = if t.loading {
            (IconName::LoaderCircle, t!("media-loading"))
        } else if t.playing {
            (IconName::Pause, t!("media-pause"))
        } else {
            (IconName::Play, t!("media-play"))
        };
        let (mute_icon, mute_label) = if t.muted {
            (IconName::Ban, t!("media-unmute"))
        } else {
            (IconName::Bell, t!("media-mute"))
        };
        let (at, total) = (clock(t.position), length(t.duration));
        let toggle = self.on_toggle.clone();
        let mute = self.on_mute.clone();
        div()
            .id(ElementId::Name(key.clone()))
            .flex()
            .items_center()
            .gap_2()
            .w_full()
            .child(ui::icon::button(
                ElementId::Name(format!("{key}-play").into()),
                icon,
                label,
                &theme,
                move |_, window, cx| toggle(window, cx),
            ))
            .child(
                div()
                    .id(ElementId::Name(format!("{key}-time").into()))
                    .test_support()
                    .aria_label(SharedString::from(t!(
                        "media-time",
                        at = at.clone(),
                        length = total.clone()
                    )))
                    .flex_none()
                    .text_xs()
                    .text_color(theme.muted)
                    .child(time_text(t)),
            )
            .child(
                div()
                    .id(ElementId::Name(format!("{key}-seek").into()))
                    .test_support()
                    .flex_1()
                    .min_w_0()
                    .child(
                        Slider::new(&self.slider)
                            .disabled(t.duration.is_none())
                            .accessibility_label(t!("media-seek")),
                    ),
            )
            .child(ui::icon::button(
                ElementId::Name(format!("{key}-mute").into()),
                mute_icon,
                mute_label,
                &theme,
                move |_, window, cx| mute(window, cx),
            ))
    }
}

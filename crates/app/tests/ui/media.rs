//! Package M0: the transport bar, the chime's rate limit and "the one thing
//! playing" that the audio card and the video player share.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::component::slider::{SliderEvent, SliderValue};
use gpui_kit::prelude::*;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{
    AnyWindowHandle, Bounds, Entity, Point, TestAppContext, WindowBounds, WindowOptions, px, size,
};
use ircord_app::media::controls::{self, Controls, Transport};
use ircord_app::media::{self, sound};
use ircord_platform::audio::Chime;

#[derive(Default)]
struct Calls {
    toggles: u32,
    mutes: u32,
    seeks: Vec<Duration>,
}

fn bar(cx: &mut TestAppContext) -> (AnyWindowHandle, Entity<Controls>, Rc<RefCell<Calls>>) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        ircord_app::init(cx);
    });
    let calls = Rc::new(RefCell::new(Calls::default()));
    let (toggle, seek, mute) = (calls.clone(), calls.clone(), calls.clone());
    let (window, view) = cx.update(|cx| {
        gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: Point::default(),
                    size: size(px(480.), px(80.)),
                })),
                ..Default::default()
            },
            cx,
            move |window, cx| {
                cx.new(|cx| {
                    Controls::new(
                        "bar",
                        Rc::new(move |_, _| toggle.borrow_mut().toggles += 1),
                        Rc::new(move |at, _, _| seek.borrow_mut().seeks.push(at)),
                        Rc::new(move |_, _| mute.borrow_mut().mutes += 1),
                        window,
                        cx,
                    )
                })
            },
        )
        .unwrap()
    });
    (window, view, calls)
}

fn at(position: u64, length: Option<u64>) -> Transport {
    Transport {
        position: Duration::from_secs(position),
        duration: length.map(Duration::from_secs),
        ..Default::default()
    }
}

fn set(cx: &mut TestAppContext, window: AnyWindowHandle, view: &Entity<Controls>, t: Transport) {
    cx.update_window(window, |_, window, cx| {
        view.update(cx, |bar, cx| bar.set(t, window, cx))
    })
    .unwrap();
    cx.run_until_parked();
}

fn thumb(cx: &mut TestAppContext, view: &Entity<Controls>) -> f32 {
    cx.update(|cx| view.read(cx).slider().read(cx).value().end())
}

#[gpui_kit::test]
fn the_bar_shows_the_time_and_plays(cx: &mut TestAppContext) {
    let (window, view, calls) = bar(cx);
    set(cx, window, &view, at(12, Some(225)));
    assert_eq!(controls::time_text(&at(12, Some(225))), "0:12 / 3:45");
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("bar-time").label(), Some("0:12 of 3:45"));
        assert_eq!(window.find("bar-play").label(), Some("Play"));
        assert_eq!(window.find("bar-mute").label(), Some("Mute"));
        window.click("bar-play", cx);
        window.click("bar-mute", cx);
    })
    .unwrap();
    assert_eq!(calls.borrow().toggles, 1);
    assert_eq!(calls.borrow().mutes, 1);
    set(
        cx,
        window,
        &view,
        Transport {
            playing: true,
            muted: true,
            ..at(12, Some(225))
        },
    );
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("bar-play").label(), Some("Pause"));
        assert_eq!(window.find("bar-mute").label(), Some("Unmute"));
    })
    .unwrap();
}

#[gpui_kit::test]
fn releasing_the_slider_seeks_and_dragging_holds_the_thumb(cx: &mut TestAppContext) {
    let (window, view, calls) = bar(cx);
    set(cx, window, &view, at(50, Some(200)));
    assert!((thumb(cx, &view) - 0.25).abs() < 1e-3);
    let slider = cx.update(|cx| view.read(cx).slider().clone());
    // The person drags to 40 %: the player's position does not move the thumb.
    slider.update(cx, |_, cx| {
        cx.emit(SliderEvent::Change(SliderValue::Single(0.4)))
    });
    cx.run_until_parked();
    let before = thumb(cx, &view);
    set(cx, window, &view, at(60, Some(200)));
    assert_eq!(
        thumb(cx, &view),
        before,
        "a set while dragging holds the thumb"
    );
    assert!(calls.borrow().seeks.is_empty(), "a change does not seek");
    // Let go at 40 %: one seek to 80 s.
    slider.update(cx, |_, cx| {
        cx.emit(SliderEvent::Release(SliderValue::Single(0.4)))
    });
    cx.run_until_parked();
    assert_eq!(calls.borrow().seeks.len(), 1);
    let seek = calls.borrow().seeks[0];
    assert!(
        (seek.as_secs_f32() - 80.0).abs() < 0.01,
        "seeks to 80 s, not {seek:?}"
    );
    // Released: the position moves the thumb again.
    set(cx, window, &view, at(100, Some(200)));
    assert!((thumb(cx, &view) - 0.5).abs() < 1e-3);
}

#[test]
fn clocks() {
    assert_eq!(controls::clock(Duration::from_secs(5)), "0:05");
    assert_eq!(controls::clock(Duration::from_secs(65)), "1:05");
    assert_eq!(controls::clock(Duration::from_secs(3725)), "1:02:05");
    assert_eq!(controls::length(None), "--:--");
}

#[gpui_kit::test]
fn a_chime_plays_at_most_once_in_a_second_and_a_half(cx: &mut TestAppContext) {
    cx.update(|cx| {
        sound::use_null(cx);
        assert!(sound::chime(Chime::Mention, cx));
        assert!(!sound::chime(Chime::Message, cx), "too soon");
        assert_eq!(sound::chimes(cx), 1);
    });
}

#[gpui_kit::test]
fn one_thing_plays_at_a_time(cx: &mut TestAppContext) {
    cx.update(|cx| {
        assert_eq!(media::holder(cx), None);
        media::claim("a", cx);
        media::claim("b", cx);
        assert_eq!(media::holder(cx).as_deref(), Some("b"));
        media::release("a", cx);
        assert_eq!(media::holder(cx).as_deref(), Some("b"));
        media::release("b", cx);
        assert_eq!(media::holder(cx), None);
    });
}

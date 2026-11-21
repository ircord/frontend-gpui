//! The video player (package M5). Starts with the view drawing a fake
//! backend's pictures, one per frame.
use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::prelude::*;
use gpui_kit::{Bounds, Point, TestAppContext, WindowBounds, WindowOptions, px, size};
use ircord_app::media::video_view::VideoView;
use ircord_platform::video::{Frame, State, Video};

struct Fake {
    serial: Rc<Cell<u64>>,
    state: Rc<Cell<State>>,
}

impl Video for Fake {
    fn play(&self) {
        self.state.set(State::Playing)
    }
    fn pause(&self) {
        self.state.set(State::Paused)
    }
    fn seek(&self, _to: Duration) {}
    fn set_volume(&self, _volume: f32) {}
    fn set_muted(&self, _muted: bool) {}
    fn state(&self) -> State {
        self.state.get()
    }
    fn position(&self) -> Duration {
        Duration::from_millis(self.serial.get() * 33)
    }
    fn duration(&self) -> Option<Duration> {
        Some(Duration::from_secs(6))
    }
    fn frame(&self, after: u64) -> Option<Frame> {
        // A new 4x2 picture every time it is asked while playing.
        if self.state.get() == State::Playing {
            self.serial.set(self.serial.get() + 1);
        }
        let serial = self.serial.get();
        (serial > after).then(|| Frame {
            width: 4,
            height: 2,
            bgra: vec![serial as u8; 4 * 2 * 4],
            serial,
        })
    }
}

#[gpui_kit::test]
fn the_video_view_draws_one_picture_per_frame_while_playing(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let serial = Rc::new(Cell::new(0));
    let state = Rc::new(Cell::new(State::Paused));
    let fake = Fake {
        serial: serial.clone(),
        state: state.clone(),
    };
    let (window, _view) = cx.update(|cx| {
        gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: Point::default(),
                    size: size(px(400.), px(300.)),
                })),
                ..Default::default()
            },
            cx,
            |_, cx| cx.new(|cx| VideoView::new(Box::new(fake), cx)),
        )
        .unwrap()
    });
    cx.run_until_parked();
    // Paused: nothing asked for, nothing drawn, no frame requested.
    assert_eq!(serial.get(), 0);
    state.set(State::Playing);
    window.update(cx, |_, window, _| window.refresh()).unwrap();
    cx.run_until_parked();
    let first = serial.get();
    assert!(first >= 1, "a picture was taken once playing");
    // Each simulated frame asks for the next picture.
    for _ in 0..5 {
        window
            .update(cx, |_, window, cx| {
                window.simulate_next_frame(cx);
            })
            .unwrap();
        cx.run_until_parked();
    }
    assert!(serial.get() >= first + 5, "{} then {}", first, serial.get());
    // Paused again: frames stop.
    state.set(State::Paused);
    for _ in 0..3 {
        window
            .update(cx, |_, window, cx| {
                window.simulate_next_frame(cx);
            })
            .unwrap();
        cx.run_until_parked();
    }
    let rested = serial.get();
    for _ in 0..3 {
        window
            .update(cx, |_, window, cx| {
                window.simulate_next_frame(cx);
            })
            .unwrap();
        cx.run_until_parked();
    }
    assert_eq!(serial.get(), rested);
}

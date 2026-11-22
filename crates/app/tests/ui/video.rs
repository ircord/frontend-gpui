//! The video player (package M5), against fake backends: the view drawing
//! pictures one per frame, and the player in a message: its box, play and
//! pause, failures, one at a time, the large viewer.
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::prelude::*;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{Bounds, Point, TestAppContext, WindowBounds, WindowOptions, px, size};
use ircord_app::features::video::{self, Available, Opener};
use ircord_app::media::{self, video_view::VideoView};
use ircord_client::contract::{Attachment, ChannelSettings, MessagePage};
use ircord_client::testing::Reply;
use ircord_platform::video::{Frame, State, Video};

use crate::fixture::{Fixture, open_with, wait};

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

/// What every fake player opened by [`Backend`] shares with the test.
#[derive(Default)]
struct Backend {
    state: Cell<Option<State>>,
    position: Cell<Duration>,
    serial: Cell<u64>,
    /// The next open fails.
    refuse: Cell<bool>,
    opens: RefCell<Vec<(String, u32, u32)>>,
    seeks: RefCell<Vec<Duration>>,
    drops: Cell<u32>,
}

impl Backend {
    fn state(&self) -> State {
        self.state.get().unwrap_or(State::Paused)
    }
}

struct Player(Rc<Backend>);

impl Video for Player {
    fn play(&self) {
        self.0.state.set(Some(State::Playing))
    }
    fn pause(&self) {
        self.0.state.set(Some(State::Paused))
    }
    fn seek(&self, to: Duration) {
        self.0.seeks.borrow_mut().push(to);
        self.0.position.set(to);
    }
    fn set_volume(&self, _volume: f32) {}
    fn set_muted(&self, _muted: bool) {}
    fn state(&self) -> State {
        self.0.state()
    }
    fn position(&self) -> Duration {
        self.0.position.get()
    }
    fn duration(&self) -> Option<Duration> {
        Some(Duration::from_secs(225))
    }
    fn frame(&self, after: u64) -> Option<Frame> {
        if self.0.state() == State::Playing {
            self.0.serial.set(self.0.serial.get() + 1);
        }
        let serial = self.0.serial.get();
        (serial > after).then(|| Frame {
            width: 4,
            height: 2,
            bgra: vec![200; 4 * 2 * 4],
            serial,
        })
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        self.0.drops.set(self.0.drops.get() + 1);
    }
}

/// The fixture with one message holding two videos in #general, video on
/// and opened by the fake `Backend`.
fn videos(cx: &mut TestAppContext, on: bool) -> (Fixture, Rc<Backend>) {
    let f = open_with(cx, true, |server| {
        let mut s = crate::fixture::snapshot();
        s.conversations[0].channel_settings = Some(ChannelSettings {
            nsfw: false,
            ..Default::default()
        });
        server.snapshot(&s);
        server.route_fn("GET", "/api/v1/conversations/*/messages", |request| {
            let mut items = Vec::new();
            if request.path.ends_with("/c-general/messages") {
                let mut m = crate::fixture::message("a1", "c-general", "");
                let clip = |name: &str| Attachment {
                    url: format!("https://x.test/files/v/t/{name}"),
                    name: name.into(),
                    content_type: Some("video/mp4".into()),
                    size: Some(3_000_000),
                    width: 1600,
                    height: 400,
                    ..Default::default()
                };
                m.attachments = Some(vec![clip("one.mp4"), clip("two.mp4")]);
                items.push(m);
            }
            Reply::json(
                200,
                serde_json::to_value(MessagePage {
                    items,
                    ..Default::default()
                })
                .unwrap(),
            )
        });
    });
    let backend = Rc::new(Backend::default());
    let b = backend.clone();
    cx.update(|cx| {
        media::sound::use_null(cx);
        cx.set_global(Available(on));
        cx.set_global(Opener(Rc::new(move |url, w, h| {
            b.opens.borrow_mut().push((url.to_string(), w, h));
            if b.refuse.take() {
                return Err("refused".into());
            }
            b.state.set(Some(State::Paused));
            b.position.set(Duration::ZERO);
            Ok(Box::new(Player(b.clone())) as Box<dyn Video>)
        })));
    });
    wait(cx, "#general's history", |cx| {
        f.session.read(cx).client().is_some_and(|c| {
            c.read()
                .timelines
                .get("c-general")
                .is_some_and(|t| t.loaded && t.entries.len() == 1)
        })
    });
    draw(&f, cx);
    (f, backend)
}

fn draw(f: &Fixture, cx: &mut TestAppContext) {
    cx.update_window(f.window, |_, window, cx| {
        window.refresh();
        window.render_frame(cx);
    })
    .unwrap();
}

fn click(f: &Fixture, cx: &mut TestAppContext, id: &'static str) {
    draw(f, cx);
    cx.update_window(f.window, |_, window, cx| window.click(id, cx))
        .unwrap();
    cx.run_until_parked();
    draw(f, cx);
}

fn shown(f: &Fixture, cx: &mut TestAppContext, id: &'static str) -> bool {
    draw(f, cx);
    cx.update_window(f.window, |_, window, _| {
        window.try_find(id).is_some_and(|e| e.visible())
    })
    .unwrap()
}

fn label(f: &Fixture, cx: &mut TestAppContext, id: &'static str) -> Option<String> {
    draw(f, cx);
    cx.update_window(f.window, |_, window, _| {
        window
            .try_find(id)
            .and_then(|e| e.label().map(str::to_string))
    })
    .unwrap()
}

fn height(f: &Fixture, cx: &mut TestAppContext, id: &'static str) -> f32 {
    draw(f, cx);
    cx.update_window(f.window, |_, window, _| {
        f32::from(window.find(id).bounds().size.height)
    })
    .unwrap()
}

fn frames(f: &Fixture, cx: &mut TestAppContext, n: usize) {
    for _ in 0..n {
        cx.update_window(f.window, |_, window, cx| window.simulate_next_frame(cx))
            .unwrap();
        cx.run_until_parked();
    }
}

fn playing(cx: &mut TestAppContext) -> Option<String> {
    cx.update(|cx| video::playing(cx))
}

#[gpui_kit::test]
fn a_video_attachment_shows_its_box_or_the_old_card(cx: &mut TestAppContext) {
    let (f, _) = videos(cx, true);
    assert!(shown(&f, cx, "file-a1-0-play-video"));
    assert!(shown(&f, cx, "file-a1-0-video"));
    assert!(!shown(&f, cx, "file-a1-0-detail"));
    assert_eq!(
        label(&f, cx, "file-a1-0-play-video").as_deref(),
        Some("Play one.mp4")
    );
    // 1600x400 fitted into 400x300, plus the row under it.
    assert_eq!(
        video::box_size(&ircord_app::features::viewer::Media {
            url: String::new(),
            name: String::new(),
            width: 1600,
            height: 400,
            content_type: String::new(),
            size: None,
        }),
        (400., 100.)
    );
    assert_eq!(video::fit(0, 0, 400., 300.), (400., 225.));
    cx.update(|cx| cx.set_global(Available(false)));
    assert!(shown(&f, cx, "file-a1-0-detail"));
    assert!(!shown(&f, cx, "file-a1-0-video"));
}

#[gpui_kit::test]
fn play_draws_pictures_pause_stops_them_and_the_time_follows(cx: &mut TestAppContext) {
    let (f, backend) = videos(cx, true);
    click(&f, cx, "file-a1-0-play-video");
    assert_eq!(playing(cx).as_deref(), Some("file-a1-0"));
    assert_eq!(
        cx.update(|cx| media::holder(cx)).as_deref(),
        Some("file-a1-0")
    );
    {
        let opens = backend.opens.borrow();
        let (url, w, h) = &opens[0];
        assert!(url.ends_with("/one.mp4"), "{url}");
        let scale = cx
            .update_window(f.window, |_, window, _| window.scale_factor())
            .unwrap();
        assert_eq!((*w, *h), ((400. * scale) as u32, (100. * scale) as u32));
    }
    assert_eq!(backend.state(), State::Playing);
    let first = backend.serial.get();
    frames(&f, cx, 5);
    assert!(backend.serial.get() >= first + 5, "one picture per frame");
    // The time text follows the player.
    backend.position.set(Duration::from_secs(12));
    wait(cx, "the bar to learn the position", |cx| {
        cx.update_window(f.window, |_, window, cx| {
            window.render_frame(cx);
            window.find("file-a1-0-time").label() == Some("0:12 of 3:45")
        })
        .unwrap()
    });
    assert_eq!(label(&f, cx, "file-a1-0-play").as_deref(), Some("Pause"));
    click(&f, cx, "file-a1-0-play");
    assert_eq!(backend.state(), State::Paused);
    frames(&f, cx, 2);
    let rested = backend.serial.get();
    frames(&f, cx, 3);
    assert_eq!(backend.serial.get(), rested, "paused: no more pictures");
    // Ended: the play button shows over the last picture and plays again
    // from the start.
    backend.state.set(Some(State::Ended));
    wait(cx, "the play button over the end", |cx| {
        cx.update_window(f.window, |_, window, cx| {
            window.render_frame(cx);
            window.try_find("file-a1-0-play-video").is_some()
        })
        .unwrap()
    });
    assert_eq!(
        playing(cx).as_deref(),
        Some("file-a1-0"),
        "ended, not stopped"
    );
    click(&f, cx, "file-a1-0-play-video");
    assert_eq!(backend.opens.borrow().len(), 1, "the same player again");
    assert_eq!(backend.state(), State::Playing);
    assert_eq!(backend.seeks.borrow().last(), Some(&Duration::ZERO));
}

#[gpui_kit::test]
fn a_video_that_cannot_play_shows_the_card_and_says_so(cx: &mut TestAppContext) {
    let (f, backend) = videos(cx, true);
    backend.refuse.set(true);
    click(&f, cx, "file-a1-0-play-video");
    assert_eq!(playing(cx), None);
    assert!(shown(&f, cx, "file-a1-0-cannot-play"));
    assert!(shown(&f, cx, "file-a1-0-open"), "the card's buttons");
    assert_eq!(
        label(&f, cx, "file-a1-0-cannot-play").as_deref(),
        Some("This video cannot play here")
    );
    // One that fails while playing.
    click(&f, cx, "file-a1-1-play-video");
    assert_eq!(playing(cx).as_deref(), Some("file-a1-1"));
    backend.state.set(Some(State::Failed));
    wait(cx, "the failure to be seen", |cx| {
        video::playing(cx).is_none()
    });
    assert!(shown(&f, cx, "file-a1-1-cannot-play"));
    assert_eq!(backend.drops.get(), 1, "the failed player is dropped");
    assert_eq!(cx.update(|cx| media::holder(cx)), None);
}

#[gpui_kit::test]
fn a_second_video_or_another_conversation_drops_the_player(cx: &mut TestAppContext) {
    let (f, backend) = videos(cx, true);
    click(&f, cx, "file-a1-0-play-video");
    click(&f, cx, "file-a1-1-play-video");
    assert_eq!(backend.drops.get(), 1, "the first is dropped");
    assert_eq!(playing(cx).as_deref(), Some("file-a1-1"));
    // Something else claims the one thing playing (an audio card).
    cx.update(|cx| media::claim("audio-x", cx));
    cx.run_until_parked();
    assert_eq!(playing(cx), None);
    assert_eq!(backend.drops.get(), 2);
    click(&f, cx, "file-a1-0-play-video");
    f.session
        .update(cx, |s, cx| s.select_conversation("c-random", cx));
    cx.run_until_parked();
    assert_eq!(playing(cx), None, "another conversation stops it");
    assert_eq!(backend.drops.get(), 3);
}

#[gpui_kit::test]
fn enlarge_opens_the_viewer_at_the_same_place_and_closing_drops_it(cx: &mut TestAppContext) {
    let (f, backend) = videos(cx, true);
    click(&f, cx, "file-a1-0-play-video");
    backend.position.set(Duration::from_secs(3));
    click(&f, cx, "file-a1-0-enlarge");
    assert_eq!(backend.drops.get(), 1, "the inline player is dropped");
    assert_eq!(backend.opens.borrow().len(), 2);
    let (_, w, _) = backend.opens.borrow()[1].clone();
    assert!(w > 400, "opened at the dialog's size, not the box's: {w}");
    assert_eq!(backend.seeks.borrow().last(), Some(&Duration::from_secs(3)));
    assert_eq!(backend.state(), State::Playing);
    assert!(shown(&f, cx, "file-a1-0-viewer"));
    assert_eq!(playing(cx).as_deref(), Some("file-a1-0"));
    // Space pauses and plays.
    cx.update_window(f.window, |_, window, cx| window.press("space", cx))
        .unwrap();
    cx.run_until_parked();
    assert_eq!(backend.state(), State::Paused);
    cx.update_window(f.window, |_, window, cx| window.press("space", cx))
        .unwrap();
    cx.run_until_parked();
    assert_eq!(backend.state(), State::Playing);
    // Escape closes it, and the player goes with it.
    cx.update_window(f.window, |_, window, cx| window.press("escape", cx))
        .unwrap();
    wait(cx, "the viewer's player to go", |cx| {
        cx.update_window(f.window, |_, window, cx| window.render_frame(cx))
            .unwrap();
        video::playing(cx).is_none()
    });
    wait(cx, "the player to be dropped", |_| backend.drops.get() == 2);
    assert!(!shown(&f, cx, "file-a1-0-viewer"));
}

fn double_click(f: &Fixture, cx: &mut TestAppContext, id: &'static str) {
    draw(f, cx);
    cx.update_window(f.window, |_, window, cx| window.double_click(id, cx))
        .unwrap();
    cx.run_until_parked();
    draw(f, cx);
}

#[gpui_kit::test]
fn a_click_on_the_picture_pauses_and_plays_it(cx: &mut TestAppContext) {
    let (f, backend) = videos(cx, true);
    click(&f, cx, "file-a1-0-play-video");
    assert_eq!(backend.state(), State::Playing);
    click(&f, cx, "file-a1-0-stage");
    assert_eq!(backend.state(), State::Paused);
    click(&f, cx, "file-a1-0-stage");
    assert_eq!(backend.state(), State::Playing);
    assert_eq!(
        backend.opens.borrow().len(),
        1,
        "no viewer opened by a click"
    );
}

#[gpui_kit::test]
fn a_double_click_on_the_picture_enlarges_it_and_keeps_it_playing(cx: &mut TestAppContext) {
    let (f, backend) = videos(cx, true);
    click(&f, cx, "file-a1-0-play-video");
    double_click(&f, cx, "file-a1-0-stage");
    assert!(shown(&f, cx, "file-a1-0-viewer"));
    assert_eq!(backend.state(), State::Playing, "the viewer plays");
    assert_eq!(playing(cx).as_deref(), Some("file-a1-0"));
}

#[gpui_kit::test]
fn in_the_viewer_a_click_pauses_and_a_double_click_leaves_it_playing(cx: &mut TestAppContext) {
    let (f, backend) = videos(cx, true);
    click(&f, cx, "file-a1-0-play-video");
    click(&f, cx, "file-a1-0-enlarge");
    assert_eq!(backend.state(), State::Playing);
    click(&f, cx, "file-a1-0-large-picture");
    assert_eq!(backend.state(), State::Paused);
    click(&f, cx, "file-a1-0-large-picture");
    assert_eq!(backend.state(), State::Playing);
    // A double-click pauses with its first click and plays with its second,
    // and toggles fullscreen on the second; the viewer stays open.
    double_click(&f, cx, "file-a1-0-large-picture");
    assert_eq!(
        backend.state(),
        State::Playing,
        "the pair leaves it playing"
    );
    assert!(shown(&f, cx, "file-a1-0-viewer"));
}

#[gpui_kit::test]
fn the_box_keeps_its_height_idle_loading_playing_and_failed(cx: &mut TestAppContext) {
    let (f, backend) = videos(cx, true);
    let idle = height(&f, cx, "file-a1-0-video");
    click(&f, cx, "file-a1-0-play-video");
    backend.state.set(Some(State::Loading));
    wait(cx, "the bar to say loading", |cx| {
        cx.update_window(f.window, |_, window, cx| {
            window.render_frame(cx);
            window.find("file-a1-0-play").label() == Some("Loading")
        })
        .unwrap()
    });
    let loading = height(&f, cx, "file-a1-0-video");
    backend.state.set(Some(State::Playing));
    frames(&f, cx, 2);
    let playing = height(&f, cx, "file-a1-0-video");
    backend.state.set(Some(State::Failed));
    wait(cx, "the failure", |cx| video::playing(cx).is_none());
    assert!(shown(&f, cx, "file-a1-0-cannot-play"));
    let failed = height(&f, cx, "file-a1-0-video");
    assert_eq!((idle, loading, playing, failed), (idle, idle, idle, idle));
}

#[gpui_kit::test]
fn the_audit_passes_with_a_video_playing(cx: &mut TestAppContext) {
    let (f, _) = videos(cx, true);
    let idle = cx
        .update_window(f.window, |_, window, cx| {
            crate::audit::audit("video", window, cx)
        })
        .unwrap();
    assert!(idle.is_empty(), "{idle:#?}");
    click(&f, cx, "file-a1-0-play-video");
    let problems = cx
        .update_window(f.window, |_, window, cx| {
            crate::audit::audit("video", window, cx)
        })
        .unwrap();
    assert!(problems.is_empty(), "{problems:#?}");
}

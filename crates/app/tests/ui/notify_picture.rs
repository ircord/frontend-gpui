//! Package M2: sender pictures on notifications, against the canned server.
//! The picture is a 96x96 PNG in the config directory, fetched once for a
//! day; a direct conversation shows its partner; nothing shows when the
//! alert hides who wrote, when no one is found, when the body is not a
//! picture or when it does not arrive within the wait; a fixed answer wins.

use std::cell::RefCell;
use std::io::Cursor;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::TestAppContext;
use ircord_app::notify::picture::{Fixed, resolve};
use ircord_client::contract::{Alert, Snapshot};
use ircord_client::testing::Reply;
use serde_json::json;

use crate::fixture::{Fixture, bootstrap, open_with, snapshot, wait};

const AVATAR: &str = "/files/bob/avatar.png";

/// A 200x100 red picture: not square, so the crop is visible in the size.
fn png() -> Vec<u8> {
    let mut bytes = Vec::new();
    image::RgbImage::from_pixel(200, 100, image::Rgb([200, 30, 30]))
        .write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png)
        .unwrap();
    bytes
}

/// The fixture's workspace with a direct conversation with bob, and bob's
/// picture on this server.
fn workspace() -> Snapshot {
    let mut s = snapshot();
    s.conversations.push(
        serde_json::from_value(json!({
            "id": "c-dm", "networkId": "n1", "kind": "direct", "name": "bob",
            "favorite": false, "joined": true, "memberIds": ["m-bob", "u-me"],
            "memberModes": {}, "modes": "", "topic": "",
            "unreadCount": 0, "mentionCount": 0
        }))
        .unwrap(),
    );
    for m in &mut s.members {
        if m.id == "m-bob" {
            m.avatar_url = Some(AVATAR.into());
        }
    }
    s
}

fn open(cx: &mut TestAppContext, reply: Reply) -> Fixture {
    open_with(cx, true, move |server| {
        server.snapshot(&workspace());
        server.route_reply("GET", AVATAR, reply);
    })
}

/// A visible alert for `message` in `conversation`.
fn alert(conversation: &str, message: &str) -> Alert {
    Alert {
        alert_id: format!("{conversation}/{message}"),
        conversation_id: conversation.into(),
        message_id: message.into(),
        preview: true,
        title: "bob".into(),
        ..Default::default()
    }
}

/// Ask for the picture and run GPUI until it answers.
fn picture(f: &Fixture, cx: &mut TestAppContext, alert: Alert) -> Option<PathBuf> {
    let answer = Rc::new(RefCell::new(None));
    let slot = answer.clone();
    cx.update(|cx| {
        let task = f.session.update(cx, |s, cx| resolve(s, &alert, cx));
        cx.spawn(async move |_| {
            *slot.borrow_mut() = Some(task.await);
        })
        .detach();
    });
    wait(cx, "the picture's answer", |_| answer.borrow().is_some());
    answer.take().unwrap()
}

fn avatar_requests(f: &Fixture) -> usize {
    f.server.requests_to("GET", AVATAR).len()
}

fn cache_dir(f: &Fixture, cx: &mut TestAppContext) -> PathBuf {
    f.session
        .read_with(cx, |s, _| s.config.path().join("notify").join("avatars"))
}

#[gpui_kit::test]
fn a_sender_picture_is_a_96_pixel_square_png_in_the_cache(cx: &mut TestAppContext) {
    let f = open(cx, Reply::bytes(200, "image/png", png()));
    bootstrap(&f, cx);
    let path = picture(&f, cx, alert("c-general", "1")).expect("bob's picture");
    assert!(path.starts_with(cache_dir(&f, cx)), "{path:?}");
    let picture = image::open(&path).expect("a PNG file");
    assert_eq!((picture.width(), picture.height()), (96, 96));
    assert_eq!(avatar_requests(&f), 1);
}

#[gpui_kit::test]
fn a_fresh_cached_picture_is_not_fetched_again(cx: &mut TestAppContext) {
    let f = open(cx, Reply::bytes(200, "image/png", png()));
    bootstrap(&f, cx);
    let first = picture(&f, cx, alert("c-general", "1")).expect("bob's picture");
    let second = picture(&f, cx, alert("c-general", "1")).expect("the cached picture");
    assert_eq!(first, second);
    assert_eq!(avatar_requests(&f), 1, "the second call read the cache");
}

#[gpui_kit::test]
fn a_direct_conversation_shows_its_partner(cx: &mut TestAppContext) {
    let f = open(cx, Reply::bytes(200, "image/png", png()));
    bootstrap(&f, cx);
    // The message is not in the timeline, so only the partner can be shown.
    let path = picture(&f, cx, alert("c-dm", "not-held")).expect("the partner's picture");
    assert!(path.exists());
    assert_eq!(avatar_requests(&f), 1);
}

#[gpui_kit::test]
fn a_hidden_preview_shows_no_picture_and_fetches_nothing(cx: &mut TestAppContext) {
    let f = open(cx, Reply::bytes(200, "image/png", png()));
    bootstrap(&f, cx);
    let hidden = Alert {
        preview: false,
        ..alert("c-general", "1")
    };
    assert_eq!(picture(&f, cx, hidden), None);
    assert_eq!(avatar_requests(&f), 0);
}

#[gpui_kit::test]
fn nobody_found_shows_no_picture(cx: &mut TestAppContext) {
    let f = open(cx, Reply::bytes(200, "image/png", png()));
    bootstrap(&f, cx);
    assert_eq!(picture(&f, cx, alert("c-general", "missing")), None);
    assert_eq!(avatar_requests(&f), 0);
}

#[gpui_kit::test]
fn a_body_that_is_not_a_picture_shows_no_picture(cx: &mut TestAppContext) {
    let f = open(
        cx,
        Reply::bytes(200, "text/plain", b"not a picture".to_vec()),
    );
    bootstrap(&f, cx);
    assert_eq!(picture(&f, cx, alert("c-general", "1")), None);
    assert_eq!(avatar_requests(&f), 1);
}

#[gpui_kit::test]
fn a_picture_that_never_arrives_is_given_up(cx: &mut TestAppContext) {
    // The server holds its answer until the test lets go of it, then sends a
    // real picture, so only giving up can make the result `None`: a client
    // that waited for the answer would find the picture. No clock is read.
    let (release, held) = std::sync::mpsc::channel::<()>();
    let held = std::sync::Mutex::new(held);
    let picture_bytes = png();
    let f = open_with(cx, true, move |server| {
        server.snapshot(&workspace());
        server.route_fn("GET", AVATAR, move |_| {
            let _ = held.lock().unwrap().recv_timeout(Duration::from_secs(60));
            Reply::bytes(200, "image/png", picture_bytes.clone())
        });
    });
    bootstrap(&f, cx);
    assert_eq!(picture(&f, cx, alert("c-general", "1")), None);
    drop(release);
}

#[gpui_kit::test]
fn a_fixed_answer_still_wins(cx: &mut TestAppContext) {
    let f = open(cx, Reply::bytes(200, "image/png", png()));
    bootstrap(&f, cx);
    let fixed = PathBuf::from("/fixed/picture.png");
    cx.update(|cx| cx.set_global(Fixed(Some(fixed.clone()))));
    assert_eq!(picture(&f, cx, alert("c-general", "1")), Some(fixed));
    assert_eq!(avatar_requests(&f), 0);
}

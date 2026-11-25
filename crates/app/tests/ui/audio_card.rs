//! Package M3: the audio card. A sound in a message plays in place, one at a
//! time, on a device that plays nothing aloud (`sound::use_null`). The sound
//! itself comes from a small HTTP server in this file.

use std::io::{Read, Write};
use std::time::Duration;

use gpui_kit::TestAppContext;
use gpui_kit::prelude::*;
use gpui_kit::test::TestWindowExt;
use ircord_app::media::{self, sound};
use ircord_client::contract::{Attachment, ChannelSettings, MessagePage};
use ircord_client::testing::Reply;

use crate::fixture::{self, Fixture};

/// A mono 16-bit WAV of `seconds`, a quiet tone.
pub(super) fn wav(seconds: u32) -> Vec<u8> {
    let rate = 44_100u32;
    let samples = rate * seconds;
    let data_len = samples * 2;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for i in 0..samples {
        let v = ((i as f32 * 0.05).sin() * 8000.0) as i16;
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

/// Serve `body` as `content_type` on a local port, for as long as the test
/// runs. Returns the address of the file.
fn serve(body: Vec<u8>, content_type: &'static str) -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut request = [0u8; 4096];
            let _ = stream.read(&mut request);
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.write_all(&body);
        }
    });
    format!("http://{addr}/file")
}

fn file(url: &str, name: &str, content_type: &str) -> Attachment {
    Attachment {
        url: url.into(),
        name: name.into(),
        content_type: Some(content_type.into()),
        size: Some(3_000_000),
        ..Default::default()
    }
}

/// A window with one message `a1` carrying `attachments`, its history shown.
fn open(cx: &mut TestAppContext, attachments: Vec<Attachment>) -> Fixture {
    let f = fixture::open_with(cx, true, |server| {
        let mut s = fixture::snapshot();
        s.conversations[0].channel_settings = Some(ChannelSettings {
            nsfw: false,
            ..Default::default()
        });
        server.snapshot(&s);
        server.route_fn("GET", "/api/v1/conversations/*/messages", move |_| {
            let mut m = fixture::message("a1", "c-general", "");
            m.attachments = Some(attachments.clone());
            Reply::json(
                200,
                serde_json::to_value(MessagePage {
                    items: vec![m],
                    ..Default::default()
                })
                .unwrap(),
            )
        });
    });
    fixture::wait(cx, "#general's history", |cx| {
        f.session.read(cx).client().is_some_and(|c| {
            c.read()
                .timelines
                .get("c-general")
                .is_some_and(|t| t.loaded && t.entries.len() == 1)
        })
    });
    cx.update(sound::use_null);
    f
}

fn render(cx: &mut TestAppContext, f: &Fixture) {
    cx.update_window(f.window, |_, window, cx| {
        window.refresh();
        window.render_frame(cx);
    })
    .unwrap();
}

fn label(cx: &mut TestAppContext, f: &Fixture, id: &'static str) -> Option<String> {
    render(cx, f);
    cx.update_window(f.window, |_, window, _| {
        window.find(id).label().map(str::to_string)
    })
    .unwrap()
}

fn height(cx: &mut TestAppContext, f: &Fixture, id: &'static str) -> f32 {
    render(cx, f);
    cx.update_window(f.window, |_, window, _| {
        f32::from(window.find(id).bounds().size.height)
    })
    .unwrap()
}

fn click(cx: &mut TestAppContext, f: &Fixture, id: &'static str) {
    render(cx, f);
    cx.update_window(f.window, |_, window, cx| window.click(id, cx))
        .unwrap();
}

fn holder(cx: &mut TestAppContext) -> Option<String> {
    cx.update(|cx| media::holder(cx).map(|s| s.to_string()))
}

/// Until `what` holds on the window's labels; the sound plays in real time.
fn until(
    cx: &mut TestAppContext,
    f: &Fixture,
    what: &str,
    ok: impl Fn(&Fixture, &mut TestAppContext) -> bool,
) {
    let deadline = std::time::Instant::now() + crate::fixture::HANG_GUARD;
    loop {
        cx.run_until_parked();
        if ok(f, cx) {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[gpui_kit::test]
fn an_mp3_attachment_shows_the_audio_card(cx: &mut TestAppContext) {
    let url = serve(wav(2), "audio/mpeg");
    let f = open(cx, vec![file(&url, "tone.mp3", "audio/mpeg")]);
    render(cx, &f);
    cx.update_window(f.window, |_, window, _| {
        assert!(window.find("file-a1-0-card").visible());
        assert!(window.try_find("file-a1-0-detail").is_none());
        assert_eq!(window.find("file-a1-0-play").label(), Some("Play"));
    })
    .unwrap();
}

#[gpui_kit::test]
fn an_opus_attachment_keeps_the_file_card(cx: &mut TestAppContext) {
    let url = serve(wav(2), "audio/ogg");
    let f = open(
        cx,
        vec![file(&format!("{url}/song.opus"), "song.opus", "audio/ogg")],
    );
    render(cx, &f);
    cx.update_window(f.window, |_, window, _| {
        assert!(window.try_find("file-a1-0-card").is_none());
        assert!(window.find("file-a1-0-detail").visible());
    })
    .unwrap();
}

#[gpui_kit::test]
fn a_wav_plays_seeks_pauses_and_returns_to_idle_at_the_end(cx: &mut TestAppContext) {
    let url = serve(wav(2), "audio/wav");
    let f = open(cx, vec![file(&url, "tone.wav", "audio/wav")]);
    let idle = height(cx, &f, "file-a1-0-card");
    assert_eq!(
        label(cx, &f, "file-a1-0-time").as_deref(),
        Some("0:00 of --:--")
    );

    click(cx, &f, "file-a1-0-play");
    until(cx, &f, "the sound to load", |f, cx| {
        label(cx, f, "file-a1-0-time").is_some_and(|t| t.ends_with("0:02"))
    });
    assert_eq!(holder(cx).as_deref(), Some("file-a1-0"));
    until(cx, &f, "the time to advance", |f, cx| {
        label(cx, f, "file-a1-0-time").is_some_and(|t| !t.starts_with("0:00"))
    });
    let playing = height(cx, &f, "file-a1-0-card");
    assert_eq!(idle, playing, "the card keeps its height");

    // Pause holds the time.
    click(cx, &f, "file-a1-0-play");
    until(cx, &f, "the pause", |f, cx| {
        label(cx, f, "file-a1-0-play").as_deref() == Some("Play")
    });
    let paused_at = label(cx, &f, "file-a1-0-time");
    std::thread::sleep(Duration::from_millis(1200));
    assert_eq!(label(cx, &f, "file-a1-0-time"), paused_at);

    // Resume, then it plays to the end and goes idle at 0:00.
    click(cx, &f, "file-a1-0-play");
    until(cx, &f, "the end of the sound", |f, cx| {
        label(cx, f, "file-a1-0-time").is_some_and(|t| t.starts_with("0:00 of"))
            && label(cx, f, "file-a1-0-play").as_deref() == Some("Play")
    });
    assert_eq!(holder(cx), None);
    assert_eq!(height(cx, &f, "file-a1-0-card"), idle);
}

#[gpui_kit::test]
fn mute_toggles_its_label(cx: &mut TestAppContext) {
    let url = serve(wav(2), "audio/wav");
    let f = open(cx, vec![file(&url, "tone.wav", "audio/wav")]);
    assert_eq!(label(cx, &f, "file-a1-0-mute").as_deref(), Some("Mute"));
    click(cx, &f, "file-a1-0-mute");
    assert_eq!(label(cx, &f, "file-a1-0-mute").as_deref(), Some("Unmute"));
}

#[gpui_kit::test]
fn a_body_that_is_not_audio_shows_the_open_button(cx: &mut TestAppContext) {
    let url = serve(b"this is not a sound".to_vec(), "audio/mpeg");
    let f = open(cx, vec![file(&url, "broken.mp3", "audio/mpeg")]);
    click(cx, &f, "file-a1-0-play");
    until(cx, &f, "the failure", |f, cx| {
        render(cx, f);
        cx.update_window(f.window, |_, window, _| {
            window.try_find("file-a1-0-failure").is_some()
        })
        .unwrap()
    });
    render(cx, &f);
    cx.update_window(f.window, |_, window, _| {
        assert_eq!(
            window.find("file-a1-0-failure").label(),
            Some("This sound cannot be played here; open it instead")
        );
        assert!(window.find("file-a1-0-open").visible());
    })
    .unwrap();
    assert_eq!(holder(cx), None);
}

#[gpui_kit::test]
fn starting_a_second_sound_stops_the_first(cx: &mut TestAppContext) {
    let url = serve(wav(2), "audio/wav");
    let f = open(
        cx,
        vec![
            file(&url, "one.wav", "audio/wav"),
            file(&url, "two.wav", "audio/wav"),
        ],
    );
    click(cx, &f, "file-a1-0-play");
    until(cx, &f, "the first to play", |_, cx| {
        holder(cx).as_deref() == Some("file-a1-0")
    });
    click(cx, &f, "file-a1-1-play");
    until(cx, &f, "the second to play", |_, cx| {
        holder(cx).as_deref() == Some("file-a1-1")
    });
    until(cx, &f, "the first to stop", |f, cx| {
        label(cx, f, "file-a1-0-play").as_deref() == Some("Play")
    });
}

#[gpui_kit::test]
fn seeking_moves_the_time(cx: &mut TestAppContext) {
    let url = serve(wav(2), "audio/wav");
    let f = open(cx, vec![file(&url, "tone.wav", "audio/wav")]);
    click(cx, &f, "file-a1-0-play");
    until(cx, &f, "the sound to load", |f, cx| {
        label(cx, f, "file-a1-0-time").is_some_and(|t| t.ends_with("0:02"))
    });
    // Near the end of the slider: 90% of 2 s is 1.8 s.
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        let bounds = window.find("file-a1-0-seek").bounds();
        let at = gpui_kit::point(bounds.size.width * 0.9, bounds.size.height / 2.0);
        window.click_at("file-a1-0-seek", at, cx);
    })
    .unwrap();
    until(cx, &f, "the seek to show", |f, cx| {
        label(cx, f, "file-a1-0-time").is_some_and(|t| t.starts_with("0:01"))
    });
}

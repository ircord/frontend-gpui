//! `ircord-video <url> <width> <height>`: play `url` with GStreamer and
//! write what the app draws to stdout; take commands on stdin.
//!
//! `playbin` fetches, decodes and plays the sound. Its pictures are
//! converted to BGRA, scaled down into the box asked for and written out.
//!
//! Out (binary, little endian):
//!   `F` u32 width, u32 height, width*height*4 bytes   a picture
//!   `S` u8 state, u64 position ms, u64 duration ms     ten times a second
//!       state: 0 loading, 1 playing, 2 paused, 3 ended, 4 failed;
//!       duration `u64::MAX` when unknown
//! In (text lines): `play`, `pause`, `seek <ms>`, `volume <0..1>`,
//! `mute <0|1>`. End of input ends the program.
//!
//! `ircord-video --probe` exits 0 when GStreamer has what this needs, and
//! otherwise 1 after naming each missing part on its own line of stdout:
//! `gstreamer` (it does not start), `base`, `good` or `libav` (its plugins
//! of those names).

#[cfg(not(target_os = "linux"))]
fn main() {
    std::process::exit(2);
}

#[cfg(target_os = "linux")]
fn main() {
    std::process::exit(linux::run());
}

#[cfg(target_os = "linux")]
mod linux {
    use std::io::{BufRead, Write};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use gstreamer as gst;
    use gstreamer::prelude::*;
    use gstreamer_app as gst_app;
    use gstreamer_video as gst_video;

    /// The plugin sets playing needs, each with elements that prove it is
    /// there: `playbin`'s own path (base), fetching over HTTP and the MP4
    /// and WebM containers (good), and the decoders (libav).
    const PARTS: [(&str, &[&str]); 3] = [
        (
            "base",
            &["playbin", "appsink", "videoconvert", "videoscale"],
        ),
        ("good", &["souphttpsrc", "qtdemux", "matroskademux"]),
        ("libav", &["avdec_h264"]),
    ];

    /// The parts GStreamer lacks; empty when it has all of them.
    fn missing() -> Vec<&'static str> {
        if gst::init().is_err() {
            return vec!["gstreamer"];
        }
        PARTS
            .iter()
            .filter(|(_, elements)| {
                !elements
                    .iter()
                    .all(|name| gst::ElementFactory::find(name).is_some())
            })
            .map(|(part, _)| *part)
            .collect()
    }

    fn ready() -> bool {
        missing().is_empty()
    }

    pub fn run() -> i32 {
        let args: Vec<String> = std::env::args().skip(1).collect();
        if args.first().map(String::as_str) == Some("--probe") {
            let missing = missing();
            for part in &missing {
                println!("{part}");
            }
            return if missing.is_empty() { 0 } else { 1 };
        }
        let (Some(url), Some(width), Some(height)) = (
            args.first(),
            args.get(1).and_then(|a| a.parse::<u32>().ok()),
            args.get(2).and_then(|a| a.parse::<u32>().ok()),
        ) else {
            eprintln!("usage: ircord-video <url> <width> <height>");
            return 2;
        };
        if !ready() {
            return 1;
        }
        match play(url, width.max(2), height.max(2)) {
            Ok(()) => 0,
            Err(e) => {
                eprintln!("ircord-video: {e}");
                1
            }
        }
    }

    fn play(url: &str, width: u32, height: u32) -> Result<(), String> {
        let description = format!(
            "videoconvert ! videoscale ! appsink name=pictures max-buffers=2 drop=true \
             caps=video/x-raw,format=BGRA,width=[1,{width}],height=[1,{height}],pixel-aspect-ratio=1/1"
        );
        let bin =
            gst::parse::bin_from_description(&description, true).map_err(|e| e.to_string())?;
        let appsink = bin
            .by_name("pictures")
            .and_then(|e| e.downcast::<gst_app::AppSink>().ok())
            .ok_or("no appsink")?;
        let out = Arc::new(Mutex::new(std::io::stdout()));
        let gone = Arc::new(AtomicBool::new(false));

        let (writer, closed) = (out.clone(), gone.clone());
        appsink.set_callbacks(
            gst_app::AppSinkCallbacks::builder()
                .new_sample(move |sink| {
                    let sample = sink.pull_sample().map_err(|_| gst::FlowError::Eos)?;
                    let (Some(buffer), Some(caps)) = (sample.buffer(), sample.caps()) else {
                        return Ok(gst::FlowSuccess::Ok);
                    };
                    let Ok(info) = gst_video::VideoInfo::from_caps(caps) else {
                        return Ok(gst::FlowSuccess::Ok);
                    };
                    let Ok(map) = buffer.map_readable() else {
                        return Ok(gst::FlowSuccess::Ok);
                    };
                    let (w, h) = (info.width() as usize, info.height() as usize);
                    let stride = info.stride()[0] as usize;
                    let mut message = Vec::with_capacity(9 + w * h * 4);
                    message.push(b'F');
                    message.extend_from_slice(&(w as u32).to_le_bytes());
                    message.extend_from_slice(&(h as u32).to_le_bytes());
                    for y in 0..h {
                        message.extend_from_slice(&map[y * stride..y * stride + w * 4]);
                    }
                    let mut out = writer.lock().unwrap();
                    if out.write_all(&message).and_then(|()| out.flush()).is_err() {
                        closed.store(true, Ordering::Relaxed);
                    }
                    Ok(gst::FlowSuccess::Ok)
                })
                .build(),
        );

        let playbin = gst::ElementFactory::make("playbin")
            .property("uri", url)
            .property("video-sink", &bin)
            .build()
            .map_err(|e| e.to_string())?;
        playbin
            .set_state(gst::State::Paused)
            .map_err(|e| e.to_string())?;

        // Commands.
        let (commands, closed) = (playbin.clone(), gone.clone());
        std::thread::spawn(move || {
            for line in std::io::stdin().lock().lines() {
                let Ok(line) = line else { break };
                let mut words = line.split_whitespace();
                match (words.next(), words.next()) {
                    (Some("play"), _) => drop(commands.set_state(gst::State::Playing)),
                    (Some("pause"), _) => drop(commands.set_state(gst::State::Paused)),
                    (Some("seek"), Some(ms)) => {
                        if let Ok(ms) = ms.parse::<u64>() {
                            let _ = commands.seek_simple(
                                gst::SeekFlags::FLUSH | gst::SeekFlags::ACCURATE,
                                gst::ClockTime::from_mseconds(ms),
                            );
                        }
                    }
                    (Some("volume"), Some(v)) => {
                        if let Ok(v) = v.parse::<f64>() {
                            commands.set_property("volume", v.clamp(0.0, 1.0));
                        }
                    }
                    (Some("mute"), Some(v)) => commands.set_property("mute", v == "1"),
                    _ => {}
                }
            }
            closed.store(true, Ordering::Relaxed);
        });

        // Status, until the app goes away.
        let bus = playbin.bus().ok_or("no bus")?;
        let (mut ended, mut failed) = (false, false);
        while !gone.load(Ordering::Relaxed) {
            while let Some(message) = bus.timed_pop(gst::ClockTime::from_mseconds(100)) {
                match message.view() {
                    gst::MessageView::Eos(_) => ended = true,
                    gst::MessageView::Error(e) => {
                        eprintln!("ircord-video: {}", e.error());
                        failed = true;
                    }
                    gst::MessageView::AsyncDone(_) => ended = false,
                    _ => {}
                }
            }
            let (_, current, pending) = playbin.state(gst::ClockTime::ZERO);
            let state: u8 = if failed {
                4
            } else if ended {
                3
            } else {
                match (current, pending) {
                    (gst::State::Playing, _) => 1,
                    (gst::State::Paused, gst::State::VoidPending) => 2,
                    _ => 0,
                }
            };
            let position = playbin
                .query_position::<gst::ClockTime>()
                .map_or(0, |t| t.mseconds());
            let duration = playbin
                .query_duration::<gst::ClockTime>()
                .map_or(u64::MAX, |t| t.mseconds());
            let mut message = vec![b'S', state];
            message.extend_from_slice(&position.to_le_bytes());
            message.extend_from_slice(&duration.to_le_bytes());
            let mut out = out.lock().unwrap();
            if out.write_all(&message).and_then(|()| out.flush()).is_err() {
                break;
            }
        }
        let _ = playbin.set_state(gst::State::Null);
        let _ = Duration::ZERO;
        Ok(())
    }
}

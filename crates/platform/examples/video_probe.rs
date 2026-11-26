//! `video_probe <url> [width height]`: play for two seconds and say what came out.
use std::time::{Duration, Instant};

fn main() {
    let mut args = std::env::args().skip(1);
    let url = args.next().expect("a URL");
    let width: u32 = args.next().and_then(|a| a.parse().ok()).unwrap_or(480);
    let height: u32 = args.next().and_then(|a| a.parse().ok()).unwrap_or(270);
    println!("probe: {:?}", ircord_platform::video::probe());
    let video = ircord_platform::video::open(&url, width, height).expect("open");
    video.set_muted(true);
    video.play();
    let started = Instant::now();
    let (mut serial, mut frames, mut size) = (0u64, 0u32, (0, 0));
    let mut seeked = false;
    while started.elapsed() < Duration::from_secs(3) {
        if let Some(frame) = video.frame(serial) {
            assert_eq!(frame.bgra.len() as u32, frame.width * frame.height * 4);
            serial = frame.serial;
            frames += 1;
            size = (frame.width, frame.height);
        }
        if !seeked && started.elapsed() > Duration::from_millis(1500) {
            println!(
                "at 1.5s: state {:?} position {:?} duration {:?} frames {frames} size {size:?}",
                video.state(),
                video.position(),
                video.duration()
            );
            video.seek(Duration::from_secs(4));
            seeked = true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    println!(
        "after seek to 4s: state {:?} position {:?} frames {frames} last serial {serial}",
        video.state(),
        video.position()
    );
    video.pause();
    std::thread::sleep(Duration::from_millis(300));
    println!("paused: state {:?}", video.state());
}

//! Video through each system's own player. The player fetches, decodes,
//! plays the sound and keeps picture and sound in step; this crate only
//! copies out the current picture as BGRA at the size asked for.
use std::time::Duration;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

/// One picture: `width * height * 4` bytes, B, G, R, A, rows top to bottom.
#[derive(Clone)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
    /// Goes up by one with every new picture.
    pub serial: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Loading,
    Playing,
    Paused,
    Ended,
    Failed,
}

pub trait Video {
    fn play(&self);
    fn pause(&self);
    fn seek(&self, to: Duration);
    fn set_volume(&self, volume: f32);
    fn set_muted(&self, muted: bool);
    fn state(&self) -> State;
    fn position(&self) -> Duration;
    fn duration(&self) -> Option<Duration>;
    /// The newest picture if it is newer than `after`.
    fn frame(&self, after: u64) -> Option<Frame>;
}

/// Whether this build can play video on this machine.
///
/// `IRCORD_VIDEO=0` turns video off; `IRCORD_VIDEO=1` turns it on where the
/// backend exists but nobody has watched it play yet (`PROVEN` is false).
pub fn available() -> bool {
    let choice = std::env::var("IRCORD_VIDEO").ok();
    if choice.as_deref() == Some("0") {
        return false;
    }
    #[allow(unused_variables)]
    let forced = choice.as_deref() == Some("1");
    #[cfg(windows)]
    return forced || windows::PROVEN;
    #[cfg(target_os = "macos")]
    return forced || macos::PROVEN;
    #[cfg(target_os = "linux")]
    return linux::available();
    #[allow(unreachable_code)]
    false
}

/// Open `url` (http or https), paused, with pictures at most `width` by
/// `height` pixels.
pub fn open(url: &str, width: u32, height: u32) -> Result<Box<dyn Video>, String> {
    #[cfg(windows)]
    return windows::open(url, width, height).map(|v| Box::new(v) as Box<dyn Video>);
    #[cfg(target_os = "macos")]
    return macos::open(url, width, height).map(|v| Box::new(v) as Box<dyn Video>);
    #[cfg(target_os = "linux")]
    return linux::open(url, width, height).map(|v| Box::new(v) as Box<dyn Video>);
    #[allow(unreachable_code)]
    {
        let _ = (url, width, height);
        Err("no video backend in this build".into())
    }
}

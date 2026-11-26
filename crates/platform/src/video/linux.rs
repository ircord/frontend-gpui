//! Linux: the `ircord-video` helper beside the app (crates/video-gst),
//! which plays through the system's GStreamer. The app itself links no
//! GStreamer, so it starts on a machine without it and shows the card.
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::{Frame, Part, Probe, State, Video};

struct Shared {
    frame: Option<Frame>,
    state: State,
    position: Duration,
    duration: Option<Duration>,
}

pub struct HelperVideo {
    child: Child,
    stdin: Mutex<ChildStdin>,
    shared: Arc<Mutex<Shared>>,
}

/// `IRCORD_VIDEO_HELPER`, or `ircord-video` in the app's own folder.
fn helper() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("IRCORD_VIDEO_HELPER") {
        return Some(PathBuf::from(path));
    }
    let beside = std::env::current_exe().ok()?.parent()?.join("ircord-video");
    beside.is_file().then_some(beside)
}

/// What the helper says of this machine's GStreamer. Runs it: call it off
/// the interface thread.
pub fn probe() -> Probe {
    let Some(path) = helper() else {
        return Probe::Off;
    };
    let mut command = Command::new(path);
    command
        .arg("--probe")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    crate::appimage::system_env(&mut command);
    match command.output() {
        Ok(out) if out.status.success() => Probe::Ready,
        Ok(out) if out.status.code() == Some(1) => {
            Probe::Missing(parts(&String::from_utf8_lossy(&out.stdout)))
        }
        // The loader found no GStreamer library to start the helper with.
        Ok(out) if out.status.code() == Some(127) => Probe::Missing(vec![Part::Gstreamer]),
        // A crash is not something to install: no video, and the log says why.
        Ok(out) => {
            log::warn!("the video helper's probe failed: {}", out.status);
            Probe::Off
        }
        Err(e) => {
            log::warn!("the video helper did not start: {e}");
            Probe::Off
        }
    }
}

/// The parts `ircord-video --probe` named, one per line; GStreamer itself
/// when it named none it knows.
pub(super) fn parts(out: &str) -> Vec<Part> {
    let parts: Vec<Part> = out
        .lines()
        .filter_map(|line| match line.trim() {
            "gstreamer" => Some(Part::Gstreamer),
            "base" => Some(Part::Base),
            "good" => Some(Part::Good),
            "libav" => Some(Part::Libav),
            _ => None,
        })
        .collect();
    if parts.is_empty() {
        vec![Part::Gstreamer]
    } else {
        parts
    }
}

pub fn open(url: &str, width: u32, height: u32) -> Result<HelperVideo, String> {
    let path = helper().ok_or("the video helper is not installed")?;
    let mut command = Command::new(path);
    crate::appimage::system_env(&mut command);
    let mut child = command
        .args([url, &width.to_string(), &height.to_string()])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| e.to_string())?;
    let stdin = child.stdin.take().ok_or("no pipe to the video helper")?;
    let mut stdout = child.stdout.take().ok_or("no pipe from the video helper")?;
    let shared = Arc::new(Mutex::new(Shared {
        frame: None,
        state: State::Loading,
        position: Duration::ZERO,
        duration: None,
    }));
    let slot = shared.clone();
    std::thread::Builder::new()
        .name("video".into())
        .spawn(move || {
            let mut serial = 0u64;
            loop {
                let mut kind = [0u8; 1];
                if stdout.read_exact(&mut kind).is_err() {
                    break;
                }
                match kind[0] {
                    b'F' => {
                        let mut head = [0u8; 8];
                        if stdout.read_exact(&mut head).is_err() {
                            break;
                        }
                        let width = u32::from_le_bytes(head[..4].try_into().unwrap());
                        let height = u32::from_le_bytes(head[4..].try_into().unwrap());
                        // A picture is never larger than the box asked for.
                        if width > 8192 || height > 8192 {
                            break;
                        }
                        let mut bgra = vec![0u8; (width * height * 4) as usize];
                        if stdout.read_exact(&mut bgra).is_err() {
                            break;
                        }
                        serial += 1;
                        slot.lock().unwrap().frame = Some(Frame {
                            width,
                            height,
                            bgra,
                            serial,
                        });
                    }
                    b'S' => {
                        let mut body = [0u8; 17];
                        if stdout.read_exact(&mut body).is_err() {
                            break;
                        }
                        let position = u64::from_le_bytes(body[1..9].try_into().unwrap());
                        let duration = u64::from_le_bytes(body[9..].try_into().unwrap());
                        let mut shared = slot.lock().unwrap();
                        shared.state = match body[0] {
                            1 => State::Playing,
                            2 => State::Paused,
                            3 => State::Ended,
                            4 => State::Failed,
                            _ => State::Loading,
                        };
                        shared.position = Duration::from_millis(position);
                        shared.duration =
                            (duration != u64::MAX).then(|| Duration::from_millis(duration));
                    }
                    _ => break,
                }
            }
            // The helper went away: a video that had not ended has failed.
            let mut shared = slot.lock().unwrap();
            if shared.state != State::Ended {
                shared.state = State::Failed;
            }
        })
        .map_err(|e| e.to_string())?;
    Ok(HelperVideo {
        child,
        stdin: Mutex::new(stdin),
        shared,
    })
}

impl HelperVideo {
    fn send(&self, command: &str) {
        let mut stdin = self.stdin.lock().unwrap();
        let _ = writeln!(stdin, "{command}").and_then(|()| stdin.flush());
    }
}

impl Video for HelperVideo {
    fn play(&self) {
        self.send("play")
    }
    fn pause(&self) {
        self.send("pause")
    }
    fn seek(&self, to: Duration) {
        self.send(&format!("seek {}", to.as_millis()))
    }
    fn set_volume(&self, volume: f32) {
        self.send(&format!("volume {}", volume.clamp(0.0, 1.0)))
    }
    fn set_muted(&self, muted: bool) {
        self.send(&format!("mute {}", u8::from(muted)))
    }
    fn state(&self) -> State {
        self.shared.lock().unwrap().state
    }
    fn position(&self) -> Duration {
        self.shared.lock().unwrap().position
    }
    fn duration(&self) -> Option<Duration> {
        self.shared.lock().unwrap().duration
    }
    fn frame(&self, after: u64) -> Option<Frame> {
        let shared = self.shared.lock().unwrap();
        shared.frame.as_ref().filter(|f| f.serial > after).cloned()
    }
}

impl Drop for HelperVideo {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_probe_names_the_missing_parts() {
        assert_eq!(parts("good\nlibav\n"), vec![Part::Good, Part::Libav]);
        assert_eq!(parts("base\n"), vec![Part::Base]);
        assert_eq!(parts("gstreamer\n"), vec![Part::Gstreamer]);
        // An answer it cannot read: GStreamer as a whole.
        assert_eq!(parts(""), vec![Part::Gstreamer]);
        assert_eq!(parts("something else\n"), vec![Part::Gstreamer]);
    }
}

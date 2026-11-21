//! The sound engine as a GPUI global: one output device, opened on first
//! use and kept, and the notification chime with the web client's rate
//! limit.

use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui_kit::*;
use ircord_platform::audio::{Chime, Output};

/// After the device fails to open, wait this long before trying again.
const RETRY: Duration = Duration::from_secs(30);
/// At most one chime this often (the web client's rule).
const CHIME_GAP: Duration = Duration::from_millis(1500);

#[derive(Default)]
struct Sound {
    output: Option<Rc<Output>>,
    failed_at: Option<Instant>,
    logged: bool,
    last_chime: Option<Instant>,
    chimes: u32,
}
impl Global for Sound {}

/// The output device, opened on first use. `None` while it cannot be
/// opened; it is tried again no sooner than 30 s after a failure.
pub fn output(cx: &mut App) -> Option<Rc<Output>> {
    let sound = cx.default_global::<Sound>();
    if let Some(output) = &sound.output {
        return Some(output.clone());
    }
    if sound.failed_at.is_some_and(|at| at.elapsed() < RETRY) {
        return None;
    }
    match Output::open() {
        Ok(output) => {
            let output = Rc::new(output);
            sound.output = Some(output.clone());
            sound.failed_at = None;
            Some(output)
        }
        Err(e) => {
            if !sound.logged {
                log::warn!("no sound output: {e}");
                sound.logged = true;
            }
            sound.failed_at = Some(Instant::now());
            None
        }
    }
}

/// Play the chime, unless one played in the last 1.5 s or there is no
/// device. Whether it played.
pub fn chime(kind: Chime, cx: &mut App) -> bool {
    let recent = cx
        .default_global::<Sound>()
        .last_chime
        .is_some_and(|at| at.elapsed() < CHIME_GAP);
    if recent {
        return false;
    }
    let Some(output) = output(cx) else {
        return false;
    };
    output.chime(kind);
    let sound = cx.default_global::<Sound>();
    sound.last_chime = Some(Instant::now());
    sound.chimes += 1;
    true
}

/// Use a device that plays nothing aloud, for tests.
pub fn use_null(cx: &mut App) {
    let sound = cx.default_global::<Sound>();
    sound.output = Some(Rc::new(Output::null()));
    sound.failed_at = None;
}

/// How many chimes played, for tests.
pub fn chimes(cx: &App) -> u32 {
    cx.try_global::<Sound>().map_or(0, |s| s.chimes)
}

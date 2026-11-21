//! Playing sound and video in messages: the shared sound engine, the
//! transport bar the audio card and the video player share, the view that
//! draws a video's pictures, and "the one thing playing".
//!
//! Only one sound or video plays at a time. A player [`claim`]s when it
//! starts and watches [`NowPlaying`] (`cx.observe_global::<NowPlaying>`):
//! when another key holds it, the player stops itself. So the audio card and
//! the video player stop each other without knowing each other.

use gpui_kit::*;

pub mod controls;
pub mod sound;
pub mod video_view;

/// The key of what is playing now, if anything.
#[derive(Default)]
pub struct NowPlaying(pub Option<SharedString>);
impl Global for NowPlaying {}

/// `key` plays now; whatever played before stops itself.
pub fn claim(key: &str, cx: &mut App) {
    cx.set_global(NowPlaying(Some(SharedString::from(key.to_string()))));
    cx.refresh_windows();
}

/// `key` stopped. Nothing changes when another key holds it.
pub fn release(key: &str, cx: &mut App) {
    if holder(cx).as_deref() == Some(key) {
        cx.set_global(NowPlaying(None));
        cx.refresh_windows();
    }
}

/// What plays now.
pub fn holder(cx: &App) -> Option<SharedString> {
    cx.try_global::<NowPlaying>().and_then(|n| n.0.clone())
}

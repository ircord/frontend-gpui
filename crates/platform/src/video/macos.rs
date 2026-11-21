//! macOS: `AVPlayer` fetches, decodes and plays the sound; an
//! `AVPlayerItemVideoOutput` hands out the picture for the current time as
//! a BGRA pixel buffer of the size asked for.
use std::cell::Cell;
use std::time::Duration;

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{AnyThread, MainThreadMarker};
use objc2_av_foundation::{
    AVPlayer, AVPlayerItem, AVPlayerItemStatus, AVPlayerItemVideoOutput, AVPlayerTimeControlStatus,
};
use objc2_core_foundation::CFString;
use objc2_core_media::{CMTime, CMTimeFlags};
use objc2_core_video::{
    CVPixelBufferGetBaseAddress, CVPixelBufferGetBytesPerRow, CVPixelBufferGetHeight,
    CVPixelBufferGetWidth, CVPixelBufferLockBaseAddress, CVPixelBufferLockFlags,
    CVPixelBufferUnlockBaseAddress, kCVPixelBufferHeightKey, kCVPixelBufferPixelFormatTypeKey,
    kCVPixelBufferWidthKey, kCVPixelFormatType_32BGRA,
};
use objc2_foundation::{NSDictionary, NSNumber, NSString, NSURL};

use super::{Frame, State, Video};

/// Set to true once a person has watched video play on a Mac (see the plan).
pub const PROVEN: bool = false;

pub struct MacVideo {
    player: Retained<AVPlayer>,
    item: Retained<AVPlayerItem>,
    output: Retained<AVPlayerItemVideoOutput>,
    serial: Cell<u64>,
    last: std::cell::RefCell<Option<Frame>>,
}

fn key(k: &CFString) -> &NSString {
    // CFString and NSString are toll-free bridged.
    unsafe { &*(k as *const CFString as *const NSString) }
}

fn time(d: Duration) -> CMTime {
    CMTime {
        value: d.as_millis() as i64,
        timescale: 1000,
        flags: CMTimeFlags::Valid,
        epoch: 0,
    }
}

fn from_time(t: CMTime) -> Option<Duration> {
    (t.flags.contains(CMTimeFlags::Valid)
        && !t.flags.contains(CMTimeFlags::Indefinite)
        && t.timescale > 0
        && t.value >= 0)
        .then(|| Duration::from_secs_f64(t.value as f64 / t.timescale as f64))
}

pub fn open(url: &str, width: u32, height: u32) -> Result<MacVideo, String> {
    let mtm = MainThreadMarker::new().ok_or("video opens on the main thread")?;
    let url = NSURL::URLWithString(&NSString::from_str(url)).ok_or("not a URL")?;
    let attributes: Retained<NSDictionary<NSString, AnyObject>> = unsafe {
        NSDictionary::from_slices(
            &[
                key(kCVPixelBufferPixelFormatTypeKey),
                key(kCVPixelBufferWidthKey),
                key(kCVPixelBufferHeightKey),
            ],
            &[
                &*NSNumber::new_u32(kCVPixelFormatType_32BGRA) as &AnyObject,
                &*NSNumber::new_u32(width) as &AnyObject,
                &*NSNumber::new_u32(height) as &AnyObject,
            ],
        )
    };
    unsafe {
        let item = AVPlayerItem::playerItemWithURL(&url, mtm);
        let output = AVPlayerItemVideoOutput::initWithPixelBufferAttributes(
            AVPlayerItemVideoOutput::alloc(),
            Some(&attributes),
        );
        item.addOutput(&output);
        let player = AVPlayer::playerWithPlayerItem(Some(&item), mtm);
        Ok(MacVideo {
            player,
            item,
            output,
            serial: Cell::new(0),
            last: Default::default(),
        })
    }
}

impl Video for MacVideo {
    fn play(&self) {
        unsafe { self.player.play() }
    }
    fn pause(&self) {
        unsafe { self.player.pause() }
    }
    fn seek(&self, to: Duration) {
        unsafe { self.player.seekToTime(time(to)) }
    }
    fn set_volume(&self, volume: f32) {
        unsafe { self.player.setVolume(volume.clamp(0.0, 1.0)) }
    }
    fn set_muted(&self, muted: bool) {
        unsafe { self.player.setMuted(muted) }
    }
    fn state(&self) -> State {
        unsafe {
            if self.item.status() == AVPlayerItemStatus::Failed {
                return State::Failed;
            }
            if let (Some(at), Some(end)) = (
                from_time(self.player.currentTime()),
                from_time(self.item.duration()),
            ) && at + Duration::from_millis(50) >= end
            {
                return State::Ended;
            }
            if self.item.status() != AVPlayerItemStatus::ReadyToPlay {
                return State::Loading;
            }
            match self.player.timeControlStatus() {
                AVPlayerTimeControlStatus::Playing => State::Playing,
                AVPlayerTimeControlStatus::Paused => State::Paused,
                _ => State::Loading,
            }
        }
    }
    fn position(&self) -> Duration {
        from_time(unsafe { self.player.currentTime() }).unwrap_or_default()
    }
    fn duration(&self) -> Option<Duration> {
        from_time(unsafe { self.item.duration() }).filter(|d| !d.is_zero())
    }
    fn frame(&self, after: u64) -> Option<Frame> {
        unsafe {
            let now = self.player.currentTime();
            if self.output.hasNewPixelBufferForItemTime(now)
                && let Some(buffer) = self
                    .output
                    .copyPixelBufferForItemTime_itemTimeForDisplay(now, std::ptr::null_mut())
            {
                CVPixelBufferLockBaseAddress(&buffer, CVPixelBufferLockFlags::ReadOnly);
                let (w, h) = (
                    CVPixelBufferGetWidth(&buffer),
                    CVPixelBufferGetHeight(&buffer),
                );
                let stride = CVPixelBufferGetBytesPerRow(&buffer);
                let base = CVPixelBufferGetBaseAddress(&buffer) as *const u8;
                let mut bgra = Vec::with_capacity(w * h * 4);
                if !base.is_null() {
                    for y in 0..h {
                        bgra.extend_from_slice(std::slice::from_raw_parts(
                            base.add(y * stride),
                            w * 4,
                        ));
                    }
                }
                CVPixelBufferUnlockBaseAddress(&buffer, CVPixelBufferLockFlags::ReadOnly);
                if bgra.len() == w * h * 4 {
                    self.serial.set(self.serial.get() + 1);
                    *self.last.borrow_mut() = Some(Frame {
                        width: w as u32,
                        height: h as u32,
                        bgra,
                        serial: self.serial.get(),
                    });
                }
            }
        }
        self.last
            .borrow()
            .as_ref()
            .filter(|f| f.serial > after)
            .cloned()
    }
}

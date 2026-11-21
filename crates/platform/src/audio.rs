//! Sound out through `rodio`: the notification chime and audio files.
use std::io::Cursor;
use std::num::NonZero;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use rodio::buffer::SamplesBuffer;
use rodio::mixer::Mixer;
use rodio::{Decoder, DeviceSinkBuilder, MixerDeviceSink, Player, Source};

pub const RATE: u32 = 44_100;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Chime {
    Mention,
    Message,
}

/// The web client's chime as samples (`useAlertSound.ts`): 880 Hz then
/// 1320 Hz for a mention, 660 Hz for a message; each note rises to 0.18 in
/// 10 ms and decays to silence at 250 ms; notes start 120 ms apart.
pub fn chime_samples(kind: Chime) -> Vec<f32> {
    let notes: &[f32] = match kind {
        Chime::Mention => &[880.0, 1320.0],
        Chime::Message => &[660.0],
    };
    let rate = RATE as f32;
    let total = ((0.12 * (notes.len() - 1) as f32 + 0.3) * rate) as usize;
    let mut out = vec![0.0f32; total];
    for (i, hz) in notes.iter().enumerate() {
        let start = (0.12 * i as f32 * rate) as usize;
        for n in 0..(0.3 * rate) as usize {
            let t = n as f32 / rate;
            // exponentialRampToValueAtTime: v(t) = v0 * (v1/v0)^(t/T)
            let gain = if t < 0.01 {
                0.0001 * (0.18f32 / 0.0001).powf(t / 0.01)
            } else if t < 0.25 {
                0.18 * (0.0001f32 / 0.18).powf((t - 0.01) / 0.24)
            } else {
                0.0
            };
            out[start + n] += gain * (std::f32::consts::TAU * hz * t).sin();
        }
    }
    out
}

/// Where sound goes. Keep it alive for as long as sound should play.
pub struct Output {
    mixer: Mixer,
    /// The device; `None` for [`Output::null`].
    _sink: Option<MixerDeviceSink>,
    /// Stops the null device's thread.
    stop: Arc<AtomicBool>,
}

impl Output {
    /// The system's default output device.
    pub fn open() -> Result<Self, String> {
        let mut sink = DeviceSinkBuilder::open_default_sink().map_err(|e| e.to_string())?;
        sink.log_on_drop(false);
        Ok(Self {
            mixer: sink.mixer().clone(),
            _sink: Some(sink),
            stop: Arc::default(),
        })
    }

    /// No device: a thread takes the samples at the speed a sound card
    /// would and throws them away. For tests; a track plays, seeks and ends
    /// as it would aloud.
    pub fn null() -> Self {
        let (mixer, mut source) =
            rodio::mixer::mixer(NonZero::new(2).unwrap(), NonZero::new(RATE).unwrap());
        let stop: Arc<AtomicBool> = Arc::default();
        let stopped = stop.clone();
        std::thread::Builder::new()
            .name("null-sound".into())
            .spawn(move || {
                // 10 ms of stereo samples, then 10 ms of sleep.
                let block = RATE as usize * 2 / 100;
                while !stopped.load(Ordering::Relaxed) {
                    for _ in 0..block {
                        // `None` is silence: nothing is playing.
                        let _ = source.next();
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
            })
            .expect("a thread for the null sound device");
        Self {
            mixer,
            _sink: None,
            stop,
        }
    }

    pub fn mixer(&self) -> &Mixer {
        &self.mixer
    }

    pub fn chime(&self, kind: Chime) {
        let samples = chime_samples(kind);
        self.mixer().add(SamplesBuffer::new(
            NonZero::new(1).unwrap(),
            NonZero::new(RATE).unwrap(),
            samples,
        ));
    }

    /// Start a downloaded file. The player is paused until `play`.
    pub fn track(&self, bytes: Vec<u8>) -> Result<Track, String> {
        Track::new(Player::connect_new(self.mixer()), bytes)
    }
}

impl Drop for Output {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

pub struct Track {
    player: Player,
    duration: Option<Duration>,
}

impl Track {
    pub fn new(player: Player, bytes: Vec<u8>) -> Result<Self, String> {
        let len = bytes.len() as u64;
        let decoder = Decoder::builder()
            .with_data(Cursor::new(bytes))
            .with_byte_len(len)
            .with_seekable(true)
            .build()
            .map_err(|e| e.to_string())?;
        let duration = decoder.total_duration();
        player.pause();
        player.append(decoder);
        Ok(Self { player, duration })
    }
    pub fn play(&self) {
        self.player.play()
    }
    pub fn pause(&self) {
        self.player.pause()
    }
    pub fn paused(&self) -> bool {
        self.player.is_paused()
    }
    pub fn ended(&self) -> bool {
        self.player.empty()
    }
    pub fn position(&self) -> Duration {
        self.player.get_pos()
    }
    pub fn duration(&self) -> Option<Duration> {
        self.duration
    }
    pub fn seek(&self, to: Duration) -> Result<(), String> {
        self.player.try_seek(to).map_err(|e| e.to_string())
    }
    pub fn set_volume(&self, volume: f32) {
        self.player.set_volume(volume)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_chime_is_two_notes_for_a_mention_and_one_for_a_message() {
        let mention = chime_samples(Chime::Mention);
        let message = chime_samples(Chime::Message);
        assert_eq!(mention.len(), (0.42 * RATE as f32) as usize);
        assert_eq!(message.len(), (0.3 * RATE as f32) as usize);
        let peak = mention.iter().fold(0.0f32, |a, b| a.max(b.abs()));
        assert!(peak > 0.1 && peak <= 0.36, "peak {peak}");
        assert!(mention.last().unwrap().abs() < 0.001);
    }

    /// A WAV made here decodes, reports its length, seeks and plays to the
    /// end without a sound card: the player's output is pulled by hand.
    #[test]
    fn a_track_decodes_seeks_and_ends_without_a_device() {
        let wav = wav_of(&chime_samples(Chime::Mention));
        let (player, mut output) = Player::new();
        let track = Track::new(player, wav).unwrap();
        let d = track.duration().unwrap();
        assert!((d.as_secs_f32() - 0.42).abs() < 0.01, "{d:?}");
        assert!(track.paused());
        track.play();
        // The seek is carried out by whoever pulls the samples (the sound
        // card's thread in the app), so pull on another thread here.
        let pulled = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (count, halt) = (pulled.clone(), stop.clone());
        let puller = std::thread::spawn(move || {
            while !halt.load(std::sync::atomic::Ordering::Relaxed) {
                let _ = output.next();
                count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
        });
        track.seek(Duration::from_millis(200)).unwrap();
        let started = std::time::Instant::now();
        while !track.ended() && started.elapsed() < Duration::from_secs(5) {
            std::thread::yield_now();
        }
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        puller.join().unwrap();
        assert!(track.ended());
        assert!(track.position() >= Duration::from_millis(200));
    }

    #[test]
    fn the_null_device_plays_a_track_to_its_end() {
        let out = Output::null();
        out.chime(Chime::Message);
        let track = out.track(wav_of(&chime_samples(Chime::Message))).unwrap();
        track.play();
        let started = std::time::Instant::now();
        while !track.ended() && started.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(track.ended());
        let took = started.elapsed();
        assert!(
            took > Duration::from_millis(200) && took < Duration::from_secs(2),
            "{took:?}"
        );
    }

    fn wav_of(samples: &[f32]) -> Vec<u8> {
        let data: Vec<u8> = samples
            .iter()
            .flat_map(|s| ((s * 32767.0) as i16).to_le_bytes())
            .collect();
        let mut out = Vec::new();
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
        out.extend_from_slice(b"WAVEfmt ");
        out.extend_from_slice(&16u32.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&RATE.to_le_bytes());
        out.extend_from_slice(&(RATE * 2).to_le_bytes());
        out.extend_from_slice(&2u16.to_le_bytes());
        out.extend_from_slice(&16u16.to_le_bytes());
        out.extend_from_slice(b"data");
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&data);
        out
    }
}

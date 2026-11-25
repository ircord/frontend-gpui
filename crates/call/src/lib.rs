//! Voice calls on the IRCord server's LiveKit: [`LiveKit`] is the
//! [`Connector`] the app uses outside tests.
//!
//! Sound goes through WebRTC's own audio device module ([`PlatformAudio`]):
//! it captures the microphone, plays the room to the speakers, and cancels
//! the speakers' echo from the microphone (with noise suppression and
//! automatic gain), all inside libwebrtc. It is opened when a call starts and
//! released when it ends, so nothing holds the sound devices between calls.
//!
//! A call joins with the microphone closed. Opening it publishes a microphone
//! track at the room's bitrate; closing it unpublishes the track and stops
//! recording, so the system's microphone indicator goes off, as the web
//! client does (`stopMicTrackOnMute`).
//!
//! The server gives a signalling address under its own host
//! (`wss://<host>/voice`). LiveKit's library sends the token as an
//! `Authorization: Bearer` header there, which the IRCord backend does not
//! read; it reads the `access_token` query parameter, as the web client
//! sends it. So the token goes into the address's query as well
//! ([`signal_url`]), and the library keeps that query when it adds its own
//! path and parameters. The query keeps the first token for the whole call:
//! a reconnect after that token has expired (30 seconds) is refused, and
//! the call ends, until the backend prefers the bearer header, which carries
//! the refreshed token.

use std::collections::HashSet;

use ircord_client::voice::{
    AudioSettings, Call, Connect, Connector, Device, Devices, Events, MediaEvent, Participant,
};
use livekit::options::{AudioEncoding, TrackPublishOptions};
use livekit::prelude::*;
use tokio::sync::mpsc;

/// The media layer on LiveKit. Its work runs on `runtime`.
pub struct LiveKit {
    runtime: tokio::runtime::Handle,
}

impl LiveKit {
    pub fn new(runtime: tokio::runtime::Handle) -> Self {
        Self { runtime }
    }
}

impl Connector for LiveKit {
    fn connect(&self, request: Connect, events: Events) -> Box<dyn Call> {
        let (commands, receiver) = mpsc::unbounded_channel();
        self.runtime.spawn(run(request, events, receiver));
        Box::new(LiveCall { commands })
    }

    fn devices(&self) -> Devices {
        match PlatformAudio::new() {
            Ok(audio) => Devices {
                microphones: audio
                    .recording_devices()
                    .map(|d| Device {
                        id: d.id.as_str().to_string(),
                        name: d.name,
                    })
                    .collect(),
                speakers: audio
                    .playout_devices()
                    .map(|d| Device {
                        id: d.id.as_str().to_string(),
                        name: d.name,
                    })
                    .collect(),
            },
            Err(e) => {
                log::warn!("voice: no sound devices: {e}");
                Devices::default()
            }
        }
    }
}

enum Command {
    Microphone(bool),
    Deafened(bool),
    Apply(AudioSettings),
    Leave,
}

struct LiveCall {
    commands: mpsc::UnboundedSender<Command>,
}

impl Call for LiveCall {
    fn set_microphone(&self, on: bool) {
        let _ = self.commands.send(Command::Microphone(on));
    }

    fn set_deafened(&self, deafened: bool) {
        let _ = self.commands.send(Command::Deafened(deafened));
    }

    fn apply(&self, settings: &AudioSettings) {
        let _ = self.commands.send(Command::Apply(settings.clone()));
    }

    fn leave(&self) {
        let _ = self.commands.send(Command::Leave);
    }
}

impl Drop for LiveCall {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Leave);
    }
}

/// One call, from connecting to leaving.
async fn run(request: Connect, events: Events, mut commands: mpsc::UnboundedReceiver<Command>) {
    // The device module must exist before connecting, or the room plays to
    // a silent one.
    let audio = match PlatformAudio::new() {
        Ok(audio) => {
            configure(&audio, &request.settings, &AudioSettings::default());
            Some(audio)
        }
        Err(e) => {
            log::warn!("voice: no sound devices: {e}");
            let _ = events.send(MediaEvent::AudioUnavailable);
            None
        }
    };
    let mut options = RoomOptions::default();
    options.auto_subscribe = true;
    let mut settings = request.settings.clone();
    let url = match signal_url(&request.url, &request.token) {
        Ok(url) => url,
        Err(e) => {
            log::warn!("voice: the server's signalling address is not usable: {e}");
            let _ = events.send(MediaEvent::Failed(t_connect_failed()));
            return;
        }
    };
    let connecting = Room::connect(&url, &request.token, options);
    let (room, mut room_events) = tokio::select! {
        connected = connecting => match connected {
            Ok(connected) => connected,
            Err(e) => {
                // The library's errors can carry the address and its token.
                log::warn!("voice: could not connect: {}", redact(&e.to_string(), &request.token));
                let _ = events.send(MediaEvent::Failed(t_connect_failed()));
                return;
            }
        },
        // Left while connecting.
        _ = until_leave(&mut commands, audio.as_ref(), &mut settings) => return,
    };
    let _ = events.send(MediaEvent::Connected(participants(&room)));
    let mut call = Live {
        room,
        audio,
        settings,
        bitrate_kbps: request.bitrate_kbps,
        microphone: None,
        recording_stopped: false,
        deafened: false,
    };
    loop {
        tokio::select! {
            command = commands.recv() => match command {
                None | Some(Command::Leave) => break,
                Some(Command::Microphone(on)) => {
                    let event = match call.set_microphone(on).await {
                        Ok(()) => MediaEvent::Microphone(on),
                        Err(e) => {
                            log::warn!("voice: microphone: {e}");
                            call.close_microphone().await;
                            MediaEvent::MicrophoneFailed
                        }
                    };
                    let _ = events.send(event);
                }
                Some(Command::Deafened(deafened)) => call.set_deafened(deafened),
                Some(Command::Apply(settings)) => {
                    if let Some(audio) = &call.audio {
                        configure(audio, &settings, &call.settings);
                        // Switching the microphone restarts recording where
                        // it was still initialised; a closed microphone stays
                        // closed.
                        if call.microphone.is_none() && audio.is_recording_initialized() {
                            let _ = audio.stop_recording();
                            call.recording_stopped = true;
                        }
                    }
                    call.settings = settings;
                }
            },
            event = room_events.recv() => {
                let Some(event) = event else {
                    let _ = events.send(MediaEvent::Ended);
                    break;
                };
                match event {
                    RoomEvent::ParticipantConnected(_) | RoomEvent::ParticipantDisconnected(_) => {
                        let _ = events.send(MediaEvent::Participants(participants(&call.room)));
                    }
                    RoomEvent::ActiveSpeakersChanged { speakers } => {
                        let speakers = speakers.iter().map(|p| p.identity().to_string()).collect();
                        let _ = events.send(MediaEvent::Speakers(speakers));
                    }
                    RoomEvent::TrackSubscribed { publication, .. } if call.deafened => {
                        publication.set_enabled(false);
                    }
                    RoomEvent::Reconnecting => {
                        let _ = events.send(MediaEvent::Reconnecting);
                    }
                    RoomEvent::Reconnected => {
                        let _ = events.send(MediaEvent::Reconnected);
                        let _ = events.send(MediaEvent::Participants(participants(&call.room)));
                    }
                    RoomEvent::Disconnected { reason } => {
                        log::info!("voice: disconnected: {reason:?}");
                        let _ = events.send(MediaEvent::Ended);
                        break;
                    }
                    _ => {}
                }
            }
        }
    }
    call.close_microphone().await;
    let _ = call.room.close().await;
}

/// Wait for the call to be left (or dropped) while it connects. Settings
/// changed meanwhile apply at once; the microphone and deafness wait for the
/// connection (the call starts muted and hearing).
async fn until_leave(
    commands: &mut mpsc::UnboundedReceiver<Command>,
    audio: Option<&PlatformAudio>,
    settings: &mut AudioSettings,
) {
    while let Some(command) = commands.recv().await {
        match command {
            Command::Leave => return,
            Command::Apply(next) => {
                if let Some(audio) = audio {
                    configure(audio, &next, settings);
                }
                *settings = next;
            }
            Command::Microphone(_) | Command::Deafened(_) => {}
        }
    }
}

struct Live {
    room: Room,
    audio: Option<PlatformAudio>,
    settings: AudioSettings,
    bitrate_kbps: i64,
    microphone: Option<LocalTrackPublication>,
    recording_stopped: bool,
    deafened: bool,
}

impl Live {
    async fn set_microphone(&mut self, on: bool) -> Result<(), String> {
        if !on {
            self.close_microphone().await;
            return Ok(());
        }
        if self.microphone.is_some() {
            return Ok(());
        }
        let audio = self.audio.as_ref().ok_or("no sound devices")?;
        if self.recording_stopped {
            audio.start_recording().map_err(|e| e.to_string())?;
            self.recording_stopped = false;
        }
        let track = LocalAudioTrack::create_audio_track("microphone", audio.rtc_source());
        let options = TrackPublishOptions {
            source: TrackSource::Microphone,
            audio_encoding: Some(AudioEncoding {
                max_bitrate: self.bitrate_kbps.clamp(8, 512) as u64 * 1000,
            }),
            ..TrackPublishOptions::default()
        };
        let publication = self
            .room
            .local_participant()
            .publish_track(LocalTrack::Audio(track), options)
            .await
            .map_err(|e| e.to_string())?;
        self.microphone = Some(publication);
        Ok(())
    }

    async fn close_microphone(&mut self) {
        if let Some(publication) = self.microphone.take() {
            let _ = self
                .room
                .local_participant()
                .unpublish_track(&publication.sid())
                .await;
        }
        if let Some(audio) = &self.audio
            && !self.recording_stopped
            && audio.is_recording_initialized()
        {
            let _ = audio.stop_recording();
            self.recording_stopped = true;
        }
    }

    fn set_deafened(&mut self, deafened: bool) {
        self.deafened = deafened;
        for participant in self.room.remote_participants().values() {
            for publication in participant.track_publications().values() {
                if publication.kind() == TrackKind::Audio {
                    publication.set_enabled(!deafened);
                }
            }
        }
    }
}

/// Apply `settings`, changing only what differs from `before`.
fn configure(audio: &PlatformAudio, settings: &AudioSettings, before: &AudioSettings) {
    let processing = AudioProcessingOptions {
        echo_cancellation: settings.echo_cancellation,
        noise_suppression: settings.noise_suppression,
        auto_gain_control: settings.auto_gain,
        prefer_hardware_processing: false,
    };
    if let Err(e) = audio.configure_audio_processing(processing) {
        log::warn!("voice: audio processing: {e}");
    }
    if settings.microphone != before.microphone {
        // Back to the default is the first device, which is the system's
        // default where the system has one.
        let switched = match &settings.microphone {
            Some(id) if audio.recording_devices().any(|d| d.id.as_str() == id) => {
                audio.switch_recording_device(&RecordingDeviceId::from_unchecked_guid(id))
            }
            _ => audio.switch_recording_device_by_index(0),
        };
        if let Err(e) = switched {
            log::warn!("voice: microphone: {e}");
        }
    }
    if settings.speaker != before.speaker {
        let switched = match &settings.speaker {
            Some(id) if audio.playout_devices().any(|d| d.id.as_str() == id) => {
                audio.switch_playout_device(&PlayoutDeviceId::from_unchecked_guid(id))
            }
            _ => audio.switch_playout_device_by_index(0),
        };
        if let Err(e) = switched {
            log::warn!("voice: speaker: {e}");
        }
    }
}

/// Everyone in the room, you included; the server names each by account.
fn participants(room: &Room) -> Vec<Participant> {
    let local = room.local_participant();
    let mut seen = HashSet::new();
    let mut people = vec![Participant {
        identity: local.identity().to_string(),
        name: local.name(),
        local: true,
    }];
    seen.insert(local.identity().to_string());
    for (identity, participant) in room.remote_participants() {
        if seen.insert(identity.to_string()) {
            people.push(Participant {
                identity: identity.to_string(),
                name: participant.name(),
                local: false,
            });
        }
    }
    people
}

/// The reason given when the media connection cannot be made; the app shows
/// its own words, so this is for the log.
fn t_connect_failed() -> String {
    "could not connect to the media server".into()
}

/// The signalling address `url` with `token` as its `access_token`, the
/// form the IRCord backend reads.
fn signal_url(url: &str, token: &str) -> Result<String, url::ParseError> {
    let mut url = url::Url::parse(url)?;
    url.query_pairs_mut().append_pair("access_token", token);
    Ok(url.into())
}

/// `text` without `token`, for the log.
fn redact(text: &str, token: &str) -> String {
    if token.is_empty() {
        text.to_string()
    } else {
        text.replace(token, "…")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_token_rides_in_the_signalling_query() {
        assert_eq!(
            signal_url("wss://chat.example/voice", "a.b+c").unwrap(),
            "wss://chat.example/voice?access_token=a.b%2Bc"
        );
        assert!(signal_url("not an address", "t").is_err());
    }

    #[test]
    fn tokens_stay_out_of_the_log() {
        assert_eq!(
            redact("wss://h/voice/rtc?access_token=abc.def failed", "abc.def"),
            "wss://h/voice/rtc?access_token=… failed"
        );
        assert_eq!(redact("plain", ""), "plain");
    }
}

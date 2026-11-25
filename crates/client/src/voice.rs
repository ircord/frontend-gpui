//! Voice rooms and the call in one, without the media itself.
//!
//! The rooms are server-wide and listed by [`crate::api`] (`voice_rooms`);
//! `voice.updated` on the socket ([`crate::store::topic::VOICE`]) says the
//! list changed. Joining reserves a place over HTTP and answers a credential
//! for the server's media connection, which a [`Connector`] opens: LiveKit in
//! `ircord-call`, a fake in tests. The app holds one [`Voice`] per session and
//! drives it with what the HTTP calls and the media connection report; every
//! rule about what may happen when lives here, as the web client's voice
//! store (`app/stores/voice.ts`) has them:
//!
//! - A call starts with the microphone closed, so joining never puts anyone
//!   on air. Deafening closes the microphone too.
//! - One call at a time; a full or unavailable room cannot be joined.
//! - A room that leaves the list ends its call.
//! - An answer that arrives after the call it was for has ended gives its
//!   reservation back instead of connecting.

use ircord_contract::{Credential, Room};
use serde::{Deserialize, Serialize};

/// The section of the device's state that holds [`AudioSettings`].
pub const SECTION: &str = "voice";

/// The sound devices and processing a call uses, kept on this device.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AudioSettings {
    /// A microphone's ID from [`Devices`]; `None` is the system's default.
    pub microphone: Option<String>,
    /// A speaker's ID from [`Devices`]; `None` is the system's default.
    pub speaker: Option<String>,
    /// Remove the room's own voices, coming out of the speakers, from the
    /// microphone.
    pub echo_cancellation: bool,
    pub noise_suppression: bool,
    /// Keep your voice at a steady level.
    pub auto_gain: bool,
}

impl Default for AudioSettings {
    fn default() -> Self {
        Self {
            microphone: None,
            speaker: None,
            echo_cancellation: true,
            noise_suppression: true,
            auto_gain: true,
        }
    }
}

/// A microphone or speaker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    pub id: String,
    pub name: String,
}

/// The sound devices the system offers.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Devices {
    pub microphones: Vec<Device>,
    pub speakers: Vec<Device>,
}

/// Someone in the call, as the media server reports them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Participant {
    /// The media server's ID for this connection.
    pub identity: String,
    /// The IRCord account name.
    pub name: String,
    /// Whether this is you.
    pub local: bool,
}

/// What the media connection reports.
#[derive(Debug, Clone, PartialEq)]
pub enum MediaEvent {
    /// In the room, with these people (you included).
    Connected(Vec<Participant>),
    /// Someone came or went.
    Participants(Vec<Participant>),
    /// Who is speaking now, by identity.
    Speakers(Vec<String>),
    /// The microphone is now open (`true`) or closed.
    Microphone(bool),
    /// The microphone could not be opened or closed; it is closed.
    MicrophoneFailed,
    /// In the call, but no sound device could be opened: you neither hear
    /// nor are heard.
    AudioUnavailable,
    Reconnecting,
    Reconnected,
    /// Could not connect.
    Failed(String),
    /// The server or the network ended the call.
    Ended,
}

/// Where [`MediaEvent`]s go.
pub type Events = tokio::sync::mpsc::UnboundedSender<MediaEvent>;

/// What opening the media connection needs.
#[derive(Debug, Clone, PartialEq)]
pub struct Connect {
    /// The signalling address the server gave (`wss://host/voice`).
    pub url: String,
    pub token: String,
    /// The room's audio bitrate, for the microphone.
    pub bitrate_kbps: i64,
    pub settings: AudioSettings,
}

/// The media layer.
pub trait Connector: Send + Sync {
    /// Start connecting. Events arrive on `events` until [`MediaEvent::Ended`]
    /// or [`MediaEvent::Failed`], or the call is left.
    fn connect(&self, request: Connect, events: Events) -> Box<dyn Call>;
    /// The microphones and speakers there are.
    fn devices(&self) -> Devices;
}

/// A media connection, from [`Connector::connect`].
pub trait Call: Send {
    /// Open or close the microphone; answered with [`MediaEvent::Microphone`]
    /// or [`MediaEvent::MicrophoneFailed`].
    fn set_microphone(&self, on: bool);
    /// Stop or resume hearing the room.
    fn set_deafened(&self, deafened: bool);
    /// Use other devices or processing from now on.
    fn apply(&self, settings: &AudioSettings);
    /// Close the connection. Nothing more is reported.
    fn leave(&self);
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CallState {
    #[default]
    Idle,
    Joining,
    Connected,
    Reconnecting,
}

/// Why a room cannot be joined now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Block {
    /// The server cannot reach its media server for this room.
    Unavailable,
    Full,
    /// You are in another call.
    OtherCall,
}

/// What went wrong, for the person to read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VoiceError {
    /// The server refused the join, in its own words.
    Join(String),
    ConnectFailed,
    /// The call ended without you leaving.
    ConnectionEnded,
    MicrophoneFailed,
    AudioUnavailable,
}

/// The call is over: close the media connection and give back the
/// reservation, if one was made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hangup {
    pub credential: Option<String>,
}

/// What to do with the answer to a join.
#[derive(Debug, Clone, PartialEq)]
pub enum Joined {
    /// Open the media connection with this credential.
    Connect(Credential),
    /// The call this was for is over: give the reservation back.
    Release(String),
    /// The server refused; [`Voice::error`] says why.
    Refused,
}

/// Turning deafness on or off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Deafen {
    pub deafened: bool,
    /// Close the microphone as well.
    pub close_microphone: bool,
}

/// The rooms and your call.
#[derive(Debug, Clone)]
pub struct Voice {
    pub rooms: Vec<Room>,
    pub state: CallState,
    /// The room you are joining or in.
    pub active: Option<Room>,
    pub muted: bool,
    pub deafened: bool,
    /// A microphone change is under way.
    pub changing_microphone: bool,
    /// The people in your call (you first), once connected.
    pub participants: Vec<Participant>,
    /// Who in your call is speaking, by identity.
    pub speakers: Vec<String>,
    pub error: Option<VoiceError>,
    credential: Option<String>,
    /// Counts joins, so an answer for an earlier one is recognised.
    attempt: u64,
}

impl Default for Voice {
    fn default() -> Self {
        Self {
            rooms: Vec::new(),
            state: CallState::Idle,
            active: None,
            muted: true,
            deafened: false,
            changing_microphone: false,
            participants: Vec::new(),
            speakers: Vec::new(),
            error: None,
            credential: None,
            attempt: 0,
        }
    }
}

impl Voice {
    /// The current join, to hand back with its answers.
    pub fn attempt(&self) -> u64 {
        self.attempt
    }

    pub fn is_active(&self, room: &str) -> bool {
        self.active.as_ref().is_some_and(|r| r.id == room)
    }

    /// Why `room` cannot be joined now, if it cannot.
    pub fn block(&self, room: &Room) -> Option<Block> {
        if !room.available {
            Some(Block::Unavailable)
        } else if room.participants.len() as i64 >= room.capacity {
            Some(Block::Full)
        } else if self.state != CallState::Idle {
            Some(Block::OtherCall)
        } else {
            None
        }
    }

    /// A newer room list. Ends the call when its room is gone.
    pub fn set_rooms(&mut self, rooms: Vec<Room>) -> Option<Hangup> {
        self.rooms = rooms;
        let active = self.active.as_ref()?;
        match self.rooms.iter().find(|r| r.id == active.id) {
            Some(room) => {
                self.active = Some(room.clone());
                None
            }
            None => Some(self.end(Some(VoiceError::ConnectionEnded))),
        }
    }

    /// Start joining `room`: `Some(attempt)` to join with, or `None` when it
    /// cannot be joined now.
    pub fn join(&mut self, room: &Room) -> Option<u64> {
        if self.block(room).is_some() {
            return None;
        }
        self.reset();
        self.attempt += 1;
        self.state = CallState::Joining;
        self.active = Some(room.clone());
        Some(self.attempt)
    }

    /// The server answered join `attempt`.
    pub fn joined(
        &mut self,
        attempt: u64,
        answer: Result<Credential, crate::api::ApiError>,
    ) -> Joined {
        let current = attempt == self.attempt && self.state == CallState::Joining;
        match answer {
            Ok(credential) if current => {
                self.credential = Some(credential.id.clone());
                Joined::Connect(credential)
            }
            Ok(credential) => Joined::Release(credential.id),
            Err(_) if !current => Joined::Refused,
            Err(e) => {
                self.end(Some(VoiceError::Join(e.to_string())));
                Joined::Refused
            }
        }
    }

    /// The media connection of join `attempt` reported `event`. `Some` when
    /// that ends the call.
    pub fn media(&mut self, attempt: u64, event: MediaEvent) -> Option<Hangup> {
        if attempt != self.attempt || self.state == CallState::Idle {
            return None;
        }
        match event {
            MediaEvent::Connected(people) => {
                self.state = CallState::Connected;
                self.participants = ordered(people);
            }
            MediaEvent::Participants(people) => self.participants = ordered(people),
            MediaEvent::Speakers(speakers) => self.speakers = speakers,
            MediaEvent::Microphone(on) => {
                self.muted = !on;
                self.changing_microphone = false;
                if self.error == Some(VoiceError::MicrophoneFailed) {
                    self.error = None;
                }
            }
            MediaEvent::MicrophoneFailed => {
                self.muted = true;
                self.changing_microphone = false;
                self.error = Some(VoiceError::MicrophoneFailed);
            }
            MediaEvent::AudioUnavailable => self.error = Some(VoiceError::AudioUnavailable),
            MediaEvent::Reconnecting => self.state = CallState::Reconnecting,
            MediaEvent::Reconnected => self.state = CallState::Connected,
            MediaEvent::Failed(_) => return Some(self.end(Some(VoiceError::ConnectFailed))),
            MediaEvent::Ended => {
                let error = if self.state == CallState::Joining {
                    VoiceError::ConnectFailed
                } else {
                    VoiceError::ConnectionEnded
                };
                return Some(self.end(Some(error)));
            }
        }
        None
    }

    /// Leave the call. `None` when there is none.
    pub fn leave(&mut self) -> Option<Hangup> {
        (self.state != CallState::Idle).then(|| self.end(None))
    }

    /// Open the microphone if it is closed, or close it: `Some(open)` to ask
    /// the call for, or `None` when it cannot change now.
    pub fn toggle_mute(&mut self) -> Option<bool> {
        if self.state != CallState::Connected || self.deafened || self.changing_microphone {
            return None;
        }
        self.changing_microphone = true;
        Some(self.muted)
    }

    /// Deafen or undeafen; deafening closes an open microphone.
    pub fn toggle_deafen(&mut self) -> Option<Deafen> {
        if self.state != CallState::Connected || self.changing_microphone {
            return None;
        }
        self.deafened = !self.deafened;
        let close_microphone = self.deafened && !self.muted;
        if close_microphone {
            self.changing_microphone = true;
        }
        Some(Deafen {
            deafened: self.deafened,
            close_microphone,
        })
    }

    /// Whether `identity` is speaking.
    pub fn speaking(&self, identity: &str) -> bool {
        self.speakers.iter().any(|s| s == identity)
    }

    fn end(&mut self, error: Option<VoiceError>) -> Hangup {
        let hangup = Hangup {
            credential: self.credential.take(),
        };
        self.reset();
        self.error = error;
        hangup
    }

    fn reset(&mut self) {
        let rooms = std::mem::take(&mut self.rooms);
        let attempt = self.attempt;
        *self = Voice {
            rooms,
            attempt,
            ..Voice::default()
        };
    }
}

/// You first, then everyone else by name.
fn ordered(mut people: Vec<Participant>) -> Vec<Participant> {
    people.sort_by(|a, b| {
        b.local
            .cmp(&a.local)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.identity.cmp(&b.identity))
    });
    people
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::ApiError;

    fn room(id: &str, people: &[&str]) -> Room {
        Room {
            id: id.into(),
            name: format!("Room {id}"),
            bitrate_kbps: 48,
            participants: people.iter().map(|p| p.to_string()).collect(),
            capacity: 2,
            available: true,
        }
    }

    fn credential(id: &str) -> Credential {
        Credential {
            id: id.into(),
            url: "wss://chat.example/voice".into(),
            token: "jwt".into(),
            ..Default::default()
        }
    }

    fn person(identity: &str, name: &str, local: bool) -> Participant {
        Participant {
            identity: identity.into(),
            name: name.into(),
            local,
        }
    }

    /// In room `r1`, connected, with Bob.
    fn connected() -> (Voice, u64) {
        let mut v = Voice::default();
        v.set_rooms(vec![room("r1", &[]), room("r2", &[])]);
        let attempt = v.join(&room("r1", &[])).unwrap();
        assert_eq!(
            v.joined(attempt, Ok(credential("c1"))),
            Joined::Connect(credential("c1"))
        );
        v.media(
            attempt,
            MediaEvent::Connected(vec![person("b", "bob", false), person("m", "me", true)]),
        );
        (v, attempt)
    }

    #[test]
    fn a_call_starts_silent_and_lists_you_first() {
        let (v, _) = connected();
        assert_eq!(v.state, CallState::Connected);
        assert!(v.muted, "joining never opens the microphone");
        assert_eq!(v.participants[0].name, "me");
        assert_eq!(v.participants[1].name, "bob");
    }

    #[test]
    fn full_unavailable_and_second_rooms_cannot_be_joined() {
        let v = Voice::default();
        assert_eq!(v.block(&room("r1", &["a", "b"])), Some(Block::Full));
        let mut down = room("r1", &[]);
        down.available = false;
        assert_eq!(v.block(&down), Some(Block::Unavailable));
        let (mut v, _) = connected();
        assert_eq!(v.block(&room("r2", &[])), Some(Block::OtherCall));
        assert_eq!(v.join(&room("r2", &[])), None);
        assert!(v.is_active("r1"));
    }

    #[test]
    fn a_refused_join_says_why_and_returns_to_idle() {
        let mut v = Voice::default();
        let attempt = v.join(&room("r1", &[])).unwrap();
        let refused = v.joined(
            attempt,
            Err(ApiError::Http {
                status: 409,
                detail: "This voice channel is full.".into(),
            }),
        );
        assert_eq!(refused, Joined::Refused);
        assert_eq!(v.state, CallState::Idle);
        assert!(v.active.is_none());
        assert_eq!(
            v.error,
            Some(VoiceError::Join("This voice channel is full.".into()))
        );
    }

    #[test]
    fn an_answer_after_leaving_gives_the_reservation_back() {
        let mut v = Voice::default();
        let attempt = v.join(&room("r1", &[])).unwrap();
        assert_eq!(v.leave(), Some(Hangup { credential: None }));
        assert_eq!(
            v.joined(attempt, Ok(credential("late"))),
            Joined::Release("late".into())
        );
        assert_eq!(v.state, CallState::Idle);
        // Media events of the old attempt are ignored too.
        assert_eq!(v.media(attempt, MediaEvent::Ended), None);
        assert_eq!(v.error, None);
    }

    #[test]
    fn leaving_hands_back_the_credential() {
        let (mut v, _) = connected();
        assert_eq!(
            v.leave(),
            Some(Hangup {
                credential: Some("c1".into())
            })
        );
        assert_eq!(v.state, CallState::Idle);
        assert!(v.participants.is_empty());
        assert_eq!(v.rooms.len(), 2, "the room list stays");
        assert_eq!(v.leave(), None);
    }

    #[test]
    fn mute_waits_for_the_microphone() {
        let (mut v, attempt) = connected();
        assert_eq!(v.toggle_mute(), Some(true));
        assert_eq!(v.toggle_mute(), None, "one change at a time");
        v.media(attempt, MediaEvent::Microphone(true));
        assert!(!v.muted);
        assert_eq!(v.toggle_mute(), Some(false));
        v.media(attempt, MediaEvent::MicrophoneFailed);
        assert!(v.muted);
        assert_eq!(v.error, Some(VoiceError::MicrophoneFailed));
        assert_eq!(v.toggle_mute(), Some(true));
        v.media(attempt, MediaEvent::Microphone(true));
        assert_eq!(v.error, None);
    }

    #[test]
    fn deafening_closes_the_microphone_and_blocks_unmuting() {
        let (mut v, attempt) = connected();
        v.toggle_mute();
        v.media(attempt, MediaEvent::Microphone(true));
        assert_eq!(
            v.toggle_deafen(),
            Some(Deafen {
                deafened: true,
                close_microphone: true
            })
        );
        v.media(attempt, MediaEvent::Microphone(false));
        assert!(v.muted);
        assert_eq!(v.toggle_mute(), None, "a deafened call stays muted");
        assert_eq!(
            v.toggle_deafen(),
            Some(Deafen {
                deafened: false,
                close_microphone: false
            })
        );
        assert_eq!(v.toggle_mute(), Some(true));
    }

    #[test]
    fn the_connection_ending_ends_the_call() {
        let (mut v, attempt) = connected();
        v.media(attempt, MediaEvent::Reconnecting);
        assert_eq!(v.state, CallState::Reconnecting);
        assert_eq!(v.toggle_mute(), None);
        v.media(attempt, MediaEvent::Reconnected);
        assert_eq!(v.state, CallState::Connected);
        assert_eq!(
            v.media(attempt, MediaEvent::Ended),
            Some(Hangup {
                credential: Some("c1".into())
            })
        );
        assert_eq!(v.error, Some(VoiceError::ConnectionEnded));
        assert_eq!(v.state, CallState::Idle);
    }

    #[test]
    fn failing_to_connect_says_so() {
        let mut v = Voice::default();
        let attempt = v.join(&room("r1", &[])).unwrap();
        v.joined(attempt, Ok(credential("c1")));
        let hangup = v.media(attempt, MediaEvent::Failed("refused".into()));
        assert_eq!(hangup.unwrap().credential.as_deref(), Some("c1"));
        assert_eq!(v.error, Some(VoiceError::ConnectFailed));
    }

    #[test]
    fn a_deleted_room_ends_its_call_and_a_renamed_one_follows() {
        let (mut v, _) = connected();
        let mut renamed = room("r1", &["me"]);
        renamed.name = "Lounge".into();
        assert_eq!(v.set_rooms(vec![renamed]), None);
        assert_eq!(v.active.as_ref().unwrap().name, "Lounge");
        let hangup = v.set_rooms(vec![room("r2", &[])]);
        assert_eq!(hangup.unwrap().credential.as_deref(), Some("c1"));
        assert_eq!(v.error, Some(VoiceError::ConnectionEnded));
    }

    #[test]
    fn audio_settings_default_to_processing_on_and_read_partial_files() {
        let s = AudioSettings::default();
        assert!(s.echo_cancellation && s.noise_suppression && s.auto_gain);
        let read: AudioSettings =
            serde_json::from_value(serde_json::json!({"microphone": "usb-1"})).unwrap();
        assert_eq!(read.microphone.as_deref(), Some("usb-1"));
        assert!(read.echo_cancellation);
    }
}

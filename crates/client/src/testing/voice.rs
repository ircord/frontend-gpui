//! A media layer for tests: [`FakeConnector`] records what the app asks of
//! a call and lets the test say what the media server answers.

use std::sync::{Arc, Mutex};

use crate::voice::{AudioSettings, Call, Connect, Connector, Devices, Events, MediaEvent};

/// What the app asked of the fake.
#[derive(Debug, Clone, PartialEq)]
pub enum Asked {
    Connect(Connect),
    Microphone(bool),
    Deafened(bool),
    Apply(AudioSettings),
    Leave,
}

#[derive(Default)]
struct Shared {
    asked: Vec<Asked>,
    events: Option<Events>,
    devices: Devices,
    /// Answer every connect with `Connected` and every microphone change
    /// with its result, as a working media server would.
    automatic: bool,
    people: Vec<crate::voice::Participant>,
}

/// A [`Connector`] that never leaves the process.
#[derive(Clone, Default)]
pub struct FakeConnector(Arc<Mutex<Shared>>);

impl FakeConnector {
    /// A fake that answers on its own: connects at once with `people` (you
    /// included) and opens and closes the microphone as asked.
    pub fn automatic(people: Vec<crate::voice::Participant>) -> Self {
        let fake = Self::default();
        {
            let mut shared = fake.0.lock().unwrap();
            shared.automatic = true;
            shared.people = people;
        }
        fake
    }

    pub fn with_devices(self, devices: Devices) -> Self {
        self.0.lock().unwrap().devices = devices;
        self
    }

    /// Everything asked so far.
    pub fn asked(&self) -> Vec<Asked> {
        self.0.lock().unwrap().asked.clone()
    }

    /// Report `event` on the open call, as the media server would.
    pub fn report(&self, event: MediaEvent) {
        if let Some(events) = &self.0.lock().unwrap().events {
            let _ = events.send(event);
        }
    }
}

impl Connector for FakeConnector {
    fn connect(&self, request: Connect, events: Events) -> Box<dyn Call> {
        let mut shared = self.0.lock().unwrap();
        shared.asked.push(Asked::Connect(request));
        if shared.automatic {
            let _ = events.send(MediaEvent::Connected(shared.people.clone()));
        }
        shared.events = Some(events);
        Box::new(FakeCall(self.0.clone()))
    }

    fn devices(&self) -> Devices {
        self.0.lock().unwrap().devices.clone()
    }
}

struct FakeCall(Arc<Mutex<Shared>>);

impl FakeCall {
    fn ask(&self, asked: Asked) {
        self.0.lock().unwrap().asked.push(asked);
    }
}

impl Call for FakeCall {
    fn set_microphone(&self, on: bool) {
        self.ask(Asked::Microphone(on));
        let shared = self.0.lock().unwrap();
        if shared.automatic
            && let Some(events) = &shared.events
        {
            let _ = events.send(MediaEvent::Microphone(on));
        }
    }

    fn set_deafened(&self, deafened: bool) {
        self.ask(Asked::Deafened(deafened));
    }

    fn apply(&self, settings: &AudioSettings) {
        self.ask(Asked::Apply(settings.clone()));
    }

    fn leave(&self) {
        let mut shared = self.0.lock().unwrap();
        shared.asked.push(Asked::Leave);
        shared.events = None;
    }
}

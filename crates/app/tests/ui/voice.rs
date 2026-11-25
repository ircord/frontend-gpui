//! Voice channels: the rooms in the sidebar, joining silent, the microphone,
//! deafening, leaving, the roster following `voice.updated`, and Settings →
//! Voice. The media server is `FakeConnector`; the call's rules are tested
//! without a window in `ircord_client::voice`.

use std::sync::Arc;
use std::time::Duration;

use gpui_kit::test::TestWindowExt;
use gpui_kit::{AppContext, TestAppContext, Window};
use ircord_app::features::voice;
use ircord_client::config::ConfigDir;
use ircord_client::testing::voice::{Asked, FakeConnector};
use ircord_client::testing::{CannedServer, Reply, ephemeral};
use ircord_client::voice::{AudioSettings, Device, Devices, MediaEvent, Participant, SECTION};
use serde_json::{Value, json};

use crate::fixture::{Fixture, bootstrap, open, open_with, snapshot, wait};

const WAIT: Duration = crate::fixture::HANG_GUARD;

fn room(id: &str, name: &str, people: &[&str], capacity: i64) -> Value {
    json!({"id": id, "name": name, "bitrateKbps": 64, "participants": people,
           "capacity": capacity, "available": true})
}

fn person(identity: &str, name: &str, local: bool) -> Participant {
    Participant {
        identity: identity.into(),
        name: name.into(),
        local,
    }
}

/// Signed in to a server that offers voice, with two rooms: Lounge (ann in
/// it) and Studio (full), and `fake` as the media server.
fn with_voice(cx: &mut TestAppContext, fake: &FakeConnector) -> Fixture {
    with_voice_and(cx, fake, |_| {})
}

fn with_voice_and(
    cx: &mut TestAppContext,
    fake: &FakeConnector,
    setup: impl FnOnce(&CannedServer),
) -> Fixture {
    let f = open_with(cx, true, |server| {
        let mut snapshot = snapshot();
        snapshot.capabilities.voice = Some(true);
        server.snapshot(&snapshot);
        server.route(
            "GET",
            "/api/v1/voice/rooms",
            200,
            json!([
                room("r1", "Lounge", &["ann"], 8),
                room("r2", "Studio", &["a", "b"], 2)
            ]),
        );
        server.route(
            "POST",
            "/api/v1/voice/rooms/r1/join",
            200,
            json!({"id": "cred1", "url": "wss://chat.example/voice", "token": "jwt"}),
        );
        server.route_reply("DELETE", "/api/v1/voice/sessions/cred1", Reply::empty());
        setup(server);
    });
    let connector: Arc<dyn ircord_client::voice::Connector> = Arc::new(fake.clone());
    cx.update(|cx| voice::use_connector(cx, connector));
    bootstrap(&f, cx);
    f
}

fn frame_do(
    f: &Fixture,
    cx: &mut TestAppContext,
    act: impl FnOnce(&mut Window, &mut gpui_kit::App),
) {
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        act(window, cx);
    })
    .unwrap();
    cx.run_until_parked();
}

fn click(f: &Fixture, cx: &mut TestAppContext, id: &'static str) {
    frame_do(f, cx, |window, cx| window.click(id, cx));
}

/// Wait until the element `id` is drawn with `label`.
fn labelled(f: &Fixture, cx: &mut TestAppContext, id: &'static str, label: &str) {
    let window = f.window;
    let label = label.to_string();
    wait(cx, &format!("{id} to read {label:?}"), move |cx| {
        window
            .update(cx, |_, window, cx| {
                window.render_frame(cx);
                window
                    .try_find(id)
                    .and_then(|e| e.label().map(|l| l.to_string()))
                    .is_some_and(|l| l == label)
            })
            .unwrap_or(false)
    });
}

fn drawn(f: &Fixture, cx: &mut TestAppContext, id: &'static str) -> bool {
    let mut found = false;
    frame_do(f, cx, |window, _| found = window.try_find(id).is_some());
    found
}

fn wait_drawn(f: &Fixture, cx: &mut TestAppContext, id: &'static str) {
    let window = f.window;
    wait(cx, &format!("{id} to be drawn"), move |cx| {
        window
            .update(cx, |_, window, cx| {
                window.render_frame(cx);
                window.try_find(id).is_some()
            })
            .unwrap_or(false)
    });
}

fn wait_asked(cx: &mut TestAppContext, fake: &FakeConnector, asked: Asked) {
    let fake = fake.clone();
    wait(cx, &format!("the call to be asked {asked:?}"), move |_| {
        fake.asked().contains(&asked)
    });
}

#[gpui_kit::test]
fn a_server_without_voice_shows_no_rooms_and_is_not_asked(cx: &mut TestAppContext) {
    let f = open(cx, true);
    bootstrap(&f, cx);
    assert!(!drawn(&f, cx, "voice-channels"));
    assert!(
        f.server
            .requests_to("GET", "/api/v1/voice/rooms")
            .is_empty()
    );
}

#[gpui_kit::test]
fn the_rooms_list_who_is_in_them_and_a_full_one_says_so(cx: &mut TestAppContext) {
    let fake = FakeConnector::default();
    let f = with_voice(cx, &fake);
    labelled(&f, cx, "voice-room-r1", "Join voice in Lounge");
    labelled(&f, cx, "voice-person-r1-0", "ann");
    // A full room does not join when clicked.
    click(&f, cx, "voice-room-r2");
    assert!(
        f.server
            .requests_to("POST", "/api/v1/voice/rooms/r2/join")
            .is_empty()
    );
    labelled(&f, cx, "voice-room-r2-tag", "Full");
}

#[gpui_kit::test]
fn joining_connects_silent_and_the_controls_mute_deafen_and_leave(cx: &mut TestAppContext) {
    let fake = FakeConnector::automatic(vec![
        person("i-ann", "ann", false),
        person("i-me", "me", true),
    ]);
    let f = with_voice(cx, &fake);
    wait_drawn(&f, cx, "voice-room-r1");
    click(&f, cx, "voice-room-r1");
    f.server
        .wait_for_nth("POST", "/api/v1/voice/rooms/r1/join", 1, WAIT);
    labelled(&f, cx, "voice-status", "Voice connected");
    let Some(Asked::Connect(request)) = fake.asked().first().cloned() else {
        panic!("connected first: {:?}", fake.asked())
    };
    assert_eq!(request.url, "wss://chat.example/voice");
    assert_eq!(request.token, "jwt");
    assert_eq!(request.bitrate_kbps, 64);
    assert!(
        !fake.asked().contains(&Asked::Microphone(true)),
        "joining never opens the microphone"
    );
    // Your call lists the media server's view, you first.
    labelled(&f, cx, "voice-room-r1", "Lounge, your call");
    labelled(&f, cx, "voice-person-r1-0", "me");
    labelled(&f, cx, "voice-person-r1-1", "ann");
    fake.report(MediaEvent::Speakers(vec!["i-ann".into()]));
    labelled(&f, cx, "voice-person-r1-1", "ann, speaking");

    labelled(&f, cx, "voice-mute", "Unmute microphone");
    click(&f, cx, "voice-mute");
    wait_asked(cx, &fake, Asked::Microphone(true));
    labelled(&f, cx, "voice-mute", "Mute microphone");

    // Deafening closes the microphone as well.
    click(&f, cx, "voice-deafen");
    wait_asked(cx, &fake, Asked::Deafened(true));
    wait_asked(cx, &fake, Asked::Microphone(false));
    labelled(&f, cx, "voice-status", "Deafened");
    labelled(&f, cx, "voice-mute", "Unmute microphone");

    click(&f, cx, "voice-leave");
    wait_asked(cx, &fake, Asked::Leave);
    f.server
        .wait_for_nth("DELETE", "/api/v1/voice/sessions/cred1", 1, WAIT);
    let window = f.window;
    wait(cx, "the controls to go", move |cx| {
        window
            .update(cx, |_, window, cx| {
                window.render_frame(cx);
                window.try_find("voice-call").is_none()
            })
            .unwrap_or(false)
    });
    labelled(&f, cx, "voice-room-r1", "Join voice in Lounge");
}

#[gpui_kit::test]
fn the_call_ending_on_its_own_says_so_and_gives_the_place_back(cx: &mut TestAppContext) {
    let fake = FakeConnector::automatic(vec![person("i-me", "me", true)]);
    let f = with_voice(cx, &fake);
    wait_drawn(&f, cx, "voice-room-r1");
    click(&f, cx, "voice-room-r1");
    labelled(&f, cx, "voice-status", "Voice connected");
    fake.report(MediaEvent::Ended);
    labelled(&f, cx, "voice-join-error", "Voice disconnected.");
    f.server
        .wait_for_nth("DELETE", "/api/v1/voice/sessions/cred1", 1, WAIT);
}

#[gpui_kit::test]
fn a_call_without_sound_devices_says_so_and_stays(cx: &mut TestAppContext) {
    let fake = FakeConnector::automatic(vec![person("i-me", "me", true)]);
    let f = with_voice(cx, &fake);
    wait_drawn(&f, cx, "voice-room-r1");
    click(&f, cx, "voice-room-r1");
    labelled(&f, cx, "voice-status", "Voice connected");
    fake.report(MediaEvent::AudioUnavailable);
    labelled(
        &f,
        cx,
        "voice-call-error",
        "No sound device could be opened: you cannot hear or be heard in this call.",
    );
    labelled(&f, cx, "voice-status", "Voice connected");
}

#[gpui_kit::test]
fn a_refused_join_shows_the_servers_reason(cx: &mut TestAppContext) {
    let fake = FakeConnector::default();
    let f = with_voice_and(cx, &fake, |server| {
        server.route_reply(
            "POST",
            "/api/v1/voice/rooms/r1/join",
            Reply::problem(
                409,
                "Leave your current voice session before joining another.",
            ),
        );
    });
    wait_drawn(&f, cx, "voice-room-r1");
    click(&f, cx, "voice-room-r1");
    labelled(
        &f,
        cx,
        "voice-join-error",
        "Leave your current voice session before joining another.",
    );
    assert!(fake.asked().is_empty(), "nothing to connect to");
}

#[gpui_kit::test]
fn voice_updated_reads_the_rooms_again(cx: &mut TestAppContext) {
    let fake = FakeConnector::default();
    let f = with_voice(cx, &fake);
    labelled(&f, cx, "voice-person-r1-0", "ann");
    f.server.route(
        "GET",
        "/api/v1/voice/rooms",
        200,
        json!([room("r1", "Lounge", &["ann", "bob"], 8)]),
    );
    f.server.push(ephemeral("voice.updated", &json!({})));
    labelled(&f, cx, "voice-person-r1-1", "bob");
    assert!(!drawn(&f, cx, "voice-room-r2"), "Studio was deleted");
}

#[gpui_kit::test]
fn settings_choose_the_devices_and_processing_and_keep_them(cx: &mut TestAppContext) {
    let fake = FakeConnector::automatic(vec![person("i-me", "me", true)]).with_devices(Devices {
        microphones: vec![
            Device {
                id: "mic-a".into(),
                name: "Desk microphone".into(),
            },
            Device {
                id: "mic-b".into(),
                name: "Headset".into(),
            },
        ],
        speakers: vec![Device {
            id: "spk-a".into(),
            name: "Speakers".into(),
        }],
    });
    let f = with_voice(cx, &fake);
    // In a call, so the change reaches it at once.
    wait_drawn(&f, cx, "voice-room-r1");
    click(&f, cx, "voice-room-r1");
    labelled(&f, cx, "voice-status", "Voice connected");
    click(&f, cx, "voice-settings");
    wait_drawn(&f, cx, "voice-microphone-choice-2");
    // System default, then the two microphones.
    click(&f, cx, "voice-microphone-choice-2");
    let expected = AudioSettings {
        microphone: Some("mic-b".into()),
        ..AudioSettings::default()
    };
    wait_asked(cx, &fake, Asked::Apply(expected.clone()));
    click(&f, cx, "voice-echo-cancellation-switch");
    let expected = AudioSettings {
        echo_cancellation: false,
        ..expected
    };
    wait_asked(cx, &fake, Asked::Apply(expected.clone()));
    let config: ConfigDir = cx.update(|cx| f.session.read(cx).config.clone());
    assert_eq!(config.section::<AudioSettings>(SECTION), expected);
}

#[gpui_kit::test]
fn the_rooms_and_the_call_pass_the_audit(cx: &mut TestAppContext) {
    let fake = FakeConnector::automatic(vec![
        person("i-ann", "ann", false),
        person("i-me", "me", true),
    ]);
    let f = with_voice(cx, &fake);
    wait_drawn(&f, cx, "voice-room-r1");
    click(&f, cx, "voice-room-r1");
    labelled(&f, cx, "voice-status", "Voice connected");
    let problems = cx
        .update_window(f.window, |_, window, cx| {
            crate::audit::audit("voice", window, cx)
        })
        .unwrap();
    assert!(problems.is_empty(), "{problems:#?}");
}

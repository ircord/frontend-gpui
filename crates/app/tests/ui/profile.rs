//! Tests for package S4: sign-up, onboarding and Settings → Profile.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gpui_kit::test::TestWindowExt;
use gpui_kit::{AppContext, TestAppContext};
use ircord_app::session::Phase;
use ircord_client::testing::{CANNED_COOKIE, CannedServer, Reply};
use serde_json::{Value, json};

use crate::fixture::{Fixture, bootstrap, open_with, wait};

const WAIT: Duration = Duration::from_secs(5);

fn me(extra: Value) -> Value {
    let mut m = json!({"id": "p-me", "account": "me", "nick": "me", "displayName": "me",
        "nameSource": "nick", "bio": "", "color": "", "role": "member", "status": "online"});
    for (k, v) in extra.as_object().unwrap() {
        m[k] = v.clone();
    }
    m
}

/// Your profile on `n1`, and saves that answer with it changed.
fn serve_profile(server: &CannedServer) {
    let stored = Arc::new(Mutex::new(me(json!({}))));
    let read = stored.clone();
    server.route_fn("GET", "/api/v1/networks/n1/profile", move |_| {
        Reply::json(200, read.lock().unwrap().clone())
    });
    server.route_fn("PATCH", "/api/v1/networks/n1/profile", move |r| {
        let mut m = stored.lock().unwrap();
        for (k, v) in r.json().as_object().unwrap() {
            match k.as_str() {
                "displayName" => {
                    m["displayName"] = v.clone();
                    m["nameSource"] = json!("profile");
                }
                "avatarDataUrl" => m["avatarUrl"] = json!("/files/avatar.png"),
                _ => m[k] = v.clone(),
            }
        }
        Reply::json(200, m.clone())
    });
    server.route(
        "GET",
        "/avatars/defaults/manifest.json",
        200,
        json!({"avatars": [{"id": "a1", "file": "a1.png"}, {"id": "a2", "file": "a2.png"}]}),
    );
}

fn render(f: &Fixture, cx: &mut TestAppContext) {
    cx.update_window(f.window, |_, window, cx| window.render_frame(cx))
        .unwrap();
}

fn find<T>(
    f: &Fixture,
    cx: &mut TestAppContext,
    id: &str,
    read: impl FnOnce(&gpui_kit::test::ElementSnapshot) -> T,
) -> Option<T> {
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window
            .try_find(gpui_kit::ElementId::Name(id.to_string().into()))
            .map(|e| read(&e))
    })
    .unwrap()
}

fn shows(f: &Fixture, cx: &mut TestAppContext, id: &str) {
    let deadline = Instant::now() + WAIT;
    while find(f, cx, id, |e| e.visible()) != Some(true) {
        assert!(Instant::now() < deadline, "timed out waiting for {id}");
        cx.run_until_parked();
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn gone(f: &Fixture, cx: &mut TestAppContext, id: &str) {
    let deadline = Instant::now() + WAIT;
    while find(f, cx, id, |e| e.visible()).unwrap_or(false) {
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {id} to go"
        );
        cx.run_until_parked();
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Wait until `id` reads `label`.
fn reads(f: &Fixture, cx: &mut TestAppContext, id: &str, label: &str) {
    let deadline = Instant::now() + WAIT;
    loop {
        let now = find(f, cx, id, |e| e.label().map(str::to_string)).flatten();
        if now.as_deref() == Some(label) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "{id} reads {now:?}, not {label:?}"
        );
        // The username check waits for a pause in typing.
        cx.executor().advance_clock(Duration::from_millis(50));
        cx.run_until_parked();
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Open the settings on `row`, scrolled to it.
fn land(f: &Fixture, cx: &mut TestAppContext, row: &str, shown: &str) {
    let session = f.session.clone();
    cx.update(|cx| ircord_app::settings::open(&session, Some(row), cx));
    shows(f, cx, shown);
}

fn enter(f: &Fixture, cx: &mut TestAppContext, id: &str) {
    shows(f, cx, id);
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.click(gpui_kit::ElementId::Name(id.to_string().into()), cx);
        window.press("enter", cx);
    })
    .unwrap();
    cx.run_until_parked();
}

fn click(f: &Fixture, cx: &mut TestAppContext, id: &str) {
    shows(f, cx, id);
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.click(gpui_kit::ElementId::Name(id.to_string().into()), cx);
    })
    .unwrap();
    cx.run_until_parked();
}

fn type_into(f: &Fixture, cx: &mut TestAppContext, id: &str, text: &str) {
    shows(f, cx, id);
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.click(gpui_kit::ElementId::Name(id.to_string().into()), cx);
        window.press("ctrl-a", cx);
        window.press("backspace", cx);
        window.input(text, cx);
    })
    .unwrap();
    cx.run_until_parked();
}

#[gpui_kit::test]
fn edits_the_profile_in_settings(cx: &mut TestAppContext) {
    let f = open_with(cx, true, serve_profile);
    bootstrap(&f, cx);
    // The gear opens Settings on Profile, the first section.
    click(&f, cx, "open-settings");
    reads(&f, cx, "settings-content", "Profile");
    land(&f, cx, "profile-name", "profile-name-input");
    render(&f, cx);
    reads(&f, cx, "profile-shown-as", "Shown as me · @me");

    type_into(&f, cx, "profile-name-input", "Petra");
    reads(&f, cx, "profile-shown-as", "Shown as Petra · @me");
    land(&f, cx, "profile-picture", "profile-default-a2");
    click(&f, cx, "profile-default-a2");
    assert_eq!(
        find(&f, cx, "profile-default-a2", |e| e.selected()).flatten(),
        Some(true)
    );
    land(&f, cx, "profile-bio", "profile-bio-input");
    type_into(&f, cx, "profile-bio-input", "Hi there");
    land(&f, cx, "profile-color", "profile-color-mode-custom");
    click(&f, cx, "profile-color-mode-custom");
    type_into(&f, cx, "profile-color-input", "blue");

    // A colour that is not #RRGGBB stops the save and says why.
    enter(&f, cx, "profile-color-input");
    shows(&f, cx, "profile-color-input-problem");
    assert!(
        f.server
            .requests_to("PATCH", "/api/v1/networks/n1/profile")
            .is_empty()
    );
    type_into(&f, cx, "profile-color-input", "#3366cc");
    land(&f, cx, "profile-look", "profile-frame-ring");
    click(&f, cx, "profile-frame-ring");
    land(&f, cx, "profile-about", "profile-pronouns");
    type_into(&f, cx, "profile-pronouns", "she/her");
    enter(&f, cx, "profile-pronouns");
    let patch = f
        .server
        .wait_for("PATCH", "/api/v1/networks/n1/profile", WAIT);
    let body = patch.json();
    assert_eq!(body["displayName"], "Petra");
    assert_eq!(body["bio"], "Hi there");
    assert_eq!(body["nameColor"], "#3366cc");
    assert_eq!(body["pronouns"], "she/her");
    assert_eq!(body["avatarFrame"], "ring");
    let avatar = body["avatarUrl"].as_str().unwrap();
    assert!(avatar.ends_with("/avatars/defaults/a2.png"), "{avatar}");
    assert!(avatar.starts_with("http://127.0.0.1"), "{avatar}");
    // Untouched fields are left out, so they keep what is saved.
    for key in [
        "headline",
        "location",
        "statusText",
        "statusExpiresAt",
        "languages",
    ] {
        assert!(body.get(key).is_none(), "{key} was sent");
    }
    wait(cx, "the saved profile in the workspace", |cx| {
        let session = f.session.read(cx);
        let w = session.client().unwrap().read();
        w.network("n1")
            .and_then(|n| n.profile.as_ref())
            .is_some_and(|p| p.display_name == "Petra" && p.pronouns.as_deref() == Some("she/her"))
    });
    land(&f, cx, "profile-name", "profile-name-input");
    reads(&f, cx, "profile-shown-as", "Shown as Petra · @me");

    // Clearing a field sends it empty; a status sets when it clears.
    land(&f, cx, "profile-about", "profile-pronouns");
    type_into(&f, cx, "profile-pronouns", "");
    land(&f, cx, "profile-status", "profile-clear-after-1h");
    type_into(&f, cx, "profile-status-text", "Writing tests");
    click(&f, cx, "profile-clear-after-1h");
    enter(&f, cx, "profile-status-text");
    let body = f
        .server
        .wait_for_nth("PATCH", "/api/v1/networks/n1/profile", 2, WAIT)
        .json();
    assert_eq!(body["pronouns"], "");
    assert_eq!(body["statusText"], "Writing tests");
    let until = body["statusExpiresAt"].as_str().unwrap();
    let at = chrono::DateTime::parse_from_rfc3339(until).unwrap();
    let from_now = at.signed_duration_since(chrono::Utc::now());
    assert!(
        from_now > chrono::Duration::minutes(55) && from_now <= chrono::Duration::minutes(61),
        "{until}"
    );
    assert!(body.get("displayName").is_none());
}

/// The server's sign-in check, the registration policy, the username check
/// (`taken` is taken) and a sign-up that starts a session. The account it
/// creates has its setup to do on `n1`.
fn serve_sign_up(server: &CannedServer, onboarding: bool) {
    server.route_fn("GET", "/api/v1/auth/session", move |r| {
        if r.header("cookie") == Some(CANNED_COOKIE) {
            let mut account = json!({"id": "a-new", "username": "newbie",
                "createdAt": "2026-11-21T10:00:00Z", "expiresAt": "2027-11-21T10:00:00Z",
                "lastSeenAt": "2026-11-21T10:00:00Z"});
            if onboarding {
                account["onboardingNetworkId"] = json!("n1");
            }
            Reply::json(200, json!({ "account": account }))
        } else {
            Reply::json(200, json!({"account": null}))
        }
    });
    server.route(
        "GET",
        "/api/v1/auth/registration",
        200,
        json!({"registrationEnabled": true, "registrationKeyRequired": false}),
    );
    server.route_fn("GET", "/api/v1/auth/username", |r| {
        let name = r.query_param("name").unwrap_or_default();
        if name == "taken" {
            Reply::json(
                200,
                json!({"available": false, "reason": "taken is already taken."}),
            )
        } else {
            Reply::json(200, json!({"available": true}))
        }
    });
    server.route_reply(
        "POST",
        "/api/v1/auth/signup",
        Reply::json(
            201,
            json!({"account": {"id": "a-new", "username": "newbie",
                "createdAt": "2026-11-21T10:00:00Z", "expiresAt": "2027-11-21T10:00:00Z",
                "lastSeenAt": "2026-11-21T10:00:00Z"}}),
        )
        .with_header("Set-Cookie", &format!("{CANNED_COOKIE}; Path=/; HttpOnly")),
    );
}

#[gpui_kit::test]
fn signs_up_from_the_sign_in_screen_and_lands_in_the_setup(cx: &mut TestAppContext) {
    let f = open_with(cx, false, |s| {
        serve_sign_up(s, true);
        serve_profile(s);
        s.route(
            "GET",
            "/api/v1/networks/n1/channel-guide",
            200,
            json!({"available": false, "revision": 0, "groups": [], "entries": []}),
        );
    });
    let address = f.server.address().origin();
    let session = f.session.clone();
    cx.update(|cx| session.update(cx, |s, cx| s.choose_server(&address, cx)));
    shows(&f, cx, "sign-up");
    click(&f, cx, "sign-up");
    reads(&f, cx, "sign-in-title", "Create your account");

    // The username is checked while it is typed.
    type_into(&f, cx, "signup-username", "taken");
    reads(&f, cx, "signup-username-note", "taken is already taken.");
    type_into(&f, cx, "signup-username", "newbie");
    reads(&f, cx, "signup-username-note", "newbie is available");

    // A short password is not sent.
    type_into(&f, cx, "signup-password", "short");
    click(&f, cx, "submit");
    reads(&f, cx, "signup-password-note", "Use at least 8 characters.");
    assert!(
        f.server
            .requests_to("POST", "/api/v1/auth/signup")
            .is_empty()
    );

    type_into(&f, cx, "signup-password", "longenough");
    click(&f, cx, "submit");
    let signup = f.server.wait_for("POST", "/api/v1/auth/signup", WAIT);
    assert_eq!(signup.json()["username"], "newbie");
    assert_eq!(signup.json()["password"], "longenough");
    assert!(signup.json().get("registrationKey").is_none());
    wait(cx, "signed in", |cx| {
        matches!(f.session.read(cx).phase, Phase::SignedIn { .. })
    });
    let stored = cx.update(|cx| {
        let s = f.session.read(cx);
        s.credentials.load(s.server().unwrap()).unwrap()
    });
    assert_eq!(stored.as_deref(), Some(CANNED_COOKIE));

    // The new account's setup opens over the workspace.
    shows(&f, cx, "onboarding-nick");
    shows(&f, cx, "onboarding-default-a1");
}

#[gpui_kit::test]
fn closed_registration_offers_no_sign_up(cx: &mut TestAppContext) {
    let f = open_with(cx, false, |s| {
        s.route(
            "GET",
            "/api/v1/auth/registration",
            200,
            json!({"registrationEnabled": false, "registrationKeyRequired": false}),
        );
    });
    let address = f.server.address().origin();
    let session = f.session.clone();
    cx.update(|cx| session.update(cx, |s, cx| s.choose_server(&address, cx)));
    shows(&f, cx, "registration-closed");
    assert_eq!(find(&f, cx, "sign-up", |_| ()), None);
}

/// A signed-in account with its setup to do on `n1`, and a channel guide:
/// `#general` for everyone (already joined), `#chess` suggested, `#misc`
/// listed. Joining `#misc` fails each time.
fn serve_onboarding(server: &CannedServer) {
    serve_sign_up(server, true);
    serve_profile(server);
    server.route(
        "GET",
        "/api/v1/networks/n1/channel-guide",
        200,
        json!({"available": true, "revision": 1, "groups": [{"name": "Games"}],
        "entries": [
            {"channel": "#general", "state": "everyone", "nsfw": false},
            {"channel": "#chess", "state": "suggested", "group": "Games", "nsfw": false,
             "description": "Play a game"},
            {"channel": "#misc", "state": "listed", "nsfw": false}
        ]}),
    );
    server.route_fn("POST", "/api/v1/networks/n1/channel-guide/join", |r| {
        let failed: Vec<Value> = r.json()["channels"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|c| *c == "#misc")
            .map(|c| json!({"channel": c, "reason": "banned"}))
            .collect();
        Reply::json(200, json!({"joined": [], "queued": [], "failed": failed}))
    });
    server.route_fn("PATCH", "/api/v1/networks/n1", |r| {
        let mut network = serde_json::to_value(&crate::fixture::snapshot().networks[0]).unwrap();
        for (k, v) in r.json().as_object().unwrap() {
            network[k] = v.clone();
        }
        Reply::json(200, network)
    });
    server.route_reply("POST", "/api/v1/auth/onboarding/complete", Reply::empty());
}

#[gpui_kit::test]
fn the_setup_saves_the_profile_picks_channels_and_completes(cx: &mut TestAppContext) {
    let f = open_with(cx, true, serve_onboarding);
    bootstrap(&f, cx);
    shows(&f, cx, "onboarding-nick");
    reads(&f, cx, "onboarding-preview-name", "me");

    // A nickname with a space is refused before anything is sent.
    type_into(&f, cx, "onboarding-nick", "pe tra");
    reads(
        &f,
        cx,
        "onboarding-nick-help",
        "Nicknames cannot contain spaces. Your display name can.",
    );
    // Enter in a field tries to save and keeps the dialog open.
    enter(&f, cx, "onboarding-nick");
    shows(&f, cx, "onboarding-nick");
    assert!(
        f.server
            .requests_to("PATCH", "/api/v1/networks/n1")
            .is_empty()
    );

    type_into(&f, cx, "onboarding-nick", "petra");
    type_into(&f, cx, "onboarding-name", "Petra P");
    reads(&f, cx, "onboarding-preview-name", "Petra P");
    type_into(&f, cx, "onboarding-realname", "Petra Person");
    click(&f, cx, "onboarding-default-a1");
    click(&f, cx, "onboarding-save");
    let profile = f
        .server
        .wait_for("PATCH", "/api/v1/networks/n1/profile", WAIT)
        .json();
    assert_eq!(profile["displayName"], "Petra P");
    assert!(
        profile["avatarUrl"]
            .as_str()
            .unwrap()
            .ends_with("/avatars/defaults/a1.png")
    );
    let network = f
        .server
        .wait_for("PATCH", "/api/v1/networks/n1", WAIT)
        .json();
    assert_eq!(
        network,
        json!({"nick": "petra", "realname": "Petra Person"})
    );

    // The channels: everyone's is fixed, the suggested one starts ticked.
    shows(&f, cx, "onboarding-channel-chess");
    assert_eq!(
        find(&f, cx, "onboarding-channel-chess", |e| e.checked()).flatten(),
        Some(true)
    );
    assert_eq!(
        find(&f, cx, "onboarding-channel-general", |e| e.checked()).flatten(),
        Some(true)
    );
    assert_eq!(
        find(&f, cx, "onboarding-channel-misc", |e| e.checked()).flatten(),
        Some(false)
    );
    click(&f, cx, "onboarding-channel-misc");
    click(&f, cx, "onboarding-join");
    let join = f
        .server
        .wait_for("POST", "/api/v1/networks/n1/channel-guide/join", WAIT)
        .json();
    assert_eq!(join["channels"], json!(["#chess", "#misc"]));

    // The first failure stays on the step with the failed channel ticked.
    shows(&f, cx, "onboarding-error");
    assert!(
        f.server
            .requests_to("POST", "/api/v1/auth/onboarding/complete")
            .is_empty()
    );
    assert_eq!(
        find(&f, cx, "onboarding-channel-chess", |e| e.checked()).flatten(),
        Some(false)
    );
    click(&f, cx, "onboarding-join");
    let again = f
        .server
        .wait_for_nth("POST", "/api/v1/networks/n1/channel-guide/join", 2, WAIT)
        .json();
    assert_eq!(again["channels"], json!(["#misc"]));
    // After that it goes on without it.
    f.server
        .wait_for("POST", "/api/v1/auth/onboarding/complete", WAIT);
    gone(&f, cx, "onboarding-join");
}

#[gpui_kit::test]
fn skipping_the_setup_completes_it_with_the_suggested_channels(cx: &mut TestAppContext) {
    let f = open_with(cx, true, serve_onboarding);
    bootstrap(&f, cx);
    shows(&f, cx, "onboarding-skip");
    click(&f, cx, "onboarding-skip");
    shows(&f, cx, "onboarding-channel-chess");
    assert!(
        f.server
            .requests_to("PATCH", "/api/v1/networks/n1/profile")
            .is_empty()
    );
    click(&f, cx, "onboarding-skip");
    let join = f
        .server
        .wait_for("POST", "/api/v1/networks/n1/channel-guide/join", WAIT)
        .json();
    assert_eq!(join["channels"], json!(["#chess"]));
    f.server
        .wait_for("POST", "/api/v1/auth/onboarding/complete", WAIT);
    gone(&f, cx, "onboarding-skip");
}

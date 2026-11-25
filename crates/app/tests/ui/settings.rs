//! Tests for package S0: the settings page, its search, the saving state,
//! Account (password, log out, delete, devices) and About, against the
//! canned server.

use std::time::Duration;

use gpui_kit::test::TestWindowExt;
use gpui_kit::{AppContext, TestAppContext};
use ircord_app::route::Route;
use ircord_app::session::Phase;
use ircord_client::testing::{CannedServer, Reply};
use serde_json::json;

use crate::fixture::{Fixture, bootstrap, open_with, wait};

const WAIT: Duration = crate::fixture::HANG_GUARD;

fn serve(server: &CannedServer) {
    server.route(
        "GET",
        "/api/v1/auth/session",
        200,
        json!({"account": {"id": "a1", "username": "me",
            "createdAt": "2026-01-02T10:00:00Z", "expiresAt": "2027-02-03T10:00:00Z",
            "lastSeenAt": "2026-11-20T10:00:00Z"}}),
    );
    server.route(
        "GET",
        "/api/v1/auth/devices",
        200,
        json!({"devices": [
            {"id": "d-phone", "current": false, "name": "", "browser": "Firefox", "os": "Android",
             "formFactor": "phone", "signedInAt": "2026-11-01T10:00:00Z", "status": "offline",
             "lastSeenAt": "2026-11-20T10:00:00Z"},
            {"id": "d-here", "current": true, "name": "", "browser": "IRCord-GPUI", "os": "Linux",
             "formFactor": "desktop", "signedInAt": "2026-11-21T09:00:00Z", "status": "connected"}
        ]}),
    );
    server.route(
        "GET",
        "/api/v1/changelog",
        200,
        json!({"version": "9.8.7", "releases": [
            {"version": "9.8.7", "date": "2026-11-01", "notes": "### New\n\n- Server things."}
        ]}),
    );
}

fn signed_in(cx: &mut TestAppContext) -> Fixture {
    let f = open_with(cx, true, serve);
    bootstrap(&f, cx);
    f
}

fn route(f: &Fixture, cx: &mut TestAppContext) -> Option<Route> {
    cx.update(|cx| f.session.read(cx).route())
}

/// Wait until an element is drawn (it may be scrolled out of view),
/// rendering between checks.
fn shows(f: &Fixture, cx: &mut TestAppContext, id: &'static str) {
    let window = f.window;
    wait(cx, id, move |cx| {
        window
            .update(cx, |_, window, cx| {
                window.render_frame(cx);
                window.try_find(id).is_some()
            })
            .unwrap_or(false)
    });
}

fn label(f: &Fixture, cx: &mut TestAppContext, id: &'static str) -> Option<String> {
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.find(id).label().map(str::to_string)
    })
    .unwrap()
}

/// Wait until `id`'s label is `text`: a control can show before its content
/// has come from the server.
fn shows_label(f: &Fixture, cx: &mut TestAppContext, id: &'static str, text: &str) {
    let window = f.window;
    wait(cx, id, move |cx| {
        window
            .update(cx, |_, window, cx| {
                window.render_frame(cx);
                window
                    .try_find(id)
                    .and_then(|e| e.label().map(str::to_string))
                    .as_deref()
                    == Some(text)
            })
            .unwrap_or(false)
    });
}

fn open_settings(f: &Fixture, cx: &mut TestAppContext) {
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.click("open-settings", cx);
    })
    .unwrap();
    assert_eq!(route(f, cx), Some(Route::Settings));
    shows(f, cx, "settings-content");
    // Profile (S4) is the first section; these tests are about Account.
    shows(f, cx, "settings-section-account");
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.click("settings-section-account", cx);
    })
    .unwrap();
    shows(f, cx, "account-summary");
}

#[gpui_kit::test]
fn the_gear_and_the_key_open_settings_with_sections_under_headings(cx: &mut TestAppContext) {
    let f = signed_in(cx);
    open_settings(&f, cx);
    // Account is under "User settings", after Profile.
    assert_eq!(
        label(&f, cx, "settings-content").as_deref(),
        Some("Account")
    );
    for id in ["settings-section-account", "settings-section-about"] {
        shows(&f, cx, id);
    }
    // Back to the conversation, then the key.
    f.session.update(cx, |s, cx| s.go_back(cx));
    assert_eq!(route(&f, cx), Some(Route::Conversation("c-general".into())));
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.click("composer-input", cx);
        window.press("ctrl-,", cx);
    })
    .unwrap();
    assert_eq!(route(&f, cx), Some(Route::Settings));
}

#[gpui_kit::test]
fn closing_settings_returns_focus_to_the_workspace(cx: &mut TestAppContext) {
    let f = signed_in(cx);
    let opener = cx
        .update_window(f.window, |_, window, cx| {
            window.render_frame(cx);
            window.click("composer-input", cx);
            let opener = window.focused(cx).expect("the composer takes focus");
            ircord_app::settings::open(&f.session, None, cx);
            opener
        })
        .unwrap();
    shows(&f, cx, "settings-dialog");
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.find("timeline").visible());
        assert!(!opener.is_focused(window), "Settings takes focus");
        window.click("settings-close", cx);
    })
    .unwrap();
    assert_eq!(route(&f, cx), Some(Route::Conversation("c-general".into())));
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("settings-dialog").is_none());
        assert!(opener.is_focused(window), "focus returns to the composer");
    })
    .unwrap();
}

#[gpui_kit::test]
fn search_finds_a_row_and_lands_on_it(cx: &mut TestAppContext) {
    let f = signed_in(cx);
    open_settings(&f, cx);
    cx.update_window(f.window, |_, window, cx| {
        window.click("settings-section-about", cx);
        window.render_frame(cx);
        window.click("settings-search", cx);
        window.input("sign out", cx);
    })
    .unwrap();
    shows(&f, cx, "settings-results");
    // "sign out" is a keyword of two Account rows, not a word of About.
    assert_eq!(
        label(&f, cx, "settings-result-log-out").as_deref(),
        Some("Log out, in Account")
    );
    shows(&f, cx, "settings-result-device-sessions");
    cx.update_window(f.window, |_, window, cx| {
        assert!(window.try_find("settings-result-about-ircord").is_none());
        window.click("settings-result-log-out", cx);
    })
    .unwrap();
    shows(&f, cx, "setting-log-out");
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find("settings-results").is_none(),
            "search closed"
        );
        assert_eq!(window.find("settings-content").label(), Some("Account"));
    })
    .unwrap();

    // Enter opens the first result; a search with nothing says so.
    cx.update_window(f.window, |_, window, cx| {
        window.click("settings-search", cx);
        window.input("zzzz", cx);
    })
    .unwrap();
    shows(&f, cx, "settings-no-matches");
    cx.update_window(f.window, |_, window, cx| {
        window.press("cmd-a", cx);
        window.press("ctrl-a", cx);
        window.input("changelog", cx);
        window.press("enter", cx);
    })
    .unwrap();
    shows(&f, cx, "setting-about-ircord");
    assert_eq!(label(&f, cx, "settings-content").as_deref(), Some("About"));
}

#[gpui_kit::test]
fn the_header_says_when_a_change_is_saved(cx: &mut TestAppContext) {
    let f = signed_in(cx);
    open_settings(&f, cx);
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("settings-save-state").is_none());
    })
    .unwrap();
    cx.update(ircord_app::settings::saved);
    assert_eq!(
        label(&f, cx, "settings-save-state").as_deref(),
        Some("Saved")
    );
}

#[gpui_kit::test]
fn changes_the_password(cx: &mut TestAppContext) {
    let f = signed_in(cx);
    f.server.route_reply(
        "POST",
        "/api/v1/auth/password",
        Reply::problem(403, "The current password is not right."),
    );
    open_settings(&f, cx);
    shows_label(&f, cx, "account-summary", "me");
    cx.update_window(f.window, |_, window, cx| {
        window.click("current-password", cx);
        window.input("wrong", cx);
        window.click("new-password", cx);
        window.input("short", cx);
        window.render_frame(cx);
        assert!(
            window.find("change-password").disabled() == Some(true),
            "too short"
        );
        window.input("-but-longer", cx);
        window.render_frame(cx);
        window.click("change-password", cx);
    })
    .unwrap();
    shows(&f, cx, "password-error");
    assert_eq!(
        label(&f, cx, "password-error").as_deref(),
        Some("The current password is not right.")
    );
    let sent = f
        .server
        .wait_for_nth("POST", "/api/v1/auth/password", 1, WAIT);
    assert_eq!(
        sent.json(),
        json!({"current": "wrong", "password": "short-but-longer"})
    );

    f.server
        .route_reply("POST", "/api/v1/auth/password", Reply::empty());
    cx.update_window(f.window, |_, window, cx| {
        window.click("change-password", cx);
    })
    .unwrap();
    shows(&f, cx, "password-saved");
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("password-error").is_none());
        assert!(
            window.find("change-password").disabled() == Some(true),
            "the fields are cleared"
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn lists_devices_and_signs_one_out(cx: &mut TestAppContext) {
    let f = signed_in(cx);
    f.server
        .route_reply("DELETE", "/api/v1/auth/devices/d-phone", Reply::empty());
    open_settings(&f, cx);
    // Land on the list the way a search result does: it scrolls into view.
    let session = f.session.clone();
    cx.update(|cx| ircord_app::settings::open(&session, Some("device-sessions"), cx));
    shows(&f, cx, "device-list");
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.render_frame(cx);
        assert!(window.find("sign-out-d-phone").visible(), "scrolled to");
    })
    .unwrap();
    shows_label(&f, cx, "device-d-here", "IRCord-GPUI on Linux, this device");
    shows_label(&f, cx, "device-d-phone", "Firefox on Android");
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        // This device is listed first.
        let rows = [window.find("device-d-here"), window.find("device-d-phone")];
        assert!(rows[0].bounds().origin.y < rows[1].bounds().origin.y);
        window.click("sign-out-d-phone", cx);
        window.render_frame(cx);
        assert!(window.try_find("dialog").is_some(), "asks first");
        window.click("ok", cx);
    })
    .unwrap();
    f.server
        .wait_for_nth("DELETE", "/api/v1/auth/devices/d-phone", 1, WAIT);
    shows(&f, cx, "devices-notice");
    assert_eq!(
        label(&f, cx, "devices-notice").as_deref(),
        Some("Signed out Firefox on Android.")
    );
    cx.update(|cx| assert!(matches!(f.session.read(cx).phase, Phase::SignedIn { .. })));
}

#[gpui_kit::test]
fn a_refused_deletion_says_why_and_a_deletion_signs_out(cx: &mut TestAppContext) {
    let f = signed_in(cx);
    f.server.route_reply(
        "DELETE",
        "/api/v1/auth/account",
        Reply::problem(403, "The password is not right."),
    );
    open_settings(&f, cx);
    let delete = |f: &Fixture, cx: &mut TestAppContext, password: &str| {
        cx.update_window(f.window, |_, window, cx| {
            window.render_frame(cx);
            window.click("delete-account-button", cx);
            window.render_frame(cx);
            window.click("delete-password", cx);
            window.input(password, cx);
            window.render_frame(cx);
            window.click("delete-confirm", cx);
        })
        .unwrap();
    };
    delete(&f, cx, "wrong");
    shows(&f, cx, "delete-error");
    assert_eq!(
        label(&f, cx, "delete-error").as_deref(),
        Some("The password is not right.")
    );
    let sent = f
        .server
        .wait_for_nth("DELETE", "/api/v1/auth/account", 1, WAIT);
    assert_eq!(sent.json(), json!({"password": "wrong"}));
    cx.update(|cx| assert!(matches!(f.session.read(cx).phase, Phase::SignedIn { .. })));

    f.server
        .route_reply("DELETE", "/api/v1/auth/account", Reply::empty());
    delete(&f, cx, "right");
    wait(cx, "signed out", |cx| {
        matches!(f.session.read(cx).phase, Phase::SignIn { .. })
    });
}

#[gpui_kit::test]
fn log_out_asks_first(cx: &mut TestAppContext) {
    let f = signed_in(cx);
    open_settings(&f, cx);
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.click("log-out-button", cx);
        window.render_frame(cx);
        window.click("ok", cx);
    })
    .unwrap();
    wait(cx, "signed out", |cx| {
        matches!(f.session.read(cx).phase, Phase::SignIn { .. })
    });
}

#[gpui_kit::test]
fn about_shows_both_versions_the_people_and_the_release_notes(cx: &mut TestAppContext) {
    let f = signed_in(cx);
    open_settings(&f, cx);
    cx.update_window(f.window, |_, window, cx| {
        window.click("settings-section-about", cx);
    })
    .unwrap();
    shows(&f, cx, "about-server-notes");
    assert_eq!(
        label(&f, cx, "about-server-version").as_deref(),
        Some("Server: 9.8.7")
    );
    assert_eq!(
        label(&f, cx, "about-app-version"),
        Some(format!("App: {}", env!("CARGO_PKG_VERSION")))
    );
    // The server signed in to, by its origin, as the web's About shows it.
    let origin = f.server.address().origin();
    assert_eq!(
        label(&f, cx, "about-server"),
        Some(format!("IRCord server: {origin}"))
    );
    for id in [
        "about-app-notes",
        "release-9.8.7",
        "about-collaborators",
        "collaborator-Peter Petermann",
        "about-report",
    ] {
        shows(&f, cx, id);
    }
}

#[gpui_kit::test]
fn settings_show_a_preview_and_log_out_and_escape_leaves(cx: &mut TestAppContext) {
    let f = signed_in(cx);
    open_settings(&f, cx);
    // Log out at the foot of the section list, asking first.
    shows(&f, cx, "settings-log-out");
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.click("settings-section-appearance", cx);
    })
    .unwrap();
    // Appearance opens with the sample messages and the theme cards.
    shows(&f, cx, "settings-preview");
    shows(&f, cx, "appearance-schemes");
    assert_eq!(
        label(&f, cx, "settings-preview").as_deref(),
        Some("Preview")
    );
    // Escape goes back to the conversation it was opened from.
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.press("escape", cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(route(&f, cx), Some(Route::Conversation("c-general".into())));
}

#[gpui_kit::test]
fn log_out_in_the_section_list_asks_first(cx: &mut TestAppContext) {
    let f = signed_in(cx);
    open_settings(&f, cx);
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.click("settings-log-out", cx);
        window.render_frame(cx);
        window.click("ok", cx);
    })
    .unwrap();
    wait(cx, "signed out", |cx| {
        matches!(f.session.read(cx).phase, Phase::SignIn { .. })
    });
}

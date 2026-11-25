//! Tests for package C1: slash commands, autocomplete and the composer's
//! keys, against the canned server.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::test::{TestEventExt, TestQueryExt, TestWindowExt};
use gpui_kit::{AppContext, Entity, MouseButton, TestAppContext};
use ircord_app::Screen;
use ircord_app::composer::ComposerView;
use ircord_app::features::commands;
use ircord_client::commands::View;
use ircord_client::contract::*;
use ircord_client::testing::{CannedServer, Reply};
use serde_json::json;

use crate::fixture::{Fixture, open_with, snapshot, wait};

const WAIT: Duration = crate::fixture::HANG_GUARD;

fn open(cx: &mut TestAppContext, snapshot: Snapshot, setup: impl FnOnce(&CannedServer)) -> Fixture {
    let f = open_with(cx, true, |server| {
        server.snapshot(&snapshot);
        server.route(
            "POST",
            "/api/v1/networks/*/commands",
            202,
            json!({"clientId": "x"}),
        );
        setup(server);
    });
    wait(cx, "#general and the live socket", |cx| {
        let session = f.session.read(cx);
        let Some(client) = session.client() else {
            return false;
        };
        let w = client.read();
        session.conversation.as_deref() == Some("c-general")
            && w.connection == Some(ircord_client::store::Connection::Live)
            && w.timelines.get("c-general").is_some_and(|t| t.loaded)
    });
    f
}

fn composer(f: &Fixture, cx: &mut TestAppContext) -> Entity<ComposerView> {
    cx.update(|cx| match f.view.read(cx).screen() {
        Screen::Workspace(w) => w.read(cx).composer.clone(),
        _ => panic!("the workspace is not showing"),
    })
}

fn text(f: &Fixture, cx: &mut TestAppContext) -> String {
    let composer = composer(f, cx);
    cx.update(|cx| composer.read(cx).text(cx))
}

/// Click the box and type `text`.
fn type_in(f: &Fixture, cx: &mut TestAppContext, text: &str) {
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.click("composer-input", cx);
        window.input(text, cx);
    })
    .unwrap();
    cx.run_until_parked();
}

fn press(f: &Fixture, cx: &mut TestAppContext, key: &str) {
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.press(key, cx);
    })
    .unwrap();
    cx.run_until_parked();
}

fn shows(f: &Fixture, cx: &mut TestAppContext, id: &'static str) -> bool {
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.try_find(id).is_some_and(|e| e.visible())
    })
    .unwrap()
}

#[gpui_kit::test]
fn slash_suggests_commands_and_tab_takes_one(cx: &mut TestAppContext) {
    let f = open(cx, snapshot(), |_| {});
    type_in(&f, cx, "/jo");
    assert!(shows(&f, cx, "suggestion-command-join"));
    press(&f, cx, "tab");
    assert_eq!(text(&f, cx), "/jo");
    assert!(shows(&f, cx, "command-editor"));
    assert!(!shows(&f, cx, "composer-suggestions"));
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        let found = window.find("command-editor-hint");
        let label = found.label().unwrap_or_default();
        assert!(label.contains("Choose or enter a channel."), "{label}");
    })
    .unwrap();
    press(&f, cx, "escape");
    assert!(!shows(&f, cx, "command-editor"));
    assert_eq!(text(&f, cx), "/jo");
}

#[gpui_kit::test]
fn arrows_move_through_suggestions_and_escape_hides_them(cx: &mut TestAppContext) {
    let f = open(cx, snapshot(), |_| {});
    // In a channel, as a plain member: /deop and /devoice.
    type_in(&f, cx, "/de");
    assert!(shows(&f, cx, "suggestion-command-deop"));
    assert!(shows(&f, cx, "suggestion-command-devoice"));
    press(&f, cx, "down");
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find("suggestion-command-devoice").selected(),
            Some(true)
        );
        assert_eq!(
            window.find("suggestion-command-deop").selected(),
            Some(false)
        );
    })
    .unwrap();
    press(&f, cx, "enter");
    assert!(shows(&f, cx, "command-editor"));
    press(&f, cx, "escape");
    assert_eq!(text(&f, cx), "/de");
    type_in(&f, cx, "voic");
    // `/devoic`: one suggestion, hidden by Escape until the text changes.
    assert!(shows(&f, cx, "suggestion-command-devoice"));
    press(&f, cx, "escape");
    assert!(!shows(&f, cx, "composer-suggestions"));
    type_in(&f, cx, "e");
    assert!(shows(&f, cx, "composer-suggestions"));
}

#[gpui_kit::test]
fn guided_join_validates_fields_and_sends_the_key_without_drafting_it(cx: &mut TestAppContext) {
    let f = open(cx, snapshot(), |server| {
        server.route(
            "POST",
            "/api/v1/conversations",
            201,
            json!({"id":"c-rust","networkId":"n1","kind":"channel","name":"rust","joined":true}),
        );
    });
    type_in(&f, cx, "/j");
    press(&f, cx, "space");
    assert!(shows(&f, cx, "command-editor"));
    press(&f, cx, "enter");
    assert!(shows(&f, cx, "command-editor-error"));
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.input("#rust", cx);
        window.click("command-argument-key", cx);
        window.input("sample-key", cx);
    })
    .unwrap();
    assert_eq!(text(&f, cx), "/j");
    press(&f, cx, "enter");
    let post = f
        .server
        .wait_for("POST", "/api/v1/conversations", WAIT)
        .json();
    assert_eq!(post["name"], "#rust");
    assert_eq!(post["key"], "sample-key");
    wait(cx, "the submitted draft to clear", |cx| {
        ircord_app::features::drafts::text(&f.session, "c-general", cx).is_empty()
    });
}

#[gpui_kit::test]
fn guided_nick_suggestions_can_be_chosen_before_running_the_command(cx: &mut TestAppContext) {
    let f = open(cx, snapshot(), |_| {});
    type_in(&f, cx, "/kick");
    press(&f, cx, "enter");
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.input("bo", cx);
    })
    .unwrap();
    assert!(shows(&f, cx, "command-option-bob"));
    press(&f, cx, "tab");
    assert!(!shows(&f, cx, "command-editor-suggestions"));
    cx.update_window(f.window, |_, window, cx| {
        window.input("Testing", cx);
    })
    .unwrap();
    press(&f, cx, "enter");
    let sent = f
        .server
        .wait_for("POST", "/api/v1/networks/n1/commands", WAIT)
        .json();
    assert_eq!(sent["line"], "KICK #general bob :Testing");
    wait(cx, "the submitted draft to clear", |cx| {
        ircord_app::features::drafts::text(&f.session, "c-general", cx).is_empty()
    });
}

#[gpui_kit::test]
fn nicknames_in_command_arguments_complete_as_the_at_list_does(cx: &mut TestAppContext) {
    let f = open(cx, snapshot(), |_| {});
    type_in(&f, cx, "/kick b");
    assert!(shows(&f, cx, "suggestion-person-bob"));
    press(&f, cx, "enter");
    assert_eq!(text(&f, cx), "/kick bob ");
}

#[gpui_kit::test]
fn invite_leaves_out_the_people_already_in_the_channel(cx: &mut TestAppContext) {
    // #general is the channel you are in, and bob is in it.
    let f = open(cx, snapshot(), |_| {});
    type_in(&f, cx, "/invite b");
    assert!(!shows(&f, cx, "suggestion-person-bob"));
    assert!(!shows(&f, cx, "composer-suggestions"));
}

#[gpui_kit::test]
fn people_and_channels_complete_with_the_sigil(cx: &mut TestAppContext) {
    let f = open(cx, snapshot(), |_| {});
    type_in(&f, cx, "hi @b");
    assert!(shows(&f, cx, "suggestion-person-bob"));
    press(&f, cx, "tab");
    assert_eq!(text(&f, cx), "hi @bob ");
    type_in(&f, cx, "see #ra");
    assert!(shows(&f, cx, "suggestion-channel-random"));
    press(&f, cx, "enter");
    assert_eq!(text(&f, cx), "hi @bob see #random ");
}

#[gpui_kit::test]
fn tab_completes_a_nick_and_adds_the_at(cx: &mut TestAppContext) {
    let f = open(cx, snapshot(), |_| {});
    type_in(&f, cx, "ev");
    press(&f, cx, "tab");
    assert_eq!(text(&f, cx), "@eve: ");
    type_in(&f, cx, "and bo");
    press(&f, cx, "tab");
    assert_eq!(text(&f, cx), "@eve: and @bob ");
}

#[gpui_kit::test]
fn join_opens_the_channel_it_joined(cx: &mut TestAppContext) {
    let f = open(cx, snapshot(), |server| {
        server.route(
            "POST",
            "/api/v1/conversations",
            200,
            json!({"id": "c-rust", "networkId": "n1", "kind": "channel", "name": "rust",
                   "joined": true, "memberIds": [], "memberModes": {}}),
        );
    });
    type_in(&f, cx, "/join #rust");
    press(&f, cx, "enter");
    let request = f.server.wait_for("POST", "/api/v1/conversations", WAIT);
    assert_eq!(request.json(), json!({"networkId": "n1", "name": "#rust"}));
    wait(cx, "#rust to open", |cx| {
        f.session.read(cx).conversation.as_deref() == Some("c-rust")
    });
    assert_eq!(text(&f, cx), "");
}

#[gpui_kit::test]
fn raw_commands_send_their_line_and_clear_the_box(cx: &mut TestAppContext) {
    let f = open(cx, snapshot(), |_| {});
    type_in(&f, cx, "/topic Fresh news");
    press(&f, cx, "enter");
    let request = f
        .server
        .wait_for("POST", "/api/v1/networks/n1/commands", WAIT);
    assert_eq!(request.json()["line"], "TOPIC #general :Fresh news");
    assert_eq!(text(&f, cx), "");
}

#[gpui_kit::test]
fn a_typed_status_command_updates_only_the_network_status_fields(cx: &mut TestAppContext) {
    const PROFILE: &str = "/api/v1/networks/n1/profile";
    let f = open(cx, snapshot(), |server| {
        server.route_fn("PATCH", PROFILE, |request| {
            let status = request.json();
            Reply::json(
                200,
                json!({
                    "id": "u-me", "account": "me", "nick": "me", "status": "online",
                    "statusText": status["statusText"], "statusEmoji": status["statusEmoji"],
                    "statusExpiresAt": status["statusExpiresAt"],
                }),
            )
        });
    });
    type_in(&f, cx, "/status ☕ At lunch --for 1h");
    press(&f, cx, "enter");
    let update = f.server.wait_for("PATCH", PROFILE, WAIT).json();
    let deadline = update["statusExpiresAt"].as_str().unwrap();
    let deadline = chrono::DateTime::parse_from_rfc3339(deadline).unwrap();
    let minutes = (deadline.to_utc() - chrono::Utc::now()).num_minutes();
    assert!((59..=60).contains(&minutes));
    assert_eq!(
        update,
        json!({
            "statusText": "At lunch", "statusEmoji": "☕", "statusExpiresAt": update["statusExpiresAt"],
        })
    );
    wait(cx, "the sent status command to clear", |cx| {
        ircord_app::features::message_actions::box_text(cx).is_empty()
    });
    type_in(&f, cx, "/status clear");
    press(&f, cx, "enter");
    let clear = f.server.wait_for_nth("PATCH", PROFILE, 2, WAIT).json();
    assert_eq!(
        clear,
        json!({"statusText": "", "statusEmoji": "", "statusExpiresAt": ""})
    );
}

#[gpui_kit::test]
fn an_invalid_status_command_stays_in_the_composer(cx: &mut TestAppContext) {
    let f = open(cx, snapshot(), |_| {});
    let invalid = "/status Busy --for";
    type_in(&f, cx, invalid);
    press(&f, cx, "enter");
    assert_eq!(text(&f, cx), invalid);
    assert!(
        f.server
            .requests_to("PATCH", "/api/v1/networks/n1/profile")
            .is_empty()
    );
}

#[gpui_kit::test]
fn the_status_form_stays_in_the_composer_and_spends_draft_on_save(cx: &mut TestAppContext) {
    let f = open(cx, snapshot(), |server| {
        server.route_fn("PATCH", "/api/v1/networks/n1/profile", |request| {
            let status = request.json();
            Reply::json(
                200,
                json!({
                    "id": "u-me", "account": "me", "nick": "me", "status": "online",
                    "statusText": status["statusText"], "statusEmoji": status["statusEmoji"],
                    "statusExpiresAt": status["statusExpiresAt"],
                }),
            )
        });
    });
    type_in(&f, cx, "/status");
    press(&f, cx, "enter");
    assert!(shows(&f, cx, "command-editor"));
    assert!(!shows(&f, cx, "status-editor"));
    press(&f, cx, "escape");
    assert!(!shows(&f, cx, "command-editor"));
    assert_eq!(text(&f, cx), "/status");

    press(&f, cx, "enter");
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.input("grinning", cx);
        window.render_frame(cx);
        window.click("command-option-😀", cx);
        window.click("command-argument-message", cx);
        window.input("At lunch", cx);
        window.click("command-status-expiry", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        let hour = window.find_by_label("1 hour").bounds().center();
        window.pointer_down(hour, MouseButton::Left, cx);
        window.pointer_up(hour, MouseButton::Left, cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.click("command-editor-run", cx);
    })
    .unwrap();
    let status = f
        .server
        .wait_for("PATCH", "/api/v1/networks/n1/profile", WAIT)
        .json();
    assert_eq!(status["statusEmoji"], "😀");
    assert_eq!(status["statusText"], "At lunch");
    assert!(
        status["statusExpiresAt"]
            .as_str()
            .is_some_and(|s| !s.is_empty())
    );
    wait(cx, "the command to clear after saving", |cx| {
        ircord_app::features::message_actions::box_text(cx).is_empty()
    });
    assert!(!shows(&f, cx, "command-editor"));
}

#[gpui_kit::test]
fn a_failed_command_comes_back_into_the_box(cx: &mut TestAppContext) {
    let f = open(cx, snapshot(), |server| {
        server.route_reply(
            "POST",
            "/api/v1/networks/*/commands",
            Reply::problem(409, "not now"),
        );
    });
    type_in(&f, cx, "/nick newme");
    press(&f, cx, "enter");
    f.server
        .wait_for("POST", "/api/v1/networks/n1/commands", WAIT);
    wait(cx, "the command back in the box", |cx| {
        let composer = match f.view.read(cx).screen() {
            Screen::Workspace(w) => w.read(cx).composer.clone(),
            _ => return false,
        };
        composer.read(cx).text(cx) == "/nick newme"
    });
}

#[gpui_kit::test]
fn a_command_that_cannot_run_stays_in_the_box(cx: &mut TestAppContext) {
    let f = open(cx, snapshot(), |_| {});
    type_in(&f, cx, "/frobnicate now");
    press(&f, cx, "enter");
    assert_eq!(text(&f, cx), "/frobnicate now");
    assert!(
        f.server
            .requests_to("POST", "/api/v1/networks/n1/commands")
            .is_empty()
    );
}

#[gpui_kit::test]
fn view_commands_go_to_the_view_that_handles_them(cx: &mut TestAppContext) {
    let f = open(cx, snapshot(), |_| {});
    let seen: Rc<RefCell<Vec<(String, String)>>> = Rc::default();
    let s = seen.clone();
    cx.update(|cx| {
        commands::on_view(cx, View::ChannelList, move |r, _, _| {
            s.borrow_mut().push((r.network.clone(), r.args.clone()));
        })
    });
    type_in(&f, cx, "/list rust");
    press(&f, cx, "enter");
    assert_eq!(*seen.borrow(), [("n1".to_string(), "rust".to_string())]);
    assert_eq!(text(&f, cx), "");
    // Nobody handles /banlist yet: it is not available, and the box clears.
    type_in(&f, cx, "/banlist");
    press(&f, cx, "enter");
    assert_eq!(text(&f, cx), "");
}

#[gpui_kit::test]
fn ignore_edits_the_ignore_list_read_at_start(cx: &mut TestAppContext) {
    let settings = json!({"allDevicesDelay": 0, "autoAwayDelay": 0, "channelDefault": "all",
        "directDefault": "all", "lastDeviceDelay": 0, "levels": {}, "ignore": ["eve"],
        "quietHours": {"enabled": false, "start": "22:00", "end": "07:00", "timezone": "UTC"}});
    let stored = settings.clone();
    let f = open(cx, snapshot(), move |server| {
        server.route("GET", "/api/v1/notifications/settings", 200, stored);
        server.route_fn("PUT", "/api/v1/notifications/settings", |r| {
            Reply::json(200, r.json())
        });
    });
    // The settings are read at start (P1 hides ignored people's messages).
    f.server
        .wait_for("GET", "/api/v1/notifications/settings", WAIT);
    wait(cx, "the settings", |cx| {
        let session = f.session.read(cx);
        let w = session.client().unwrap().read();
        w.documents.notification_settings.is_loaded()
    });
    type_in(&f, cx, "/ignore bob");
    press(&f, cx, "enter");
    assert_eq!(text(&f, cx), "");
    let put = f
        .server
        .wait_for("PUT", "/api/v1/notifications/settings", WAIT);
    assert_eq!(put.json()["ignore"], json!(["eve", "bob"]));
}

#[gpui_kit::test]
fn a_moderated_channel_says_why_and_keeps_the_message(cx: &mut TestAppContext) {
    let mut s = snapshot();
    s.conversations[0].modes = "m".into();
    let f = open(cx, s, |_| {});
    assert!(shows(&f, cx, "composer-send-reason"));
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        let found = window.find("composer-send-reason");
        let label = found.label().unwrap_or_default();
        assert!(label.contains("moderated"), "{label}");
    })
    .unwrap();
    type_in(&f, cx, "hello");
    press(&f, cx, "enter");
    assert_eq!(text(&f, cx), "hello");
    cx.update(|cx| {
        let session = f.session.read(cx);
        assert!(
            session
                .client()
                .unwrap()
                .read()
                .pending_in("c-general")
                .is_empty()
        );
    });
}

#[gpui_kit::test]
fn text_over_the_line_limit_says_why_and_is_kept(cx: &mut TestAppContext) {
    let f = open(cx, snapshot(), |_| {});
    let long = "x".repeat(600);
    type_in(&f, cx, &long);
    assert!(shows(&f, cx, "composer-limit"));
    press(&f, cx, "enter");
    assert_eq!(text(&f, cx), long);
}

#[gpui_kit::test]
fn formatting_keys_wrap_the_selection(cx: &mut TestAppContext) {
    let f = open(cx, snapshot(), |_| {});
    type_in(&f, cx, "hi");
    press(&f, cx, "ctrl-a");
    press(&f, cx, "ctrl-b");
    assert_eq!(text(&f, cx), "**hi**");
    press(&f, cx, "ctrl-u");
    assert_eq!(text(&f, cx), "**__hi__**");
}

#[gpui_kit::test]
fn typing_anywhere_goes_into_the_box(cx: &mut TestAppContext) {
    let f = open(cx, snapshot(), |_| {});
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.input("hey", cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(text(&f, cx), "hey");
}

//! Tests for package CH1: the join dialog with the channel guide and the
//! channel search, creating a channel, and the one-time guide notice.

use std::time::Duration;

use gpui_kit::test::{TestQueryExt, TestWindowExt};
use gpui_kit::{AppContext, TestAppContext};
use ircord_app::features::shortcuts::JoinChannel;
use ircord_app::features::{create_channel, join};
use ircord_client::testing::{CannedServer, Reply};
use serde_json::{Value, json};

use crate::fixture::{Fixture, bootstrap, open_with, snapshot, wait};

const WAIT: Duration = crate::fixture::HANG_GUARD;

fn conversation(id: &str, name: &str) -> Value {
    json!({"id": id, "networkId": "n1", "kind": "channel", "name": name,
           "favorite": false, "joined": true, "memberIds": [], "memberModes": {},
           "modes": "", "topic": "", "unreadCount": 0, "mentionCount": 0})
}

fn listing(server: &CannedServer) {
    server.route_fn("GET", "/api/v1/networks/n1/channels", |request| {
        let q = request.query_param("q").unwrap_or_default();
        let all = [
            ("#rust", "\x02Crabs\x02 welcome", 12),
            ("#general", "Welcome", 2),
            ("#go", "", 4),
        ];
        let items: Vec<Value> = all
            .iter()
            .filter(|(name, _, _)| name.contains(&q))
            .map(|(name, topic, users)| json!({"name": name, "topic": topic, "users": users}))
            .collect();
        Reply::json(200, json!({ "items": items }))
    });
}

fn no_guide(server: &CannedServer) {
    server.route(
        "GET",
        "/api/v1/networks/n1/channel-guide",
        200,
        json!({"available": false, "entries": [], "groups": [], "revision": 0}),
    );
}

fn guide(server: &CannedServer, revision: i64) {
    server.route(
        "GET",
        "/api/v1/networks/n1/channel-guide",
        200,
        json!({"available": true, "revision": revision,
        "groups": [{"name": "Code"}],
        "entries": [
            {"channel": "#general", "state": "everyone", "nsfw": false, "members": 2},
            {"channel": "#rust", "state": "suggested", "group": "Code", "nsfw": false,
             "description": "All things Rust", "members": 12},
            {"channel": "#late", "state": "listed", "nsfw": true}
        ]}),
    );
}

fn open(cx: &mut TestAppContext, setup: impl FnOnce(&CannedServer)) -> Fixture {
    let f = open_with(cx, true, |server| {
        listing(server);
        setup(server);
    });
    bootstrap(&f, cx);
    f
}

fn click(f: &Fixture, cx: &mut TestAppContext, id: &'static str) {
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.click(id, cx);
    })
    .unwrap();
    cx.run_until_parked();
}

/// Replace the text of the input `id`.
fn type_into(f: &Fixture, cx: &mut TestAppContext, id: &'static str, text: &str) {
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.click(id, cx);
        window.press("ctrl-a", cx);
        window.press("backspace", cx);
        window.input(text, cx);
    })
    .unwrap();
    cx.run_until_parked();
}

fn showing(f: &Fixture, cx: &mut TestAppContext, id: &'static str) -> bool {
    cx.run_until_parked();
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.try_find(id).is_some_and(|e| e.visible())
    })
    .unwrap()
}

fn wait_showing(f: &Fixture, cx: &mut TestAppContext, id: &'static str, shown: bool) {
    let deadline = std::time::Instant::now() + WAIT;
    while showing(f, cx, id) != shown {
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for {id} to be {}",
            if shown { "shown" } else { "gone" }
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn label(f: &Fixture, cx: &mut TestAppContext, id: &'static str) -> Option<String> {
    cx.run_until_parked();
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.find(id).label().map(str::to_string)
    })
    .unwrap()
}

fn selected(f: &Fixture, cx: &mut TestAppContext) -> Option<String> {
    cx.update(|cx| f.session.read(cx).conversation.clone())
}

fn open_join(f: &Fixture, cx: &mut TestAppContext, filter: &str) {
    let session = f.session.clone();
    let filter = filter.to_string();
    cx.update_window(f.window, |_, window, cx| {
        join::open(&session, None, &filter, window, cx)
    })
    .unwrap();
    wait_showing(f, cx, "join-dialog", true);
}

#[gpui_kit::test]
fn the_join_key_opens_the_search_and_a_row_joins_its_channel(cx: &mut TestAppContext) {
    let f = open(cx, |server| {
        no_guide(server);
        server.route(
            "POST",
            "/api/v1/conversations",
            200,
            conversation("c-rust", "rust"),
        );
    });
    cx.update_window(f.window, |_, window, cx| {
        window.dispatch_action(Box::new(JoinChannel), cx)
    })
    .unwrap();
    wait_showing(&f, cx, "join-dialog", true);
    // No guide on this network: no tabs, the search straight away.
    wait_showing(&f, cx, "join-channel-rust", true);
    assert!(!showing(&f, cx, "join-views"));
    assert!(showing(&f, cx, "join-channel-general"));
    assert!(showing(&f, cx, "join-channel-go"));
    assert_eq!(label(&f, cx, "join-channel-rust").as_deref(), Some("#rust"));
    assert_eq!(
        label(&f, cx, "join-channel-rust-users").as_deref(),
        Some("Users: 12")
    );

    // Typing filters the list on the server.
    type_into(&f, cx, "join-name", "#ru");
    f.server
        .wait_for_nth("GET", "/api/v1/networks/n1/channels", 2, WAIT);
    wait_showing(&f, cx, "join-channel-go", false);
    let asked = f.server.requests_to("GET", "/api/v1/networks/n1/channels");
    assert_eq!(
        asked.last().unwrap().query_param("q").as_deref(),
        Some("ru")
    );

    click(&f, cx, "join-channel-rust");
    let sent = f.server.wait_for("POST", "/api/v1/conversations", WAIT);
    assert_eq!(sent.json(), json!({"networkId": "n1", "name": "#rust"}));
    wait(cx, "#rust to open", |cx| {
        f.session.read(cx).conversation.as_deref() == Some("c-rust")
    });
    wait_showing(&f, cx, "join-dialog", false);
}

#[gpui_kit::test]
fn a_channel_you_are_in_opens_without_joining_and_a_typed_name_joins(cx: &mut TestAppContext) {
    let f = open(cx, |server| {
        no_guide(server);
        server.route_fn("POST", "/api/v1/conversations", |r| {
            if r.json()["name"] == "#new" {
                Reply::json(200, conversation("c-new", "new"))
            } else {
                Reply::problem(403, "Cannot join channel (+i)")
            }
        });
    });
    f.session
        .update(cx, |s, cx| s.select_conversation("c-random", cx));
    open_join(&f, cx, "");
    wait_showing(&f, cx, "join-channel-general", true);
    click(&f, cx, "join-channel-general");
    wait_showing(&f, cx, "join-dialog", false);
    assert_eq!(selected(&f, cx).as_deref(), Some("c-general"));
    assert!(
        f.server
            .requests_to("POST", "/api/v1/conversations")
            .is_empty()
    );

    // The server's refusal shows in the dialog.
    open_join(&f, cx, "secret");
    type_into(&f, cx, "join-name", "secret");
    click(&f, cx, "join-submit");
    wait_showing(&f, cx, "join-error", true);
    assert_eq!(
        label(&f, cx, "join-error").as_deref(),
        Some("Cannot join channel (+i)")
    );
    // Enter joins what was typed.
    type_into(&f, cx, "join-name", "new");
    cx.update_window(f.window, |_, window, cx| window.press("enter", cx))
        .unwrap();
    let sent = f
        .server
        .wait_for_nth("POST", "/api/v1/conversations", 2, WAIT);
    assert_eq!(sent.json()["name"], "#new");
    wait(cx, "#new to open", |cx| {
        f.session.read(cx).conversation.as_deref() == Some("c-new")
    });
    wait_showing(&f, cx, "join-dialog", false);
}

#[gpui_kit::test]
fn the_guide_shows_featured_channels_and_joins_the_ticked_ones(cx: &mut TestAppContext) {
    let f = open(cx, |server| {
        guide(server, 1);
        server.route_fn("POST", "/api/v1/networks/n1/channel-guide/join", |r| {
            let asked = r.json()["channels"].clone();
            if asked.as_array().unwrap().iter().any(|c| c == "#late") {
                Reply::json(
                    200,
                    json!({"joined": [], "queued": [],
                           "failed": [{"channel": "#late", "reason": "banned"}]}),
                )
            } else {
                Reply::json(
                    200,
                    json!({"joined": [conversation("c-rust", "rust")], "failed": [], "queued": []}),
                )
            }
        });
    });
    open_join(&f, cx, "");
    wait_showing(&f, cx, "join-views", true);
    assert!(showing(&f, cx, "join-guide"));
    assert!(showing(&f, cx, "join-section-everyone"));
    assert!(showing(&f, cx, "join-section-code"));
    assert!(showing(&f, cx, "join-section-more"));
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        // Everyone's channel (and one you are in) is ticked and locked.
        let general = window.find("join-guide-general");
        assert_eq!(general.checked(), Some(true));
        assert_eq!(general.disabled(), Some(true));
        assert_eq!(window.find("join-guide-rust").checked(), Some(false));
    })
    .unwrap();
    assert_eq!(
        label(&f, cx, "join-guide-join").as_deref(),
        Some("Join selected (0)")
    );

    // A channel that cannot be joined stays ticked with the error.
    click(&f, cx, "join-guide-late");
    click(&f, cx, "join-guide-join");
    wait_showing(&f, cx, "join-error", true);
    assert!(label(&f, cx, "join-error").unwrap().contains("#late"));
    click(&f, cx, "join-guide-late");
    click(&f, cx, "join-guide-rust");
    assert_eq!(
        label(&f, cx, "join-guide-join").as_deref(),
        Some("Join selected (1)")
    );
    click(&f, cx, "join-guide-join");
    let sent = f
        .server
        .wait_for_nth("POST", "/api/v1/networks/n1/channel-guide/join", 2, WAIT);
    assert_eq!(sent.json(), json!({"channels": ["#rust"]}));
    wait(cx, "#rust to open", |cx| {
        f.session.read(cx).conversation.as_deref() == Some("c-rust")
    });
    wait_showing(&f, cx, "join-dialog", false);

    // `/list` with a filter opens All channels instead.
    open_join(&f, cx, "#ru");
    wait_showing(&f, cx, "join-channel-rust", true);
    assert!(!showing(&f, cx, "join-guide"));
    click(&f, cx, "join-view-featured");
    wait_showing(&f, cx, "join-guide", true);
}

fn creatable(server: &CannedServer) {
    let mut snap = snapshot();
    snap.networks[0]
        .capabilities
        .push("labeled-response".into());
    server.snapshot(&snap);
    server.route_fn("GET", "/api/v1/networks/n1/channels/availability", |r| {
        let status = if r.query_param("name").as_deref() == Some("#general") {
            "unavailable"
        } else {
            "available"
        };
        Reply::json(200, json!({ "status": status }))
    });
}

fn open_create(f: &Fixture, cx: &mut TestAppContext) {
    let session = f.session.clone();
    cx.update_window(f.window, |_, window, cx| {
        create_channel::open(&session, "n1", window, cx)
    })
    .unwrap();
    wait_showing(f, cx, "create-channel", true);
}

#[gpui_kit::test]
fn a_channel_is_created_from_a_template_after_the_name_check(cx: &mut TestAppContext) {
    let f = open(cx, |server| {
        creatable(server);
        server.route(
            "POST",
            "/api/v1/networks/n1/channels",
            200,
            json!({"conversation": conversation("c-team", "team"), "registered": true,
                   "configured": true, "warnings": []}),
        );
    });
    open_create(&f, cx);
    assert!(!showing(&f, cx, "create-channel-unsupported"));
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        // The private template: invite only, secret, no outside messages, topic lock.
        assert_eq!(window.find("create-mode-i").checked(), Some(true));
        assert_eq!(window.find("create-mode-m").checked(), Some(false));
        assert_eq!(window.find("create-channel-submit").disabled(), Some(true));
    })
    .unwrap();

    type_into(&f, cx, "create-channel-name", "general");
    wait(cx, "the name check", |_| true);
    let deadline = std::time::Instant::now() + WAIT;
    while label(&f, cx, "create-channel-name-note").as_deref()
        != Some("This name is unavailable. Choose another name.")
    {
        assert!(
            std::time::Instant::now() < deadline,
            "the name check answers"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(
        cx.update_window(f.window, |_, window, cx| {
            window.render_frame(cx);
            window.find("create-channel-submit").disabled()
        })
        .unwrap(),
        Some(true)
    );

    type_into(&f, cx, "create-channel-name", "#team");
    let deadline = std::time::Instant::now() + WAIT;
    while !label(&f, cx, "create-channel-name-note")
        .unwrap_or_default()
        .starts_with("This name appears available")
    {
        assert!(
            std::time::Instant::now() < deadline,
            "the name check answers"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    type_into(&f, cx, "create-channel-topic", "Plans");
    click(&f, cx, "create-channel-template-announcements");
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("create-mode-m").checked(), Some(true));
        assert_eq!(window.find("create-mode-i").checked(), Some(false));
    })
    .unwrap();
    click(&f, cx, "create-mode-s");
    click(&f, cx, "create-channel-submit");
    let sent = f
        .server
        .wait_for("POST", "/api/v1/networks/n1/channels", WAIT);
    assert_eq!(
        sent.json(),
        json!({"name": "#team", "topic": "Plans", "flags": "+mnts-ip", "modeParams": {}})
    );
    wait(cx, "#team to open", |cx| {
        f.session.read(cx).conversation.as_deref() == Some("c-team")
    });
    wait_showing(&f, cx, "create-channel", false);
    cx.update(|cx| {
        let session = f.session.read(cx);
        assert!(
            session
                .client()
                .unwrap()
                .read()
                .conversation("c-team")
                .is_some()
        );
    });
}

#[gpui_kit::test]
fn a_partly_set_up_channel_lists_what_went_wrong(cx: &mut TestAppContext) {
    let f = open(cx, |server| {
        creatable(server);
        server.route(
            "POST",
            "/api/v1/networks/n1/channels",
            200,
            json!({"conversation": conversation("c-team", "team"), "registered": false,
                   "configured": false, "warnings": ["Could not register #team"]}),
        );
    });
    open_create(&f, cx);
    type_into(&f, cx, "create-channel-name", "team");
    let deadline = std::time::Instant::now() + WAIT;
    while label(&f, cx, "create-channel-name-note")
        .unwrap_or_default()
        .starts_with("Checking")
    {
        assert!(
            std::time::Instant::now() < deadline,
            "the name check answers"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    click(&f, cx, "create-channel-submit");
    let sent = f
        .server
        .wait_for("POST", "/api/v1/networks/n1/channels", WAIT);
    assert_eq!(sent.json()["name"], "#team");
    wait_showing(&f, cx, "create-channel-partial", true);
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.find("create-channel-warnings").visible());
        assert!(
            window
                .try_find_by_label("Could not register #team")
                .is_some()
        );
        // The form gave way to the answer: only Open channel is left.
        assert!(window.try_find("create-channel-submit").is_none());
    })
    .unwrap();
    click(&f, cx, "create-channel-open");
    wait(cx, "#team to open", |cx| {
        f.session.read(cx).conversation.as_deref() == Some("c-team")
    });
    wait_showing(&f, cx, "create-channel", false);
}

#[gpui_kit::test]
fn without_labeled_response_creating_is_explained_and_typing_asks_before_closing(
    cx: &mut TestAppContext,
) {
    let f = open(cx, no_guide);
    open_join(&f, cx, "");
    // The join dialog hands over to the create dialog.
    click(&f, cx, "join-create");
    wait_showing(&f, cx, "create-channel", true);
    assert!(!showing(&f, cx, "join-dialog"));
    assert!(showing(&f, cx, "create-channel-unsupported"));
    type_into(&f, cx, "create-channel-name", "team");
    assert_eq!(
        cx.update_window(f.window, |_, window, cx| {
            window.render_frame(cx);
            window.find("create-channel-submit").disabled()
        })
        .unwrap(),
        Some(true)
    );
    // Cancel with something typed asks first.
    click(&f, cx, "create-channel-cancel");
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.find_by_label("Discard").visible());
    })
    .unwrap();
}

#[gpui_kit::test]
fn the_guide_notice_shows_once_and_opens_featured(cx: &mut TestAppContext) {
    let f = open(cx, |server| {
        guide(server, 3);
        server.route(
            "GET",
            "/api/v1/app/preferences",
            200,
            json!({"preferences": {}}),
        );
        server.route_fn("PUT", "/api/v1/app/preferences", |r| {
            Reply::json(200, json!({"preferences": r.json()["preferences"]}))
        });
    });
    wait_showing(&f, cx, "channel-guide-notice", true);
    assert_eq!(
        label(&f, cx, "channel-guide-notice").as_deref(),
        Some("Browse this server’s channels")
    );
    click(&f, cx, "channel-guide-notice-open");
    wait_showing(&f, cx, "join-guide", true);
    let saved = f.server.wait_for("PUT", "/api/v1/app/preferences", WAIT);
    assert_eq!(saved.json()["preferences"]["channelGuideSeen"], true);
    wait_showing(&f, cx, "channel-guide-notice", false);
}

#[gpui_kit::test]
fn the_guide_notice_waits_for_a_saved_guide_and_can_be_dismissed(cx: &mut TestAppContext) {
    let f = open(cx, |server| {
        guide(server, 0);
        server.route(
            "GET",
            "/api/v1/app/preferences",
            200,
            json!({"preferences": {}}),
        );
    });
    f.server
        .wait_for("GET", "/api/v1/networks/n1/channel-guide", WAIT);
    std::thread::sleep(Duration::from_millis(50));
    assert!(!showing(&f, cx, "channel-guide-notice"));

    // Owners saved a guide (another sign-in reads it fresh).
    let f = open(cx, |server| {
        guide(server, 2);
        server.route(
            "GET",
            "/api/v1/app/preferences",
            200,
            json!({"preferences": {}}),
        );
        server.route_fn("PUT", "/api/v1/app/preferences", |r| {
            Reply::json(200, json!({"preferences": r.json()["preferences"]}))
        });
    });
    wait_showing(&f, cx, "channel-guide-notice", true);
    click(&f, cx, "channel-guide-notice-dismiss");
    let saved = f.server.wait_for("PUT", "/api/v1/app/preferences", WAIT);
    assert_eq!(saved.json()["preferences"]["channelGuideSeen"], true);
    wait_showing(&f, cx, "channel-guide-notice", false);
    assert!(!showing(&f, cx, "join-dialog"));
}

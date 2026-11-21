//! Tests for package P1 against the canned server: the member list (counts,
//! filter, status under names, bot marks, hiding it), the profile card
//! with its server lookup and "ask their client", the member menu
//! (mention, message, ignore), ignored people hidden from the timeline,
//! your status from your own menu, an empty direct conversation and the
//! people picker.

use std::time::Duration;

use gpui_kit::test::{TestEventExt, TestQueryExt, TestWindowExt};
use gpui_kit::{AppContext, Entity, MouseButton, TestAppContext};
use ircord_app::Screen;
use ircord_app::composer::ComposerView;
use ircord_app::timeline::rows::Row;
use ircord_client::contract::*;
use ircord_client::testing::{CannedServer, Reply};
use serde_json::{Value, json};

use crate::fixture::{Fixture, open_with, snapshot, wait};

const WAIT: Duration = Duration::from_secs(5);

fn settings(ignore: &[&str]) -> Value {
    json!({"allDevicesDelay": 0, "autoAwayDelay": 0, "channelDefault": "all",
        "directDefault": "all", "lastDeviceDelay": 0, "levels": {}, "ignore": ignore,
        "quietHours": {"enabled": false, "start": "22:00", "end": "07:00", "timezone": "UTC"}})
}

/// The fixture's workspace; bob has a status and a known address, eve is
/// a bot on a phone.
fn workspace() -> Snapshot {
    let mut s = snapshot();
    for m in &mut s.members {
        if m.id == "m-bob" {
            m.status_emoji = Some("🌴".into());
            m.status_text = Some("On holiday".into());
            m.username = Some("b".into());
            m.host = Some("example.org".into());
            m.bio = "Sails a lot.".into();
        }
        if m.id == "m-eve" {
            m.role = MemberRole::Bot;
            m.device = Some(MemberDevice::Phone);
        }
    }
    s
}

fn open(cx: &mut TestAppContext, s: Snapshot, setup: impl FnOnce(&CannedServer)) -> Fixture {
    let f = open_with(cx, true, |server| {
        server.snapshot(&s);
        // Kept like the server keeps them: a read after a reconnect gets
        // what was saved.
        let stored = std::sync::Arc::new(std::sync::Mutex::new(settings(&[])));
        let read = stored.clone();
        server.route_fn("GET", "/api/v1/notifications/settings", move |_| {
            Reply::json(200, read.lock().unwrap().clone())
        });
        server.route_fn("PUT", "/api/v1/notifications/settings", move |r| {
            *stored.lock().unwrap() = r.json();
            Reply::json(200, r.json())
        });
        server.route(
            "POST",
            "/api/v1/networks/*/commands",
            202,
            json!({"clientId": "x"}),
        );
        setup(server);
    });
    wait(cx, "#general and the ignore list", |cx| {
        let session = f.session.read(cx);
        let Some(client) = session.client() else {
            return false;
        };
        let w = client.read();
        session.conversation.as_deref() == Some("c-general")
            && w.timelines.get("c-general").is_some_and(|t| t.loaded)
            && w.documents.notification_settings.is_loaded()
    });
    f
}

fn frame(
    f: &Fixture,
    cx: &mut TestAppContext,
    run: impl FnOnce(&mut gpui_kit::Window, &mut gpui_kit::App),
) {
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        run(window, cx);
    })
    .unwrap();
    cx.run_until_parked();
}

fn click(f: &Fixture, cx: &mut TestAppContext, id: &'static str) {
    frame(f, cx, |window, cx| window.click(id, cx));
}

fn click_label(f: &Fixture, cx: &mut TestAppContext, label: &str) {
    frame(f, cx, |window, cx| {
        let at = window.find_by_label(label).bounds().center();
        window.pointer_down(at, MouseButton::Left, cx);
        window.pointer_up(at, MouseButton::Left, cx);
    });
}

/// Right-click `id` and pick the menu entry labelled `label`.
fn menu(f: &Fixture, cx: &mut TestAppContext, id: &'static str, label: &str) {
    frame(f, cx, |window, cx| {
        window.hover(id, cx);
        window.right_click(id, cx);
    });
    click_label(f, cx, label);
}

fn type_text(f: &Fixture, cx: &mut TestAppContext, text: &str) {
    frame(f, cx, |window, cx| window.input(text, cx));
}

fn shows(f: &Fixture, cx: &mut TestAppContext, id: &'static str) -> bool {
    let mut found = false;
    frame(f, cx, |window, _| found = window.try_find(id).is_some());
    found
}

fn label(f: &Fixture, cx: &mut TestAppContext, id: &'static str) -> String {
    let mut text = String::new();
    frame(f, cx, |window, _| {
        text = window.find(id).label().unwrap_or_default().to_string()
    });
    text
}

/// Wait until `id` draws.
fn wait_for(f: &Fixture, cx: &mut TestAppContext, id: &'static str) {
    let deadline = std::time::Instant::now() + WAIT;
    while !shows(f, cx, id) {
        assert!(std::time::Instant::now() < deadline, "{id} never showed");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn composer(f: &Fixture, cx: &mut TestAppContext) -> Entity<ComposerView> {
    cx.update(|cx| match f.view.read(cx).screen() {
        Screen::Workspace(w) => w.read(cx).composer.clone(),
        _ => panic!("the workspace is not showing"),
    })
}

fn messages_shown(f: &Fixture, cx: &mut TestAppContext) -> usize {
    cx.update(|cx| match f.view.read(cx).screen() {
        Screen::Workspace(w) => w
            .read(cx)
            .timeline
            .read(cx)
            .rows()
            .iter()
            .filter(|r| matches!(r, Row::Message(_)))
            .count(),
        _ => 0,
    })
}

#[gpui_kit::test]
fn the_member_list_counts_filters_marks_and_hides(cx: &mut TestAppContext) {
    let mut s = workspace();
    // A long list gets a filter field.
    for i in 0..12 {
        let mut m = s.members[1].clone();
        m.id = format!("m-x{i}");
        m.nick = format!("extra{i}");
        m.role = MemberRole::Member;
        m.device = None;
        s.conversations[0].member_ids.push(m.id.clone());
        s.members.push(m);
    }
    let f = open(cx, s, |_| {});
    assert_eq!(label(&f, cx, "member-count"), "1 of 14 online");
    assert_eq!(label(&f, cx, "member-status-m-bob"), "🌴 On holiday");
    assert!(shows(&f, cx, "member-bot-m-eve"));
    assert!(shows(&f, cx, "member-device-m-eve"));

    click(&f, cx, "member-filter");
    type_text(&f, cx, "extra1");
    assert!(shows(&f, cx, "member-m-x1"));
    assert!(!shows(&f, cx, "member-m-bob"));
    type_text(&f, cx, "zzz");
    assert!(shows(&f, cx, "member-no-matches"));

    click(&f, cx, "hide-members");
    assert!(!shows(&f, cx, "member-list"));
    click(&f, cx, "members-toggle");
    assert!(shows(&f, cx, "member-list"));
}

#[gpui_kit::test]
fn the_card_looks_them_up_and_asks_their_client(cx: &mut TestAppContext) {
    let f = open(cx, workspace(), |server| {
        server.route(
            "GET",
            "/api/v1/networks/n1/whois",
            200,
            json!({"found": true, "nickname": "bob", "channels": ["@#general"], "secure": true,
                   "operator": true, "username": "b", "host": "example.org",
                   "server": "irc.example.org", "idleSeconds": 120}),
        );
        server.route_fn("POST", "/api/v1/networks/n1/ctcp", |r| {
            let command = r.json()["command"].as_str().unwrap_or_default().to_string();
            Reply::json(
                200,
                json!({"command": command, "answered": command != "TIME",
                       "reply": "HexChat 2.16", "latencyMs": 42}),
            )
        });
    });
    click(&f, cx, "member-m-bob");
    let whois = f.server.wait_for("GET", "/api/v1/networks/n1/whois", WAIT);
    assert_eq!(whois.query_param("nick").as_deref(), Some("bob"));
    wait_for(&f, cx, "profile-whois-0");
    assert_eq!(label(&f, cx, "profile-whois-0"), "Address: b@example.org");
    assert_eq!(label(&f, cx, "profile-whois-2"), "Last active: 2 min ago");
    assert!(shows(&f, cx, "profile-badge-irc-operator"));
    assert_eq!(label(&f, cx, "profile-status"), "🌴 On holiday");
    assert_eq!(label(&f, cx, "profile-bio"), "Sails a lot.");
    assert!(shows(&f, cx, "profile-channel-c-general"));

    click(&f, cx, "profile-ask-client");
    wait_for(&f, cx, "profile-ctcp-2");
    assert_eq!(
        f.server
            .requests_to("POST", "/api/v1/networks/n1/ctcp")
            .len(),
        3
    );
    assert_eq!(label(&f, cx, "profile-ctcp-0"), "Client: HexChat 2.16");
    assert_eq!(label(&f, cx, "profile-ctcp-1"), "Their time: No reply");
    assert_eq!(label(&f, cx, "profile-ctcp-2"), "Ping: 42 ms");
}

#[gpui_kit::test]
fn a_name_in_the_timeline_opens_the_card(cx: &mut TestAppContext) {
    let f = open(cx, workspace(), |server| {
        server.route(
            "GET",
            "/api/v1/networks/n1/whois",
            200,
            json!({"found": false, "nickname": "bob", "channels": [], "secure": false,
                   "operator": false}),
        );
    });
    click(&f, cx, "sender-1");
    wait_for(&f, cx, "profile-whois-absent");
    assert_eq!(label(&f, cx, "profile-name"), "bob");
}

#[gpui_kit::test]
fn the_member_menu_mentions_messages_and_ignores(cx: &mut TestAppContext) {
    let f = open(cx, workspace(), |server| {
        server.route_fn("POST", "/api/v1/conversations", |r| {
            assert_eq!(r.json()["memberId"], "m-bob");
            Reply::json(
                200,
                json!({"id": "d-bob", "networkId": "n1", "kind": "direct", "name": "bob",
                       "favorite": false, "joined": true, "memberIds": ["m-bob"],
                       "memberModes": {}, "modes": "", "topic": "",
                       "unreadCount": 0, "mentionCount": 0}),
            )
        });
    });
    menu(&f, cx, "member-m-bob", "Mention");
    let composer = composer(&f, cx);
    assert_eq!(cx.update(|cx| composer.read(cx).text(cx)), "@bob ");

    assert_eq!(messages_shown(&f, cx), 2);
    menu(&f, cx, "member-m-bob", "Ignore");
    let put = f
        .server
        .wait_for("PUT", "/api/v1/notifications/settings", WAIT);
    assert_eq!(put.json()["ignore"], json!(["bob"]));
    wait(cx, "bob's messages hidden", |cx| {
        match f.view.read(cx).screen() {
            Screen::Workspace(w) => w
                .read(cx)
                .timeline
                .read(cx)
                .rows()
                .iter()
                .all(|r| !matches!(r, Row::Message(_))),
            _ => false,
        }
    });
    menu(&f, cx, "member-m-bob", "Stop ignoring");
    let put = f
        .server
        .wait_for_nth("PUT", "/api/v1/notifications/settings", 2, WAIT);
    assert_eq!(put.json()["ignore"], json!([]));

    menu(&f, cx, "member-m-bob", "Message");
    wait(cx, "the direct conversation", |cx| {
        f.session.read(cx).conversation.as_deref() == Some("d-bob")
    });
}

#[gpui_kit::test]
fn your_status_from_your_own_menu(cx: &mut TestAppContext) {
    let f = open(cx, workspace(), |server| {
        server.route_fn("PATCH", "/api/v1/networks/n1/profile", |r| {
            let body = r.json();
            Reply::json(
                200,
                json!({"id": "u-me", "account": "me", "nick": "me", "displayName": "",
                       "bio": "", "color": "", "role": "member", "status": "online",
                       "statusText": body["statusText"], "statusEmoji": body["statusEmoji"]}),
            )
        });
        server.route(
            "PATCH",
            "/api/v1/networks/n1/self",
            200,
            serde_json::to_value(&snapshot().networks[0]).unwrap(),
        );
    });
    assert_eq!(label(&f, cx, "self-status"), "Online");
    click(&f, cx, "self-menu");
    click_label(&f, cx, "Set a status…");
    wait_for(&f, cx, "status-editor");
    click(&f, cx, "status-text");
    type_text(&f, cx, "Lunch");
    click(&f, cx, "status-expiry-1h");
    click(&f, cx, "status-save");
    let patch = f
        .server
        .wait_for("PATCH", "/api/v1/networks/n1/profile", WAIT);
    let body = patch.json();
    assert_eq!(body["statusText"], "Lunch");
    assert!(
        body["statusExpiresAt"]
            .as_str()
            .is_some_and(|s| !s.is_empty())
    );
    wait(cx, "your status", |cx| {
        let session = f.session.read(cx);
        let w = session.client().unwrap().read();
        w.user.status_text.as_deref() == Some("Lunch")
    });
    assert_eq!(label(&f, cx, "self-status"), "Online · Lunch");

    click(&f, cx, "self-menu");
    click_label(&f, cx, "Set yourself away");
    let away = f.server.wait_for("PATCH", "/api/v1/networks/n1/self", WAIT);
    assert_eq!(away.json(), json!({"away": "Away"}));
}

#[gpui_kit::test]
fn an_empty_direct_conversation_shows_the_person(cx: &mut TestAppContext) {
    let mut s = workspace();
    let mut direct = s.conversations[1].clone();
    direct.id = "d-bob".into();
    direct.name = "bob".into();
    direct.kind = ConversationKind::Direct;
    s.conversations.push(direct);
    let f = open(cx, s, |_| {});
    cx.update(|cx| {
        f.session
            .update(cx, |s, cx| s.select_conversation("d-bob", cx))
    });
    wait_for(&f, cx, "direct-intro");
    assert_eq!(label(&f, cx, "direct-intro"), "Your conversation with bob");
}

#[gpui_kit::test]
fn the_people_picker_finds_people_here_and_on_the_server(cx: &mut TestAppContext) {
    let f = open(cx, workspace(), |server| {
        server.route(
            "GET",
            "/api/v1/people",
            200,
            json!({"items": [{"key": "k-zed", "networkId": "n1", "displayName": "Zed",
                              "nickname": "zed", "presence": "online", "eligible": true}],
                   "more": false, "source": "known", "status": "ready"}),
        );
        server.route_fn("POST", "/api/v1/conversations", |r| {
            assert_eq!(r.json()["nickname"], "zed");
            Reply::json(
                200,
                json!({"id": "d-zed", "networkId": "n1", "kind": "direct", "name": "zed",
                       "favorite": false, "joined": true, "memberIds": [],
                       "memberModes": {}, "modes": "", "topic": "",
                       "unreadCount": 0, "mentionCount": 0}),
            )
        });
    });
    menu(&f, cx, "network-n1", "Message someone…");
    wait_for(&f, cx, "people-picker");
    type_text(&f, cx, "e");
    assert_eq!(label(&f, cx, "people-picker-option-0"), "eve");
    frame(&f, cx, |window, cx| {
        window.press("backspace", cx);
        window.input("z", cx);
    });
    let lookup = f.server.wait_for("GET", "/api/v1/people", WAIT);
    assert_eq!(lookup.query_param("purpose").as_deref(), Some("dm"));
    let deadline = std::time::Instant::now() + WAIT;
    loop {
        let shown = label(&f, cx, "people-picker-option-0");
        if shown == "Zed, zed" {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "zed never showed: {shown}"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    frame(&f, cx, |window, cx| window.press("enter", cx));
    wait(cx, "the direct conversation", |cx| {
        f.session.read(cx).conversation.as_deref() == Some("d-zed")
    });
}

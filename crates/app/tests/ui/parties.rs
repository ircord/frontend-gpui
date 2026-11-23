//! Tests for package PT1: groups and invitations, against the canned server.

use std::time::Duration;

use gpui_kit::component::WindowExt;
use gpui_kit::test::{TestEventExt, TestQueryExt, TestWindowExt};
use gpui_kit::{AppContext, MouseButton, ScrollDelta, TestAppContext, Window, point, px};
use ircord_client::contract::*;
use ircord_client::store::Connection;
use ircord_client::testing::{CannedServer, Reply, frame};
use serde_json::{Value, json};

use crate::fixture::{Fixture, open_with, snapshot, wait};

const WAIT: Duration = crate::fixture::HANG_GUARD;

fn party(revision: i64, owner: &str, members: &[(&str, &str, &str)]) -> Value {
    json!({
        "id": "p1", "conversationId": "c-group", "networkId": "n1", "name": "",
        "description": "Weekend plans", "ownerId": owner, "revision": revision,
        "state": "active", "createdAt": "2026-11-20T10:00:00Z",
        "updatedAt": "2026-11-20T10:00:00Z",
        "members": members.iter().map(|(id, account, name)| json!({
            "id": id, "backendAccountId": account, "displayName": name,
            "servicesAccount": name.to_lowercase(), "username": name.to_lowercase(),
            "joinedAt": "2026-11-20T10:00:00Z", "servicesRegisteredAt": ""
        })).collect::<Vec<_>>()
    })
}

fn members() -> Vec<(&'static str, &'static str, &'static str)> {
    vec![
        ("pm-me", "u-me", "Me"),
        ("pm-bob", "u-bob", "Bob"),
        ("pm-eve", "u-eve", "Eve"),
    ]
}

fn group_conversation(party: Value) -> Conversation {
    serde_json::from_value(json!({
        "id": "c-group", "networkId": "n1", "kind": "channel", "name": "",
        "target": "#party-p1", "favorite": false, "joined": true,
        "memberIds": [], "memberModes": {}, "modes": "", "topic": "Weekend plans",
        "unreadCount": 0, "mentionCount": 0, "party": party
    }))
    .unwrap()
}

/// The fixture's workspace with a group you own, or not, and invitations.
fn workspace(owner: &str, invitations: Value) -> Snapshot {
    let mut s = snapshot();
    s.conversations
        .push(group_conversation(party(2, owner, &members())));
    s.invitations = serde_json::from_value(invitations).unwrap();
    s.capabilities.parties = Some(PartyCapability {
        network_id: "n1".into(),
        max_members: 32,
        max_name_length: 100,
        max_description_length: 500,
    });
    s
}

fn open(cx: &mut TestAppContext, s: Snapshot, setup: impl FnOnce(&CannedServer)) -> Fixture {
    let f = open_with(cx, true, |server| {
        server.snapshot(&s);
        setup(server);
    });
    wait(cx, "the live socket", |cx| {
        let session = f.session.read(cx);
        let Some(client) = session.client() else {
            return false;
        };
        let w = client.read();
        w.connection == Some(Connection::Live) && w.conversation("c-group").is_some()
    });
    select(&f, cx, "c-general");
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

fn click_label(f: &Fixture, cx: &mut TestAppContext, label: &str) {
    frame_do(f, cx, |window, cx| {
        let at = window.find_by_label(label).bounds().center();
        window.pointer_down(at, MouseButton::Left, cx);
        window.pointer_up(at, MouseButton::Left, cx);
    });
}

fn shows(f: &Fixture, cx: &mut TestAppContext, id: &'static str) -> bool {
    let mut found = false;
    frame_do(f, cx, |window, _| found = window.try_find(id).is_some());
    found
}

fn has_label(f: &Fixture, cx: &mut TestAppContext, label: &str) -> bool {
    let mut found = false;
    frame_do(f, cx, |window, _| {
        found = window.try_find_by_label(label).is_some()
    });
    found
}

fn label(f: &Fixture, cx: &mut TestAppContext, id: &'static str) -> String {
    // The model can be ready a frame or more before the window has swapped
    // its loading screen for the app, so wait for the row to be drawn.
    let deadline = std::time::Instant::now() + WAIT;
    loop {
        cx.run_until_parked();
        let mut text = None;
        frame_do(f, cx, |window, _| {
            text = window
                .try_find(id)
                .map(|e| e.label().unwrap_or_default().to_string())
        });
        if let Some(text) = text {
            return text;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for {id} to be drawn"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn select(f: &Fixture, cx: &mut TestAppContext, id: &str) {
    let id = id.to_string();
    cx.update(|cx| f.session.update(cx, |s, cx| s.select_conversation(&id, cx)));
    cx.run_until_parked();
}

fn showing(f: &Fixture, cx: &mut TestAppContext) -> Option<String> {
    cx.update(|cx| f.session.read(cx).conversation.clone())
}

fn has_conversation(f: &Fixture, cx: &mut TestAppContext, id: &str) -> bool {
    cx.update(|cx| {
        f.session
            .read(cx)
            .client()
            .is_some_and(|c| c.read().conversation(id).is_some())
    })
}

fn group_invitation() -> Value {
    json!([{
        "id": "i-group", "networkId": "n1", "channel": "", "inviterId": "m-bob",
        "inviterNick": "bob", "createdAt": "2026-11-21T09:00:00Z",
        "party": {"id": "p2", "name": "Book club", "description": "", "inviterAccountId": "u-bob",
                  "memberCount": 2}
    }, {
        "id": "i-chan", "networkId": "n1", "channel": "#secret", "inviterId": "m-eve",
        "inviterNick": "eve", "createdAt": "2026-11-21T09:05:00Z"
    }])
}

#[gpui_kit::test]
fn a_group_is_titled_by_its_people_and_shows_no_channel_prefix(cx: &mut TestAppContext) {
    let f = open(cx, workspace("pm-me", json!([])), |_| {});
    // Unnamed: the people in it but you.
    assert_eq!(label(&f, cx, "conversation-c-group"), "Bob, Eve");
    select(&f, cx, "c-group");
    assert!(
        has_label(&f, cx, "Group details"),
        "the header's details button"
    );
    // A rename from the server renames the row.
    let mut renamed = party(3, "pm-me", &members());
    renamed["name"] = json!("Weekend crew");
    f.server.push(frame(4, "party.updated", &renamed));
    wait(cx, "the rename", |cx| {
        let session = f.session.read(cx);
        let w = session.client().unwrap().read();
        w.conversation("c-group")
            .and_then(|c| c.party.as_ref())
            .is_some_and(|p| p.name == "Weekend crew")
    });
    assert_eq!(label(&f, cx, "conversation-c-group"), "Weekend crew");
}

#[gpui_kit::test]
fn accepting_a_group_invitation_opens_the_group(cx: &mut TestAppContext) {
    let f = open(cx, workspace("pm-me", group_invitation()), |server| {
        let mut joined = group_conversation(party(
            1,
            "pm-bob",
            &[("pm-bob", "u-bob", "Bob"), ("pm-me", "u-me", "Me")],
        ));
        joined.id = "c-book".into();
        joined.party.as_mut().unwrap().id = "p2".into();
        joined.party.as_mut().unwrap().conversation_id = "c-book".into();
        joined.party.as_mut().unwrap().name = "Book club".into();
        server.route(
            "POST",
            "/api/v1/invitations/i-group/accept",
            200,
            serde_json::to_value(joined).unwrap(),
        );
    });
    assert!(shows(&f, cx, "invitation-banner"));
    assert_eq!(
        label(&f, cx, "invitation-text"),
        "bob invited you to Book club on Home."
    );
    assert_eq!(label(&f, cx, "invitation-more"), "· 1 more");
    click(&f, cx, "invitation-accept");
    f.server
        .wait_for("POST", "/api/v1/invitations/i-group/accept", WAIT);
    wait(cx, "the group to open", |cx| {
        f.session.read(cx).conversation.as_deref() == Some("c-book")
    });
    assert_eq!(label(&f, cx, "conversation-c-book"), "Book club");
    // The channel invitation is next.
    assert_eq!(
        label(&f, cx, "invitation-text"),
        "eve invited you to #secret on Home"
    );
    assert!(has_label(&f, cx, "Join"));
    assert!(!shows(&f, cx, "invitation-more"));
}

#[gpui_kit::test]
fn rejecting_a_channel_invitation_removes_the_banner(cx: &mut TestAppContext) {
    let only_channel = json!([group_invitation()[1].clone()]);
    let f = open(cx, workspace("pm-me", only_channel), |server| {
        server.route_reply("DELETE", "/api/v1/invitations/i-chan", Reply::empty());
    });
    assert!(has_label(&f, cx, "Reject"));
    click(&f, cx, "invitation-decline");
    f.server
        .wait_for("DELETE", "/api/v1/invitations/i-chan", WAIT);
    wait(cx, "the invitation to go", |cx| {
        let session = f.session.read(cx);
        session.client().unwrap().read().invitations.is_empty()
    });
    assert!(!shows(&f, cx, "invitation-banner"));
}

#[gpui_kit::test]
fn a_failed_answer_stays_on_the_banner(cx: &mut TestAppContext) {
    let f = open(cx, workspace("pm-me", group_invitation()), |server| {
        server.route_reply(
            "POST",
            "/api/v1/invitations/i-group/accept",
            Reply::problem(404, "This invitation is no longer available."),
        );
    });
    click(&f, cx, "invitation-accept");
    f.server
        .wait_for("POST", "/api/v1/invitations/i-group/accept", WAIT);
    let mut shown = false;
    for _ in 0..200 {
        if shows(&f, cx, "invitation-error") {
            shown = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(shown, "the error shows on the banner");
    assert_eq!(
        label(&f, cx, "invitation-error"),
        "This invitation is no longer available."
    );
}

#[gpui_kit::test]
fn the_owner_sees_the_people_and_removes_someone(cx: &mut TestAppContext) {
    let f = open(cx, workspace("pm-me", json!([])), |server| {
        server.route(
            "DELETE",
            "/api/v1/parties/p1/members/pm-eve",
            200,
            party(
                3,
                "pm-me",
                &[("pm-me", "u-me", "Me"), ("pm-bob", "u-bob", "Bob")],
            ),
        );
    });
    select(&f, cx, "c-group");
    click(&f, cx, "party-details-toggle");
    assert!(shows(&f, cx, "party-details"));
    assert_eq!(label(&f, cx, "party-people"), "People (3)");
    assert!(shows(&f, cx, "party-description"));
    // Your own row offers nothing; the others can be removed or promoted.
    assert!(!has_label(&f, cx, "Remove Me"));
    assert!(has_label(&f, cx, "Transfer ownership to Bob"));
    assert!(shows(&f, cx, "party-convert"));
    click_label(&f, cx, "Remove Eve");
    click_label(&f, cx, "Remove");
    f.server
        .wait_for("DELETE", "/api/v1/parties/p1/members/pm-eve", WAIT);
    wait(cx, "Eve to leave the group", |cx| {
        let session = f.session.read(cx);
        let w = session.client().unwrap().read();
        w.conversation("c-group")
            .and_then(|c| c.party.as_ref())
            .is_some_and(|p| p.members.len() == 2)
    });
    assert_eq!(label(&f, cx, "party-people"), "People (2)");
    assert_eq!(label(&f, cx, "conversation-c-group"), "Bob");
}

#[gpui_kit::test]
fn a_member_who_does_not_own_the_group_cannot_manage_people(cx: &mut TestAppContext) {
    let f = open(cx, workspace("pm-bob", json!([])), |_| {});
    select(&f, cx, "c-group");
    click(&f, cx, "party-details-toggle");
    assert!(shows(&f, cx, "party-details"));
    assert!(!has_label(&f, cx, "Remove Eve"));
    assert!(!has_label(&f, cx, "Transfer ownership to Eve"));
    assert!(!shows(&f, cx, "party-convert"));
    assert!(shows(&f, cx, "party-invite"));
    assert!(shows(&f, cx, "party-leave"));
}

#[gpui_kit::test]
fn leaving_drops_the_group_and_moves_on(cx: &mut TestAppContext) {
    let f = open(cx, workspace("pm-bob", json!([])), |server| {
        server.route("POST", "/api/v1/parties/p1/leave", 200, json!({"id": "p1"}));
    });
    select(&f, cx, "c-group");
    click(&f, cx, "party-details-toggle");
    frame_do(&f, cx, |window, cx| {
        window.scroll(
            "party-details",
            ScrollDelta::Pixels(point(px(0.), -window.viewport_size().height)),
            cx,
        );
    });
    click(&f, cx, "party-leave");
    click_label(&f, cx, "Leave");
    f.server.wait_for("POST", "/api/v1/parties/p1/leave", WAIT);
    wait(cx, "the group to go", |cx| {
        let session = f.session.read(cx);
        session.conversation.as_deref() != Some("c-group")
            && session
                .client()
                .is_some_and(|c| c.read().conversation("c-group").is_none())
    });
    assert!(!shows(&f, cx, "party-details"));
    assert!(!shows(&f, cx, "conversation-c-group"));
}

#[gpui_kit::test]
fn a_group_removed_while_showing_says_so_and_moves_on(cx: &mut TestAppContext) {
    let f = open(cx, workspace("pm-bob", json!([])), |_| {});
    select(&f, cx, "c-group");
    assert_eq!(showing(&f, cx).as_deref(), Some("c-group"));
    f.server.push(frame(
        4,
        "party.removed",
        &json!({"id": "p1", "conversationId": "c-group", "revision": 3}),
    ));
    wait(cx, "the group to go", |cx| {
        f.session.read(cx).conversation.as_deref() != Some("c-group")
    });
    assert!(!has_conversation(&f, cx, "c-group"));
    wait(cx, "a toast saying the group is gone", |cx| {
        let mut shown = 0;
        let _ = f.window.update(cx, |_, window, cx| {
            shown = window.notifications(cx).len();
        });
        shown == 1
    });
}

#[gpui_kit::test]
fn renaming_patches_the_group(cx: &mut TestAppContext) {
    let f = open(cx, workspace("pm-me", json!([])), |server| {
        let mut renamed = party(3, "pm-me", &members());
        renamed["name"] = json!("Crew");
        server.route("PATCH", "/api/v1/parties/p1", 200, renamed);
    });
    select(&f, cx, "c-group");
    click(&f, cx, "party-details-toggle");
    click(&f, cx, "party-rename");
    frame_do(&f, cx, |window, cx| window.input("Crew", cx));
    click_label(&f, cx, "Save changes");
    let request = f.server.wait_for("PATCH", "/api/v1/parties/p1", WAIT);
    assert_eq!(request.json()["name"], "Crew");
    assert!(request.json().get("description").is_none());
    wait(cx, "the new name", |cx| {
        let session = f.session.read(cx);
        let w = session.client().unwrap().read();
        w.conversation("c-group")
            .and_then(|c| c.party.as_ref())
            .is_some_and(|p| p.name == "Crew")
    });
    assert_eq!(label(&f, cx, "conversation-c-group"), "Crew");
}

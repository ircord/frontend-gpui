//! The client against the in-process canned server: the seams later features
//! build on, exercised the way a feature uses them.

use std::sync::Arc;
use std::time::Duration;

use crate::Client;
use crate::api::{ApiError, Method};
use crate::client::runtime;
use crate::contract::event::Document;
use crate::documents::PREFERENCES;
use crate::events::Update;
use crate::store::{Changes, SaveState, Workspace, topic};
use crate::testing::*;
use futures::StreamExt;
use serde_json::{Value, json};

const WAIT: Duration = Duration::from_secs(10);

fn started(snapshot: SnapshotBuilder) -> (CannedServer, Client) {
    let rt = runtime().unwrap();
    let server = CannedServer::start(&rt);
    server.snapshot(&snapshot.build());
    let (client, _updates) = Client::start(rt, server.api());
    wait_live(&client);
    (server, client)
}

#[test]
fn call_runs_a_request_and_applies_its_result() {
    let (server, client) = started(SnapshotBuilder::new().channel("c-a", "alpha"));
    server.route(
        "PATCH",
        "/api/v1/conversations/*",
        200,
        json!({"id": "c-a", "favorite": true}),
    );
    let task = client.call(
        |api| async move {
            api.call::<_, Value>(
                Method::PATCH,
                "/api/v1/conversations/c-a",
                &json!({"favorite": true}),
            )
            .await
        },
        |w: &mut Workspace, result| {
            let answer = result.expect("the canned server answers");
            let c = w.conversations.iter_mut().find(|c| c.id == "c-a").unwrap();
            c.favorite = answer["favorite"].as_bool().unwrap();
            Changes {
                sidebar: true,
                ..Default::default()
            }
        },
    );
    client.runtime().block_on(task).unwrap();
    assert!(client.read().conversation("c-a").unwrap().favorite);
    let request = server.wait_for("PATCH", "/api/v1/conversations/c-a", WAIT);
    assert_eq!(request.json(), json!({"favorite": true}));
    assert_eq!(request.header("cookie"), Some(CANNED_COOKIE));
    assert_eq!(
        request.header("origin"),
        Some(server.address().origin().as_str())
    );
}

#[test]
fn call_reports_failures_and_an_expired_session_signs_out() {
    let rt = runtime().unwrap();
    let server = CannedServer::start(&rt);
    server.snapshot(&SnapshotBuilder::new().build());
    let (client, mut updates) = Client::start(rt.clone(), server.api());
    wait_live(&client);
    server.route_reply("GET", "/api/v1/inbox", Reply::problem(500, "disk full"));
    let seen = Arc::new(std::sync::Mutex::new(None));
    let s = seen.clone();
    let task = client.call(
        |api| async move { api.get::<Value>("/api/v1/inbox").await },
        move |_, result| {
            *s.lock().unwrap() = Some(result.unwrap_err());
            Changes::default()
        },
    );
    rt.block_on(task).unwrap();
    assert!(matches!(
        seen.lock().unwrap().take(),
        Some(ApiError::Http { status: 500, detail }) if detail == "disk full"
    ));

    server.route_reply("GET", "/api/v1/inbox", Reply::problem(401, "expired"));
    let answer = rt
        .block_on(client.request(|api| async move { api.get::<Value>("/api/v1/inbox").await }))
        .unwrap();
    assert!(matches!(answer, Err(ApiError::Unauthorized)));
    let signed_out = rt.block_on(async {
        tokio::time::timeout(WAIT, async {
            while let Some(update) = updates.next().await {
                if update == Update::SignedOut {
                    return true;
                }
            }
            false
        })
        .await
        .unwrap_or(false)
    });
    assert!(signed_out);
}

#[test]
fn pushed_frames_reach_the_workspace_and_watchers_see_them_first() {
    let (server, client) = started(
        SnapshotBuilder::new()
            .member("m-bob", "bob")
            .channel("c-a", "alpha")
            .cursor(4),
    );
    let attracted = Arc::new(std::sync::Mutex::new(Vec::new()));
    let a = attracted.clone();
    client.watch(move |_, changes| {
        if changes.touches_id(topic::ATTRACT, "c-a") {
            a.lock().unwrap().push("c-a");
        }
    });
    client.open("c-a");
    wait_until(&client, WAIT, "the timeline", |w| {
        w.timelines.get("c-a").is_some_and(|t| t.loaded)
    });
    server.push(frame(
        5,
        "message.created",
        &message("m1", "c-a", "m-bob", "hi"),
    ));
    server.push(ephemeral(
        "attract",
        &json!({"conversationId": "c-a", "memberId": "m-bob", "messageId": "m1"}),
    ));
    wait_until(&client, WAIT, "the pushed message", |w| {
        w.cursor == 5 && w.timelines["c-a"].find("m1").is_some()
    });
    let deadline = std::time::Instant::now() + WAIT;
    while attracted.lock().unwrap().is_empty() {
        assert!(
            std::time::Instant::now() < deadline,
            "the watcher never ran"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    let resume: Value = serde_json::from_str(&server.client_frames()[0]).unwrap();
    assert_eq!(resume["after"], 4);
}

#[test]
fn revisioned_documents_read_save_and_follow_remote_changes() {
    let (server, client) = started(SnapshotBuilder::new().cursor(1));
    let stored = |compact: bool| json!({"preferences": {"showNsfw": compact}, "updatedAt": "2026-11-21T10:00:00Z"});
    server.route("GET", "/api/v1/app/preferences", 200, stored(false));
    // The server stores what it is sent and answers with it.
    server.route_fn("PUT", "/api/v1/app/preferences", |r| {
        let mut body = r.json();
        body["updatedAt"] = json!("2026-11-21T10:05:00Z");
        Reply::json(200, body)
    });
    let compact = |w: &Workspace| {
        w.documents
            .preferences
            .value()
            .and_then(|p| p.preferences.as_ref())
            .and_then(|p| p.show_nsfw)
    };
    client.watch_document(&PREFERENCES);
    wait_until(&client, WAIT, "the first read", |w| {
        compact(w) == Some(false)
    });

    // Two quick edits become one save of the newer one.
    let mut edit = client
        .read()
        .documents
        .preferences
        .value()
        .cloned()
        .unwrap();
    edit.preferences.as_mut().unwrap().show_nsfw = Some(true);
    let mut first = edit.clone();
    first.preferences.as_mut().unwrap().channel_guide_seen = Some(true);
    client.edit_document(&PREFERENCES, first);
    client.edit_document(&PREFERENCES, edit);
    assert_eq!(compact(&client.read()), Some(true), "an edit shows at once");
    wait_until(&client, WAIT, "the save", |w| {
        w.documents.preferences.save_state() == &SaveState::Saved
    });
    let saves = server.requests_to("PUT", "/api/v1/app/preferences");
    assert_eq!(saves.len(), 1, "the edits were coalesced");
    assert_eq!(saves[0].json()["preferences"]["showNsfw"], true);
    assert_eq!(
        saves[0].json()["preferences"]["channelGuideSeen"],
        Value::Null
    );

    // Another device changes it: the event makes the client read it again.
    server.route("GET", "/api/v1/app/preferences", 200, stored(false));
    server.push(frame(
        2,
        "client-preferences.updated",
        &json!({"revision": "r2"}),
    ));
    wait_until(&client, WAIT, "the re-read", |w| compact(w) == Some(false));
    assert_eq!(
        server.requests_to("GET", "/api/v1/app/preferences").len(),
        2
    );
    assert_eq!(
        client
            .read()
            .revisions
            .get(&Document::Preferences)
            .map(String::as_str),
        Some("r2")
    );
}

#[test]
fn discovered_pictures_are_read_with_the_session() {
    let rt = runtime().unwrap();
    let server = CannedServer::start(&rt);
    let png = b"\x89PNG\r\n\x1a\nrest".to_vec();
    server.route_reply(
        "GET",
        "/api/v1/emotes/discover/*/*/preview",
        Reply::bytes(200, "application/octet-stream", png.clone()),
    );
    let api = server.api();
    let path = "/api/v1/emotes/discover/emoji.gg/739853-oof/preview";
    let picture = rt
        .block_on(api.discovered_picture(path, true))
        .expect("the canned server answers");
    assert_eq!(picture.bytes, png);
    assert_eq!(picture.content_type, "image/png");
    let request = server.wait_for("GET", path, WAIT);
    assert_eq!(request.header("cookie"), Some(CANNED_COOKIE));
    assert_eq!(request.query_param("static").as_deref(), Some("true"));

    // Played: no `static`. Anywhere else: refused before anything is sent.
    rt.block_on(api.discovered_picture(path, false)).unwrap();
    let played = server.wait_for_nth("GET", path, 2, WAIT);
    assert_eq!(played.query_param("static"), None);
    let elsewhere = rt.block_on(api.discovered_picture("https://cdn.example/a.png", false));
    assert!(matches!(elsewhere, Err(ApiError::Decode(_))));
    assert_eq!(server.requests().len(), 2);

    // A still the server cannot make (an animated PNG): the whole picture.
    let apng = "/api/v1/emotes/discover/emoji.gg/1-apng/preview";
    let whole = png.clone();
    server.route_fn("GET", apng, move |request| {
        if request.query_param("static").is_some() {
            Reply::problem(422, "Animated images need a still frame.")
        } else {
            Reply::bytes(200, "image/png", whole.clone())
        }
    });
    let picture = rt.block_on(api.discovered_picture(apng, true)).unwrap();
    assert_eq!(picture.bytes, png);
    assert_eq!(server.requests_to("GET", apng).len(), 2);
}

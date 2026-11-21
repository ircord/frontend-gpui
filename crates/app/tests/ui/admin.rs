//! Tests for package AD1: Administration, against the canned server. Each
//! section has a test; the entry shows only to server owners.

use std::time::Duration;

#[allow(unused_imports)]
use gpui_kit::test::{TestEventExt, TestQueryExt, TestWindowExt};
use gpui_kit::{AppContext, ScrollDelta, TestAppContext, point, px};
use ircord_app::features::admin;
use ircord_app::route::Route;
use ircord_client::testing::{CannedServer, Reply};
use serde_json::{Value, json};

use crate::fixture::{Fixture, bootstrap, open_with, wait};

const WAIT: Duration = Duration::from_secs(5);

fn user(id: &str, name: &str, owner: bool, source: Option<&str>) -> Value {
    let mut u = json!({"id": id, "username": name, "owner": owner,
                       "createdAt": "2026-01-02T10:00:00Z", "lastSeenAt": "2026-11-20T10:00:00Z",
                       "sessions": 2, "networks": 1});
    if let Some(source) = source {
        u["ownerSource"] = json!(source);
    }
    u
}

/// An owner's server: every Administration route answered.
fn serve(server: &CannedServer) {
    server.route(
        "GET",
        "/api/v1/administration/access",
        200,
        json!({"owner": true}),
    );
    server.route(
        "GET",
        "/api/v1/auth/session",
        200,
        json!({"account": {"id": "a-me", "username": "me", "createdAt": "2026-01-01T00:00:00Z",
                           "expiresAt": "2027-01-01T00:00:00Z", "lastSeenAt": "2026-11-21T00:00:00Z"}}),
    );
    server.route(
        "GET",
        "/api/v1/administration/status",
        200,
        json!({
            "status": "ok", "database": "available", "version": "1.4.0", "commit": "abcdef123456",
            "goVersion": "go1.25", "startedAt": "2026-11-19T08:00:00Z",
            "uptimeSeconds": 2 * 86_400 + 3 * 3_600 + 4 * 60,
            "overview": {"accounts": 1234, "sessions": 3, "networks": 5, "ircConnections": 4,
                         "messages": 98765, "databaseBytes": 8_808_038},
            "ergo": {"server": "irc.example", "state": "ok", "checkedAt": "2026-11-21T10:00:00Z",
                     "statistics": {"version": "ergo-2.14", "channels": 12, "servers": 1,
                                    "goVersion": "go1.25", "startedAt": "2026-11-01T00:00:00Z",
                                    "uptimeSeconds": 3_600,
                                    "users": {"total": 42, "invisible": 0, "operators": 1,
                                              "peak": 50, "unknown": 0}}}
        }),
    );
    server.route(
        "GET",
        "/api/v1/administration/audit",
        200,
        json!({"items": [
            {"action": "owner-grant", "actor": "me", "target": "bob", "outcome": "granted",
             "at": "2026-11-21T09:00:00Z", "reason": "helps out"},
            {"action": "bot-mark", "actor": "me", "target": "eve", "outcome": "refused",
             "at": "2026-11-21T09:30:00Z"}
        ]}),
    );
    server.route(
        "GET",
        "/api/v1/administration/users",
        200,
        json!({"items": [user("a-me", "me", true, Some("environment")),
                         user("a-bob", "bob", true, Some("owner")),
                         user("a-eve", "eve", false, None)],
               "more": true}),
    );
}

fn open(cx: &mut TestAppContext, setup: impl FnOnce(&CannedServer)) -> Fixture {
    let f = open_with(cx, true, |server| {
        serve(server);
        setup(server);
    });
    bootstrap(&f, cx);
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
    reveal(f, cx, id);
    frame(f, cx, |window, cx| window.click(id, cx));
}

/// Wait for `id`, then scroll the section until it is in view.
fn reveal(f: &Fixture, cx: &mut TestAppContext, id: &'static str) {
    let window = f.window;
    wait(cx, id, move |cx| {
        window
            .update(cx, |_, window, cx| {
                window.render_frame(cx);
                window.try_find(id).is_some()
            })
            .unwrap_or(false)
    });
    // Scroll the section until the element sits inside it.
    for _ in 0..10 {
        let mut by = 0.0;
        frame(f, cx, |window, _| {
            let (Some(e), Some(pane)) = (window.try_find(id), window.try_find("admin-content"))
            else {
                return;
            };
            let (e, pane) = (e.bounds(), pane.bounds());
            if e.top() < pane.top() || e.bottom() > pane.bottom() {
                by = f32::from(pane.center().y - e.center().y);
            }
        });
        if by == 0.0 {
            return;
        }
        frame(f, cx, |window, cx| {
            window.scroll(
                "admin-content",
                ScrollDelta::Pixels(point(px(0.), px(by))),
                cx,
            )
        });
    }
}

fn type_into(f: &Fixture, cx: &mut TestAppContext, id: &'static str, text: &str) {
    click(f, cx, id);
    frame(f, cx, |window, cx| {
        window.press("ctrl-a", cx);
        window.press("backspace", cx);
        window.input(text, cx);
    });
}

/// Wait until the window shows `id`.
fn shows(f: &Fixture, cx: &mut TestAppContext, id: &'static str) {
    let window = f.window;
    wait(cx, id, move |cx| {
        window
            .update(cx, |_, window, cx| {
                window.render_frame(cx);
                window.try_find(id).is_some_and(|e| e.visible())
            })
            .unwrap_or(false)
    });
}

/// Wait until `id` reads `want`.
fn reads(f: &Fixture, cx: &mut TestAppContext, id: &'static str, want: &str) {
    let window = f.window;
    let want = want.to_string();
    wait(cx, &format!("{id} reading {want:?}"), move |cx| {
        window
            .update(cx, |_, window, cx| {
                window.render_frame(cx);
                window
                    .try_find(id)
                    .is_some_and(|e| e.label() == Some(want.as_str()))
            })
            .unwrap_or(false)
    });
}

/// Wait, running the app, for a request to `method path` (one sent after
/// a pause in typing).
fn asked(f: &Fixture, cx: &mut TestAppContext, method: &'static str, path: &'static str) {
    wait(cx, path, |_| !f.server.requests_to(method, path).is_empty());
}

fn gone(f: &Fixture, cx: &mut TestAppContext, id: &'static str) -> bool {
    let mut gone = false;
    frame(f, cx, |window, _| gone = window.try_find(id).is_none());
    gone
}

fn page(f: &Fixture, cx: &mut TestAppContext) {
    click(f, cx, "sidebar-administration");
    assert_eq!(
        cx.update(|cx| f.session.read(cx).route()),
        Some(Route::Page(admin::PAGE.into()))
    );
}

#[gpui_kit::test]
fn the_entry_is_hidden_from_people_who_are_not_owners(cx: &mut TestAppContext) {
    let f = open(cx, |server| {
        server.route(
            "GET",
            "/api/v1/administration/access",
            200,
            json!({"owner": false}),
        );
    });
    f.server
        .wait_for("GET", "/api/v1/administration/access", WAIT);
    assert!(gone(&f, cx, "sidebar-administration"));
    click(&f, cx, "self-menu");
    frame(&f, cx, |window, _| {
        assert!(window.try_find_by_label("Administration").is_none());
    });
    // Reached anyway (an old key), the page says why it shows nothing.
    let session = f.session.clone();
    cx.update(|cx| admin::open(&session, cx));
    shows(&f, cx, "admin-not-owner");
}

#[gpui_kit::test]
fn the_overview_shows_the_backend_ircord_and_the_irc_server(cx: &mut TestAppContext) {
    let f = open(cx, |_| {});
    page(&f, cx);
    reads(&f, cx, "admin-health", "Health: Healthy");
    reads(&f, cx, "admin-version", "Version: 1.4.0 (abcdef1)");
    reads(&f, cx, "admin-uptime", "Up for: 2 days 3 hours 4 minutes");
    reads(&f, cx, "admin-overview-accounts", "Active accounts: 1,234");
    reads(&f, cx, "admin-overview-size", "Database size: 8.4 MB");
    reads(&f, cx, "admin-irc-users", "Users online: 42");
    reads(&f, cx, "admin-irc-uptime", "Up for: 1 hour 0 minutes");
    reads(&f, cx, "admin-audit-0", "me bob made an owner — helps out");
    reads(
        &f,
        cx,
        "admin-audit-1",
        "me tried: eve marked as a bot (refused)",
    );

    // Refresh reads both again.
    let before = f
        .server
        .requests_to("GET", "/api/v1/administration/status")
        .len();
    click(&f, cx, "admin-status-refresh");
    wait(cx, "the status read again", |_| {
        f.server
            .requests_to("GET", "/api/v1/administration/status")
            .len()
            > before
    });

    // Without an IRC server the section says so.
    f.server.route(
        "GET",
        "/api/v1/administration/status",
        200,
        json!({"status": "degraded", "database": "unavailable", "version": "1.4.0", "commit": "",
               "goVersion": "go1.25", "startedAt": "2026-11-21T08:00:00Z", "uptimeSeconds": 60}),
    );
    click(&f, cx, "admin-status-refresh");
    reads(&f, cx, "admin-health", "Health: Degraded");
    reads(&f, cx, "admin-database-state", "Database: Unavailable");
}

#[gpui_kit::test]
fn the_database_section_compares_two_samples(cx: &mut TestAppContext) {
    let f = open(cx, |server| {
        server.route(
            "GET",
            "/api/v1/administration/database",
            200,
            json!({"available": true, "engine": "sqlite", "version": "3.50.1",
                   "journalMode": "wal", "synchronous": "normal", "foreignKeys": true,
                   "busyTimeoutMs": 5000, "autoCheckpointPages": 1000,
                   "allocatedBytes": 125_829_120, "reusableBytes": 1024, "fileBytes": 125_829_120,
                   "walBytes": null, "startedAt": "2026-11-21T08:00:00Z",
                   "sampledAt": "2026-11-21T10:00:00Z",
                   "pool": {"inUse": 1, "idle": 2, "open": 3, "maxOpen": 0,
                            "waitCount": 10, "waitTotalMs": 100.0}}),
        );
    });
    page(&f, cx);
    click(&f, cx, "admin-tab-database");
    reads(&f, cx, "admin-db-pool-limit", "Connection limit: No limit");
    reads(
        &f,
        cx,
        "admin-db-pool-recent-waits",
        "Recent waits per minute: Not measured",
    );
    reads(
        &f,
        cx,
        "admin-db-storage-allocated",
        "Allocated pages: 120 MB",
    );
    reads(
        &f,
        cx,
        "admin-db-storage-wal",
        "Write-ahead log file: Not measured",
    );
    reads(&f, cx, "admin-db-config-engine", "Engine: SQLite 3.50.1");
    reads(&f, cx, "admin-db-config-journal", "Journal mode: WAL");
    reads(
        &f,
        cx,
        "admin-db-config-checkpoint",
        "Automatic checkpoint threshold: 1,000 pages",
    );

    // A second sample thirty seconds later gives the recent waits.
    f.server.route(
        "GET",
        "/api/v1/administration/database",
        200,
        json!({"available": true, "engine": "sqlite", "startedAt": "2026-11-21T08:00:00Z",
               "sampledAt": "2026-11-21T10:00:30Z",
               "pool": {"inUse": 0, "idle": 3, "open": 3, "maxOpen": 8,
                        "waitCount": 13, "waitTotalMs": 160.0}}),
    );
    click(&f, cx, "admin-db-refresh");
    reads(
        &f,
        cx,
        "admin-db-pool-recent-waits",
        "Recent waits per minute: 6",
    );
    reads(
        &f,
        cx,
        "admin-db-pool-recent-wait-time",
        "Wait time since last sample: 60 ms",
    );
    reads(&f, cx, "admin-db-pool-limit", "Connection limit: 8");
    click(&f, cx, "admin-db-live");
}

#[gpui_kit::test]
fn owners_are_added_and_removed_with_a_reason(cx: &mut TestAppContext) {
    let f = open(cx, |server| {
        server.route(
            "GET",
            "/api/v1/administration/identities",
            200,
            json!({"items": [
                {"nickname": "Ghost", "eligible": false, "note": "No IRCord account uses this nickname"},
                {"nickname": "Evie", "account": "eve", "eligible": true,
                 "user": user("a-eve", "eve", false, None)}
            ]}),
        );
        server.route(
            "POST",
            "/api/v1/administration/owners",
            200,
            user("a-eve", "eve", true, Some("owner")),
        );
        server.route(
            "DELETE",
            "/api/v1/administration/owners/*",
            200,
            user("a-bob", "bob", false, None),
        );
    });
    page(&f, cx);
    click(&f, cx, "admin-tab-users");
    // Owners first, you marked, the configured owner locked.
    reads(
        &f,
        cx,
        "admin-user-0",
        "me · Owner (you). Joined 2 January 2026 · last seen 20 November 2026 · 2 sessions · 1 network",
    );
    assert!(gone(&f, cx, "admin-user-0-revoke"));
    shows(&f, cx, "admin-user-1-revoke");
    shows(&f, cx, "admin-users-more");

    // Find eve by her nickname; the ineligible match says why.
    type_into(&f, cx, "admin-add-owner-input", "evie");
    asked(&f, cx, "GET", "/api/v1/administration/identities");
    reads(
        &f,
        cx,
        "admin-add-owner-match-0",
        "Ghost, No IRCord account uses this nickname",
    );
    reads(&f, cx, "admin-add-owner-match-1", "Evie → eve");
    click(&f, cx, "admin-add-owner-match-1");
    type_into(&f, cx, "prompt-input", "runs events");
    click(&f, cx, "ok");
    let grant = f
        .server
        .wait_for("POST", "/api/v1/administration/owners", WAIT);
    assert_eq!(
        grant.json(),
        json!({"account": "a-eve", "reason": "runs events"})
    );
    reads(&f, cx, "admin-users-notice", "eve is now an owner.");
    shows(&f, cx, "admin-user-2-revoke");

    // Remove bob, without a reason.
    click(&f, cx, "admin-user-1-revoke");
    shows(&f, cx, "prompt-input");
    click(&f, cx, "ok");
    let revoke = f
        .server
        .wait_for("DELETE", "/api/v1/administration/owners/*", WAIT);
    assert_eq!(revoke.path, "/api/v1/administration/owners/a-bob");
    assert_eq!(revoke.query_param("reason"), None);
    reads(&f, cx, "admin-users-notice", "bob is no longer an owner.");

    // The filter reads the list again, from the start.
    type_into(&f, cx, "admin-users-filter", "bo");
    wait(cx, "the filtered list", |_| {
        f.server
            .requests_to("GET", "/api/v1/administration/users")
            .iter()
            .any(|r| r.query_param("query").as_deref() == Some("bo"))
    });
}

fn guide() -> Value {
    json!({
        "defaults": [], "directory": true, "revision": 3, "saved": true,
        "updatedAt": "2026-11-20T10:00:00Z", "updatedBy": "bob",
        "guide": {"groups": [{"name": "Games"}],
                  "entries": [{"channel": "#help", "state": "everyone", "description": "Ask here"},
                              {"channel": "#chess", "group": "Games", "state": "suggested"}]},
        "requests": [{"channel": "#go", "createdAt": "2026-11-20T09:00:00Z", "requester": "eve",
                      "group": "games", "description": "Baduk", "members": 4}],
        "warnings": [{"channel": "#chess", "kind": "unregistered"}]
    })
}

#[gpui_kit::test]
fn the_channel_guide_is_edited_previewed_and_saved(cx: &mut TestAppContext) {
    let f = open(cx, |server| {
        server.route("GET", "/api/v1/administration/channel-guide", 200, guide());
        server.route_fn("PUT", "/api/v1/administration/channel-guide", |r| {
            let mut saved = guide();
            saved["guide"] = r.json()["guide"].clone();
            saved["revision"] = json!(4);
            saved["requests"] = json!([]);
            Reply::json(200, saved)
        });
        server.route(
            "GET",
            "/api/v1/administration/channel-guide/channels",
            200,
            json!({"items": [{"name": "#rust", "users": 7, "registered": true, "topic": "Crabs"}]}),
        );
    });
    page(&f, cx);
    click(&f, cx, "admin-tab-welcome");
    reads(
        &f,
        cx,
        "admin-guide-1-warning",
        "Nobody registered this channel, so it could disappear or be taken over.",
    );
    // The preview: for everyone first and ticked, then the group, the
    // suggested channel ticked.
    reads(&f, cx, "admin-preview-0", "#help, ticked");
    reads(&f, cx, "admin-preview-1", "#chess, ticked");

    // Change #chess to listed; the preview leaves it unticked.
    click(&f, cx, "admin-guide-1-state-listed");
    reads(&f, cx, "admin-preview-1", "#chess");

    // Add a channel found on the IRC server, and a group.
    type_into(&f, cx, "admin-guide-add-input", "ru");
    asked(
        &f,
        cx,
        "GET",
        "/api/v1/administration/channel-guide/channels",
    );
    reads(&f, cx, "admin-directory-0", "#rust, 7 members");
    click(&f, cx, "admin-directory-0");
    shows(&f, cx, "admin-guide-2");
    type_into(&f, cx, "admin-new-group", "Code");
    click(&f, cx, "admin-add-group");
    click(&f, cx, "admin-guide-2-group-2");
    click(&f, cx, "admin-guide-save");
    let saved = f
        .server
        .wait_for("PUT", "/api/v1/administration/channel-guide", WAIT);
    let body = saved.json();
    assert_eq!(body["revision"], 3);
    assert_eq!(
        body["guide"]["groups"],
        json!([{"name": "Games"}, {"name": "Code"}])
    );
    assert_eq!(body["guide"]["entries"][1]["state"], "listed");
    assert_eq!(
        body["guide"]["entries"][2],
        json!({"channel": "#rust", "group": "Code", "state": "listed"})
    );
    reads(&f, cx, "admin-guide-notice", "Channel guide saved.");
}

#[gpui_kit::test]
fn listing_requests_are_approved_or_declined(cx: &mut TestAppContext) {
    let f = open(cx, |server| {
        server.route("GET", "/api/v1/administration/channel-guide", 200, guide());
        server.route_reply(
            "DELETE",
            "/api/v1/administration/channel-guide/requests/*",
            Reply::empty(),
        );
        server.route_reply(
            "PUT",
            "/api/v1/administration/channel-guide",
            Reply::problem(409, "the guide changed"),
        );
    });
    page(&f, cx);
    click(&f, cx, "admin-tab-welcome");
    reads(
        &f,
        cx,
        "admin-request-0",
        &format!(
            "#go. Asked by eve on {}. Baduk",
            chrono::DateTime::parse_from_rfc3339("2026-11-20T09:00:00Z")
                .unwrap()
                .with_timezone(&chrono::Local)
                .format("%-d %B %Y %H:%M")
        ),
    );

    // Approving adds it to the existing group and saves; another owner
    // saved meanwhile, so the guide asks to be reloaded.
    click(&f, cx, "admin-request-0-approve");
    let put = f
        .server
        .wait_for("PUT", "/api/v1/administration/channel-guide", WAIT);
    assert_eq!(
        put.json()["guide"]["entries"][2],
        json!({"channel": "#go", "group": "Games", "description": "Baduk", "state": "listed"})
    );
    reads(
        &f,
        cx,
        "admin-guide-error",
        "Another owner changed the channel guide. Reload it to see their changes, then make yours again.",
    );
    click(&f, cx, "admin-guide-reload");
    shows(&f, cx, "admin-request-0");
    assert!(gone(&f, cx, "admin-guide-error"));

    click(&f, cx, "admin-request-0-decline");
    let declined = f.server.wait_for(
        "DELETE",
        "/api/v1/administration/channel-guide/requests/*",
        WAIT,
    );
    assert_eq!(
        declined.path,
        "/api/v1/administration/channel-guide/requests/%23go"
    );
    reads(
        &f,
        cx,
        "admin-guide-notice",
        "Declined the request for #go.",
    );
    assert!(gone(&f, cx, "admin-request-0"));
}

#[gpui_kit::test]
fn emoji_management_is_limited_to_chosen_accounts(cx: &mut TestAppContext) {
    let f = open(cx, |server| {
        server.route(
            "GET",
            "/api/v1/administration/emoji",
            200,
            json!({"restricted": false, "managers": []}),
        );
        server.route_fn("PUT", "/api/v1/administration/emoji", |r| {
            let body = r.json();
            let managers: Vec<Value> = body["managers"]
                .as_array()
                .unwrap()
                .iter()
                .map(|id| user(id.as_str().unwrap(), "eve", false, None))
                .collect();
            Reply::json(
                200,
                json!({"restricted": body["restricted"], "managers": managers}),
            )
        });
        server.route(
            "GET",
            "/api/v1/administration/identities",
            200,
            json!({"items": [{"account": "eve", "eligible": true,
                              "user": user("a-eve", "eve", false, None)}]}),
        );
    });
    page(&f, cx);
    click(&f, cx, "admin-tab-emoji");
    reads(
        &f,
        cx,
        "admin-emoji-none",
        "No managers yet. Server owners can still manage every emoji.",
    );
    click(&f, cx, "admin-emoji-restricted");
    let put = f
        .server
        .wait_for("PUT", "/api/v1/administration/emoji", WAIT);
    assert_eq!(put.json(), json!({"restricted": true, "managers": []}));

    type_into(&f, cx, "admin-emoji-add-input", "eve");
    reads(&f, cx, "admin-emoji-add-match-0", "eve");
    frame(&f, cx, |window, cx| window.press("enter", cx));
    let put = f
        .server
        .wait_for_nth("PUT", "/api/v1/administration/emoji", 2, WAIT);
    assert_eq!(
        put.json(),
        json!({"restricted": true, "managers": ["a-eve"]})
    );
    reads(&f, cx, "admin-emoji-manager-0", "eve");

    click(&f, cx, "admin-emoji-manager-0-remove");
    let put = f
        .server
        .wait_for_nth("PUT", "/api/v1/administration/emoji", 3, WAIT);
    assert_eq!(put.json(), json!({"restricted": true, "managers": []}));
}

#[gpui_kit::test]
fn a_refused_request_closes_administration(cx: &mut TestAppContext) {
    let f = open(cx, |_| {});
    page(&f, cx);
    reads(&f, cx, "admin-health", "Health: Healthy");
    // Ownership was taken away elsewhere: the next request is refused, the
    // client asks again and the page and its entry go.
    f.server.route_reply(
        "GET",
        "/api/v1/administration/status",
        Reply::problem(403, "only server owners can do this"),
    );
    f.server.route(
        "GET",
        "/api/v1/administration/access",
        200,
        json!({"owner": false}),
    );
    click(&f, cx, "admin-status-refresh");
    shows(&f, cx, "admin-not-owner");
    assert!(gone(&f, cx, "sidebar-administration"));
}

//! Tests for package SV1: the server window, raw lines and connection
//! details, against the canned server.

use std::time::Duration;

use gpui_kit::test::TestWindowExt;
use gpui_kit::{AppContext, TestAppContext};
use ircord_app::route::Route;
use ircord_client::store::Connection;
use ircord_client::store::server_log::Buffer;
use ircord_client::testing::{CannedServer, frame};
use serde_json::{Value, json};

use crate::fixture::{Fixture, open_with, wait};

const WAIT: Duration = Duration::from_secs(5);

fn line(id: &str, kind: &str, command: &str, text: &str) -> Value {
    json!({"id": id, "networkId": "n1", "kind": kind, "command": command, "params": [],
           "text": text, "time": "2026-11-21T10:00:00Z"})
}

fn routes(server: &CannedServer) {
    server.route(
        "GET",
        "/api/v1/networks/n1/server-log",
        200,
        json!({"items": [
            line("1", "numeric", "001", "Welcome to the network"),
            line("2", "raw", "PING", "a"),
            line("3", "raw", "PING", "b"),
            line("4", "raw", "PING", "c"),
            line("5", "motd", "372", "Be nice"),
        ]}),
    );
    server.route(
        "GET",
        "/api/v1/networks/n1/connection/trace",
        200,
        json!({"items": [line("t1", "raw", "PRIVMSG", "hello on the wire")]}),
    );
    server.route(
        "GET",
        "/api/v1/networks/n1/connection",
        200,
        json!({"networkId": "n1", "state": "connected", "serverName": "irc.example",
               "isupport": {"NETWORK": "Example", "WHOX": ""},
               "offeredCapabilities": {"sasl": "PLAIN", "echo-message": ""},
               "negotiatedCapabilities": ["echo-message"], "motd": [],
               "tls": {"version": "TLS 1.3", "cipher": "TLS_AES_128_GCM_SHA256",
                       "verified": true, "serverName": "irc.example", "forwardSecrecy": true,
                       "grade": "A+", "gradeNotes": [], "keyAlgorithm": "ECDSA",
                       "keyBits": 256}}),
    );
    server.route(
        "GET",
        "/api/v1/networks/n1/connection/latency",
        200,
        json!({"networkId": "n1", "intervalSeconds": 30, "windowSeconds": 1800,
               "samples": []}),
    );
    server.route(
        "GET",
        "/api/v1/ping",
        200,
        json!({"serverTime": "2026-11-21T10:00:00Z"}),
    );
    server.route(
        "POST",
        "/api/v1/networks/*/commands",
        202,
        json!({"clientId": "x"}),
    );
}

fn open(cx: &mut TestAppContext) -> Fixture {
    let f = open_with(cx, true, routes);
    wait(cx, "the live socket", |cx| {
        let session = f.session.read(cx);
        session.client().is_some_and(|c| {
            let w = c.read();
            w.connection == Some(Connection::Live) && w.timelines.contains_key("c-general")
        }) && session.conversation.is_some()
    });
    f
}

fn shows(f: &Fixture, cx: &mut TestAppContext, id: &'static str) -> bool {
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.try_find(id).is_some_and(|e| e.visible())
    })
    .unwrap()
}

fn click(f: &Fixture, cx: &mut TestAppContext, id: &'static str) {
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.click(id, cx);
    })
    .unwrap();
    cx.run_until_parked();
}

fn label(f: &Fixture, cx: &mut TestAppContext, id: &'static str) -> String {
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.find(id).label().unwrap_or_default().to_string()
    })
    .unwrap()
}

fn open_window(f: &Fixture, cx: &mut TestAppContext) {
    f.session
        .update(cx, |s, cx| s.navigate(Route::ServerWindow("n1".into()), cx));
    wait(cx, "the server log", |cx| {
        let session = f.session.read(cx);
        let w = session.client().unwrap().read();
        w.server_log
            .log("n1", Buffer::Log)
            .is_some_and(|l| l.loaded && l.lines.len() == 5)
    });
}

#[gpui_kit::test]
fn the_server_window_folds_runs_and_sends_a_raw_line(cx: &mut TestAppContext) {
    let f = open(cx);
    open_window(&f, cx);
    assert!(shows(&f, cx, "server-window"));
    assert!(shows(&f, cx, "server-line-1"));
    assert!(shows(&f, cx, "server-line-5"));
    // Three pings in a row are one row until unfolded.
    assert!(shows(&f, cx, "server-group:2"));
    assert!(!shows(&f, cx, "server-line-3"));
    assert!(label(&f, cx, "server-group:2").contains("3 pings"));
    click(&f, cx, "server-group:2");
    assert!(shows(&f, cx, "server-line-3"));
    click(&f, cx, "server-group:2");
    assert!(!shows(&f, cx, "server-line-3"));

    // A live line appends.
    f.server.push(frame(
        4,
        "server.line",
        &line("6", "notice", "NOTICE", "Hello from services"),
    ));
    wait(cx, "the live line", |cx| {
        let session = f.session.read(cx);
        let w = session.client().unwrap().read();
        w.server_log.logs["n1"].lines.len() == 6
    });
    assert!(shows(&f, cx, "server-line-6"));

    // A raw line goes to the network's commands.
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.click("server-window-raw", cx);
        window.input("WHOIS bob", cx);
        window.press("enter", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let sent = f
        .server
        .wait_for("POST", "/api/v1/networks/n1/commands", WAIT);
    assert_eq!(sent.json()["line"], "WHOIS bob");
}

#[gpui_kit::test]
fn the_filter_shows_matching_lines_unfolded(cx: &mut TestAppContext) {
    let f = open(cx);
    open_window(&f, cx);
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.click("server-window-filter", cx);
        window.input("ping", cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert!(shows(&f, cx, "server-line-2"));
    assert!(shows(&f, cx, "server-line-4"));
    assert!(!shows(&f, cx, "server-line-1"));
    assert!(!shows(&f, cx, "server-group:2"));
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.input("zzz", cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert!(label(&f, cx, "server-window-empty").contains("No server lines match"));
}

#[gpui_kit::test]
fn the_connection_tab_shows_tls_capabilities_and_settings(cx: &mut TestAppContext) {
    let f = open(cx);
    open_window(&f, cx);
    click(&f, cx, "server-window-tab-connection");
    wait(cx, "the connection details", |cx| {
        let session = f.session.read(cx);
        let w = session.client().unwrap().read();
        w.server_log
            .connections
            .get("n1")
            .is_some_and(|c| c.value.is_some())
    });
    assert!(shows(&f, cx, "connection-details"));
    assert_eq!(label(&f, cx, "connection-grade"), "Security grade A+");
    assert_eq!(
        label(&f, cx, "connection-capability-count"),
        "1 of 2 active"
    );
    assert!(label(&f, cx, "connection-state").contains("irc.example"));
    // Further down the page, out of view until scrolled to.
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("connection-isupport").is_some());
    })
    .unwrap();
    // The trace tab reads the trace.
    click(&f, cx, "server-window-tab-trace");
    wait(cx, "the trace", |cx| {
        let session = f.session.read(cx);
        let w = session.client().unwrap().read();
        w.server_log
            .log("n1", Buffer::Trace)
            .is_some_and(|l| l.loaded)
    });
    assert!(shows(&f, cx, "server-line-t1"));
    assert!(!shows(&f, cx, "server-window-raw"));
}

#[gpui_kit::test]
fn the_indicator_shows_the_ping_and_opens_the_server_window(cx: &mut TestAppContext) {
    let f = open(cx);
    wait(cx, "the first ping", |cx| {
        let session = f.session.read(cx);
        let w = session.client().unwrap().read();
        !w.server_log.device.is_empty()
    });
    assert!(shows(&f, cx, "connection-indicator"));
    assert!(shows(&f, cx, "connection-latency"));
    let text = label(&f, cx, "connection-indicator");
    assert!(text.starts_with("Connected · "), "{text}");
    assert!(text.ends_with(" ms to IRCord"), "{text}");
    click(&f, cx, "connection-indicator");
    assert_eq!(
        cx.update(|cx| f.session.read(cx).route()),
        Some(Route::ServerWindow("n1".into()))
    );
}

#[gpui_kit::test]
fn sending_while_disconnected_says_so(cx: &mut TestAppContext) {
    let mut snapshot = crate::fixture::snapshot();
    snapshot.networks[0].status = ircord_client::contract::NetworkStatus::Disconnected;
    let f = open_with(cx, true, |server| {
        routes(server);
        server.snapshot(&snapshot);
    });
    wait(cx, "the live socket", |cx| {
        f.session
            .read(cx)
            .client()
            .is_some_and(|c| c.read().connection == Some(Connection::Live))
    });
    open_window(&f, cx);
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.click("server-window-raw", cx);
        window.input("WHOIS bob", cx);
        window.press("enter", cx);
    })
    .unwrap();
    cx.run_until_parked();
    std::thread::sleep(Duration::from_millis(100));
    assert!(
        f.server
            .requests_to("POST", "/api/v1/networks/n1/commands")
            .is_empty()
    );
    assert!(label(&f, cx, "connection-indicator").starts_with("IRC disconnected"));
}

#[gpui_kit::test]
fn the_server_window_opens_on_overview_cards_over_the_log(cx: &mut TestAppContext) {
    let f = open(cx);
    open_window(&f, cx);
    wait(cx, "the connection details", |cx| {
        let session = f.session.read(cx);
        let w = session.client().unwrap().read();
        w.server_log
            .connections
            .get("n1")
            .is_some_and(|c| c.value.is_some())
    });
    // As the web's: the first tab is the overview, with four cards and
    // the log under them.
    assert_eq!(label(&f, cx, "server-window-tab-log"), "Overview");
    for id in [
        "server-window-card-status",
        "server-window-card-latency",
        "server-window-card-security",
        "server-window-card-session",
        "server-line-1",
    ] {
        assert!(shows(&f, cx, id), "{id} is shown");
    }
    assert_eq!(label(&f, cx, "server-window-status-value"), "Connected");
    assert_eq!(label(&f, cx, "server-window-grade"), "Security grade A+");
    assert_eq!(label(&f, cx, "server-window-title"), "Home · Server");
}

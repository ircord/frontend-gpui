//! Folding ported from the web client's `shared/utils/server-log.test.ts`,
//! the events, and the requests against the canned server.

use std::time::Duration;

use serde_json::json;

use super::*;
use crate::Client;
use crate::testing::{CannedServer, SnapshotBuilder, frame, wait_live, wait_until};

fn line(id: &str, kind: ServerLineKind, command: &str, text: &str) -> ServerLine {
    ServerLine {
        id: id.into(),
        network_id: "n1".into(),
        kind,
        command: command.into(),
        text: text.into(),
        time: "2026-11-21T10:00:00Z".into(),
        ..Default::default()
    }
}

fn from(mut l: ServerLine, source: &str) -> ServerLine {
    l.source = Some(source.into());
    l
}

fn ids(items: &[Item<'_>]) -> Vec<String> {
    items
        .iter()
        .map(|i| match i {
            Item::Line(l) => l.command.clone(),
            Item::Group { family, .. } => format!("group:{family:?}"),
        })
        .collect()
}

#[test]
fn echoes_of_sent_lines_drop_and_read_markers_fold() {
    use ServerLineKind::*;
    let lines = vec![
        line("1", Outgoing, "MARKREAD", "timestamp=1"),
        from(line("2", Raw, "MARKREAD", "timestamp=1"), "irc.example"),
        line("3", Outgoing, "MARKREAD", "timestamp=2"),
        from(line("4", Raw, "MARKREAD", "timestamp=2"), "irc.example"),
        line("5", Outgoing, "MARKREAD", "timestamp=3"),
        line("6", Numeric, "001", "Welcome"),
    ];
    let items = fold(&lines, MIN_GROUP);
    assert_eq!(items.len(), 2);
    let Item::Group { id, family, lines } = &items[0] else {
        panic!("{items:?}")
    };
    assert_eq!(id, "group:1");
    assert_eq!(*family, Family::Markread);
    let kept: Vec<&str> = lines.iter().map(|l| l.id.as_str()).collect();
    assert_eq!(kept, ["1", "3", "5"]);
    assert!(matches!(items[1], Item::Line(l) if l.command == "001"));
}

#[test]
fn histserv_presence_folds_and_short_runs_stay_lines() {
    use ServerLineKind::*;
    let hist = |id, text| {
        from(
            line(id, Notice, "NOTICE", text),
            "HistServ!HistServ@services",
        )
    };
    let lines = vec![
        hist("1", "[#general] jay quit (connection closed)"),
        hist("2", "[#general] jay [account: jay] joined the channel"),
        hist("3", "[#general] ana quit (connection closed)"),
        line("4", Raw, "AWAY", "away"),
        line("5", Numeric, "306", "You have been marked as being away"),
        hist("6", "Some other notice"),
    ];
    assert_eq!(
        ids(&fold(&lines, MIN_GROUP)),
        ["group:Presence", "AWAY", "306", "NOTICE"]
    );
}

#[test]
fn families() {
    use ServerLineKind::*;
    assert_eq!(family(&line("1", Raw, "pong", "")), Some(Family::Ping));
    assert_eq!(
        family(&line("1", Raw, "CHATHISTORY", "")),
        Some(Family::Batch)
    );
    assert_eq!(family(&line("1", Raw, "NICK", "")), Some(Family::Presence));
    assert_eq!(family(&line("1", Numeric, "305", "")), Some(Family::Away));
    assert_eq!(family(&line("1", Numeric, "372", "")), None);
    // A notice about quitting from anyone but HistServ stays a line.
    let other = from(line("1", Notice, "NOTICE", "jay quit"), "HistServant");
    assert_eq!(family(&other), None);
    let quitting = from(line("1", Notice, "NOTICE", "jay quitting soon"), "HistServ");
    assert_eq!(family(&quitting), None);
    let nick = from(
        line("1", Notice, "NOTICE", "jay changed their nick to j"),
        "histserv",
    );
    assert_eq!(family(&nick), Some(Family::Presence));
}

#[test]
fn filters_match_command_source_and_plain_text() {
    let l = from(
        line("1", ServerLineKind::Notice, "NOTICE", "\x02Hello\x02 there"),
        "NickServ!s@services",
    );
    assert!(matches(&l, "hello there"));
    assert!(matches(&l, "nickserv"));
    assert!(matches(&l, "notice"));
    assert!(matches(&l, "  "));
    assert!(!matches(&l, "welcome"));
    assert_eq!(source_nick(&l), Some("NickServ"));
}

#[test]
fn raw_lines_are_checked() {
    assert_eq!(check_raw("  WHOIS bob  "), Ok("WHOIS bob".into()));
    assert_eq!(check_raw("   "), Err(RawProblem::Empty));
    assert_eq!(check_raw("A\nB"), Err(RawProblem::LineBreak));
    assert_eq!(
        check_raw(&"x".repeat(RAW_MAX + 1)),
        Err(RawProblem::TooLong)
    );
}

#[test]
fn latency_pairs_hops_and_goes_stale() {
    let at = |s: &str| LatencySample {
        at: format!("2026-11-21T10:00:{s}Z"),
        ms: 10.0,
    };
    let base = parse_time("2026-11-21T10:00:00Z");
    let device = [
        DeviceSample {
            at_ms: base + 5_000,
            ms: 30.0,
        },
        DeviceSample {
            at_ms: base + 40_000,
            ms: 50.0,
        },
    ];
    let t = totals(&[at("00"), at("30")], &device);
    assert_eq!(
        t,
        [
            DeviceSample {
                at_ms: base,
                ms: 40.0
            },
            DeviceSample {
                at_ms: base + 30_000,
                ms: 60.0
            },
        ]
    );
    assert_eq!(average_and_peak(t.iter().map(|s| s.ms)), Some((50.0, 60.0)));
    assert_eq!(average_and_peak([]), None);

    let mut state = State {
        device: device.to_vec(),
        ..Default::default()
    };
    assert_eq!(state.device_latency(base + 40_000 + STALE_MS), Some(50.0));
    assert_eq!(state.device_latency(base + 40_001 + STALE_MS), None);
    state.ping_failing = true;
    assert_eq!(state.device_latency(base + 40_000), None);
}

#[test]
fn live_lines_append_once_loaded_and_updates_mark_stale() {
    let mut w = SnapshotBuilder::new().workspace();
    // Each event takes the next sequence number, as the server sends them.
    let seq = std::cell::Cell::new(w.cursor);
    let event = |kind: &str, data: serde_json::Value| {
        seq.set(seq.get() + 1);
        ServerEvent::decode(&frame(seq.get(), kind, &data)).unwrap()
    };
    let numeric = json!({"id": "s1", "networkId": "n1", "kind": "numeric", "command": "001",
                         "params": [], "text": "Welcome", "time": "2026-11-21T10:00:00Z"});
    // Not read yet: nothing to append to.
    assert!(
        w.apply(&event("server.line", numeric.clone()), 0)
            .is_empty()
    );
    w.server_log.logs.entry("n1".into()).or_default().loaded = true;
    let changes = w.apply(&event("server.line", numeric.clone()), 0);
    assert!(changes.touches_id(TOPIC, "n1"));
    // The same line twice is one line.
    w.apply(&event("server.line", numeric), 0);
    assert_eq!(w.server_log.logs["n1"].lines.len(), 1);

    let changes = w.apply(&event("server-log.updated", json!({"networkId": "n1"})), 0);
    assert!(changes.touches_id(TOPIC, "n1"));
    assert!(w.server_log.logs["n1"].stale);

    let changes = w.apply(
        &event(
            "connection.updated",
            json!({"networkId": "n1", "state": "connected", "serverName": "irc.example",
                   "isupport": {}, "offeredCapabilities": {}, "negotiatedCapabilities": [],
                   "motd": []}),
        ),
        0,
    );
    assert!(changes.touches_id(TOPIC, "n1"));
    let info = w.server_log.connections["n1"].value.as_ref().unwrap();
    assert_eq!(info.server_name, "irc.example");
}

#[test]
fn the_log_keeps_the_newest_lines() {
    let mut log = Log {
        loaded: true,
        ..Default::default()
    };
    for i in 0..KEEP + 5 {
        log.push(line(&i.to_string(), ServerLineKind::Raw, "PING", ""));
    }
    assert_eq!(log.lines.len(), KEEP);
    assert_eq!(log.lines[0].id, "5");
}

fn started() -> (CannedServer, Client) {
    let rt = crate::client::runtime().unwrap();
    let server = CannedServer::start(&rt);
    server.snapshot(&SnapshotBuilder::new().build());
    let (client, _updates) = Client::start(rt, server.api());
    wait_live(&client);
    (server, client)
}

const WAIT: Duration = Duration::from_secs(5);

#[test]
fn pages_of_the_log_and_the_trace() {
    let (server, client) = started();
    server.route_fn("GET", "/api/v1/networks/n1/server-log", |r| {
        let items = match r.query_param("before").as_deref() {
            None => vec![
                line("3", ServerLineKind::Numeric, "001", "Welcome"),
                line("4", ServerLineKind::Motd, "372", "Hi"),
            ],
            Some("3") => vec![line("1", ServerLineKind::Notice, "NOTICE", "Looking up")],
            Some(other) => panic!("before={other}"),
        };
        let next = r.query_param("before").is_none().then_some("3");
        crate::testing::Reply::json(200, json!({"items": items, "nextCursor": next}))
    });
    server.route(
        "GET",
        "/api/v1/networks/n1/connection/trace",
        200,
        json!({"items": [line("t1", ServerLineKind::Raw, "PRIVMSG", "hi")]}),
    );
    load(client.handle(), "n1", Buffer::Log);
    wait_until(&client, WAIT, "the first page", |w| {
        w.server_log
            .log("n1", Buffer::Log)
            .is_some_and(|l| l.loaded)
    });
    let r = server.wait_for("GET", "/api/v1/networks/n1/server-log", WAIT);
    assert_eq!(r.query_param("limit").as_deref(), Some("100"));
    load_older(client.handle(), "n1", Buffer::Log);
    wait_until(&client, WAIT, "the older page", |w| {
        w.server_log.logs["n1"].lines.len() == 3
    });
    {
        let w = client.read();
        let log = &w.server_log.logs["n1"];
        let order: Vec<&str> = log.lines.iter().map(|l| l.id.as_str()).collect();
        assert_eq!(order, ["1", "3", "4"]);
        assert_eq!(log.older, None);
    }
    load(client.handle(), "n1", Buffer::Trace);
    wait_until(&client, WAIT, "the trace", |w| {
        w.server_log
            .log("n1", Buffer::Trace)
            .is_some_and(|l| l.lines.len() == 1)
    });
}

#[test]
fn details_latency_ping_and_raw_lines() {
    let (server, client) = started();
    server.route(
        "GET",
        "/api/v1/networks/n1/connection",
        200,
        json!({"networkId": "n1", "state": "connected", "serverName": "irc.example",
               "isupport": {"NETWORK": "Example"}, "offeredCapabilities": {"sasl": "PLAIN"},
               "negotiatedCapabilities": ["sasl"], "motd": [],
               "tls": {"version": "TLS 1.3", "cipher": "TLS_AES_128_GCM_SHA256",
                       "verified": true, "serverName": "irc.example", "forwardSecrecy": true,
                       "grade": "A+", "gradeNotes": []}}),
    );
    server.route(
        "GET",
        "/api/v1/networks/n1/connection/latency",
        200,
        json!({"networkId": "n1", "intervalSeconds": 30, "windowSeconds": 1800,
               "samples": [{"at": "2026-11-21T10:00:00Z", "ms": 12.5}]}),
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

    load_connection(client.handle(), "n1");
    load_latency(client.handle(), "n1");
    let rt = client.runtime().clone();
    rt.block_on(ping(client.handle())).unwrap();
    wait_until(&client, WAIT, "details and latency", |w| {
        w.server_log
            .connections
            .get("n1")
            .is_some_and(|c| c.value.is_some())
            && w.server_log
                .latency
                .get("n1")
                .is_some_and(|c| c.value.is_some())
    });
    {
        let w = client.read();
        let info = w.server_log.connections["n1"].value.as_ref().unwrap();
        assert_eq!(info.tls.as_ref().unwrap().grade, TLSInfoGrade::APlus);
        assert_eq!(
            w.server_log.latency["n1"].value.as_ref().unwrap().samples[0].ms,
            12.5
        );
        assert_eq!(w.server_log.device.len(), 1);
        assert!(!w.server_log.ping_failing);
    }

    rt.block_on(send_raw(client.handle(), "n1", "WHOIS bob".into()))
        .unwrap()
        .unwrap();
    let r = server.wait_for("POST", "/api/v1/networks/n1/commands", WAIT);
    assert_eq!(r.json()["line"], "WHOIS bob");
    assert!(!r.json()["clientId"].as_str().unwrap().is_empty());
}

#[test]
fn failures_are_kept_for_the_view() {
    let (server, client) = started();
    server.route_reply(
        "GET",
        "/api/v1/networks/n1/connection",
        crate::testing::Reply::problem(503, "Not connected"),
    );
    load_connection(client.handle(), "n1");
    wait_until(&client, WAIT, "the failure", |w| {
        w.server_log
            .connections
            .get("n1")
            .is_some_and(|c| c.error.as_deref() == Some("Not connected"))
    });
    // No /ping route: the canned server answers 404, which is no sample
    // but not an unreachable server either.
    let rt = client.runtime().clone();
    rt.block_on(ping(client.handle())).unwrap();
    assert!(!client.read().server_log.ping_failing);
    assert!(client.read().server_log.device.is_empty());
}

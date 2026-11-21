//! Every command and alias, planned against a workspace and run against the
//! canned server, checking the request each one makes.

use std::time::Duration;

use serde_json::{Value, json};

use super::*;
use crate::testing::{CannedServer, ME, Reply, SnapshotBuilder, wait_live};

/// What one typed line should come to.
enum Expect {
    /// A raw IRC line sent on `n1`.
    Raw(&'static str),
    /// `POST /api/v1/conversations` with this body; the conversation opens.
    Opens(Value),
    /// `DELETE /api/v1/conversations/c-a` with this query.
    Parts(Option<&'static str>),
    Attracts,
    /// A plan the app carries out itself.
    Is(Plan),
    Fails(Problem),
}

fn snapshot() -> SnapshotBuilder {
    SnapshotBuilder::new()
        .with_network(|n| n.features.attract = true)
        .member("m-bob", "bob")
        .channel("c-a", "alpha")
        .in_conversation("c-a", ME, "@")
        .in_conversation("c-a", "m-bob", "")
        .direct("c-d", "m-bob")
}

fn started() -> (CannedServer, Client) {
    let rt = crate::client::runtime().unwrap();
    let server = CannedServer::start(&rt);
    server.snapshot(&snapshot().build());
    server.route(
        "POST",
        "/api/v1/networks/*/commands",
        202,
        json!({"clientId": "x"}),
    );
    server.route_fn("POST", "/api/v1/conversations", |r| {
        let body = r.json();
        let (kind, name) = match body["name"].as_str() {
            Some(name) => ("channel", name.trim_start_matches('#').to_string()),
            None => (
                "direct",
                body["nickname"].as_str().unwrap_or("bob").to_string(),
            ),
        };
        Reply::json(
            200,
            json!({"id": format!("c-{name}"), "networkId": "n1", "kind": kind,
                   "name": name, "joined": true}),
        )
    });
    server.route_reply("DELETE", "/api/v1/conversations/*", Reply::empty());
    server.route(
        "POST",
        "/api/v1/conversations/*/attract",
        200,
        json!({"id": "m1", "conversationId": "c-a"}),
    );
    let (client, _updates) = Client::start(rt, server.api());
    wait_live(&client);
    (server, client)
}

fn raw(line: &'static str) -> Expect {
    Expect::Raw(line)
}

fn open(view: View, args: &str) -> Expect {
    Expect::Is(Plan::Open {
        view,
        args: args.into(),
    })
}

fn spec(name: &str) -> &'static Spec {
    catalog::resolve(name).unwrap()
}

#[test]
fn every_command_and_alias_makes_its_request() {
    use Expect::*;
    let table: Vec<(&str, &str, Expect)> = vec![
        ("c-a", "hello", Is(Plan::Text("hello".into()))),
        ("c-a", "//etc/hosts", Is(Plan::Text("//etc/hosts".into()))),
        ("c-a", "/me waves", Is(Plan::Text("/me waves".into()))),
        ("c-a", "/me", Fails(Problem::Usage(spec("me")))),
        (
            "c-a",
            "/slap _jules_",
            Is(Plan::Action(
                "slaps _jules_ around a bit with a large cucumber".into(),
            )),
        ),
        ("c-a", "/slap a b", Fails(Problem::Usage(spec("slap")))),
        (
            "c-a",
            "/msg bob hi there",
            Is(Plan::Direct {
                network: "n1".into(),
                member: Some("m-bob".into()),
                nickname: None,
                text: Some("hi there".into()),
            }),
        ),
        (
            "c-a",
            "/msg stranger hi there",
            raw("PRIVMSG stranger :hi there"),
        ),
        (
            "c-a",
            "/msg Bob hi",
            Opens(json!({"networkId": "n1", "memberId": "m-bob"})),
        ),
        (
            "c-a",
            "/query carol",
            Opens(json!({"networkId": "n1", "nickname": "carol"})),
        ),
        (
            "c-a",
            "/q bob",
            Opens(json!({"networkId": "n1", "memberId": "m-bob"})),
        ),
        (
            "c-a",
            "/query 9lives",
            Fails(Problem::PersonMissing("9lives".into())),
        ),
        (
            "c-a",
            "/join #rust",
            Opens(json!({"networkId": "n1", "name": "#rust"})),
        ),
        (
            "c-a",
            "/j #vault sesame",
            Opens(json!({"networkId": "n1", "name": "#vault", "key": "sesame"})),
        ),
        ("c-a", "/part", Parts(None)),
        ("c-a", "/leave going home", Parts(Some("reason=going+home"))),
        ("c-d", "/part", Fails(Problem::ChannelOnly("part"))),
        ("c-a", "/nick newme", raw("NICK newme")),
        (
            "c-a",
            "/nick two words",
            Fails(Problem::Nickname(NickError::Spaces)),
        ),
        (
            "c-a",
            "/nick 1abc",
            Fails(Problem::Nickname(NickError::Syntax)),
        ),
        ("c-a", "/topic", raw("TOPIC #alpha")),
        ("c-a", "/topic Fresh news", raw("TOPIC #alpha :Fresh news")),
        ("c-a", "/away", raw("AWAY :Away")),
        ("c-a", "/away lunch", raw("AWAY :lunch")),
        ("c-a", "/afk", raw("AWAY :AFK")),
        ("c-a", "/brb coffee", raw("AWAY :BRB: coffee")),
        ("c-a", "/back", raw("AWAY")),
        ("c-a", "/op bob", raw("MODE #alpha +o bob")),
        ("c-a", "/deop bob", raw("MODE #alpha -o bob")),
        ("c-a", "/voice bob", raw("MODE #alpha +v bob")),
        ("c-a", "/devoice bob", raw("MODE #alpha -v bob")),
        ("c-a", "/kick bob", raw("KICK #alpha bob")),
        (
            "c-a",
            "/kick bob too loud",
            raw("KICK #alpha bob :too loud"),
        ),
        ("c-a", "/ban bob", raw("MODE #alpha +b bob!*@*")),
        (
            "c-a",
            "/ban *!*@bad.host",
            raw("MODE #alpha +b *!*@bad.host"),
        ),
        ("c-a", "/invite carol", raw("INVITE carol #alpha")),
        ("c-d", "/invite carol #rust", raw("INVITE carol #rust")),
        (
            "c-d",
            "/invite carol",
            Fails(Problem::Usage(spec("invite"))),
        ),
        ("c-a", "/mode +m", raw("MODE #alpha +m")),
        ("c-a", "/mode me +i", raw("MODE me +i")),
        ("c-a", "/whois bob", raw("WHOIS bob")),
        ("c-a", "/w bob", raw("WHOIS bob")),
        (
            "c-a",
            "/ignore bob",
            Is(Plan::Ignore {
                unignore: false,
                arg: "bob".into(),
            }),
        ),
        (
            "c-a",
            "/unignore bob",
            Is(Plan::Ignore {
                unignore: true,
                arg: "bob".into(),
            }),
        ),
        ("c-a", "/list rust", open(View::ChannelList, "rust")),
        ("c-a", "/server", open(View::ServerWindow, "")),
        ("c-a", "/settings", open(View::ChannelSettings, "")),
        ("c-a", "/banlist", open(View::BanList, "")),
        ("c-a", "/gif cats", open(View::Gif, "cats")),
        (
            "c-a",
            "/status 🍕 lunch --for 1h",
            open(View::Status, "🍕 lunch --for 1h"),
        ),
        ("c-a", "/quote PING :x", raw("PING :x")),
        ("c-a", "/raw PING :y", raw("PING :y")),
        ("c-a", "/attract", Attracts),
        ("c-d", "/attract", Fails(Problem::Unknown("attract".into()))),
        (
            "c-a",
            "/frobnicate",
            Fails(Problem::Unknown("frobnicate".into())),
        ),
    ];
    // Every command and alias is in the table.
    for name in catalog::COMMANDS
        .iter()
        .map(|c| c.name)
        .chain(catalog::ALIASES.iter().map(|a| a.0))
    {
        assert!(
            table
                .iter()
                .any(|(_, input, _)| input[1..].split(' ').next() == Some(name)),
            "/{name} is not in the table"
        );
    }

    let (server, client) = started();
    let wait = Duration::from_secs(10);
    let (mut raws, mut opens, mut parts, mut attracts) = (0, 0, 0, 0);
    for (conversation, input, expect) in table {
        let planned = plan(&client.read(), conversation, input);
        let planned = match (planned, expect) {
            (Err(problem), Fails(want)) => {
                assert_eq!(problem, want, "{input}");
                continue;
            }
            (Ok(p), Is(want)) => {
                assert_eq!(p, want, "{input}");
                continue;
            }
            (Ok(p), expect) => (p, expect),
            (Err(problem), _) => panic!("{input}: {problem:?}"),
        };
        let (p, expect) = planned;
        let done = client.runtime().block_on(execute(&client, p)).unwrap();
        let done = done.unwrap_or_else(|e| panic!("{input}: {e}"));
        match expect {
            Raw(line) => {
                raws += 1;
                let r = server.wait_for_nth("POST", "/api/v1/networks/n1/commands", raws, wait);
                assert_eq!(r.json()["line"], line, "{input}");
                assert_eq!(r.json()["clientId"].as_str().unwrap().len(), 32);
            }
            Opens(body) => {
                opens += 1;
                let r = server.wait_for_nth("POST", "/api/v1/conversations", opens, wait);
                assert_eq!(r.json(), body, "{input}");
                let id = done.open.expect("a conversation to open");
                assert!(client.read().conversation(&id).is_some(), "{input}");
                if input.starts_with("/msg") {
                    assert_eq!(done.send.as_deref(), Some("hi"));
                }
            }
            Parts(query) => {
                parts += 1;
                let r = server.wait_for_nth("DELETE", "/api/v1/conversations/c-a", parts, wait);
                assert_eq!(r.query.as_deref(), query, "{input}");
            }
            Attracts => {
                attracts += 1;
                server.wait_for_nth("POST", "/api/v1/conversations/c-a/attract", attracts, wait);
            }
            Is(_) | Fails(_) => unreachable!(),
        }
    }
}

#[test]
fn raw_lines_need_a_live_network() {
    let mut w = snapshot().workspace();
    w.connection = Some(Connection::Live);
    assert!(matches!(plan(&w, "c-a", "/back"), Ok(Plan::Raw { .. })));
    w.networks[0].status = NetworkStatus::Disconnected;
    assert_eq!(plan(&w, "c-a", "/back"), Err(Problem::NotConnected));
    // Commands that do not send a raw line still run.
    assert!(matches!(plan(&w, "c-a", "/join #x"), Ok(Plan::Join { .. })));
}

#[test]
fn attract_needs_the_network_feature_and_operators() {
    let mut w = snapshot()
        .with_network(|n| n.features.attract = false)
        .workspace();
    w.connection = Some(Connection::Live);
    assert_eq!(
        plan(&w, "c-a", "/attract"),
        Err(Problem::AttractUnsupported)
    );
    let mut w = SnapshotBuilder::new()
        .with_network(|n| n.features.attract = true)
        .channel("c-a", "alpha")
        .workspace();
    w.connection = Some(Connection::Live);
    assert_eq!(
        plan(&w, "c-a", "/attract"),
        Err(Problem::Unknown("attract".into()))
    );
}

#[test]
fn nicknames_follow_irc_rules() {
    assert_eq!(nickname_error("", 32), Some(NickError::Required));
    assert_eq!(nickname_error("[away]", 32), None);
    assert_eq!(nickname_error("a-b", 32), None);
    assert_eq!(nickname_error("-ab", 32), Some(NickError::Syntax));
    assert_eq!(nickname_error("abcdef", 5), Some(NickError::TooLong(5)));
    let isupport = [("NICKLEN".to_string(), "9".to_string())].into();
    assert_eq!(nickname_limit(&isupport), 9);
    assert_eq!(nickname_limit(&Default::default()), 32);
}

#[test]
fn ignore_uses_the_loaded_settings() {
    let mut w = snapshot().workspace();
    assert!(ignore_outcome(&w, "c-a", false, "bob").is_none());
    w.documents
        .notification_settings
        .read_done(Ok(c::Settings::default()));
    let out = ignore_outcome(&w, "c-a", false, "bob").unwrap();
    assert_eq!(out.ignore.unwrap(), ["bob"]);
    let out = ignore_outcome(&w, "c-a", false, "me").unwrap();
    assert_eq!(out.notice, ignore::Notice::Yourself);
}

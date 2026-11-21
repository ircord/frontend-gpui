//! Tests for package FD1 against the canned server: the search page finds a
//! message and opens it in the timeline (and remembers the search), typed
//! filters become chips and are sent, the filters dialog applies several at
//! once, and a conversation's own search beside the chat asks the server's
//! history and opens its results there.

use gpui_kit::test::TestWindowExt;
use gpui_kit::{AppContext, TestAppContext};
use ircord_app::features::search;
use ircord_app::route::Route;
use ircord_client::contract::*;
use ircord_client::search_filter::load_recent;
use ircord_client::testing::{CannedServer, Recorded, Reply};
use serde_json::json;

use crate::fixture::{Fixture, bootstrap, message, open_with, wait};

/// The search route answers with `#general`'s "old" message (not loaded in
/// the timeline) for any text, and the source the request asked for.
fn serve_search(server: &CannedServer) {
    server.route_fn("GET", "/api/v1/search", |r| {
        let mut old = message("m-old", "c-general", "hello from long ago");
        old.created_at = "2026-10-01T09:00:00Z".into();
        let upstream = r.query_param("source").as_deref() == Some("upstream");
        let mut items = vec![json!({
            "conversation": {"id": "c-general", "networkId": "n1", "kind": "channel",
                             "name": "general"},
            "message": serde_json::to_value(&old).unwrap(),
        })];
        if upstream {
            items.push(json!({
                "context": {"available": false, "identity": "ephemeral"},
                "senderNick": "Mira",
                "conversation": {"id": "c-general", "networkId": "n1", "kind": "channel",
                                 "name": "general"},
                "message": serde_json::to_value(Message {
                    sender_id: String::new(),
                    ..message("", "c-general", "hello upstream")
                })
                .unwrap(),
            }));
        }
        let source = if upstream {
            json!({"requested": "upstream", "actual": "upstream",
                   "coverage": "upstream-bounded", "completeness": "unknown"})
        } else {
            json!({"requested": "retained", "actual": "retained",
                   "coverage": "retained-indexed", "completeness": "unknown"})
        };
        Reply::json(
            200,
            json!({"items": items, "nextCursor": null, "source": source}),
        )
    });
    server.route_fn("GET", "/api/v1/conversations/*/messages", |r| {
        let mut items = vec![
            message("1", "c-general", "hello \x02world\x02 https://ircord.com"),
            message("2", "c-general", "second line"),
        ];
        if r.query_param("around").is_some() {
            let mut old = message("m-old", "c-general", "hello from long ago");
            old.created_at = "2026-10-01T09:00:00Z".into();
            items = vec![old];
        }
        if !r.path.ends_with("/c-general/messages") {
            items.clear();
        }
        let page = MessagePage {
            items,
            ..Default::default()
        };
        Reply::json(200, serde_json::to_value(page).unwrap())
    });
}

fn open(cx: &mut TestAppContext) -> Fixture {
    let f = open_with(cx, true, serve_search);
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
    frame(f, cx, |window, cx| window.click(id, cx));
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

fn label(f: &Fixture, cx: &mut TestAppContext, id: &'static str) -> Option<String> {
    let mut found = None;
    frame(f, cx, |window, _| {
        found = window.find(id).label().map(str::to_string);
    });
    found
}

/// Every value of `key` in the request's query, decoded.
fn param(r: &Recorded, key: &str) -> Vec<String> {
    let decode = |v: &str| {
        let v = v.replace('+', " ");
        let mut out = Vec::new();
        let b = v.as_bytes();
        let mut i = 0;
        while i < b.len() {
            if b[i] == b'%'
                && i + 2 < b.len()
                && let Ok(byte) = u8::from_str_radix(&v[i + 1..i + 3], 16)
            {
                out.push(byte);
                i += 3;
                continue;
            }
            out.push(b[i]);
            i += 1;
        }
        String::from_utf8_lossy(&out).into_owned()
    };
    r.query
        .as_deref()
        .unwrap_or_default()
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .filter(|(k, _)| *k == key)
        .map(|(_, v)| decode(v))
        .collect()
}

/// Wait for a search request that `matches`.
fn searched(f: &Fixture, cx: &mut TestAppContext, what: &str, matches: impl Fn(&Recorded) -> bool) {
    wait(cx, what, |_| {
        f.server
            .requests_to("GET", "/api/v1/search")
            .iter()
            .any(&matches)
    });
}

fn open_page(f: &Fixture, cx: &mut TestAppContext, query: Option<&str>) {
    let session = f.session.clone();
    let query = query.map(str::to_string);
    cx.update(|cx| search::open(&session, query.as_deref(), cx));
    shows(f, cx, "search-page");
}

#[gpui_kit::test]
fn searches_and_opens_a_result_in_the_timeline(cx: &mut TestAppContext) {
    let f = open(cx);
    open_page(&f, cx, None);
    assert_eq!(
        cx.update(|cx| f.session.read(cx).route()),
        Some(Route::Search)
    );
    frame(&f, cx, |window, cx| window.input("hello", cx));
    searched(&f, cx, "a search for hello", |r| {
        param(r, "q") == ["hello"] && param(r, "source") == ["retained"]
    });
    shows(&f, cx, "search-result-0");
    let row = label(&f, cx, "search-result-0").unwrap();
    assert!(row.starts_with("bob in #general"), "{row}");
    assert!(row.ends_with("hello from long ago"), "{row}");
    assert_eq!(label(&f, cx, "search-count").as_deref(), Some("1 message"));
    assert_eq!(
        label(&f, cx, "search-coverage").as_deref(),
        Some("Coverage: messages retained by IRCord; server history may include more.")
    );

    // Opening it loads the history around it and shows it in #general.
    click(&f, cx, "search-result-0");
    wait(cx, "the old message in the timeline", |cx| {
        let session = f.session.read(cx);
        let client = session.client().unwrap();
        session.route() == Some(Route::Conversation("c-general".into()))
            && client.read().focused("c-general") == Some("m-old")
    });

    // The search is remembered on this device.
    let recent = cx.update(|cx| load_recent(&f.session.read(cx).config, "u-me"));
    assert_eq!(recent.len(), 1);
    assert_eq!(recent[0].text, "hello");
}

#[gpui_kit::test]
fn typed_filters_become_chips_and_backspace_removes_them(cx: &mut TestAppContext) {
    let f = open(cx);
    open_page(&f, cx, None);
    frame(&f, cx, |window, cx| window.input("cats has:", cx));
    // A token being typed offers what it can be.
    shows(&f, cx, "search-offers");
    assert_eq!(label(&f, cx, "search-offer-1").as_deref(), Some("Image"));
    frame(&f, cx, |window, cx| window.input("image ", cx));
    shows(&f, cx, "search-chip-0");
    frame(&f, cx, |window, cx| window.input("from:bob ", cx));
    shows(&f, cx, "search-chip-1");
    assert_eq!(
        label(&f, cx, "search-chip-0").as_deref(),
        Some("Remove from filter: bob")
    );
    assert_eq!(
        label(&f, cx, "search-chip-1").as_deref(),
        Some("Remove has filter: Image")
    );
    searched(&f, cx, "a search with both filters", |r| {
        param(r, "q") == ["cats"] && param(r, "has") == ["image"] && param(r, "from") == ["m-bob"]
    });
    // With filters on, only messages are listed.
    shows(&f, cx, "search-result-0");
    frame(&f, cx, |window, _| {
        assert!(window.try_find("search-item-0").is_none());
    });

    // Removing a chip searches again without it.
    click(&f, cx, "search-chip-1");
    searched(&f, cx, "a search without has", |r| {
        param(r, "has").is_empty() && param(r, "from") == ["m-bob"]
    });
    // Backspace in an empty field removes the last one.
    let session = f.session.clone();
    cx.update(|cx| search::open(&session, Some(""), cx));
    frame(&f, cx, |window, cx| window.press("backspace", cx));
    shows(&f, cx, "search-item-0");
    frame(&f, cx, |window, _| {
        assert!(window.try_find("search-chip-0").is_none());
    });
}

#[gpui_kit::test]
fn the_page_lists_conversations_and_people_by_name(cx: &mut TestAppContext) {
    let f = open(cx);
    open_page(&f, cx, None);
    // Nothing typed: where to jump to, what needs attention first.
    shows(&f, cx, "search-item-0");
    assert_eq!(
        label(&f, cx, "search-item-0").as_deref(),
        Some("#random, Home · Channel")
    );
    frame(&f, cx, |window, cx| window.input("gen", cx));
    shows(&f, cx, "search-item-0");
    assert_eq!(
        label(&f, cx, "search-item-0").as_deref(),
        Some("#general, Home · Channel")
    );
    // Enter opens the first one.
    frame(&f, cx, |window, cx| window.press("enter", cx));
    wait(cx, "#general open", |cx| {
        f.session.read(cx).route() == Some(Route::Conversation("c-general".into()))
    });
    // People match by nick.
    open_page(&f, cx, Some("eve"));
    shows(&f, cx, "search-item-0");
    assert_eq!(
        label(&f, cx, "search-item-0").as_deref(),
        Some("Message eve")
    );
}

#[gpui_kit::test]
fn the_filters_dialog_applies_several_at_once(cx: &mut TestAppContext) {
    let f = open(cx);
    open_page(&f, cx, None);
    click(&f, cx, "search-filters-button");
    shows(&f, cx, "search-filters-dialog");
    click(&f, cx, "search-filter-from-m-bob");
    click(&f, cx, "search-filter-has-link");
    click(&f, cx, "search-filter-in-c-random");
    click(&f, cx, "search-filter-author-bot");
    click(&f, cx, "search-filter-when-on");
    frame(&f, cx, |window, cx| {
        window.click("search-filter-on", cx);
        window.input("2026-11-02", cx);
    });
    // Nothing is searched before Apply.
    assert!(f.server.requests_to("GET", "/api/v1/search").is_empty());
    click(&f, cx, "search-filters-apply");
    searched(&f, cx, "the dialog's filters", |r| {
        param(r, "from") == ["m-bob"]
            && param(r, "has") == ["link"]
            && param(r, "conversationId") == ["c-random"]
            && param(r, "author") == ["bot"]
            && param(r, "after").len() == 1
            && param(r, "before").len() == 1
            && param(r, "q").is_empty()
    });
    assert_eq!(
        label(&f, cx, "search-filters-button").as_deref(),
        Some("Filters, 5 on")
    );
    // One conversation chosen: server history can be asked now.
    click(&f, cx, "search-server-history");
    searched(&f, cx, "server history", |r| {
        param(r, "source") == ["upstream"]
    });
}

#[gpui_kit::test]
fn searching_one_conversation_beside_it_with_server_history(cx: &mut TestAppContext) {
    let f = open(cx);
    click(&f, cx, "search-conversation-toggle");
    shows(&f, cx, "conversation-search");
    assert_eq!(
        label(&f, cx, "conversation-search").as_deref(),
        Some("Search #general")
    );
    frame(&f, cx, |window, cx| window.input("hello", cx));
    searched(&f, cx, "a search in #general", |r| {
        param(r, "q") == ["hello"] && param(r, "conversationId") == ["c-general"]
    });
    shows(&f, cx, "conversation-search-result-0");

    // Server history: a bounded set, with a hit IRCord does not retain.
    click(&f, cx, "conversation-search-server-history");
    searched(&f, cx, "server history in #general", |r| {
        param(r, "source") == ["upstream"] && param(r, "conversationId") == ["c-general"]
    });
    shows(&f, cx, "conversation-search-result-1");
    assert_eq!(
        label(&f, cx, "conversation-search-coverage").as_deref(),
        Some("Server results are bounded and may not include every match.")
    );
    assert_eq!(
        label(&f, cx, "conversation-search-count").as_deref(),
        Some("2 results in this server response")
    );
    let remote = label(&f, cx, "conversation-search-result-1").unwrap();
    assert!(remote.starts_with("Mira in #general"), "{remote}");
    assert!(
        remote.ends_with("this result cannot open in the timeline."),
        "{remote}"
    );

    // A result opens in the timeline and the panel stays.
    click(&f, cx, "conversation-search-result-0");
    wait(cx, "the old message in the timeline", |cx| {
        let session = f.session.read(cx);
        session.client().unwrap().read().focused("c-general") == Some("m-old")
    });
    frame(&f, cx, |window, _| {
        assert!(window.find("conversation-search").visible());
    });

    // "Search everywhere" takes the text to the search page.
    click(&f, cx, "conversation-search-everywhere");
    shows(&f, cx, "search-page");
    searched(&f, cx, "everywhere", |r| {
        param(r, "q") == ["hello"] && param(r, "conversationId").is_empty()
    });
}

#[gpui_kit::test]
fn escape_closes_the_conversation_search(cx: &mut TestAppContext) {
    let f = open(cx);
    click(&f, cx, "search-conversation-toggle");
    shows(&f, cx, "conversation-search");
    frame(&f, cx, |window, cx| window.press("escape", cx));
    wait(cx, "the panel closed", |cx| {
        f.session.read(cx).panel.is_none()
    });
    frame(&f, cx, |window, _| {
        assert!(window.try_find("conversation-search").is_none());
    });
}

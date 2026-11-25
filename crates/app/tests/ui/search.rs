//! Tests for package FD1 against the canned server: the search page finds a
//! message and opens it in the timeline (and remembers the search), typed
//! filters become chips and are sent, the filters dialog applies several at
//! once, and opening Search from a conversation applies its In filter.

use std::time::Duration;

use chrono::Datelike;
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

fn open_with_many_results(cx: &mut TestAppContext) -> Fixture {
    let f = open_with(cx, true, |server| {
        let items: Vec<_> = (0..24)
            .map(|i| {
                json!({
                    "conversation": {"id": "c-general", "networkId": "n1", "kind": "channel",
                                     "name": "general"},
                    "message": message(&format!("m-{i}"), "c-general", "hello from the archives"),
                })
            })
            .collect();
        server.route("GET", "/api/v1/search", 200, json!({"items": items}));
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
        Some("Showing messages stored on the IRCord server. The IRC network may have more.")
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
fn a_failed_search_reports_the_server_error_and_retries_the_same_query(cx: &mut TestAppContext) {
    let unavailable = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    let state = unavailable.clone();
    let f = open_with(cx, true, move |server| {
        server.route_fn("GET", "/api/v1/search", move |_| {
            if state.load(std::sync::atomic::Ordering::Relaxed) {
                Reply::problem(503, "Message search is temporarily unavailable.")
            } else {
                Reply::json(200, json!({ "items": [] }))
            }
        });
    });
    bootstrap(&f, cx);
    click(&f, cx, "search-conversation");
    shows(&f, cx, "search-failed");
    assert!(
        label(&f, cx, "search-failed")
            .is_some_and(|error| error.contains("Message search is temporarily unavailable."))
    );
    unavailable.store(false, std::sync::atomic::Ordering::Relaxed);
    click(&f, cx, "search-retry");
    shows(&f, cx, "search-count");
    let requests = f.server.requests_to("GET", "/api/v1/search");
    assert_eq!(requests.len(), 2);
    assert!(
        requests
            .iter()
            .all(|r| param(r, "conversationId") == ["c-general"])
    );
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
fn arrow_keys_and_tab_accept_a_workspace_search_suggestion(cx: &mut TestAppContext) {
    let f = open(cx);
    open_page(&f, cx, None);
    frame(&f, cx, |window, cx| window.input("cats has:", cx));
    shows(&f, cx, "search-offers");
    frame(&f, cx, |window, cx| {
        assert_eq!(window.find("search-offer-0").selected(), Some(true));
        window.press("down", cx);
        assert_eq!(window.find("search-offer-1").selected(), Some(true));
        window.press("tab", cx);
    });
    shows(&f, cx, "search-chip-0");
    searched(&f, cx, "the selected suggestion", |request| {
        param(request, "q") == ["cats"] && param(request, "has") == ["image"]
    });
    frame(&f, cx, |window, _| {
        assert_eq!(window.find("search-input").focused(), Some(true));
        assert!(window.try_find("search-offers").is_none());
    });
    frame(&f, cx, |window, cx| window.input("has:", cx));
    shows(&f, cx, "search-offers");
    assert_eq!(label(&f, cx, "search-offer-1").as_deref(), Some("GIF"));
}

#[gpui_kit::test]
fn filter_suggestions_float_over_results_and_clear_with_the_query(cx: &mut TestAppContext) {
    let f = open(cx);
    open_page(&f, cx, None);
    shows(&f, cx, "search-item-0");
    frame(&f, cx, |window, cx| {
        let result = window.find("search-item-0").bounds();
        window.input("from:", cx);
        assert_eq!(window.find("search-item-0").bounds(), result);
        assert_eq!(window.find("search-offer-0").selected(), Some(true));
    });
    click(&f, cx, "search-offer-0");
    shows(&f, cx, "search-chip-0");
    searched(&f, cx, "the self filter", |request| {
        param(request, "from") == ["u-me"]
    });
    click(&f, cx, "search-clear-query");
    shows(&f, cx, "search-item-0");
    frame(&f, cx, |window, _| {
        assert!(window.try_find("search-chip-0").is_none());
        assert!(window.try_find("search-offers").is_none());
        assert_eq!(window.find("search-input").focused(), Some(true));
    });
}

#[gpui_kit::test]
fn typed_date_suggestions_show_presets_and_accept_a_specific_day(cx: &mut TestAppContext) {
    let f = open(cx);
    open_page(&f, cx, None);
    frame(&f, cx, |window, cx| window.input("on:", cx));
    shows(&f, cx, "search-offers");
    let today = chrono::Local::now().date_naive().format("%Y-%m-%d");
    assert_eq!(
        label(&f, cx, "search-offer-0").as_deref(),
        Some(format!("Today {today}").as_str())
    );
    frame(&f, cx, |window, cx| {
        window.input("2020-02-20", cx);
        window.press("enter", cx);
    });
    shows(&f, cx, "search-chip-0");
    let wanted = ircord_client::search_filter::Query {
        filters: ircord_client::search_filter::Filters {
            on: Some("2020-02-20".into()),
            ..Default::default()
        },
        ..Default::default()
    }
    .pairs();
    searched(&f, cx, "the chosen day", |request| {
        wanted
            .iter()
            .filter(|(key, _)| matches!(*key, "after" | "before"))
            .all(|(key, value)| param(request, key) == [value.as_str()])
    });
}

#[gpui_kit::test]
fn the_query_calendar_adds_the_day_as_a_filter(cx: &mut TestAppContext) {
    let f = open(cx);
    open_page(&f, cx, None);
    click(&f, cx, "search-quick-on");
    click(&f, cx, "query-calendar");
    let day = chrono::Local::now().date_naive().with_day(1).unwrap();
    frame(&f, cx, |window, cx| {
        let id = gpui_kit::ElementId::Name(format!("calendar-{day}-0-0").into());
        window.click(id, cx);
    });
    shows(&f, cx, "search-chip-0");
}

#[gpui_kit::test]
fn workspace_search_scrolls_to_the_selected_result(cx: &mut TestAppContext) {
    let f = open_with_many_results(cx);
    open_page(&f, cx, None);
    frame(&f, cx, |window, cx| window.input("hello world", cx));
    shows(&f, cx, "search-result-0");
    frame(&f, cx, |window, cx| {
        assert!(
            !window
                .try_find("search-result-23")
                .is_some_and(|e| e.visible())
        );
        window.press("up", cx);
    });
    shows(&f, cx, "search-result-23");
    frame(&f, cx, |window, cx| {
        assert_eq!(window.find("search-result-23").selected(), Some(true));
        let input = window.find("search-input");
        assert_eq!(input.focused(), Some(true));
        assert!(input.visible());
        window.press("down", cx);
    });
    shows(&f, cx, "search-result-0");
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
    click(&f, cx, "search-filter-menu-from");
    frame(&f, cx, |window, cx| window.input("bob", cx));
    cx.executor().advance_clock(Duration::from_millis(200));
    cx.run_until_parked();
    shows(&f, cx, "search-filter-from-m-bob");
    frame(&f, cx, |window, _| {
        assert!(
            window
                .try_find("search-filter-from-m-eve")
                .is_none_or(|item| !item.visible())
        );
    });
    click(&f, cx, "search-filter-from-m-bob");
    frame(&f, cx, |window, cx| window.press("escape", cx));
    click(&f, cx, "search-filter-menu-has");
    click(&f, cx, "search-filter-has-link");
    click(&f, cx, "search-filter-has-image");
    frame(&f, cx, |window, cx| window.press("escape", cx));
    click(&f, cx, "search-filter-menu-in");
    click(&f, cx, "search-filter-in-c-random");
    frame(&f, cx, |window, cx| window.press("escape", cx));
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
        let mut kinds = param(r, "has");
        kinds.sort_unstable();
        param(r, "from") == ["m-bob"]
            && kinds == ["image", "link"]
            && param(r, "conversationId") == ["c-random"]
            && param(r, "author") == ["bot"]
            && param(r, "after").len() == 1
            && param(r, "before").len() == 1
            && param(r, "q").is_empty()
    });
    assert_eq!(
        label(&f, cx, "search-filters-button").as_deref(),
        Some("Filters, 6 on")
    );
    // One conversation chosen: server history can be asked now.
    click(&f, cx, "search-server-history");
    searched(&f, cx, "server history", |r| {
        param(r, "source") == ["upstream"]
    });
}

#[gpui_kit::test]
fn the_filters_dialog_rejects_future_dates_and_reversed_ranges(cx: &mut TestAppContext) {
    let f = open(cx);
    open_page(&f, cx, None);
    click(&f, cx, "search-filters-button");
    click(&f, cx, "search-filter-when-on");
    frame(&f, cx, |window, cx| {
        window.click("search-filter-on", cx);
        window.input("2999-01-01", cx);
    });
    click(&f, cx, "search-filters-apply");
    assert_eq!(
        label(&f, cx, "search-filters-error").as_deref(),
        Some("Choose a date on or before today.")
    );

    click(&f, cx, "search-filter-when-between");
    frame(&f, cx, |window, cx| {
        window.click("search-filter-after", cx);
        window.input("2020-03-01", cx);
        window.click("search-filter-before", cx);
        window.input("2020-02-01", cx);
    });
    click(&f, cx, "search-filters-apply");
    assert_eq!(
        label(&f, cx, "search-filters-error").as_deref(),
        Some("The start date must be on or before the end date.")
    );
    assert!(f.server.requests_to("GET", "/api/v1/search").is_empty());
}

#[gpui_kit::test]
fn calendar_respects_range_bounds_and_keeps_typed_dates(cx: &mut TestAppContext) {
    let f = open(cx);
    open_page(&f, cx, None);
    click(&f, cx, "search-filters-button");
    click(&f, cx, "search-filter-when-between");
    frame(&f, cx, |window, cx| {
        window.click("search-filter-after", cx);
        window.input("2020-02-10", cx);
        window.click("search-filter-before", cx);
        window.input("2020-02-20", cx);
    });
    click(&f, cx, "search-calendar-before");
    shows(&f, cx, "calendar-2020-02-10-0-2");
    frame(&f, cx, |window, _| {
        assert_eq!(
            window.find("calendar-2020-02-09-0-2").label(),
            Some("2020-02-09")
        );
    });
    click(&f, cx, "calendar-2020-02-09-0-2");
    shows(&f, cx, "calendar-2020-02-10-0-2");
    click(&f, cx, "calendar-2020-02-15-0-2");
    click(&f, cx, "search-calendar-after");
    shows(&f, cx, "calendar-2020-02-15-0-2");
    click(&f, cx, "calendar-2020-02-16-0-3");
    shows(&f, cx, "calendar-2020-02-15-0-2");
    click(&f, cx, "search-calendar-after");
    click(&f, cx, "search-filters-apply");
    assert_eq!(
        label(&f, cx, "search-filters-button").as_deref(),
        Some("Filters, 2 on")
    );
    searched(&f, cx, "calendar date range", |r| {
        param(r, "after").len() == 1 && param(r, "before").len() == 1
    });
}

#[gpui_kit::test]
fn conversation_search_fills_in_the_search_page_and_uses_server_history(cx: &mut TestAppContext) {
    let f = open(cx);
    click(&f, cx, "search-conversation");
    shows(&f, cx, "search-page");
    assert_eq!(
        label(&f, cx, "search-chip-0").as_deref(),
        Some("Remove in filter: #general")
    );
    frame(&f, cx, |window, _| {
        assert_eq!(window.find("search-input").focused(), Some(true));
        assert!(window.try_find("conversation-search").is_none());
    });
    searched(&f, cx, "recent messages in #general", |r| {
        param(r, "q").is_empty()
            && param(r, "conversationId") == ["c-general"]
            && param(r, "source") == ["retained"]
    });
    frame(&f, cx, |window, cx| window.input("hello", cx));
    searched(&f, cx, "a search in #general", |r| {
        param(r, "q") == ["hello"] && param(r, "conversationId") == ["c-general"]
    });
    shows(&f, cx, "search-result-0");
    click(&f, cx, "search-server-history");
    searched(&f, cx, "server history in #general", |r| {
        param(r, "source") == ["upstream"] && param(r, "conversationId") == ["c-general"]
    });
    shows(&f, cx, "search-result-1");
    assert_eq!(
        label(&f, cx, "search-coverage").as_deref(),
        Some("Server results are bounded and may not include every match.")
    );
    assert_eq!(
        label(&f, cx, "search-count").as_deref(),
        Some("2 results in this server response")
    );
    let remote = label(&f, cx, "search-result-1").unwrap();
    assert!(remote.starts_with("Mira in #general"), "{remote}");
    assert!(
        remote.ends_with("this result cannot open in the timeline."),
        "{remote}"
    );
    click(&f, cx, "search-result-0");
    wait(cx, "the old message in the timeline", |cx| {
        let session = f.session.read(cx);
        session.route() == Some(Route::Conversation("c-general".into()))
            && session.client().unwrap().read().focused("c-general") == Some("m-old")
    });
}

#[gpui_kit::test]
fn the_conversation_search_shortcut_uses_the_same_page_in_a_narrow_window(cx: &mut TestAppContext) {
    let f = open(cx);
    cx.simulate_window_resize(
        f.window,
        gpui_kit::size(gpui_kit::px(1000.), gpui_kit::px(720.)),
    );
    frame(&f, cx, |window, cx| {
        let key = if cfg!(target_os = "macos") {
            "cmd-alt-f"
        } else {
            "ctrl-alt-f"
        };
        window.press(key, cx);
    });
    shows(&f, cx, "search-page");
    shows(&f, cx, "search-chip-0");
    frame(&f, cx, |window, cx| window.press("escape", cx));
    shows(&f, cx, "timeline");
}

#[gpui_kit::test]
fn results_carry_the_senders_picture_and_the_page_closes(cx: &mut TestAppContext) {
    let f = open_with(cx, true, |server| {
        let mut s = crate::fixture::snapshot();
        for m in &mut s.members {
            if m.id == "m-bob" {
                m.avatar_url = Some("/files/bob/avatar.png".into());
            }
        }
        server.snapshot(&s);
        serve_search(server);
    });
    bootstrap(&f, cx);
    open_page(&f, cx, None);
    assert_eq!(
        label(&f, cx, "search-page-title").as_deref(),
        Some("Search")
    );
    frame(&f, cx, |window, cx| window.input("hello", cx));
    shows(&f, cx, "search-result-0");
    shows(&f, cx, "avatar-picture-search-result-0");
    shows(&f, cx, "author-search-result-0");
    click(&f, cx, "search-page-close");
    wait(cx, "back in #general", |cx| {
        f.session.read(cx).route() == Some(Route::Conversation("c-general".into()))
    });
}

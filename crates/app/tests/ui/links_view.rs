//! Tests for package FD3: the links view and the channel gallery, against
//! the canned server.

use gpui_kit::test::TestWindowExt;
use gpui_kit::{AppContext, TestAppContext};
use ircord_app::features::{gallery, links};
use ircord_app::route::Route;
use ircord_client::testing::{CannedServer, Recorded, Reply};
use serde_json::{Value, json};

use crate::fixture::{Fixture, bootstrap, open_with, wait};

fn source(message: &str, conversation: &str, url: &str, nsfw_known: bool) -> Value {
    json!({
        "conversationId": conversation, "networkId": "n1", "messageId": message,
        "senderId": "m-bob", "postedAt": "2026-11-21T10:00:00Z",
        "excerpt": format!("look at this: {url}"), "url": url,
        "nsfw": false, "nsfwKnown": nsfw_known
    })
}

fn card(identity: &str, kind: &str, url: &str, sources: Vec<Value>, count: i64) -> Value {
    json!({
        "identity": identity, "kind": kind, "host": "example.com", "url": url,
        "lastPostedAt": "2026-11-21T10:00:00Z", "count": count, "sources": sources
    })
}

/// The catalog: one page and one picture in `#general`, a second page of
/// one older link behind a cursor; images only when asked for.
fn serve(server: &CannedServer) {
    server.route_fn("GET", "/api/v1/links", |r| {
        let kinds = param(r, "kind");
        let pictures = vec![card(
            "pic",
            "image",
            "https://example.com/cat.png",
            vec![source(
                "2",
                "c-general",
                "https://example.com/cat.png",
                false,
            )],
            1,
        )];
        let page = if !param(r, "cursor").is_empty() {
            json!({"items": [card("old", "link", "https://example.com/old",
                vec![source("2", "c-general", "https://example.com/old", true)], 1)],
                "indexing": false})
        } else if kinds == ["image", "gif"] || kinds == ["image"] {
            json!({"items": pictures, "indexing": false})
        } else if param(r, "q") == ["nothing"] {
            json!({"items": [], "indexing": false})
        } else {
            json!({
                "items": [
                    card("home", "link", "https://ircord.com",
                        vec![source("1", "c-general", "https://ircord.com", true),
                             source("2", "c-general", "https://ircord.com", true)], 3),
                    pictures[0].clone(),
                ],
                "nextCursor": "page-2", "indexing": true
            })
        };
        Reply::json(200, page)
    });
    server.route_fn(
        "POST",
        "/api/v1/conversations/*/messages/*/link-media",
        |r| {
            let url = r.json()["url"].as_str().unwrap_or("").to_string();
            let answer = if url.ends_with(".png") {
                json!({"kind": "media", "provider": "media", "url": url,
                       "assets": [{"kind": "image", "url": "https://media.invalid/cat.png",
                                   "width": 640, "height": 480, "contentType": "image/png"}]})
            } else {
                json!({"kind": "card", "provider": "page", "url": url, "assets": [],
                       "card": {"site": "IRCord", "title": "IRCord, the IRC client"}})
            };
            Reply::json(200, answer)
        },
    );
}

fn open(cx: &mut TestAppContext) -> Fixture {
    let f = open_with(cx, true, serve);
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

/// Wait until `id`'s label starts with `want`.
fn starts(f: &Fixture, cx: &mut TestAppContext, id: &'static str, want: &str) {
    let window = f.window;
    let want = want.to_string();
    wait(cx, &format!("{id} starting {want:?}"), move |cx| {
        window
            .update(cx, |_, window, cx| {
                window.render_frame(cx);
                window
                    .try_find(id)
                    .and_then(|e| e.label().map(|l| l.starts_with(want.as_str())))
                    .unwrap_or(false)
            })
            .unwrap_or(false)
    });
}

fn gone(f: &Fixture, cx: &mut TestAppContext, id: &'static str) -> bool {
    let mut gone = false;
    frame(f, cx, |window, _| gone = window.try_find(id).is_none());
    gone
}

/// Every value of `key` in the request's query, decoded.
fn param(r: &Recorded, key: &str) -> Vec<String> {
    url::form_urlencoded::parse(r.query.as_deref().unwrap_or_default().as_bytes())
        .filter(|(k, _)| k == key)
        .map(|(_, v)| v.into_owned())
        .collect()
}

/// Wait for a catalog request that `matches`.
fn listed(f: &Fixture, cx: &mut TestAppContext, what: &str, matches: impl Fn(&Recorded) -> bool) {
    wait(cx, what, |_| {
        f.server
            .requests_to("GET", "/api/v1/links")
            .iter()
            .any(&matches)
    });
}

fn route(f: &Fixture, cx: &mut TestAppContext) -> Option<Route> {
    cx.update(|cx| f.session.read(cx).route())
}

#[gpui_kit::test]
fn the_links_view_lists_cards_with_previews_and_jumps_to_a_post(cx: &mut TestAppContext) {
    let f = open(cx);
    click(&f, cx, "sidebar-links");
    assert_eq!(route(&f, cx), Some(Route::Links(None)));
    listed(&f, cx, "the whole catalog", |r| {
        param(r, "conversation").is_empty() && param(r, "limit") == ["30"]
    });
    // The page card takes its title from the server's preview.
    reads(&f, cx, "links-card-0", "IRCord, the IRC client");
    reads(&f, cx, "links-card-0-posts", "Posted 3 times");
    // Without a picture the page draws its artwork, which carries the
    // title, so the body leaves the title out.
    reads(&f, cx, "links-card-0-art", "IRCord, the IRC client");
    assert!(gone(&f, cx, "links-card-0-title"));
    reads(
        &f,
        cx,
        "links-card-0-source",
        "Jump to the message: bob in #general",
    );
    reads(&f, cx, "links-meta", "2+ links");
    shows(&f, cx, "links-indexing");
    // The picture's channel settings are unknown, so it is covered.
    shows(&f, cx, "links-card-1-cover");
    click(&f, cx, "links-card-1-show");
    assert!(gone(&f, cx, "links-card-1-cover"));
    assert!(
        gone(&f, cx, "links-card-1-art"),
        "a picture needs no artwork"
    );

    // Older links load behind the cursor.
    click(&f, cx, "links-load-more");
    listed(&f, cx, "the second page", |r| {
        param(r, "cursor") == ["page-2"]
    });
    shows(&f, cx, "links-card-2");

    // "Posted 3 times" lists the posts; one jumps to its message.
    click(&f, cx, "links-card-0-posts");
    shows(&f, cx, "links-post-1");
    click(&f, cx, "links-post-1");
    wait(cx, "the conversation", |cx| {
        f.session.read(cx).route() == Some(Route::Conversation("c-general".into()))
    });
}

#[gpui_kit::test]
fn typed_filters_and_type_pills_narrow_the_list(cx: &mut TestAppContext) {
    let f = open(cx);
    let session = f.session.clone();
    cx.update(|cx| links::open(&session, None, cx));
    reads(&f, cx, "links-card-0", "IRCord, the IRC client");

    // A pill asks for one kind.
    click(&f, cx, "links-kind-image");
    listed(&f, cx, "images only", |r| param(r, "kind") == ["image"]);
    reads(&f, cx, "links-chip-0", "Remove the filter Type: Images");
    click(&f, cx, "links-chip-0");
    assert!(gone(&f, cx, "links-chip-0"));

    // Finished tokens become chips.
    click(&f, cx, "links-filter");
    frame(&f, cx, |window, cx| {
        window.input("in:#general host:Example.COM ", cx)
    });
    listed(&f, cx, "the typed filters", |r| {
        param(r, "conversation") == ["c-general"] && param(r, "host") == ["example.com"]
    });
    reads(
        &f,
        cx,
        "links-chip-0",
        "Remove the filter Channel: #general",
    );
    reads(
        &f,
        cx,
        "links-chip-1",
        "Remove the filter Site: Example.COM",
    );

    // A site that is not one is explained, and not asked for; Backspace in
    // the empty box removes it again.
    click(&f, cx, "links-chip-1");
    click(&f, cx, "links-filter");
    frame(&f, cx, |window, cx| window.input("host:exa_mple..com ", cx));
    reads(&f, cx, "links-problem", "Enter a host such as example.com.");
    assert!(
        f.server
            .requests_to("GET", "/api/v1/links")
            .iter()
            .all(|r| !param(r, "host").iter().any(|h| h.contains("exa_mple")))
    );
    frame(&f, cx, |window, cx| window.press("backspace", cx));
    assert!(gone(&f, cx, "links-problem"));

    // The text is searched for; nothing matching offers to clear the filters.
    frame(&f, cx, |window, cx| window.input("nothing", cx));
    listed(&f, cx, "the typed text", |r| param(r, "q") == ["nothing"]);
    reads(&f, cx, "links-empty", "No links match these filters.");
    click(&f, cx, "links-clear");
    listed(&f, cx, "the list without text", |r| {
        param(r, "q").is_empty() && param(r, "conversation") == ["c-general"]
    });
}

#[gpui_kit::test]
fn a_channel_opens_its_own_links_and_its_gallery(cx: &mut TestAppContext) {
    let f = open(cx);
    let session = f.session.clone();
    cx.update(|cx| links::open(&session, Some("c-general"), cx));
    assert_eq!(route(&f, cx), Some(Route::Links(Some("c-general".into()))));
    listed(&f, cx, "#general's links", |r| {
        param(r, "conversation") == ["c-general"]
    });
    reads(
        &f,
        cx,
        "links-chip-0",
        "Remove the filter Channel: #general",
    );

    // The gallery beside the channel lists its pictures and opens the viewer.
    cx.update(|cx| session.update(cx, |s, cx| s.select_conversation("c-general", cx)));
    click(&f, cx, "gallery-toggle");
    listed(&f, cx, "#general's pictures", |r| {
        param(r, "conversation") == ["c-general"] && param(r, "kind") == ["image", "gif"]
    });
    reads(&f, cx, "gallery-meta", "1 picture");
    shows(&f, cx, "gallery-item-0-cover");
    click(&f, cx, "gallery-item-0-show");
    starts(
        &f,
        cx,
        "gallery-item-0",
        "Open picture 1 of 1, posted by bob ",
    );
    click(&f, cx, "gallery-item-0");
    shows(&f, cx, "viewer-position");

    // Closing the gallery leaves the channel.
    cx.update(|cx| gallery::toggle(&session, false, cx));
    assert!(gone(&f, cx, "gallery-panel"));
}

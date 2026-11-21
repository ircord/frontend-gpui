//! Tests for package E1: the emoji and GIF pickers, emoji autocomplete,
//! reacting with any emoji and custom emoji drawn inline in messages.

use std::time::Duration;

use gpui_kit::test::{TestQueryExt, TestWindowExt};
use gpui_kit::{AppContext, TestAppContext};
use ircord_app::composer::expressions::Tab;
use ircord_app::features::{klipy, message_actions};
use ircord_client::contract::*;
use ircord_client::testing::{CannedServer, Reply};
use serde_json::{Value, json};

use crate::fixture::{Fixture, message, open_with, snapshot, wait};

const WAIT: Duration = Duration::from_secs(5);

/// The fixture's snapshot on a network that offers reactions.
fn capable() -> Snapshot {
    let mut s = snapshot();
    let n = &mut s.networks[0];
    n.account = Some("me".into());
    n.features.reactions = true;
    n.features.replies = true;
    s
}

fn library(usage: Value, gifs: Value) -> Value {
    json!({"gifs": gifs, "emojiUsage": usage, "skinTone": "0", "updatedAt": null})
}

fn gif(id: &str) -> Value {
    json!({
        "id": id, "slug": format!("cat-{id}"), "title": format!("Cat {id}"),
        "file": {
            "hd": {"gif": {"url": format!("https://static.klipy.com/{id}/hd.gif"), "width": 400, "height": 300}},
            "sm": {"webp": {"url": format!("https://static.klipy.com/{id}/sm.webp"), "width": 200, "height": 150},
                   "jpg": {"url": format!("https://static.klipy.com/{id}/sm.jpg"), "width": 200, "height": 150}}
        }
    })
}

/// bob's message with custom emoji, with a network ID so it takes reactions.
fn first() -> Message {
    let mut m = message("1", "c-general", "hi :party: and :pog: but not :nope:");
    m.upstream_id = Some("up-1".into());
    m
}

/// Everything E1 reads: the server's emote `party`, a 7TV `pog`, an empty
/// library, Klipy on the canned server, and messages from bob, the first
/// with custom emoji in it.
fn serve(server: &CannedServer) {
    server.snapshot(&capable());
    let history = serde_json::to_value(MessagePage {
        items: vec![first(), message("2", "c-general", "second line")],
        ..Default::default()
    })
    .unwrap();
    server.route_fn("GET", "/api/v1/conversations/*/messages", move |r| {
        if r.path.ends_with("/c-general/messages") {
            Reply::json(200, history.clone())
        } else {
            Reply::json(200, json!({"items": []}))
        }
    });
    server.route(
        "GET",
        "/api/v1/emotes",
        200,
        json!({"version": 3, "canManage": false, "canAdd": false, "maxBytes": 0,
               "types": [], "providers": [],
               "items": [{"id": "e1", "name": "Party", "shortcode": "party", "aliases": [],
                          "retired": [], "enabled": true, "animated": false,
                          "url": "/api/v1/emotes/e1/image", "staticUrl": "/api/v1/emotes/e1/image",
                          "width": 64, "height": 64, "source": "upload", "canManage": false,
                          "createdAt": "2026-11-01T00:00:00Z", "updatedAt": "2026-11-01T00:00:00Z"}]}),
    );
    server.route(
        "GET",
        "/api/v1/app/emoji/catalog",
        200,
        json!({"standardUrl": "/data/emoji-v18.json", "sources": ["workspace", "7tv"],
               "items": [{"id": "7tv:pog", "label": "Pog", "shortcode": "pog", "aliases": [],
                          "group": "7tv", "source": "7tv",
                          "url": "https://cdn.7tv.app/emote/pog/1x.webp",
                          "staticUrl": "https://cdn.7tv.app/emote/pog/1x.png"}]}),
    );
    server.route(
        "GET",
        "/api/v1/app/library",
        200,
        library(json!([]), json!([])),
    );
    server.route_fn("PATCH", "/api/v1/app/library", |r| {
        let uses = r.json()["uses"].clone();
        let id = uses
            .as_object()
            .and_then(|o| o.keys().next().cloned())
            .unwrap_or_default();
        Reply::json(
            200,
            library(
                json!([{"id": id, "count": 1, "lastUsed": "2026-11-21T10:00:00Z"}]),
                json!([]),
            ),
        )
    });
    server.route_fn("POST", "/api/v1/conversations/*/messages", |r| {
        let body = r.json();
        Reply::json(
            202,
            json!({"clientId": body["clientId"], "conversationId": "c-general", "state": "queued"}),
        )
    });
    server.route(
        "GET",
        "/klipy/test-key/gifs/categories",
        200,
        json!({"result": true, "data": {"categories": [
            {"category": "Cats", "query": "cats", "preview_url": "https://static.klipy.com/c.gif"}]}}),
    );
    server.route(
        "GET",
        "/klipy/test-key/gifs/trending",
        200,
        json!({"result": true, "data": {"data": [gif("g1"), gif("g2")], "current_page": 1, "has_next": false}}),
    );
    server.route(
        "GET",
        "/klipy/test-key/gifs/search",
        200,
        json!({"result": true, "data": {"data": [gif("s1")], "current_page": 1, "has_next": false}}),
    );
    server.route(
        "POST",
        "/klipy/test-key/gifs/share/*",
        200,
        json!({"result": true}),
    );
}

fn open(cx: &mut TestAppContext, setup: impl FnOnce(&CannedServer)) -> Fixture {
    let f = open_with(cx, true, |server| {
        serve(server);
        setup(server);
    });
    let base = format!("{}/klipy", f.server.address().origin());
    cx.update(|cx| klipy::configure(cx, base, "test-key"));
    wait(
        cx,
        "the history, the live socket and the emoji catalogs",
        |cx| {
            let session = f.session.read(cx);
            let Some(client) = session.client() else {
                return false;
            };
            let w = client.read();
            session.conversation.as_deref() == Some("c-general")
                && w.connection == Some(ircord_client::store::Connection::Live)
                && w.emoji.is_loaded()
                && w.documents.library.is_loaded()
                && w.timelines
                    .get("c-general")
                    .is_some_and(|t| t.entries.len() == 2)
        },
    );
    draw(&f, cx);
    f
}

/// Two frames: pictures and pills are placed from the frame before.
fn draw(f: &Fixture, cx: &mut TestAppContext) {
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.render_frame(cx);
    })
    .unwrap();
    cx.run_until_parked();
}

fn click(f: &Fixture, cx: &mut TestAppContext, id: &'static str) {
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.click(id, cx);
    })
    .unwrap();
    cx.run_until_parked();
}

#[gpui_kit::test]
fn custom_emoji_draw_as_pictures_in_messages(cx: &mut TestAppContext) {
    let f = open(cx, |_| {});
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        let party = window.find_by_label(":party:");
        assert!(party.visible(), "the server's emote");
        assert!(window.find_by_label(":pog:").visible(), "a 7TV global");
        assert!(
            window.try_find_by_label(":nope:").is_none(),
            "an unknown name stays text"
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn picking_an_emoji_writes_it_into_the_box(cx: &mut TestAppContext) {
    let f = open(cx, |_| {});
    click(&f, cx, "composer-emoji");
    cx.update(|cx| assert_eq!(klipy::open_tab(cx), Some(Tab::Emoji)));
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.find_by_label("Popular").visible(), "no history yet");
        window.input("grinning", cx);
    })
    .unwrap();
    cx.run_until_parked();
    click(&f, cx, "emoji-1F600");
    cx.update(|cx| {
        assert_eq!(message_actions::box_text(cx), "😀");
        assert_eq!(klipy::open_tab(cx), None, "picking closes the picker");
    });
    let body = f
        .server
        .wait_for("PATCH", "/api/v1/app/library", WAIT)
        .json();
    assert_eq!(body["uses"], json!({"1F600": 1}));
    // The pick now leads "Frequently used".
    click(&f, cx, "composer-emoji");
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        // The group's tab and the grid under its title.
        assert_eq!(window.find_all_by_label("Frequently used").len(), 2);
        assert!(window.try_find("emoji-1F600").is_some());
    })
    .unwrap();
}

#[gpui_kit::test]
fn the_servers_emoji_have_their_own_group_and_go_in_as_names(cx: &mut TestAppContext) {
    let f = open(cx, |_| {});
    click(&f, cx, "composer-emoji");
    click(&f, cx, "emoji-group-workspace");
    click(&f, cx, "emoji-instance:e1");
    cx.update(|cx| assert_eq!(message_actions::box_text(cx), ":party:"));
}

#[gpui_kit::test]
fn a_colon_word_suggests_emoji(cx: &mut TestAppContext) {
    let f = open(cx, |_| {});
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.click("composer-input", cx);
        window.input("so :grinn", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("suggestion-emoji-1F600").is_some());
        window.press("tab", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update(|cx| assert_eq!(message_actions::box_text(cx), "so 😀"));
    // Custom emoji complete to their name and a space.
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.input(" :par", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.press("tab", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update(|cx| assert_eq!(message_actions::box_text(cx), "so 😀 :party: "));
}

#[gpui_kit::test]
fn react_with_any_emoji_from_the_full_picker(cx: &mut TestAppContext) {
    let f = open(cx, |server| {
        server.route_fn("POST", "/api/v1/conversations/*/reactions", |r| {
            let body = r.json();
            let mut m = first();
            m.reactions = vec![Reaction {
                emoji: body["emoji"].as_str().unwrap().into(),
                member_ids: vec!["u-me".into()],
            }];
            Reply::json(200, serde_json::to_value(m).unwrap())
        });
    });
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.hover("message-1", cx);
        window.click("message-toolbar-react", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.find_by_label("Quick reactions").visible());
        window.input("tears of joy", cx);
    })
    .unwrap();
    cx.run_until_parked();
    click(&f, cx, "emoji-1F602");
    let body = f
        .server
        .wait_for("POST", "/api/v1/conversations/c-general/reactions", WAIT)
        .json();
    assert_eq!(body["emoji"], "😂");
    wait(cx, "the reaction on the message", |cx| {
        let session = f.session.read(cx);
        let w = session.client().unwrap().read();
        w.message("c-general", "1")
            .is_some_and(|m| m.reactions.iter().any(|r| r.emoji == "😂"))
    });
}

#[gpui_kit::test]
fn a_trending_gif_is_saved_and_sent(cx: &mut TestAppContext) {
    let f = open(cx, |server| {
        server.route_fn("POST", "/api/v1/app/library/gifs", |r| {
            let mut saved = r.json();
            saved["savedAt"] = json!("2026-11-21T10:00:00Z");
            Reply::json(200, library(json!([]), json!([saved])))
        });
    });
    click(&f, cx, "composer-gif");
    cx.update(|cx| assert_eq!(klipy::open_tab(cx), Some(Tab::Gifs)));
    wait(cx, "Klipy's categories", |_| {
        f.server
            .requests_to("GET", "/klipy/test-key/gifs/categories")
            .len()
            == 1
    });
    draw(&f, cx);
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.find_by_label("Cats").visible(), "a category");
        assert!(window.find_by_label("Saved GIFs (0)").visible());
    })
    .unwrap();
    click(&f, cx, "gif-trending");
    wait(cx, "the trending GIFs", |cx| {
        cx.update_window(f.window, |_, window, cx| {
            window.render_frame(cx);
            window.try_find("gif-g1").is_some()
        })
        .unwrap_or(false)
    });
    click(&f, cx, "gif-save-g1");
    let saved = f
        .server
        .wait_for("POST", "/api/v1/app/library/gifs", WAIT)
        .json();
    assert_eq!(saved["id"], "g1");
    assert_eq!(saved["url"], "https://static.klipy.com/g1/hd.gif");
    wait(cx, "the saved GIF in the library", |cx| {
        let session = f.session.read(cx);
        let w = session.client().unwrap().read();
        w.documents
            .library
            .value()
            .is_some_and(|l| l.gifs.iter().any(|g| g.id == "g1"))
    });
    click(&f, cx, "gif-g2");
    let sent = f
        .server
        .wait_for("POST", "/api/v1/conversations/c-general/messages", WAIT)
        .json();
    assert_eq!(sent["text"], "https://static.klipy.com/g2/hd.gif");
    assert_eq!(sent["gif"]["id"], "g2");
    assert_eq!(sent["gif"]["provider"], "klipy");
    cx.update(|cx| assert_eq!(klipy::open_tab(cx), None));
    f.server
        .wait_for("POST", "/klipy/test-key/gifs/share/cat-g2", WAIT);
}

#[gpui_kit::test]
fn slash_gif_sends_a_matching_gif(cx: &mut TestAppContext) {
    let f = open(cx, |_| {});
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.click("composer-input", cx);
        window.input("/gif cats", cx);
        window.press("enter", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let search = f
        .server
        .wait_for("GET", "/klipy/test-key/gifs/search", WAIT);
    assert_eq!(search.query_param("q").as_deref(), Some("cats"));
    let sent = f
        .server
        .wait_for("POST", "/api/v1/conversations/c-general/messages", WAIT)
        .json();
    assert_eq!(sent["gif"]["id"], "s1");
    cx.update(|cx| assert_eq!(message_actions::box_text(cx), ""));
}

#[gpui_kit::test]
fn the_gif_key_opens_the_gif_picker(cx: &mut TestAppContext) {
    let f = open(cx, |_| {});
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.click("composer-input", cx);
        window.press("ctrl-g", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update(|cx| assert_eq!(klipy::open_tab(cx), Some(Tab::Gifs)));
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.press("escape", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update(|cx| assert_eq!(klipy::open_tab(cx), None));
}

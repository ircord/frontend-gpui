//! Tests for package E1: the emoji and GIF pickers, emoji autocomplete,
//! reacting with any emoji and custom emoji drawn inline in messages.

use std::sync::{Mutex, mpsc};
use std::time::Duration;

use gpui_kit::test::{TestEventExt, TestQueryExt, TestWindowExt};
use gpui_kit::{AppContext, ClipboardItem, Focusable, MouseButton, TestAppContext, point, px};
use ircord_app::Screen;
use ircord_app::composer::expressions::Tab;
use ircord_app::features::{klipy, message_actions};
use ircord_app::message::emoji;
use ircord_app::registry::MessageCtx;
use ircord_client::contract::*;
use ircord_client::testing::{CannedServer, Reply};
use serde_json::{Value, json};

use crate::fixture::{Fixture, message, open_with, snapshot, wait};

const WAIT: Duration = crate::fixture::HANG_GUARD;

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
pub(crate) fn serve(server: &CannedServer) {
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
    draw(&f, cx);
    assert_eq!(
        tokens(&f, cx),
        1,
        "the pick draws as its picture in the box"
    );
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
    wait(cx, "the sent command draft to clear", |cx| {
        message_actions::box_text(cx).is_empty()
    });
}

const GIF_SEARCH: &str = "/klipy/test-key/gifs/search";
const SEND_MESSAGE: &str = "/api/v1/conversations/c-general/messages";

fn delayed_search(cx: &mut TestAppContext, items: Value) -> (Fixture, mpsc::Sender<()>) {
    let (release, received) = mpsc::channel();
    let received = Mutex::new(received);
    let f = open(cx, |server| {
        server.route_fn("GET", GIF_SEARCH, move |_| {
            received.lock().unwrap().recv_timeout(WAIT).unwrap();
            Reply::json(
                200,
                json!({"result": true, "data": {
                    "data": items, "current_page": 1, "has_next": false
                }}),
            )
        });
    });
    (f, release)
}

fn search_pending(f: &Fixture, cx: &mut gpui_kit::App) -> bool {
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            window.try_find("composer-gif-lookup").is_some()
        })
        .unwrap()
}

#[gpui_kit::test]
fn slash_gif_without_matches_keeps_the_draft(cx: &mut TestAppContext) {
    let (f, release) = delayed_search(cx, json!([]));
    type_in_box(&f, cx, "/gif cats");
    press(&f, cx, &["enter"]);
    f.server.wait_for("GET", GIF_SEARCH, WAIT);
    assert!(cx.update(|cx| search_pending(&f, cx)));
    release.send(()).unwrap();
    wait(cx, "the GIF search to finish", |cx| !search_pending(&f, cx));
    assert_eq!(box_text(cx), "/gif cats");
    assert!(f.server.requests_to("POST", SEND_MESSAGE).is_empty());
}

#[gpui_kit::test]
fn slash_gif_sends_the_original_reply_and_preserves_newer_changes(cx: &mut TestAppContext) {
    let (f, release) = delayed_search(cx, json!([gif("s1")]));
    let reply = |message: &str, cx: &mut TestAppContext| {
        cx.update_window(f.window, |_, window, cx| {
            message_actions::start_reply(
                &MessageCtx {
                    session: f.session.clone(),
                    conversation: "c-general".into(),
                    message: message.into(),
                    own: false,
                },
                window,
                cx,
            );
        })
        .unwrap();
    };
    reply("1", cx);
    type_on(&f, cx, "/gif cats");
    press(&f, cx, &["enter"]);
    f.server.wait_for("GET", GIF_SEARCH, WAIT);
    type_on(&f, cx, " later");
    reply("2", cx);
    release.send(()).unwrap();
    wait(cx, "the GIF reply", |_| {
        !f.server.requests_to("POST", SEND_MESSAGE).is_empty()
    });
    let sent = f.server.requests_to("POST", SEND_MESSAGE)[0].json();
    assert_eq!(sent["replyToId"], "1");
    assert_eq!(box_text(cx), "/gif cats later");
    cx.update(|cx| {
        assert!(matches!(
            message_actions::mode("c-general", cx),
            Some(message_actions::Mode::Reply { message, .. }) if message == "2"
        ));
    });
}

#[gpui_kit::test]
fn the_gif_key_opens_the_gif_picker(cx: &mut TestAppContext) {
    let f = open(cx, |_| {});
    type_in_box(&f, cx, "one two three");
    let input = cx.update(|cx| match f.view.read(cx).screen() {
        Screen::Workspace(workspace) => workspace.read(cx).composer.read(cx).input.clone(),
        _ => panic!("the workspace is not showing"),
    });
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        input.update(cx, |state, cx| {
            state.set_selected_range_with_direction(4..7, true, cx)
        });
        window.press(
            if cfg!(target_os = "macos") {
                "cmd-g"
            } else {
                "ctrl-g"
            },
            cx,
        );
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
    cx.update_window(f.window, |_, window, cx| {
        let state = input.read(cx);
        assert!(state.focus_handle(cx).is_focused(window));
        assert_eq!(state.selected_range(), 4..7);
        assert!(state.is_selection_reversed());
    })
    .unwrap();
}

#[gpui_kit::test]
fn the_expression_popover_leaves_the_timeline_and_composer_in_place(cx: &mut TestAppContext) {
    let f = open(cx, |_| {});
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        let timeline = window.find("timeline").bounds();
        let composer = window.find("composer-field").bounds();
        window.click("composer-emoji", cx);
        window.render_frame(cx);
        let picker = window.find("expression-picker").bounds();
        assert!(picker.bottom() <= composer.top());
        assert_eq!(window.find("timeline").bounds(), timeline);
        assert_eq!(window.find("composer-field").bounds(), composer);
        assert_eq!(window.find("expression-search").focused(), Some(true));
        window.press("escape", cx);
        assert!(window.try_find("expression-picker").is_none());
        assert_eq!(window.find("composer-emoji").focused(), Some(true));
    })
    .unwrap();
}

#[gpui_kit::test]
fn the_expression_popover_resizes_and_dismisses_on_an_outside_click(cx: &mut TestAppContext) {
    let f = open(cx, |_| {});
    let width = cx
        .update_window(f.window, |_, window, cx| {
            window.render_frame(cx);
            window.click("composer-emoji", cx);
            window.render_frame(cx);
            let initial = window.find("expression-picker").bounds();
            window.click("expression-resize", cx);
            window.press("left", cx);
            let resized = window.find("expression-picker").bounds();
            assert_eq!(resized.size.width, initial.size.width + px(16.));
            assert_eq!(resized.right(), initial.right());
            let outside = window.find("timeline").bounds().origin + point(px(10.), px(10.));
            window.pointer_down(outside, MouseButton::Left, cx);
            window.pointer_up(outside, MouseButton::Left, cx);
            window.render_frame(cx);
            assert!(window.try_find("expression-picker").is_none());
            resized.size.width
        })
        .unwrap();
    click(&f, cx, "composer-emoji");
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("expression-picker").bounds().size.width, width);
    })
    .unwrap();
}

// ---- Server emoji as pictures in the message box ----

/// How many tokens (pictures) the message box draws.
fn tokens(f: &Fixture, cx: &mut TestAppContext) -> usize {
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window
            .find_all("composer-token")
            .into_iter()
            .filter(|t| t.visible())
            .count()
    })
    .unwrap()
}

fn press(f: &Fixture, cx: &mut TestAppContext, keys: &[&str]) {
    cx.update_window(f.window, |_, window, cx| {
        for key in keys {
            window.press(key, cx);
        }
    })
    .unwrap();
    cx.run_until_parked();
}

fn type_in_box(f: &Fixture, cx: &mut TestAppContext, text: &str) {
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.click("composer-input", cx);
        window.input(text, cx);
    })
    .unwrap();
    cx.run_until_parked();
    draw(f, cx);
}

/// Type at the caret, the box already focused.
fn type_on(f: &Fixture, cx: &mut TestAppContext, text: &str) {
    cx.update_window(f.window, |_, window, cx| window.input(text, cx))
        .unwrap();
    cx.run_until_parked();
    draw(f, cx);
}

fn box_text(cx: &mut TestAppContext) -> String {
    cx.update(|cx| message_actions::box_text(cx))
}

#[gpui_kit::test]
fn a_typed_server_emoji_draws_as_its_picture_and_edits_as_one(cx: &mut TestAppContext) {
    let f = open(cx, |_| {});
    type_in_box(&f, cx, "hi :part");
    assert_eq!(tokens(&f, cx), 0, "a name half typed stays text");
    type_on(&f, cx, "y:");
    assert_eq!(tokens(&f, cx), 1, "the picture is drawn in the box");
    assert_eq!(box_text(cx), "hi :party:", "the text is still the name");

    // Left steps over the whole picture.
    press(&f, cx, &["left"]);
    type_on(&f, cx, "X");
    assert_eq!(box_text(cx), "hi X:party:");
    // Right steps back over it, then Backspace takes it whole.
    press(&f, cx, &["right", "backspace"]);
    draw(&f, cx);
    assert_eq!(box_text(cx), "hi X");
    assert_eq!(tokens(&f, cx), 0);
    // Undo brings it back as a picture, redo takes it away again.
    press(&f, cx, &["ctrl-z"]);
    draw(&f, cx);
    assert_eq!(box_text(cx), "hi X:party:");
    assert_eq!(tokens(&f, cx), 1, "after undo");
    press(&f, cx, &["ctrl-y"]);
    draw(&f, cx);
    assert_eq!(box_text(cx), "hi X");
    assert_eq!(tokens(&f, cx), 0, "after redo");
}

#[gpui_kit::test]
fn only_known_names_outside_code_become_pictures(cx: &mut TestAppContext) {
    let f = open(cx, |_| {});
    type_in_box(&f, cx, ":nope: `:party:` :pog: :party:");
    assert_eq!(
        tokens(&f, cx),
        2,
        "an unknown name and one in code stay text; the 7TV and server emoji are pictures"
    );
}

#[gpui_kit::test]
fn a_pasted_name_becomes_a_picture(cx: &mut TestAppContext) {
    let f = open(cx, |_| {});
    cx.update(|cx| cx.write_to_clipboard(ClipboardItem::new_string("so :party:".into())));
    type_in_box(&f, cx, "");
    press(&f, cx, &["ctrl-v"]);
    draw(&f, cx);
    assert_eq!(box_text(cx), "so :party:");
    assert_eq!(tokens(&f, cx), 1);
}

#[gpui_kit::test]
fn a_picture_on_a_wrapped_line_still_draws(cx: &mut TestAppContext) {
    let f = open(cx, |_| {});
    let long = "word ".repeat(80);
    type_in_box(&f, cx, &format!("{long}:party: end"));
    assert_eq!(tokens(&f, cx), 1);
}

#[gpui_kit::test]
fn sending_a_picture_sends_its_name(cx: &mut TestAppContext) {
    let f = open(cx, |_| {});
    type_in_box(&f, cx, "hi :party:");
    assert_eq!(tokens(&f, cx), 1);
    press(&f, cx, &["enter"]);
    let body = f
        .server
        .wait_for("POST", "/api/v1/conversations/c-general/messages", WAIT)
        .json();
    assert_eq!(body["text"], "hi :party:");
    draw(&f, cx);
    assert_eq!(box_text(cx), "");
    assert_eq!(tokens(&f, cx), 0);
}

#[gpui_kit::test]
fn a_draft_turns_into_pictures_once_the_emoji_are_known(cx: &mut TestAppContext) {
    let f = open(cx, |_| {});
    type_in_box(&f, cx, "later :party:");
    f.session
        .update(cx, |s, cx| s.select_conversation("c-random", cx));
    cx.run_until_parked();
    // The catalog is not known yet when the draft comes back.
    let pictures = cx.update(|cx| {
        let kept = cx.global::<emoji::Pictures>().0.as_ref().clone();
        emoji::set(Default::default(), cx);
        kept
    });
    f.session
        .update(cx, |s, cx| s.select_conversation("c-general", cx));
    cx.run_until_parked();
    draw(&f, cx);
    assert_eq!(box_text(cx), "later :party:", "the draft is back");
    assert_eq!(tokens(&f, cx), 0, "as text while no emoji are known");
    cx.update(|cx| emoji::set(pictures, cx));
    cx.run_until_parked();
    draw(&f, cx);
    assert_eq!(tokens(&f, cx), 1, "a picture once they are");
}

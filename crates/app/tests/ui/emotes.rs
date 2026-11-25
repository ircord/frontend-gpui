//! Tests for package S5: Settings → Emotes. The server's emoji listed and
//! searched; rename, aliases, disable and remove; adding by upload, by a
//! pasted link and from Discover; Discover's filters and pages.

use std::time::Duration;

use gpui_kit::test::TestWindowExt;
use gpui_kit::{AppContext, ScrollDelta, TestAppContext, Window, point, px};
use ircord_app::settings::emotes::{self, Tab};
use ircord_client::testing::{CANNED_COOKIE, CannedServer, Reply};
use serde_json::{Value, json};

use crate::fixture::{Fixture, bootstrap, open_with, wait};

const WAIT: Duration = crate::fixture::HANG_GUARD;

/// An 8×8 red PNG.
const RED_PNG: [u8; 75] = [
    137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 8, 0, 0, 0, 8, 8, 6, 0,
    0, 0, 196, 15, 190, 139, 0, 0, 0, 18, 73, 68, 65, 84, 120, 156, 99, 248, 207, 192, 240, 31, 31,
    102, 24, 25, 10, 0, 194, 215, 127, 129, 47, 113, 224, 1, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66,
    96, 130,
];

fn emote(id: &str, code: &str, enabled: bool, manage: bool, source: &str) -> Value {
    json!({"id": id, "name": code, "shortcode": code, "aliases": [format!("{code}_x")],
           "retired": [], "enabled": enabled, "animated": false,
           "url": format!("/api/v1/emotes/{id}/image"),
           "staticUrl": format!("/api/v1/emotes/{id}/image"),
           "width": 32, "height": 32, "source": source, "providerId": format!("p-{id}"),
           "creator": "Ann", "canManage": manage, "createdAt": "", "updatedAt": ""})
}

fn catalog(can_manage: bool) -> Value {
    json!({"version": 1, "canManage": can_manage, "canAdd": true, "maxBytes": 262144,
           "types": ["image/png", "image/gif", "image/webp", "image/jpeg"],
           "providers": ["7tv", "ffz", "emoji.gg"],
           "items": [emote("e1", "party", true, true, "upload"),
                     emote("e2", "sleepy", false, true, "7tv"),
                     emote("e3", "theirs", true, false, "ffz")]})
}

fn discovery(cursor: &str) -> Value {
    let item = |id: &str, name: &str, installed: bool, animated: bool| {
        json!({"provider": "emoji.gg", "providerId": id, "name": name, "creator": "Bo",
               "sourceUrl": "https://emoji.gg/x", "previewUrl":
               format!("/api/v1/emotes/discover/emoji.gg/{id}/preview"),
               "license": "free", "animated": animated, "width": 0, "height": 0,
               "category": "", "installed": installed})
    };
    let items = if cursor.is_empty() {
        vec![
            item("g1", "Cat Wave", false, false),
            item("g2", "dog", true, false),
            item("g4", "gone", false, false),
        ]
    } else {
        vec![item("g3", "page two", false, true)]
    };
    json!({"items": items, "nextCursor": if cursor.is_empty() { "c2" } else { "" },
           "stale": false,
           "filters": {"sources": ["emoji.gg", "7tv"], "categories": [],
                       "animations": ["all", "static", "animated"]}})
}

fn serve(server: &CannedServer, can_manage: bool) {
    server.route("GET", "/api/v1/emotes", 200, catalog(can_manage));
    server.route(
        "POST",
        "/api/v1/emotes",
        201,
        emote("e9", "red", true, true, "upload"),
    );
    server.route(
        "POST",
        "/api/v1/emotes/imports",
        201,
        emote("e8", "imported", true, true, "7tv"),
    );
    server.route(
        "PATCH",
        "/api/v1/emotes/*",
        200,
        emote("e1", "party", true, true, "upload"),
    );
    server.route_reply("DELETE", "/api/v1/emotes/*", Reply::empty());
    server.route_fn("GET", "/api/v1/emotes/discover", |request| {
        let cursor = request.query_param("cursor").unwrap_or_default();
        Reply::json(200, discovery(&cursor))
    });
    // The pictures, which the real server serves only with the session;
    // `g4`'s is unavailable.
    server.route_fn(
        "GET",
        "/api/v1/emotes/discover/emoji.gg/*/preview",
        |request| {
            if request.header("cookie") != Some(CANNED_COOKIE) {
                Reply::problem(401, "authentication required")
            } else if request.path.contains("/g4/") {
                Reply::problem(503, "The preview is unavailable.")
            } else {
                Reply::bytes(200, "image/png", RED_PNG.to_vec())
            }
        },
    );
}

fn open(cx: &mut TestAppContext) -> Fixture {
    let f = open_with(cx, true, |s| serve(s, false));
    bootstrap(&f, cx);
    let session = f.session.clone();
    cx.update(|cx| ircord_app::settings::open(&session, Some("server-emotes"), cx));
    shows(&f, cx, "emote-e1");
    f
}

fn frame(f: &Fixture, cx: &mut TestAppContext, run: impl FnOnce(&mut Window, &mut gpui_kit::App)) {
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        run(window, cx);
    })
    .unwrap();
    cx.run_until_parked();
}

fn found(f: &Fixture, cx: &mut TestAppContext, id: &str) -> bool {
    let id = gpui_kit::ElementId::Name(id.to_string().into());
    let mut found = false;
    frame(f, cx, |window, _| found = window.try_find(id).is_some());
    found
}

fn shows(f: &Fixture, cx: &mut TestAppContext, id: &str) {
    let (window, name) = (f.window, gpui_kit::ElementId::Name(id.to_string().into()));
    wait(cx, id, move |cx| {
        window
            .update(cx, |_, window, cx| {
                window.render_frame(cx);
                window.try_find(name.clone()).is_some()
            })
            .unwrap_or(false)
    });
}

fn reads(f: &Fixture, cx: &mut TestAppContext, id: &str, want: &str) {
    let (window, name) = (f.window, gpui_kit::ElementId::Name(id.to_string().into()));
    let want = want.to_string();
    wait(cx, &format!("{id} reading {want:?}"), move |cx| {
        window
            .update(cx, |_, window, cx| {
                window.render_frame(cx);
                window
                    .try_find(name.clone())
                    .is_some_and(|e| e.label() == Some(want.as_str()))
            })
            .unwrap_or(false)
    });
}

/// Scroll the settings page until `id` is in view, then click it.
fn click(f: &Fixture, cx: &mut TestAppContext, id: &str) {
    shows(f, cx, id);
    let name = gpui_kit::ElementId::Name(id.to_string().into());
    for _ in 0..10 {
        let mut by = 0.0;
        frame(f, cx, |window, _| {
            let (Some(e), Some(pane)) = (
                window.try_find(name.clone()),
                window.try_find("settings-content"),
            ) else {
                return;
            };
            let (e, pane) = (e.bounds(), pane.bounds());
            if e.top() < pane.top() || e.bottom() > pane.bottom() {
                by = f32::from(pane.center().y - e.center().y);
            }
        });
        if by == 0.0 {
            break;
        }
        frame(f, cx, |window, cx| {
            window.scroll(
                "settings-content",
                ScrollDelta::Pixels(point(px(0.), px(by))),
                cx,
            )
        });
    }
    frame(f, cx, |window, cx| window.click(name, cx));
}

fn type_into(f: &Fixture, cx: &mut TestAppContext, id: &str, text: &str) {
    click(f, cx, id);
    frame(f, cx, |window, cx| {
        window.press("ctrl-a", cx);
        window.press("backspace", cx);
        window.input(text, cx);
    });
}

#[gpui_kit::test]
fn the_server_list_searches_and_offers_what_you_may_manage(cx: &mut TestAppContext) {
    let f = open(cx);
    reads(&f, cx, "emotes-tab-server", "Server emoji (3)");
    for id in ["emote-e1", "emote-e2", "emote-e3"] {
        assert!(found(&f, cx, id), "{id}");
    }
    reads(&f, cx, "emote-e1", ":party:");
    // Yours to manage: rename and the rest; not someone else's.
    assert!(found(&f, cx, "emote-rename-e1"));
    assert!(found(&f, cx, "emote-toggle-e2"));
    assert!(!found(&f, cx, "emote-rename-e3"));
    assert!(found(&f, cx, "emotes-upload"));

    // Aliases match too.
    type_into(&f, cx, "emote-search", "sleepy_x");
    wait(cx, "the search", |_| true);
    assert!(!found(&f, cx, "emote-e1"));
    assert!(found(&f, cx, "emote-e2"));
    type_into(&f, cx, "emote-search", "nothing");
    shows(&f, cx, "emotes-no-matches");
}

#[gpui_kit::test]
fn a_manager_renames_re_aliases_disables_and_removes(cx: &mut TestAppContext) {
    let f = open(cx);

    click(&f, cx, "emote-rename-e1");
    type_into(&f, cx, "prompt-input", "partytime");
    click(&f, cx, "ok");
    let rename = f.server.wait_for("PATCH", "/api/v1/emotes/*", WAIT);
    assert_eq!(rename.path, "/api/v1/emotes/e1");
    assert_eq!(rename.json(), json!({"shortcode": "partytime"}));
    reads(&f, cx, "emotes-status", "Emote renamed.");

    click(&f, cx, "emote-aliases-e1");
    type_into(&f, cx, "prompt-input", "a1, , b2 ");
    click(&f, cx, "ok");
    let aliases = f.server.wait_for_nth("PATCH", "/api/v1/emotes/*", 2, WAIT);
    assert_eq!(aliases.json(), json!({"aliases": ["a1", "b2"]}));
    reads(&f, cx, "emotes-status", "Aliases saved.");

    // A disabled emote is enabled again.
    reads(&f, cx, "emote-toggle-e2", "Enable");
    click(&f, cx, "emote-toggle-e2");
    let toggle = f.server.wait_for_nth("PATCH", "/api/v1/emotes/*", 3, WAIT);
    assert_eq!(toggle.path, "/api/v1/emotes/e2");
    assert_eq!(toggle.json(), json!({"enabled": true}));
    reads(&f, cx, "emotes-status", "Emote enabled.");

    click(&f, cx, "emote-remove-e1");
    click(&f, cx, "ok");
    let remove = f.server.wait_for("DELETE", "/api/v1/emotes/*", WAIT);
    assert_eq!(remove.path, "/api/v1/emotes/e1");
    reads(&f, cx, "emotes-status", "Emote removed.");
    // Each change reads the catalog again.
    assert!(f.server.requests_to("GET", "/api/v1/emotes").len() >= 5);
}

#[gpui_kit::test]
fn an_upload_sends_the_picture_its_still_and_names(cx: &mut TestAppContext) {
    let f = open(cx);
    click(&f, cx, "emotes-upload");
    shows(&f, cx, "emote-choose-file");

    // Not a picture the server takes.
    let session = f.session.clone();
    frame(&f, cx, |window, cx| {
        emotes::choose(&session, "notes.txt", b"hello".to_vec(), window, cx)
    });
    reads(
        &f,
        cx,
        "emotes-error",
        "This image type is not supported. Choose a supported file.",
    );

    let session = f.session.clone();
    frame(&f, cx, |window, cx| {
        emotes::choose(&session, "Red Dot.png", RED_PNG.to_vec(), window, cx)
    });
    reads(&f, cx, "emote-file-name", "Red Dot.png");
    shows(&f, cx, "emote-chat-preview");
    type_into(&f, cx, "emote-aliases", "rd, dot");
    click(&f, cx, "emote-add");
    let upload = f.server.wait_for("POST", "/api/v1/emotes", WAIT);
    assert!(
        upload
            .header("content-type")
            .is_some_and(|t| t.starts_with("multipart/form-data; boundary="))
    );
    let body = String::from_utf8_lossy(&upload.body);
    // The shortcode suggested from the file name.
    assert!(
        body.contains("name=\"shortcode\"\r\n\r\nred_dot\r\n"),
        "{body}"
    );
    assert!(
        body.contains("name=\"aliases\"\r\n\r\nrd,dot\r\n"),
        "{body}"
    );
    assert!(body.contains("name=\"file\"; filename=\"Red Dot.png\"\r\nContent-Type: image/png"));
    assert!(body.contains("name=\"still\"; filename=\"still.png\""));
    reads(&f, cx, "emotes-status", "Emote added.");
    // The form is ready for the next one.
    assert!(!found(&f, cx, "emote-file-name"));
}

#[gpui_kit::test]
fn a_pasted_link_imports_from_its_provider(cx: &mut TestAppContext) {
    let f = open(cx);
    click(&f, cx, "emotes-upload");
    click(&f, cx, "emote-how-import");
    type_into(&f, cx, "emote-link", "not a link");
    click(&f, cx, "emote-add");
    reads(
        &f,
        cx,
        "emotes-error",
        "Paste a 7TV or FrankerFaceZ emote link.",
    );
    type_into(
        &f,
        cx,
        "emote-link",
        "https://www.frankerfacez.com/emoticon/128054-OMEGALUL",
    );
    type_into(&f, cx, "emote-shortcode", "omega");
    click(&f, cx, "emote-add");
    let import = f.server.wait_for("POST", "/api/v1/emotes/imports", WAIT);
    assert_eq!(
        import.json(),
        json!({"provider": "ffz", "providerId": "128054", "shortcode": "omega", "aliases": []})
    );
    reads(&f, cx, "emotes-status", "Emote imported.");
}

#[gpui_kit::test]
fn discover_filters_pages_and_adds(cx: &mut TestAppContext) {
    let f = open(cx);
    let session = f.session.clone();
    cx.update(|cx| emotes::show_tab(&session, Tab::Discover, cx));
    shows(&f, cx, "discover-emoji.gg-g1");
    let first = f.server.wait_for("GET", "/api/v1/emotes/discover", WAIT);
    assert_eq!(first.query_param("animation").as_deref(), Some("all"));
    assert_eq!(first.query_param("limit").as_deref(), Some("30"));
    reads(&f, cx, "discover-emoji.gg-g1", ":Cat Wave:");

    // A filter asks again from the first page.
    click(&f, cx, "emote-animation-static");
    let still = f
        .server
        .wait_for_nth("GET", "/api/v1/emotes/discover", 2, WAIT);
    assert_eq!(still.query_param("animation").as_deref(), Some("static"));
    click(&f, cx, "emote-source-7tv");
    let source = f
        .server
        .wait_for_nth("GET", "/api/v1/emotes/discover", 3, WAIT);
    assert_eq!(source.query_param("source").as_deref(), Some("7tv"));

    // Search, after typing settles.
    type_into(&f, cx, "discover-search", "cat");
    wait(cx, "the search", |_| {
        f.server
            .requests_to("GET", "/api/v1/emotes/discover")
            .iter()
            .any(|r| r.query_param("query").as_deref() == Some("cat"))
    });

    // Pages.
    click(&f, cx, "discover-next");
    wait(cx, "page two", |_| {
        f.server
            .requests_to("GET", "/api/v1/emotes/discover")
            .iter()
            .any(|r| r.query_param("cursor").as_deref() == Some("c2"))
    });
    shows(&f, cx, "discover-emoji.gg-g3");
    click(&f, cx, "discover-previous");
    shows(&f, cx, "discover-emoji.gg-g1");

    // Pick one: its shortcode is suggested; add it.
    click(&f, cx, "discover-emoji.gg-g1");
    shows(&f, cx, "emote-chat-preview");
    click(&f, cx, "emote-add");
    let import = f.server.wait_for("POST", "/api/v1/emotes/imports", WAIT);
    assert_eq!(
        import.json(),
        json!({"provider": "emoji.gg", "providerId": "g1", "shortcode": "cat_wave",
               "aliases": []})
    );
    reads(&f, cx, "emotes-status", "Emote imported.");

    // One already on the server cannot be added again.
    click(&f, cx, "discover-emoji.gg-g2");
    reads(&f, cx, "emote-add", "Installed");
}

#[gpui_kit::test]
fn discover_shows_the_pictures(cx: &mut TestAppContext) {
    let f = open(cx);
    let session = f.session.clone();
    cx.update(|cx| emotes::show_tab(&session, Tab::Discover, cx));

    // Every tile's still, read with the session, as the web's tiles ask.
    shows(&f, cx, "discover-emoji.gg-g1-picture");
    shows(&f, cx, "discover-emoji.gg-g2-picture");
    let path = "/api/v1/emotes/discover/emoji.gg/g1/preview";
    let still = f.server.wait_for("GET", path, WAIT);
    assert_eq!(still.header("cookie"), Some(CANNED_COOKIE));
    assert_eq!(still.query_param("static").as_deref(), Some("true"));
    // One the server cannot give keeps its tile, without a picture.
    f.server
        .wait_for("GET", "/api/v1/emotes/discover/emoji.gg/g4/preview", WAIT);
    reads(&f, cx, "discover-emoji.gg-g4", ":gone:");
    assert!(!found(&f, cx, "discover-emoji.gg-g4-picture"));
    // Each is read once, however often the page is drawn.
    assert_eq!(f.server.requests_to("GET", path).len(), 1);

    // Chosen, an animated one plays in the preview.
    click(&f, cx, "discover-next");
    shows(&f, cx, "discover-emoji.gg-g3-picture");
    click(&f, cx, "discover-emoji.gg-g3");
    shows(&f, cx, "emote-chat-preview");
    let g3 = "/api/v1/emotes/discover/emoji.gg/g3/preview";
    wait(cx, "the played picture", |_| {
        f.server
            .requests_to("GET", g3)
            .iter()
            .any(|r| r.query_param("static").is_none())
    });
}

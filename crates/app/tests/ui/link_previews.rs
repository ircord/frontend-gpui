//! Headless tests for package L1: link previews.
//!
//! The links come from the web client's link fixtures
//! (`shared/embed-fixtures/iframely.json`); the canned server answers each
//! with the kind of preview the backend gives it.

use gpui_kit::test::TestWindowExt;
use gpui_kit::{AppContext, TestAppContext};
use ircord_client::contract::*;
use ircord_client::store::link_media::Preview;
use ircord_client::testing::{CannedServer, Reply};
use serde_json::{Value, json};

use crate::fixture::{Fixture, message, open_with, snapshot, wait};

const POST: &str = "https://www.instagram.com/p/HbBy-ExIyF/";
const PLAYER: &str = "https://www.youtube.com/watch?v=etDRmrB9Css";
const PAGE: &str = "https://vimeo.com/blog/post/hello";
const FILE: &str = "https://files.example/report.pdf";
const RESTRICTED: &str = "https://twitter.com/potus";
const PICTURE: &str = "https://media.giphy.com/media/3o6Zt09XtyqOJWVgRO/source.gif";
const LONG: &str = "https://github.com/zed-industries/zed";

/// What the server finds behind each link.
fn answer(url: &str) -> Value {
    match url {
        POST => json!({
            "kind": "media", "provider": "instagram", "url": POST,
            "title": "Post by nssmagazine", "author": "nssmagazine",
            "description": "Spring looks\n\n\n\nfrom Milan",
            "assets": [
                {"kind": "image", "url": "https://cdn.test/a.jpg", "width": 1080, "height": 1350},
                {"kind": "image", "url": "https://cdn.test/b.jpg", "width": 1080, "height": 1350},
                {"kind": "image", "url": "https://cdn.test/c.jpg", "width": 1080, "height": 1350}
            ]
        }),
        PLAYER => json!({
            "kind": "widget", "provider": "oembed", "url": PLAYER,
            "title": "Rick Astley - Never Gonna Give You Up", "assets": [],
            "card": {"site": "youtube.com", "title": "Never Gonna Give You Up"},
            "widget": {"external": true, "height": 270, "token": "t", "url": "https://www.youtube.com/embed/x",
                       "thumbnail": "https://cdn.test/thumb.jpg"}
        }),
        PAGE => json!({
            "kind": "card", "provider": "page", "url": PAGE, "assets": [],
            "card": {"site": "vimeo.com", "title": "Hello from the blog",
                     "description": "What we built this year", "image": "https://cdn.test/page.jpg"}
        }),
        FILE => json!({
            "kind": "file", "provider": "file", "url": FILE, "assets": [],
            "file": {"name": "report.pdf", "contentType": "application/pdf", "size": 2_500_000,
                     "url": "https://cdn.test/report.pdf"}
        }),
        RESTRICTED => json!({
            "kind": "card", "provider": "link", "url": RESTRICTED, "assets": [],
            "card": {"site": "twitter.com", "title": "twitter.com", "reason": "restricted"}
        }),
        LONG => json!({
            "kind": "card", "provider": "page", "url": LONG, "assets": [],
            "card": {"site": "github.com",
                     "title": "GitHub - zed-industries/zed: Code at the speed of thought – Zed is a high-performance, multiplayer code editor from the creators of Atom and Tree-sitter.",
                     "description": "Code at the speed of thought – Zed is a high-performance, multiplayer code editor from the creators of Atom and Tree-sitter. It is also open source and written in Rust, with a lot of words after that.",
                     "reason": "no_oembed"}
        }),
        PICTURE => json!({
            "kind": "media", "provider": "media", "url": PICTURE,
            "assets": [{"kind": "image", "url": "https://cdn.test/source.gif", "contentType": "image/gif",
                        "width": 480, "height": 270}]
        }),
        _ => json!({}),
    }
}

/// `#general`'s history is `messages`; link media answers after
/// [`answer`], and a message lookup finds `2`'s copy or nothing.
fn server(server: &CannedServer, nsfw: bool, messages: Vec<Message>) {
    let mut s = snapshot();
    s.conversations[0].channel_settings = Some(ChannelSettings {
        nsfw,
        ..Default::default()
    });
    server.snapshot(&s);
    let count = messages.len();
    server.route_fn("GET", "/api/v1/conversations/*/messages", move |request| {
        let items = if request.path.ends_with("/c-general/messages") {
            messages.clone()
        } else {
            Vec::new()
        };
        Reply::json(
            200,
            serde_json::to_value(MessagePage {
                items,
                ..Default::default()
            })
            .unwrap(),
        )
    });
    let _ = count;
    server.route_fn(
        "POST",
        "/api/v1/conversations/*/messages/*/link-media",
        |request| {
            let url = request.json()["url"].as_str().unwrap_or("").to_string();
            Reply::json(200, answer(&url))
        },
    );
    server.route_fn("GET", "/api/v1/messages/*", |request| {
        if request.path.ends_with("/elsewhere") {
            let mut m = message("elsewhere", "c-random", "over in random, a longer thought");
            m.sender_id = "m-eve".into();
            Reply::json(200, serde_json::to_value(m).unwrap())
        } else {
            Reply::problem(404, "message not found")
        }
    });
}

fn loaded(f: &Fixture, cx: &mut TestAppContext, count: usize) {
    wait(cx, "#general's history", |cx| {
        f.session.read(cx).client().is_some_and(|c| {
            c.read()
                .timelines
                .get("c-general")
                .is_some_and(|t| t.loaded && t.entries.len() == count)
        })
    });
}

/// Draw until every link in `message` has an answer.
fn resolved(f: &Fixture, cx: &mut TestAppContext, message: &str, urls: &[&str]) {
    let urls: Vec<String> = urls.iter().map(|u| u.to_string()).collect();
    let message = message.to_string();
    let window = f.window;
    wait(cx, "the previews", move |cx| {
        let _ = window.update(cx, |_, window, cx| window.render_frame(cx));
        f.session.read(cx).client().is_some_and(|c| {
            let w = c.read();
            urls.iter().all(|u| {
                matches!(
                    w.link_preview(&message, u),
                    Some(Preview::Ready { .. } | Preview::Failed { .. })
                )
            })
        })
    });
}

#[gpui_kit::test]
fn cards_show_what_the_server_found(cx: &mut TestAppContext) {
    let text = format!("look {POST} and {PLAYER}, {PAGE} {FILE} ({RESTRICTED})");
    let f = open_with(cx, true, |s| {
        server(s, false, vec![message("p1", "c-general", &text)])
    });
    loaded(&f, cx, 1);
    resolved(&f, cx, "p1", &[POST, PLAYER, PAGE, FILE, RESTRICTED]);
    let asked = f.server.requests_to(
        "POST",
        "/api/v1/conversations/c-general/messages/p1/link-media",
    );
    assert_eq!(asked.len(), 5, "each link is resolved once");
    let mut urls: Vec<String> = asked
        .iter()
        .map(|r| r.json()["url"].as_str().unwrap_or("").to_string())
        .collect();
    urls.sort();
    let mut expected = vec![POST, PLAYER, PAGE, FILE, RESTRICTED];
    expected.sort();
    assert_eq!(urls, expected, "punctuation stays outside the links");
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        // The social post: its byline, caption and pictures as a gallery.
        assert_eq!(window.find("link-p1-0").label(), Some("Spring looks"));
        assert_eq!(
            window.find("link-p1-0-byline").label(),
            Some("@nssmagazine · Instagram")
        );
        assert_eq!(
            window.find("link-p1-0-caption").label(),
            Some("Spring looks\n\nfrom Milan")
        );
        assert_eq!(
            window.find("gallery-link-p1-0").label(),
            Some("Gallery of 3 files")
        );
        // The player opens the browser.
        assert_eq!(
            window.find("link-p1-1-player").label(),
            Some("Open Rick Astley - Never Gonna Give You Up in your browser")
        );
        assert_eq!(
            window.find("link-p1-1-player-detail").label(),
            Some("youtube.com · plays in your browser")
        );
        // A page: site, title and description.
        assert_eq!(window.find("link-p1-2").label(), Some(PAGE));
        assert_eq!(window.find("link-p1-2-site").label(), Some("vimeo.com"));
        assert_eq!(
            window.find("link-p1-2-title").label(),
            Some("Hello from the blog")
        );
        assert_eq!(
            window.find("link-p1-2").bounds().size.height,
            gpui_kit::px(150.),
            "a page card has one height"
        );
        // A file: its detected type and size.
        assert_eq!(window.find("link-p1-3").label(), Some("report.pdf"));
        assert_eq!(
            window.find("link-p1-3-detail").label(),
            Some("PDF document · 2.5 MB")
        );
        // A site that refuses previews says so, and still has a tile.
        assert_eq!(
            window.find("link-p1-4-status").label(),
            Some("This site does not allow previews.")
        );
        assert_eq!(window.find("link-p1-4-art").label(), Some("twitter.com"));
    })
    .unwrap();
}

#[gpui_kit::test]
fn a_long_page_card_keeps_every_line_in_its_own_row(cx: &mut TestAppContext) {
    let f = open_with(cx, true, |s| {
        server(s, false, vec![message("p1", "c-general", LONG)])
    });
    loaded(&f, cx, 1);
    resolved(&f, cx, "p1", &[LONG]);
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        let card = window.find("link-p1-0").bounds();
        assert_eq!(card.size.height, gpui_kit::px(150.));
        // No picture: the generated artwork sits beside the text.
        let art = window.find("link-p1-0-art");
        assert!(art.visible());
        let label = art.label().unwrap_or_default();
        assert!(
            label.starts_with("GitHub - zed-industries/zed") && label.ends_with('…'),
            "the artwork carries the cut title: {label}"
        );
        assert_eq!(art.bounds().size.width, gpui_kit::px(96.));
        let site = window.find("link-p1-0-site").bounds();
        let title = window.find("link-p1-0-title").bounds();
        let description = window.find("link-p1-0-description").bounds();
        let status = window.find("link-p1-0-status").bounds();
        assert!(site.bottom() <= title.top(), "{site:?} above {title:?}");
        assert!(
            title.size.height <= gpui_kit::px(40.),
            "two title lines at most"
        );
        assert!(
            title.bottom() <= description.top(),
            "{title:?} above {description:?}"
        );
        assert!(
            description.bottom() <= status.top(),
            "{description:?} above {status:?}"
        );
        assert!(status.bottom() <= card.bottom(), "{status:?} in {card:?}");
        assert!(art.bounds().right() <= title.left());
    })
    .unwrap();
}

#[gpui_kit::test]
fn a_post_gallery_opens_the_viewer(cx: &mut TestAppContext) {
    let f = open_with(cx, true, |s| {
        server(s, false, vec![message("p1", "c-general", POST)])
    });
    loaded(&f, cx, 1);
    resolved(&f, cx, "p1", &[POST]);
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.click("attachment-link-p1-0-1", cx);
        window.render_frame(cx);
        assert_eq!(
            window.find("viewer-position").label(),
            Some("Post by nssmagazine · 2 of 3")
        );
        window.press("right", cx);
        assert_eq!(
            window.find("viewer-position").label(),
            Some("Post by nssmagazine · 3 of 3")
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn a_linked_picture_reserves_its_size_and_comes_from_the_cache(cx: &mut TestAppContext) {
    let mut m = message("p1", "c-general", PICTURE);
    m.embed_geometry = Some(vec![EmbedGeometry {
        url: PICTURE.into(),
        width: 480,
        height: 270,
    }]);
    let f = open_with(cx, true, |s| server(s, false, vec![m]));
    loaded(&f, cx, 1);
    resolved(&f, cx, "p1", &[PICTURE]);
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        let picture = window.find("image-link-p1-0-0");
        assert_eq!(
            picture.label(),
            Some("https://cdn.test/source.gif"),
            "the server's copy, not the original"
        );
        let size = picture.bounds().size;
        assert_eq!(
            (size.width, size.height),
            (gpui_kit::px(400.), gpui_kit::px(225.))
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn sensitive_channels_cover_link_media(cx: &mut TestAppContext) {
    let f = open_with(cx, true, |s| {
        server(s, true, vec![message("p1", "c-general", POST)])
    });
    loaded(&f, cx, 1);
    resolved(&f, cx, "p1", &[POST]);
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.find("media-cover-link-p1-0").visible());
        assert!(window.try_find("gallery-link-p1-0").is_none());
        window.click("media-reveal-link-p1-0", cx);
        window.render_frame(cx);
        assert!(window.find("gallery-link-p1-0").visible());
    })
    .unwrap();
}

#[gpui_kit::test]
fn message_links_show_the_message(cx: &mut TestAppContext) {
    let f = open_with(cx, true, |s| {
        let origin = s.address().origin();
        let text = format!(
            "{origin}/channel/c-general?m=2 {origin}/channel/c-random?m=elsewhere {origin}/channel/c-general?m=gone"
        );
        server(
            s,
            false,
            vec![
                message("1", "c-general", &text),
                message("2", "c-general", "second \x02line\x02"),
            ],
        )
    });
    loaded(&f, cx, 2);
    let window = f.window;
    wait(cx, "the linked messages", |cx| {
        let _ = window.update(cx, |_, window, cx| window.render_frame(cx));
        f.session.read(cx).client().is_some_and(|c| {
            let w = c.read();
            w.linked_message("c-random", "elsewhere")
                .is_some_and(|l| matches!(l, ircord_client::store::link_media::Linked::Found(_)))
                && w.linked_message("c-general", "gone")
                    == Some(ircord_client::store::link_media::Linked::Missing)
        })
    });
    assert!(
        f.server.requests_to("GET", "/api/v1/messages/2").is_empty(),
        "a loaded message is not looked up"
    );
    assert!(
        f.server
            .requests_to(
                "POST",
                "/api/v1/conversations/c-general/messages/1/link-media"
            )
            .is_empty(),
        "message links are never resolved as sites"
    );
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("link-1-0-excerpt").label(), Some("second line"));
        assert_eq!(
            window.find("link-1-0").label(),
            Some("Message from bob in #general: second line")
        );
        assert_eq!(
            window.find("link-1-1").label(),
            Some("Message from eve in #random: over in random, a longer thought")
        );
        assert_eq!(
            window.find("link-1-2-excerpt").label(),
            Some("This message is not available to you.")
        );
        assert_eq!(
            window.find("link-1-0").bounds().size.height,
            gpui_kit::px(58.)
        );
        // Clicking one jumps to the message.
        window.click("link-1-0", cx);
    })
    .unwrap();
    wait(cx, "the jump", |cx| {
        f.session
            .read(cx)
            .client()
            .is_some_and(|c| c.read().focused("c-general") == Some("2"))
    });
}

#[gpui_kit::test]
fn a_failed_preview_still_opens_the_link(cx: &mut TestAppContext) {
    // No link-media route: the server answers 404 and the card falls back.
    let f = open_with(cx, true, |_| {});
    crate::fixture::bootstrap(&f, cx);
    resolved(&f, cx, "1", &["https://ircord.com"]);
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("link-1-0").label(), Some("https://ircord.com"));
        assert_eq!(window.find("link-1-0-title").label(), Some("ircord.com"));
        assert_eq!(
            window.find("link-1-0-status").label(),
            Some("The preview could not be loaded.")
        );
    })
    .unwrap();
}

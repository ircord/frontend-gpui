//! A personal theme's background picture: read from the signed-in server,
//! drawn behind the app, and the shell's surfaces turn translucent over it.

use gpui_kit::test::TestWindowExt;
use gpui_kit::{Hsla, TestAppContext};
use ircord_app::theme::{self, backdrop};
use ircord_client::store::appearance::BackdropPlacement;
use ircord_client::testing::{CannedServer, Reply};
use serde_json::{Value, json};

use crate::fixture::{Fixture, bootstrap, open_with, wait};

const PICTURE: &str = "/files/aaaa/bbbb/backdrop.png";

fn png() -> Vec<u8> {
    let picture = image::RgbaImage::from_fn(8, 4, |x, y| {
        image::Rgba([(x * 30) as u8, (y * 60) as u8, 120, 255])
    });
    let mut bytes = Vec::new();
    picture
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )
        .unwrap();
    bytes
}

fn theme_document(background: Value) -> Value {
    json!({
        "format": 1, "id": "t-bg", "name": "Backdrop", "updatedAt": "s1",
        "base": {"dark": "ircord-dark", "light": "ircord-light"},
        "colors": {"dark": {"canvas": "#0a1018", "surface": "#102030", "overlay": "#2a3a4a",
            "ink": "#e0f0ff", "muted": "#90a0b0", "accent": "#40c0ff",
            "palette": {"red": "#ff6060", "orange": "#ffa060", "yellow": "#ffd060",
                "green": "#60ff90", "cyan": "#60e0ff", "blue": "#6090ff",
                "purple": "#c090ff", "pink": "#ff90c0"}}},
        "background": background,
        "css": ""
    })
}

/// The solid colours of the quads the window drew last, with the top-left
/// corner radius of each.
fn quads(f: &Fixture, cx: &mut TestAppContext) -> Vec<(Hsla, f32)> {
    let window = f.window;
    window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            window
                .painted_quads()
                .into_iter()
                .filter_map(|q| Some((q.background.as_solid()?, q.corner_radii.top_left.0)))
                .collect()
        })
        .unwrap()
}

fn drawn_quad(quads: &[(Hsla, f32)], color: Hsla) -> bool {
    quads.iter().any(|(c, _)| *c == color)
}

fn radius_of(quads: &[(Hsla, f32)], color: Hsla) -> Option<f32> {
    quads.iter().find(|(c, _)| *c == color).map(|(_, r)| *r)
}

fn serve(server: &CannedServer, theme: Value) {
    server.route(
        "GET",
        "/api/v1/app/appearance",
        200,
        json!({"settings": {"mode": "dark", "theme": "t-bg"}, "updatedAt": "r1"}),
    );
    server.route(
        "GET",
        "/api/v1/app/themes",
        200,
        json!({"format": 1, "customCss": true, "themes": [theme], "updatedAt": "l1"}),
    );
    server.route_reply("GET", PICTURE, Reply::bytes(200, "image/png", png()));
    server.route_fn("PUT", "/api/v1/app/appearance", |request| {
        Reply::json(200, request.json())
    });
}

fn signed_in(cx: &mut TestAppContext, background: Value) -> Fixture {
    let theme = theme_document(background);
    let f = open_with(cx, true, move |s| serve(s, theme));
    bootstrap(&f, cx);
    wait(cx, "the theme in use", |cx| {
        theme::get(cx).main == theme::hex(0x102030)
    });
    f
}

#[gpui_kit::test]
fn a_theme_with_a_background_draws_it_behind_the_app(cx: &mut TestAppContext) {
    let f = signed_in(
        cx,
        json!({"url": PICTURE, "placement": "app", "fit": "cover", "position": "bottom",
               "dim": 55, "blur": 0, "panelOpacity": 85}),
    );
    wait(cx, "the picture", |cx| backdrop::get(cx).is_some());
    let (drawn, shell, sidebar) = cx.update(|cx| {
        let t = theme::get(cx).clone();
        (
            backdrop::get(cx).unwrap(),
            backdrop::shell(&t, cx),
            t.sidebar,
        )
    });
    assert_eq!(drawn.spec.placement, BackdropPlacement::App);
    // The picture has no measured tones, so the dim is raised to the web's
    // safe 60 % although the theme asks for 55.
    assert_eq!((drawn.spec.dim, drawn.spec.panel), (0.60, 0.85));
    // The conversation list lets the picture through; it is not its colour.
    assert_eq!(shell.a, 0.);
    assert_ne!(shell, sidebar);
    // The picture was read once, with the signed-in session's server only.
    assert_eq!(f.server.requests_to("GET", PICTURE).len(), 1);
    // The window draws it: the sidebar colour at the panel opacity over the
    // picture, and the card as the main colour at the dim.
    let t = cx.update(|cx| theme::get(cx).clone());
    let frame = quads(&f, cx);
    assert!(drawn_quad(&frame, t.sidebar.opacity(0.85)));
    assert!(drawn_quad(&frame, t.main.opacity(0.60)));
}

#[gpui_kit::test]
fn a_conversation_background_leaves_the_shell_opaque(cx: &mut TestAppContext) {
    let f = signed_in(
        cx,
        json!({"url": PICTURE, "placement": "conversation", "dim": 70, "panelOpacity": 85}),
    );
    wait(cx, "the picture", |cx| backdrop::get(cx).is_some());
    let t = cx.update(|cx| theme::get(cx).clone());
    let frame = quads(&f, cx);
    // The cover sits inside the card and follows its rounded corners.
    let radius = radius_of(&frame, t.main.opacity(0.70)).expect("the card's cover");
    assert!(radius > 0.);
    assert!(!drawn_quad(&frame, t.sidebar.opacity(0.85)));
    cx.update(|cx| {
        let t = theme::get(cx).clone();
        assert_eq!(
            backdrop::get(cx).unwrap().spec.placement,
            BackdropPlacement::Conversation
        );
        assert_eq!(backdrop::shell(&t, cx), t.sidebar);
    });
}

#[gpui_kit::test]
fn a_theme_without_a_background_draws_none(cx: &mut TestAppContext) {
    let f = signed_in(cx, Value::Null);
    cx.update(|cx| {
        let t = theme::get(cx).clone();
        assert!(backdrop::get(cx).is_none());
        assert_eq!(backdrop::shell(&t, cx), t.sidebar);
    });
    assert!(no_upload_requested(&f));
    let t = cx.update(|cx| theme::get(cx).clone());
    let frame = quads(&f, cx);
    assert!(!drawn_quad(&frame, t.sidebar.opacity(0.85)));
    assert!(!drawn_quad(&frame, t.main.opacity(0.60)));
}

/// No picture of any name was asked for.
fn no_upload_requested(f: &Fixture) -> bool {
    f.server
        .requests()
        .iter()
        .all(|r| !r.path.starts_with("/files/") && !r.path.starts_with("/api/v1/uploads/"))
}

#[gpui_kit::test]
fn a_background_that_is_not_an_upload_on_this_server_is_not_read(cx: &mut TestAppContext) {
    // Another host, and a path on this server that is not an upload: the
    // second would be asked for if only the host were checked.
    for (n, url) in ["https://tracker.example/castle.png", "/avatars/castle.png"]
        .into_iter()
        .enumerate()
    {
        let f = signed_in(cx, json!({"url": url, "placement": "app"}));
        cx.update(|cx| assert!(backdrop::get(cx).is_none(), "{n}"));
        assert!(
            f.server
                .requests()
                .iter()
                .all(|r| !r.path.contains("castle.png")),
            "{url}"
        );
    }
}

#[gpui_kit::test]
fn a_failed_read_is_tried_again_and_a_theme_without_a_picture_drops_it(cx: &mut TestAppContext) {
    let theme = theme_document(json!({"url": PICTURE, "placement": "app"}));
    let answers = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let seen = answers.clone();
    let f = open_with(cx, true, move |s| {
        serve(s, theme);
        // The first read fails, as a server restarting would.
        s.route_fn("GET", PICTURE, move |_| {
            if seen.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                Reply::problem(500, "restarting")
            } else {
                Reply::bytes(200, "image/png", png())
            }
        });
    });
    bootstrap(&f, cx);
    wait(cx, "the first read", |_| {
        answers.load(std::sync::atomic::Ordering::SeqCst) >= 1
    });
    cx.run_until_parked();
    cx.update(|cx| assert!(backdrop::get(cx).is_none()));
    // The session comes back and applies the appearance again.
    cx.update(theme::appearance::apply);
    wait(cx, "the picture after the retry", |cx| {
        theme::appearance::apply(cx);
        backdrop::get(cx).is_some()
    });
    // Switching to a theme without one removes it.
    cx.update(|cx| {
        theme::appearance::edit(cx, |a| a.theme = None);
    });
    wait(cx, "no picture", |cx| backdrop::get(cx).is_none());
}

/// Over an app-wide picture the panels are frosted: the top bar, the
/// conversation list and the card each draw the blurred copy, and the window
/// draws the sharp picture once for the gaps between them.
#[gpui_kit::test]
fn panels_over_an_app_wide_picture_are_frosted(cx: &mut TestAppContext) {
    let f = signed_in(
        cx,
        json!({"url": PICTURE, "placement": "app", "dim": 55, "panelOpacity": 85}),
    );
    wait(cx, "the picture", |cx| backdrop::get(cx).is_some());
    let before = cx.update(|cx| backdrop::painted(cx));
    quads(&f, cx);
    let after = cx.update(|cx| backdrop::painted(cx));
    assert_eq!(after.0 - before.0, 1, "one sharp picture behind the window");
    assert_eq!(after.1 - before.1, 3, "the bar, the list and the card");
}

#[gpui_kit::test]
fn a_conversation_picture_is_sharp_and_nothing_is_frosted(cx: &mut TestAppContext) {
    let f = signed_in(
        cx,
        json!({"url": PICTURE, "placement": "conversation", "dim": 55, "panelOpacity": 85}),
    );
    wait(cx, "the picture", |cx| backdrop::get(cx).is_some());
    let before = cx.update(|cx| backdrop::painted(cx));
    quads(&f, cx);
    let after = cx.update(|cx| backdrop::painted(cx));
    assert_eq!((after.0 - before.0, after.1 - before.1), (1, 0));
}

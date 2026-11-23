//! A personal theme's background picture: read from the signed-in server,
//! drawn behind the app, and the shell's surfaces turn translucent over it.

use gpui_kit::TestAppContext;
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
    // The web never lets the dim or the panel opacity fall below its floors.
    assert!(drawn.spec.dim >= 0.55 && drawn.spec.panel >= 0.85);
    // The conversation list lets the picture through; it is not its colour.
    assert_eq!(shell.a, 0.);
    assert_ne!(shell, sidebar);
    // The picture was read once, with the signed-in session's server only.
    assert_eq!(f.server.requests_to("GET", PICTURE).len(), 1);
}

#[gpui_kit::test]
fn a_conversation_background_leaves_the_shell_opaque(cx: &mut TestAppContext) {
    let _f = signed_in(
        cx,
        json!({"url": PICTURE, "placement": "conversation", "dim": 70, "panelOpacity": 85}),
    );
    wait(cx, "the picture", |cx| backdrop::get(cx).is_some());
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
    assert!(f.server.requests_to("GET", PICTURE).is_empty());
}

#[gpui_kit::test]
fn a_background_that_is_not_an_upload_on_this_server_is_not_read(cx: &mut TestAppContext) {
    let f = signed_in(
        cx,
        json!({"url": "https://tracker.example/castle.png", "placement": "app"}),
    );
    cx.update(|cx| assert!(backdrop::get(cx).is_none()));
    assert!(
        f.server
            .requests()
            .iter()
            .all(|r| !r.path.contains("castle.png"))
    );
}

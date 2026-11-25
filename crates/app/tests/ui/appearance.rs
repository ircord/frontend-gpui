//! Tests for package S1: Settings → Appearance, against the canned server.

use std::sync::{Arc, Mutex};

use gpui_kit::test::TestWindowExt;
use gpui_kit::{AppContext, ClipboardItem, TestAppContext, point, px};
use ircord_app::theme;
use ircord_client::store::appearance::{self as look, Device, Mode};
use ircord_client::testing::{CannedServer, Reply};
use serde_json::{Value, json};

use crate::fixture::{Fixture, bootstrap, open_with, wait};

/// The account's appearance (`settings`) and theme library (`themes`); a
/// save answers what was sent, and a read after it answers the saved copy,
/// as the backend does.
fn serve(server: &CannedServer, settings: Value, themes: Value) {
    let appearance = Arc::new(Mutex::new(json!({"settings": settings, "updatedAt": "r1"})));
    let stored = appearance.clone();
    server.route_fn("GET", "/api/v1/app/appearance", move |_| {
        Reply::json(200, stored.lock().unwrap().clone())
    });
    server.route_fn("PUT", "/api/v1/app/appearance", move |request| {
        let mut body = request.json();
        body["updatedAt"] = json!("r2");
        *appearance.lock().unwrap() = body.clone();
        Reply::json(200, body)
    });
    let library = Arc::new(Mutex::new(
        json!({"format": 1, "customCss": true, "themes": themes, "updatedAt": "l1"}),
    ));
    let stored = library.clone();
    server.route_fn("GET", "/api/v1/app/themes", move |_| {
        Reply::json(200, stored.lock().unwrap().clone())
    });
    let stored = library.clone();
    server.route_fn("PATCH", "/api/v1/app/themes", move |request| {
        let mut library = stored.lock().unwrap();
        library["customCss"] = request.json()["customCss"].clone();
        Reply::json(200, library.clone())
    });
    server.route_fn("PUT", "/api/v1/app/themes/*", move |request| {
        let mut library = library.lock().unwrap();
        let theme = request.json()["theme"].clone();
        library["themes"].as_array_mut().unwrap().push(theme);
        Reply::json(200, library.clone())
    });
}

fn signed_in(cx: &mut TestAppContext, settings: Value, themes: Value) -> Fixture {
    let f = open_with(cx, true, move |s| serve(s, settings, themes));
    bootstrap(&f, cx);
    wait(cx, "the appearance and themes", |cx| {
        let session = f.session.read(cx);
        let client = session.client().unwrap().read();
        client.documents.appearance.is_loaded() && client.documents.themes.is_loaded()
    });
    f
}

/// Open settings on `row` and wait until it is drawn.
fn open_at(f: &Fixture, cx: &mut TestAppContext, row: &'static str) {
    let session = f.session.clone();
    cx.update(|cx| ircord_app::settings::open(&session, Some(row), cx));
    let window = f.window;
    let id = format!("setting-{row}");
    wait(cx, row, move |cx| {
        window
            .update(cx, |_, window, cx| {
                window.render_frame(cx);
                window
                    .try_find(gpui_kit::ElementId::Name(id.clone().into()))
                    .is_some()
            })
            .unwrap_or(false)
    });
}

/// Scroll the settings page until `id` is wholly in view.
fn reveal(f: &Fixture, cx: &mut TestAppContext, id: &gpui_kit::ElementId) {
    for _ in 0..80 {
        let (shown, above) = cx
            .update_window(f.window, |_, window, cx| {
                window.render_frame(cx);
                let pane = window.find("settings-content").bounds();
                window
                    .try_find(id.clone())
                    .map(|e| {
                        let b = e.bounds();
                        (
                            e.visible() && b.top() >= pane.top() && b.bottom() <= pane.bottom(),
                            b.top() < pane.top(),
                        )
                    })
                    .unwrap_or((false, false))
            })
            .unwrap();
        if shown {
            break;
        }
        cx.update_window(f.window, |_, window, cx| {
            window.scroll(
                "settings-content",
                gpui_kit::ScrollDelta::Pixels(gpui_kit::point(
                    gpui_kit::px(0.),
                    gpui_kit::px(if above { 120. } else { -120. }),
                )),
                cx,
            )
        })
        .unwrap();
    }
}

/// Click `id`, scrolling the settings page down until it is wholly in view.
fn click(f: &Fixture, cx: &mut TestAppContext, id: &str) {
    let id = gpui_kit::ElementId::Name(id.to_string().into());
    reveal(f, cx, &id);
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.click(id, cx);
    })
    .unwrap();
    cx.run_until_parked();
}

/// The tokens drawn once a change has finished fading in.
fn colour(cx: &mut TestAppContext) -> theme::Theme {
    wait(cx, "the colours to settle", |cx| !theme::fade::active(cx));
    cx.update(|cx| theme::get(cx).clone())
}

fn saved_appearances(f: &Fixture) -> Vec<Value> {
    f.server
        .requests()
        .into_iter()
        .filter(|r| r.method == "PUT" && r.path == "/api/v1/app/appearance")
        .map(|r| r.json()["settings"].clone())
        .collect()
}

fn dark_tokens(scheme: &str, accent: Option<u32>) -> look::Tokens {
    look::scheme_tokens(look::scheme_for(scheme, Mode::Dark), accent)
}

#[gpui_kit::test]
fn hibiki_effects_follow_the_selected_scheme(cx: &mut TestAppContext) {
    let f = signed_in(
        cx,
        json!({"mode": "dark", "schemeDark": "hibiki-dark"}),
        json!([]),
    );
    assert!(colour(cx).hibiki);
    cx.update(|cx| {
        theme::appearance::edit(cx, |appearance| {
            appearance.scheme_dark = Some("github-dark".into());
        });
    });
    let t = colour(cx);
    assert!(!t.hibiki);
    assert_eq!(t.main, theme::hex(dark_tokens("github-dark", None).main));
    wait(cx, "GitHub Dark saved", |_| {
        saved_appearances(&f)
            .last()
            .is_some_and(|s| s["schemeDark"] == "github-dark")
    });
}

#[gpui_kit::test]
fn the_accounts_appearance_applies_when_it_arrives(cx: &mut TestAppContext) {
    let f = signed_in(
        cx,
        json!({"mode": "dark", "schemeDark": "dracula", "font": "inter", "interfaceScale": 120,
               "corners": "square", "density": "compact"}),
        json!([]),
    );
    wait(cx, "Dracula", |cx| {
        theme::get(cx).main == theme::hex(dark_tokens("dracula", None).main)
    });
    let t = colour(cx);
    assert_eq!(t.font.as_ref(), "Inter");
    assert_eq!(t.scale, 1.2);
    assert!(t.compact);
    assert_eq!(f32::from(t.radius), 2.);
    // With sync on, this device remembers it for the next start.
    let device: Device = cx.update(|cx| f.session.read(cx).config.section("appearance"));
    assert!(device.sync);
    assert_eq!(
        device.settings.and_then(|s| s.scheme_dark).as_deref(),
        Some("dracula")
    );
}

#[gpui_kit::test]
fn switching_scheme_accent_and_font_redraws_and_saves_to_the_account(cx: &mut TestAppContext) {
    let f = signed_in(cx, json!({"mode": "dark"}), json!([]));
    open_at(&f, cx, "appearance-scheme");
    let before = colour(cx).main;
    assert_eq!(before, theme::hex(dark_tokens("ircord-dark", None).main));

    click(&f, cx, "appearance-scheme-nord");
    colour(cx);
    let nord = dark_tokens("nord-dark", None);
    wait(cx, "Nord", |cx| {
        theme::get(cx).main == theme::hex(nord.main)
    });
    assert_ne!(colour(cx).main, before);
    wait(cx, "the scheme saved", |_| {
        saved_appearances(&f)
            .last()
            .is_some_and(|s| s["schemeDark"] == "nord-dark")
    });
    // The Message settings in the same document are kept.
    assert_eq!(saved_appearances(&f).last().unwrap()["mode"], "dark");

    open_at(&f, cx, "appearance-accent");
    click(&f, cx, "appearance-accent-lime");
    let lime = dark_tokens("nord-dark", Some(0x84cc16));
    wait(cx, "the lime accent", |cx| {
        theme::get(cx).accent == theme::hex(lime.accent)
    });

    open_at(&f, cx, "appearance-font");
    click(&f, cx, "appearance-font-atkinson");
    wait(cx, "Atkinson", |cx| {
        theme::get(cx).font.as_ref() == "Atkinson Hyperlegible Next"
    });

    // A click at the track's right end takes the scale to its largest step.
    open_at(&f, cx, "appearance-scale");
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.click_at("appearance-scale-track", point(px(215.), px(12.)), cx);
    })
    .unwrap();
    wait(cx, "150 %", |cx| theme::get(cx).scale == 1.5);
    let label = cx
        .update_window(f.window, |_, window, cx| {
            window.render_frame(cx);
            window
                .find("appearance-scale-value")
                .label()
                .map(str::to_string)
        })
        .unwrap();
    assert_eq!(label.as_deref(), Some("150 %"));

    wait(cx, "every change saved", |_| {
        saved_appearances(&f).last().is_some_and(|s| {
            s["accent"] == "lime" && s["font"] == "atkinson" && s["interfaceScale"] == 150
        })
    });
}

#[gpui_kit::test]
fn with_sync_off_a_change_stays_on_this_device(cx: &mut TestAppContext) {
    let f = signed_in(cx, json!({"mode": "dark"}), json!([]));
    open_at(&f, cx, "appearance-sync");
    click(&f, cx, "appearance-sync-switch");
    wait(cx, "sync off", |cx| {
        !ircord_app::theme::appearance::sync(cx)
    });
    let saves = saved_appearances(&f).len();

    open_at(&f, cx, "appearance-mode");
    click(&f, cx, "appearance-mode-light");
    let light = look::scheme_tokens(look::scheme_for("ircord-light", Mode::Light), None);
    wait(cx, "the light scheme", |cx| {
        theme::get(cx).main == theme::hex(light.main) && !theme::get(cx).dark
    });
    cx.run_until_parked();
    assert_eq!(
        saved_appearances(&f).len(),
        saves,
        "nothing sent to the account"
    );
    let device: Device = cx.update(|cx| f.session.read(cx).config.section("appearance"));
    assert!(!device.sync);
    assert_eq!(
        device.settings.and_then(|s| s.mode),
        Some(ircord_client::contract::AppearanceMode::Light)
    );
}

#[gpui_kit::test]
fn a_personal_theme_is_used_and_another_imported(cx: &mut TestAppContext) {
    let sea = json!({
        "format": 1, "id": "t-sea", "name": "Sea", "updatedAt": "s1",
        "base": {"dark": "ircord-dark", "light": "ircord-light"},
        "colors": {"dark": {"canvas": "#0a1018", "surface": "#102030", "overlay": "#2a3a4a",
            "ink": "#e0f0ff", "muted": "#90a0b0", "accent": "#40c0ff",
            "palette": {"red": "#ff6060", "orange": "#ffa060", "yellow": "#ffd060",
                "green": "#60ff90", "cyan": "#60e0ff", "blue": "#6090ff",
                "purple": "#c090ff", "pink": "#ff90c0"}}},
        "css": ""
    });
    let f = signed_in(cx, json!({"mode": "dark"}), json!([sea]));
    open_at(&f, cx, "appearance-themes");
    click(&f, cx, "appearance-theme-use-t-sea");
    wait(cx, "the Sea theme", |cx| {
        theme::get(cx).main == theme::hex(0x102030)
    });
    wait(cx, "the theme saved as in use", |_| {
        saved_appearances(&f)
            .last()
            .is_some_and(|s| s["theme"] == "t-sea")
    });

    // Import a theme copied from elsewhere: it is added, not applied.
    let file = json!({"ircordTheme": 1, "name": "Copied", "base": {"dark": "nord-dark"},
        "colors": {}, "background": null, "css": "body { color: red }", "madeWith": "web"});
    cx.update(|cx| cx.write_to_clipboard(ClipboardItem::new_string(file.to_string())));
    click(&f, cx, "appearance-themes-import-paste");
    let saved = f
        .server
        .wait_for("PUT", "/api/v1/app/themes/*", crate::fixture::HANG_GUARD);
    let body = saved.json();
    assert_eq!(body["theme"]["name"], "Copied");
    assert_eq!(body["expected"], Value::Null);
    wait(cx, "the imported theme listed", |cx| {
        ircord_app::theme::appearance::themes(cx).len() == 2
    });
    // Still the Sea theme in use; the import is added and not applied.
    assert_eq!(colour(cx).main, theme::hex(0x102030));
    let notice = cx
        .update_window(f.window, |_, window, cx| {
            window.render_frame(cx);
            window
                .find("appearance-themes-notice")
                .label()
                .map(str::to_string)
        })
        .unwrap();
    assert!(notice.unwrap().contains("It is not applied"));
}

#[gpui_kit::test]
fn a_theme_css_sets_the_root_variables_until_the_switch_is_off(cx: &mut TestAppContext) {
    let red = json!({
        "format": 1, "id": "t-red", "name": "Red", "updatedAt": "s1",
        "base": {"dark": "ircord-dark", "light": "ircord-light"},
        "colors": {},
        "css": ":root { --accent: #ff0000; --rail: #00ff00 } .x { color: red }"
    });
    let f = signed_in(cx, json!({"mode": "dark"}), json!([red]));
    open_at(&f, cx, "appearance-themes");
    click(&f, cx, "appearance-theme-use-t-red");
    wait(cx, "the theme's accent from its CSS", |cx| {
        theme::get(cx).accent == theme::hex(0xff0000)
    });
    // The rule that restyles an element is counted, not applied.
    let report = cx
        .update_window(f.window, |_, window, cx| {
            window.render_frame(cx);
            window
                .find("appearance-themes-css-skipped")
                .label()
                .map(str::to_string)
        })
        .unwrap();
    assert!(report.unwrap().contains("not used"));
    // The rail is drawn in borders and backdrops, not in the default view.
    assert_eq!(colour(cx).rail, theme::hex(0x00ff00));

    // Switched off: the account is saved with CSS off and the accent is the scheme's.
    // The switch itself is not reported visible to the harness, so the setter it calls runs here.
    cx.update(|cx| ircord_app::theme::appearance::set_custom_css(cx, false));
    let saved = f
        .server
        .wait_for("PATCH", "/api/v1/app/themes", crate::fixture::HANG_GUARD);
    assert_eq!(saved.json()["customCss"], false);
    wait(cx, "the scheme's accent without the CSS", |cx| {
        theme::get(cx).accent != theme::hex(0xff0000)
    });
}

#[gpui_kit::test]
fn every_scheme_can_be_picked_and_reads_clearly(cx: &mut TestAppContext) {
    let f = signed_in(cx, json!({"mode": "dark"}), json!([]));
    open_at(&f, cx, "appearance-scheme");
    for family in look::FAMILIES {
        let Some(dark) = family.variants.iter().find(|s| s.mode == Mode::Dark) else {
            continue;
        };
        click(&f, cx, &format!("appearance-scheme-{}", family.id));
        let tokens = look::scheme_tokens(dark, None);
        wait(cx, family.id, |cx| {
            theme::get(cx).main == theme::hex(tokens.main)
        });
        assert!(
            look::contrast(tokens.text, tokens.main) >= 4.5,
            "{}",
            family.id
        );
    }
}

// ---- A theme's background picture ----

const PICTURE_ONE: &str = "/files/aaaaaaaa/bbbbbbbb/castle.png";
const PICTURE_TWO: &str = "/files/cccccccc/dddddddd/keep.png";

fn picture(fill: impl Fn(u32, u32) -> [u8; 4]) -> Vec<u8> {
    let image = image::RgbaImage::from_fn(8, 4, |x, y| image::Rgba(fill(x, y)));
    let mut bytes = Vec::new();
    image
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )
        .unwrap();
    bytes
}

/// The server takes images up to 1 MB, keeps one theme, and answers each
/// upload with the next picture's address.
fn serve_pictures(server: &CannedServer, themes: Vec<Value>) {
    let mut snapshot = crate::fixture::snapshot();
    snapshot.capabilities.upload = Some(ircord_client::contract::UploadCapability {
        max_bytes: 1_000_000,
        types: vec!["image/*".into()],
    });
    server.snapshot(&snapshot);
    let library = Arc::new(Mutex::new(themes));
    let document = |themes: &Vec<Value>| json!({"format": 1, "customCss": true, "themes": themes, "updatedAt": "l1"});
    let stored = library.clone();
    server.route_fn("GET", "/api/v1/app/themes", move |_| {
        Reply::json(200, document(&stored.lock().unwrap()))
    });
    server.route_fn("PUT", "/api/v1/app/themes/*", move |request| {
        let theme = request.json()["theme"].clone();
        let mut themes = library.lock().unwrap();
        themes.retain(|t| t["id"] != theme["id"]);
        themes.push(theme);
        Reply::json(200, document(&themes))
    });
    let uploads = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    server.route_fn("POST", "/api/v1/uploads", move |request| {
        assert_eq!(request.query_param("networkId").as_deref(), Some("n1"));
        assert_eq!(
            request.query_param("use"),
            None,
            "a theme picture stays on this server"
        );
        let n = uploads.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let url = [PICTURE_ONE, PICTURE_TWO][n];
        Reply::json(
            201,
            json!({"id": format!("u{n}"), "url": url, "name": "x.png", "size": 4,
                   "contentType": "image/png", "width": 8, "height": 4}),
        )
    });
    let (one, two) = (
        picture(|x, y| [(x * 30) as u8, (y * 60) as u8, 120, 255]),
        picture(|_, y| [10, (200 - y * 40) as u8, 30, 255]),
    );
    server.route_reply("GET", PICTURE_ONE, Reply::bytes(200, "image/png", one));
    server.route_reply("GET", PICTURE_TWO, Reply::bytes(200, "image/png", two));
}

/// Answer the open file prompt with `file`.
fn choose_file(cx: &mut TestAppContext, file: &std::path::Path) {
    cx.run_until_parked();
    assert!(cx.did_prompt_for_paths(), "the picker was asked");
    let file = file.to_path_buf();
    cx.simulate_path_prompt_response(move |_| Some(vec![file]));
}

fn shown(f: &Fixture, cx: &mut gpui_kit::App, id: &'static str) -> bool {
    f.window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            window
                .try_find(gpui_kit::ElementId::Name(id.into()))
                .is_some()
        })
        .unwrap_or(false)
}

fn present(f: &Fixture, cx: &mut TestAppContext, id: &str) -> bool {
    let id = gpui_kit::ElementId::Name(id.to_string().into());
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.try_find(id).is_some()
    })
    .unwrap()
}

fn saved_theme(f: &Fixture, n: usize) -> Value {
    f.server
        .wait_for_nth("PUT", "/api/v1/app/themes/*", n, crate::fixture::HANG_GUARD)
        .json()["theme"]
        .clone()
}

fn tones_of(bytes: &[u8]) -> Value {
    let decoded = image::load_from_memory(bytes).unwrap().to_rgba8();
    let small = image::imageops::resize(&decoded, 24, 24, image::imageops::FilterType::Triangle);
    json!(look::measure_tones(small.as_raw()).unwrap())
}

#[gpui_kit::test]
fn a_background_is_chosen_changed_and_removed_as_the_web_saves_it(cx: &mut TestAppContext) {
    let sea = json!({
        "format": 1, "id": "t-sea", "name": "Sea", "updatedAt": "s1",
        "base": {"dark": "ircord-dark", "light": "ircord-light"},
        "colors": {"dark": {"canvas": "#0a1018", "surface": "#102030", "overlay": "#2a3a4a",
            "ink": "#e0f0ff", "muted": "#90a0b0", "accent": "#40c0ff",
            "palette": {"red": "#ff6060", "orange": "#ffa060", "yellow": "#ffd060",
                "green": "#60ff90", "cyan": "#60e0ff", "blue": "#6090ff",
                "purple": "#c090ff", "pink": "#ff90c0"}}},
        "css": ""
    });
    let f = open_with(cx, true, {
        let sea = sea.clone();
        move |s| {
            serve(s, json!({"mode": "dark"}), json!([sea.clone()]));
            serve_pictures(s, vec![sea]);
        }
    });
    bootstrap(&f, cx);
    wait(cx, "the themes", |cx| {
        f.session
            .read(cx)
            .client()
            .unwrap()
            .read()
            .documents
            .themes
            .is_loaded()
    });
    let dir = std::env::temp_dir().join(format!(
        "ircord-background-{}",
        ircord_client::client::random_id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let (one, two) = (dir.join("castle.png"), dir.join("keep.png"));
    std::fs::write(
        &one,
        picture(|x, y| [(x * 30) as u8, (y * 60) as u8, 120, 255]),
    )
    .unwrap();
    std::fs::write(&two, picture(|_, y| [10, (200 - y * 40) as u8, 30, 255])).unwrap();

    // Choose: the picture is uploaded, and saved with the theme as the web
    // saves it, with the web's defaults for where it sits.
    open_at(&f, cx, "appearance-themes");
    click(&f, cx, "appearance-theme-edit-t-sea");
    assert!(present(&f, cx, "appearance-bg-choose"));
    assert!(
        !present(&f, cx, "appearance-bg-remove"),
        "nothing to remove yet"
    );
    click(&f, cx, "appearance-bg-choose");
    choose_file(cx, &one);
    wait(cx, "the picture in the draft", |cx| {
        shown(&f, cx, "appearance-bg-thumb")
    });
    click(&f, cx, "appearance-theme-save");
    let background = saved_theme(&f, 1)["background"].clone();
    assert_eq!(
        background,
        json!({"url": PICTURE_ONE, "placement": "conversation", "fit": "cover",
               "position": "center", "dim": 55, "blur": 0, "panelOpacity": 85,
               "tones": tones_of(&std::fs::read(&one).unwrap())})
    );

    // Change: another picture, shown behind the whole app, dimmed to the
    // end of its slider.
    wait(cx, "the editor closed", |cx| {
        !shown(&f, cx, "appearance-bg-choose")
    });
    click(&f, cx, "appearance-theme-edit-t-sea");
    wait(cx, "the saved picture shown", |cx| {
        shown(&f, cx, "appearance-bg-thumb")
    });
    click(&f, cx, "appearance-bg-choose");
    choose_file(cx, &two);
    wait(cx, "the second upload", |_| {
        f.server.requests_to("POST", "/api/v1/uploads").len() == 2
    });
    click(&f, cx, "appearance-bg-placement-app");
    click(&f, cx, "appearance-bg-fit-tile");
    click(&f, cx, "appearance-bg-position-top");
    let track = gpui_kit::ElementId::Name("appearance-bg-dim-track".into());
    reveal(&f, cx, &track);
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.click_at("appearance-bg-dim-track", point(px(215.), px(12.)), cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert!(
        present(&f, cx, "appearance-bg-panel-track"),
        "panels, behind the whole app"
    );
    click(&f, cx, "appearance-theme-save");
    let background = saved_theme(&f, 2)["background"].clone();
    assert_eq!(
        background,
        json!({"url": PICTURE_TWO, "placement": "app", "fit": "tile",
               "position": "top", "dim": 90, "blur": 0, "panelOpacity": 85,
               "tones": tones_of(&std::fs::read(&two).unwrap())})
    );

    // Remove: the theme is saved without a background.
    wait(cx, "the editor closed", |cx| {
        !shown(&f, cx, "appearance-bg-choose")
    });
    click(&f, cx, "appearance-theme-edit-t-sea");
    click(&f, cx, "appearance-bg-remove");
    assert!(!present(&f, cx, "appearance-bg-thumb"));
    click(&f, cx, "appearance-theme-save");
    assert!(saved_theme(&f, 3)["background"].is_null());
    let _ = std::fs::remove_dir_all(&dir);
}

#[gpui_kit::test]
fn a_server_without_uploads_gives_a_theme_no_background(cx: &mut TestAppContext) {
    let sea = json!({"format": 1, "id": "t-sea", "name": "Sea", "updatedAt": "s1",
        "base": {"dark": "ircord-dark", "light": "ircord-light"}, "colors": {}, "css": ""});
    let f = signed_in(cx, json!({"mode": "dark"}), json!([sea]));
    open_at(&f, cx, "appearance-themes");
    click(&f, cx, "appearance-theme-edit-t-sea");
    assert!(present(&f, cx, "appearance-bg-unavailable"));
    assert!(!present(&f, cx, "appearance-bg-choose"));
}

/// Two themes, the server holding uploads until `release` is sent, and a
/// folder with a picture and an SVG.
fn two_themes(
    cx: &mut TestAppContext,
    hold: bool,
) -> (
    Fixture,
    std::sync::mpsc::Sender<()>,
    std::path::PathBuf,
    std::path::PathBuf,
) {
    let theme = |id: &str, name: &str| {
        json!({"format": 1, "id": id, "name": name, "updatedAt": "s1",
            "base": {"dark": "ircord-dark", "light": "ircord-light"}, "colors": {}, "css": ""})
    };
    let (release, held) = crate::fixture::held_reply(|| {
        Reply::json(
            201,
            json!({"id": "u0", "url": PICTURE_ONE, "name": "x.png", "size": 4,
                   "contentType": "image/png", "width": 8, "height": 4}),
        )
    });
    let themes = vec![theme("t-sea", "Sea"), theme("t-sky", "Sky")];
    let f = open_with(cx, true, move |s| {
        serve(s, json!({"mode": "dark"}), json!([]));
        serve_pictures(s, themes);
        if hold {
            s.route_fn("POST", "/api/v1/uploads", held);
        }
    });
    bootstrap(&f, cx);
    wait(cx, "the themes", |cx| {
        let client = f.session.read(cx).client().unwrap().read();
        client.documents.themes.is_loaded()
    });
    let dir = std::env::temp_dir().join(format!(
        "ircord-background-{}",
        ircord_client::client::random_id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let (png, svg) = (dir.join("castle.png"), dir.join("logo.svg"));
    std::fs::write(
        &png,
        picture(|x, y| [(x * 30) as u8, (y * 60) as u8, 120, 255]),
    )
    .unwrap();
    std::fs::write(&svg, "<svg xmlns='http://www.w3.org/2000/svg'/>").unwrap();
    (f, release, png, svg)
}

#[gpui_kit::test]
fn a_picture_the_app_cannot_draw_is_refused_before_it_is_sent(cx: &mut TestAppContext) {
    let (f, _release, _png, svg) = two_themes(cx, false);
    open_at(&f, cx, "appearance-themes");
    click(&f, cx, "appearance-theme-edit-t-sea");
    click(&f, cx, "appearance-bg-choose");
    choose_file(cx, &svg);
    wait(cx, "the refusal", |cx| shown(&f, cx, "appearance-bg-error"));
    assert!(f.server.requests_to("POST", "/api/v1/uploads").is_empty());
    assert!(!present(&f, cx, "appearance-bg-remove"));
}

#[gpui_kit::test]
fn an_upload_that_finishes_after_the_editor_moved_on_is_dropped(cx: &mut TestAppContext) {
    let (f, release, png, _svg) = two_themes(cx, true);
    open_at(&f, cx, "appearance-themes");
    // Cancel while the upload is in flight, then edit the other theme.
    click(&f, cx, "appearance-theme-edit-t-sea");
    click(&f, cx, "appearance-bg-choose");
    choose_file(cx, &png);
    wait(cx, "the upload sent", |_| {
        !f.server.requests_to("POST", "/api/v1/uploads").is_empty()
    });
    click(&f, cx, "appearance-theme-cancel");
    click(&f, cx, "appearance-theme-edit-t-sky");
    release.send(()).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(500));
    cx.run_until_parked();
    assert!(
        !present(&f, cx, "appearance-bg-remove"),
        "no picture in Sky's editor"
    );
    assert!(!present(&f, cx, "appearance-bg-thumb"));
}

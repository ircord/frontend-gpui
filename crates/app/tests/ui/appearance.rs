//! Tests for package S1: Settings → Appearance, against the canned server.

use std::sync::{Arc, Mutex};

use gpui_kit::test::{TestEventExt, TestQueryExt, TestWindowExt};
use gpui_kit::{AppContext, ClipboardItem, MouseButton, TestAppContext, point, px};
use ircord_app::theme;
use ircord_client::store::appearance::{self as look, Device, Mode};
use ircord_client::testing::{CANNED_COOKIE, CannedServer, Reply};
use serde_json::{Value, json};

use crate::fixture::{Fixture, bootstrap, open_with, wait};

/// The account's appearance (`settings`) and theme library (`themes`); a
/// save answers what was sent, and a read after it answers the saved copy,
/// as the backend does.
fn serve(server: &CannedServer, settings: Value, themes: Value) -> Arc<Mutex<Value>> {
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
    let stored = library.clone();
    server.route_fn("PUT", "/api/v1/app/themes/*", move |request| {
        let mut library = stored.lock().unwrap();
        let body = request.json();
        let theme = body["theme"].clone();
        let themes = library["themes"].as_array_mut().unwrap();
        if let Some(existing) = themes.iter_mut().find(|item| item["id"] == theme["id"]) {
            if body["force"] != true && body["expected"] != existing["updatedAt"] {
                return Reply::json(412, json!({"data": {"theirs": existing}}));
            }
            *existing = theme;
        } else {
            themes.push(theme);
        }
        Reply::json(200, library.clone())
    });
    library
}

fn signed_in(cx: &mut TestAppContext, settings: Value, themes: Value) -> Fixture {
    let f = open_with(cx, true, move |s| {
        serve(s, settings, themes);
    });
    bootstrap(&f, cx);
    wait(cx, "the appearance and themes", |cx| {
        let session = f.session.read(cx);
        let client = session.client().unwrap().read();
        client.documents.appearance.is_loaded() && client.documents.themes.is_loaded()
    });
    f
}

#[gpui_kit::test]
fn an_active_synced_theme_loads_its_background_from_the_signed_in_server(cx: &mut TestAppContext) {
    const PATH: &str = "/files/wallpaper/token/blue.png";
    let f = open_with(cx, true, |server| {
        serve(
            server,
            json!({"mode": "dark", "theme": "t-background"}),
            json!([{
                "id": "t-background", "name": "Background", "format": 1,
                "background": {"url": PATH, "placement": "app", "fit": "tile", "dim": 65}
            }]),
        );
        let mut png = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            4,
            4,
            image::Rgba([20, 90, 200, 255]),
        ))
        .write_to(&mut png, image::ImageFormat::Png)
        .unwrap();
        let png = png.into_inner();
        server.route_fn("GET", PATH, move |request| {
            assert_eq!(request.header("cookie"), Some(CANNED_COOKIE));
            Reply::bytes(200, "image/png", png.clone())
        });
    });
    bootstrap(&f, cx);
    wait(cx, "the active theme's background is ready", |cx| {
        f.window
            .update(cx, |_, window, cx| window.render_frame(cx))
            .unwrap();
        theme::backdrop::get(cx).is_some()
    });
    cx.update(|cx| {
        let bg = theme::backdrop::get(cx).unwrap().spec;
        assert_eq!(bg.path, PATH);
        assert_eq!(bg.dim, 0.65);
    });
}

#[gpui_kit::test]
fn background_changes_stay_in_the_preview_until_the_theme_is_saved(cx: &mut TestAppContext) {
    use ircord_client::contract::{ThemeBackgroundFit, UploadCapability};

    const PHOTO: &str = "/files/a/b/replacement.png";
    let f = open_with(cx, true, |server| {
        let mut snapshot = crate::fixture::snapshot();
        snapshot.capabilities.upload = Some(UploadCapability {
            max_bytes: 1 << 20,
            types: vec!["image/png".into()],
        });
        server.snapshot(&snapshot);
        serve(
            server,
            json!({"mode": "dark", "theme": "t-bg"}),
            json!([{
                "id": "t-bg", "name": "Window", "format": 1, "updatedAt": "revision-1",
                "background": {"url": "/files/a/b/photo.png", "fit": "cover", "position": "center"}
            }]),
        );
        server.route("POST", "/api/v1/uploads", 201, json!({
            "id": "a", "url": PHOTO, "name": "replacement.png", "contentType": "image/png", "size": 64
        }));
    });
    bootstrap(&f, cx);
    wait(cx, "the personal theme is active", |cx| {
        theme::appearance::compiled(cx).backdrop.is_some()
    });
    let path = cx.update(|cx| f.session.read(cx).config.path().join("replacement.png"));
    image::RgbaImage::from_pixel(4, 4, image::Rgba([20, 90, 200, 255]))
        .save(&path)
        .unwrap();

    open_at(&f, cx, "appearance-themes");
    click(&f, cx, "appearance-theme-edit-t-bg");
    click(&f, cx, "appearance-theme-tab-background");
    click(&f, cx, "theme-background-choose");
    assert!(cx.did_prompt_for_paths());
    cx.simulate_path_prompt_response(move |_| Some(vec![path]));
    wait(cx, "the draft previews the uploaded image", |cx| {
        f.window
            .update(cx, |_, window, cx| window.render_frame(cx))
            .unwrap();
        !f.server.requests_to("GET", PHOTO).is_empty()
    });
    let upload = &f.server.requests_to("POST", "/api/v1/uploads")[0];
    assert_eq!(upload.query_param("networkId").as_deref(), Some("n1"));
    click(&f, cx, "theme-background-fit-tile");
    click(&f, cx, "theme-background-position-right");
    cx.update(|cx| {
        assert_eq!(
            theme::appearance::compiled(cx).backdrop.unwrap().fit,
            look::BackdropFit::Tile
        )
    });
    cx.update(|cx| {
        assert_eq!(
            theme::appearance::themes(cx)[0]
                .background
                .as_ref()
                .unwrap()
                .fit,
            Some(ThemeBackgroundFit::Cover)
        );
    });
    assert!(
        f.server
            .requests_to("PUT", "/api/v1/app/themes/t-bg")
            .is_empty()
    );

    click(&f, cx, "appearance-theme-save");
    wait(cx, "the saved background is active", |cx| {
        theme::appearance::themes(cx)[0]
            .background
            .as_ref()
            .unwrap()
            .fit
            == Some(ThemeBackgroundFit::Tile)
    });
    let body = f.server.requests_to("PUT", "/api/v1/app/themes/t-bg")[0].json();
    assert_eq!(body["theme"]["background"]["position"], "right");
    assert_eq!(body["theme"]["background"]["url"], PHOTO);
    assert_eq!(
        body["theme"]["background"]["tones"],
        json!(["#145ac8", "#145ac8"])
    );
    assert_eq!(body["expected"], "revision-1");
    assert_eq!(body["theme"]["colors"], Value::Null);
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

#[gpui_kit::test]
fn theme_cards_reflow_when_the_window_narrows(cx: &mut TestAppContext) {
    let f = signed_in(cx, json!({}), json!([]));
    open_at(&f, cx, "appearance-scheme");
    let window = f.window;
    let same_row = move |cx: &mut gpui_kit::App| {
        window
            .update(cx, |_, window, cx| {
                window.render_frame(cx);
                let first = window.find("appearance-scheme-ircord").bounds();
                let third = window.find("appearance-scheme-midnight").bounds();
                first.top() == third.top()
            })
            .unwrap()
    };
    wait(cx, "three themes fit on one row", same_row);
    cx.simulate_window_resize(f.window, gpui_kit::size(px(760.), px(800.)));
    wait(cx, "the themes wrap into more rows", |cx| !same_row(cx));
}

#[gpui_kit::test]
fn light_and_dark_variants_can_be_chosen_independently(cx: &mut TestAppContext) {
    let f = signed_in(
        cx,
        json!({"mode": "dark", "schemeDark": "github-dark", "schemeLight": "ircord-light"}),
        json!([]),
    );
    open_at(&f, cx, "appearance-scheme-variants");
    click(&f, cx, "appearance-light-variant");
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        let item = window.find_by_label("Hibiki Paper").bounds().center();
        window.pointer_down(item, MouseButton::Left, cx);
        window.pointer_up(item, MouseButton::Left, cx);
    })
    .unwrap();
    wait(cx, "the separate light palette was saved", |_| {
        saved_appearances(&f).last().is_some_and(|s| {
            s["mode"] == "dark"
                && s["schemeDark"] == "github-dark"
                && s["schemeLight"] == "hibiki-light"
        })
    });
}

/// Scroll the active settings or studio form until its control is in view.
fn reveal(f: &Fixture, cx: &mut TestAppContext, id: &str) {
    let footer = matches!(
        id,
        "appearance-theme-save"
            | "appearance-theme-cancel"
            | "theme-studio-compare"
            | "theme-studio-collapse"
            | "theme-studio-keep-mine"
            | "theme-studio-load-saved"
            | "theme-studio-restore"
    );
    let id = gpui_kit::ElementId::Name(id.to_string().into());
    for _ in 0..40 {
        let shown = cx
            .update_window(f.window, |_, window, cx| {
                window.render_frame(cx);
                let zone = if window.try_find("settings-content").is_some() {
                    "settings-content"
                } else {
                    "theme-studio-content"
                };
                let pane = window.try_find(zone);
                let target = window.try_find(id.clone());
                if target.as_ref().is_some_and(|e| {
                    let b = e.bounds();
                    e.visible()
                        && (footer
                            || pane.as_ref().is_some_and(|pane| {
                                b.top() >= pane.bounds().top()
                                    && b.bottom() <= pane.bounds().bottom()
                            }))
                }) {
                    return true;
                }
                if let Some(pane) = pane {
                    let direction = if target
                        .is_some_and(|target| target.bounds().top() < pane.bounds().top())
                    {
                        120.
                    } else {
                        -120.
                    };
                    window.scroll(
                        zone,
                        gpui_kit::ScrollDelta::Pixels(point(px(0.), px(direction))),
                        cx,
                    );
                }
                false
            })
            .unwrap();
        if shown {
            break;
        }
    }
}

fn click(f: &Fixture, cx: &mut TestAppContext, id: &str) {
    reveal(f, cx, id);
    let id = gpui_kit::ElementId::Name(id.to_string().into());
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
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        let sync = window.find("appearance-sync-switch");
        assert!(sync.bounds().bottom() <= window.find("settings-content").bounds().top());
        window.click("appearance-sync-switch", cx);
    })
    .unwrap();
    wait(cx, "sync off", |cx| {
        !ircord_app::theme::appearance::sync(cx)
    });
    let saves = saved_appearances(&f).len();

    open_at(&f, cx, "appearance-mode");
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("appearance-local-only").is_some());
        assert_eq!(window.find("appearance-mode-dark").selected(), Some(true));
    })
    .unwrap();
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
fn duplicating_a_theme_keeps_its_settings_without_applying_the_copy(cx: &mut TestAppContext) {
    let original = json!({
        "format": 1, "id": "t-original", "name": "Original", "updatedAt": "original-revision",
        "base": {"dark": "github-dark", "light": "hibiki-light"},
        "background": {"url": "/files/album/photo/sky.png", "placement": "app"},
        "css": ":root { --accent: #759aff }"
    });
    let f = signed_in(cx, json!({"mode": "dark"}), json!([original.clone()]));
    open_at(&f, cx, "appearance-themes");
    click(&f, cx, "appearance-theme-duplicate-t-original");
    wait(cx, "both themes exist in the library", |cx| {
        theme::appearance::themes(cx).len() == 2
    });
    let request = f.server.requests_to("PUT", "/api/v1/app/themes/*");
    let saved = request[0].json();
    assert_eq!(saved["expected"], Value::Null);
    assert_eq!(saved["theme"]["name"], "Copy of Original");
    assert_ne!(saved["theme"]["id"], original["id"]);
    for key in ["base", "background", "css"] {
        assert_eq!(saved["theme"][key], original[key]);
    }
    assert!(saved_appearances(&f).is_empty());
    cx.update(|cx| assert!(theme::appearance::compiled(cx).theme.is_none()));
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
    assert!(saved_appearances(&f).is_empty());
    trial_action(&f, cx, "theme-trial-keep");
    wait(cx, "the kept theme is saved", |_| {
        saved_appearances(&f)
            .last()
            .is_some_and(|settings| settings["theme"] == "t-red")
    });
    open_at(&f, cx, "appearance-themes");
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

    let saves = saved_appearances(&f).len();
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.press("shift-f8", cx);
    })
    .unwrap();
    wait(cx, "Shift+F8 pauses the theme", |cx| {
        theme::get(cx).accent != theme::hex(0xff0000)
    });
    trial_action(&f, cx, "theme-trial-resume");
    wait(cx, "the paused theme resumes", |cx| {
        theme::get(cx).accent == theme::hex(0xff0000)
    });
    assert_eq!(saved_appearances(&f).len(), saves);

    cx.simulate_window_resize(f.window, gpui_kit::size(px(880.), px(800.)));
    open_at(&f, cx, "appearance-themes");
    click(&f, cx, "appearance-themes-css-switch");
    let saved = f.server.wait_for(
        "PATCH",
        "/api/v1/app/themes",
        std::time::Duration::from_secs(5),
    );
    assert_eq!(saved.json()["customCss"], false);
    wait(cx, "the scheme's accent without the CSS", |cx| {
        theme::get(cx).accent != theme::hex(0xff0000)
    });
}

fn trial_action(f: &Fixture, cx: &mut TestAppContext, id: &'static str) {
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.click(id, cx);
    })
    .unwrap();
    cx.run_until_parked();
}

#[gpui_kit::test]
fn a_theme_trial_reverts_on_request_and_when_the_timer_expires(cx: &mut TestAppContext) {
    let f = signed_in(
        cx,
        json!({"mode": "dark"}),
        json!([{
            "format": 1, "id": "t-red", "name": "Red", "updatedAt": "s1",
            "css": ":root { --accent: #ff0000 }"
        }]),
    );
    let original = colour(cx).accent;

    for expire in [false, true] {
        open_at(&f, cx, "appearance-themes");
        click(&f, cx, "appearance-theme-use-t-red");
        wait(cx, "the new theme is on trial", |cx| {
            theme::get(cx).accent == theme::hex(0xff0000)
        });
        if expire {
            cx.executor()
                .advance_clock(std::time::Duration::from_secs(21));
        } else {
            trial_action(&f, cx, "theme-trial-revert");
        }
        wait(cx, "the previous appearance returns", |cx| {
            theme::get(cx).accent == original
        });
        assert!(saved_appearances(&f).is_empty());
    }
}

#[gpui_kit::test]
fn editing_the_active_themes_css_saves_only_after_keep(cx: &mut TestAppContext) {
    let f = signed_in(
        cx,
        json!({"mode": "dark", "theme": "t-custom"}),
        json!([{
            "format": 1, "id": "t-custom", "name": "Custom", "updatedAt": "revision-1",
            "css": ""
        }]),
    );
    wait(cx, "the old theme is active", |cx| {
        theme::appearance::compiled(cx).theme.as_deref() == Some("t-custom")
    });
    let original = colour(cx).accent;
    let css = ":root { --accent: #ff0000 }\n.message { color: red }";
    open_at(&f, cx, "appearance-themes");
    click(&f, cx, "appearance-theme-edit-t-custom");
    click(&f, cx, "appearance-theme-tab-css");
    click(&f, cx, "appearance-theme-css-editor");
    cx.update_window(f.window, |_, window, cx| window.input(css, cx))
        .unwrap();
    wait(cx, "the draft reports unsupported style rules", |cx| {
        f.window
            .update(cx, |_, window, cx| {
                window.render_frame(cx);
                window.try_find("appearance-draft-css-skipped").is_some()
            })
            .unwrap()
    });
    wait(cx, "the workspace shows the edit before saving", |cx| {
        theme::get(cx).accent == theme::hex(0xff0000)
    });
    assert!(
        f.server
            .requests_to("PUT", "/api/v1/app/themes/t-custom")
            .is_empty()
    );
    click(&f, cx, "theme-studio-compare");
    wait(cx, "Compare shows the built-in scheme", |cx| {
        theme::get(cx).accent == original
    });
    click(&f, cx, "theme-studio-compare");
    click(&f, cx, "appearance-theme-save");
    wait(cx, "the edit appears in a trial", |cx| {
        theme::get(cx).accent == theme::hex(0xff0000)
    });
    assert!(
        f.server
            .requests_to("PUT", "/api/v1/app/themes/t-custom")
            .is_empty()
    );
    trial_action(&f, cx, "theme-trial-revert");
    wait(cx, "the saved theme is restored", |cx| {
        theme::get(cx).accent == original
    });
    assert!(
        f.server
            .requests_to("PUT", "/api/v1/app/themes/t-custom")
            .is_empty()
    );

    open_at(&f, cx, "appearance-themes");
    click(&f, cx, "appearance-theme-edit-t-custom");
    click(&f, cx, "appearance-theme-save");
    trial_action(&f, cx, "theme-trial-keep");
    wait(cx, "the kept edit is active", |cx| {
        theme::get(cx).accent == theme::hex(0xff0000)
    });
    let body = f.server.requests_to("PUT", "/api/v1/app/themes/t-custom")[0].json();
    assert_eq!(body["expected"], "revision-1");
    assert_eq!(body["theme"]["css"], css);
}

#[gpui_kit::test]
fn theme_studio_docks_and_preview_mode_does_not_change_the_account(cx: &mut TestAppContext) {
    let f = signed_in(cx, json!({"mode": "dark"}), json!([]));
    open_at(&f, cx, "appearance-themes");
    click(&f, cx, "appearance-themes-new");
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        let studio = window.find("theme-studio").bounds();
        let timeline = window.find("timeline").bounds();
        assert!(timeline.right() <= studio.left());
        assert!(window.try_find("settings-content").is_none());
    })
    .unwrap();
    click(&f, cx, "theme-studio-light");
    wait(cx, "the live workspace previews light mode", |cx| {
        !theme::get(cx).dark
    });
    assert!(saved_appearances(&f).is_empty());
    assert!(
        f.server
            .requests_to("PUT", "/api/v1/app/themes/*")
            .is_empty()
    );
    cx.simulate_window_resize(f.window, gpui_kit::size(px(640.), px(800.)));
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        let studio = window.find("theme-studio").bounds();
        assert_eq!(studio.size.width, px(640.));
        assert_eq!(studio.bottom(), px(800.));
    })
    .unwrap();
    click(&f, cx, "appearance-theme-cancel");
    wait(cx, "discard restores dark mode", |cx| theme::get(cx).dark);
    assert!(saved_appearances(&f).is_empty());
}

#[gpui_kit::test]
fn theme_studio_saves_separate_base_schemes_and_keeps_the_other_modes_colors(
    cx: &mut TestAppContext,
) {
    let f = signed_in(
        cx,
        json!({"mode": "dark"}),
        json!([{
            "id": "t-preview", "format": 1, "name": "Preview", "updatedAt": "revision-1"
        }]),
    );
    open_at(&f, cx, "appearance-themes");
    click(&f, cx, "appearance-theme-edit-t-preview");
    pick_studio_base(&f, cx, "Hibiki Stage");
    wait(cx, "the dark base is previewed", |cx| {
        theme::appearance::compiled(cx).scheme == "hibiki-dark"
    });
    click(&f, cx, "theme-studio-customize");
    click(&f, cx, "appearance-seed-blue");
    cx.update_window(f.window, |_, window, cx| {
        window.press(
            if cfg!(target_os = "macos") {
                "cmd-a"
            } else {
                "ctrl-a"
            },
            cx,
        );
        window.input("#468def", cx);
    })
    .unwrap();
    click(&f, cx, "theme-studio-light");
    pick_studio_base(&f, cx, "Hibiki Paper");
    wait(cx, "the light base is previewed", |cx| {
        theme::appearance::compiled(cx).scheme == "hibiki-light"
    });
    click(&f, cx, "theme-studio-customize");
    click(&f, cx, "theme-studio-reset-colors");
    assert!(
        f.server
            .requests_to("PUT", "/api/v1/app/themes/t-preview")
            .is_empty()
    );
    click(&f, cx, "appearance-theme-save");
    wait(cx, "the edited theme is saved", |_| {
        !f.server
            .requests_to("PUT", "/api/v1/app/themes/t-preview")
            .is_empty()
    });
    let body = f.server.requests_to("PUT", "/api/v1/app/themes/t-preview")[0].json();
    assert_eq!(
        body["theme"]["base"],
        json!({"dark": "hibiki-dark", "light": "hibiki-light"})
    );
    assert_eq!(
        body["theme"]["colors"]["dark"]["palette"]["blue"],
        "#468def"
    );
    assert_eq!(body["theme"]["colors"]["light"], Value::Null);
    assert!(saved_appearances(&f).is_empty());
}

fn pick_studio_base(f: &Fixture, cx: &mut TestAppContext, name: &str) {
    click(f, cx, "theme-studio-base");
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        let item = window.find_by_label(name).bounds().center();
        window.pointer_down(item, MouseButton::Left, cx);
        window.pointer_up(item, MouseButton::Left, cx);
    })
    .unwrap();
}

fn draft_store(f: &Fixture, cx: &mut TestAppContext) -> look::drafts::Store {
    cx.update(|cx| {
        let session = f.session.read(cx);
        let client = session.client().unwrap();
        look::drafts::Store::new(
            &session.config,
            client.api().server(),
            &client.read().user.id,
        )
        .unwrap()
    })
}

#[gpui_kit::test]
fn a_new_theme_draft_survives_the_editor_reopening(cx: &mut TestAppContext) {
    let f = signed_in(cx, json!({"mode": "dark"}), json!([]));
    let store = draft_store(&f, cx);
    open_at(&f, cx, "appearance-themes");
    click(&f, cx, "appearance-themes-new");
    click(&f, cx, "appearance-themes-name");
    cx.update_window(f.window, |_, window, cx| {
        window.press(
            if cfg!(target_os = "macos") {
                "cmd-a"
            } else {
                "ctrl-a"
            },
            cx,
        );
        window.input("Night sky", cx);
    })
    .unwrap();
    wait(cx, "the new theme draft on this device", |_| {
        store
            .get()
            .is_some_and(|draft| draft.theme.name.as_deref() == Some("Night sky"))
    });
    let draft = store.get().unwrap();
    assert!(draft.new_theme);
    let id = draft.theme.id.unwrap();

    let server = f.server.address();
    f.session.update(cx, move |session, cx| {
        session.start(server, CANNED_COOKIE.into(), cx)
    });
    bootstrap(&f, cx);
    wait(cx, "the reconnected theme library", |cx| {
        theme::appearance::library(cx).is_some()
    });
    open_at(&f, cx, "appearance-themes");
    click(&f, cx, "appearance-themes-new");
    click(&f, cx, "theme-studio-restore");
    click(&f, cx, "appearance-theme-save");
    wait(
        cx,
        "the recovered theme is saved and the draft cleared",
        |cx| {
            theme::appearance::themes(cx)
                .iter()
                .any(|theme| theme.id.as_deref() == Some(&id))
                && store.get().is_none()
        },
    );
    let sent = f
        .server
        .requests_to("PUT", &format!("/api/v1/app/themes/{id}"));
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].json()["theme"]["name"], "Night sky");
    assert!(sent[0].json()["expected"].is_null());
}

#[gpui_kit::test]
fn restoring_an_existing_theme_keeps_the_drafts_original_revision(cx: &mut TestAppContext) {
    let f = signed_in(
        cx,
        json!({"mode": "dark"}),
        json!([{
            "id": "t-restored", "format": 1, "name": "Saved elsewhere", "updatedAt": "r2"
        }]),
    );
    let store = draft_store(&f, cx);
    store
        .put(look::drafts::Draft {
            theme: serde_json::from_value(json!({
                "id": "t-restored", "format": 1, "name": "Recovered", "updatedAt": "r1",
                "base": {"light": "hibiki-light"}, "css": ":root { --accent: #aaccee }"
            }))
            .unwrap(),
            mode: Mode::Light,
            new_theme: false,
        })
        .unwrap();

    open_at(&f, cx, "appearance-themes");
    click(&f, cx, "appearance-theme-edit-t-restored");
    click(&f, cx, "theme-studio-restore");
    wait(cx, "the recovered light palette", |cx| {
        theme::appearance::compiled(cx).scheme == "hibiki-light"
    });
    click(&f, cx, "appearance-theme-save");
    wait(cx, "the stale draft is refused", |cx| {
        f.window
            .update(cx, |_, window, cx| {
                window.render_frame(cx);
                window.try_find("theme-studio-conflict").is_some()
            })
            .unwrap()
    });
    let sent = f.server.requests_to("PUT", "/api/v1/app/themes/t-restored");
    assert_eq!(sent.len(), 1);
    let request = sent[0].json();
    assert_eq!(request["expected"], "r1");
    assert_eq!(request["theme"]["name"], "Recovered");
    assert_eq!(request["theme"]["css"], ":root { --accent: #aaccee }");
    assert!(request["force"].is_null());
    click(&f, cx, "appearance-theme-cancel");
    assert!(store.get().is_none());
}

fn conflicted_theme(cx: &mut TestAppContext) -> Fixture {
    let mut library = None;
    let f = open_with(cx, true, |server| {
        library = Some(serve(
            server,
            json!({"mode": "dark"}),
            json!([{"id": "t-conflict", "format": 1, "name": "Original", "updatedAt": "r1"}]),
        ));
    });
    bootstrap(&f, cx);
    wait(cx, "the original theme", |cx| {
        theme::appearance::themes(cx).len() == 1
    });
    open_at(&f, cx, "appearance-themes");
    click(&f, cx, "appearance-theme-edit-t-conflict");
    pick_studio_base(&f, cx, "Hibiki Stage");
    library.unwrap().lock().unwrap()["themes"][0] = json!({
        "id": "t-conflict", "format": 1, "name": "Saved elsewhere", "updatedAt": "r2",
        "base": {"dark": "github-dark"}
    });
    click(&f, cx, "appearance-theme-save");
    wait(cx, "the conflict choices", |cx| {
        f.window
            .update(cx, |_, window, cx| {
                window.render_frame(cx);
                window.try_find("theme-studio-conflict").is_some()
            })
            .unwrap()
    });
    assert_eq!(
        f.server.requests_to("PUT", "/api/v1/app/themes/t-conflict")[0].json()["expected"],
        "r1"
    );
    f
}

#[gpui_kit::test]
fn theme_studio_keeps_the_local_draft_when_confirming_a_conflict(cx: &mut TestAppContext) {
    let f = conflicted_theme(cx);
    cx.update(|cx| assert_eq!(theme::appearance::compiled(cx).scheme, "hibiki-dark"));
    click(&f, cx, "theme-studio-keep-mine");
    wait(cx, "the local theme was saved", |cx| {
        theme::appearance::themes(cx).iter().any(|theme| {
            theme.name.as_deref() == Some("Original")
                && theme.base.as_ref().and_then(|base| base.dark.as_deref()) == Some("hibiki-dark")
        })
    });
    let updates = f.server.requests_to("PUT", "/api/v1/app/themes/t-conflict");
    let body = updates[1].json();
    assert_eq!(body["force"], true);
    assert_eq!(body["theme"]["name"], "Original");
    assert_eq!(body["theme"]["base"]["dark"], "hibiki-dark");
}

#[gpui_kit::test]
fn theme_studio_loads_the_saved_version_and_uses_its_revision(cx: &mut TestAppContext) {
    let f = conflicted_theme(cx);
    click(&f, cx, "theme-studio-load-saved");
    wait(cx, "the saved palette is previewed", |cx| {
        theme::appearance::compiled(cx).scheme == "github-dark"
    });
    assert_eq!(
        f.server
            .requests_to("PUT", "/api/v1/app/themes/t-conflict")
            .len(),
        1
    );
    click(&f, cx, "appearance-theme-save");
    wait(cx, "the save uses the latest revision", |_| {
        f.server
            .requests_to("PUT", "/api/v1/app/themes/t-conflict")
            .len()
            == 2
    });
    let updates = f.server.requests_to("PUT", "/api/v1/app/themes/t-conflict");
    let body = updates[1].json();
    assert_eq!(body["expected"], "r2");
    assert_eq!(body["force"], Value::Null);
    assert_eq!(body["theme"]["name"], "Saved elsewhere");
    assert_eq!(body["theme"]["base"]["dark"], "github-dark");
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
    click(&f, cx, "appearance-theme-tab-background");
    assert!(present(&f, cx, "theme-background-choose"));
    assert!(
        !present(&f, cx, "theme-background-remove"),
        "nothing to remove yet"
    );
    click(&f, cx, "theme-background-choose");
    choose_file(cx, &one);
    wait(cx, "the picture in the draft", |cx| {
        shown(&f, cx, "theme-background-preview")
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
    wait(cx, "the editor closed", |cx| !shown(&f, cx, "theme-studio"));
    open_at(&f, cx, "appearance-themes");
    click(&f, cx, "appearance-theme-edit-t-sea");
    click(&f, cx, "appearance-theme-tab-background");
    wait(cx, "the saved picture shown", |cx| {
        shown(&f, cx, "theme-background-preview")
    });
    click(&f, cx, "theme-background-choose");
    choose_file(cx, &two);
    wait(cx, "the second picture in the draft", |cx| {
        theme::appearance::compiled(cx)
            .backdrop
            .is_some_and(|spec| spec.path == PICTURE_TWO)
    });
    click(&f, cx, "theme-background-placement-app");
    click(&f, cx, "theme-background-fit-tile");
    click(&f, cx, "theme-background-position-top");
    let track = "theme-background-dim-track";
    reveal(&f, cx, track);
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        let width = window.find(track).bounds().size.width;
        window.click_at(track, point(width - px(5.), px(12.)), cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert!(
        present(&f, cx, "theme-background-panels-track"),
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
    wait(cx, "the editor closed", |cx| !shown(&f, cx, "theme-studio"));
    open_at(&f, cx, "appearance-themes");
    click(&f, cx, "appearance-theme-edit-t-sea");
    click(&f, cx, "appearance-theme-tab-background");
    click(&f, cx, "theme-background-remove");
    assert!(!present(&f, cx, "theme-background-preview"));
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
    click(&f, cx, "appearance-theme-tab-background");
    assert!(present(&f, cx, "theme-background-unavailable"));
    assert!(!present(&f, cx, "theme-background-choose"));
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
    click(&f, cx, "appearance-theme-tab-background");
    click(&f, cx, "theme-background-choose");
    choose_file(cx, &svg);
    wait(cx, "the refusal", |cx| {
        shown(&f, cx, "appearance-themes-notice")
    });
    assert!(f.server.requests_to("POST", "/api/v1/uploads").is_empty());
    assert!(!present(&f, cx, "theme-background-remove"));
}

#[gpui_kit::test]
fn an_upload_that_finishes_after_the_editor_moved_on_is_dropped(cx: &mut TestAppContext) {
    let (f, release, png, _svg) = two_themes(cx, true);
    open_at(&f, cx, "appearance-themes");
    // Cancel while the upload is in flight, then edit the other theme.
    click(&f, cx, "appearance-theme-edit-t-sea");
    click(&f, cx, "appearance-theme-tab-background");
    click(&f, cx, "theme-background-choose");
    choose_file(cx, &png);
    wait(cx, "the upload sent", |_| {
        !f.server.requests_to("POST", "/api/v1/uploads").is_empty()
    });
    click(&f, cx, "appearance-theme-cancel");
    open_at(&f, cx, "appearance-themes");
    click(&f, cx, "appearance-theme-edit-t-sky");
    click(&f, cx, "appearance-theme-tab-background");
    release.send(()).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(500));
    cx.run_until_parked();
    assert!(
        !present(&f, cx, "theme-background-remove"),
        "no picture in Sky's editor"
    );
    assert!(!present(&f, cx, "theme-background-preview"));
}

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
        let themes = library["themes"].as_array_mut().unwrap();
        if let Some(existing) = themes.iter_mut().find(|item| item["id"] == theme["id"]) {
            *existing = theme;
        } else {
            themes.push(theme);
        }
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
    wait(cx, "the active theme's background request", |cx| {
        f.window
            .update(cx, |_, window, cx| window.render_frame(cx))
            .unwrap();
        !f.server.requests_to("GET", PATH).is_empty()
    });
    cx.update(|cx| {
        let bg = theme::get(cx).background.as_ref().unwrap();
        assert_eq!(bg.path, PATH);
        assert_eq!(bg.dim, 65);
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
        theme::get(cx).background.is_some()
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
            theme::get(cx).background.as_ref().unwrap().fit,
            ThemeBackgroundFit::Cover
        )
    });
    assert!(
        f.server
            .requests_to("PUT", "/api/v1/app/themes/t-bg")
            .is_empty()
    );

    click(&f, cx, "appearance-theme-save");
    wait(cx, "the saved background is active", |cx| {
        theme::get(cx)
            .background
            .as_ref()
            .is_some_and(|bg| bg.fit == ThemeBackgroundFit::Tile)
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

/// Click `id`, scrolling the settings page down until it is wholly in view.
fn click(f: &Fixture, cx: &mut TestAppContext, id: &str) {
    let id = gpui_kit::ElementId::Name(id.to_string().into());
    for _ in 0..40 {
        let shown = cx
            .update_window(f.window, |_, window, cx| {
                window.render_frame(cx);
                let pane = window.find("settings-content").bounds();
                window.try_find(id.clone()).is_some_and(|e| {
                    let b = e.bounds();
                    e.visible() && b.top() >= pane.top() && b.bottom() <= pane.bottom()
                })
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
                    gpui_kit::px(-120.),
                )),
                cx,
            )
        })
        .unwrap();
    }
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.click(id, cx);
    })
    .unwrap();
    cx.run_until_parked();
}

fn colour(cx: &mut TestAppContext) -> theme::Theme {
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
    let saved = f.server.wait_for(
        "PUT",
        "/api/v1/app/themes/*",
        std::time::Duration::from_secs(5),
    );
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

    // Switched off: the account is saved with CSS off and the accent is the scheme's.
    // The switch itself is not reported visible to the harness, so the setter it calls runs here.
    cx.update(|cx| ircord_app::theme::appearance::set_custom_css(cx, false));
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

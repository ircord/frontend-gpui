//! Tests for package S1: Settings → Appearance, against the canned server.

use gpui_kit::test::TestWindowExt;
use gpui_kit::{AppContext, ClipboardItem, TestAppContext};
use ircord_app::theme;
use ircord_client::store::appearance::{self as look, Device, Mode};
use ircord_client::testing::{CannedServer, Reply};
use serde_json::{Value, json};

use crate::fixture::{Fixture, bootstrap, open_with, wait};

/// The account's appearance (`settings`) and theme library (`themes`); a
/// save answers what was sent.
fn serve(server: &CannedServer, settings: Value, themes: Value) {
    server.route(
        "GET",
        "/api/v1/app/appearance",
        200,
        json!({"settings": settings, "updatedAt": "r1"}),
    );
    server.route_fn("PUT", "/api/v1/app/appearance", |request| {
        let mut body = request.json();
        body["updatedAt"] = json!("r2");
        Reply::json(200, body)
    });
    let library = json!({"format": 1, "customCss": true, "themes": themes, "updatedAt": "l1"});
    server.route("GET", "/api/v1/app/themes", 200, library.clone());
    server.route_fn("PUT", "/api/v1/app/themes/*", move |request| {
        let mut library = library.clone();
        let theme = request.json()["theme"].clone();
        library["themes"].as_array_mut().unwrap().push(theme);
        Reply::json(200, library)
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

fn click(f: &Fixture, cx: &mut TestAppContext, id: &str) {
    let id = id.to_string();
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.click(gpui_kit::ElementId::Name(id.into()), cx);
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

    open_at(&f, cx, "appearance-scale");
    click(&f, cx, "appearance-scale-up");
    wait(cx, "105 %", |cx| theme::get(cx).scale == 1.05);
    let label = cx
        .update_window(f.window, |_, window, cx| {
            window.render_frame(cx);
            window
                .find("appearance-scale-value")
                .label()
                .map(str::to_string)
        })
        .unwrap();
    assert_eq!(label.as_deref(), Some("105 %"));

    wait(cx, "every change saved", |_| {
        saved_appearances(&f).last().is_some_and(|s| {
            s["accent"] == "lime" && s["font"] == "atkinson" && s["interfaceScale"] == 105
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
    // Still the Sea theme in use; the import says its CSS is not applied.
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
    assert!(notice.unwrap().contains("cannot apply CSS"));
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

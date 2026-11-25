//! A theme change fades over 200 ms (`theme::fade`), and snaps with reduced motion.

use std::sync::{Arc, Mutex};

use gpui_kit::{AppContext, Rgba, TestAppContext};
use ircord_app::theme::{self, fade};
use ircord_client::testing::{CannedServer, Reply};
use serde_json::{Value, json};

use crate::fixture::{Fixture, bootstrap, open_with, wait};

fn serve(server: &CannedServer, settings: Value) {
    let stored = Arc::new(Mutex::new(json!({"settings": settings, "updatedAt": "r1"})));
    let put = stored.clone();
    server.route_fn("GET", "/api/v1/app/appearance", move |_| {
        Reply::json(200, stored.lock().unwrap().clone())
    });
    server.route_fn("PUT", "/api/v1/app/appearance", move |request| {
        let mut body = request.json();
        body["updatedAt"] = json!("r2");
        *put.lock().unwrap() = body.clone();
        Reply::json(200, body)
    });
    server.route_fn("GET", "/api/v1/app/themes", |_| {
        Reply::json(
            200,
            json!({"format": 1, "customCss": true, "themes": [], "updatedAt": "l1"}),
        )
    });
}

fn signed_in(cx: &mut TestAppContext, settings: Value) -> Fixture {
    let f = open_with(cx, true, move |s| serve(s, settings));
    bootstrap(&f, cx);
    wait(cx, "the appearance", |cx| {
        let session = f.session.read(cx);
        let client = session.client().unwrap().read();
        client.documents.appearance.is_loaded() && client.documents.themes.is_loaded()
    });
    // The account's own look has arrived and settled.
    wait(cx, "no fade", |cx| !fade::active(cx));
    f
}

fn to_light(cx: &mut TestAppContext) {
    cx.update(|cx| {
        theme::appearance::edit(cx, |a| {
            a.mode = Some(ircord_client::contract::AppearanceMode::Light)
        });
    });
}

#[gpui_kit::test]
fn a_change_of_colours_fades_and_ends_on_the_new_ones(cx: &mut TestAppContext) {
    let _f = signed_in(cx, json!({"mode": "dark"}));
    let (dark, light) = (theme::Theme::dark(), theme::Theme::light());
    assert_eq!(cx.update(|cx| theme::get(cx).main), dark.main);

    to_light(cx);
    // Started: the colours are still (nearly) the old ones, the fade is
    // running, and what is costly to redraw already has the final colours.
    cx.update(|cx| {
        assert!(fade::active(cx), "a fade runs");
        assert_eq!(theme::settled(cx).main, light.main);
        assert_ne!(theme::get(cx).main, light.main);
        assert!(
            !theme::get(cx).dark,
            "non-colour tokens change at the start"
        );
    });
    // Ended: the new colours, and nothing runs any more.
    wait(cx, "the fade ends", |cx| !fade::active(cx));
    cx.update(|cx| {
        assert_eq!(theme::get(cx).main, light.main);
        assert_eq!(*theme::get(cx), *theme::settled(cx));
    });
}

#[gpui_kit::test]
fn partway_the_colours_are_between_the_old_and_the_new(cx: &mut TestAppContext) {
    let _f = signed_in(cx, json!({"mode": "dark"}));
    let (dark, light) = (theme::Theme::dark(), theme::Theme::light());
    to_light(cx);
    // A frame visibly between the two, not the first one that is the old
    // colour to within rounding.
    wait(cx, "a frame between", |cx| {
        let (m, d, l) = (
            Rgba::from(theme::get(cx).main),
            Rgba::from(dark.main),
            Rgba::from(light.main),
        );
        fade::active(cx) && (m.r - d.r).abs() > 0.05 && (m.r - l.r).abs() > 0.05
    });
    wait(cx, "the fade ends", |cx| !fade::active(cx));
}

#[gpui_kit::test]
fn with_reduced_motion_the_change_is_at_once(cx: &mut TestAppContext) {
    let _f = signed_in(cx, json!({"mode": "dark", "motion": "reduced"}));
    let light = theme::Theme::light();
    to_light(cx);
    cx.update(|cx| {
        assert!(!fade::active(cx), "no fade");
        assert_eq!(theme::get(cx).main, light.main);
    });
}

#[gpui_kit::test]
fn a_change_with_reduced_motion_ends_a_fade_already_running(cx: &mut TestAppContext) {
    let _f = signed_in(cx, json!({"mode": "dark"}));
    to_light(cx);
    cx.update(|cx| assert!(fade::active(cx)));
    cx.update(|cx| {
        theme::appearance::edit(cx, |a| {
            a.motion = Some(ircord_client::contract::AppearanceMotion::Reduced)
        });
    });
    cx.update(|cx| {
        assert!(!fade::active(cx), "the fade stopped");
        assert!(theme::get(cx).reduced_motion);
    });
    // And the stopped fade does not draw a later frame over the new tokens.
    std::thread::sleep(std::time::Duration::from_millis(300));
    cx.run_until_parked();
    cx.update(|cx| {
        assert!(theme::get(cx).reduced_motion);
        assert_eq!(*theme::get(cx), *theme::settled(cx));
    });
}

fn brightness(cx: &mut TestAppContext) -> f32 {
    cx.update(|cx| {
        let m = Rgba::from(theme::get(cx).main);
        m.r + m.g + m.b
    })
}

#[gpui_kit::test]
fn a_change_mid_fade_takes_over_and_the_old_fade_draws_nothing_more(cx: &mut TestAppContext) {
    let _f = signed_in(cx, json!({"mode": "dark"}));
    to_light(cx);
    cx.update(|cx| assert!(fade::active(cx)));
    // Back to a dark scheme while the fade to light runs.
    cx.update(|cx| {
        theme::appearance::edit(cx, |a| {
            a.mode = Some(ircord_client::contract::AppearanceMode::Dark);
            a.scheme_dark = Some("nord-dark".into());
        });
    });
    let nord = cx.update(|cx| theme::settled(cx).clone());
    let nord_brightness = {
        let m = Rgba::from(nord.main);
        m.r + m.g + m.b
    };
    // No later frame heads for light: every one is at most as bright as the
    // colours at the takeover (light's end would be far above Nord).
    let started = std::time::Instant::now();
    while cx.update(|cx| fade::active(cx)) && started.elapsed() < std::time::Duration::from_secs(2)
    {
        assert!(
            brightness(cx) <= nord_brightness + 1.0,
            "a frame of the old fade drew"
        );
        std::thread::sleep(std::time::Duration::from_millis(4));
        cx.run_until_parked();
    }
    wait(cx, "the fade ends", |cx| !fade::active(cx));
    std::thread::sleep(std::time::Duration::from_millis(300));
    cx.run_until_parked();
    cx.update(|cx| {
        assert_eq!(theme::get(cx).main, nord.main);
        assert!(theme::get(cx).dark);
    });
}

#[gpui_kit::test]
fn a_change_that_moves_no_colour_does_not_fade(cx: &mut TestAppContext) {
    let _f = signed_in(cx, json!({"mode": "dark"}));
    cx.update(|cx| {
        theme::appearance::edit(cx, |a| {
            a.font = Some(ircord_client::contract::AppearanceFont::Inter)
        });
    });
    cx.update(|cx| {
        assert!(!fade::active(cx), "nothing to fade");
        assert_eq!(theme::get(cx).font.as_ref(), "Inter");
    });
}

#[gpui_kit::test]
fn at_start_up_the_stored_look_shows_at_once(cx: &mut TestAppContext) {
    use ircord_client::config::ConfigDir;
    use ircord_client::contract::{Appearance, AppearanceMode};
    use ircord_client::credentials::{CredentialStore, MemoryStore};
    use ircord_client::store::appearance::Device;

    // The production order: the session, then the window, in one update, so
    // the stored look is applied with a window already open.
    let dir = std::env::temp_dir().join(format!(
        "ircord-fade-{}",
        ircord_client::client::random_id()
    ));
    let config = ConfigDir::at(dir.join("config"));
    config
        .save_section(
            "appearance",
            &Device {
                sync: false,
                settings: Some(Appearance {
                    mode: Some(AppearanceMode::Dark),
                    ..Default::default()
                }),
            },
        )
        .unwrap();
    let runtime = ircord_client::client::runtime().unwrap();
    let credentials: std::sync::Arc<dyn CredentialStore> =
        std::sync::Arc::new(MemoryStore::default());
    cx.executor().allow_parking();
    cx.update(|cx| {
        ircord_app::notify::identify(cx);
        gpui_kit::init(cx);
        ircord_app::init(cx);
        let session =
            cx.new(|cx| ircord_app::session::Session::new(runtime, config, credentials, cx));
        ircord_app::open_main_window(session, cx);
    });
    cx.update(|cx| {
        assert!(!fade::active(cx), "no fade at start-up");
        assert_eq!(theme::get(cx).main, theme::Theme::dark().main);
    });
    let _ = std::fs::remove_dir_all(&dir);
}

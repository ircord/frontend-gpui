//! A theme change fades over 200 ms (`theme::fade`), and snaps with reduced motion.

use std::sync::{Arc, Mutex};

use gpui_kit::TestAppContext;
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
    wait(cx, "a frame between", |cx| {
        let main = theme::get(cx).main;
        fade::active(cx) && main != dark.main && main != light.main
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
    cx.update(|cx| assert_eq!(theme::get(cx).main, theme::Theme::light().main));
}

//! Ircoid, the game behind About's logo: the door, the dialog and the game.

use gpui_kit::test::TestWindowExt;
use gpui_kit::{AppContext, TestAppContext};
use ircord_app::route::Route;

use crate::fixture::{Fixture, bootstrap, open, wait};

/// Draw the window and click the element with `id`.
fn click(f: &Fixture, cx: &mut TestAppContext, id: &'static str) {
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.click(id, cx);
    })
    .unwrap();
    cx.run_until_parked();
}

fn press(f: &Fixture, cx: &mut TestAppContext, key: &str) {
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.press(key, cx);
    })
    .unwrap();
    cx.run_until_parked();
}

fn shown(f: &Fixture, cx: &mut TestAppContext, id: &'static str) -> bool {
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.try_find(id).is_some()
    })
    .unwrap()
}

fn shows(f: &Fixture, cx: &mut TestAppContext, id: &'static str) {
    let window = f.window;
    wait(cx, id, move |cx| {
        window
            .update(cx, |_, window, cx| {
                window.render_frame(cx);
                window.try_find(id).is_some()
            })
            .unwrap_or(false)
    });
}

/// Settings → About, signed in.
pub(crate) fn about(cx: &mut TestAppContext) -> Fixture {
    let f = open(cx, true);
    bootstrap(&f, cx);
    click(&f, cx, "open-settings");
    shows(&f, cx, "settings-section-about");
    click(&f, cx, "settings-section-about");
    shows(&f, cx, "about-logo");
    f
}

/// About with the game open.
pub(crate) fn game(cx: &mut TestAppContext) -> Fixture {
    let f = about(cx);
    for _ in 0..5 {
        click(&f, cx, "about-logo");
    }
    shows(&f, cx, "ircoid-stage");
    f
}

#[gpui_kit::test]
fn about_shows_the_logo_as_a_button(cx: &mut TestAppContext) {
    let f = about(cx);
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        let logo = window.find("about-logo");
        assert_eq!(logo.role(), Some(gpui_kit::Role::Button));
        assert_eq!(logo.label(), Some("IRCord"));
        assert!(logo.focused().is_some(), "Tab reaches it");
    })
    .unwrap();
}

#[gpui_kit::test]
fn four_presses_open_nothing_and_five_open_the_game(cx: &mut TestAppContext) {
    let f = about(cx);
    for _ in 0..4 {
        click(&f, cx, "about-logo");
    }
    assert!(!shown(&f, cx, "ircoid"), "four presses open nothing");
    click(&f, cx, "about-logo");
    assert!(shown(&f, cx, "ircoid-stage"), "the fifth opens the game");
}

#[gpui_kit::test]
fn escape_closes_the_game_back_to_about(cx: &mut TestAppContext) {
    let f = game(cx);
    press(&f, cx, "escape");
    assert!(!shown(&f, cx, "ircoid"), "Escape closes it");
    assert_eq!(
        cx.update(|cx| f.session.read(cx).route()),
        Some(Route::Settings)
    );
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find("about-logo").focused(),
            Some(true),
            "focus is back on the logo"
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn back_to_about_closes_the_game(cx: &mut TestAppContext) {
    let f = game(cx);
    click(&f, cx, "ircoid-back");
    assert!(!shown(&f, cx, "ircoid"));
    assert!(shown(&f, cx, "about-logo"));
}

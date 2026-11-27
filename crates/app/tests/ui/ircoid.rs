//! Ircoid, the game behind About's logo: the door, the dialog and the game.

use gpui_kit::test::{TestEventExt, TestWindowExt};
use gpui_kit::{AppContext, Entity, TestAppContext, VisualTestContext};
use ircord_app::features::ircoid::{self, Ircoid, Phase};
use ircord_app::route::Route;
use ircord_client::ircoid::{Brick, BrickKind, Game};

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

fn game_view(cx: &mut TestAppContext) -> Entity<Ircoid> {
    cx.update(|cx| ircoid::current(cx))
        .expect("the game is open")
}

fn phase(cx: &mut TestAppContext) -> Phase {
    let view = game_view(cx);
    cx.update(|cx| view.read(cx).phase())
}

fn advance(cx: &mut TestAppContext, frames: usize) {
    let view = game_view(cx);
    for _ in 0..frames {
        view.update(cx, |v, cx| v.advance(1. / 60., cx));
    }
}

fn key(f: &Fixture, cx: &mut TestAppContext, key: &str, down: bool) {
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        if down {
            window.key_down(key, false, cx);
        } else {
            window.key_up(key, cx);
        }
    })
    .unwrap();
    cx.run_until_parked();
}

/// How many callbacks a frame drawn now leaves for the display's next frame.
/// The dialog's entrance asks for one of its own in a test, where its clock
/// stands still, so a test compares with the count on the ready screen.
fn next_frame_asks(f: &Fixture, cx: &mut TestAppContext) -> usize {
    cx.run_until_parked();
    cx.update_window(f.window, |_, window, cx| {
        // Earlier frames' requests are not this frame's.
        window.simulate_next_frame(cx);
        window.render_frame(cx);
        window.simulate_next_frame(cx)
    })
    .unwrap()
}

#[gpui_kit::test]
fn nothing_moves_until_the_game_starts_and_after_it_pauses(cx: &mut TestAppContext) {
    let f = game(cx);
    assert_eq!(phase(cx), Phase::Ready);
    let still = next_frame_asks(&f, cx);
    click(&f, cx, "ircoid-play");
    assert_eq!(phase(cx), Phase::Playing);
    assert_eq!(
        next_frame_asks(&f, cx),
        still + 1,
        "a game being played asks for frames"
    );
    press(&f, cx, "p");
    assert_eq!(phase(cx), Phase::Paused);
    assert_eq!(
        next_frame_asks(&f, cx),
        still,
        "a paused game asks for none"
    );
    press(&f, cx, "p");
    assert_eq!(phase(cx), Phase::Playing);
}

#[gpui_kit::test]
fn holding_right_moves_the_paddle_and_letting_go_stops_it(cx: &mut TestAppContext) {
    let f = game(cx);
    click(&f, cx, "ircoid-play");
    let view = game_view(cx);
    let x = |cx: &mut TestAppContext| cx.update(|cx| view.read(cx).game().paddle.x);
    let start = x(cx);
    key(&f, cx, "right", true);
    advance(cx, 10);
    let moved = x(cx);
    assert!(moved > start + 10., "{start} → {moved}");
    key(&f, cx, "right", false);
    let stopped = x(cx);
    advance(cx, 10);
    assert_eq!(x(cx), stopped, "letting go stops it");
}

#[gpui_kit::test]
fn space_starts_and_launches(cx: &mut TestAppContext) {
    let f = game(cx);
    press(&f, cx, "space");
    assert_eq!(phase(cx), Phase::Playing);
    advance(cx, 1);
    let view = game_view(cx);
    assert!(!cx.update(|cx| view.read(cx).game().held), "launched");
}

#[gpui_kit::test]
fn enter_launches_and_leaves_the_game_open(cx: &mut TestAppContext) {
    let f = game(cx);
    press(&f, cx, "enter");
    assert!(shown(&f, cx, "ircoid"), "Enter does not close the dialog");
    assert_eq!(phase(cx), Phase::Playing);
}

#[gpui_kit::test]
fn deactivating_the_window_pauses(cx: &mut TestAppContext) {
    let f = game(cx);
    click(&f, cx, "ircoid-play");
    assert_eq!(phase(cx), Phase::Playing);
    cx.update_window(f.window, |_, window, _| window.activate_window())
        .unwrap();
    cx.run_until_parked();
    assert_eq!(phase(cx), Phase::Playing);
    let mut visual = VisualTestContext::from_window(f.window, cx);
    visual.deactivate_window();
    cx.run_until_parked();
    assert_eq!(phase(cx), Phase::Paused);
}

#[gpui_kit::test]
fn three_lost_balls_end_the_game_and_only_restart_starts_another(cx: &mut TestAppContext) {
    let f = game(cx);
    click(&f, cx, "ircoid-play");
    let view = game_view(cx);
    // No bricks to bounce off and the paddle far from the ball.
    let mut game = Game::new(1);
    game.bricks = vec![Brick::new(12, 0, BrickKind::Message)];
    game.paddle.x = 30.;
    game.last_dir = -1.;
    view.update(cx, |v, cx| v.set_game(game, cx));
    for _ in 0..3 {
        press(&f, cx, "space");
        for _ in 0..600 {
            if phase(cx) == Phase::Over || cx.update(|cx| view.read(cx).game().held) {
                break;
            }
            view.update(cx, |v, cx| {
                // Keep the paddle away from the ball.
                let ball = v.game().balls[0].x;
                let mut g = v.game().clone();
                g.paddle.x = if ball > 200. { 30. } else { 370. };
                v.set_game(g, cx);
                v.advance(1. / 30., cx);
            });
        }
    }
    assert_eq!(phase(cx), Phase::Over);
    press(&f, cx, "space");
    assert_eq!(phase(cx), Phase::Over, "Space does not start a game");
    click(&f, cx, "ircoid-restart");
    assert_eq!(phase(cx), Phase::Playing);
    assert_eq!(cx.update(|cx| view.read(cx).game().lives), 3);
}

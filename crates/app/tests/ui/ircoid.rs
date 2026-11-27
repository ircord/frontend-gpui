//! Ircoid, the game behind About's logo: the door, the dialog and the game.

use gpui_kit::test::{TestEventExt, TestWindowExt};
use gpui_kit::{AppContext, Entity, TestAppContext, VisualTestContext};
use ircord_app::features::ircoid::{self, Ircoid, Phase};
use ircord_app::route::Route;
use ircord_client::ircoid::{Ball, Brick, BrickKind, CapsuleKind, Game};

use ircord_client::testing::{CannedServer, Reply};
use serde_json::json;

use crate::fixture::{Fixture, bootstrap, wait};

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
    about_with(cx, |_| {})
}

/// [`about`], with `setup` run on the server first.
fn about_with(cx: &mut TestAppContext, setup: impl FnOnce(&CannedServer)) -> Fixture {
    let f = crate::fixture::open_with(cx, true, setup);
    bootstrap(&f, cx);
    click(&f, cx, "open-settings");
    shows(&f, cx, "settings-section-about");
    click(&f, cx, "settings-section-about");
    shows(&f, cx, "about-logo");
    f
}

/// About with the game open.
pub(crate) fn game(cx: &mut TestAppContext) -> Fixture {
    game_with(cx, |_| {})
}

/// [`game`], with `setup` run on the server first.
fn game_with(cx: &mut TestAppContext, setup: impl FnOnce(&CannedServer)) -> Fixture {
    let f = about_with(cx, setup);
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
    assert!(!shown(&f, cx, "ircoid-restart"), "only Play again");
    std::thread::sleep(ircoid::AGAIN_GUARD);
    click(&f, cx, "ircoid-again");
    assert_eq!(phase(cx), Phase::Playing);
    assert_eq!(cx.update(|cx| view.read(cx).game().lives), 3);
}

/// The stage's bounds in the window.
fn stage(f: &Fixture, cx: &mut TestAppContext) -> gpui_kit::Bounds<gpui_kit::Pixels> {
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.find("ircoid-stage").bounds()
    })
    .unwrap()
}

#[gpui_kit::test]
fn the_paddle_follows_the_pointer_and_a_click_launches(cx: &mut TestAppContext) {
    let f = game(cx);
    click(&f, cx, "ircoid-play");
    let bounds = stage(&f, cx);
    // A quarter of the way across the stage.
    let at = gpui_kit::point(
        bounds.origin.x + bounds.size.width * 0.25,
        bounds.origin.y + bounds.size.height * 0.5,
    );
    cx.update_window(f.window, |_, window, cx| window.pointer_move(at, None, cx))
        .unwrap();
    advance(cx, 1);
    let view = game_view(cx);
    let x = cx.update(|cx| view.read(cx).game().paddle.x);
    assert!(
        (x - 100.).abs() < 1.,
        "the paddle is under the pointer: {x}"
    );
    assert!(cx.update(|cx| view.read(cx).game().held));
    cx.update_window(f.window, |_, window, cx| {
        window.pointer_down(at, gpui_kit::MouseButton::Left, cx);
        window.pointer_up(at, gpui_kit::MouseButton::Left, cx);
    })
    .unwrap();
    advance(cx, 1);
    assert!(
        !cx.update(|cx| view.read(cx).game().held),
        "the click launched"
    );
}

#[gpui_kit::test]
fn a_key_takes_the_paddle_back_from_the_pointer(cx: &mut TestAppContext) {
    let f = game(cx);
    click(&f, cx, "ircoid-play");
    let bounds = stage(&f, cx);
    let at = gpui_kit::point(
        bounds.origin.x + bounds.size.width * 0.25,
        bounds.origin.y + bounds.size.height * 0.5,
    );
    cx.update_window(f.window, |_, window, cx| window.pointer_move(at, None, cx))
        .unwrap();
    advance(cx, 1);
    key(&f, cx, "right", true);
    advance(cx, 10);
    let view = game_view(cx);
    let x = cx.update(|cx| view.read(cx).game().paddle.x);
    assert!(x > 140., "the keys moved it from under the pointer: {x}");
}

fn label_of(f: &Fixture, cx: &mut TestAppContext, id: &'static str) -> Option<String> {
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window
            .try_find(id)
            .and_then(|e| e.label().map(str::to_string))
    })
    .unwrap()
}

#[gpui_kit::test]
fn the_status_bar_reads_the_game(cx: &mut TestAppContext) {
    let f = game(cx);
    click(&f, cx, "ircoid-play");
    let view = game_view(cx);
    let mut game = Game::new(1);
    game.score = 1240;
    game.apply(CapsuleKind::Kick);
    view.update(cx, |v, cx| v.set_game(game, cx));
    // Half a second of game time, the ball still on the paddle. Frames
    // drawn meanwhile step the game a little too.
    advance(cx, 30);
    let status = label_of(&f, cx, "ircoid-status").unwrap();
    let left = status
        .strip_prefix("Score 1240, 3 lives, level 1 #the-hash, /kick ")
        .unwrap_or_else(|| panic!("{status}"));
    assert!(["7", "8"].contains(&left), "{status}");
}

#[gpui_kit::test]
fn each_screen_shows_its_notice(cx: &mut TestAppContext) {
    let f = game(cx);
    assert_eq!(
        label_of(&f, cx, "ircoid-notice-ready").as_deref(),
        Some("*** Now talking in #the-hash")
    );
    click(&f, cx, "ircoid-play");
    assert!(!shown(&f, cx, "ircoid-notice-ready"));
    press(&f, cx, "p");
    assert!(shown(&f, cx, "ircoid-notice-paused"));
    press(&f, cx, "p");
    assert!(!shown(&f, cx, "ircoid-notice-paused"));

    // The last brick of level 1 breaks: the new topic shows for a while.
    let view = game_view(cx);
    let mut game = Game::new(1);
    game.bricks = vec![Brick::new(0, 0, BrickKind::Message)];
    game.held = false;
    let (x, y) = game.bricks[0].origin();
    game.balls = vec![Ball {
        x: x + 14.,
        y: y + 13.,
        vx: 0.,
        vy: -170.,
    }];
    view.update(cx, |v, cx| v.set_game(game, cx));
    advance(cx, 1);
    assert_eq!(
        label_of(&f, cx, "ircoid-notice-level").as_deref(),
        Some("*** Topic is now: level 2, #motd")
    );
    assert_eq!(
        cx.update(|cx| view.read(cx).announcement().to_string()),
        "Level 2, #motd."
    );
    advance(cx, 100);
    assert!(!shown(&f, cx, "ircoid-notice-level"), "gone after 1.5 s");
}

/// Plays until a ball is lost, the paddle kept away from it.
fn lose_a_ball(f: &Fixture, cx: &mut TestAppContext) {
    let view = game_view(cx);
    press(f, cx, "space");
    for _ in 0..600 {
        if phase(cx) == Phase::Over || cx.update(|cx| view.read(cx).game().held) {
            return;
        }
        view.update(cx, |v, cx| {
            let ball = v.game().balls[0].x;
            let mut g = v.game().clone();
            g.paddle.x = if ball > 200. { 30. } else { 370. };
            v.set_game(g, cx);
            v.advance(1. / 30., cx);
        });
    }
    panic!("the ball was never lost");
}

#[gpui_kit::test]
fn the_live_region_says_a_life_was_lost_and_the_game_is_over(cx: &mut TestAppContext) {
    let f = game(cx);
    click(&f, cx, "ircoid-play");
    let view = game_view(cx);
    let mut game = Game::new(1);
    game.bricks = vec![Brick::new(12, 0, BrickKind::Message)];
    game.score = 470;
    view.update(cx, |v, cx| v.set_game(game, cx));
    lose_a_ball(&f, cx);
    assert_eq!(
        label_of(&f, cx, "ircoid-announcement").as_deref(),
        Some("Ball lost: 2 lives left.")
    );
    lose_a_ball(&f, cx);
    lose_a_ball(&f, cx);
    assert_eq!(phase(cx), Phase::Over);
    assert_eq!(
        label_of(&f, cx, "ircoid-announcement").as_deref(),
        Some("Game over. Final score 470.")
    );
    assert_eq!(
        label_of(&f, cx, "ircoid-notice-over").as_deref(),
        Some("ERROR :Closing Link (Ping timeout) Final score: 470")
    );
}

#[gpui_kit::test]
fn the_key_hints_name_the_game_keys(cx: &mut TestAppContext) {
    let f = game(cx);
    assert_eq!(
        label_of(&f, cx, "ircoid-hints").as_deref(),
        Some("← → Move, Space Launch, P Pause, Esc Back to About")
    );
}

const SCORES: &str = "/api/v1/ircoid/scores";

fn two_rows(server: &CannedServer) {
    server.route(
        "GET",
        SCORES,
        200,
        json!({"items": [
            {"rank": 1, "name": "Peter Petermann", "score": 225357},
            {"rank": 2, "name": "Michael Karbowiak", "score": 2102}
        ]}),
    );
}

/// Ends the game with `score`: one life left, and that ball lost.
fn end_game(f: &Fixture, cx: &mut TestAppContext, score: u32) {
    click(f, cx, "ircoid-play");
    let view = game_view(cx);
    let mut game = Game::new(1);
    game.bricks = vec![Brick::new(12, 0, BrickKind::Message)];
    game.score = score;
    game.lives = 1;
    view.update(cx, |v, cx| v.set_game(game, cx));
    lose_a_ball(f, cx);
    assert_eq!(phase(cx), Phase::Over);
}

#[gpui_kit::test]
fn high_scores_pause_the_game_and_list_the_table(cx: &mut TestAppContext) {
    let f = game_with(cx, two_rows);
    click(&f, cx, "ircoid-play");
    assert_eq!(phase(cx), Phase::Playing);
    click(&f, cx, "ircoid-scores");
    let view = game_view(cx);
    assert!(
        cx.update(|cx| view.read(cx).scores_shown()),
        "the table opened"
    );
    assert_eq!(phase(cx), Phase::Paused);
    shows(&f, cx, "ircoid-score-2");
    assert_eq!(
        label_of(&f, cx, "ircoid-score-1").as_deref(),
        Some("1. Peter Petermann, 225357")
    );
    assert_eq!(
        label_of(&f, cx, "ircoid-score-2").as_deref(),
        Some("2. Michael Karbowiak, 2102")
    );
    click(&f, cx, "ircoid-scores-back");
    assert!(!shown(&f, cx, "ircoid-scores-panel"));
    assert_eq!(phase(cx), Phase::Paused);
}

#[gpui_kit::test]
fn a_placing_score_asks_for_a_name_and_saves_it(cx: &mut TestAppContext) {
    let f = game_with(cx, |server| {
        two_rows(server);
        server.route(
            "POST",
            SCORES,
            200,
            json!({"placed": true, "rank": 2, "items": [
                {"rank": 1, "name": "Peter Petermann", "score": 225357},
                {"rank": 2, "name": "me", "score": 4560},
                {"rank": 3, "name": "Michael Karbowiak", "score": 2102}
            ]}),
        );
    });
    end_game(&f, cx, 4560);
    shows(&f, cx, "ircoid-name");
    click(&f, cx, "ircoid-save");
    shows(&f, cx, "ircoid-scores-outcome");
    assert_eq!(
        label_of(&f, cx, "ircoid-scores-outcome").as_deref(),
        Some("Saved at rank 2.")
    );
    assert_eq!(
        label_of(&f, cx, "ircoid-score-2").as_deref(),
        Some("2. me, 4560, your new entry")
    );
    assert!(!shown(&f, cx, "ircoid-name"));
    let sent = f.server.requests_to("POST", SCORES);
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].json(), json!({"name": "me", "score": 4560}));
}

#[gpui_kit::test]
fn a_score_that_does_not_place_asks_for_nothing(cx: &mut TestAppContext) {
    let f = game_with(cx, |server| {
        let items: Vec<_> = (0..10)
            .map(|n| json!({"rank": n + 1, "name": format!("p{n}"), "score": 100000 - n * 10}))
            .collect();
        server.route("GET", SCORES, 200, json!({ "items": items }));
    });
    end_game(&f, cx, 470);
    shows(&f, cx, "ircoid-score-10");
    assert!(!shown(&f, cx, "ircoid-name"));
}

#[gpui_kit::test]
fn a_server_without_the_table_still_plays(cx: &mut TestAppContext) {
    let f = game(cx);
    end_game(&f, cx, 470);
    shows(&f, cx, "ircoid-scores-unavailable");
    assert_eq!(
        label_of(&f, cx, "ircoid-scores-unavailable").as_deref(),
        Some("High scores are not available on this server.")
    );
    std::thread::sleep(ircoid::AGAIN_GUARD);
    click(&f, cx, "ircoid-again");
    assert_eq!(phase(cx), Phase::Playing);
    assert!(!shown(&f, cx, "ircoid-scores-panel"));
}

#[gpui_kit::test]
fn a_refused_name_shows_the_servers_reason(cx: &mut TestAppContext) {
    let f = game_with(cx, |server| {
        two_rows(server);
        server.route_reply(
            "POST",
            SCORES,
            Reply::problem(422, "That name is already taken by a ghost."),
        );
    });
    end_game(&f, cx, 4560);
    shows(&f, cx, "ircoid-name");
    click(&f, cx, "ircoid-save");
    shows(&f, cx, "ircoid-name-error");
    assert_eq!(
        label_of(&f, cx, "ircoid-name-error").as_deref(),
        Some("That name is already taken by a ghost.")
    );
    assert!(
        shown(&f, cx, "ircoid-name"),
        "the field stays for another try"
    );
}

#[gpui_kit::test]
fn escape_in_the_name_field_skips_the_entry(cx: &mut TestAppContext) {
    let f = game_with(cx, two_rows);
    end_game(&f, cx, 4560);
    shows(&f, cx, "ircoid-name");
    press(&f, cx, "escape");
    assert!(shown(&f, cx, "ircoid"), "the game stays open");
    assert!(!shown(&f, cx, "ircoid-name"));
    assert!(shown(&f, cx, "ircoid-score-2"), "the table stays");
    assert!(f.server.requests_to("POST", SCORES).is_empty());
}

fn focused_on(f: &Fixture, cx: &mut TestAppContext, id: &'static str) -> bool {
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.find(id).focused() == Some(true)
    })
    .unwrap()
}

/// The game with the table open from the High scores button.
fn table(cx: &mut TestAppContext, setup: impl FnOnce(&CannedServer)) -> Fixture {
    let f = game_with(cx, setup);
    click(&f, cx, "ircoid-scores");
    shows(&f, cx, "ircoid-credits-open");
    f
}

#[gpui_kit::test]
fn the_credits_line_is_under_the_table_even_without_scores(cx: &mut TestAppContext) {
    let f = table(cx, two_rows);
    shows(&f, cx, "ircoid-score-2");
    assert!(shown(&f, cx, "ircoid-credits-open"));
    let f = table(cx, |_| {});
    shows(&f, cx, "ircoid-scores-unavailable");
    assert!(shown(&f, cx, "ircoid-credits-open"));
}

#[gpui_kit::test]
fn the_credits_name_everyone_and_escape_goes_back_to_the_table(cx: &mut TestAppContext) {
    let f = table(cx, two_rows);
    shows(&f, cx, "ircoid-score-2");
    click(&f, cx, "ircoid-credits-open");
    assert!(
        !shown(&f, cx, "ircoid-scores-panel"),
        "the credits replace the table"
    );
    let stage = label_of(&f, cx, "ircoid-stage").unwrap();
    for name in ircord_client::ircoid::credits::contributors() {
        assert!(stage.contains(name), "{stage}");
    }
    assert_eq!(
        label_of(&f, cx, "ircoid-announcement").as_deref(),
        Some(stage.as_str())
    );
    press(&f, cx, "escape");
    assert!(shown(&f, cx, "ircoid"), "the game stays open");
    assert!(shown(&f, cx, "ircoid-score-2"), "the table is back");
    assert!(
        focused_on(&f, cx, "ircoid-credits-open"),
        "focus is on the line"
    );
    assert_eq!(
        f.server.requests_to("GET", SCORES).len(),
        1,
        "no new request"
    );

    click(&f, cx, "ircoid-credits-open");
    click(&f, cx, "ircoid-credits-back");
    assert!(shown(&f, cx, "ircoid-score-2"));
    assert!(focused_on(&f, cx, "ircoid-credits-open"));
}

#[gpui_kit::test]
fn the_credits_move_only_while_open_and_motion_is_allowed(cx: &mut TestAppContext) {
    let f = table(cx, two_rows);
    cx.update_window(f.window, |_, window, _| window.activate_window())
        .unwrap();
    cx.run_until_parked();
    let still = next_frame_asks(&f, cx);
    click(&f, cx, "ircoid-credits-open");
    assert_eq!(next_frame_asks(&f, cx), still + 1, "the credits move");
    press(&f, cx, "escape");
    assert_eq!(next_frame_asks(&f, cx), still, "and stop when closed");

    cx.update(|cx| {
        let mut theme = ircord_app::theme::get(cx).clone();
        theme.reduced_motion = true;
        ircord_app::theme::set(theme, cx);
    });
    click(&f, cx, "ircoid-credits-open");
    assert_eq!(next_frame_asks(&f, cx), still, "a still picture");
}

#[gpui_kit::test]
fn the_game_keys_do_nothing_on_the_credits(cx: &mut TestAppContext) {
    let f = table(cx, two_rows);
    click(&f, cx, "ircoid-credits-open");
    let view = game_view(cx);
    let before = cx.update(|cx| view.read(cx).game().clone());
    let phase_before = phase(cx);
    for key in ["space", "enter", "up", "p", "a", "d"] {
        press(&f, cx, key);
    }
    assert_eq!(phase(cx), phase_before);
    assert_eq!(cx.update(|cx| view.read(cx).game().clone()), before);
    assert!(cx.update(|cx| view.read(cx).credits_open()));
}

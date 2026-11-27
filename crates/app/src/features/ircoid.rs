//! Ircoid, the brick-breaking game behind About's logo: five presses on the
//! logo in a row open it in a dialog over Settings, as the web's logo opens
//! Hash Field. Escape closes it and About is still there underneath.
//!
//! The rules are `ircord_client::ircoid`; this module draws them and takes
//! the keys. Frames are asked for only while a game is being played: the
//! ready, paused and game over screens cost nothing. The game pauses when
//! the window loses focus, when focus leaves the game, and on P.

use std::time::{Duration, Instant};

use gpui_kit::TestSupportExt;
use gpui_kit::component::WindowExt;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::prelude::*;
use gpui_kit::*;
use ircord_client::ircoid::{self as rules, FIELD_H, FIELD_W, Game, Input, MAX_DT};

use crate::session::Session;
use crate::theme;

/// Presses on the logo that open the game.
pub const PRESSES: u32 = 5;
/// A pause this long between presses starts the count again.
pub const PRESS_GAP: Duration = Duration::from_millis(1500);
/// The dialog is at most this wide.
const MAX_WIDTH: f32 = 848.;
/// The status bar under the field, in game units.
pub const BAR_H: f32 = 16.;

/// Counts presses on About's logo. It is given the time of each press, so
/// it needs no timer.
#[derive(Debug, Default, Clone)]
pub struct Presses {
    count: u32,
    last: Option<Instant>,
}

impl Presses {
    /// Counts a press at `at`; true when it is the one that opens the game,
    /// and the count starts again.
    pub fn press(&mut self, at: Instant) -> bool {
        let close = self
            .last
            .is_some_and(|last| at.saturating_duration_since(last) <= PRESS_GAP);
        self.count = if close { self.count + 1 } else { 1 };
        self.last = Some(at);
        if self.count >= PRESSES {
            *self = Self::default();
            return true;
        }
        false
    }
}

/// Where a game is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Not started: the ready screen.
    Ready,
    Playing,
    Paused,
    /// The last life is gone.
    Over,
}

/// The open game, while one is open.
struct Open(WeakEntity<Ircoid>);
impl Global for Open {}

/// The open game, if there is one.
pub fn current(cx: &App) -> Option<Entity<Ircoid>> {
    cx.try_global::<Open>().and_then(|open| open.0.upgrade())
}

/// Open the game in a dialog.
pub fn open(session: &Entity<Session>, window: &mut Window, cx: &mut App) {
    let view = cx.new(|cx| Ircoid::new(session.clone(), window, cx));
    cx.set_global(Open(view.downgrade()));
    let focus = view.read(cx).focus.clone();
    let viewport = window.viewport_size();
    let width = px(MAX_WIDTH).min(viewport.width - px(64.)).max(px(320.));
    window.open_dialog(cx, move |dialog, _, _| {
        let game = view.clone();
        dialog
            .width(width)
            .title(t!("ircoid-title"))
            .overlay_closable(false)
            // Enter reaches the dialog as its Confirm, before the game's
            // keys: it launches, and must not close the game.
            .on_ok(move |_, _, cx| {
                game.update(cx, |game, cx| game.primary(cx));
                false
            })
            .child(view.clone())
    });
    window.focus(&focus, cx);
}

/// A game seed from the clock.
fn seed() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(1)
}

/// The keys held down.
#[derive(Debug, Default, Clone, Copy)]
struct Held {
    left: bool,
    right: bool,
}

/// The open game.
pub struct Ircoid {
    #[allow(dead_code)]
    session: Entity<Session>,
    focus: FocusHandle,
    game: Game,
    phase: Phase,
    held: Held,
    /// A launch asked for since the last step.
    launch: bool,
    /// When the last frame was drawn while playing.
    last_frame: Option<Instant>,
    _subscriptions: Vec<Subscription>,
}

impl Ircoid {
    fn new(session: Entity<Session>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        let subscriptions = vec![
            cx.observe_window_activation(window, |this, window, cx| {
                if !window.is_window_active() {
                    this.pause(cx);
                }
            }),
            cx.on_focus_out(&focus, window, |this, _, _, cx| this.pause(cx)),
        ];
        Self {
            session,
            focus,
            game: Game::new(seed()),
            phase: Phase::Ready,
            held: Held::default(),
            launch: false,
            last_frame: None,
            _subscriptions: subscriptions,
        }
    }

    pub fn phase(&self) -> Phase {
        self.phase
    }

    pub fn game(&self) -> &Game {
        &self.game
    }

    /// Replaces the game, as a test or the capture sets one up.
    pub fn set_game(&mut self, game: Game, cx: &mut Context<Self>) {
        self.game = game;
        cx.notify();
    }

    /// Moves the game on by `dt` seconds (at most [`MAX_DT`]) with the keys
    /// held now. Frames call it while playing; tests call it directly.
    pub fn advance(&mut self, dt: f32, cx: &mut Context<Self>) {
        self.step(dt);
        cx.notify();
    }

    fn step(&mut self, dt: f32) {
        if self.phase != Phase::Playing {
            return;
        }
        let input = Input {
            left: self.held.left,
            right: self.held.right,
            launch: std::mem::take(&mut self.launch),
            pointer: None,
        };
        let events = self.game.step(&input, dt.min(MAX_DT));
        if events.game_over {
            self.set_phase(Phase::Over);
        }
    }

    fn set_phase(&mut self, phase: Phase) {
        self.phase = phase;
        self.last_frame = None;
        self.launch = false;
        if phase != Phase::Playing {
            self.held = Held::default();
        }
    }

    /// Start, or resume: Space, Up, Enter and the Start button.
    pub fn play(&mut self, cx: &mut Context<Self>) {
        match self.phase {
            Phase::Ready | Phase::Paused => self.set_phase(Phase::Playing),
            Phase::Playing | Phase::Over => return,
        }
        cx.notify();
    }

    pub fn pause(&mut self, cx: &mut Context<Self>) {
        if self.phase == Phase::Playing {
            self.set_phase(Phase::Paused);
            cx.notify();
        }
    }

    /// A new game, being played, with the ball on the paddle.
    pub fn restart(&mut self, cx: &mut Context<Self>) {
        self.game = Game::new(seed());
        self.set_phase(Phase::Playing);
        cx.notify();
    }

    /// Space, Up or Enter: start from the ready screen and launch, resume
    /// from the paused one, or launch while playing.
    fn primary(&mut self, cx: &mut Context<Self>) {
        match self.phase {
            Phase::Ready => {
                self.set_phase(Phase::Playing);
                self.launch = true;
            }
            Phase::Paused => self.set_phase(Phase::Playing),
            Phase::Playing => self.launch = true,
            // Only Restart starts a new game: Space may still be held.
            Phase::Over => return,
        }
        cx.notify();
    }

    fn key_down(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) -> bool {
        let k = &event.keystroke;
        if k.modifiers.secondary(cx) || k.modifiers.alt || k.modifiers.control {
            return false;
        }
        match k.key.as_str() {
            "left" | "a" => self.hold(true, cx),
            "right" | "d" => self.hold(false, cx),
            "space" | "up" if !event.is_held => self.primary(cx),
            "space" | "up" => {}
            "p" if !event.is_held => match self.phase {
                Phase::Playing => self.pause(cx),
                Phase::Paused => self.play(cx),
                _ => {}
            },
            "p" => {}
            _ => return false,
        }
        true
    }

    fn key_up(&mut self, event: &KeyUpEvent) -> bool {
        match event.keystroke.key.as_str() {
            "left" | "a" => self.held.left = false,
            "right" | "d" => self.held.right = false,
            _ => return false,
        }
        true
    }

    /// Holds Left (`left` true) or Right, while playing.
    fn hold(&mut self, left: bool, cx: &mut Context<Self>) {
        if self.phase != Phase::Playing {
            return;
        }
        if left {
            self.held.left = true;
        } else {
            self.held.right = true;
        }
        cx.notify();
    }

    /// While playing, step by the time since the last frame and ask for the
    /// next one.
    fn frame(&mut self, window: &mut Window) {
        if self.phase != Phase::Playing {
            return;
        }
        let now = Instant::now();
        if let Some(last) = self.last_frame {
            self.step(now.saturating_duration_since(last).as_secs_f32());
        }
        if self.phase == Phase::Playing {
            self.last_frame = Some(now);
            window.request_animation_frame();
        }
    }

    fn buttons(&self, cx: &mut Context<Self>) -> Div {
        let primary = match self.phase {
            Phase::Ready => Some((t!("ircoid-start"), false)),
            Phase::Playing => Some((t!("ircoid-pause"), true)),
            Phase::Paused => Some((t!("ircoid-resume"), false)),
            Phase::Over => None,
        };
        div()
            .flex()
            .gap_2()
            .children(primary.map(|(label, pause)| {
                Button::new("ircoid-play")
                    .primary()
                    .label(label)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if pause {
                            this.pause(cx);
                        } else {
                            this.play(cx);
                        }
                        window.focus(&this.focus, cx);
                    }))
            }))
            .child(
                Button::new("ircoid-restart")
                    .label(t!("ircoid-restart"))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.restart(cx);
                        window.focus(&this.focus, cx);
                    })),
            )
            .child(
                Button::new("ircoid-back")
                    .label(t!("ircoid-back"))
                    .on_click(|_, window, cx| window.close_dialog(cx)),
            )
    }
}

impl Focusable for Ircoid {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

/// What the stage draws: a copy of the game.
#[derive(Clone)]
struct Picture {
    game: Game,
    wall: Hsla,
    brick: Hsla,
    ban: Hsla,
    paddle: Hsla,
    ball: Hsla,
    bar: Hsla,
}

/// Game units to pixels inside `bounds`.
fn to_px(bounds: Bounds<Pixels>, x: f32, y: f32, w: f32, h: f32) -> Bounds<Pixels> {
    let k = f32::from(bounds.size.width) / FIELD_W;
    Bounds::new(
        point(bounds.origin.x + px(x * k), bounds.origin.y + px(y * k)),
        size(px(w * k), px(h * k)),
    )
}

fn paint_plain(picture: &Picture, bounds: Bounds<Pixels>, window: &mut Window) {
    use rules::{BALL_R, BRICK_H, BRICK_W, BrickKind, PADDLE_H, PADDLE_Y, WALL};
    let game = &picture.game;
    let quad = |window: &mut Window, x, y, w, h, color: Hsla| {
        window.paint_quad(fill(to_px(bounds, x, y, w, h), color));
    };
    quad(window, 0., 0., WALL, FIELD_H, picture.wall);
    quad(window, FIELD_W - WALL, 0., WALL, FIELD_H, picture.wall);
    quad(window, 0., 0., FIELD_W, WALL, picture.wall);
    for brick in &game.bricks {
        let (x, y) = brick.origin();
        let color = match brick.kind {
            BrickKind::Ban => picture.ban,
            _ => picture.brick,
        };
        quad(window, x, y, BRICK_W, BRICK_H, color);
    }
    let paddle = &game.paddle;
    quad(
        window,
        paddle.left(),
        PADDLE_Y,
        paddle.w,
        PADDLE_H,
        picture.paddle,
    );
    for ball in &game.balls {
        let b = to_px(
            bounds,
            ball.x - BALL_R,
            ball.y - BALL_R,
            BALL_R * 2.,
            BALL_R * 2.,
        );
        window.paint_quad(fill(b, picture.ball).corner_radii(b.size.width / 2.));
    }
    quad(window, 0., FIELD_H, FIELD_W, BAR_H, picture.bar);
}

impl Render for Ircoid {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.frame(window);
        let theme = theme::get(cx).clone();
        let picture = Picture {
            game: self.game.clone(),
            wall: theme.border,
            brick: theme.accent,
            ban: theme.faint,
            paddle: theme.text,
            ball: theme.text,
            bar: theme.sidebar,
        };
        div()
            .id("ircoid")
            .test_support()
            .track_focus(&self.focus)
            .flex()
            .flex_col()
            .gap_3()
            .on_key_down(cx.listener(|this, e: &KeyDownEvent, _, cx| {
                if this.key_down(e, cx) {
                    cx.stop_propagation();
                }
            }))
            .on_key_up(cx.listener(|this, e: &KeyUpEvent, _, cx| {
                if this.key_up(e) {
                    cx.stop_propagation();
                }
            }))
            .child(
                div()
                    .id("ircoid-stage")
                    .test_support()
                    .role(Role::Image)
                    .aria_label(t!("ircoid-stage"))
                    .relative()
                    .w_full()
                    .aspect_ratio(FIELD_W / (FIELD_H + BAR_H))
                    .rounded_md()
                    .overflow_hidden()
                    .bg(theme.rail)
                    .child(
                        canvas(
                            |_, _, _| (),
                            move |bounds, _, window, _| paint_plain(&picture, bounds, window),
                        )
                        .absolute()
                        .size_full(),
                    ),
            )
            .child(div().flex().justify_end().child(self.buttons(cx)))
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::Presses;

    #[test]
    fn five_quick_presses_open_the_game() {
        let start = Instant::now();
        let mut presses = Presses::default();
        let opened: Vec<bool> = (0..5)
            .map(|n| presses.press(start + Duration::from_millis(300 * n)))
            .collect();
        assert_eq!(opened, [false, false, false, false, true]);
        // The count starts again after opening.
        assert!(!presses.press(start + Duration::from_millis(1600)));
    }

    #[test]
    fn a_pause_starts_the_count_again() {
        let start = Instant::now();
        let mut presses = Presses::default();
        for n in 0..4 {
            assert!(!presses.press(start + Duration::from_millis(200 * n)));
        }
        let later = start + Duration::from_millis(600 + 2000);
        assert!(!presses.press(later), "four, a 2 s gap, then one");
        for n in 1..4 {
            assert!(!presses.press(later + Duration::from_millis(200 * n)));
        }
        assert!(presses.press(later + Duration::from_millis(800)));
    }
}

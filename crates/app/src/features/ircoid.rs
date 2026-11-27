//! Ircoid, the brick-breaking game behind About's logo: five presses on the
//! logo in a row open it in a dialog over Settings, as the web's logo opens
//! Hash Field. Escape closes it and About is still there underneath.
//!
//! The rules are `ircord_client::ircoid`; this module draws them and takes
//! the keys. Frames are asked for only while a game is being played: the
//! ready, paused and game over screens cost nothing. The game pauses when
//! the window loses focus, when focus leaves the game, and on P.

use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui_kit::TestSupportExt;
use gpui_kit::component::WindowExt;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::prelude::*;
use gpui_kit::*;
use ircord_client::ircoid::{self as rules, FIELD_H, FIELD_W, Game, Input, MAX_DT};

use crate::session::Session;
use crate::theme;

pub mod stage;

/// Presses on the logo that open the game.
pub const PRESSES: u32 = 5;
/// A pause this long between presses starts the count again.
pub const PRESS_GAP: Duration = Duration::from_millis(1500);
/// The dialog is at most this wide.
const MAX_WIDTH: f32 = 848.;
/// The status bar under the field, in game units.
pub const BAR_H: f32 = 16.;
/// How fast the grid runs toward the viewer, in cross lines a second.
const GRID_SPEED: f32 = 0.8;
/// How long the level's new topic shows, in seconds of game time.
pub const LEVEL_NOTICE_SECS: f32 = 1.5;

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

/// The field's x under a window's `x`, for a stage drawn `left` to
/// `left + width`; `None` off the stage.
pub fn field_x(x: f32, left: f32, width: f32) -> Option<f32> {
    if width <= 0. || x < left || x > left + width {
        return None;
    }
    Some((x - left) / width * FIELD_W)
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
    /// Where the pointer put the paddle, until a key moves it.
    pointer: Option<f32>,
    /// Where the stage was last drawn.
    stage: Rc<Cell<Option<Bounds<Pixels>>>>,
    /// When the last frame was drawn while playing.
    last_frame: Option<Instant>,
    /// How far the grid has run, in cross lines.
    travel: f32,
    /// The balls' trails and the sparks.
    fx: stage::Fx,
    /// Reduced motion, as the theme last said.
    reduced: bool,
    /// For the sparks' directions.
    rng: rules::Rng,
    /// Seconds of game time the new level's topic still shows.
    level_notice: f32,
    /// The live region's text.
    announcement: String,
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
            pointer: None,
            stage: Rc::default(),
            last_frame: None,
            travel: 0.,
            fx: stage::Fx::default(),
            reduced: false,
            rng: rules::Rng::new(seed()),
            level_notice: 0.,
            announcement: String::new(),
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
            pointer: self.pointer,
        };
        let dt = dt.min(MAX_DT);
        self.travel = (self.travel + dt * GRID_SPEED).fract();
        let events = self.game.step(&input, dt);
        if self.reduced {
            self.fx = stage::Fx::default();
        } else {
            self.fx.tick(dt);
            for hit in events.hits.iter().filter(|hit| hit.broken) {
                let brick = rules::Brick::new(hit.col, hit.row, hit.kind);
                self.fx.burst(&brick, &mut self.rng);
            }
            self.fx.follow(&self.game);
        }
        self.level_notice = (self.level_notice - dt).max(0.);
        self.announce(&events);
        if events.game_over {
            self.set_phase(Phase::Over);
        }
    }

    /// Says in the live region what a screen reader cannot see.
    fn announce(&mut self, events: &rules::Events) {
        let game = &self.game;
        let mut said: Vec<String> = Vec::new();
        for kind in &events.caught {
            said.push(t!("ircoid-announce-caught", capsule = capsule_name(*kind)));
        }
        if events.level_cleared {
            self.level_notice = LEVEL_NOTICE_SECS;
            said.push(t!(
                "ircoid-announce-level",
                level = game.level,
                channel = game.channel()
            ));
        }
        if events.life_gained {
            said.push(t!("ircoid-announce-life-gained", lives = game.lives));
        }
        if events.game_over {
            said.push(t!("ircoid-announce-over", score = game.score));
        } else if events.life_lost {
            said.push(t!("ircoid-announce-life-lost", lives = game.lives));
        }
        if !said.is_empty() {
            self.announcement = said.join(" ");
        }
    }

    /// The live region's text, for tests.
    pub fn announcement(&self) -> &str {
        &self.announcement
    }

    /// The status bar as one line, as a screen reader reads it.
    pub fn status(&self) -> String {
        let game = &self.game;
        let mut line = t!(
            "ircoid-status",
            score = game.score,
            lives = game.lives,
            level = game.level,
            channel = game.channel()
        );
        for (label, seconds) in running(game) {
            line.push_str(", ");
            match seconds {
                Some(seconds) => line.push_str(&t!(
                    "ircoid-status-effect",
                    effect = label,
                    seconds = seconds
                )),
                None => line.push_str(label),
            }
        }
        line
    }

    fn set_phase(&mut self, phase: Phase) {
        if phase == Phase::Paused && self.phase == Phase::Playing {
            self.announcement = t!("ircoid-notice-paused").to_string();
        }
        self.phase = phase;
        self.last_frame = None;
        self.launch = false;
        if phase != Phase::Playing {
            self.held = Held::default();
            self.pointer = None;
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
        self.fx = stage::Fx::default();
        self.level_notice = 0.;
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
        // The keys act last: the paddle stops following the pointer.
        self.pointer = None;
        cx.notify();
    }

    /// The field's x under a window position, if it is over the stage.
    fn stage_x(&self, x: Pixels) -> Option<f32> {
        let stage = self.stage.get()?;
        field_x(
            f32::from(x),
            f32::from(stage.origin.x),
            f32::from(stage.size.width),
        )
    }

    /// The pointer moved over the stage: the paddle follows it.
    pub fn pointer_moved(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        if self.phase != Phase::Playing {
            return;
        }
        if let Some(x) = self.stage_x(position.x) {
            self.pointer = Some(x);
            cx.notify();
        }
    }

    /// A sideways scroll nudges the paddle by the distance scrolled.
    fn scrolled(&mut self, dx: Pixels, cx: &mut Context<Self>) {
        let Some(stage) = self.stage.get() else {
            return;
        };
        if self.phase != Phase::Playing || dx == px(0.) {
            return;
        }
        let units = f32::from(dx) / f32::from(stage.size.width) * FIELD_W;
        self.pointer = Some(self.pointer.unwrap_or(self.game.paddle.x) + units);
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

/// The glyphs on bricks and the labels on capsules, as text over the
/// picture, for a stage `width` pixels wide.
fn stage_labels(game: &Game, width: f32) -> Vec<Div> {
    let k = stage::scale(width);
    stage::labels(game, width)
        .into_iter()
        .map(|label| {
            let (text, size) = match label.text {
                stage::LabelText::Op => (t!("ircoid-glyph-op"), 8.5),
                stage::LabelText::Owner => (t!("ircoid-glyph-owner"), 8.5),
                stage::LabelText::Capsule(kind) => (capsule_label(kind), 6.5),
            };
            div()
                .absolute()
                .left(px(label.at.x))
                .top(px(label.at.y))
                .w(px(label.at.w))
                .h(px(label.at.h))
                .flex()
                .items_center()
                .justify_center()
                .font_family(stage::FONT)
                .font_weight(FontWeight::BOLD)
                .text_size(px(size * k))
                .line_height(px(label.at.h))
                .text_color(stage::color(label.color, 1.))
                .child(text)
        })
        .collect()
}

/// What a capsule is called when it is announced.
pub fn capsule_name(kind: rules::CapsuleKind) -> &'static str {
    use rules::CapsuleKind::*;
    match kind {
        Op => t!("ircoid-capsule-name-op"),
        Join => t!("ircoid-capsule-name-join"),
        Moderated => t!("ircoid-capsule-name-moderated"),
        Kick => t!("ircoid-capsule-name-kick"),
        Bouncer => t!("ircoid-capsule-name-bouncer"),
        Lag => t!("ircoid-capsule-name-lag"),
    }
}

/// The capsules at work, as the status bar shows them: each label with the
/// whole seconds it has left (none for the Bouncer).
fn running(game: &Game) -> Vec<(&'static str, Option<u32>)> {
    let e = &game.effects;
    let secs = |t: f32| Some(t.ceil() as u32);
    let mut out = Vec::new();
    if e.op > 0. {
        out.push((capsule_label(rules::CapsuleKind::Op), secs(e.op)));
    }
    match e.pace {
        Some((rules::Pace::Moderated, t)) => {
            out.push((capsule_label(rules::CapsuleKind::Moderated), secs(t)))
        }
        Some((rules::Pace::Lag, t)) => out.push((capsule_label(rules::CapsuleKind::Lag), secs(t))),
        None => {}
    }
    if e.kick > 0. {
        out.push((capsule_label(rules::CapsuleKind::Kick), secs(e.kick)));
    }
    if e.bouncer {
        out.push((capsule_label(rules::CapsuleKind::Bouncer), None));
    }
    out
}

/// A piece of the status bar's text in a colour.
fn span(text: impl Into<SharedString>, hex: u32) -> Div {
    div().text_color(stage::color(hex, 1.)).child(text.into())
}

/// The irssi-style status bar: `[score 001240] [lives ♥♥♥] [1:#the-hash]
/// [/kick 07s]`, as text over the bar's ground.
fn status_bar(game: &Game, width: f32, line: String) -> impl IntoElement {
    use stage::palette::*;
    let k = stage::scale(width);
    let item = |parts: Vec<Div>| {
        div()
            .flex()
            .child(span("[", VIOLET))
            .children(parts)
            .child(span("]", VIOLET))
    };
    let mut items = vec![
        item(vec![
            span(t!("ircoid-status-score"), CYAN),
            span(format!(" {:06}", game.score), WHITE_HOT),
        ]),
        item(vec![
            span(t!("ircoid-status-lives"), CYAN),
            span(" ", WHITE_HOT),
            span("♥".repeat(game.lives as usize), PINK),
        ]),
        item(vec![
            span(format!("{}:", game.level), WHITE_HOT),
            span(game.channel(), CYAN),
        ]),
    ];
    for (label, seconds) in running(game) {
        let mut parts = vec![span(label, CYAN)];
        if let Some(seconds) = seconds {
            parts.push(span(format!(" {seconds:02}s"), SUN));
        }
        items.push(item(parts));
    }
    div()
        .id("ircoid-status")
        .test_support()
        .aria_label(SharedString::from(line))
        .absolute()
        .left_0()
        .top(px(FIELD_H * k))
        .w_full()
        .h(px(BAR_H * k))
        .px(px(6. * k))
        .flex()
        .items_center()
        .gap(px(5. * k))
        .overflow_hidden()
        .font_family(stage::FONT)
        .font_weight(FontWeight::BOLD)
        .text_size(px(8.5 * k))
        .children(items)
}

/// The IRC line over the stage for the phase, if any: its ID, its lines and
/// its colour.
fn notice(this: &Ircoid) -> Option<(&'static str, Vec<String>, u32)> {
    use stage::palette::*;
    let game = &this.game;
    match this.phase {
        Phase::Ready => Some((
            "ircoid-notice-ready",
            vec![t!("ircoid-notice-ready", channel = game.channel())],
            MINT,
        )),
        Phase::Paused => Some((
            "ircoid-notice-paused",
            vec![t!("ircoid-notice-paused").to_string()],
            CYAN,
        )),
        Phase::Over => Some((
            "ircoid-notice-over",
            vec![
                t!("ircoid-notice-over").to_string(),
                t!("ircoid-notice-score", score = game.score),
            ],
            RED,
        )),
        Phase::Playing if this.level_notice > 0. => Some((
            "ircoid-notice-level",
            vec![t!(
                "ircoid-notice-level",
                level = game.level,
                channel = game.channel()
            )],
            SUN,
        )),
        Phase::Playing => None,
    }
}

/// The notice, in the middle of the floor.
fn notice_box(id: &'static str, lines: Vec<String>, hex: u32, width: f32) -> impl IntoElement {
    let k = stage::scale(width);
    let label = lines.join(" ");
    div()
        .absolute()
        .left_0()
        .w_full()
        .top(px(196. * k))
        .flex()
        .justify_center()
        .child(
            div()
                .id(id)
                .test_support()
                .aria_label(SharedString::from(label))
                .px(px(8. * k))
                .py(px(3. * k))
                .rounded(px(3. * k))
                .bg(stage::color(stage::palette::BAR, 0.82))
                .border_1()
                .border_color(stage::color(hex, 0.5))
                .flex()
                .flex_col()
                .items_center()
                .font_family(stage::FONT)
                .font_weight(FontWeight::BOLD)
                .text_size(px(9. * k))
                .text_color(stage::color(hex, 1.))
                .children(lines),
        )
}

/// A capsule's label: its IRC command or mode.
pub fn capsule_label(kind: rules::CapsuleKind) -> &'static str {
    use rules::CapsuleKind::*;
    match kind {
        Op => t!("ircoid-capsule-op"),
        Join => t!("ircoid-capsule-join"),
        Moderated => t!("ircoid-capsule-moderated"),
        Kick => t!("ircoid-capsule-kick"),
        Bouncer => t!("ircoid-capsule-bouncer"),
        Lag => t!("ircoid-capsule-lag"),
    }
}

impl Render for Ircoid {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.frame(window);
        let theme = theme::get(cx).clone();
        self.reduced = theme.reduced_motion;
        let scroll = if self.reduced { 0. } else { self.travel };
        let (game, fx, reduced) = (self.game.clone(), self.fx.clone(), self.reduced);
        // Labels sit on the stage as text; they are placed by the width the
        // stage was last drawn at (a guess before the first frame).
        let width = self
            .stage
            .get()
            .map(|b| f32::from(b.size.width))
            .unwrap_or(MAX_WIDTH - 48.);
        let labels = stage_labels(&self.game, width);
        let status = status_bar(&self.game, width, self.status());
        let notice = notice(self).map(|(id, lines, hex)| notice_box(id, lines, hex, width));
        let hints = crate::ui::page::hint_bar(
            "ircoid-hints".into(),
            &[
                crate::ui::page::Hint::new(
                    &[t!("ircoid-key-left"), t!("ircoid-key-right")],
                    t!("ircoid-hint-move"),
                ),
                crate::ui::page::Hint::new(&[t!("ircoid-key-space")], t!("ircoid-hint-launch")),
                crate::ui::page::Hint::new(&[t!("ircoid-key-p")], t!("ircoid-hint-pause")),
                crate::ui::page::Hint::new(&[t!("ircoid-key-escape")], t!("ircoid-back")),
            ],
            &theme,
        );
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
                    .bg(stage::color(stage::palette::VOID, 1.))
                    .on_mouse_move(cx.listener(|this, e: &MouseMoveEvent, _, cx| {
                        this.pointer_moved(e.position, cx);
                    }))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, e: &MouseDownEvent, window, cx| {
                            this.pointer_moved(e.position, cx);
                            this.primary(cx);
                            window.focus(&this.focus, cx);
                            cx.stop_propagation();
                        }),
                    )
                    .on_scroll(cx.listener(|this, e: &ScrollEvent, window, cx| {
                        let delta = e.delta.pixel_delta(window.line_height());
                        if delta.x.abs() > delta.y.abs() {
                            this.scrolled(delta.x, cx);
                            cx.stop_propagation();
                        }
                    }))
                    .child({
                        let stage = self.stage.clone();
                        canvas(
                            move |bounds, window, _| {
                                // A first or new size places the labels again.
                                if stage.replace(Some(bounds)).map(|b| b.size) != Some(bounds.size)
                                {
                                    window.refresh();
                                }
                            },
                            move |bounds, _, window, _| {
                                let (w, h) =
                                    (f32::from(bounds.size.width), f32::from(bounds.size.height));
                                stage::paint(&stage::backdrop(w, h, scroll), bounds.origin, window);
                                let scene = stage::scene(&game, w, &fx, reduced);
                                stage::paint(scene.iter().map(|(_, s)| s), bounds.origin, window);
                            },
                        )
                        .absolute()
                        .size_full()
                    })
                    .children(labels)
                    .child(status)
                    .children(notice)
                    .child(
                        canvas(
                            |_, _, _| (),
                            |bounds, _, window, _| {
                                let (w, h) =
                                    (f32::from(bounds.size.width), f32::from(bounds.size.height));
                                stage::paint(&stage::overlay(w, h), bounds.origin, window);
                            },
                        )
                        .absolute()
                        .size_full(),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_3()
                    .child(div().flex_1().min_w_0().child(hints))
                    .child(self.buttons(cx)),
            )
            .child(
                // The one live region: level, lives, capsules, game over.
                div()
                    .id("ircoid-announcement")
                    .test_support()
                    .role(Role::Status)
                    .aria_label(SharedString::from(self.announcement.clone()))
                    .a11y_synthetic_children(|tree| {
                        tree.parent_node().set_live(accesskit::Live::Polite);
                    })
                    .absolute()
                    .size_0()
                    .overflow_hidden(),
            )
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::{Presses, field_x};

    #[test]
    fn the_pointer_maps_to_the_field() {
        assert_eq!(field_x(100., 100., 800.), Some(0.));
        assert_eq!(field_x(500., 100., 800.), Some(200.));
        assert_eq!(field_x(900., 100., 800.), Some(400.));
        assert_eq!(field_x(99., 100., 800.), None);
        assert_eq!(field_x(901., 100., 800.), None);
    }

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

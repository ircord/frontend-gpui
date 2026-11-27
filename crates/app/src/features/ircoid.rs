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
use gpui_kit::component::Sizable;
use gpui_kit::component::WindowExt;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::dialog::Cancel;
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::prelude::*;
use gpui_kit::*;
use ircord_client::ircoid::{self as rules, FIELD_H, FIELD_W, Game, Input, MAX_DT};

use crate::session::Session;
use crate::theme;

pub mod credits;
mod scores;
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
/// Play again ignores a press this soon after the game ends: Space may still
/// be held.
pub const AGAIN_GUARD: Duration = Duration::from_millis(500);
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
            // Escape in the name field skips the entry instead of closing
            // the game.
            .on_cancel({
                let game = view.clone();
                move |_, window, cx| game.update(cx, |game, cx| !game.skip_entry(window, cx))
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

/// The credits screen while it is open.
struct CreditsState {
    opened: Instant,
    page: usize,
    page_at: Instant,
    names: Vec<&'static str>,
    field: Vec<credits::Star>,
}

impl CreditsState {
    fn pages(&self) -> usize {
        rules::credits::pages(self.names.len())
    }
}

/// The keys held down.
#[derive(Debug, Default, Clone, Copy)]
struct Held {
    left: bool,
    right: bool,
}

/// The open game.
pub struct Ircoid {
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
    /// The high score table.
    scores: scores::Scores,
    /// The game just ended; the table loads on the next update.
    just_over: bool,
    /// When the game ended.
    over_at: Option<Instant>,
    /// Focus goes back to the game on the next frame (the name field went).
    refocus: bool,
    /// The credits, while they are open.
    credits: Option<CreditsState>,
    /// The `/credits` line's own focus, so Escape can put focus back on it.
    credits_focus: FocusHandle,
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
                // The credits move again when the window comes back.
                if this.credits.is_some() {
                    cx.notify();
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
            scores: scores::Scores::default(),
            just_over: false,
            over_at: None,
            refocus: false,
            credits: None,
            credits_focus: cx.focus_handle().tab_stop(true),
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
        self.after_step(cx);
        cx.notify();
    }

    /// At game over, the table loads and opens.
    fn after_step(&mut self, cx: &mut Context<Self>) {
        if std::mem::take(&mut self.just_over) {
            self.scores.shown = true;
            self.load_scores(Some(self.game.score), cx);
        }
    }

    /// Whether the table is drawn over the stage.
    pub fn scores_shown(&self) -> bool {
        self.scores.shown
    }

    /// Whether the credits are open.
    pub fn credits_open(&self) -> bool {
        self.credits.is_some()
    }

    /// The `/credits` line: the credits replace the table on the stage.
    pub fn open_credits(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let names = rules::credits::contributors();
        self.announcement = t!("ircoid-credits-stage", names = names.join(", "));
        let now = Instant::now();
        self.credits = Some(CreditsState {
            opened: now,
            page: 0,
            page_at: now,
            names,
            field: credits::star_field(),
        });
        window.focus(&self.focus, cx);
        cx.notify();
    }

    /// Escape or Back to high scores: the table as it was, focus on the line.
    pub fn close_credits(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.credits.take().is_some() {
            window.focus(&self.credits_focus, cx);
            cx.notify();
        }
    }

    /// Left and Right: another page of names.
    fn turn_page(&mut self, by: isize, cx: &mut Context<Self>) {
        if let Some(c) = &mut self.credits {
            let pages = c.pages() as isize;
            if pages > 1 {
                c.page = (c.page as isize + by).rem_euclid(pages) as usize;
                c.page_at = Instant::now();
                cx.notify();
            }
        }
    }

    /// The High scores button: pause, and show the table.
    pub fn show_scores(&mut self, cx: &mut Context<Self>) {
        self.pause(cx);
        self.scores.shown = true;
        if self.phase != Phase::Over || self.scores.table == scores::Table::Unavailable {
            self.load_scores(None, cx);
        }
        cx.notify();
    }

    fn hide_scores(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.scores.shown = false;
        window.focus(&self.focus, cx);
        cx.notify();
    }

    /// Loads the table; with `asking`, a score that places asks for a name.
    fn load_scores(&mut self, asking: Option<u32>, cx: &mut Context<Self>) {
        self.scores.generation += 1;
        let generation = self.scores.generation;
        if self.scores.table != scores::Table::Unavailable {
            self.scores.table = scores::Table::Loading;
        }
        let Some(client) = self.session.read(cx).client().cloned() else {
            self.scores.table = scores::Table::Unavailable;
            return;
        };
        let task = client.request(|api| async move { api.ircoid_scores().await });
        cx.spawn(async move |this, cx| {
            let result = task.await.ok().and_then(Result::ok);
            let _ = this.update(cx, |this, cx| {
                if this.scores.generation != generation {
                    return;
                }
                let table = match result {
                    Some(table) => scores::Table::Ready(table.items.unwrap_or_default()),
                    None => scores::Table::Unavailable,
                };
                this.scores.loaded(table, asking.filter(|s| *s > 0));
                cx.notify();
            });
        })
        .detach();
    }

    /// The name the table offers: the person's display name, clipped.
    fn default_name(&self, cx: &App) -> String {
        let name = self
            .session
            .read(cx)
            .client()
            .map(|client| {
                let w = client.read();
                let user = &w.user;
                [&user.display_name, &user.nick, &user.account]
                    .into_iter()
                    .find(|n| !n.trim().is_empty())
                    .cloned()
                    .unwrap_or_default()
            })
            .unwrap_or_default();
        rules::scores::clip_name(&name)
    }

    /// Makes the name field a placing score asked for.
    fn make_entry(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // On the credits the field would be out of sight: it waits for the
        // table to show again.
        if !self.scores.wanted || self.scores.entry.is_some() || self.credits.is_some() {
            return;
        }
        self.scores.wanted = false;
        let name = self.default_name(cx);
        let input = cx.new(|cx| {
            let mut state = InputState::new(window, cx);
            state.set_value(name, window, cx);
            state
        });
        self._subscriptions.push(cx.subscribe_in(
            &input,
            window,
            |this: &mut Self, _, event: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    this.save(window, cx);
                }
            },
        ));
        let focus = input.read(cx).focus_handle(cx);
        self.scores.entry = Some(input);
        window.focus(&focus, cx);
    }

    /// Save: the name is checked, then sent with the score.
    pub fn save(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(input) = self.scores.entry.clone() else {
            return;
        };
        if self.scores.saving {
            return;
        }
        let raw = input.read(cx).value().to_string();
        let name = match rules::scores::table_name(&raw) {
            Ok(name) => name,
            Err(error) => {
                self.scores.error = Some(scores::name_error(error));
                cx.notify();
                return;
            }
        };
        let Some(client) = self.session.read(cx).client().cloned() else {
            return;
        };
        self.scores.error = None;
        self.scores.saving = true;
        self.scores.generation += 1;
        let generation = self.scores.generation;
        let score = self.scores.score;
        let task =
            client.request(move |api| async move { api.submit_ircoid_score(&name, score).await });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.scores.generation != generation {
                    return;
                }
                this.scores.saving = false;
                match result {
                    Ok(Ok(result)) => {
                        this.scores.table = scores::Table::Ready(result.items.unwrap_or_default());
                        this.scores.entry = None;
                        if result.placed {
                            this.scores.mark = Some(result.rank);
                            this.scores.outcome = Some(scores::Outcome::Saved(result.rank));
                        } else {
                            this.scores.outcome = Some(scores::Outcome::TookPlace);
                        }
                        this.refocus = true;
                    }
                    Ok(Err(ircord_client::api::ApiError::Http {
                        status: 422,
                        detail,
                    })) => {
                        this.scores.error = Some(detail);
                    }
                    Ok(Err(e)) => {
                        this.scores.error = Some(t!("ircoid-save-failed", reason = e.to_string()));
                    }
                    Err(e) => {
                        this.scores.error = Some(t!("ircoid-save-failed", reason = e.to_string()));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Skip, or Escape in the name field: no entry, the table stays. True
    /// when there was an entry to skip.
    pub fn skip_entry(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !self.scores.asking() {
            return false;
        }
        // A save on its way is not dropped: its answer shows the table.
        if self.scores.saving {
            return true;
        }
        self.scores.entry = None;
        self.scores.wanted = false;
        self.scores.error = None;
        self.scores.saving = false;
        self.scores.generation += 1;
        window.focus(&self.focus, cx);
        cx.notify();
        true
    }

    /// Play again, which ignores a press in the first moment after the end.
    pub fn play_again(&mut self, cx: &mut Context<Self>) {
        if self.over_at.is_some_and(|at| at.elapsed() < AGAIN_GUARD) {
            return;
        }
        self.restart(cx);
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
            self.just_over = true;
            self.over_at = Some(Instant::now());
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
        // Under the table the game stays still; Back returns to it.
        if self.scores.shown {
            return;
        }
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
        self.scores.reset();
        self.just_over = false;
        self.over_at = None;
        self.set_phase(Phase::Playing);
        cx.notify();
    }

    /// Space, Up or Enter: start from the ready screen and launch, resume
    /// from the paused one, or launch while playing.
    fn primary(&mut self, cx: &mut Context<Self>) {
        // Over the table or the credits the game's keys do nothing.
        if self.scores.shown {
            return;
        }
        match self.phase {
            Phase::Ready => {
                self.set_phase(Phase::Playing);
                self.launch = true;
            }
            Phase::Paused => self.set_phase(Phase::Playing),
            Phase::Playing => self.launch = true,
            // Only Play again starts a new game: Space may still be held.
            Phase::Over => return,
        }
        cx.notify();
    }

    fn key_down(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) -> bool {
        if self.credits.is_some() {
            match event.keystroke.key.as_str() {
                "left" => self.turn_page(-1, cx),
                "right" => self.turn_page(1, cx),
                _ => return false,
            }
            return true;
        }
        // The name field and the table take their own keys.
        if self.scores.shown {
            return false;
        }
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
        let x = self.pointer.unwrap_or(self.game.paddle.x) + units;
        // The game keeps the paddle inside the walls; keep the target near
        // them too, so scrolling back answers at once.
        let half = self.game.paddle.w / 2.;
        self.pointer = Some(x.clamp(half, FIELD_W - half));
        cx.notify();
    }

    /// While playing, step by the time since the last frame and ask for the
    /// next one.
    fn frame(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.make_entry(window, cx);
        if std::mem::take(&mut self.refocus) {
            window.focus(&self.focus, cx);
        }
        let reduced = self.reduced;
        if let Some(c) = &mut self.credits {
            if !reduced && window.is_window_active() {
                if c.pages() > 1 && c.page_at.elapsed().as_secs_f64() >= credits::PAGE_SECS {
                    c.page = (c.page + 1) % c.pages();
                    c.page_at = Instant::now();
                }
                window.request_animation_frame();
            }
            return;
        }
        if self.phase != Phase::Playing {
            return;
        }
        let now = Instant::now();
        if let Some(last) = self.last_frame {
            self.step(now.saturating_duration_since(last).as_secs_f32());
            self.after_step(cx);
        }
        if self.phase == Phase::Playing {
            self.last_frame = Some(now);
            window.request_animation_frame();
        }
    }

    fn buttons(&self, cx: &mut Context<Self>) -> Div {
        if self.credits.is_some() {
            return div().flex().child(
                Button::new("ircoid-credits-back")
                    .small()
                    .label(t!("ircoid-credits-back"))
                    .on_click(cx.listener(|this, _, window, cx| this.close_credits(window, cx))),
            );
        }
        // Under the table only its own Back starts the game again.
        let primary = match self.phase {
            _ if self.scores.shown => None,
            Phase::Ready => Some((t!("ircoid-start"), false)),
            Phase::Playing => Some((t!("ircoid-pause"), true)),
            Phase::Paused => Some((t!("ircoid-resume"), false)),
            Phase::Over => None,
        };
        let again = (self.phase == Phase::Over).then(|| {
            Button::new("ircoid-again")
                .small()
                .primary()
                .label(t!("ircoid-play-again"))
                .on_click(cx.listener(|this, _, window, cx| {
                    this.play_again(cx);
                    window.focus(&this.focus, cx);
                }))
        });
        div()
            .flex()
            .gap_2()
            .children(primary.map(|(label, pause)| {
                Button::new("ircoid-play")
                    .small()
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
            .children(again)
            .child(
                Button::new("ircoid-scores")
                    .small()
                    .label(t!("ircoid-scores"))
                    .on_click(cx.listener(|this, _, _, cx| this.show_scores(cx))),
            )
            // At game over only Play again starts a new game.
            .when(self.phase != Phase::Over, |d| {
                d.child(
                    Button::new("ircoid-restart")
                        .small()
                        .label(t!("ircoid-restart"))
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.restart(cx);
                            window.focus(&this.focus, cx);
                        })),
                )
            })
            .child(
                Button::new("ircoid-back")
                    .small()
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
            parts.push(span(
                format!(
                    " {}",
                    t!("ircoid-status-seconds", seconds = format!("{seconds:02}"))
                ),
                SUN,
            ));
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

/// A line of text centred on `cy` (units) across the stage, `size` units
/// high, without wrapping.
fn centred_line(cy: f32, size: f32, k: f32) -> Div {
    div()
        .absolute()
        .left_0()
        .w_full()
        .top(px((cy - size * credits::LINE_SHARE / 2.) * k))
        .h(px(size * credits::LINE_SHARE * k))
        .flex()
        .justify_center()
        .items_center()
        .whitespace_nowrap()
        .font_family(stage::FONT)
        .font_weight(FontWeight::BOLD)
        .text_size(px(size * k))
        .line_height(px(size * credits::LINE_SHARE * k))
}

/// The credits' text over their picture: the presents line, the tagline,
/// the box's title, the names and the scroller (a paragraph with reduced
/// motion).
fn credits_layers(c: &CreditsState, width: f32, t: f64, reduced: bool) -> Vec<AnyElement> {
    use stage::palette::*;
    let k = stage::scale(width);
    let rgba = |c: Rgba| -> Hsla { c.into() };
    let mut out: Vec<AnyElement> = Vec::new();
    let presents = credits::mix(rgb(CYAN), rgb(0xffffff), 0.35);
    out.push(
        centred_line(9., 6.5, k)
            .text_color(rgba(presents))
            .child(credits::spaced(t!("ircoid-credits-presents")))
            .into_any_element(),
    );
    out.push(
        centred_line(84., 6.5, k)
            .text_color(stage::color(WHITE_HOT, 0.92))
            .child(t!("ircoid-credits-tagline").to_uppercase())
            .into_any_element(),
    );
    // The box's title, on a patch of the box's colour that cuts its border.
    let size = credits::TITLE_SIZE;
    out.push(
        div()
            .absolute()
            .left(px(credits::TITLE_X * k))
            .top(px((credits::BOX.y - size * credits::LINE_SHARE / 2.) * k))
            .h(px(size * credits::LINE_SHARE * k))
            .px(px(3. * k))
            .bg(stage::color(BAR, 1.))
            .flex()
            .items_center()
            .whitespace_nowrap()
            .font_family(stage::FONT)
            .font_weight(FontWeight::BOLD)
            .text_size(px(size * k))
            .child(span("[ ", VIOLET))
            .child(span(t!("ircoid-credits-channel"), CYAN))
            .child(span(" ]", VIOLET))
            .into_any_element(),
    );
    // The names, typed out unless motion is reduced. The rest of each name
    // is there but clear, so a row does not shift while it is typed.
    let page = rules::credits::page(&c.names, c.page);
    let typed = (!reduced).then(|| credits::typed(c.page_at.elapsed().as_secs_f64()));
    for row in credits::names(page, typed) {
        let size = row.size;
        let shown = row.shown.saturating_sub(1);
        let name: String = row.name.chars().take(shown).collect();
        let rest: String = row.name.chars().skip(shown).collect();
        out.push(
            div()
                .absolute()
                .left(px((row.cx - 100.) * k))
                .w(px(200. * k))
                .top(px((row.cy - size * credits::LINE_SHARE / 2.) * k))
                .h(px(size * credits::LINE_SHARE * k))
                .flex()
                .justify_center()
                .items_center()
                .whitespace_nowrap()
                .font_family(stage::FONT)
                .font_weight(FontWeight::BOLD)
                .text_size(px(size * k))
                .child(
                    div()
                        .text_color(stage::color(SUN, if row.shown > 0 { 1. } else { 0. }))
                        .child(t!("ircoid-glyph-op")),
                )
                .child(span(name, WHITE_HOT))
                .child(div().text_color(stage::color(WHITE_HOT, 0.)).child(rest))
                .into_any_element(),
        );
    }
    let parts = credits_parts(&c.names);
    if reduced {
        let size = 7.;
        out.push(
            div()
                .absolute()
                .left(px(16. * k))
                .w(px(368. * k))
                .top(px(credits::BAND_TOP * k))
                .h(px((credits::BAND_BOTTOM - credits::BAND_TOP) * k))
                .overflow_hidden()
                .flex()
                .items_center()
                .justify_center()
                .text_center()
                .font_family(stage::FONT)
                .font_weight(FontWeight::BOLD)
                .text_size(px(size * k))
                .line_height(px(10.5 * k))
                .text_color(stage::color(WHITE_HOT, 0.92))
                .child(credits::paragraph(&parts))
                .into_any_element(),
        );
    } else {
        let text = credits::scroll_text(&parts);
        let size = credits::SCROLL_SIZE;
        let (w, h) = (size * credits::CELL_SHARE, size * credits::LINE_SHARE);
        for glyph in credits::scroller(&text, t) {
            out.push(
                div()
                    .absolute()
                    .left(px((glyph.cx - w / 2.) * k))
                    .top(px((glyph.cy - h / 2.) * k))
                    .w(px(w * k))
                    .h(px(h * k))
                    .flex()
                    .justify_center()
                    .items_center()
                    .font_family(stage::FONT)
                    .font_weight(FontWeight::BOLD)
                    .text_size(px(size * k))
                    .line_height(px(h * k))
                    .text_color(rgba(glyph.color))
                    .child(glyph.ch.to_string())
                    .into_any_element(),
            );
        }
    }
    out
}

/// The scroller's eight parts, in sentence case.
fn credits_parts(names: &[&str]) -> Vec<String> {
    let names = names.join(", ");
    vec![
        t!("ircoid-credits-scroll-1").to_string(),
        t!("ircoid-credits-scroll-2", names = names),
        t!("ircoid-credits-scroll-3").to_string(),
        t!("ircoid-credits-scroll-4").to_string(),
        t!("ircoid-credits-scroll-5").to_string(),
        t!("ircoid-credits-scroll-6").to_string(),
        t!("ircoid-credits-scroll-7").to_string(),
        t!("ircoid-credits-scroll-8").to_string(),
    ]
}

/// The status bar on the credits: `[#credits] [ops 2]`, and the page when
/// there is more than one.
fn credits_status(c: &CreditsState, width: f32) -> impl IntoElement {
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
        item(vec![span(t!("ircoid-credits-channel"), CYAN)]),
        item(vec![
            span(t!("ircoid-credits-ops"), CYAN),
            span(format!(" {}", c.names.len()), WHITE_HOT),
        ]),
    ];
    let mut line = format!(
        "{} {} {}",
        t!("ircoid-credits-channel"),
        t!("ircoid-credits-ops"),
        c.names.len()
    );
    if c.pages() > 1 {
        let page = format!("{}/{}", c.page + 1, c.pages());
        line.push_str(&format!(" {} {page}", t!("ircoid-credits-page")));
        items.push(item(vec![
            span(t!("ircoid-credits-page"), CYAN),
            span(format!(" {page}"), WHITE_HOT),
        ]));
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
        let theme = theme::get(cx).clone();
        self.reduced = theme.reduced_motion;
        self.frame(window, cx);
        let scroll = if self.reduced { 0. } else { self.travel };
        let (game, fx, reduced) = (self.game.clone(), self.fx.clone(), self.reduced);
        // Labels sit on the stage as text; they are placed by the width the
        // stage was last drawn at (a guess before the first frame).
        let width = self
            .stage
            .get()
            .map(|b| f32::from(b.size.width))
            .unwrap_or(MAX_WIDTH - 48.);
        // The credits replace the game's picture, its text and the table.
        let credits_t = self.credits.as_ref().map(|c| {
            if self.reduced {
                0.
            } else {
                c.opened.elapsed().as_secs_f64()
            }
        });
        let field = self.credits.as_ref().map(|c| c.field.clone());
        let mut layers: Vec<AnyElement> = Vec::new();
        if let (Some(c), Some(t)) = (&self.credits, credits_t) {
            layers.extend(credits_layers(c, width, t, self.reduced));
            layers.push(credits_status(c, width).into_any_element());
        } else {
            layers.extend(
                stage_labels(&self.game, width)
                    .into_iter()
                    .map(IntoElement::into_any_element),
            );
            layers.push(status_bar(&self.game, width, self.status()).into_any_element());
        }
        let stage_name = match &self.credits {
            Some(c) => SharedString::from(t!("ircoid-credits-stage", names = c.names.join(", "))),
            None => SharedString::from(t!("ircoid-stage")),
        };
        let over_lines = notice(self).filter(|(id, _, _)| *id == "ircoid-notice-over");
        let notice = notice(self)
            .filter(|_| !self.scores.shown && self.credits.is_none())
            .map(|(id, lines, hex)| notice_box(id, lines, hex, width));
        let line = (!self.scores.asking()).then(|| {
            let ring = theme.accent;
            div()
                .id("ircoid-credits-open")
                .test_support()
                .role(Role::Button)
                .aria_label(t!("ircoid-credits-open"))
                .track_focus(&self.credits_focus)
                .focus_visible(move |s| {
                    s.shadow(vec![BoxShadow {
                        color: ring,
                        offset: point(px(0.), px(0.)),
                        blur_radius: px(0.),
                        spread_radius: px(2.),
                        inset: true,
                    }])
                    .text_color(stage::color(stage::palette::PINK, 1.))
                })
                .mx_auto()
                .px_2()
                .rounded_sm()
                .cursor_pointer()
                .font_family(stage::FONT)
                .font_weight(FontWeight::BOLD)
                .text_xs()
                .text_color(theme.faint)
                .hover(|s| s.text_color(stage::color(stage::palette::PINK, 1.)))
                .on_click(cx.listener(|this, _, window, cx| this.open_credits(window, cx)))
                .child(t!("ircoid-credits-open"))
                .into_any_element()
        });
        let overlay = (self.scores.shown && self.credits.is_none()).then(|| {
            let (back, save, skip) = (
                cx.listener(|this: &mut Self, _: &ClickEvent, window, cx| {
                    this.hide_scores(window, cx)
                }),
                cx.listener(|this: &mut Self, _: &ClickEvent, window, cx| this.save(window, cx)),
                cx.listener(|this: &mut Self, _: &ClickEvent, window, cx| {
                    this.skip_entry(window, cx);
                }),
            );
            let over = over_lines.map(|(_, lines, _)| lines);
            scores::overlay(
                &self.scores,
                &theme,
                scores::Extras {
                    back: self.phase != Phase::Over,
                    over,
                    credits_line: line,
                },
                back,
                save,
                skip,
            )
        });
        let credit_hints = self.credits.as_ref().map(|c| {
            let mut hints = vec![crate::ui::page::Hint::new(
                &[t!("ircoid-key-escape")],
                t!("ircoid-credits-back"),
            )];
            if c.pages() > 1 {
                hints.push(crate::ui::page::Hint::new(
                    &[t!("ircoid-key-left"), t!("ircoid-key-right")],
                    t!("ircoid-credits-hint-page"),
                ));
            }
            hints
        });
        let hints = crate::ui::page::hint_bar(
            "ircoid-hints".into(),
            credit_hints.as_deref().unwrap_or(&[
                crate::ui::page::Hint::new(
                    &[t!("ircoid-key-left"), t!("ircoid-key-right")],
                    t!("ircoid-hint-move"),
                ),
                crate::ui::page::Hint::new(&[t!("ircoid-key-space")], t!("ircoid-hint-launch")),
                crate::ui::page::Hint::new(&[t!("ircoid-key-p")], t!("ircoid-hint-pause")),
                crate::ui::page::Hint::new(&[t!("ircoid-key-escape")], t!("ircoid-back")),
            ]),
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
            // Escape on the credits goes back to the table, before the
            // dialog's own Cancel would close the game.
            .when(self.credits.is_some(), |d| {
                d.on_action(
                    cx.listener(|this, _: &Cancel, window, cx| this.close_credits(window, cx)),
                )
            })
            .child(
                div()
                    .relative()
                    .w_full()
                    .child(
                        div()
                            .id("ircoid-stage")
                            .test_support()
                            .role(Role::Image)
                            .aria_label(stage_name)
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
                                    // The table over the stage takes its own clicks.
                                    if this.scores.shown {
                                        return;
                                    }
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
                                        if stage.replace(Some(bounds)).map(|b| b.size)
                                            != Some(bounds.size)
                                        {
                                            window.refresh();
                                        }
                                    },
                                    move |bounds, _, window, _| {
                                        let (w, h) = (
                                            f32::from(bounds.size.width),
                                            f32::from(bounds.size.height),
                                        );
                                        if let (Some(field), Some(t)) = (&field, credits_t) {
                                            let k = stage::scale(w);
                                            let shapes: Vec<stage::Shape> =
                                                credits::picture(field, t)
                                                    .iter()
                                                    .map(|s| credits::scaled(s, k))
                                                    .collect();
                                            stage::paint(&shapes, bounds.origin, window);
                                            return;
                                        }
                                        stage::paint(
                                            &stage::backdrop(w, h, scroll),
                                            bounds.origin,
                                            window,
                                        );
                                        let scene = stage::scene(&game, w, &fx, reduced);
                                        stage::paint(
                                            scene.iter().map(|(_, s)| s),
                                            bounds.origin,
                                            window,
                                        );
                                    },
                                )
                                .absolute()
                                .size_full()
                            })
                            .children(layers)
                            .children(notice)
                            .child(
                                canvas(
                                    |_, _, _| (),
                                    |bounds, _, window, _| {
                                        let (w, h) = (
                                            f32::from(bounds.size.width),
                                            f32::from(bounds.size.height),
                                        );
                                        stage::paint(&stage::overlay(w, h), bounds.origin, window);
                                    },
                                )
                                .absolute()
                                .size_full(),
                            ),
                    )
                    .children(overlay),
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

//! Ircoid, the brick-breaking game behind About's logo: its rules, with no
//! drawing in them.
//!
//! Everything is in game units: the field is [`FIELD_W`] × [`FIELD_H`], the
//! origin top left, y down. The app scales units to pixels, holds the keys
//! and calls [`Game::step`] once a frame; the game answers with the
//! [`Events`] a frame drew or announced.
//!
//! A step is cut into slices in which a ball moves at most [`SLICE`] units, so
//! it never passes through a wall at the top speed and the longest frame.

use std::f32::consts::PI;

pub mod levels;

/// The field's width.
pub const FIELD_W: f32 = 400.0;
/// The field's height. It is open at the bottom: a ball past it is lost.
pub const FIELD_H: f32 = 300.0;
/// The walls on the left, right and top.
pub const WALL: f32 = 2.0;

/// The paddle's width.
pub const PADDLE_W: f32 = 56.0;
/// The paddle's height.
pub const PADDLE_H: f32 = 8.0;
/// The paddle's top edge.
pub const PADDLE_Y: f32 = 276.0;
/// How fast the keys move the paddle, in units a second.
pub const PADDLE_SPEED: f32 = 300.0;

/// The ball's radius.
pub const BALL_R: f32 = 3.5;
/// The ball's speed on level 1, in units a second.
pub const BALL_SPEED: f32 = 170.0;
/// How much faster each level is.
pub const BALL_SPEED_STEP: f32 = 10.0;
/// The fastest the levels make the ball.
pub const BALL_SPEED_MAX: f32 = 290.0;

/// The longest a ball moves in one slice of a step.
pub const SLICE: f32 = 2.0;
/// The longest step: a slower frame plays in slow motion, as the web does.
pub const MAX_DT: f32 = 1.0 / 30.0;

/// Lives at the start of a game.
pub const LIVES: u32 = 3;

/// A brick's width and height.
pub const BRICK_W: f32 = 28.0;
pub const BRICK_H: f32 = 11.0;
/// From one brick to the next, across and down.
pub const BRICK_PITCH_X: f32 = 30.0;
pub const BRICK_PITCH_Y: f32 = 13.0;
/// The top left of the first brick.
pub const BRICK_X0: f32 = 5.0;
pub const BRICK_Y0: f32 = 30.0;

/// Points are multiplied by the level, up to this.
pub const MAX_MULTIPLIER: u32 = 20;
/// Clearing a level adds this times the level.
pub const CLEAR_BONUS: u32 = 500;
/// The highest score, the bound the server's score table accepts.
pub const MAX_SCORE: u32 = 10_000_000;

/// A capsule's width and height.
pub const CAPSULE_W: f32 = 26.0;
pub const CAPSULE_H: f32 = 10.0;
/// How fast a capsule falls, in units a second.
pub const CAPSULE_SPEED: f32 = 70.0;
/// A broken brick drops a capsule one time in this many...
pub const DROP_ONE_IN: u32 = 8;
/// ...and at most this many fall at once.
pub const MAX_CAPSULES: usize = 2;
/// Catching a capsule adds this times the level.
pub const CATCH_POINTS: u32 = 50;
/// The paddle's width with Op.
pub const PADDLE_W_OP: f32 = 84.0;
/// Join makes no more balls than this.
pub const MAX_BALLS: usize = 6;
/// How long the timed capsules last, in seconds of game time.
pub const OP_SECS: f32 = 15.0;
pub const MODERATED_SECS: f32 = 10.0;
pub const KICK_SECS: f32 = 8.0;
pub const LAG_SECS: f32 = 8.0;
/// Moderated and Lag change the ball's speed by these.
pub const MODERATED_SPEED: f32 = 0.7;
pub const LAG_SPEED: f32 = 1.3;
/// The Bouncer's line, just above the floor.
pub const BOUNCER_Y: f32 = FIELD_H - 4.0;
/// A life every this many points...
pub const LIFE_EVERY: u32 = 20_000;
/// ...up to this many.
pub const MAX_LIVES: u32 = 5;
/// Join's new balls leave this far either side of the old one, in degrees.
const JOIN_ANGLE: f32 = 20.0;

/// A launch leaves the paddle this far off vertical, in degrees.
const LAUNCH_ANGLE: f32 = 15.0;
/// A ball off the paddle's end leaves at this angle off vertical, in degrees.
const BOUNCE_ANGLE: f32 = 60.0;
/// After a bounce the vertical part of the speed is at least this share.
const MIN_VERTICAL: f32 = 0.25;

/// A small seeded generator (xorshift64), so a game is repeatable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        // splitmix64 spreads near seeds (1, 2, 3…) apart.
        let mut z = seed.wrapping_add(0x9e37_79b9_7f4a_7c15);
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^= z >> 31;
        Self(if z == 0 { 0x9e37_79b9_7f4a_7c15 } else { z })
    }

    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    /// A number below `n`.
    pub fn below(&mut self, n: u32) -> u32 {
        (self.next_u64() % u64::from(n.max(1))) as u32
    }
}

/// The paddle: its centre and width.
#[derive(Debug, Clone, PartialEq)]
pub struct Paddle {
    pub x: f32,
    pub w: f32,
}

impl Paddle {
    pub fn left(&self) -> f32 {
        self.x - self.w / 2.0
    }

    pub fn right(&self) -> f32 {
        self.x + self.w / 2.0
    }
}

/// What a brick is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrickKind {
    /// One hit.
    Message,
    /// `@`: two hits.
    Op,
    /// `~`: three hits.
    Owner,
    /// `+b`: never breaks, and does not count toward clearing a level.
    Ban,
}

impl BrickKind {
    /// Hits it takes to break.
    pub fn hits(self) -> u8 {
        match self {
            Self::Message => 1,
            Self::Op => 2,
            Self::Owner => 3,
            Self::Ban => u8::MAX,
        }
    }

    /// Points for breaking it on level 1.
    pub fn points(self) -> u32 {
        match self {
            Self::Message => 10,
            Self::Op => 30,
            Self::Owner => 50,
            Self::Ban => 0,
        }
    }
}

/// A brick in the field.
#[derive(Debug, Clone, PartialEq)]
pub struct Brick {
    pub col: u8,
    pub row: u8,
    pub kind: BrickKind,
    /// Hits it still takes.
    pub hits: u8,
}

impl Brick {
    pub fn new(col: u8, row: u8, kind: BrickKind) -> Self {
        Self {
            col,
            row,
            kind,
            hits: kind.hits(),
        }
    }

    /// Its left and top edges.
    pub fn origin(&self) -> (f32, f32) {
        (
            BRICK_X0 + f32::from(self.col) * BRICK_PITCH_X,
            BRICK_Y0 + f32::from(self.row) * BRICK_PITCH_Y,
        )
    }

    /// Whether it has been hit and not broken yet.
    pub fn cracked(&self) -> bool {
        self.kind != BrickKind::Ban && self.hits < self.kind.hits()
    }
}

/// A brick a ball hit during a step.
#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    pub col: u8,
    pub row: u8,
    pub kind: BrickKind,
    /// The hit broke it.
    pub broken: bool,
}

/// What a capsule does when the paddle catches it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapsuleKind {
    /// `+o`: a wider paddle for a while.
    Op,
    /// `/join`: each ball becomes three.
    Join,
    /// `+m`: a slower ball for a while.
    Moderated,
    /// `/kick`: for a while the ball breaks bricks without bouncing off.
    Kick,
    /// `BNC`: a line at the floor saves one ball.
    Bouncer,
    /// `lag`: a faster ball for a while; the one to avoid.
    Lag,
}

impl CapsuleKind {
    pub const ALL: [Self; 6] = [
        Self::Op,
        Self::Join,
        Self::Moderated,
        Self::Kick,
        Self::Bouncer,
        Self::Lag,
    ];
}

/// A falling capsule: its centre across, its top, and what it is.
#[derive(Debug, Clone, PartialEq)]
pub struct Capsule {
    pub x: f32,
    pub y: f32,
    pub kind: CapsuleKind,
}

/// A change of the ball's speed that lasts a while.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pace {
    Moderated,
    Lag,
}

/// The capsules working now, with the seconds each has left.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Effects {
    pub op: f32,
    /// Moderated or Lag; one replaces the other.
    pub pace: Option<(Pace, f32)>,
    pub kick: f32,
    /// The Bouncer's line is there.
    pub bouncer: bool,
}

/// A ball: its centre and velocity.
#[derive(Debug, Clone, PartialEq)]
pub struct Ball {
    pub x: f32,
    pub y: f32,
    pub vx: f32,
    pub vy: f32,
}

/// What the player does during one step.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Input {
    /// Held: move the paddle left.
    pub left: bool,
    /// Held: move the paddle right.
    pub right: bool,
    /// Pressed since the last step: launch a held ball.
    pub launch: bool,
    /// Where the pointer puts the paddle's centre, in field units. It wins
    /// over the keys when set.
    pub pointer: Option<f32>,
}

/// What happened during one step.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Events {
    /// A held ball left the paddle.
    pub launched: bool,
    /// A ball bounced off a wall or the paddle.
    pub bounced: bool,
    /// The last ball was lost and a life with it.
    pub life_lost: bool,
    /// The last life was lost.
    pub game_over: bool,
    /// Bricks hit, in order.
    pub hits: Vec<Hit>,
    /// The last breakable brick broke: the game is on the next level now.
    pub level_cleared: bool,
    /// Capsules the paddle caught.
    pub caught: Vec<CapsuleKind>,
    /// The score passed another [`LIFE_EVERY`] and a life came with it.
    pub life_gained: bool,
    /// The Bouncer's line saved a ball.
    pub bounced_off_bouncer: bool,
}

/// A game of Ircoid.
#[derive(Debug, Clone, PartialEq)]
pub struct Game {
    pub paddle: Paddle,
    pub balls: Vec<Ball>,
    pub bricks: Vec<Brick>,
    pub capsules: Vec<Capsule>,
    pub effects: Effects,
    /// The score that brings the next life.
    pub next_life: u32,
    /// The ball waits on the paddle for a launch.
    pub held: bool,
    pub lives: u32,
    /// From 1.
    pub level: u32,
    pub score: u32,
    pub over: bool,
    /// The side the paddle last moved to: -1 left, 1 right.
    pub last_dir: f32,
    pub rng: Rng,
}

impl Game {
    pub fn new(seed: u64) -> Self {
        let mut game = Self {
            paddle: Paddle {
                x: FIELD_W / 2.0,
                w: PADDLE_W,
            },
            balls: Vec::new(),
            bricks: levels::parse(levels::layout(1).map),
            capsules: Vec::new(),
            effects: Effects::default(),
            next_life: LIFE_EVERY,
            held: true,
            lives: LIVES,
            level: 1,
            score: 0,
            over: false,
            last_dir: 1.0,
            rng: Rng::new(seed),
        };
        game.serve();
        game
    }

    /// The current level's channel name.
    pub fn channel(&self) -> &'static str {
        levels::layout(self.level).channel
    }

    /// Adds `points` to the score, up to [`MAX_SCORE`], and a life for
    /// each [`LIFE_EVERY`] passed, up to [`MAX_LIVES`].
    fn add_points(&mut self, points: u32, events: &mut Events) {
        self.score = self.score.saturating_add(points).min(MAX_SCORE);
        while self.score >= self.next_life {
            self.next_life = self.next_life.saturating_add(LIFE_EVERY);
            if self.lives < MAX_LIVES {
                self.lives += 1;
                events.life_gained = true;
            }
            if self.next_life == u32::MAX {
                break;
            }
        }
    }

    /// The levels' speed for the ball, without capsules.
    pub fn level_speed(&self) -> f32 {
        (BALL_SPEED + BALL_SPEED_STEP * (self.level.saturating_sub(1)) as f32).min(BALL_SPEED_MAX)
    }

    /// The ball's speed now, in units a second.
    pub fn speed(&self) -> f32 {
        let pace = match self.effects.pace {
            Some((Pace::Moderated, _)) => MODERATED_SPEED,
            Some((Pace::Lag, _)) => LAG_SPEED,
            None => 1.0,
        };
        self.level_speed() * pace
    }

    /// The points multiplier on this level.
    fn multiplier(&self) -> u32 {
        self.level.min(MAX_MULTIPLIER)
    }

    /// Counts the timed capsules down by `dt` seconds.
    fn tick(&mut self, dt: f32) {
        let effects = &mut self.effects;
        if effects.op > 0.0 {
            effects.op = (effects.op - dt).max(0.0);
            if effects.op == 0.0 {
                self.paddle.w = PADDLE_W;
                self.clamp_paddle();
            }
        }
        let effects = &mut self.effects;
        effects.kick = (effects.kick - dt).max(0.0);
        if let Some((pace, left)) = effects.pace {
            let left = left - dt;
            effects.pace = (left > 0.0).then_some((pace, left));
        }
    }

    /// Ends the timed capsules: a life was lost.
    fn clear_effects(&mut self) {
        self.effects.op = 0.0;
        self.effects.kick = 0.0;
        self.effects.pace = None;
        self.paddle.w = PADDLE_W;
        self.clamp_paddle();
        self.capsules.clear();
    }

    /// What a caught capsule does.
    pub fn apply(&mut self, kind: CapsuleKind) {
        match kind {
            CapsuleKind::Op => {
                self.effects.op = OP_SECS;
                self.paddle.w = PADDLE_W_OP;
                self.clamp_paddle();
            }
            CapsuleKind::Join => {
                if self.held {
                    return;
                }
                let turn = JOIN_ANGLE * PI / 180.0;
                let mut more = Vec::new();
                for ball in &self.balls {
                    for angle in [-turn, turn] {
                        if self.balls.len() + more.len() >= MAX_BALLS {
                            break;
                        }
                        let (sin, cos) = angle.sin_cos();
                        let mut new = Ball {
                            x: ball.x,
                            y: ball.y,
                            vx: ball.vx * cos - ball.vy * sin,
                            vy: ball.vx * sin + ball.vy * cos,
                        };
                        keep_vertical(&mut new, self.speed());
                        more.push(new);
                    }
                }
                self.balls.extend(more);
            }
            CapsuleKind::Moderated => {
                self.effects.pace = Some((Pace::Moderated, MODERATED_SECS));
            }
            CapsuleKind::Lag => self.effects.pace = Some((Pace::Lag, LAG_SECS)),
            CapsuleKind::Kick => self.effects.kick = KICK_SECS,
            CapsuleKind::Bouncer => self.effects.bouncer = true,
        }
    }

    /// Capsules fall; the paddle catches them or they are lost.
    fn fall(&mut self, dt: f32, events: &mut Events) {
        let paddle = self.paddle.clone();
        let mut caught = Vec::new();
        self.capsules.retain_mut(|capsule| {
            capsule.y += CAPSULE_SPEED * dt;
            let bottom = capsule.y + CAPSULE_H;
            let on_paddle = bottom >= PADDLE_Y
                && capsule.y <= PADDLE_Y + PADDLE_H
                && capsule.x + CAPSULE_W / 2.0 >= paddle.left()
                && capsule.x - CAPSULE_W / 2.0 <= paddle.right();
            if on_paddle {
                caught.push(capsule.kind);
                return false;
            }
            capsule.y <= FIELD_H
        });
        for kind in caught {
            self.apply(kind);
            self.add_points(CATCH_POINTS * self.multiplier(), events);
            events.caught.push(kind);
        }
    }

    /// One ball, held on the paddle.
    fn serve(&mut self) {
        self.held = true;
        self.balls = vec![Ball {
            x: self.paddle.x,
            y: PADDLE_Y - BALL_R,
            vx: 0.0,
            vy: 0.0,
        }];
    }

    /// Moves the game on by `dt` seconds, at most [`MAX_DT`].
    pub fn step(&mut self, input: &Input, dt: f32) -> Events {
        let mut events = Events::default();
        if self.over {
            return events;
        }
        let dt = dt.clamp(0.0, MAX_DT);
        self.tick(dt);
        self.move_paddle(input, dt);
        if self.held {
            for ball in &mut self.balls {
                ball.x = self.paddle.x;
                ball.y = PADDLE_Y - BALL_R;
            }
            if input.launch {
                self.launch();
                events.launched = true;
            } else {
                return events;
            }
        }
        self.fall(dt, &mut events);
        let speed = self.speed();
        let mut index = 0;
        while index < self.balls.len() {
            let mut ball = self.balls[index].clone();
            rescale(&mut ball, speed);
            let distance = speed * dt;
            let slices = (distance / SLICE).ceil().max(1.0) as u32;
            let part = dt / slices as f32;
            let mut lost = false;
            for _ in 0..slices {
                self.slice(&mut ball, part, &mut events);
                if ball.y - BALL_R > FIELD_H {
                    lost = true;
                    break;
                }
            }
            if lost {
                self.balls.remove(index);
            } else {
                self.balls[index] = ball;
                index += 1;
            }
        }
        if self.balls.is_empty() {
            self.lose_life(&mut events);
        } else if !self.bricks.iter().any(|b| b.kind != BrickKind::Ban) {
            self.clear_level(&mut events);
        }
        events
    }

    /// The bonus, then the next level with the ball back on the paddle.
    fn clear_level(&mut self, events: &mut Events) {
        self.add_points(CLEAR_BONUS.saturating_mul(self.level), events);
        self.level += 1;
        self.bricks = levels::parse(levels::layout(self.level).map);
        self.capsules.clear();
        self.serve();
        events.level_cleared = true;
    }

    fn move_paddle(&mut self, input: &Input, dt: f32) {
        let before = self.paddle.x;
        if let Some(x) = input.pointer {
            self.paddle.x = x;
        } else {
            let dir = f32::from(i8::from(input.right) - i8::from(input.left));
            self.paddle.x += dir * PADDLE_SPEED * dt;
        }
        self.clamp_paddle();
        let moved = self.paddle.x - before;
        if moved.abs() > f32::EPSILON {
            self.last_dir = moved.signum();
        }
    }

    pub(crate) fn clamp_paddle(&mut self) {
        let half = self.paddle.w / 2.0;
        self.paddle.x = self.paddle.x.clamp(WALL + half, FIELD_W - WALL - half);
    }

    fn launch(&mut self) {
        self.held = false;
        let speed = self.speed();
        let angle = LAUNCH_ANGLE * PI / 180.0 * self.last_dir;
        for ball in &mut self.balls {
            ball.vx = speed * angle.sin();
            ball.vy = -speed * angle.cos();
        }
    }

    /// Moves one ball through one slice: walls, a brick, then the paddle.
    fn slice(&mut self, ball: &mut Ball, dt: f32, events: &mut Events) {
        let speed = self.speed();
        let before_bottom = ball.y + BALL_R;
        ball.x += ball.vx * dt;
        ball.y += ball.vy * dt;

        if ball.x - BALL_R < WALL {
            ball.x = WALL + BALL_R;
            ball.vx = ball.vx.abs();
            keep_vertical(ball, speed);
            events.bounced = true;
        } else if ball.x + BALL_R > FIELD_W - WALL {
            ball.x = FIELD_W - WALL - BALL_R;
            ball.vx = -ball.vx.abs();
            keep_vertical(ball, speed);
            events.bounced = true;
        }
        if ball.y - BALL_R < WALL {
            ball.y = WALL + BALL_R;
            ball.vy = ball.vy.abs();
            keep_vertical(ball, speed);
            events.bounced = true;
        }

        self.hit_brick(ball, speed, events);

        if self.effects.bouncer && ball.vy > 0.0 && ball.y + BALL_R >= BOUNCER_Y {
            ball.y = BOUNCER_Y - BALL_R;
            ball.vy = -ball.vy.abs();
            keep_vertical(ball, speed);
            self.effects.bouncer = false;
            events.bounced_off_bouncer = true;
        }

        let bottom = ball.y + BALL_R;
        let paddle = &self.paddle;
        if ball.vy > 0.0
            && before_bottom <= PADDLE_Y + 0.01
            && bottom >= PADDLE_Y
            && ball.x + BALL_R >= paddle.left()
            && ball.x - BALL_R <= paddle.right()
        {
            let offset = ((ball.x - paddle.x) / (paddle.w / 2.0)).clamp(-1.0, 1.0);
            let angle = offset * BOUNCE_ANGLE * PI / 180.0;
            ball.vx = speed * angle.sin();
            ball.vy = -speed * angle.cos();
            ball.y = PADDLE_Y - BALL_R;
            keep_vertical(ball, speed);
            events.bounced = true;
        }
    }

    /// At most one brick a slice: the one the ball overlaps most. The ball
    /// reflects on the axis it overlaps least.
    fn hit_brick(&mut self, ball: &mut Ball, speed: f32, events: &mut Events) {
        let mut best: Option<(usize, f32, f32)> = None;
        for (index, brick) in self.bricks.iter().enumerate() {
            let (left, top) = brick.origin();
            let (right, bottom) = (left + BRICK_W, top + BRICK_H);
            // The nearest point of the brick to the ball's centre.
            let nx = ball.x.clamp(left, right);
            let ny = ball.y.clamp(top, bottom);
            let (dx, dy) = (ball.x - nx, ball.y - ny);
            if dx * dx + dy * dy > BALL_R * BALL_R {
                continue;
            }
            let over_x = (ball.x + BALL_R - left).min(right - (ball.x - BALL_R));
            let over_y = (ball.y + BALL_R - top).min(bottom - (ball.y - BALL_R));
            let depth = over_x.min(over_y);
            if best.is_none_or(|(_, x, y)| depth > x.min(y)) {
                best = Some((index, over_x, over_y));
            }
        }
        let Some((index, over_x, over_y)) = best else {
            return;
        };
        let (left, top) = self.bricks[index].origin();
        if self.effects.kick > 0.0 && self.bricks[index].kind != BrickKind::Ban {
            // Kick: straight through, and the brick goes in one.
            self.bricks[index].hits = 1;
            self.strike(index, events);
            return;
        }
        if over_x < over_y {
            if ball.x < left + BRICK_W / 2.0 {
                ball.vx = -ball.vx.abs();
                ball.x -= over_x;
            } else {
                ball.vx = ball.vx.abs();
                ball.x += over_x;
            }
        } else if ball.y < top + BRICK_H / 2.0 {
            ball.vy = -ball.vy.abs();
            ball.y -= over_y;
        } else {
            ball.vy = ball.vy.abs();
            ball.y += over_y;
        }
        keep_vertical(ball, speed);
        self.strike(index, events);
    }

    /// One hit on the brick at `index`.
    fn strike(&mut self, index: usize, events: &mut Events) {
        let brick = &mut self.bricks[index];
        let kind = brick.kind;
        let broken = kind != BrickKind::Ban && {
            brick.hits = brick.hits.saturating_sub(1);
            brick.hits == 0
        };
        events.hits.push(Hit {
            col: brick.col,
            row: brick.row,
            kind,
            broken,
        });
        if broken {
            let removed = self.bricks.remove(index);
            self.add_points(kind.points() * self.multiplier(), events);
            if self.capsules.len() < MAX_CAPSULES && self.rng.below(DROP_ONE_IN) == 0 {
                let (x, y) = removed.origin();
                let kind = CapsuleKind::ALL[self.rng.below(CapsuleKind::ALL.len() as u32) as usize];
                self.capsules.push(Capsule {
                    x: x + BRICK_W / 2.0,
                    y,
                    kind,
                });
            }
        }
    }

    fn lose_life(&mut self, events: &mut Events) {
        self.clear_effects();
        self.lives = self.lives.saturating_sub(1);
        events.life_lost = true;
        if self.lives == 0 {
            self.over = true;
            events.game_over = true;
        } else {
            self.serve();
        }
    }
}

/// Keeps a ball's direction and sets its speed.
fn rescale(ball: &mut Ball, speed: f32) {
    let now = (ball.vx * ball.vx + ball.vy * ball.vy).sqrt();
    if now > f32::EPSILON {
        ball.vx *= speed / now;
        ball.vy *= speed / now;
    }
}

/// Keeps at least [`MIN_VERTICAL`] of the speed vertical, so a ball never
/// gets stuck travelling sideways.
fn keep_vertical(ball: &mut Ball, speed: f32) {
    let min = speed * MIN_VERTICAL;
    if ball.vy.abs() < min {
        let up = if ball.vy < 0.0 { -1.0 } else { 1.0 };
        ball.vy = up * min;
        let side = if ball.vx < 0.0 { -1.0 } else { 1.0 };
        ball.vx = side * (speed * speed - min * min).sqrt();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DT: f32 = 1.0 / 60.0;

    fn hold(left: bool, right: bool) -> Input {
        Input {
            left,
            right,
            ..Input::default()
        }
    }

    fn launch() -> Input {
        Input {
            launch: true,
            ..Input::default()
        }
    }

    fn flying(x: f32, y: f32, vx: f32, vy: f32) -> Game {
        let mut game = Game::new(1);
        game.held = false;
        // One brick out of the way, so the level is not cleared.
        game.bricks = vec![far()];
        game.balls = vec![Ball { x, y, vx, vy }];
        game
    }

    fn far() -> Brick {
        Brick::new(12, 0, BrickKind::Message)
    }

    /// A game with only `bricks`, and one ball at `x`, `y` moving at
    /// `vx`, `vy`; a brick far away keeps the level from clearing.
    fn with_bricks(bricks: Vec<Brick>, x: f32, y: f32, vx: f32, vy: f32) -> Game {
        let mut game = flying(x, y, vx, vy);
        game.bricks = bricks;
        game.bricks.push(far());
        game
    }

    /// The centre of the brick at `col`, `row`.
    fn centre(col: u8, row: u8) -> (f32, f32) {
        let (x, y) = Brick::new(col, row, BrickKind::Message).origin();
        (x + BRICK_W / 2.0, y + BRICK_H / 2.0)
    }

    /// Steps until a brick is hit, or a second has passed.
    fn until_hit(game: &mut Game) -> Vec<Hit> {
        for _ in 0..60 {
            let events = game.step(&Input::default(), DT);
            if !events.hits.is_empty() {
                return events.hits;
            }
        }
        Vec::new()
    }

    #[test]
    fn an_op_needs_two_hits() {
        let (x, y) = centre(4, 4);
        let mut game = with_bricks(
            vec![Brick::new(4, 4, BrickKind::Op)],
            x,
            y + 20.0,
            0.0,
            -BALL_SPEED,
        );
        let hits = until_hit(&mut game);
        assert!(!hits[0].broken);
        assert!(game.bricks[0].cracked());
        assert_eq!(game.score, 0);
        game.balls[0] = Ball {
            x,
            y: y + 20.0,
            vx: 0.0,
            vy: -BALL_SPEED,
        };
        let hits = until_hit(&mut game);
        assert!(hits[0].broken);
        assert_eq!(game.score, 30);
        assert_eq!(game.bricks, [far()], "only the far brick is left");
    }

    #[test]
    fn a_ban_reflects_and_stays() {
        let (x, y) = centre(6, 6);
        let mut game = with_bricks(
            vec![Brick::new(6, 6, BrickKind::Ban)],
            x,
            y + 20.0,
            0.0,
            -BALL_SPEED,
        );
        for _ in 0..5 {
            let hits = until_hit(&mut game);
            assert_eq!(hits[0].kind, BrickKind::Ban);
            assert!(!hits[0].broken);
            assert!(game.balls[0].vy > 0.0);
            game.balls[0].vy = -BALL_SPEED;
        }
        assert_eq!(game.bricks.len(), 2);
        assert_eq!(game.score, 0);
    }

    #[test]
    fn a_brick_reflects_the_ball_on_the_side_it_hits() {
        let (x, y) = centre(6, 5);
        let brick = || vec![Brick::new(6, 5, BrickKind::Owner)];
        // From below and above.
        let mut game = with_bricks(brick(), x, y + 15.0, 0.0, -BALL_SPEED);
        until_hit(&mut game);
        assert!(game.balls[0].vy > 0.0, "from below");
        let mut game = with_bricks(brick(), x, y - 15.0, 0.0, BALL_SPEED);
        until_hit(&mut game);
        assert!(game.balls[0].vy < 0.0, "from above");
        // From the left and the right, nearly level.
        let mut game = with_bricks(brick(), x - 25.0, y, 160.0, 57.0);
        until_hit(&mut game);
        assert!(game.balls[0].vx < 0.0, "from the left");
        let mut game = with_bricks(brick(), x + 25.0, y, -160.0, 57.0);
        until_hit(&mut game);
        assert!(game.balls[0].vx > 0.0, "from the right");
    }

    #[test]
    fn a_ball_at_top_speed_never_passes_through_a_row() {
        for n in 0..40 {
            let row: Vec<Brick> = (0..13)
                .map(|col| Brick::new(col, 5, BrickKind::Ban))
                .collect();
            let (_, top) = row[0].origin();
            let bottom = top + BRICK_H;
            let angle = (n as f32 - 20.0) * 3.0 * PI / 180.0;
            let x = 30.0 + n as f32 * 8.5;
            let mut game = with_bricks(
                row,
                x,
                bottom + 30.0,
                BALL_SPEED_MAX * angle.sin(),
                -BALL_SPEED_MAX * angle.cos(),
            );
            game.level = 99;
            for _ in 0..30 {
                game.step(&Input::default(), MAX_DT);
                assert!(
                    game.balls[0].y - BALL_R > top,
                    "angle {n}: the ball got into or past the row"
                );
            }
        }
    }

    #[test]
    fn clearing_level_one_without_a_miss_scores_1120() {
        let mut game = Game::new(3);
        let mut events = Events::default();
        // Break every brick in turn by aiming the ball at it from below.
        while !events.level_cleared {
            let brick = game.bricks[0].clone();
            let (x, y) = centre(brick.col, brick.row);
            game.held = false;
            game.balls = vec![Ball {
                x,
                y: y + 7.0,
                vx: 0.0,
                vy: -BALL_SPEED,
            }];
            events = game.step(&Input::default(), DT);
            assert!(!events.hits.is_empty());
        }
        assert_eq!(game.score, 1120);
        assert_eq!(game.level, 2);
        assert_eq!(game.channel(), "#motd");
        assert!(game.held, "the ball is back on the paddle");
        assert_eq!(game.speed(), BALL_SPEED + BALL_SPEED_STEP);
    }

    #[test]
    fn level_seven_uses_the_first_layout() {
        let mut game = Game::new(1);
        game.level = 6;
        game.bricks = vec![Brick::new(0, 0, BrickKind::Message)];
        let (x, y) = centre(0, 0);
        game.held = false;
        game.balls = vec![Ball {
            x,
            y: y + 7.0,
            vx: 0.0,
            vy: -BALL_SPEED,
        }];
        assert!(game.step(&Input::default(), DT).level_cleared);
        assert_eq!(game.level, 7);
        assert_eq!(game.channel(), "#the-hash");
        assert_eq!(game.bricks, levels::parse(levels::layout(1).map));
        assert_eq!(game.score, 60 + 3000);
    }

    #[test]
    fn the_score_is_always_a_multiple_of_ten() {
        for seed in 1..6 {
            let mut game = Game::new(seed);
            for frame in 0..6000 {
                let x = game.balls.first().map(|b| b.x + (frame % 23) as f32 - 11.0);
                let input = Input {
                    launch: true,
                    pointer: x,
                    ..Input::default()
                };
                game.step(&input, DT);
                assert_eq!(game.score % 10, 0);
            }
            assert!(game.score > 0, "seed {seed} broke something");
        }
    }

    #[test]
    fn the_paddle_stops_at_both_walls() {
        let mut game = Game::new(1);
        for _ in 0..200 {
            game.step(&hold(true, false), DT);
        }
        assert_eq!(game.paddle.left(), WALL);
        for _ in 0..200 {
            game.step(&hold(false, true), DT);
        }
        assert_eq!(game.paddle.right(), FIELD_W - WALL);
    }

    #[test]
    fn a_held_ball_follows_the_paddle() {
        let mut game = Game::new(1);
        for _ in 0..10 {
            game.step(&hold(false, true), DT);
        }
        assert!(game.held);
        assert!(game.paddle.x > FIELD_W / 2.0);
        assert_eq!(game.balls[0].x, game.paddle.x);
        assert_eq!(game.balls[0].y, PADDLE_Y - BALL_R);
    }

    #[test]
    fn a_launched_ball_goes_up_toward_the_last_move() {
        let mut game = Game::new(1);
        game.step(&hold(true, false), DT);
        let events = game.step(&launch(), DT);
        assert!(events.launched);
        assert!(!game.held);
        let ball = &game.balls[0];
        assert!(ball.vy < 0.0);
        assert!(ball.vx < 0.0, "launched left after moving left");
        let speed = (ball.vx * ball.vx + ball.vy * ball.vy).sqrt();
        assert!((speed - BALL_SPEED).abs() < 0.01);
    }

    #[test]
    fn a_ball_reflects_off_each_wall() {
        let mut game = flying(WALL + BALL_R + 0.5, 150.0, -150.0, -80.0);
        game.step(&Input::default(), DT);
        assert!(game.balls[0].vx > 0.0, "left wall");

        let mut game = flying(FIELD_W - WALL - BALL_R - 0.5, 150.0, 150.0, -80.0);
        game.step(&Input::default(), DT);
        assert!(game.balls[0].vx < 0.0, "right wall");

        let mut game = flying(200.0, WALL + BALL_R + 0.5, 30.0, -165.0);
        game.step(&Input::default(), DT);
        assert!(game.balls[0].vy > 0.0, "top wall");
    }

    fn landing(offset: f32) -> Ball {
        let x = FIELD_W / 2.0 + offset;
        let mut game = flying(x, PADDLE_Y - BALL_R - 1.0, 0.0, BALL_SPEED);
        game.step(&Input::default(), DT);
        game.balls[0].clone()
    }

    #[test]
    fn the_landing_point_sets_the_angle() {
        let left = landing(-PADDLE_W / 2.0);
        let middle = landing(0.0);
        let right = landing(PADDLE_W / 2.0);
        assert!(left.vy < 0.0 && middle.vy < 0.0 && right.vy < 0.0);
        assert!(left.vx < -100.0, "left end goes left: {left:?}");
        assert!(middle.vx.abs() < 1.0, "middle goes up: {middle:?}");
        assert!(right.vx > 100.0, "right end goes right: {right:?}");
        let speed = (left.vx * left.vx + left.vy * left.vy).sqrt();
        assert!((speed - BALL_SPEED).abs() < 0.01, "speed kept");
    }

    #[test]
    fn a_bounce_keeps_a_quarter_of_the_speed_vertical() {
        let mut game = flying(WALL + BALL_R + 0.5, 150.0, -169.0, -2.0);
        game.step(&Input::default(), DT);
        let ball = &game.balls[0];
        assert!(ball.vy.abs() >= BALL_SPEED * MIN_VERTICAL - 0.01);
    }

    #[test]
    fn a_ball_never_ends_a_step_inside_a_wall() {
        let mut game = Game::new(7);
        game.level = 99;
        assert_eq!(game.speed(), BALL_SPEED_MAX);
        game.step(&launch(), MAX_DT);
        for frame in 0..2000 {
            // Keep the ball in play: the paddle follows it.
            let x = game.balls.first().map(|ball| ball.x);
            let input = Input {
                pointer: x,
                ..Input::default()
            };
            game.step(&input, MAX_DT);
            for ball in &game.balls {
                assert!(ball.x - BALL_R >= WALL - 0.001, "frame {frame}: {ball:?}");
                assert!(ball.x + BALL_R <= FIELD_W - WALL + 0.001, "frame {frame}");
                assert!(ball.y - BALL_R >= WALL - 0.001, "frame {frame}");
            }
            assert_eq!(
                game.lives, LIVES,
                "frame {frame}: the paddle under the ball saves it"
            );
        }
    }

    #[test]
    fn three_lost_balls_end_the_game() {
        let mut game = Game::new(1);
        let mut lost = 0;
        let mut over = false;
        for _ in 0..3 {
            game.step(&launch(), DT);
            // Stand aside; the ball flies up, comes back and passes the paddle.
            for _ in 0..2000 {
                let aside = if game.balls[0].x > FIELD_W / 2.0 {
                    0.0
                } else {
                    FIELD_W
                };
                let input = Input {
                    pointer: Some(aside),
                    ..Input::default()
                };
                let events = game.step(&input, DT);
                if events.life_lost {
                    lost += 1;
                    over = events.game_over;
                    break;
                }
            }
        }
        assert_eq!(lost, 3);
        assert!(over);
        assert!(game.over);
        assert_eq!(game.lives, 0);
        assert_eq!(game.step(&launch(), DT), Events::default());
    }

    #[test]
    fn the_same_seed_and_inputs_give_the_same_game() {
        let play = |seed| {
            let mut game = Game::new(seed);
            for frame in 0..600 {
                let input = Input {
                    left: frame % 50 < 20,
                    right: frame % 70 > 40,
                    launch: frame == 5,
                    pointer: None,
                };
                game.step(&input, DT);
            }
            game
        };
        assert_eq!(play(42), play(42));
    }
    /// A capsule of `kind` just above the paddle's middle.
    fn catch(game: &mut Game, kind: CapsuleKind) -> Events {
        game.capsules = vec![Capsule {
            x: game.paddle.x,
            y: PADDLE_Y - CAPSULE_H - 0.5,
            kind,
        }];
        game.step(&Input::default(), DT)
    }

    /// A game with one ball high in the field, going up, and nothing else.
    fn open_field() -> Game {
        flying(100.0, 200.0, 60.0, -150.0)
    }

    #[test]
    fn op_widens_the_paddle_for_fifteen_seconds() {
        let mut game = open_field();
        let events = catch(&mut game, CapsuleKind::Op);
        assert_eq!(events.caught, [CapsuleKind::Op]);
        assert_eq!(game.score, 50);
        assert_eq!(game.paddle.w, PADDLE_W_OP);
        game.balls.clear();
        game.held = true;
        game.balls.push(Ball {
            x: 0.0,
            y: 0.0,
            vx: 0.0,
            vy: 0.0,
        });
        for _ in 0..(14 * 60) {
            game.step(&Input::default(), DT);
        }
        assert_eq!(game.paddle.w, PADDLE_W_OP, "still wide after 14 s");
        for _ in 0..(2 * 60) {
            game.step(&Input::default(), DT);
        }
        assert_eq!(game.paddle.w, PADDLE_W, "narrow again after 15 s");
    }

    #[test]
    fn join_makes_three_balls_and_never_a_seventh() {
        let mut game = open_field();
        catch(&mut game, CapsuleKind::Join);
        assert_eq!(game.balls.len(), 3);
        for ball in &game.balls {
            let speed = (ball.vx * ball.vx + ball.vy * ball.vy).sqrt();
            assert!((speed - BALL_SPEED).abs() < 0.5);
        }
        catch(&mut game, CapsuleKind::Join);
        assert_eq!(game.balls.len(), MAX_BALLS);
        catch(&mut game, CapsuleKind::Join);
        assert_eq!(game.balls.len(), MAX_BALLS, "never a seventh");
    }

    #[test]
    fn moderated_and_lag_change_the_speed_and_replace_each_other() {
        let mut game = open_field();
        catch(&mut game, CapsuleKind::Moderated);
        assert!((game.speed() - BALL_SPEED * MODERATED_SPEED).abs() < 0.01);
        game.step(&Input::default(), DT);
        let ball = &game.balls[0];
        let speed = (ball.vx * ball.vx + ball.vy * ball.vy).sqrt();
        assert!(
            (speed - BALL_SPEED * MODERATED_SPEED).abs() < 0.5,
            "{speed}"
        );
        catch(&mut game, CapsuleKind::Lag);
        assert!((game.speed() - BALL_SPEED * LAG_SPEED).abs() < 0.01);
        assert_eq!(game.effects.pace.map(|(pace, _)| pace), Some(Pace::Lag));
        catch(&mut game, CapsuleKind::Moderated);
        assert_eq!(
            game.effects.pace.map(|(pace, _)| pace),
            Some(Pace::Moderated)
        );
        for _ in 0..(11 * 60) {
            game.step(&Input::default(), DT);
            if game.balls.is_empty() || game.held {
                break;
            }
        }
        assert_eq!(game.effects.pace, None);
    }

    #[test]
    fn kick_breaks_an_op_in_one_pass_and_still_bounces_off_a_ban() {
        let (x, y) = centre(4, 4);
        let mut game = with_bricks(
            vec![Brick::new(4, 4, BrickKind::Op)],
            x,
            y + 20.0,
            0.0,
            -BALL_SPEED,
        );
        game.apply(CapsuleKind::Kick);
        let hits = until_hit(&mut game);
        assert!(hits[0].broken, "one pass breaks the op");
        assert!(game.balls[0].vy < 0.0, "no bounce");
        assert_eq!(game.score, 30);

        let (x, y) = centre(6, 6);
        let mut game = with_bricks(
            vec![Brick::new(6, 6, BrickKind::Ban)],
            x,
            y + 20.0,
            0.0,
            -BALL_SPEED,
        );
        game.apply(CapsuleKind::Kick);
        let hits = until_hit(&mut game);
        assert!(!hits[0].broken);
        assert!(game.balls[0].vy > 0.0, "a ban still bounces");
    }

    #[test]
    fn the_bouncer_saves_exactly_one_ball() {
        let mut game = flying(40.0, 280.0, 0.0, BALL_SPEED);
        game.paddle.x = 300.0;
        game.apply(CapsuleKind::Bouncer);
        let mut saved = 0;
        let mut lost = false;
        for _ in 0..600 {
            let events = game.step(&Input::default(), DT);
            saved += usize::from(events.bounced_off_bouncer);
            if events.life_lost {
                lost = true;
                break;
            }
            // Send the ball straight back down each time it goes up.
            if let Some(ball) = game.balls.first_mut()
                && ball.vy < 0.0
                && ball.y < 250.0
            {
                ball.vy = ball.vy.abs();
                ball.vx = 0.0;
            }
        }
        assert_eq!(saved, 1);
        assert!(lost, "the second time the ball is lost");
        assert!(!game.effects.bouncer);
    }

    #[test]
    fn timers_run_on_game_time_only() {
        let mut game = open_field();
        catch(&mut game, CapsuleKind::Kick);
        let left = game.effects.kick;
        // A paused game is not stepped: nothing counts down.
        let paused = game.clone();
        assert_eq!(paused.effects.kick, left);
        game.step(&Input::default(), 0.5 * MAX_DT);
        assert!(game.effects.kick < left);
    }

    #[test]
    fn losing_a_life_ends_the_timed_capsules() {
        let mut game = flying(40.0, 290.0, 0.0, BALL_SPEED);
        game.paddle.x = 300.0;
        game.apply(CapsuleKind::Op);
        game.apply(CapsuleKind::Lag);
        game.apply(CapsuleKind::Kick);
        let mut lost = false;
        for _ in 0..60 {
            lost |= game.step(&Input::default(), DT).life_lost;
        }
        assert!(lost);
        assert_eq!(game.paddle.w, PADDLE_W);
        assert_eq!(game.effects.pace, None);
        assert_eq!(game.effects.kick, 0.0);
    }

    #[test]
    fn a_life_every_twenty_thousand_up_to_five() {
        let mut game = Game::new(1);
        let mut events = Events::default();
        game.add_points(19_990, &mut events);
        assert_eq!(game.lives, LIVES);
        game.add_points(10, &mut events);
        assert!(events.life_gained);
        assert_eq!(game.lives, LIVES + 1);
        game.add_points(100_000, &mut events);
        assert_eq!(game.lives, MAX_LIVES, "not past five");
        game.add_points(MAX_SCORE, &mut events);
        assert_eq!(game.score, MAX_SCORE, "the score stops at the bound");
    }

    #[test]
    fn broken_bricks_drop_capsules_now_and_then() {
        let mut drops = 0;
        let mut kinds = std::collections::HashSet::new();
        for seed in 1..400 {
            let mut game = Game::new(seed);
            // The bottom row, so the ball touches one brick only.
            let brick = game.bricks.last().unwrap().clone();
            let (x, y) = centre(brick.col, brick.row);
            game.held = false;
            game.balls = vec![Ball {
                x,
                y: y + 7.0,
                vx: 0.0,
                vy: -BALL_SPEED,
            }];
            game.step(&Input::default(), DT);
            assert!(game.capsules.len() <= MAX_CAPSULES);
            if let Some(capsule) = game.capsules.first() {
                drops += 1;
                kinds.insert(format!("{:?}", capsule.kind));
            }
        }
        assert!((25..75).contains(&drops), "about one in eight: {drops}");
        assert_eq!(kinds.len(), 6, "every kind drops");
    }
}

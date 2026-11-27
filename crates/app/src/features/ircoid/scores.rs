//! Ircoid's high score table over the stage: the server's instance-wide top
//! 10, opened with the High scores button (which pauses a running game) and
//! at game over, where a score that places asks for a name.
//!
//! A server without the table (an older backend) answers 404: the table says
//! it is not available and the game plays on. Results that arrive after a
//! newer game or load are dropped, as the web's Hash Field does.

use gpui_kit::TestSupportExt;
use gpui_kit::component::Sizable;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::prelude::*;
use gpui_kit::*;
use ircord_client::contract::IrcoidEntry;
use ircord_client::ircoid::scores::{NameError, places_in};

use crate::theme::Theme;

/// The table as last loaded.
#[derive(Debug, Clone, PartialEq)]
pub enum Table {
    Loading,
    Unavailable,
    Ready(Vec<IrcoidEntry>),
}

/// What became of a saved score.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    Saved(i64),
    TookPlace,
}

/// The table's state in the open game.
pub struct Scores {
    pub table: Table,
    /// The table is drawn over the stage.
    pub shown: bool,
    /// The name field, while a placing score asks for a name.
    pub entry: Option<Entity<InputState>>,
    /// A placing score is waiting for its name field to be made.
    pub wanted: bool,
    /// The score a name is asked for.
    pub score: u32,
    pub error: Option<String>,
    pub saving: bool,
    pub outcome: Option<Outcome>,
    /// The rank of the row just saved.
    pub mark: Option<i64>,
    /// Bumped by each load, save and new game; an answer for an older one
    /// is dropped.
    pub generation: u64,
}

impl Default for Scores {
    fn default() -> Self {
        Self {
            table: Table::Loading,
            shown: false,
            entry: None,
            wanted: false,
            score: 0,
            error: None,
            saving: false,
            outcome: None,
            mark: None,
            generation: 0,
        }
    }
}

impl Scores {
    /// Starts over for a new game: nothing shown, older answers dropped.
    pub fn reset(&mut self) {
        let generation = self.generation + 1;
        *self = Self {
            generation,
            ..Self::default()
        };
    }

    /// The table arrived: at game over, a placing score asks for a name.
    pub fn loaded(&mut self, table: Table, asking: Option<u32>) {
        if let (Table::Ready(rows), Some(score)) = (&table, asking)
            && places_in(rows, score)
        {
            self.wanted = true;
            self.score = score;
        }
        self.table = table;
    }

    /// Whether a name is being asked for.
    pub fn asking(&self) -> bool {
        self.entry.is_some() || self.wanted
    }
}

/// Why a name was refused, in words.
pub fn name_error(error: NameError) -> String {
    match error {
        NameError::Length => t!(
            "ircoid-name-length",
            max = ircord_client::ircoid::scores::MAX_NAME
        ),
        NameError::Characters => t!("ircoid-name-characters").to_string(),
    }
}

/// A row of the table.
fn row(entry: &IrcoidEntry, marked: bool, theme: &Theme) -> impl IntoElement {
    let label = if marked {
        t!(
            "ircoid-scores-row-new",
            rank = entry.rank,
            name = entry.name.clone(),
            score = entry.score
        )
    } else {
        t!(
            "ircoid-scores-row",
            rank = entry.rank,
            name = entry.name.clone(),
            score = entry.score
        )
    };
    div()
        .id(ElementId::Name(
            format!("ircoid-score-{}", entry.rank).into(),
        ))
        .test_support()
        .role(Role::ListItem)
        .aria_label(SharedString::from(label))
        .flex()
        .gap_3()
        .px_2()
        .py_0p5()
        .rounded_sm()
        .when(marked, |d| d.bg(theme.accent.opacity(0.25)))
        .font_family(super::stage::FONT)
        .text_sm()
        .text_color(theme.text)
        .child(
            div()
                .w(px(28.))
                .text_color(theme.muted)
                .child(format!("{}.", entry.rank)),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .child(entry.name.clone()),
        )
        .child(div().child(entry.score.to_string()))
}

/// A line of text with an ID and its own text as its name.
fn line(id: &'static str, text: String, color: Hsla) -> impl IntoElement {
    div()
        .id(id)
        .test_support()
        .aria_label(SharedString::from(text.clone()))
        .text_sm()
        .text_color(color)
        .child(text)
}

/// What the panel shows around the table.
pub struct Extras {
    /// The Back button, when the table was opened with High scores.
    pub back: bool,
    /// The game over lines, at the top.
    pub over: Option<Vec<String>>,
    /// The `/credits` line, at the bottom.
    pub credits_line: Option<AnyElement>,
}

/// The table over the stage.
pub fn overlay(
    scores: &Scores,
    theme: &Theme,
    extras: Extras,
    on_back: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    on_save: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    on_skip: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let Extras {
        back,
        over,
        credits_line,
    } = extras;
    let body: AnyElement = match &scores.table {
        Table::Loading => line(
            "ircoid-scores-loading",
            t!("ircoid-scores-loading").to_string(),
            theme.muted,
        )
        .into_any_element(),
        Table::Unavailable => line(
            "ircoid-scores-unavailable",
            t!("ircoid-scores-unavailable").to_string(),
            theme.muted,
        )
        .into_any_element(),
        Table::Ready(rows) if rows.is_empty() => line(
            "ircoid-scores-empty",
            t!("ircoid-scores-empty").to_string(),
            theme.muted,
        )
        .into_any_element(),
        Table::Ready(rows) => div()
            .id("ircoid-scores-list")
            .test_support()
            .role(Role::List)
            .aria_label(t!("ircoid-scores-title"))
            .flex()
            .flex_col()
            .children(
                rows.iter()
                    .map(|r| row(r, scores.mark == Some(r.rank), theme)),
            )
            .into_any_element(),
    };
    let entry = scores.entry.clone().map(|input| {
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(line(
                "ircoid-scores-placed",
                t!("ircoid-scores-placed").to_string(),
                theme.text,
            ))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div().flex_1().child(
                            Input::new(&input)
                                .id("ircoid-name")
                                .aria_label(t!("ircoid-name")),
                        ),
                    )
                    .child(
                        Button::new("ircoid-save")
                            .small()
                            .primary()
                            .label(t!("ircoid-save"))
                            .loading(scores.saving)
                            .on_click(on_save),
                    )
                    .child(
                        Button::new("ircoid-skip")
                            .small()
                            .label(t!("ircoid-skip"))
                            .on_click(on_skip),
                    ),
            )
            .children(
                scores
                    .error
                    .clone()
                    .map(|e| line("ircoid-name-error", e, theme.danger)),
            )
    });
    let outcome = scores.outcome.as_ref().map(|outcome| {
        let text = match outcome {
            Outcome::Saved(rank) => t!("ircoid-saved", rank = rank),
            Outcome::TookPlace => t!("ircoid-took-place").to_string(),
        };
        line("ircoid-scores-outcome", text, theme.text)
    });
    div()
        .id("ircoid-scores-panel")
        .test_support()
        .absolute()
        .inset_0()
        .occlude()
        .p_6()
        .flex()
        .justify_center()
        .items_center()
        .bg(super::stage::color(super::stage::palette::VOID, 0.6))
        .child(
            div()
                .w(px(380.))
                .max_w_full()
                .p_4()
                .rounded_lg()
                .border_1()
                .border_color(theme.border)
                .bg(theme.main)
                .shadow_lg()
                .flex()
                .flex_col()
                .gap_3()
                .children(over.map(|lines| {
                    div()
                        .id("ircoid-notice-over")
                        .test_support()
                        .aria_label(SharedString::from(lines.join(" ")))
                        .flex()
                        .flex_col()
                        .font_family(super::stage::FONT)
                        .font_weight(FontWeight::BOLD)
                        .text_sm()
                        .text_color(theme.danger)
                        .children(lines)
                }))
                .child(
                    div()
                        .id("ircoid-scores-heading")
                        .test_support()
                        .role(Role::Heading)
                        .aria_label(t!("ircoid-scores-title"))
                        .font_weight(FontWeight::BOLD)
                        .text_color(theme.text)
                        .child(t!("ircoid-scores-title")),
                )
                .child(body)
                .children(entry)
                .children(outcome)
                .children(credits_line)
                .when(back, |d| {
                    d.child(
                        div().flex().justify_end().child(
                            Button::new("ircoid-scores-back")
                                .small()
                                .label(t!("ircoid-scores-back"))
                                .on_click(on_back),
                        ),
                    )
                }),
        )
}

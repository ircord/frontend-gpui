//! Ircoid, the brick-breaking game behind About's logo: five presses on the
//! logo in a row open it in a dialog over Settings, as the web's logo opens
//! Hash Field. Escape closes it and About is still there underneath.
//!
//! The rules are `ircord_client::ircoid`; this module draws them and takes
//! the keys.

use std::time::{Duration, Instant};

use gpui_kit::TestSupportExt;
use gpui_kit::component::WindowExt;
use gpui_kit::component::button::Button;
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::session::Session;
use crate::theme;

/// Presses on the logo that open the game.
pub const PRESSES: u32 = 5;
/// A pause this long between presses starts the count again.
pub const PRESS_GAP: Duration = Duration::from_millis(1500);
/// The dialog is at most this wide.
const MAX_WIDTH: f32 = 848.;

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

/// Open the game in a dialog.
pub fn open(session: &Entity<Session>, window: &mut Window, cx: &mut App) {
    let view = cx.new(|cx| Ircoid::new(session.clone(), cx));
    let focus = view.read(cx).focus.clone();
    let viewport = window.viewport_size();
    let width = px(MAX_WIDTH).min(viewport.width - px(64.)).max(px(320.));
    window.open_dialog(cx, move |dialog, _, _| {
        dialog
            .width(width)
            .title(t!("ircoid-title"))
            .overlay_closable(false)
            .child(view.clone())
    });
    window.focus(&focus, cx);
}

/// The open game.
pub struct Ircoid {
    #[allow(dead_code)]
    session: Entity<Session>,
    focus: FocusHandle,
}

impl Ircoid {
    fn new(session: Entity<Session>, cx: &mut Context<Self>) -> Self {
        Self {
            session,
            focus: cx.focus_handle(),
        }
    }
}

impl Focusable for Ircoid {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for Ircoid {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme::get(cx).clone();
        div()
            .id("ircoid")
            .test_support()
            .track_focus(&self.focus)
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .id("ircoid-stage")
                    .test_support()
                    .role(Role::Image)
                    .aria_label(t!("ircoid-stage"))
                    .w_full()
                    .aspect_ratio(4. / 3.)
                    .rounded_md()
                    .overflow_hidden()
                    .bg(theme.rail),
            )
            .child(
                div().flex().justify_end().child(
                    Button::new("ircoid-back")
                        .label(t!("ircoid-back"))
                        .on_click(|_, window, cx| window.close_dialog(cx)),
                ),
            )
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

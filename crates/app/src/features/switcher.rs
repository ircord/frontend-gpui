//! The quick switcher (Ctrl+K) (package N1): type part of a conversation's
//! or network's name and press Enter. With nothing typed it lists the
//! conversations you opened last. The last entry searches messages for
//! what you typed (the search page is FD1's).

use std::collections::VecDeque;

use gpui_kit::TestSupportExt;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::*;
use gpui_kit::*;
use ircord_client::contract::ConversationKind;
use ircord_client::store::Workspace;
use ircord_client::store::sidebar_order as order;

use crate::route::Route;
use crate::session::{Session, SessionEvent};
use crate::sidebar::layout;
use crate::theme;
use crate::ui;

/// Results shown at most.
const LIMIT: usize = 12;
/// Conversations remembered as recent.
const RECENT: usize = 20;

/// Conversations opened, newest first.
#[derive(Default)]
struct Recent(VecDeque<String>);
impl Global for Recent {}

pub fn init(cx: &mut App) {
    cx.default_global::<Recent>();
    cx.observe_new::<Session>(|_, _, cx| {
        cx.subscribe_self(|session: &mut Session, event: &SessionEvent, cx| {
            if let SessionEvent::Selection = event
                && let Some(id) = session.conversation.clone()
            {
                let recent = &mut cx.global_mut::<Recent>().0;
                recent.retain(|r| *r != id);
                recent.push_front(id);
                recent.truncate(RECENT);
            }
        })
        .detach();
    })
    .detach();
}

/// One result.
#[derive(Debug, Clone, PartialEq)]
pub enum Pick {
    Conversation(String),
    Network(String),
    /// Search messages for the text.
    Search(String),
}

/// How well `query` matches `name`: a prefix beats a word start beats a
/// substring beats letters in order. `None` when it does not match.
pub fn score(name: &str, query: &str) -> Option<u32> {
    let name = name.to_lowercase();
    let query = query.trim().trim_start_matches(['#', '@']).to_lowercase();
    if query.is_empty() {
        return Some(0);
    }
    if name.starts_with(&query) {
        return Some(400 - name.len().min(300) as u32);
    }
    if let Some(at) = name.find(&query) {
        let word = name[..at].ends_with(|c: char| !c.is_alphanumeric());
        return Some(if word { 300 } else { 200 } - (at as u32).min(150));
    }
    let mut rest = name.chars();
    query
        .chars()
        .all(|q| rest.by_ref().any(|c| c == q))
        .then_some(100)
}

/// The results for `query`, best first.
pub fn results(w: &Workspace, query: &str, recent: &[String]) -> Vec<Pick> {
    let query = query.trim();
    if query.is_empty() {
        let mut picks: Vec<Pick> = recent
            .iter()
            .filter(|id| w.conversation(id).is_some())
            .map(|id| Pick::Conversation(id.clone()))
            .collect();
        // Then what wants attention, then everything else in sidebar order.
        let rest = w
            .networks
            .iter()
            .flat_map(|n| order::ordered(w, &n.id))
            .filter(|c| !recent.contains(&c.id));
        let (waiting, quiet): (Vec<_>, Vec<_>) = rest.partition(|c| order::has_unread(c));
        picks.extend(
            waiting
                .into_iter()
                .chain(quiet)
                .map(|c| Pick::Conversation(c.id.clone())),
        );
        picks.truncate(LIMIT);
        return picks;
    }
    let mut scored: Vec<(u32, usize, Pick)> = Vec::new();
    for (i, c) in w.conversations.iter().enumerate() {
        if let Some(s) = score(&c.name, query) {
            // Recent and unread ones first among equals.
            let boost =
                u32::from(recent.contains(&c.id)) * 20 + u32::from(order::has_unread(c)) * 10;
            scored.push((s + boost, i, Pick::Conversation(c.id.clone())));
        }
    }
    for (i, n) in w.networks.iter().enumerate() {
        if let Some(s) = score(&n.name, query) {
            scored.push((s, w.conversations.len() + i, Pick::Network(n.id.clone())));
        }
    }
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    let mut picks: Vec<Pick> = scored.into_iter().map(|(_, _, p)| p).take(LIMIT).collect();
    picks.push(Pick::Search(query.to_string()));
    picks
}

pub struct SwitcherView {
    session: Entity<Session>,
    input: Entity<InputState>,
    selected: usize,
    _subscription: Subscription,
}

/// Open the switcher over the window.
pub fn open(window: &mut Window, cx: &mut App) {
    let Some(session) = layout::session(cx) else {
        return;
    };
    if session.read(cx).client().is_none() {
        return;
    }
    let view = cx.new(|cx| SwitcherView::new(session, window, cx));
    let input = view.read(cx).input.clone();
    let focus = std::cell::Cell::new(true);
    ui::dialog::open(
        window,
        cx,
        t!("sidebar-switcher-title"),
        move |dialog, window, cx| {
            if focus.replace(false) {
                input.focus_handle(cx).focus(window, cx);
            }
            dialog.w(px(520.)).child(view.clone())
        },
    );
}

impl SwitcherView {
    fn new(session: Entity<Session>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx
            .new(|cx| InputState::new(window, cx).placeholder(t!("sidebar-switcher-placeholder")));
        let subscription = cx.subscribe_in(
            &input,
            window,
            |this: &mut Self, _, event: &InputEvent, window, cx| match event {
                InputEvent::Change => {
                    this.selected = 0;
                    cx.notify();
                }
                InputEvent::PressEnter { .. } => this.choose(this.selected, window, cx),
                _ => {}
            },
        );
        Self {
            session,
            input,
            selected: 0,
            _subscription: subscription,
        }
    }

    fn picks(&self, cx: &App) -> Vec<Pick> {
        let query = self.input.read(cx).value().to_string();
        let recent: Vec<String> = cx
            .try_global::<Recent>()
            .map(|r| r.0.iter().cloned().collect())
            .unwrap_or_default();
        let session = self.session.read(cx);
        match session.client() {
            Some(client) => results(&client.read(), &query, &recent),
            None => Vec::new(),
        }
    }

    fn choose(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(pick) = self.picks(cx).get(index).cloned() else {
            return;
        };
        ui::dialog::close(window, cx);
        self.session.update(cx, |s, cx| match pick {
            Pick::Conversation(id) => s.select_conversation(&id, cx),
            Pick::Network(id) => s.select_network(&id, cx),
            Pick::Search(_) => s.navigate(Route::Search, cx),
        });
    }
}

impl Render for SwitcherView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme::get(cx).clone();
        let picks = self.picks(cx);
        self.selected = self.selected.min(picks.len().saturating_sub(1));
        let session = self.session.read(cx);
        let client = session.client().cloned();
        let mut list = div()
            .id("switcher-results")
            .test_support()
            .role(Role::ListBox)
            .aria_label(t!("sidebar-switcher-results"))
            .flex()
            .flex_col()
            .gap_0p5()
            .max_h(px(380.))
            .overflow_y_scroll();
        for (i, pick) in picks.iter().enumerate() {
            let (title, detail) = match (pick, &client) {
                (Pick::Conversation(id), Some(client)) => {
                    let w = client.read();
                    match w.conversation(id) {
                        Some(c) => {
                            let prefix = if c.kind == ConversationKind::Channel {
                                "#"
                            } else {
                                "@"
                            };
                            let network = w.network(&c.network_id).map(|n| n.name.clone());
                            (format!("{prefix}{}", c.name), network)
                        }
                        None => continue,
                    }
                }
                (Pick::Network(id), Some(client)) => {
                    let w = client.read();
                    match w.network(id) {
                        Some(n) => (
                            n.name.clone(),
                            Some(t!("sidebar-switcher-network").to_string()),
                        ),
                        None => continue,
                    }
                }
                (Pick::Search(query), _) => (t!("sidebar-switcher-search", query = query), None),
                _ => continue,
            };
            let selected = i == self.selected;
            let label = match &detail {
                Some(d) => format!("{title}, {d}"),
                None => title.clone(),
            };
            list = list.child(
                div()
                    .id(ElementId::Name(format!("switcher-result-{i}").into()))
                    .test_support()
                    .role(Role::ListBoxOption)
                    .aria_label(SharedString::from(label))
                    .aria_selected(selected)
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .flex()
                    .items_center()
                    .gap_2()
                    .cursor_pointer()
                    .when(selected, |d| d.bg(theme.accent).text_color(theme.on_accent))
                    .when(!selected, |d| d.hover(|d| d.bg(theme.accent.opacity(0.15))))
                    .child(div().flex_1().truncate().child(title))
                    .children(detail.map(|d| div().text_xs().opacity(0.7).child(d)))
                    .on_click(cx.listener(move |this, _, window, cx| this.choose(i, window, cx))),
            );
        }
        let count = picks.len();
        div()
            .id("switcher")
            .test_support()
            .flex()
            .flex_col()
            .gap_2()
            .capture_key_down(cx.listener(move |this, e: &KeyDownEvent, _, cx| {
                let k = &e.keystroke;
                if k.modifiers.modified() {
                    return;
                }
                match k.key.as_str() {
                    "down" if count > 0 => this.selected = (this.selected + 1) % count,
                    "up" if count > 0 => this.selected = (this.selected + count - 1) % count,
                    _ => return,
                }
                cx.stop_propagation();
                cx.notify();
            }))
            .child(
                Input::new(&self.input)
                    .id("switcher-input")
                    .aria_label(t!("sidebar-switcher-placeholder")),
            )
            .child(list)
    }
}

#[cfg(test)]
mod tests {
    use super::score;

    #[test]
    fn prefixes_beat_words_beat_substrings_beat_letters() {
        let prefix = score("random", "ran").unwrap();
        let word = score("the-random", "ran").unwrap();
        let inside = score("grandma", "ran").unwrap();
        let letters = score("rusty-anchor", "ran").unwrap();
        assert!(prefix > word && word > inside && inside > letters);
        assert_eq!(score("general", "xyz"), None);
        assert!(score("general", "#gen").is_some(), "a leading # is ignored");
    }
}

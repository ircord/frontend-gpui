//! Find an IRCord account by username or by a nickname it uses on the IRC
//! server, after `AdminAccountPicker.vue`. Used to add owners and emoji
//! managers. Matches that cannot be chosen are shown with the server's
//! note; Enter or the button takes the first one that can.

use crate::ui::focus::Reachable;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::TestSupportExt;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::*;
use gpui_kit::*;
use ircord_client::api::admin as a;
use ircord_client::contract as c;

use super::{Kind, button, client, error_text, notice};
use crate::session::Session;
use crate::theme;

/// How long typing pauses before the server is asked.
const DEBOUNCE: Duration = Duration::from_millis(250);

type Choose = Rc<dyn Fn(c::AdminUser, &mut Window, &mut App)>;

pub struct Picker {
    session: Entity<Session>,
    /// The prefix of every element ID: `{id}-input`, `{id}-match-0`…
    id: &'static str,
    /// The button's text ("Make owner", "Add").
    action: &'static str,
    /// Shown beside an owner among the matches.
    owner_note: &'static str,
    input: Entity<InputState>,
    query: String,
    matches: Vec<c::IdentityMatch>,
    error: Option<String>,
    loading: bool,
    generation: u64,
    /// The section is saving: nothing can be chosen meanwhile.
    pub busy: bool,
    on_choose: Choose,
    _subscription: Subscription,
}

impl Picker {
    pub fn new(
        session: Entity<Session>,
        id: &'static str,
        action: &'static str,
        owner_note: &'static str,
        on_choose: impl Fn(c::AdminUser, &mut Window, &mut App) + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let input =
            cx.new(|cx| InputState::new(window, cx).placeholder(t!("admin-username-or-nickname")));
        let subscription = cx.subscribe_in(
            &input,
            window,
            |this: &mut Self, input, event: &InputEvent, window, cx| match event {
                InputEvent::Change => {
                    let text = input.read(cx).value().to_string();
                    this.typed(text, cx);
                }
                InputEvent::PressEnter { .. } => this.choose(None, window, cx),
                _ => {}
            },
        );
        Self {
            session,
            id,
            action,
            owner_note,
            input,
            query: String::new(),
            matches: Vec::new(),
            error: None,
            loading: false,
            generation: 0,
            busy: false,
            on_choose: Rc::new(on_choose),
            _subscription: subscription,
        }
    }

    fn typed(&mut self, text: String, cx: &mut Context<Self>) {
        self.generation += 1;
        let generation = self.generation;
        self.query = text.trim().to_string();
        if self.query.is_empty() {
            self.matches.clear();
            self.error = None;
            self.loading = false;
            cx.notify();
            return;
        }
        let Some(client) = client(&self.session, cx) else {
            return;
        };
        self.loading = true;
        let query = self.query.clone();
        let settle = super::pause(&self.session, cx, DEBOUNCE);
        cx.spawn(async move |this, cx| {
            let _ = settle.await;
            let still = this
                .update(cx, |this, _| this.generation == generation)
                .unwrap_or(false);
            if !still {
                return;
            }
            let result = client
                .request(move |api| async move { api.administration_identities(&query).await })
                .await;
            this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                this.loading = false;
                match result {
                    Ok(Ok(page)) => {
                        this.matches = page.items;
                        this.error = None;
                    }
                    Ok(Err(e)) => {
                        super::refused(&client, &e, cx);
                        this.matches.clear();
                        this.error = Some(error_text(&e));
                    }
                    Err(e) => this.error = Some(e.to_string()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn first(&self) -> Option<usize> {
        self.matches
            .iter()
            .position(|m| m.eligible && m.user.is_some())
    }

    /// Choose match `index`, or the first that can be chosen.
    fn choose(&mut self, index: Option<usize>, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let Some(index) = index.or_else(|| self.first()) else {
            return;
        };
        let Some(user) = self
            .matches
            .get(index)
            .filter(|m| m.eligible)
            .and_then(|m| m.user.clone())
        else {
            return;
        };
        self.generation += 1;
        self.matches.clear();
        self.query.clear();
        self.input.update(cx, |s, cx| s.set_value("", window, cx));
        cx.notify();
        let on_choose = self.on_choose.clone();
        window.defer(cx, move |window, cx| on_choose(user, window, cx));
    }
}

impl Render for Picker {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme::get(cx).clone();
        let id = self.id;
        let chosen = self.first().is_some() && !self.busy;
        let row = div()
            .flex()
            .items_center()
            .gap_2()
            .child(
                div().flex_1().child(
                    Input::new(&self.input)
                        .id(ElementId::Name(format!("{id}-input").into()))
                        .aria_label(t!("admin-username-or-nickname")),
                ),
            )
            .child(button(
                ElementId::Name(format!("{id}-choose").into()),
                self.action,
                None,
                Kind::Primary,
                chosen,
                &theme,
                cx.listener(|this, _, window, cx| this.choose(None, window, cx)),
            ));
        let mut body = div().flex().flex_col().gap_1().child(row);
        if !self.matches.is_empty() {
            let mut list = div()
                .id(ElementId::Name(format!("{id}-matches").into()))
                .test_support()
                .role(Role::ListBox)
                .aria_label(t!("admin-matches"))
                .flex()
                .flex_col()
                .gap_0p5();
            for (i, m) in self.matches.iter().enumerate() {
                let can = m.eligible && m.user.is_some();
                let label = a::describe(m);
                let note = m.note.clone().or_else(|| {
                    m.user
                        .as_ref()
                        .is_some_and(|u| u.owner)
                        .then(|| self.owner_note.to_string())
                });
                let aria = match &note {
                    Some(note) => format!("{label}, {note}"),
                    None => label.clone(),
                };
                list = list.child(
                    div()
                        .id(ElementId::Name(format!("{id}-match-{i}").into()))
                        .test_support()
                        .role(Role::ListBoxOption)
                        .reachable(&theme)
                        .aria_label(aria)
                        .aria_selected(can && Some(i) == self.first())
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .text_sm()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(if can { theme.text } else { theme.faint })
                                .child(label),
                        )
                        .when_some(note, |d, note| {
                            d.child(div().text_xs().text_color(theme.muted).child(note))
                        })
                        .when(can, |d| {
                            let hover = theme.hover;
                            d.cursor_pointer()
                                .hover(move |d| d.bg(hover))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.choose(Some(i), window, cx)
                                }))
                        }),
                );
            }
            body = body.child(list);
        }
        if let Some(error) = &self.error {
            body = body.child(notice("admin-picker-error", error.clone(), true, &theme));
        } else if !self.query.is_empty() && self.matches.is_empty() && !self.loading {
            body = body.child(notice(
                "admin-picker-none",
                t!("admin-no-account-match", query = self.query.as_str()),
                false,
                &theme,
            ));
        }
        body
    }
}

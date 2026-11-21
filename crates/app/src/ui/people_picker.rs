//! The shared people picker (package P1), used by invitations, group
//! creation and channel moderation: type part of a nick, an account or a
//! display name and pick a person. People known in the workspace show at
//! once; the server's people lookup (`GET /people`) adds the rest for the
//! purpose given, such as registered accounts for a group.
//!
//! ```ignore
//! ui::people_picker::open(window, cx, Picker {
//!     title: t!("parties-invite-title").into(),
//!     session, network, purpose: Purpose::Party, conversation: None,
//! }, |picked, window, cx| { /* picked.nick, picked.member */ });
//! ```
//!
//! Or embed [`PeoplePicker`] in a form of your own with [`PeoplePicker::new`].

use std::rc::Rc;

use gpui_kit::TestSupportExt;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::*;
use gpui_kit::*;
use ircord_client::api::people::{PeopleQuery, Purpose};
use ircord_client::commands::run::{nickname_error, nickname_limit};
use ircord_client::contract::{Person, PersonPresence};
use ircord_client::store::people;

use crate::session::Session;
use crate::theme;

/// Results shown at most.
const LIMIT: usize = 12;

/// The person picked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Picked {
    /// Their member ID, when the workspace knows them.
    pub member: Option<String>,
    pub nick: String,
    /// What the picker showed: the display name, else the nick.
    pub name: String,
    /// Their services account, when the server said.
    pub account: Option<String>,
    /// Nobody matched: the nick as typed.
    pub typed: bool,
}

pub type OnPick = Rc<dyn Fn(&Picked, &mut Window, &mut App)>;

/// Where a picker looks.
#[derive(Clone)]
pub struct Picker {
    pub title: SharedString,
    pub session: Entity<Session>,
    pub network: String,
    pub purpose: Purpose,
    pub conversation: Option<String>,
}

pub struct PeoplePicker {
    session: Entity<Session>,
    network: String,
    purpose: Purpose,
    conversation: Option<String>,
    pub input: Entity<InputState>,
    selected: usize,
    /// What the server found for `searched`.
    remote: Vec<Person>,
    searched: String,
    searching: bool,
    on_pick: OnPick,
    _subscription: Subscription,
}

/// Open a picker in a dialog; it closes when someone is picked.
pub fn open(
    window: &mut Window,
    cx: &mut App,
    picker: Picker,
    on_pick: impl Fn(&Picked, &mut Window, &mut App) + 'static,
) {
    let on_pick: OnPick = Rc::new(on_pick);
    let view = cx.new(|cx| {
        PeoplePicker::new(
            &picker,
            Rc::new(move |picked: &Picked, window: &mut Window, cx: &mut App| {
                super::dialog::close(window, cx);
                on_pick(picked, window, cx);
            }),
            window,
            cx,
        )
    });
    let input = view.read(cx).input.clone();
    let focus = std::cell::Cell::new(true);
    super::dialog::open(
        window,
        cx,
        picker.title.clone(),
        move |dialog, window, cx| {
            if focus.replace(false) {
                input.focus_handle(cx).focus(window, cx);
            }
            dialog.w(px(460.)).child(view.clone())
        },
    );
}

impl PeoplePicker {
    pub fn new(
        picker: &Picker,
        on_pick: OnPick,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let input =
            cx.new(|cx| InputState::new(window, cx).placeholder(t!("people-picker-placeholder")));
        let subscription = cx.subscribe_in(
            &input,
            window,
            |this: &mut Self, _, event: &InputEvent, window, cx| match event {
                InputEvent::Change => {
                    this.selected = 0;
                    this.lookup(cx);
                    cx.notify();
                }
                InputEvent::PressEnter { .. } => this.choose(this.selected, window, cx),
                _ => {}
            },
        );
        let mut this = Self {
            session: picker.session.clone(),
            network: picker.network.clone(),
            purpose: picker.purpose,
            conversation: picker.conversation.clone(),
            input,
            selected: 0,
            remote: Vec::new(),
            searched: String::new(),
            searching: false,
            on_pick,
            _subscription: subscription,
        };
        this.lookup(cx);
        this
    }

    fn query(&self, cx: &App) -> String {
        self.input.read(cx).value().trim().to_string()
    }

    /// Ask the server for people matching what is typed.
    fn lookup(&mut self, cx: &mut Context<Self>) {
        let Some(client) = self.session.read(cx).client().cloned() else {
            return;
        };
        let query = PeopleQuery {
            network_id: self.network.clone(),
            purpose: self.purpose,
            conversation_id: self.conversation.clone(),
            query: self.query(cx).trim_start_matches('@').to_string(),
            limit: LIMIT as u32,
        };
        self.searching = true;
        let asked = query.query.clone();
        let task = client
            .handle()
            .request(move |api| async move { api.people(&query).await });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                // An answer to an older query is dropped.
                if this.query(cx).trim_start_matches('@') != asked {
                    return;
                }
                this.searching = false;
                match result {
                    Ok(Ok(page)) => {
                        this.remote = page.items;
                        this.searched = asked;
                    }
                    Ok(Err(e)) => log::info!("people lookup: {e}"),
                    Err(e) => log::warn!("people lookup: {e}"),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The people to offer for what is typed: those the workspace knows,
    /// then the server's, then the typed nick itself when it is one.
    pub fn options(&self, cx: &App) -> Vec<Picked> {
        let query = self.query(cx);
        let Some(client) = self.session.read(cx).client() else {
            return Vec::new();
        };
        let w = client.read();
        let mut options: Vec<Picked> = people::find_people(&w, &self.network, &query, LIMIT)
            .into_iter()
            .map(|m| Picked {
                member: Some(m.id.clone()),
                nick: m.nick.clone(),
                name: people::name(m).to_string(),
                account: (!m.account.is_empty()).then(|| m.account.clone()),
                typed: false,
            })
            .collect();
        let bare = query.trim_start_matches('@').to_lowercase();
        if self.searched.to_lowercase() == bare {
            for p in self.remote.iter().filter(|p| p.eligible) {
                let nick = p.nickname.clone().unwrap_or_default();
                let known = options.iter().any(|o| {
                    (p.member_id.is_some() && o.member == p.member_id)
                        || (!nick.is_empty() && o.nick.eq_ignore_ascii_case(&nick))
                });
                if known || options.len() >= LIMIT {
                    continue;
                }
                let nick = if nick.is_empty() {
                    p.services_account.clone().unwrap_or_default()
                } else {
                    nick
                };
                if nick.is_empty() {
                    continue;
                }
                options.push(Picked {
                    member: p.member_id.clone(),
                    name: if p.display_name.is_empty() {
                        nick.clone()
                    } else {
                        p.display_name.clone()
                    },
                    nick,
                    account: p.services_account.clone(),
                    typed: false,
                });
            }
        }
        let typed = query.trim_start_matches('@');
        let limit = w
            .network(&self.network)
            .map(|n| nickname_limit(&n.isupport))
            .unwrap_or(32);
        if !typed.is_empty()
            && nickname_error(typed, limit).is_none()
            && !options.iter().any(|o| o.nick.eq_ignore_ascii_case(typed))
            && matches!(self.purpose, Purpose::Dm | Purpose::Command | Purpose::Mask)
        {
            options.push(Picked {
                member: None,
                nick: typed.to_string(),
                name: typed.to_string(),
                account: None,
                typed: true,
            });
        }
        options
    }

    fn choose(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(picked) = self.options(cx).get(index).cloned() else {
            return;
        };
        let on_pick = self.on_pick.clone();
        on_pick(&picked, window, cx);
    }
}

fn presence_color(person: Option<PersonPresence>, theme: &theme::Theme) -> Hsla {
    match person {
        Some(PersonPresence::Online) => theme.online,
        Some(PersonPresence::Away) => theme.away,
        _ => theme.offline,
    }
}

impl Render for PeoplePicker {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme::get(cx).clone();
        let options = self.options(cx);
        self.selected = self.selected.min(options.len().saturating_sub(1));
        let presence = |o: &Picked| -> Option<PersonPresence> {
            let session = self.session.read(cx);
            let w = session.client()?.read();
            match o.member.as_ref().and_then(|id| w.members.get(id)) {
                Some(m) => Some(match m.status {
                    ircord_client::contract::MemberStatus::Online => PersonPresence::Online,
                    ircord_client::contract::MemberStatus::Away => PersonPresence::Away,
                    _ => PersonPresence::Offline,
                }),
                None => self
                    .remote
                    .iter()
                    .find(|p| p.nickname.as_deref() == Some(&o.nick))
                    .map(|p| p.presence),
            }
        };
        let mut list = div()
            .id("people-picker-results")
            .test_support()
            .role(Role::ListBox)
            .aria_label(t!("people-picker-results"))
            .flex()
            .flex_col()
            .gap_0p5()
            .max_h(px(320.))
            .overflow_y_scroll();
        for (i, option) in options.iter().enumerate() {
            let selected = i == self.selected;
            let typed = option.typed;
            let title = if typed {
                t!("people-picker-use-nick", nick = option.nick)
            } else {
                option.name.clone()
            };
            let detail = (!typed && option.name != option.nick).then(|| option.nick.clone());
            let label = match &detail {
                Some(d) => format!("{title}, {d}"),
                None => title.clone(),
            };
            let dot = presence_color(presence(option), &theme);
            list = list.child(
                div()
                    .id(ElementId::Name(format!("people-picker-option-{i}").into()))
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
                    .when(!typed, |d| {
                        d.child(div().size(px(8.)).flex_none().rounded_full().bg(dot))
                    })
                    .child(div().flex_1().truncate().child(title))
                    .children(detail.map(|d| div().text_xs().opacity(0.7).child(d)))
                    .on_click(cx.listener(move |this, _, window, cx| this.choose(i, window, cx))),
            );
        }
        if options.is_empty() {
            let text = if self.searching {
                t!("people-picker-searching")
            } else {
                t!("people-picker-nobody")
            };
            list = list.child(
                div()
                    .id("people-picker-empty")
                    .test_support()
                    .role(Role::Status)
                    .aria_label(text)
                    .px_2()
                    .py_1()
                    .text_sm()
                    .text_color(theme.muted)
                    .child(text),
            );
        }
        let count = options.len();
        div()
            .id("people-picker")
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
                    .id("people-picker-input")
                    .aria_label(t!("people-picker-placeholder")),
            )
            .child(list)
    }
}

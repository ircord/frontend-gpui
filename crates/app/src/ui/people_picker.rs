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
//! An embedding form can hide people ([`PeoplePicker::hide`]), mark those
//! who cannot be picked with the reason ([`PeoplePicker::blocked`]), and
//! offer what is typed as an account name ([`PeoplePicker::typed_accounts`],
//! for encrypted groups).

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
/// Whether an option is left out of the list.
pub type Hide = Rc<dyn Fn(&Picked, &App) -> bool>;
/// Why an option cannot be picked, if it cannot.
pub type Blocked = Rc<dyn Fn(&Picked, &App) -> Option<SharedString>>;

/// Where a picker looks.
#[derive(Clone)]
pub struct Picker {
    pub title: SharedString,
    pub session: Entity<Session>,
    pub network: String,
    pub purpose: Purpose,
    pub conversation: Option<String>,
    /// Member IDs left out of the list, such as people already in the
    /// channel an invitation is for.
    pub except: Vec<String>,
}

pub struct PeoplePicker {
    session: Entity<Session>,
    network: String,
    purpose: Purpose,
    conversation: Option<String>,
    except: Vec<String>,
    pub input: Entity<InputState>,
    selected: usize,
    /// What the server found for `searched`.
    remote: Vec<Person>,
    searched: String,
    searching: bool,
    on_pick: OnPick,
    /// Options left out of the list (people already chosen).
    pub hide: Option<Hide>,
    /// Options shown but not pickable, with the reason under the name.
    pub blocked: Option<Blocked>,
    /// A line under an option's name that does not stop it being picked.
    pub note: Option<Blocked>,
    /// Offer what is typed, when it matches nobody, as an account name.
    pub typed_accounts: bool,
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
            except: picker.except.clone(),
            input,
            selected: 0,
            remote: Vec::new(),
            searched: String::new(),
            searching: false,
            on_pick,
            hide: None,
            blocked: None,
            note: None,
            typed_accounts: false,
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
        // Asks for a few more than shown, so leaving people out still fills the list.
        let mut options: Vec<Picked> =
            people::find_people(&w, &self.network, &query, LIMIT + self.except.len())
                .into_iter()
                .filter(|m| !self.except.contains(&m.id))
                .take(LIMIT)
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
            for p in self.remote.iter().filter(|p| {
                p.eligible
                    && !p
                        .member_id
                        .as_ref()
                        .is_some_and(|id| self.except.contains(id))
            }) {
                let nick = p.nickname.clone().unwrap_or_default();
                let known = options.iter().any(|o| {
                    (p.member_id.is_some() && o.member == p.member_id)
                        || (!nick.is_empty() && o.nick.eq_ignore_ascii_case(&nick))
                        || (o.account.is_some() && o.account == p.services_account)
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
        // A nick left out above is not offered as a typed one either.
        let left_out = self
            .except
            .iter()
            .filter_map(|id| w.member(id, &self.network))
            .any(|m| m.nick.eq_ignore_ascii_case(typed));
        if self.typed_accounts {
            if !typed.is_empty()
                && !typed.contains(|c: char| c.is_whitespace() || c == ',')
                && !options.iter().any(|o| {
                    o.account
                        .as_deref()
                        .is_some_and(|a| a.eq_ignore_ascii_case(typed))
                })
            {
                options.push(Picked {
                    member: None,
                    nick: typed.to_string(),
                    name: typed.to_string(),
                    account: Some(typed.to_string()),
                    typed: true,
                });
            }
        } else if !typed.is_empty()
            && nickname_error(typed, limit).is_none()
            && !left_out
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
        if let Some(hide) = &self.hide {
            options.retain(|o| !hide(o, cx));
        }
        options
    }

    /// Why `option` cannot be picked, if it cannot.
    pub fn reason(&self, option: &Picked, cx: &App) -> Option<SharedString> {
        self.blocked.as_ref().and_then(|b| b(option, cx))
    }

    fn choose(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(picked) = self.options(cx).get(index).cloned() else {
            return;
        };
        if self.reason(&picked, cx).is_some() {
            return;
        }
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
        let reasons: Vec<Option<SharedString>> =
            options.iter().map(|o| self.reason(o, cx)).collect();
        let notes: Vec<Option<SharedString>> = options
            .iter()
            .map(|o| self.note.as_ref().and_then(|n| n(o, cx)))
            .collect();
        let server = self.session.read(cx).server().map(|s| s.url().clone());
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
            let title = if typed && self.typed_accounts {
                t!("people-picker-use-account", account = option.nick)
            } else if typed {
                t!("people-picker-use-nick", nick = option.nick)
            } else {
                option.name.clone()
            };
            let detail = (!typed && option.name != option.nick).then(|| option.nick.clone());
            let blocked = reasons[i].is_some();
            let reason = reasons[i].clone().or_else(|| notes[i].clone());
            let mut label = match &detail {
                Some(d) => format!("{title}, {d}"),
                None => title.clone(),
            };
            if let Some(reason) = &reason {
                label = format!("{label}. {reason}");
            }
            let dot = presence_color(presence(option), &theme);
            let picture = {
                let session = self.session.read(cx);
                let w = session.client().map(|c| c.read());
                let member = w
                    .as_ref()
                    .and_then(|w| option.member.as_ref().and_then(|id| w.members.get(id)));
                let size = px(super::avatar::TINY);
                match member {
                    Some(m) => super::avatar::person(
                        super::avatar::Person {
                            member: m,
                            name: &option.name,
                            server: server.as_ref(),
                            presence: false,
                            key: &format!("people-picker-{i}"),
                        },
                        size,
                        &theme,
                    ),
                    None => super::avatar::avatar(&option.name, None, size, &theme),
                }
            };
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
                    .when(!blocked, |d| d.cursor_pointer())
                    .when(selected, |d| d.bg(theme.accent).text_color(theme.on_accent))
                    .when(!selected && !blocked, |d| {
                        d.hover(|d| d.bg(theme.accent.opacity(0.15)))
                    })
                    .when(!typed, |d| {
                        d.child(picture.opacity(if blocked { 0.5 } else { 1. }))
                            .child(div().size(px(8.)).flex_none().rounded_full().bg(dot))
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .truncate()
                                    .when(blocked && !selected, |d| d.text_color(theme.muted))
                                    .child(title),
                            )
                            .children(reason.map(|r| {
                                div()
                                    .text_xs()
                                    .when(!selected, |d| d.text_color(theme.muted))
                                    .child(r)
                            })),
                    )
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

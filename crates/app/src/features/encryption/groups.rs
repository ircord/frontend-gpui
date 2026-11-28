//! Encrypted groups (experimental, phase 2 of ircord/issues#19): starting
//! one from the lock beside the sidebar's Direct messages heading, as the
//! web client does, the invitation card with Join and Decline,
//! and the group's details in the encryption panel: the trust caveat, every
//! person with their devices and whether they are verified, the security
//! code for verifying one, and for admins adding, removing and making
//! admins. Everyone can leave.
//!
//! The engine (`ircord_client::e2ee::Engine`) does the work; the core
//! decides who may change what, so the buttons here follow its rule.

use gpui_kit::TestSupportExt;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{Disableable, Sizable};
use gpui_kit::prelude::*;
use gpui_kit::*;
use ircord_client::contract as c;
use ircord_client::store::Workspace;
use ircord_client::store::e2ee::{Group, GroupEvent, GroupMember};

use super::{engine, qr_code, tag, with};
use crate::registry::{BannerCtx, Registry, SidebarCtx, Slot};
use crate::session::{Session, SessionEvent};
use crate::theme::{self, Theme};
use crate::ui;
use crate::ui::icon::{IconName, LucideIcon};
use crate::ui::people_picker::{PeoplePicker, Picked};
use ircord_client::api::people::Purpose;
use ircord_client::e2ee::Invitee;
use std::cell::RefCell;
use std::rc::Rc;

pub fn register(r: &mut Registry) {
    r.directs_heading
        .add(Slot::new("encryption-group-new", new_group_button));
    r.banners
        .add(Slot::new("encryption-invitations", invitations).order(4));
}

/// The lock beside the Direct messages heading that starts a group, where
/// groups are offered. Until this device is set up it says so and opens
/// Settings → Encryption instead.
fn new_group_button(ctx: &SidebarCtx, _: &mut Window, cx: &mut App) -> Option<AnyElement> {
    let network = ctx.network.as_deref()?;
    if !offered(&ctx.session, network, cx) {
        return None;
    }
    let ready = with(&ctx.session, cx, |w| w.e2ee.device.ready()).unwrap_or(false);
    let session = ctx.session.clone();
    let theme = theme::get(cx);
    let label: SharedString = if ready {
        t!("encryption-group-new-title").into()
    } else {
        t!("encryption-group-new-not-ready").into()
    };
    Some(
        ui::icon::button(
            "encryption-group-new",
            LucideIcon::Lock,
            label,
            theme,
            move |_, window, cx| {
                if ready {
                    open_new(&session, None, window, cx);
                } else {
                    crate::settings::open(&session, Some("encryption-device"), cx);
                }
            },
        )
        .into_any_element(),
    )
}

/// Whether encrypted groups are offered on `network`: the server has them
/// and the network is the one encryption works on.
fn offered(session: &Entity<Session>, network: &str, cx: &App) -> bool {
    with(session, cx, |w| {
        w.capabilities.encryption.as_ref().is_some_and(|e| e.groups)
            && w.network(network).is_some_and(|n| n.features.encryption)
    })
    .unwrap_or(false)
}

/// When each group was last looked at for pending invitees.
type Looked = std::collections::HashMap<String, std::time::Instant>;
thread_local! {
    static LOOKED: RefCell<Looked> = RefCell::default();
}

/// Ask whether the people invited to `group` before they had encryption
/// keys have them now, and add those who do: when the group is opened, and
/// at most every 15 seconds while it stays open.
pub fn look_for_pending(session: &Entity<Session>, conversation: &str, group: &Group, cx: &App) {
    if group.pending.is_empty() || !group.admin {
        return;
    }
    let due = LOOKED.with_borrow_mut(|looked| {
        let due = looked
            .get(conversation)
            .is_none_or(|at| at.elapsed() >= std::time::Duration::from_secs(15));
        if due {
            looked.insert(conversation.to_string(), std::time::Instant::now());
        }
        due
    });
    if due && let Some(engine) = engine(session, cx) {
        engine.check_pending(conversation);
    }
}

/// The group in `conversation`, if it is one.
pub fn group_of(session: &Entity<Session>, conversation: &str, cx: &App) -> Option<Group> {
    with(session, cx, |w| w.e2ee.group(conversation).cloned()).flatten()
}

/// The words for a group action's error code.
pub fn error_text(error: &str) -> String {
    let (code, account) = error.split_once(':').unwrap_or((error, ""));
    match code {
        "peer-not-set-up" => t!("encryption-blocked-peer", name = account),
        "peer-no-answer" => t!("encryption-blocked-no-answer", name = account),
        "needs-update" => t!("encryption-group-needs-update", name = account),
        "group-needs-people" => t!("encryption-group-needs-people").into(),
        "group-nobody-ready" => t!("encryption-group-nobody-ready").into(),
        "group-raced" => t!("encryption-group-raced").into(),
        "not-admin" => t!("encryption-group-not-admin").into(),
        "not-in-group" => t!("encryption-group-not-in").into(),
        other => t!("encryption-error", error = other),
    }
}

// ---- Starting a group, adding people ----

/// Open the picker: a new group, or people to add to the group in
/// `conversation`.
pub fn open_new(
    session: &Entity<Session>,
    add_to: Option<String>,
    window: &mut Window,
    cx: &mut App,
) {
    let view = cx.new(|cx| Picker::new(session.clone(), add_to.clone(), window, cx));
    let title = if add_to.is_some() {
        t!("encryption-group-add-title")
    } else {
        t!("encryption-group-new-title")
    };
    let first = std::cell::Cell::new(true);
    ui::dialog::open(window, cx, title, move |dialog, window, cx| {
        if first.replace(false) {
            view.read(cx).first_field(cx).focus(window, cx);
        }
        dialog
            .w(px(520.))
            .on_ok(|_, _, _| false)
            .footer(view.update(cx, |v, cx| v.actions(cx)))
            .child(view.clone())
    });
}

/// One person chosen for the group: their account and the name shown.
#[derive(Clone, PartialEq, Eq)]
struct Chosen {
    account: String,
    name: String,
    /// Where the notice goes if they have no encryption keys yet.
    nick: String,
}

pub struct Picker {
    session: Entity<Session>,
    add_to: Option<String>,
    title: Entity<InputState>,
    /// Finds people: those you share a conversation with first, then the
    /// server's people lookup; an account name that matches nobody can be
    /// added as typed.
    people: Option<Entity<PeoplePicker>>,
    /// The picker's search box, kept here: picking runs while the picker
    /// is being updated, so it cannot be read through the picker then.
    search: Option<Entity<InputState>>,
    chosen: Rc<RefCell<Vec<Chosen>>>,
    _subscriptions: Vec<Subscription>,
}

impl Picker {
    fn new(
        session: Entity<Session>,
        add_to: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let title = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t!("encryption-group-title-placeholder"))
        });
        let chosen: Rc<RefCell<Vec<Chosen>>> = Rc::default();
        let present: Vec<String> = add_to
            .as_ref()
            .and_then(|conv| group_of(&session, conv, cx))
            .map(|g| g.members.iter().map(|m| m.account.clone()).collect())
            .unwrap_or_default();
        let home = with(&session, cx, |w| {
            w.networks
                .iter()
                .find(|n| n.features.encryption)
                .map(|n| (n.id.clone(), n.account.clone().unwrap_or_default()))
        })
        .flatten();
        let me = cx.entity().downgrade();
        let people = home.map(|(network, own)| {
            let finder = ui::people_picker::Picker {
                title: t!("encryption-group-people").into(),
                session: session.clone(),
                network,
                purpose: Purpose::Party,
                conversation: None,
                except: Vec::new(),
            };
            let on_pick = Rc::new(move |picked: &Picked, window: &mut Window, cx: &mut App| {
                let Some(account) = picked.account.clone() else {
                    return;
                };
                let person = Chosen {
                    account,
                    name: picked.name.clone(),
                    nick: picked.nick.clone(),
                };
                me.update(cx, |this, cx| this.choose(person, window, cx))
                    .ok();
            });
            let (hide_chosen, session2) = (chosen.clone(), session.clone());
            cx.new(|cx| {
                let mut p = PeoplePicker::new(&finder, on_pick, window, cx);
                p.typed_accounts = true;
                p.hide = Some(Rc::new(move |o: &Picked, _| {
                    o.account.as_deref().is_some_and(|a| {
                        a.eq_ignore_ascii_case(&own)
                            || present.iter().any(|p| p.eq_ignore_ascii_case(a))
                            || hide_chosen
                                .borrow()
                                .iter()
                                .any(|c| c.account.eq_ignore_ascii_case(a))
                    })
                }));
                let session3 = session2.clone();
                p.blocked = Some(Rc::new(move |o: &Picked, cx: &App| {
                    cannot_join(&session2, o, cx)
                }));
                p.note = Some(Rc::new(move |o: &Picked, cx: &App| {
                    key_note(&session3, o, cx)
                }));
                p
            })
        });
        let subscriptions = vec![cx.subscribe_in(
            &title,
            window,
            |this: &mut Self, _, e: &InputEvent, w, cx| {
                if let InputEvent::PressEnter { .. } = e {
                    this.submit(w, cx);
                }
            },
        )];
        Self {
            session,
            add_to,
            title,
            search: people.as_ref().map(|p| p.read(cx).input.clone()),
            people,
            chosen,
            _subscriptions: subscriptions,
        }
    }

    /// Where the dialog puts the caret first.
    fn first_field(&self, cx: &App) -> FocusHandle {
        match (&self.add_to, &self.search) {
            (Some(_), Some(search)) => search.focus_handle(cx),
            _ => self.title.focus_handle(cx),
        }
    }

    fn choose(&mut self, person: Chosen, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(engine) = engine(&self.session, cx) {
            engine.check_account(&person.account);
        }
        {
            let mut chosen = self.chosen.borrow_mut();
            if !chosen
                .iter()
                .any(|c| c.account.eq_ignore_ascii_case(&person.account))
            {
                chosen.push(person);
            }
        }
        if let Some(search) = self.search.clone() {
            search.update(cx, |s, cx| s.set_value(String::new(), window, cx));
        }
        cx.notify();
    }

    fn unchoose(&mut self, index: usize, cx: &mut Context<Self>) {
        let mut chosen = self.chosen.borrow_mut();
        if index < chosen.len() {
            chosen.remove(index);
        }
        drop(chosen);
        if let Some(people) = &self.people {
            people.update(cx, |_, cx| cx.notify());
        }
        cx.notify();
    }

    /// The people chosen.
    fn invitees(&self) -> Vec<Invitee> {
        self.chosen
            .borrow()
            .iter()
            .map(|c| Invitee {
                account: c.account.clone(),
                nick: c.nick.clone(),
            })
            .collect()
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let people = self.invitees();
        if people.is_empty() {
            return;
        }
        let Some(engine) = engine(&self.session, cx) else {
            return;
        };
        match &self.add_to {
            Some(conversation) => engine.add_people_inviting(conversation, people),
            None => {
                let title = self.title.read(cx).value().to_string();
                engine.create_group_inviting(&title, people);
            }
        }
        ui::dialog::close(window, cx);
    }

    fn actions(&self, cx: &mut Context<Self>) -> Div {
        let can = !self.chosen.borrow().is_empty();
        div()
            .w_full()
            .flex()
            .justify_end()
            .gap_2()
            .child(
                Button::new("encryption-group-cancel")
                    .ghost()
                    .label(t!("core-cancel"))
                    .on_click(|_, window, cx| ui::dialog::close(window, cx)),
            )
            .child(
                Button::new("encryption-group-submit")
                    .primary()
                    .label(if self.add_to.is_some() {
                        t!("encryption-group-add")
                    } else {
                        t!("encryption-group-create")
                    })
                    .disabled(!can)
                    .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx))),
            )
    }
}

/// Why `person` cannot be in an encrypted group, as the direct message lock
/// says it: a bot, or nobody signed in to an account. Someone without
/// encryption keys can be chosen: they are invited and wait (see [`waits`]).
fn cannot_join(session: &Entity<Session>, person: &Picked, cx: &App) -> Option<SharedString> {
    let bot = with(session, cx, |w| {
        person
            .member
            .as_ref()
            .and_then(|id| w.members.get(id))
            .is_some_and(|m| m.role == c::MemberRole::Bot)
    })
    .unwrap_or(false);
    if bot {
        return Some(t!("encryption-group-bot").into());
    }
    if person
        .account
        .as_deref()
        .is_none_or(|a| a.is_empty() || a == "*")
    {
        return Some(t!("encryption-blocked-no-account").into());
    }
    None
}

/// What a suggestion says about someone who has no encryption keys yet,
/// asking the server about keys it does not know yet. Not for an account
/// being typed: each would be asked about over IRC on every keystroke; it
/// is asked about once chosen.
fn key_note(session: &Entity<Session>, person: &Picked, cx: &App) -> Option<SharedString> {
    if person.typed {
        return None;
    }
    let account = person.account.as_deref()?;
    if waits(session, account, cx) {
        return Some(t!("encryption-group-no-keys").into());
    }
    if let Some(engine) = engine(session, cx) {
        engine.check_account(account);
    }
    None
}

/// Whether `account` is known to have no encryption keys yet: chosen, they
/// get an ordinary message saying how to set it up, and are added once
/// they have.
fn waits(session: &Entity<Session>, account: &str, cx: &App) -> bool {
    with(session, cx, |w| w.e2ee.peers.get(account) == Some(&false)).unwrap_or(false)
}

impl Render for Picker {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme::get(cx).clone();
        let mut body = div()
            .id("encryption-group-picker")
            .test_support()
            .flex()
            .flex_col()
            .gap_3();
        if self.add_to.is_none() {
            body = body
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.text)
                        .child(t!("encryption-group-title")),
                )
                .child(Input::new(&self.title));
        }
        body = body.child(
            div()
                .text_sm()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.text)
                .child(t!("encryption-group-people")),
        );
        body = body.child(
            div()
                .text_sm()
                .text_color(theme.muted)
                .child(t!("encryption-group-people-help")),
        );
        let chosen = self.chosen.borrow().clone();
        if !chosen.is_empty() {
            body = body.child(
                div()
                    .id("encryption-group-chosen")
                    .test_support()
                    .role(Role::List)
                    .aria_label(t!("encryption-group-chosen"))
                    .flex()
                    .flex_wrap()
                    .gap_1p5()
                    .children(chosen.into_iter().enumerate().map(|(i, person)| {
                        let label: SharedString = if person.name == person.account {
                            person.account.clone().into()
                        } else {
                            format!("{} ({})", person.name, person.account).into()
                        };
                        let waiting = waits(&self.session, &person.account, cx);
                        let label: SharedString = if waiting {
                            t!("encryption-group-pending-chip", name = label).into()
                        } else {
                            label
                        };
                        div()
                            .id(ElementId::Name(format!("encryption-group-chip-{i}").into()))
                            .test_support()
                            .role(Role::ListItem)
                            .aria_label(label.clone())
                            .flex()
                            .items_center()
                            .gap_1()
                            .pl_2()
                            .rounded_md()
                            .bg(theme.hover)
                            .text_sm()
                            .text_color(theme.text)
                            .when(waiting, |d| {
                                d.border_1()
                                    .border_dashed()
                                    .border_color(theme.muted)
                                    .child(ui::icon::icon(IconName::Loader, theme.muted))
                            })
                            .child(label)
                            .child(ui::icon::button(
                                ElementId::Name(format!("encryption-group-unchoose-{i}").into()),
                                IconName::Close,
                                t!("encryption-group-unchoose", name = person.name),
                                &theme,
                                cx.listener(move |this, _, _, cx| this.unchoose(i, cx)),
                            ))
                    })),
            );
        }
        let someone_waits = self
            .chosen
            .borrow()
            .iter()
            .any(|c| waits(&self.session, &c.account, cx));
        if someone_waits {
            body = body.child(
                div()
                    .id("encryption-group-pending-note")
                    .test_support()
                    .role(Role::Note)
                    .aria_label(t!("encryption-group-pending-note"))
                    .text_sm()
                    .text_color(theme.muted)
                    .child(t!("encryption-group-pending-note")),
            );
        }
        if let Some(people) = &self.people {
            // Backspace in an empty search takes back the last person chosen.
            let input = self.search.clone();
            body = body.child(
                div()
                    .capture_key_down(cx.listener(move |this, e: &KeyDownEvent, _, cx| {
                        if e.keystroke.key == "backspace"
                            && !e.keystroke.modifiers.modified()
                            && input
                                .as_ref()
                                .is_some_and(|i| i.read(cx).value().is_empty())
                        {
                            let last = this.chosen.borrow().len();
                            if last > 0 {
                                this.unchoose(last - 1, cx);
                                cx.stop_propagation();
                            }
                        }
                    }))
                    .child(people.clone()),
            );
        }
        body.child(
            div()
                .id("encryption-group-caveat-new")
                .test_support()
                .role(Role::Note)
                .aria_label(SharedString::from(t!("encryption-group-caveat")))
                .text_sm()
                .text_color(theme.muted)
                .child(t!("encryption-group-caveat")),
        )
        .child(tag(t!("encryption-experimental"), &theme))
    }
}

// ---- Invitations ----

fn invitations(ctx: &BannerCtx, _window: &mut Window, cx: &mut App) -> Option<AnyElement> {
    let (list, error) = with(&ctx.session, cx, |w| {
        (
            w.e2ee.invitations.clone(),
            w.e2ee.group_errors.get("new").cloned(),
        )
    })?;
    if list.is_empty() && error.is_none() {
        return None;
    }
    let theme = theme::get(cx).clone();
    let mut column = div().flex().flex_col();
    if let Some(error) = error {
        column = column.child(
            super::strip("encryption-group-error", theme.danger, &theme)
                .child(super::text_column(error_text(&error), &theme))
                .test_support(),
        );
    }
    for (i, inv) in list.into_iter().enumerate() {
        let name = with(&ctx.session, cx, |w| name_of(w, &inv.inviter)).unwrap_or_default();
        let busy =
            with(&ctx.session, cx, |w| w.e2ee.group_busy.contains(&inv.chat)).unwrap_or(false);
        let (s1, c1) = (ctx.session.clone(), inv.chat.clone());
        let (s2, c2) = (ctx.session.clone(), inv.chat.clone());
        column = column.child(
            div()
                .id(ElementId::Name(format!("encryption-invitation-{i}").into()))
                .test_support()
                .role(Role::Alert)
                .aria_label(SharedString::from(t!(
                    "encryption-invitation",
                    name = name.clone(),
                    title = inv.title.clone()
                )))
                .flex()
                .items_center()
                .gap_3()
                .px_4()
                .py_2()
                .border_b_1()
                .border_color(theme.border)
                .bg(theme.online.opacity(0.10))
                .child(ui::icon::icon(LucideIcon::Lock, theme.online))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .text_sm()
                        .text_color(theme.text)
                        .child(t!(
                            "encryption-invitation",
                            name = name.clone(),
                            title = inv.title.clone()
                        ))
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.muted)
                                .child(t!("encryption-invitation-people", count = inv.people)),
                        ),
                )
                .child(
                    Button::new(ElementId::Name(
                        format!("encryption-invitation-decline-{i}").into(),
                    ))
                    .small()
                    .label(t!("encryption-invitation-decline"))
                    .disabled(busy)
                    .on_click(move |_, _, cx| {
                        if let Some(engine) = engine(&s1, cx) {
                            engine.decline_invitation(&c1);
                        }
                    }),
                )
                .child(
                    Button::new(ElementId::Name(
                        format!("encryption-invitation-join-{i}").into(),
                    ))
                    .small()
                    .primary()
                    .label(t!("encryption-invitation-join"))
                    .disabled(busy)
                    .on_click(move |_, _, cx| {
                        if let Some(engine) = engine(&s2, cx) {
                            engine.accept_invitation(&c2);
                        }
                    }),
                ),
        );
    }
    Some(column.into_any_element())
}

/// A person's name for an account: their display name where the network
/// knows them, else the account.
pub fn name_of(w: &Workspace, account: &str) -> String {
    w.members
        .values()
        .find(|m| m.account == account)
        .map(|m| ircord_client::store::people::name(m).to_string())
        .unwrap_or_else(|| account.to_string())
}

// ---- The group's details, in the encryption panel ----

pub struct Details {
    session: Entity<Session>,
    conversation: String,
    /// The person whose security code is shown, and the code.
    verifying: Option<(String, Option<ircord_client::e2ee::engine::Code>)>,
    _subscription: Subscription,
}

impl Details {
    pub fn new(session: Entity<Session>, conversation: String, cx: &mut Context<Self>) -> Self {
        let subscription = cx.subscribe(&session, |_, _, event: &SessionEvent, cx| {
            if let SessionEvent::Changed(changes) = event
                && (changes.touches(ircord_client::store::e2ee::TOPIC) || changes.members)
            {
                cx.notify();
            }
        });
        Self {
            session,
            conversation,
            verifying: None,
            _subscription: subscription,
        }
    }

    pub fn session(&self) -> &Entity<Session> {
        &self.session
    }

    pub fn conversation(&self) -> &str {
        &self.conversation
    }

    fn verify(&mut self, account: String, cx: &mut Context<Self>) {
        if self.verifying.as_ref().is_some_and(|(a, _)| *a == account) {
            self.verifying = None;
            cx.notify();
            return;
        }
        self.verifying = Some((account.clone(), None));
        cx.notify();
        let Some(engine) = engine(&self.session, cx) else {
            return;
        };
        let task = self
            .session
            .read(cx)
            .runtime
            .spawn(async move { engine.code_for(&account).await });
        cx.spawn(async move |this, cx| {
            let code = task.await.ok().flatten();
            let _ = this.update(cx, |this, cx| {
                if let Some((_, slot)) = this.verifying.as_mut() {
                    *slot = code;
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn member_row(
        &self,
        i: usize,
        m: &GroupMember,
        group: &Group,
        name: String,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let status: SharedString = if m.you {
            t!("encryption-group-you").into()
        } else if m.started_fresh {
            t!("encryption-group-started-fresh").into()
        } else if m.verified {
            t!("encryption-group-verified").into()
        } else {
            t!("encryption-group-not-verified").into()
        };
        let role: Option<SharedString> = if m.creator {
            Some(t!("encryption-group-creator").into())
        } else if m.admin {
            Some(t!("encryption-group-admin").into())
        } else {
            None
        };
        let mut actions = div().flex().flex_wrap().gap_1();
        if !m.you {
            let account = m.account.clone();
            actions = actions.child(
                Button::new(ElementId::Name(
                    format!("encryption-group-verify-{i}").into(),
                ))
                .xsmall()
                .label(t!("encryption-group-verify"))
                .on_click(cx.listener(move |this, _, _, cx| this.verify(account.clone(), cx))),
            );
        }
        if group.admin && !m.you && !m.creator {
            let (s, conv, account) = (
                self.session.clone(),
                self.conversation.clone(),
                m.account.clone(),
            );
            actions = actions.child(
                Button::new(ElementId::Name(
                    format!("encryption-group-remove-{i}").into(),
                ))
                .xsmall()
                .danger()
                .label(t!("encryption-group-remove"))
                .on_click(move |_, _, cx| {
                    if let Some(engine) = engine(&s, cx) {
                        engine.remove_person(&conv, &account);
                    }
                }),
            );
        }
        if group.creator && !m.you {
            let (s, conv, account, admin) = (
                self.session.clone(),
                self.conversation.clone(),
                m.account.clone(),
                m.admin,
            );
            actions = actions.child(
                Button::new(ElementId::Name(
                    format!("encryption-group-admin-{i}").into(),
                ))
                .xsmall()
                .label(if admin {
                    t!("encryption-group-unmake-admin")
                } else {
                    t!("encryption-group-make-admin")
                })
                .on_click(move |_, _, cx| {
                    if let Some(engine) = engine(&s, cx) {
                        engine.set_admin(&conv, &account, !admin);
                    }
                }),
            );
        }
        let devices = m.devices.iter().map(|(id, this)| {
            let short: String = id.chars().take(8).collect();
            div()
                .text_xs()
                .text_color(theme.muted)
                .font_family(theme.mono_font.clone())
                .child(if *this {
                    t!("encryption-group-device-this", id = short)
                } else {
                    t!("encryption-group-device", id = short)
                })
        });
        div()
            .id(ElementId::Name(
                format!("encryption-group-member-{i}").into(),
            ))
            .test_support()
            .role(Role::Group)
            .aria_label(SharedString::from(format!("{name}: {status}")))
            .flex()
            .flex_col()
            .gap_0p5()
            .py_1()
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_x_2()
                    .child(
                        div()
                            .min_w_0()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.text)
                            .child(name),
                    )
                    .children(role.map(|r| tag_text(r, theme)))
                    .child(
                        div()
                            .text_xs()
                            .text_color(if m.started_fresh {
                                theme.warning
                            } else if m.verified {
                                theme.online
                            } else {
                                theme.muted
                            })
                            .child(status),
                    ),
            )
            .children(devices)
            .child(actions)
            .into_any_element()
    }
}

fn tag_text(text: SharedString, theme: &Theme) -> impl IntoElement {
    div()
        .px_1p5()
        .rounded_md()
        .text_xs()
        .text_color(theme.accent)
        .bg(theme.accent.opacity(0.14))
        .child(text)
}

fn event_text(w: &Workspace, e: &GroupEvent) -> String {
    let n = |a: &str| name_of(w, a);
    match e {
        GroupEvent::Added { by, account } => {
            t!(
                "encryption-group-event-added",
                by = n(by),
                name = n(account)
            )
        }
        GroupEvent::Removed { by, account } => {
            t!(
                "encryption-group-event-removed",
                by = n(by),
                name = n(account)
            )
        }
        GroupEvent::Left { account } => t!("encryption-group-event-left", name = n(account)),
        GroupEvent::Admin {
            by,
            account,
            admin: true,
        } => t!(
            "encryption-group-event-admin",
            by = n(by),
            name = n(account)
        ),
        GroupEvent::Admin { by, account, .. } => {
            t!(
                "encryption-group-event-unadmin",
                by = n(by),
                name = n(account)
            )
        }
        GroupEvent::DeviceAdded { account } => {
            t!("encryption-group-event-device-added", name = n(account))
        }
        GroupEvent::DeviceRemoved { account } => {
            t!("encryption-group-event-device-removed", name = n(account))
        }
    }
}

impl Render for Details {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme::get(cx).clone();
        let Some(group) = group_of(&self.session, &self.conversation, cx) else {
            return div().into_any_element();
        };
        let (busy, error, outsiders, names, events) = with(&self.session, cx, |w| {
            let accounts: Vec<&str> = group.members.iter().map(|m| m.account.as_str()).collect();
            // People in the channel who are not in the encrypted group: a
            // plain IRC user, or someone an operator let in.
            let outsiders: Vec<String> = w
                .conversation(&self.conversation)
                .map(|conv| {
                    conv.member_ids
                        .iter()
                        .filter(|id| !w.is_self(id, &conv.network_id))
                        .filter_map(|id| w.members.get(id))
                        .filter(|m| !accounts.contains(&m.account.as_str()))
                        .map(|m| ircord_client::store::people::name(m).to_string())
                        .collect()
                })
                .unwrap_or_default();
            let names: Vec<String> = group
                .members
                .iter()
                .map(|m| name_of(w, &m.account))
                .collect();
            let events: Vec<String> = group
                .events
                .iter()
                .rev()
                .take(12)
                .map(|e| event_text(w, e))
                .collect();
            (
                w.e2ee.group_busy.contains(&group.chat),
                w.e2ee.group_errors.get(&group.chat).cloned(),
                outsiders,
                names,
                events,
            )
        })
        .unwrap_or_default();
        let (verified, total) = group.verified();
        let state_text: String = if total > 0 && verified == total {
            t!("encryption-group-all-verified").to_string()
        } else {
            t!(
                "encryption-group-verified-count",
                count = verified,
                total = total
            )
        };
        let mut body = div()
            .id("encryption-group-panel")
            .test_support()
            .size_full()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap_3()
            .p_4()
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_2()
                    .child(ui::icon::icon(LucideIcon::Lock, theme.online))
                    .child(
                        div()
                            .text_color(theme.text)
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(group.title.clone()),
                    )
                    .child(tag(t!("encryption-experimental"), &theme)),
            )
            .child(
                div()
                    .id("encryption-group-state")
                    .test_support()
                    .role(Role::Status)
                    .aria_label(SharedString::from(state_text.clone()))
                    .text_sm()
                    .text_color(theme.text)
                    .child(state_text),
            )
            .child(
                div()
                    .id("encryption-group-caveat")
                    .test_support()
                    .role(Role::Note)
                    .aria_label(SharedString::from(t!("encryption-group-caveat")))
                    .text_sm()
                    .text_color(theme.muted)
                    .child(t!("encryption-group-caveat")),
            );
        if busy {
            body = body.child(
                div()
                    .text_xs()
                    .text_color(theme.muted)
                    .child(t!("encryption-busy")),
            );
        }
        if let Some(error) = error {
            body = body.child(
                div()
                    .id("encryption-group-panel-error")
                    .test_support()
                    .role(Role::Alert)
                    .aria_label(SharedString::from(error_text(&error)))
                    .text_sm()
                    .text_color(theme.danger)
                    .child(error_text(&error)),
            );
        }
        body = body.child(
            div()
                .text_sm()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.text)
                .child(t!("encryption-group-members", count = group.members.len())),
        );
        for (i, (m, name)) in group.members.iter().zip(names).enumerate() {
            body = body.child(self.member_row(i, m, &group, name.clone(), &theme, cx));
            if let Some((account, code)) = &self.verifying
                && *account == m.account
            {
                body = body.child(match code {
                    None => div()
                        .text_xs()
                        .text_color(theme.muted)
                        .child(t!("encryption-busy"))
                        .into_any_element(),
                    Some(code) => {
                        let (s, account, verified) =
                            (self.session.clone(), account.clone(), code.verified);
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .pl_3()
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(theme.muted)
                                    .child(t!("encryption-panel-code-help", name = name.clone())),
                            )
                            .child(
                                div()
                                    .id("encryption-group-digits")
                                    .test_support()
                                    .role(Role::Note)
                                    .aria_label(SharedString::from(code.groups.join(" ")))
                                    .w(px(260.))
                                    .flex()
                                    .flex_wrap()
                                    .gap_y_1()
                                    .font_family(theme.mono_font.clone())
                                    .text_color(theme.text)
                                    .children(
                                        code.groups
                                            .iter()
                                            .map(|g| div().w(px(65.)).child(g.clone())),
                                    ),
                            )
                            .child(
                                Button::new("encryption-group-mark")
                                    .small()
                                    .label(if verified {
                                        t!("encryption-panel-unverify").to_string()
                                    } else {
                                        t!("encryption-panel-verify", name = name.clone())
                                    })
                                    .when(!verified, |b| b.primary())
                                    .on_click(move |_, _, cx| {
                                        if let Some(engine) = engine(&s, cx) {
                                            engine.set_verified_account(&account, !verified);
                                        }
                                    }),
                            )
                            .child(qr_code(
                                "encryption-group-qr",
                                &code.qr,
                                t!("encryption-panel-qr"),
                                3.0,
                            ))
                            .into_any_element()
                    }
                });
            }
        }
        if !outsiders.is_empty() {
            body = body.child(
                div()
                    .id("encryption-group-outsiders")
                    .test_support()
                    .role(Role::Note)
                    .text_sm()
                    .text_color(theme.warning)
                    .child(t!(
                        "encryption-group-outsiders",
                        names = outsiders.join(", ")
                    )),
            );
        }
        if !group.pending.is_empty() {
            look_for_pending(&self.session, &self.conversation, &group, cx);
            body = body
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.text)
                        .child(t!("encryption-group-pending")),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted)
                        .child(t!("encryption-group-pending-help")),
                );
            for (i, account) in group.pending.iter().enumerate() {
                let name = with(&self.session, cx, |w| name_of(w, account))
                    .unwrap_or_else(|| account.clone());
                let (s, conv, a) = (
                    self.session.clone(),
                    self.conversation.clone(),
                    account.clone(),
                );
                body = body.child(
                    div()
                        .id(ElementId::Name(
                            format!("encryption-group-pending-{i}").into(),
                        ))
                        .test_support()
                        .role(Role::Group)
                        .aria_label(SharedString::from(format!(
                            "{name}, {}",
                            t!("encryption-group-pending")
                        )))
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(ui::icon::icon(IconName::Loader, theme.muted))
                        .child(
                            div()
                                .flex_1()
                                .text_sm()
                                .text_color(theme.muted)
                                .child(name.clone()),
                        )
                        .when(group.admin, |d| {
                            d.child(
                                Button::new(ElementId::Name(
                                    format!("encryption-group-pending-cancel-{i}").into(),
                                ))
                                .xsmall()
                                .label(t!("core-cancel"))
                                .accessibility_label(t!(
                                    "encryption-group-pending-cancel",
                                    name = name
                                ))
                                .disabled(busy)
                                .on_click(move |_, _, cx| {
                                    if let Some(engine) = engine(&s, cx) {
                                        engine.cancel_pending(&conv, &a);
                                    }
                                }),
                            )
                        }),
                );
            }
        }
        let mut buttons = div().flex().flex_wrap().gap_2();
        if group.admin {
            let (s, conv) = (self.session.clone(), self.conversation.clone());
            buttons = buttons.child(
                Button::new("encryption-group-add-people")
                    .small()
                    .primary()
                    .label(t!("encryption-group-add-title"))
                    .disabled(busy)
                    .on_click(move |_, window, cx| open_new(&s, Some(conv.clone()), window, cx)),
            );
        }
        let (s, conv) = (self.session.clone(), self.conversation.clone());
        buttons = buttons.child(
            Button::new("encryption-group-leave")
                .small()
                .danger()
                .label(t!("encryption-group-leave"))
                .disabled(busy)
                .on_click(move |_, window, cx| ask_leave(&s, &conv, window, cx)),
        );
        body = body.child(buttons);
        if !events.is_empty() {
            body = body
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.text)
                        .child(t!("encryption-group-history")),
                )
                .children(
                    events
                        .into_iter()
                        .map(|e| div().text_xs().text_color(theme.muted).child(e)),
                );
        }
        body.child(
            div()
                .text_xs()
                .text_color(theme.muted)
                .child(t!("encryption-panel-experimental")),
        )
        .into_any_element()
    }
}

fn ask_leave(session: &Entity<Session>, conversation: &str, window: &mut Window, cx: &mut App) {
    let creator = group_of(session, conversation, cx).is_some_and(|g| g.creator);
    let (session, conversation) = (session.clone(), conversation.to_string());
    ui::confirm::confirm(
        window,
        cx,
        ui::confirm::Confirm {
            title: t!("encryption-group-leave-title").into(),
            body: if creator {
                t!("encryption-group-leave-body-creator").into()
            } else {
                t!("encryption-group-leave-body").into()
            },
            ok: t!("encryption-group-leave").into(),
            danger: true,
        },
        move |_, cx| {
            if let Some(engine) = engine(&session, cx) {
                engine.leave_group(&conversation);
            }
        },
    );
}

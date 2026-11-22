//! Groups (package PT1).
//!
//! A group conversation's details open beside it, from its header or its
//! menu: the name and description, the people with the owner and you marked,
//! inviting more people (registered accounts, through the people picker),
//! and for the owner removing someone, handing over ownership and converting
//! the group to an ordinary channel. Anyone can leave, from here or the
//! conversation's menu. When a group goes away while you look at it (you
//! were taken out, it was archived), you are told and moved on.
//!
//! The server no longer creates groups ("Create an ordinary channel
//! instead"), and the web client offers no way to; groups made earlier are
//! managed here. Follows the web client's `PartyManager.vue`,
//! `PartyDetailsDialog.vue` and the group entries of `useMenus.ts`.

use std::collections::BTreeSet;
use std::future::Future;
use std::rc::Rc;

use gpui_kit::TestSupportExt;
use gpui_kit::component::Disableable;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::prelude::*;
use gpui_kit::*;
use ircord_client::Client;
use ircord_client::api::people::Purpose;
use ircord_client::api::{Api, ApiError};
use ircord_client::contract::{Party, PartyConversionResultStatus, PartyState};
use ircord_client::store::{Workspace, parties};

use crate::menu_order;
use crate::registry::{
    self, BannerCtx, ConversationCtx, HeaderCtx, MenuItem, Panel, PanelCtx, Slot,
};
use crate::route::Route;
use crate::session::{Session, SessionEvent};
use crate::sidebar::layout;
use crate::theme::{self, Theme};
use crate::ui;
use crate::ui::confirm::Confirm;
use crate::ui::icon::IconName;
use crate::ui::people_picker::{self, Picker};
use crate::ui::prompt::Prompt;

/// The right-hand panel's ID for a group's details.
pub const PANEL: &str = "party-details";

pub fn init(cx: &mut App) {
    registry::register(cx, |r| {
        r.panels.add(Panel::new(PANEL, panel));
        r.header_buttons.add(
            Slot::new("party-details", |ctx: &HeaderCtx, _, cx| {
                let Route::Conversation(id) = &ctx.route else {
                    return None;
                };
                party_of(&ctx.session, id, cx)?;
                let session = ctx.session.clone();
                let open = session.read(cx).panel.as_deref() == Some(PANEL);
                Some(
                    ui::icon::button(
                        "party-details-toggle",
                        IconName::Info,
                        t!("parties-details"),
                        theme::get(cx),
                        move |_, _, cx| show_details(&session, !open, cx),
                    )
                    .into_any_element(),
                )
            })
            .order(70),
        );
        r.conversation_menu.add(
            MenuItem::new(
                "party-details",
                t!("parties-details"),
                |ctx: &ConversationCtx, _, cx: &mut App| {
                    open_details(&ctx.session, &ctx.conversation, cx)
                },
            )
            .icon(IconName::Info)
            .order(menu_order::conversation::PARTY_DETAILS)
            .when(|ctx: &ConversationCtx, cx: &App| {
                party_of(&ctx.session, &ctx.conversation, cx).is_some()
            }),
        );
        r.conversation_menu.add(
            MenuItem::new(
                "party-leave",
                t!("parties-leave"),
                |ctx: &ConversationCtx, window: &mut Window, cx: &mut App| {
                    leave(&ctx.session, &ctx.conversation, window, cx)
                },
            )
            .icon(IconName::Close)
            .danger()
            .order(menu_order::conversation::LEAVE)
            .when(|ctx: &ConversationCtx, cx: &App| {
                party_of(&ctx.session, &ctx.conversation, cx).is_some()
            })
            .unavailable_when(|ctx: &ConversationCtx, cx: &App| {
                let party = party_of(&ctx.session, &ctx.conversation, cx)?;
                (party.state == PartyState::Archived).then(|| t!("parties-state-archived").into())
            }),
        );
        // Not drawn: keeps the watcher for groups going away alive.
        r.banners.add(
            Slot::new("party-watch", |ctx: &BannerCtx, _, cx| {
                watch(&ctx.session, cx);
                None
            })
            .order(-100),
        );
    });
}

/// The group in a conversation, if it is one.
fn party_of(session: &Entity<Session>, conversation: &str, cx: &App) -> Option<Party> {
    let client = session.read(cx).client()?;
    client.read().conversation(conversation)?.party.clone()
}

/// Open a group's details beside it.
pub fn open_details(session: &Entity<Session>, conversation: &str, cx: &mut App) {
    let showing = session.read(cx).conversation.as_deref() == Some(conversation)
        && matches!(session.read(cx).route(), Some(Route::Conversation(_)));
    let id = conversation.to_string();
    session.update(cx, |s, cx| {
        if !showing {
            s.select_conversation(&id, cx);
        }
        s.set_panel(Some(SharedString::from(PANEL)), cx);
    });
}

fn show_details(session: &Entity<Session>, show: bool, cx: &mut App) {
    session.update(cx, |s, cx| {
        s.set_panel(show.then(|| SharedString::from(PANEL)), cx)
    });
}

/// Run `request` off the UI thread; `done` gets the answer, a failure shows
/// as a toast.
pub(crate) fn run<T, Fut>(
    session: &Entity<Session>,
    window: &mut Window,
    cx: &mut App,
    request: impl FnOnce(Api) -> Fut + Send + 'static,
    done: impl FnOnce(T, &Rc<Client>, &mut Window, &mut App) + 'static,
) where
    T: Send + 'static,
    Fut: Future<Output = Result<T, ApiError>> + Send + 'static,
{
    let Some(client) = session.read(cx).client().cloned() else {
        return;
    };
    let task = client.request(request);
    window
        .spawn(cx, async move |acx| {
            let result = task
                .await
                .map_err(|e| e.to_string())
                .and_then(|r| r.map_err(|e| e.to_string()));
            let _ = acx.update(|window, cx| match result {
                Ok(answer) => done(answer, &client, window, cx),
                Err(e) => ui::toast::error(window, cx, e),
            });
        })
        .detach();
}

// ---- Leaving ----

/// Groups being left on purpose, so leaving does not also say the group is
/// no longer available.
#[derive(Default)]
struct Leaving(BTreeSet<String>);

impl Global for Leaving {}

/// Ask, then leave the group in `conversation`.
pub fn leave(session: &Entity<Session>, conversation: &str, window: &mut Window, cx: &mut App) {
    let Some(party) = party_of(session, conversation, cx) else {
        return;
    };
    let (session, conversation) = (session.clone(), conversation.to_string());
    ui::confirm::confirm(
        window,
        cx,
        Confirm {
            title: t!("parties-leave-confirm").into(),
            body: t!("parties-leave-confirm-body").into(),
            ok: t!("parties-leave-ok").into(),
            danger: true,
        },
        move |window, cx| {
            let id = party.id.clone();
            let gone = conversation.clone();
            let s = session.clone();
            run(
                &session,
                window,
                cx,
                move |api| async move { api.leave_party(&id).await },
                move |(), client, _, cx| {
                    cx.default_global::<Leaving>().0.insert(gone.clone());
                    client.change(|w| parties::left(w, &gone));
                    move_on(&s, &gone, cx);
                },
            );
        },
    );
}

/// After the group in `conversation` went away: close its details, and
/// show another conversation if it was showing.
fn move_on(session: &Entity<Session>, conversation: &str, cx: &mut App) {
    let Some(client) = session.read(cx).client().cloned() else {
        return;
    };
    let (showing, network) = {
        let s = session.read(cx);
        (
            s.conversation.as_deref() == Some(conversation),
            s.network.clone(),
        )
    };
    if !showing {
        return;
    }
    let next = network.and_then(|n| {
        let w = client.read();
        w.sidebar(&n)
            .into_iter()
            .find(|c| c.id != conversation)
            .map(|c| c.id.clone())
    });
    session.update(cx, |s, cx| {
        if s.panel.as_deref() == Some(PANEL) {
            s.set_panel(None, cx);
        }
        if let Some(next) = next {
            s.select_conversation(&next, cx);
        }
    });
}

// ---- Watching for groups going away ----

struct Watcher {
    session: Entity<Session>,
    _subscription: Subscription,
}

struct Watching(Entity<Watcher>);

impl Global for Watching {}

fn watch(session: &Entity<Session>, cx: &mut App) {
    if let Some(Watching(w)) = cx.try_global::<Watching>()
        && w.read(cx).session == *session
    {
        return;
    }
    let watcher = cx.new(|cx| Watcher {
        session: session.clone(),
        _subscription: cx.subscribe(session, |_, session, event: &SessionEvent, cx| {
            if let SessionEvent::Changed(changes) = event
                && changes.touches(parties::TOPIC)
            {
                gone(&session, cx);
            }
        }),
    });
    cx.set_global(Watching(watcher));
}

/// The showing conversation's group was removed: say so and move on.
fn gone(session: &Entity<Session>, cx: &mut App) {
    let Some(open) = session.read(cx).conversation.clone() else {
        return;
    };
    let removed = session.read(cx).client().is_some_and(|client| {
        let w = client.read();
        w.conversation(&open).is_none() && w.parties.gone(&open)
    });
    if !removed {
        return;
    }
    let on_purpose = cx.default_global::<Leaving>().0.remove(&open);
    move_on(session, &open, cx);
    if !on_purpose {
        layout::with_window(cx, |window, cx| {
            ui::toast::info(window, cx, t!("parties-unavailable"))
        });
    }
}

// ---- The details panel ----

struct DetailsPanel(Entity<DetailsView>);

impl Global for DetailsPanel {}

fn panel(ctx: &PanelCtx, _window: &mut Window, cx: &mut App) -> AnyElement {
    let theme = theme::get(cx).clone();
    let Some(conversation) = ctx
        .conversation
        .clone()
        .filter(|id| party_of(&ctx.session, id, cx).is_some())
    else {
        return ui::empty::empty(
            "party-details-none",
            t!("parties-unavailable"),
            None,
            &theme,
        )
        .into_any_element();
    };
    let view = match cx.try_global::<DetailsPanel>() {
        Some(DetailsPanel(view))
            if view.read(cx).session == ctx.session
                && view.read(cx).conversation == conversation =>
        {
            view.clone()
        }
        _ => {
            let session = ctx.session.clone();
            let view = cx.new(|cx| DetailsView::new(session, conversation, cx));
            cx.set_global(DetailsPanel(view.clone()));
            view
        }
    };
    view.into_any_element()
}

pub struct DetailsView {
    session: Entity<Session>,
    conversation: String,
    /// What a conversion that waits on the server said.
    pending: Option<String>,
    warnings: Vec<String>,
    _subscription: Subscription,
}

impl DetailsView {
    fn new(session: Entity<Session>, conversation: String, cx: &mut Context<Self>) -> Self {
        let watched = conversation.clone();
        let subscription = cx.subscribe(&session, move |_, _, event: &SessionEvent, cx| {
            if let SessionEvent::Changed(changes) = event
                && (changes.touches_id(parties::TOPIC, &watched)
                    || changes.members
                    || changes.sidebar)
            {
                cx.notify();
            }
        });
        Self {
            session,
            conversation,
            pending: None,
            warnings: Vec::new(),
            _subscription: subscription,
        }
    }

    fn rename(
        session: &Entity<Session>,
        party: &Party,
        description: bool,
        window: &mut Window,
        cx: &mut App,
    ) {
        let (title, label, initial) = if description {
            (
                t!("parties-edit-description-title"),
                t!("parties-description"),
                party.description.clone(),
            )
        } else {
            (
                t!("parties-rename-title"),
                t!("parties-name"),
                party.name.clone(),
            )
        };
        let (session, id) = (session.clone(), party.id.clone());
        ui::prompt::prompt(
            window,
            cx,
            Prompt {
                title: title.into(),
                label: label.into(),
                initial,
                ok: t!("parties-save").into(),
            },
            move |text, window, cx| {
                let text = text.trim().to_string();
                let id = id.clone();
                let (name, description) = if description {
                    (None, Some(text))
                } else {
                    (Some(text), None)
                };
                run(
                    &session,
                    window,
                    cx,
                    move |api| async move { api.update_party(&id, name, description).await },
                    |party, client, window, cx| {
                        client.change(|w| parties::put_party(w, &party));
                        ui::toast::success(window, cx, t!("parties-saved"));
                    },
                );
            },
        );
    }

    fn invite(session: &Entity<Session>, party: &Party, window: &mut Window, cx: &mut App) {
        let Some(client) = session.read(cx).client().cloned() else {
            return;
        };
        let limit = client
            .read()
            .capabilities
            .parties
            .as_ref()
            .map(|p| p.max_members)
            .filter(|max| *max > 0);
        if let Some(max) = limit
            && party.members.len() as i64 >= max
        {
            ui::toast::error(window, cx, t!("parties-member-limit", count = max));
            return;
        }
        let title = t!(
            "parties-invite-title",
            name = parties::title(&client.read(), &conversation_of(&client.read(), party))
        );
        let (session, id) = (session.clone(), party.id.clone());
        people_picker::open(
            window,
            cx,
            Picker {
                title: title.into(),
                session: session.clone(),
                network: party.network_id.clone(),
                purpose: Purpose::Party,
                conversation: None,
                except: Vec::new(),
            },
            move |picked, window, cx| {
                let name = picked.name.clone();
                let Some(account) = picked.account.clone().filter(|a| !a.is_empty()) else {
                    ui::toast::error(window, cx, t!("parties-not-eligible", name = name));
                    return;
                };
                let id = id.clone();
                run(
                    &session,
                    window,
                    cx,
                    move |api| async move { api.invite_to_party(&id, &account).await },
                    move |_, _, window, cx| {
                        ui::toast::success(window, cx, t!("parties-invited", name = name))
                    },
                );
            },
        );
    }

    fn change_member(
        session: &Entity<Session>,
        party: &Party,
        member: String,
        name: String,
        transfer: bool,
        window: &mut Window,
        cx: &mut App,
    ) {
        let confirm = if transfer {
            Confirm {
                title: t!("parties-transfer-confirm", name = name).into(),
                body: t!("parties-transfer-confirm-body").into(),
                ok: t!("parties-transfer-ok").into(),
                danger: false,
            }
        } else {
            Confirm {
                title: t!("parties-remove-confirm", name = name).into(),
                body: t!("parties-remove-confirm-body").into(),
                ok: t!("parties-remove-ok").into(),
                danger: true,
            }
        };
        let (session, id) = (session.clone(), party.id.clone());
        ui::confirm::confirm(window, cx, confirm, move |window, cx| {
            let (id, member) = (id.clone(), member.clone());
            run(
                &session,
                window,
                cx,
                move |api| async move {
                    if transfer {
                        api.transfer_party(&id, &member).await
                    } else {
                        api.remove_party_member(&id, &member).await
                    }
                },
                |party, client, _, _| {
                    client.change(|w| parties::put_party(w, &party));
                },
            );
        });
    }

    fn convert(&self, party: &Party, window: &mut Window, cx: &mut Context<Self>) {
        let (session, id) = (self.session.clone(), party.id.clone());
        let this = cx.entity().downgrade();
        ui::confirm::confirm(
            window,
            cx,
            Confirm {
                title: t!("parties-convert-confirm").into(),
                body: t!("parties-convert-help").into(),
                ok: t!("parties-convert-ok").into(),
                danger: false,
            },
            move |window, cx| {
                let id = id.clone();
                let (this, session2) = (this.clone(), session.clone());
                run(
                    &session,
                    window,
                    cx,
                    move |api| async move { api.convert_party(&id).await },
                    move |result, _, window, cx| {
                        if result.status == PartyConversionResultStatus::Pending {
                            let status = [
                                t!("parties-convert-pending").to_string(),
                                result.prerequisite.clone().unwrap_or_default(),
                            ]
                            .join(" ")
                            .trim()
                            .to_string();
                            let _ = this.update(cx, |this, cx| {
                                this.pending = Some(status);
                                this.warnings = result.warnings.clone();
                                cx.notify();
                            });
                            return;
                        }
                        let mut text = t!("parties-converted").to_string();
                        if !result.warnings.is_empty() {
                            text = format!("{text} {}", result.warnings.join(" "));
                        }
                        ui::toast::success(window, cx, text);
                        show_details(&session2, false, cx);
                    },
                );
            },
        );
    }
}

/// The conversation a group belongs to, for its title.
fn conversation_of(w: &Workspace, party: &Party) -> ircord_client::contract::Conversation {
    w.conversation(&party.conversation_id)
        .cloned()
        .unwrap_or_else(|| ircord_client::contract::Conversation {
            party: Some(party.clone()),
            ..Default::default()
        })
}

fn state_text(state: PartyState) -> Option<&'static str> {
    match state {
        PartyState::Active => None,
        PartyState::Provisioning => Some(t!("parties-state-provisioning")),
        PartyState::Recovering => Some(t!("parties-state-recovering")),
        PartyState::Archived => Some(t!("parties-state-archived")),
        _ => Some(t!("parties-state-converted")),
    }
}

fn heading(text: impl Into<SharedString>, theme: &Theme) -> Div {
    let text: SharedString = text.into();
    div()
        .text_xs()
        .font_weight(FontWeight::BOLD)
        .text_color(theme.muted)
        .child(text.to_uppercase())
}

fn badge(text: &'static str, theme: &Theme) -> Div {
    div()
        .px_1p5()
        .rounded(theme.radius)
        .text_xs()
        .text_color(theme.muted)
        .bg(theme.selected)
        .child(text)
}

impl Render for DetailsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme::get(cx).clone();
        let Some(client) = self.session.read(cx).client().cloned() else {
            return div().into_any_element();
        };
        let w = client.read();
        let Some(conversation) = w.conversation(&self.conversation).cloned() else {
            return div().into_any_element();
        };
        let Some(party) = conversation.party.clone() else {
            return div().into_any_element();
        };
        let title = parties::title(&w, &conversation);
        let owned = parties::owned(&w, &party);
        let members: Vec<(String, String, String, bool, bool)> = party
            .members
            .iter()
            .map(|m| {
                let detail = if m.services_account.is_empty() {
                    m.username.clone()
                } else {
                    m.services_account.clone()
                };
                (
                    parties::member_id(m).to_string(),
                    parties::member_name(m).to_string(),
                    detail,
                    parties::is_owner(&party, m),
                    parties::is_me(&w, &party, m),
                )
            })
            .collect();
        drop(w);
        let active = party.state == PartyState::Active;
        let view = cx.entity();
        let session = self.session.clone();

        let header = div()
            .flex()
            .items_center()
            .gap_2()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .font_weight(FontWeight::BOLD)
                    .text_color(theme.text)
                    .child(title.clone()),
            )
            .child(ui::icon::button(
                "party-details-close",
                IconName::Close,
                t!("parties-details-close"),
                &theme,
                {
                    let session = session.clone();
                    move |_, _, cx| show_details(&session, false, cx)
                },
            ));

        let mut about = div()
            .flex()
            .flex_col()
            .gap_1()
            .child(heading(t!("parties-name"), &theme))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .id("party-name")
                            .test_support()
                            .role(Role::Group)
                            .aria_label(SharedString::from(title.clone()))
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_sm()
                            .child(if party.name.trim().is_empty() {
                                t!("parties-unnamed").to_string()
                            } else {
                                party.name.clone()
                            }),
                    )
                    .child(
                        Button::new("party-rename")
                            .ghost()
                            .label(t!("parties-rename"))
                            .disabled(!active)
                            .on_click({
                                let (view, party) = (view.clone(), party.clone());
                                move |_, window, cx| {
                                    DetailsView::rename(
                                        &view.read(cx).session.clone(),
                                        &party,
                                        false,
                                        window,
                                        cx,
                                    )
                                }
                            }),
                    ),
            );
        about = about
            .child(heading(t!("parties-description"), &theme))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .id("party-description")
                            .test_support()
                            .role(Role::Group)
                            .flex_1()
                            .min_w_0()
                            .text_sm()
                            .text_color(if party.description.is_empty() {
                                theme.faint
                            } else {
                                theme.text
                            })
                            .child(if party.description.is_empty() {
                                t!("parties-no-description").to_string()
                            } else {
                                party.description.clone()
                            }),
                    )
                    .child(
                        Button::new("party-edit-description")
                            .ghost()
                            .label(t!("parties-edit-description"))
                            .disabled(!active)
                            .on_click({
                                let (view, party) = (view.clone(), party.clone());
                                move |_, window, cx| {
                                    DetailsView::rename(
                                        &view.read(cx).session.clone(),
                                        &party,
                                        true,
                                        window,
                                        cx,
                                    )
                                }
                            }),
                    ),
            );

        let mut people = div().flex().flex_col().gap_1().child({
            let text = t!("parties-people", count = members.len());
            heading(text.clone(), &theme)
                .id("party-people")
                .test_support()
                .role(Role::Heading)
                .aria_label(SharedString::from(text))
        });
        for (id, name, detail, owner, me) in members {
            let mut row = div()
                .id(ElementId::Name(format!("party-member-{id}").into()))
                .test_support()
                .role(Role::ListItem)
                .aria_label(SharedString::from(name.clone()))
                .flex()
                .items_center()
                .gap_2()
                .py_1()
                .child(ui::avatar::avatar(&name, None, px(28.), &theme))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_1()
                                .text_sm()
                                .child(div().truncate().child(name.clone()))
                                .when(owner, |d| d.child(badge(t!("parties-owner"), &theme)))
                                .when(me, |d| d.child(badge(t!("parties-you"), &theme))),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.faint)
                                .truncate()
                                .child(detail),
                        ),
                );
            if owned && !me && active {
                let transfer = {
                    let (view, party, id, name) =
                        (view.clone(), party.clone(), id.clone(), name.clone());
                    move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
                        DetailsView::change_member(
                            &view.read(cx).session.clone(),
                            &party,
                            id.clone(),
                            name.clone(),
                            true,
                            window,
                            cx,
                        )
                    }
                };
                let remove = {
                    let (view, party, id, name) =
                        (view.clone(), party.clone(), id.clone(), name.clone());
                    move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
                        DetailsView::change_member(
                            &view.read(cx).session.clone(),
                            &party,
                            id.clone(),
                            name.clone(),
                            false,
                            window,
                            cx,
                        )
                    }
                };
                row = row
                    .child(ui::icon::button(
                        ElementId::Name(format!("party-transfer-{id}").into()),
                        IconName::Star,
                        t!("parties-transfer", name = name),
                        &theme,
                        transfer,
                    ))
                    .child(ui::icon::button(
                        ElementId::Name(format!("party-remove-{id}").into()),
                        IconName::Delete,
                        t!("parties-remove", name = name),
                        &theme,
                        remove,
                    ));
            }
            people = people.child(row);
        }

        let mut actions = div().flex().flex_col().gap_2().child(
            Button::new("party-invite")
                .ghost()
                .label(t!("parties-invite-people"))
                .disabled(!active)
                .on_click({
                    let (view, party) = (view.clone(), party.clone());
                    move |_, window, cx| {
                        DetailsView::invite(&view.read(cx).session.clone(), &party, window, cx)
                    }
                }),
        );
        if owned {
            actions = actions
                .child(
                    Button::new("party-convert")
                        .ghost()
                        .label(t!("parties-convert"))
                        .on_click({
                            let (view, party) = (view.clone(), party.clone());
                            move |_, window, cx| {
                                view.update(cx, |this, cx| this.convert(&party, window, cx))
                            }
                        }),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.faint)
                        .child(t!("parties-convert-help")),
                );
            if let Some(pending) = &self.pending {
                actions = actions.child(
                    div()
                        .id("party-convert-status")
                        .test_support()
                        .role(Role::Status)
                        .aria_label(SharedString::from(pending.clone()))
                        .text_sm()
                        .text_color(theme.warning)
                        .child(pending.clone())
                        .children(
                            self.warnings
                                .iter()
                                .map(|w| div().text_xs().child(w.clone())),
                        ),
                );
            }
        }
        actions = actions.child(
            Button::new("party-leave")
                .danger()
                .label(t!("parties-leave"))
                .disabled(party.state == PartyState::Archived)
                .on_click({
                    let (session, conversation) = (session.clone(), self.conversation.clone());
                    move |_, window, cx| leave(&session, &conversation, window, cx)
                }),
        );

        div()
            .id("party-details")
            .test_support()
            .role(Role::Region)
            .aria_label(SharedString::from(t!("parties-details")))
            .size_full()
            .flex()
            .flex_col()
            .gap_4()
            .p_4()
            .overflow_y_scroll()
            .bg(theme.sidebar)
            .text_color(theme.text)
            .child(header)
            .when_some(state_text(party.state), |d, text| {
                d.child(
                    div()
                        .id("party-state")
                        .test_support()
                        .role(Role::Status)
                        .aria_label(text)
                        .text_sm()
                        .text_color(theme.warning)
                        .child(text),
                )
            })
            .child(about)
            .child(people)
            .child(actions)
            .into_any_element()
    }
}

//! Invitations to groups and channels (package PT1).
//!
//! A banner across the top of the main pane shows the oldest invitation,
//! with how many more wait: Accept or Decline for a group, Join or Reject
//! for a channel (rejecting also blocks further invitations to it).
//! Accepting opens the conversation. Follows the web client's
//! `InvitationBanner.vue`.

use gpui_kit::TestSupportExt;
use gpui_kit::component::Disableable;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::prelude::*;
use gpui_kit::*;
use ircord_client::contract::Invitation;
use ircord_client::store::{Connection, parties};

use crate::registry::{self, BannerCtx, Slot};
use crate::session::{Session, SessionEvent};
use crate::theme;
use crate::ui;
use crate::ui::icon::IconName;

pub fn init(cx: &mut App) {
    registry::register(cx, |r| {
        r.banners.add(
            Slot::new("invitations", |ctx: &BannerCtx, _, cx| {
                let waiting = ctx
                    .session
                    .read(cx)
                    .client()
                    .is_some_and(|c| !c.read().invitations.is_empty());
                waiting.then(|| view(&ctx.session, cx).into_any_element())
            })
            .order(10),
        );
    });
}

struct Banner(Entity<BannerView>);

impl Global for Banner {}

fn view(session: &Entity<Session>, cx: &mut App) -> Entity<BannerView> {
    if let Some(Banner(view)) = cx.try_global::<Banner>()
        && view.read(cx).session == *session
    {
        return view.clone();
    }
    let view = cx.new(|cx| BannerView::new(session.clone(), cx));
    cx.set_global(Banner(view.clone()));
    view
}

pub struct BannerView {
    session: Entity<Session>,
    /// The invitation being answered.
    busy: Option<String>,
    /// Why the last answer failed, for that invitation.
    error: Option<(String, String)>,
    _subscription: Subscription,
}

impl BannerView {
    fn new(session: Entity<Session>, cx: &mut Context<Self>) -> Self {
        let subscription = cx.subscribe(&session, |_, _, event: &SessionEvent, cx| match event {
            SessionEvent::Changed(changes)
                if changes.sidebar || changes.touches(parties::TOPIC) =>
            {
                cx.notify()
            }
            SessionEvent::Connection => cx.notify(),
            _ => {}
        });
        Self {
            session,
            busy: None,
            error: None,
            _subscription: subscription,
        }
    }

    fn answer(&mut self, invitation: &Invitation, accept: bool, cx: &mut Context<Self>) {
        if self.busy.is_some() {
            return;
        }
        let Some(client) = self.session.read(cx).client().cloned() else {
            return;
        };
        let id = invitation.id.clone();
        self.busy = Some(id.clone());
        self.error = None;
        cx.notify();
        let session = self.session.clone();
        let request = id.clone();
        let accepting = accept.then(|| {
            client.request(move |api| async move { api.accept_invitation(&request).await })
        });
        let request = id.clone();
        let declining = (!accept).then(|| {
            client.request(move |api| async move { api.decline_invitation(&request).await })
        });
        cx.spawn(async move |this, cx| {
            let result = match (accepting, declining) {
                (Some(task), _) => task
                    .await
                    .map_err(|e| e.to_string())
                    .and_then(|r| r.map(Some).map_err(|e| e.to_string())),
                (_, Some(task)) => task
                    .await
                    .map_err(|e| e.to_string())
                    .and_then(|r| r.map(|()| None).map_err(|e| e.to_string())),
                _ => Ok(None),
            };
            let _ = this.update(cx, |this, cx| {
                this.busy = None;
                match result {
                    Ok(Some(conversation)) => {
                        let open = conversation.id.clone();
                        client.change(|w| parties::accepted(w, &id, conversation));
                        session.update(cx, |s, cx| s.select_conversation(&open, cx));
                    }
                    Ok(None) => client.change(|w| parties::declined(w, &id)),
                    Err(e) => this.error = Some((id.clone(), e)),
                }
                cx.notify();
            });
        })
        .detach();
    }
}

impl Render for BannerView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme::get(cx).clone();
        let Some(client) = self.session.read(cx).client().cloned() else {
            return div().into_any_element();
        };
        let w = client.read();
        let Some(invitation) = w.invitations.first().cloned() else {
            return div().into_any_element();
        };
        let more = w.invitations.len() - 1;
        let network = w
            .network(&invitation.network_id)
            .map_or_else(|| "IRC".to_string(), |n| n.name.clone());
        let live = w.connection == Some(Connection::Live);
        drop(w);
        let group = invitation.party.is_some();
        let text = match &invitation.party {
            Some(party) => {
                let name = if party.name.trim().is_empty() {
                    t!("parties-unnamed").to_string()
                } else {
                    party.name.clone()
                };
                t!(
                    "parties-invitation-group",
                    inviter = invitation.inviter_nick,
                    name = name,
                    network = network
                )
            }
            None => t!(
                "parties-invitation-channel",
                inviter = invitation.inviter_nick,
                channel = invitation.channel,
                network = network
            ),
        };
        let error = self
            .error
            .as_ref()
            .filter(|(id, _)| *id == invitation.id)
            .map(|(_, e)| e.clone());
        let disabled = !live || self.busy.is_some();
        let view = cx.entity();
        div()
            .id("invitation-banner")
            .test_support()
            .role(Role::Status)
            .aria_label(SharedString::from(t!("parties-invitation-label")))
            .aria_description(SharedString::from(text.clone()))
            .flex()
            .items_center()
            .gap_2()
            .px_4()
            .py_1p5()
            .flex_none()
            .text_sm()
            .text_color(theme.muted)
            .bg(theme.sidebar)
            .border_b_1()
            .border_color(theme.border)
            .child(ui::icon::icon(
                if group {
                    IconName::User
                } else {
                    IconName::Inbox
                },
                theme.muted,
            ))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .flex()
                            .gap_1()
                            .child(
                                div()
                                    .id("invitation-text")
                                    .test_support()
                                    .role(Role::Group)
                                    .aria_label(SharedString::from(text.clone()))
                                    .text_color(theme.text)
                                    .child(text),
                            )
                            .when(more > 0, |d| {
                                let text = t!("parties-invitation-more", count = more);
                                d.child(
                                    div()
                                        .id("invitation-more")
                                        .test_support()
                                        .role(Role::Group)
                                        .aria_label(SharedString::from(text.clone()))
                                        .child(text),
                                )
                            }),
                    )
                    .when_some(error, |d, error| {
                        d.child(
                            div()
                                .id("invitation-error")
                                .test_support()
                                .role(Role::Alert)
                                .aria_label(SharedString::from(error.clone()))
                                .text_color(theme.danger)
                                .child(error),
                        )
                    }),
            )
            .child(
                Button::new("invitation-accept")
                    .ghost()
                    .label(if group {
                        t!("parties-accept")
                    } else {
                        t!("parties-join")
                    })
                    .disabled(disabled)
                    .on_click({
                        let (view, invitation) = (view.clone(), invitation.clone());
                        move |_, _, cx| {
                            view.update(cx, |this, cx| this.answer(&invitation, true, cx))
                        }
                    }),
            )
            .child(
                Button::new("invitation-decline")
                    .ghost()
                    .label(if group {
                        t!("parties-decline")
                    } else {
                        t!("parties-reject")
                    })
                    .tooltip(if group {
                        t!("parties-decline-help")
                    } else {
                        t!("parties-reject-help")
                    })
                    .disabled(disabled)
                    .on_click({
                        let view = view.clone();
                        move |_, _, cx| {
                            view.update(cx, |this, cx| this.answer(&invitation, false, cx))
                        }
                    }),
            )
            .into_any_element()
    }
}

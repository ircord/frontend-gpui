//! Administration → Users, after `AdminUsers.vue`: every IRCord account,
//! owners first, filtered by username and paged; make an account an owner
//! (found by username or IRC nickname) or remove one, with a reason for the
//! log. Owners set in the server configuration cannot be removed here.

use std::time::Duration;

use gpui_kit::TestSupportExt;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::*;
use gpui_kit::*;
use ircord_client::api::admin::{self as a, USERS_PAGE};
use ircord_client::contract as c;

use super::picker::Picker;
use super::{Kind, button, client, day, error_text, group, hint, notice};
use crate::session::Session;
use crate::theme;
use crate::ui;
use crate::ui::prompt::Prompt;

const DEBOUNCE: Duration = Duration::from_millis(250);

pub struct UsersPanel {
    session: Entity<Session>,
    picker: Entity<Picker>,
    filter: Entity<InputState>,
    query: String,
    users: Vec<c::AdminUser>,
    more: bool,
    loading: bool,
    error: Option<String>,
    notice: Option<String>,
    /// The account being changed.
    busy: Option<String>,
    /// The signed-in account's ID.
    me: Option<String>,
    generation: u64,
    _subscription: Subscription,
}

impl UsersPanel {
    pub fn new(session: Entity<Session>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let this = cx.weak_entity();
        let picker = cx.new(|cx| {
            Picker::new(
                session.clone(),
                "admin-add-owner",
                t!("admin-make-owner"),
                t!("admin-already-owner"),
                move |user, window, cx| {
                    this.update(cx, |this, cx| this.grant(user, window, cx))
                        .ok();
                },
                window,
                cx,
            )
        });
        let filter =
            cx.new(|cx| InputState::new(window, cx).placeholder(t!("admin-filter-by-username")));
        let subscription =
            cx.subscribe(&filter, |this: &mut Self, input, event: &InputEvent, cx| {
                if let InputEvent::Change = event {
                    this.query = input.read(cx).value().trim().to_string();
                    this.generation += 1;
                    let generation = this.generation;
                    let settle = super::pause(&this.session, cx, DEBOUNCE);
                    cx.spawn(async move |this, cx| {
                        let _ = settle.await;
                        this.update(cx, |this, cx| {
                            if this.generation == generation {
                                this.load(false, cx);
                            }
                        })
                        .ok();
                    })
                    .detach();
                }
            });
        let mut panel = Self {
            session,
            picker,
            filter,
            query: String::new(),
            users: Vec::new(),
            more: false,
            loading: false,
            error: None,
            notice: None,
            busy: None,
            me: None,
            generation: 0,
            _subscription: subscription,
        };
        panel.who_am_i(cx);
        panel.load(false, cx);
        panel
    }

    /// Read the first page for the filter, or (with `append`) the next.
    pub fn load(&mut self, append: bool, cx: &mut Context<Self>) {
        let Some(client) = client(&self.session, cx) else {
            return;
        };
        self.generation += 1;
        let generation = self.generation;
        self.loading = true;
        self.error = None;
        let (query, offset) = (
            self.query.clone(),
            if append { self.users.len() } else { 0 },
        );
        let task = client.request(move |api| async move {
            api.administration_users(&query, offset, USERS_PAGE).await
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                if generation != this.generation {
                    return;
                }
                this.loading = false;
                match result {
                    Ok(Ok(page)) => {
                        if append {
                            this.users.extend(page.items);
                        } else {
                            this.users = page.items;
                        }
                        this.more = page.more;
                    }
                    Ok(Err(e)) => {
                        super::refused(&client, &e, cx);
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

    /// Learn the signed-in account's ID, to mark it "(you)".
    fn who_am_i(&mut self, cx: &mut Context<Self>) {
        let Some(client) = client(&self.session, cx) else {
            return;
        };
        let task = client.request(|api| async move { api.account().await });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(account))) = task.await {
                this.update(cx, |this, cx| {
                    this.me = Some(account.id);
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    fn is_me(&self, user: &c::AdminUser, _cx: &App) -> bool {
        self.me.as_deref() == Some(user.id.as_str())
    }

    fn grant(&mut self, user: c::AdminUser, window: &mut Window, cx: &mut Context<Self>) {
        if user.owner {
            self.notice = Some(t!("admin-already-owner").to_string());
            cx.notify();
            return;
        }
        let this = cx.weak_entity();
        let username = user.username.clone();
        ui::prompt::prompt(
            window,
            cx,
            Prompt {
                title: t!("admin-make-owner-confirm", username = username.as_str()).into(),
                label: t!("admin-reason-optional").into(),
                initial: String::new(),
                ok: t!("admin-make-owner").into(),
            },
            move |reason, _, cx| {
                let user = user.clone();
                this.update(cx, |this, cx| {
                    let id = user.id.clone();
                    let done = t!("admin-user-made-owner", username = user.username.as_str());
                    this.change(
                        &user,
                        move |api| async move { api.grant_owner(&id, Some(reason)).await },
                        done,
                        cx,
                    );
                })
                .ok();
            },
        );
    }

    fn revoke(&mut self, user: c::AdminUser, window: &mut Window, cx: &mut Context<Self>) {
        let me = self.is_me(&user, cx);
        let username = user.username.clone();
        let title = if me {
            t!("admin-stop-being-owner").to_string()
        } else {
            t!("admin-remove-owner-confirm", username = username.as_str())
        };
        let this = cx.weak_entity();
        ui::prompt::prompt(
            window,
            cx,
            Prompt {
                title: title.into(),
                label: t!("admin-reason-optional").into(),
                initial: String::new(),
                ok: t!("admin-remove-owner").into(),
            },
            move |reason, _, cx| {
                let user = user.clone();
                this.update(cx, |this, cx| {
                    let id = user.id.clone();
                    let done = t!(
                        "admin-user-no-longer-owner",
                        username = user.username.as_str()
                    );
                    this.change(
                        &user,
                        move |api| async move { api.revoke_owner(&id, Some(reason)).await },
                        done,
                        cx,
                    );
                })
                .ok();
            },
        );
    }

    fn change<Fut>(
        &mut self,
        user: &c::AdminUser,
        run: impl FnOnce(ircord_client::api::Api) -> Fut + Send + 'static,
        done: String,
        cx: &mut Context<Self>,
    ) where
        Fut: std::future::Future<Output = Result<c::AdminUser, ircord_client::api::ApiError>>
            + Send
            + 'static,
    {
        let Some(client) = client(&self.session, cx) else {
            return;
        };
        let me = self.is_me(user, cx);
        self.busy = Some(user.id.clone());
        self.error = None;
        self.notice = None;
        self.picker.update(cx, |p, cx| {
            p.busy = true;
            cx.notify();
        });
        let task = client.request(run);
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                this.busy = None;
                this.picker.update(cx, |p, cx| {
                    p.busy = false;
                    cx.notify();
                });
                match result {
                    Ok(Ok(changed)) => {
                        a::replace_user(&mut this.users, changed);
                        this.notice = Some(done);
                        if me {
                            // Your own ownership changed: ask again.
                            super::recheck(&client, cx);
                        }
                    }
                    Ok(Err(e)) => {
                        super::refused(&client, &e, cx);
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

    fn row(&self, i: usize, user: &c::AdminUser, cx: &mut Context<Self>) -> AnyElement {
        let theme = theme::get(cx).clone();
        let me = self.is_me(user, cx);
        let source = match user.owner_source {
            Some(c::AdminUserOwnerSource::Environment) => t!("admin-owner-source-environment"),
            Some(c::AdminUserOwnerSource::Operator) => t!("admin-owner-source-operator"),
            _ => t!("admin-owner-source-owner"),
        };
        let plural = |n: i64, one: &'static str, many: String| {
            if n == 1 { one.to_string() } else { many }
        };
        let facts = [
            t!("admin-joined", date = day(&user.created_at)),
            t!("admin-last-seen", date = day(&user.last_seen_at)),
            plural(
                user.sessions,
                t!("admin-user-sessions-one"),
                t!("admin-user-sessions", count = a::count(user.sessions)),
            ),
            plural(
                user.networks,
                t!("admin-user-networks-one"),
                t!("admin-user-networks", count = a::count(user.networks)),
            ),
        ]
        .join(" · ");
        let mut name = user.username.clone();
        if user.owner {
            name = format!("{name} · {}", t!("admin-owner-badge"));
        }
        if me {
            name = format!("{name} {}", t!("admin-you"));
        }
        let enabled = self.busy.is_none();
        let action: AnyElement = if !user.owner {
            let user = user.clone();
            button(
                ElementId::Name(format!("admin-user-{i}-grant").into()),
                t!("admin-make-owner"),
                Some(t!(
                    "admin-make-user-owner",
                    username = user.username.as_str()
                )),
                Kind::Secondary,
                enabled,
                &theme,
                cx.listener(move |this, _, window, cx| this.grant(user.clone(), window, cx)),
            )
            .into_any_element()
        } else if user.owner_source != Some(c::AdminUserOwnerSource::Environment) {
            let user = user.clone();
            button(
                ElementId::Name(format!("admin-user-{i}-revoke").into()),
                t!("admin-remove-owner"),
                Some(t!(
                    "admin-remove-user-owner",
                    username = user.username.as_str()
                )),
                Kind::Secondary,
                enabled,
                &theme,
                cx.listener(move |this, _, window, cx| this.revoke(user.clone(), window, cx)),
            )
            .into_any_element()
        } else {
            div()
                .max_w(px(140.))
                .text_xs()
                .text_color(theme.faint)
                .child(source)
                .into_any_element()
        };
        div()
            .id(ElementId::Name(format!("admin-user-{i}").into()))
            .test_support()
            .role(Role::ListItem)
            .aria_label(format!("{name}. {facts}"))
            .flex()
            .items_center()
            .gap_3()
            .px_3p5()
            .py_3()
            .rounded_lg()
            .bg(theme.raised)
            .border_1()
            .border_color(theme.border)
            .child(
                div()
                    .size(px(36.))
                    .flex_none()
                    .rounded_full()
                    .bg(theme.selected)
                    .flex()
                    .items_center()
                    .justify_center()
                    .font_weight(FontWeight::BOLD)
                    .text_color(theme.accent)
                    .child(
                        user.username
                            .chars()
                            .next()
                            .map(|c| c.to_uppercase().to_string())
                            .unwrap_or_default(),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.text)
                            .child(user.username.clone())
                            .when(user.owner, |d| {
                                d.child(
                                    div()
                                        .px_1p5()
                                        .rounded_sm()
                                        .bg(theme.selected)
                                        .text_xs()
                                        .text_color(theme.accent)
                                        .child(t!("admin-owner-badge")),
                                )
                            })
                            .when(me, |d| {
                                d.child(
                                    div()
                                        .font_weight(FontWeight::NORMAL)
                                        .text_color(theme.muted)
                                        .child(t!("admin-you")),
                                )
                            }),
                    )
                    .child(div().text_xs().text_color(theme.muted).child(facts)),
            )
            .child(action)
            .into_any_element()
    }
}

impl Render for UsersPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme::get(cx).clone();
        let add = div()
            .flex()
            .flex_col()
            .gap_2()
            .child(hint(t!("admin-add-owner-help"), &theme))
            .child(self.picker.clone());
        let mut accounts = div().flex().flex_col().gap_2().child(
            Input::new(&self.filter)
                .id("admin-users-filter")
                .aria_label(t!("admin-filter-accounts")),
        );
        if let Some(text) = &self.notice {
            accounts = accounts.child(notice("admin-users-notice", text.clone(), false, &theme));
        }
        if let Some(error) = &self.error {
            accounts = accounts.child(notice("admin-users-error", error.clone(), true, &theme));
        }
        let users = self.users.clone();
        let mut list = div()
            .id("admin-users")
            .test_support()
            .role(Role::List)
            .aria_label(t!("admin-accounts-list"))
            .flex()
            .flex_col()
            .gap_2();
        for (i, user) in users.iter().enumerate() {
            list = list.child(self.row(i, user, cx));
        }
        accounts = accounts.child(list);
        if !self.loading && self.users.is_empty() && self.error.is_none() {
            accounts = accounts.child(notice(
                "admin-users-empty",
                t!("admin-no-accounts-match", query = self.query.as_str()),
                false,
                &theme,
            ));
        }
        if self.more {
            accounts = accounts.child(div().child(button(
                "admin-users-more",
                t!("admin-show-more"),
                None,
                Kind::Secondary,
                !self.loading,
                &theme,
                cx.listener(|this, _, _, cx| this.load(true, cx)),
            )));
        }
        div()
            .flex()
            .flex_col()
            .gap_6()
            .child(group("admin-add-owner", t!("admin-add-owner"), add, &theme))
            .child(group(
                "admin-accounts",
                t!("admin-accounts-list"),
                accounts,
                &theme,
            ))
    }
}

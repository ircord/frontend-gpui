//! Channel registration with services (package CH2), as the web client's
//! `ChannelRegistration.vue` draws it: the Registration tab of the channel
//! settings, with who founded the channel and when, registering it to your
//! account, the access list (roles ChanServ gives by account when people
//! join), transferring the channel to another account, and unregistering
//! it, which asks you to type its name. A network without a ChanServ IRCord
//! can manage says so and how to ask its services instead; what you may not
//! do is explained rather than hidden.

use std::rc::Rc;

use gpui_kit::TestSupportExt;
use gpui_kit::component::Disableable;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::*;
use gpui_kit::*;
use ircord_client::Client;
use ircord_client::api::ApiError;
use ircord_client::contract::{self as c, AccessChangeBodyMode as Grant};

use crate::features::channel_settings::{Here, choices, date, heading, here, hint};
use crate::session::Session;
use crate::theme::Theme;
use crate::{theme, ui};

pub fn init(_cx: &mut App) {
    // Drawn inside the channel settings (`channel_settings.rs`).
}

enum Status {
    Loading,
    /// The network has no ChanServ IRCord can manage.
    Unsupported,
    Failed(SharedString),
    Ready(c::ChannelRegistration),
}

enum Access {
    Loading,
    /// Only the founder and admins or operators may read it.
    Denied,
    Failed(SharedString),
    Ready(Vec<c::AccessEntry>),
}

pub struct RegistrationView {
    session: Entity<Session>,
    conversation: String,
    status: Status,
    access: Access,
    busy: bool,
    error: Option<SharedString>,
    saved: Option<SharedString>,
    new_account: Entity<InputState>,
    new_role: Grant,
    transfer_to: Entity<InputState>,
    /// The account a transfer was offered to, until they accept.
    transfer_pending: Option<String>,
    /// Empty the add field on the next draw (an add went through).
    pending_clear: bool,
    pending_transfer_clear: bool,
    _subscriptions: Vec<Subscription>,
}

fn role_label(mode: c::AccessEntryMode) -> &'static str {
    match mode {
        c::AccessEntryMode::Q => t!("channel-settings-role-founder"),
        c::AccessEntryMode::A => t!("channel-settings-role-admin"),
        c::AccessEntryMode::O => t!("channel-settings-role-operator"),
        c::AccessEntryMode::H => t!("channel-settings-role-half-operator"),
        c::AccessEntryMode::V => t!("channel-settings-role-voice"),
        c::AccessEntryMode::Unknown => t!("channel-settings-role-other"),
    }
}

/// The roles that can be given: everything but founder.
fn grantable() -> Vec<(Grant, SharedString)> {
    vec![
        (Grant::A, t!("channel-settings-role-admin").into()),
        (Grant::O, t!("channel-settings-role-operator").into()),
        (Grant::H, t!("channel-settings-role-half-operator").into()),
        (Grant::V, t!("channel-settings-role-voice").into()),
    ]
}

fn as_change(mode: c::AccessEntryMode) -> Grant {
    match mode {
        c::AccessEntryMode::A => Grant::A,
        c::AccessEntryMode::H => Grant::H,
        c::AccessEntryMode::V => Grant::V,
        _ => Grant::O,
    }
}

fn status_of(e: &ApiError) -> Option<u16> {
    match e {
        ApiError::Http { status, .. } => Some(*status),
        _ => None,
    }
}

impl RegistrationView {
    pub fn new(
        session: Entity<Session>,
        conversation: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let new_account = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t!("channel-settings-access-account"))
        });
        let transfer_to = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t!("channel-settings-transfer-placeholder"))
        });
        let subscriptions = vec![
            cx.subscribe_in(
                &new_account,
                window,
                |this: &mut Self, _, event: &InputEvent, window, cx| match event {
                    InputEvent::Change => cx.notify(),
                    InputEvent::PressEnter { .. } => this.add_access(window, cx),
                    _ => {}
                },
            ),
            cx.subscribe_in(
                &transfer_to,
                window,
                |this: &mut Self, _, event: &InputEvent, window, cx| match event {
                    InputEvent::Change => cx.notify(),
                    InputEvent::PressEnter { .. } => this.transfer(window, cx),
                    _ => {}
                },
            ),
        ];
        let mut view = Self {
            session,
            conversation,
            status: Status::Loading,
            access: Access::Loading,
            busy: false,
            error: None,
            saved: None,
            new_account,
            new_role: Grant::O,
            transfer_to,
            transfer_pending: None,
            pending_clear: false,
            pending_transfer_clear: false,
            _subscriptions: subscriptions,
        };
        view.reload(cx);
        view
    }

    fn client(&self, cx: &App) -> Option<Rc<Client>> {
        self.session.read(cx).client().cloned()
    }

    /// Ask for the registration, and the access list when it is registered.
    fn reload(&mut self, cx: &mut Context<Self>) {
        let Some(client) = self.client(cx) else {
            return;
        };
        let conversation = self.conversation.clone();
        let task = client.request(move |api| async move {
            let status = api.registration(&conversation).await;
            let access = match &status {
                Ok(r) if r.registered => Some(api.channel_access(&conversation).await),
                _ => None,
            };
            Ok((status, access))
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok((status, access))) = task.await else {
                return;
            };
            this.update(cx, |this, cx| {
                this.status = match status {
                    Ok(r) => Status::Ready(r),
                    Err(e) if status_of(&e) == Some(501) => Status::Unsupported,
                    Err(e) => Status::Failed(e.to_string().into()),
                };
                this.access = match access {
                    None => Access::Loading,
                    Some(Ok(items)) => Access::Ready(items),
                    Some(Err(e)) if status_of(&e) == Some(403) => Access::Denied,
                    Some(Err(e)) => Access::Failed(e.to_string().into()),
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Run one services action; errors show here, and the views refresh after.
    fn run<T: Send + 'static, Fut>(
        &mut self,
        action: impl FnOnce(ircord_client::api::Api) -> Fut + Send + 'static,
        done: impl FnOnce(&mut Self, T) -> Option<SharedString> + 'static,
        cx: &mut Context<Self>,
    ) where
        Fut: std::future::Future<Output = Result<T, ApiError>> + Send + 'static,
    {
        let Some(client) = self.client(cx) else {
            return;
        };
        if self.busy {
            return;
        }
        self.busy = true;
        self.error = None;
        self.saved = None;
        cx.notify();
        let task = client.request(action);
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                this.busy = false;
                match result {
                    Ok(Ok(value)) => this.saved = done(this, value),
                    Ok(Err(e)) => this.error = Some(e.to_string().into()),
                    Err(e) => this.error = Some(e.to_string().into()),
                }
                this.reload(cx);
            })
            .ok();
        })
        .detach();
    }

    fn register(&mut self, cx: &mut Context<Self>) {
        let Some(h) = here(&self.session, &self.conversation, cx) else {
            return;
        };
        let conversation = self.conversation.clone();
        let channel = h.target();
        self.run(
            move |api| async move { api.register_channel(&conversation).await },
            move |_, _| Some(t!("channel-settings-registered-now", channel = channel).into()),
            cx,
        );
    }

    fn unregister(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(h) = here(&self.session, &self.conversation, cx) else {
            return;
        };
        let channel = h.target();
        let this = cx.entity().downgrade();
        // Like the web client, the channel's name must be typed to go on.
        ui::prompt::prompt(
            window,
            cx,
            ui::prompt::Prompt {
                title: t!(
                    "channel-settings-unregister-title",
                    channel = channel.clone()
                )
                .into(),
                label: t!(
                    "channel-settings-unregister-type",
                    channel = channel.clone()
                )
                .into(),
                initial: String::new(),
                ok: t!("channel-settings-unregister").into(),
            },
            move |typed, window, cx| {
                if !typed.trim().eq_ignore_ascii_case(&channel) {
                    return ui::toast::info(
                        window,
                        cx,
                        t!(
                            "channel-settings-unregister-mismatch",
                            channel = channel.clone()
                        ),
                    );
                }
                let channel = channel.clone();
                this.update(cx, |v, cx| {
                    let conversation = v.conversation.clone();
                    v.run(
                        move |api| async move { api.unregister_channel(&conversation).await },
                        move |_, _| {
                            Some(t!("channel-settings-unregistered-now", channel = channel).into())
                        },
                        cx,
                    )
                })
                .ok();
            },
        );
    }

    fn add_access(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let account = self.new_account.read(cx).value().trim().to_string();
        if account.is_empty() {
            return;
        }
        let (conversation, role) = (self.conversation.clone(), self.new_role);
        self.run(
            move |api| async move { api.set_channel_access(&conversation, &account, role).await },
            |this, _| {
                this.pending_clear = true;
                None
            },
            cx,
        );
    }

    fn change_role(&mut self, account: String, role: Grant, cx: &mut Context<Self>) {
        let conversation = self.conversation.clone();
        self.run(
            move |api| async move { api.set_channel_access(&conversation, &account, role).await },
            |_, _| None,
            cx,
        );
    }

    fn remove_access(
        &mut self,
        entry: c::AccessEntry,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let this = cx.entity().downgrade();
        let account = entry.account.clone();
        ui::confirm::confirm(
            window,
            cx,
            ui::confirm::Confirm {
                title: t!(
                    "channel-settings-access-remove-title",
                    account = account.clone()
                )
                .into(),
                body: t!(
                    "channel-settings-access-remove-body",
                    role = role_label(entry.mode).to_lowercase()
                )
                .into(),
                ok: t!("channel-settings-access-remove").into(),
                danger: false,
            },
            move |_, cx| {
                let account = account.clone();
                this.update(cx, |v, cx| {
                    let conversation = v.conversation.clone();
                    v.run(
                        move |api| async move {
                            api.remove_channel_access(&conversation, &account).await
                        },
                        |_, _| None,
                        cx,
                    )
                })
                .ok();
            },
        );
    }

    fn transfer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(h) = here(&self.session, &self.conversation, cx) else {
            return;
        };
        let account = self.transfer_to.read(cx).value().trim().to_string();
        if account.is_empty() {
            return;
        }
        let this = cx.entity().downgrade();
        ui::confirm::confirm(
            window,
            cx,
            ui::confirm::Confirm {
                title: t!(
                    "channel-settings-transfer-title",
                    channel = h.target(),
                    account = account.clone()
                )
                .into(),
                body: t!("channel-settings-transfer-body").into(),
                ok: t!("channel-settings-transfer").into(),
                danger: true,
            },
            move |_, cx| {
                let account = account.clone();
                this.update(cx, |v, cx| {
                    let conversation = v.conversation.clone();
                    let to = account.clone();
                    v.run(
                        move |api| async move { api.transfer_channel(&conversation, &to).await },
                        move |this, result: c::TransferResult| {
                            this.pending_transfer_clear = true;
                            if result.status == c::TransferResultStatus::Pending {
                                this.transfer_pending = Some(account);
                                None
                            } else {
                                this.transfer_pending = None;
                                Some(t!("channel-settings-transfer-done").into())
                            }
                        },
                        cx,
                    )
                })
                .ok();
            },
        );
    }

    // ---- Drawing ----

    fn render_access(
        &self,
        info: &c::ChannelRegistration,
        founder: bool,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Div {
        let mut out = div()
            .flex()
            .flex_col()
            .gap_2()
            .child(heading(t!("channel-settings-access"), theme))
            .child(hint(t!("channel-settings-access-about"), theme));
        if founder {
            let this = cx.entity().downgrade();
            out = out.child(
                div()
                    .id("channel-settings-access-add")
                    .test_support()
                    .role(Role::Form)
                    .aria_label(t!("channel-settings-access-add"))
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        Input::new(&self.new_account)
                            .id("channel-settings-access-account")
                            .aria_label(t!("channel-settings-access-account")),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(choices(
                                "channel-settings-access-role",
                                t!("channel-settings-access-role"),
                                grantable(),
                                self.new_role,
                                !self.busy,
                                move |role, _, cx| {
                                    this.update(cx, |v, cx| {
                                        v.new_role = role;
                                        cx.notify();
                                    })
                                    .ok();
                                },
                            ))
                            .child(
                                Button::new("channel-settings-access-add-button")
                                    .primary()
                                    .label(t!("channel-settings-access-add"))
                                    .disabled(
                                        self.busy
                                            || self.new_account.read(cx).value().trim().is_empty(),
                                    )
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.add_access(window, cx)
                                    })),
                            ),
                    ),
            );
        }
        let founder_account = info.founder.clone().unwrap_or_default().to_lowercase();
        let list: AnyElement = match &self.access {
            Access::Loading => {
                hint(t!("channel-settings-access-loading"), theme).into_any_element()
            }
            Access::Denied => hint(t!("channel-settings-access-denied"), theme).into_any_element(),
            Access::Failed(e) => div()
                .text_sm()
                .text_color(theme.danger)
                .child(e.clone())
                .into_any_element(),
            Access::Ready(items) => {
                let mut list = div()
                    .id("channel-settings-access-list")
                    .test_support()
                    .role(Role::List)
                    .aria_label(t!("channel-settings-access"))
                    .flex()
                    .flex_col()
                    .gap_1();
                for (i, entry) in items.iter().enumerate() {
                    let editable = founder && entry.account.to_lowercase() != founder_account;
                    let account = entry.account.clone();
                    let this = cx.entity().downgrade();
                    let mut row = div()
                        .id(ElementId::Name(
                            format!("channel-settings-access-{i}").into(),
                        ))
                        .test_support()
                        .role(Role::ListItem)
                        .aria_label(format!("{} · {}", entry.account, role_label(entry.mode)))
                        .flex()
                        .items_center()
                        .gap_3()
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .bg(theme.panel)
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_sm()
                                .truncate()
                                .child(entry.account.clone()),
                        );
                    if editable {
                        row = row
                            .child(choices(
                                format!("channel-settings-access-{i}-role"),
                                t!(
                                    "channel-settings-access-role-for",
                                    account = account.clone()
                                ),
                                grantable(),
                                as_change(entry.mode),
                                !self.busy,
                                move |role, _, cx| {
                                    let account = account.clone();
                                    this.update(cx, |v, cx| v.change_role(account, role, cx))
                                        .ok();
                                },
                            ))
                            .child({
                                let entry = entry.clone();
                                Button::new(ElementId::Name(
                                    format!("channel-settings-access-{i}-remove").into(),
                                ))
                                .ghost()
                                .label(t!("channel-settings-access-remove"))
                                .accessibility_label(t!(
                                    "channel-settings-access-remove-account",
                                    account = entry.account.clone()
                                ))
                                .disabled(self.busy)
                                .on_click(cx.listener(
                                    move |this, _, window, cx| {
                                        this.remove_access(entry.clone(), window, cx)
                                    },
                                ))
                            });
                    } else {
                        row = row.child(
                            div()
                                .text_sm()
                                .text_color(theme.muted)
                                .child(role_label(entry.mode)),
                        );
                    }
                    list = list.child(row);
                }
                list.into_any_element()
            }
        };
        out.child(list)
    }

    fn render_ownership(&self, h: &Here, theme: &Theme, cx: &mut Context<Self>) -> Div {
        let mut out = div()
            .flex()
            .flex_col()
            .gap_2()
            .child(heading(t!("channel-settings-ownership"), theme))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div().flex_1().child(
                            Input::new(&self.transfer_to)
                                .id("channel-settings-transfer-account")
                                .aria_label(t!("channel-settings-transfer-account")),
                        ),
                    )
                    .child(
                        Button::new("channel-settings-transfer")
                            .label(t!("channel-settings-transfer"))
                            .disabled(
                                self.busy || self.transfer_to.read(cx).value().trim().is_empty(),
                            )
                            .on_click(cx.listener(|this, _, window, cx| this.transfer(window, cx))),
                    ),
            );
        if let Some(account) = &self.transfer_pending {
            out = out
                .child(
                    div()
                        .id("channel-settings-transfer-pending")
                        .test_support()
                        .role(Role::Status)
                        .aria_label(t!(
                            "channel-settings-transfer-offered",
                            account = account.clone()
                        ))
                        .text_sm()
                        .child(t!(
                            "channel-settings-transfer-offered",
                            account = account.clone()
                        )),
                )
                .child(hint(t!("channel-settings-transfer-accept"), theme))
                .child(
                    div()
                        .text_sm()
                        .font_family("monospace")
                        .child(format!("/msg ChanServ TRANSFER accept {}", h.target())),
                );
        }
        out.child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap_4()
                .pt_2()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_0p5()
                        .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child(t!(
                            "channel-settings-unregister-channel",
                            channel = h.target()
                        )))
                        .child(hint(t!("channel-settings-unregister-about"), theme)),
                )
                .child(
                    Button::new("channel-settings-unregister")
                        .danger()
                        .label(t!("channel-settings-unregister"))
                        .disabled(self.busy)
                        .on_click(cx.listener(|this, _, window, cx| this.unregister(window, cx))),
                ),
        )
    }
}

impl Render for RegistrationView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if std::mem::take(&mut self.pending_clear) {
            self.new_account
                .update(cx, |s, cx| s.set_value("", window, cx));
        }
        if std::mem::take(&mut self.pending_transfer_clear) {
            self.transfer_to
                .update(cx, |s, cx| s.set_value("", window, cx));
        }
        let theme = theme::get(cx).clone();
        let page = div()
            .id("channel-settings-registration")
            .test_support()
            .role(Role::Region)
            .aria_label(t!("channel-settings-tab-registration"))
            .flex()
            .flex_col()
            .gap_5();
        let Some(h) = here(&self.session, &self.conversation, cx) else {
            return page;
        };
        let network = h.network_name();
        let page = page
            .children(self.error.clone().map(|e| {
                div()
                    .id("channel-settings-registration-error")
                    .test_support()
                    .role(Role::Alert)
                    .aria_label(e.clone())
                    .text_sm()
                    .text_color(theme.danger)
                    .child(e)
            }))
            .children(self.saved.clone().map(|s| {
                div()
                    .id("channel-settings-registration-saved")
                    .test_support()
                    .role(Role::Status)
                    .aria_label(s.clone())
                    .text_sm()
                    .text_color(theme.online)
                    .child(s)
            }));
        let info = match &self.status {
            Status::Loading => {
                return page.child(hint(t!("channel-settings-registration-loading"), &theme));
            }
            Status::Unsupported => {
                return page
                    .child(heading(t!("channel-settings-tab-registration"), &theme))
                    .child(
                        div()
                            .id("channel-settings-registration-unsupported")
                            .test_support()
                            .role(Role::Status)
                            .aria_label(t!(
                                "channel-settings-registration-unsupported",
                                network = network.clone()
                            ))
                            .text_sm()
                            .child(t!(
                                "channel-settings-registration-unsupported",
                                network = network
                            )),
                    )
                    .child(hint(
                        t!(
                            "channel-settings-registration-unsupported-help",
                            command = "/msg ChanServ help"
                        ),
                        &theme,
                    ));
            }
            Status::Failed(e) => {
                return page
                    .child(div().text_sm().text_color(theme.danger).child(e.clone()))
                    .child(
                        div().child(
                            Button::new("channel-settings-registration-retry")
                                .label(t!("channel-settings-try-again"))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.status = Status::Loading;
                                    this.reload(cx);
                                    cx.notify();
                                })),
                        ),
                    );
            }
            Status::Ready(info) => info.clone(),
        };
        let account = h
            .network
            .as_ref()
            .and_then(|n| n.account.clone())
            .filter(|a| !a.is_empty());
        let founder = account.as_ref().is_some_and(|a| {
            info.founder
                .as_deref()
                .is_some_and(|f| f.eq_ignore_ascii_case(a))
        });
        let mut status = div()
            .flex()
            .flex_col()
            .gap_2()
            .child(heading(t!("channel-settings-tab-registration"), &theme));
        if info.registered {
            let founder_text = match (&info.founder, founder) {
                (Some(f), true) => t!("channel-settings-founder-you", account = f.clone()),
                (Some(f), false) => f.clone(),
                (None, _) => String::new(),
            };
            let mut facts = div()
                .id("channel-settings-registration-facts")
                .test_support()
                .role(Role::Group)
                .aria_label(t!("channel-settings-tab-registration"))
                .flex()
                .gap_6()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.faint)
                                .child(t!("channel-settings-role-founder")),
                        )
                        .child(div().text_sm().child(founder_text)),
                );
            if let Some(at) = info
                .registered_at
                .as_deref()
                .map(date)
                .filter(|d| !d.is_empty())
            {
                let when = if info.automatic == Some(true) {
                    t!("channel-settings-registered-automatically", date = at)
                } else {
                    at
                };
                facts = facts.child(
                    div()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.faint)
                                .child(t!("channel-settings-registered")),
                        )
                        .child(div().text_sm().child(when)),
                );
            }
            status = status.child(facts).child(hint(
                t!("channel-settings-registration-persistence"),
                &theme,
            ));
        } else {
            let why: SharedString = match &account {
                None => t!(
                    "channel-settings-register-log-in",
                    network = network.clone()
                )
                .into(),
                Some(_) if !h.rights.op => t!(
                    "channel-settings-register-operator",
                    prefix = "@",
                    channel = h.target()
                )
                .into(),
                Some(a) => t!("channel-settings-register-as", account = a.clone()).into(),
            };
            status = status
                .child(
                    div()
                        .id("channel-settings-registration-none")
                        .test_support()
                        .role(Role::Status)
                        .aria_label(t!("channel-settings-not-registered", channel = h.target()))
                        .text_sm()
                        .child(t!("channel-settings-not-registered", channel = h.target())),
                )
                .child(
                    div().flex().items_center().gap_3().child(
                        Button::new("channel-settings-register")
                            .primary()
                            .label(t!("channel-settings-register", channel = h.target()))
                            .disabled(self.busy || account.is_none() || !h.rights.op)
                            .on_click(cx.listener(|this, _, _, cx| this.register(cx))),
                    ),
                )
                .child(hint(why, &theme));
        }
        let mut page = page.child(status);
        if info.registered {
            page = page.child(self.render_access(&info, founder, &theme, cx));
            if founder {
                page = page.child(self.render_ownership(&h, &theme, cx));
            }
        }
        page
    }
}

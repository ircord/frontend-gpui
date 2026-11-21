//! The server address screen, the sign-in screen and creating an account
//! (package S4: the web client's `LoginScreen.vue`). The username is also
//! the account's nick, so it is checked with the server while it is typed.

use crate::ui::focus::Reachable;
use gpui_kit::TestSupportExt;
use gpui_kit::component::Disableable;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::*;
use gpui_kit::*;

use std::time::Duration;

use ircord_client::api::Api;
use ircord_client::api::auth::{self, PasswordProblem};
use ircord_client::contract::{RegistrationPolicy, SignupCredentials, UsernameCheck};

use crate::session::{Phase, Session, SessionEvent};
use crate::settings::profile::error_text;
use crate::theme;
use crate::ui::field::field;

/// How long typing pauses before the username is checked.
const CHECK_DELAY: Duration = Duration::from_millis(350);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    SignIn,
    SignUp,
}

/// What the server said about a typed username.
#[derive(Clone, Debug, PartialEq)]
enum Check {
    None,
    Checking,
    Answer(UsernameCheck),
}

pub struct SignInView {
    session: Entity<Session>,
    address: Entity<InputState>,
    username: Entity<InputState>,
    password: Entity<InputState>,
    pub mode: Mode,
    new_username: Entity<InputState>,
    new_password: Entity<InputState>,
    key: Entity<InputState>,
    /// The server's registration policy: `None` while it is read, `Err`
    /// when it could not be.
    policy: Option<Result<RegistrationPolicy, String>>,
    policy_for: Option<String>,
    check: Check,
    /// The latest username check; earlier answers are dropped.
    ticket: u64,
    _check: Option<Task<()>>,
    /// Why the sign-up form was not sent, under the field it is about.
    problem: Option<(&'static str, SharedString)>,
    _subscriptions: Vec<Subscription>,
}

impl SignInView {
    pub fn new(session: Entity<Session>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let address = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t!("core-signin-address-placeholder"))
        });
        let username =
            cx.new(|cx| InputState::new(window, cx).placeholder(t!("core-signin-username")));
        let password = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t!("core-signin-password"))
                .masked(true)
        });
        let enter = |field: &Entity<InputState>, window: &mut Window, cx: &mut Context<Self>| {
            cx.subscribe_in(
                field,
                window,
                |this: &mut Self, _, event: &InputEvent, window, cx| {
                    if let InputEvent::PressEnter { .. } = event {
                        this.submit(window, cx);
                    }
                },
            )
        };
        let new_username =
            cx.new(|cx| InputState::new(window, cx).placeholder(t!("profile-signup-username")));
        let new_password = cx.new(|cx| InputState::new(window, cx).masked(true));
        let key = cx.new(|cx| InputState::new(window, cx));
        let edited = |field: &Entity<InputState>,
                      which: &'static str,
                      window: &mut Window,
                      cx: &mut Context<Self>| {
            cx.subscribe_in(
                field,
                window,
                move |this: &mut Self, _, event: &InputEvent, window, cx| match event {
                    InputEvent::Change => {
                        if this.problem.as_ref().is_some_and(|(id, _)| *id == which) {
                            this.problem = None;
                        }
                        if which != "signup-password" {
                            this.recheck(cx);
                        }
                        cx.notify();
                    }
                    InputEvent::PressEnter { .. } => this.submit(window, cx),
                    _ => {}
                },
            )
        };
        let subscriptions = vec![
            enter(&address, window, cx),
            enter(&username, window, cx),
            enter(&password, window, cx),
            edited(&new_username, "signup-username", window, cx),
            edited(&new_password, "signup-password", window, cx),
            edited(&key, "registration-key", window, cx),
            cx.observe(&session, |this, _, cx| {
                this.load_policy(cx);
                cx.notify()
            }),
            cx.subscribe_in(
                &session,
                window,
                |this, _, event: &SessionEvent, window, cx| {
                    if let SessionEvent::Phase = event {
                        this.mode = Mode::SignIn;
                        this.focus_first(window, cx);
                        cx.notify();
                    }
                },
            ),
        ];
        let mut view = Self {
            session,
            address,
            username,
            password,
            mode: Mode::SignIn,
            new_username,
            new_password,
            key,
            policy: None,
            policy_for: None,
            check: Check::None,
            ticket: 0,
            _check: None,
            problem: None,
            _subscriptions: subscriptions,
        };
        view.load_policy(cx);
        view.focus_first(window, cx);
        view
    }

    /// Put the keyboard in the form's first field, so typing and Tab work
    /// as soon as the screen shows.
    fn focus_first(&self, window: &mut Window, cx: &mut Context<Self>) {
        let first = if self.choosing_server(cx) {
            &self.address
        } else {
            match self.mode {
                Mode::SignIn => &self.username,
                Mode::SignUp => &self.new_username,
            }
        };
        first.focus_handle(cx).focus(window, cx);
    }

    /// Read whether the chosen server takes new accounts, once per server.
    fn load_policy(&mut self, cx: &mut Context<Self>) {
        let Some(server) = self.session.read(cx).server().cloned() else {
            return;
        };
        if matches!(self.session.read(cx).phase, Phase::SignedIn { .. }) {
            return;
        }
        let origin = server.origin();
        if self.policy_for.as_deref() == Some(origin.as_str()) {
            return;
        }
        self.policy_for = Some(origin.clone());
        self.policy = None;
        let api = Api::new(server);
        let task = self
            .session
            .read(cx)
            .runtime
            .spawn(async move { api.registration().await });
        cx.spawn(async move |this, cx| {
            let result = match task.await {
                Ok(Ok(policy)) => Ok(policy),
                Ok(Err(e)) => Err(error_text(&e)),
                Err(e) => Err(e.to_string()),
            };
            this.update(cx, |this, cx| {
                if this.policy_for.as_deref() == Some(origin.as_str()) {
                    if matches!(
                        result,
                        Ok(RegistrationPolicy {
                            registration_enabled: false,
                            ..
                        })
                    ) {
                        this.mode = Mode::SignIn;
                    }
                    this.policy = Some(result);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    fn key_required(&self) -> bool {
        matches!(&self.policy, Some(Ok(p)) if p.registration_key_required)
    }

    /// Check the typed username after a pause; only the latest answer counts.
    fn recheck(&mut self, cx: &mut Context<Self>) {
        self.ticket += 1;
        self.check = Check::None;
        self._check = None;
        let name = self.new_username.read(cx).value().trim().to_string();
        let key = self.key.read(cx).value().trim().to_string();
        if self.mode != Mode::SignUp || name.is_empty() || (self.key_required() && key.is_empty()) {
            return;
        }
        let Some(server) = self.session.read(cx).server().cloned() else {
            return;
        };
        self.check = Check::Checking;
        let ticket = self.ticket;
        let runtime = self.session.read(cx).runtime.clone();
        let key = self.key_required().then_some(key);
        self._check = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(CHECK_DELAY).await;
            let api = Api::new(server);
            let answer = runtime
                .spawn(async move { api.check_username(&name, key.as_deref()).await })
                .await;
            this.update(cx, |this, cx| {
                if this.ticket != ticket {
                    return;
                }
                // A failed lookup just drops the hint; creating checks again.
                this.check = match answer {
                    Ok(Ok(check)) => Check::Answer(check),
                    _ => Check::None,
                };
                cx.notify();
            })
            .ok();
        }));
    }

    /// Show the sign-in or the sign-up form.
    pub fn go(&mut self, mode: Mode, window: &mut Window, cx: &mut Context<Self>) {
        self.mode = mode;
        self.problem = None;
        self.session.update(cx, |s, cx| {
            s.error = None;
            cx.notify()
        });
        for field in [&self.password, &self.new_password, &self.key] {
            field.update(cx, |s, cx| s.set_value("", window, cx));
        }
        let first = match mode {
            Mode::SignIn => &self.username,
            Mode::SignUp => &self.new_username,
        };
        first.focus_handle(cx).focus(window, cx);
        self.recheck(cx);
        cx.notify();
    }

    /// Create the account, after the checks the form can make itself.
    fn sign_up(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let username = self.new_username.read(cx).value().trim().to_string();
        let password = self.new_password.read(cx).value().to_string();
        let key = self.key.read(cx).value().trim().to_string();
        let stop = |this: &mut Self,
                    id: &'static str,
                    text: SharedString,
                    field: Entity<InputState>,
                    window: &mut Window,
                    cx: &mut Context<Self>| {
            this.problem = Some((id, text));
            field.focus_handle(cx).focus(window, cx);
            cx.notify();
        };
        if username.is_empty() {
            let f = self.new_username.clone();
            return stop(
                self,
                "signup-username",
                t!("profile-signup-enter-username").into(),
                f,
                window,
                cx,
            );
        }
        if let Check::Answer(check) = &self.check
            && !check.available
        {
            let reason: SharedString = check
                .reason
                .clone()
                .unwrap_or_else(|| t!("profile-signup-unavailable").into())
                .into();
            let f = self.new_username.clone();
            return stop(self, "signup-username", reason, f, window, cx);
        }
        if let Some(problem) = auth::password_problem(&password) {
            let text = match problem {
                PasswordProblem::Short => t!("profile-signup-password-short"),
                PasswordProblem::Spaces => t!("profile-signup-password-spaces"),
            };
            let f = self.new_password.clone();
            return stop(self, "signup-password", text.into(), f, window, cx);
        }
        if self.key_required() && key.is_empty() {
            let f = self.key.clone();
            return stop(
                self,
                "registration-key",
                t!("profile-signup-enter-key").into(),
                f,
                window,
                cx,
            );
        }
        let Some(server) = self.session.read(cx).server().cloned() else {
            return;
        };
        let (runtime, store) = {
            let s = self.session.read(cx);
            (s.runtime.clone(), s.credentials.clone())
        };
        self.session.update(cx, |s, cx| {
            s.busy = true;
            s.error = None;
            cx.notify();
        });
        let body = SignupCredentials {
            username,
            password,
            registration_key: (!key.is_empty()).then_some(key),
            display_name: None,
        };
        let api = Api::new(server.clone());
        let task = runtime.spawn(async move {
            let (cookie, _) = api.signup(&body).await.map_err(|e| error_text(&e))?;
            store
                .save(api.server(), &cookie)
                .map_err(|e| e.to_string())?;
            Ok::<_, String>(cookie)
        });
        let session = self.session.clone();
        cx.spawn(async move |_, cx| {
            let result = task.await.map_err(|e| e.to_string()).and_then(|r| r);
            session.update(cx, |s, cx| match result {
                Ok(cookie) => s.start(server, cookie, cx),
                Err(e) => {
                    s.busy = false;
                    s.error = Some(e);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn choosing_server(&self, cx: &App) -> bool {
        matches!(self.session.read(cx).phase, Phase::ChooseServer)
    }

    fn submit(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if self.session.read(cx).busy {
            return;
        }
        if self.choosing_server(cx) {
            let address = self.address.read(cx).value().to_string();
            self.session
                .update(cx, |s, cx| s.choose_server(&address, cx));
        } else if self.mode == Mode::SignUp {
            self.sign_up(_window, cx);
        } else {
            let username = self.username.read(cx).value().to_string();
            let password = self.password.read(cx).value().to_string();
            self.session
                .update(cx, |s, cx| s.sign_in(&username, &password, cx));
        }
    }
}

impl SignInView {
    /// A line under a sign-up field: the reason it stopped the form, else `help`.
    fn note(
        &self,
        id: &'static str,
        help: Option<SharedString>,
        theme: &theme::Theme,
    ) -> Option<AnyElement> {
        let (text, danger) = match &self.problem {
            Some((which, text)) if *which == id => (text.clone(), true),
            _ => (help?, false),
        };
        Some(
            div()
                .id(ElementId::Name(format!("{id}-note").into()))
                .test_support()
                .role(if danger { Role::Alert } else { Role::Note })
                .aria_label(text.clone())
                .text_xs()
                .text_color(if danger { theme.danger } else { theme.muted })
                .child(text)
                .into_any_element(),
        )
    }

    fn sign_up_fields(&self, form: Stateful<Div>, theme: &theme::Theme, cx: &App) -> Stateful<Div> {
        let typed = self.new_username.read(cx).value().trim().to_string();
        let username_help: SharedString = match &self.check {
            Check::Checking => t!("profile-signup-checking").into(),
            Check::Answer(c) if c.available => {
                t!("profile-signup-available", username = typed).into()
            }
            Check::Answer(c) => c
                .reason
                .clone()
                .unwrap_or_else(|| t!("profile-signup-unavailable").into())
                .into(),
            Check::None => t!("profile-signup-username-help").into(),
        };
        let mut form = form
            .child(
                field(
                    t!("profile-signup-username"),
                    Input::new(&self.new_username)
                        .id("signup-username")
                        .aria_label(t!("profile-signup-username")),
                    theme,
                )
                .children(self.note("signup-username", Some(username_help), theme)),
            )
            .child(
                field(
                    t!("core-signin-password"),
                    Input::new(&self.new_password)
                        .id("signup-password")
                        .aria_label(t!("core-signin-password")),
                    theme,
                )
                .children(self.note(
                    "signup-password",
                    Some(t!("profile-signup-password-help").into()),
                    theme,
                )),
            );
        if self.key_required() {
            form = form.child(
                field(
                    t!("profile-signup-key"),
                    Input::new(&self.key)
                        .id("registration-key")
                        .aria_label(t!("profile-signup-key")),
                    theme,
                )
                .children(self.note(
                    "registration-key",
                    Some(t!("profile-signup-key-help").into()),
                    theme,
                )),
            );
        }
        form
    }
}

impl Render for SignInView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme::get(cx).clone();
        let (busy, error, choosing, server) = {
            let session = self.session.read(cx);
            (
                session.busy,
                session.error.clone(),
                matches!(session.phase, Phase::ChooseServer),
                session.server().map(|s| s.origin()).unwrap_or_default(),
            )
        };
        let signing_up = !choosing && self.mode == Mode::SignUp;

        let title = if choosing {
            t!("core-signin-connect-title")
        } else if signing_up {
            t!("profile-signup-title")
        } else {
            t!("core-signin-welcome")
        };
        let subtitle = if choosing {
            t!("core-signin-connect-hint").to_string()
        } else if signing_up {
            t!("profile-signup-description", server = server.clone())
        } else {
            t!("core-signin-sign-in-to", server = server)
        };

        let mut form = div()
            .id("sign-in-form")
            .w(px(400.))
            .p_8()
            .rounded_lg()
            .bg(theme.sidebar)
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .id("sign-in-title")
                    .test_support()
                    .role(Role::Heading)
                    .aria_label(SharedString::from(title))
                    .text_2xl()
                    .font_weight(FontWeight::BOLD)
                    .text_color(theme.text)
                    .child(title),
            )
            .child(div().text_sm().text_color(theme.muted).child(subtitle));

        if choosing {
            form = form.child(field(
                t!("core-signin-address"),
                Input::new(&self.address)
                    .id("address")
                    .aria_label(t!("core-signin-address")),
                &theme,
            ));
        } else if signing_up {
            form = self.sign_up_fields(form, &theme, cx);
        } else {
            form = form
                .child(field(
                    t!("core-signin-username"),
                    Input::new(&self.username)
                        .id("username")
                        .aria_label(t!("core-signin-username")),
                    &theme,
                ))
                .child(field(
                    t!("core-signin-password"),
                    Input::new(&self.password)
                        .id("password")
                        .aria_label(t!("core-signin-password")),
                    &theme,
                ));
        }
        if let Some(error) = error {
            form = form.child(
                div()
                    .id("sign-in-error")
                    .test_support()
                    .role(Role::Alert)
                    .aria_label(SharedString::from(error.clone()))
                    .text_sm()
                    .text_color(theme.danger)
                    .child(error),
            );
        }
        let label = if choosing {
            if busy {
                t!("core-signin-checking")
            } else {
                t!("core-signin-continue")
            }
        } else if signing_up {
            if busy {
                t!("profile-signup-creating")
            } else {
                t!("profile-signup-submit")
            }
        } else if busy {
            t!("core-signin-signing-in")
        } else {
            t!("core-signin-submit")
        };
        let primary = Button::new("submit")
            .primary()
            .label(label)
            .disabled(busy)
            .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx)));
        form = form.child(primary);
        if !choosing {
            let link = |id: &'static str, text: SharedString, theme: &theme::Theme| {
                div()
                    .id(id)
                    .test_support()
                    .role(Role::Link)
                    .reachable(&theme)
                    .aria_label(text.clone())
                    .text_color(theme.link)
                    .cursor_pointer()
                    .child(text)
            };
            let entity = cx.entity();
            let switch: AnyElement = if signing_up {
                div()
                    .flex()
                    .gap_1()
                    .text_color(theme.muted)
                    .child(t!("profile-signup-have-account"))
                    .child(
                        link("sign-in", t!("core-signin-submit").into(), &theme).on_click(
                            move |_, window, cx| {
                                entity.update(cx, |v, cx| v.go(Mode::SignIn, window, cx))
                            },
                        ),
                    )
                    .into_any_element()
            } else {
                match &self.policy {
                    Some(Ok(p)) if !p.registration_enabled => div()
                        .id("registration-closed")
                        .test_support()
                        .role(Role::Note)
                        .aria_label(t!("profile-signup-closed"))
                        .text_color(theme.muted)
                        .child(t!("profile-signup-closed"))
                        .into_any_element(),
                    _ => div()
                        .flex()
                        .gap_1()
                        .text_color(theme.muted)
                        .child(t!("profile-signup-new-here"))
                        .child(
                            link("sign-up", t!("core-signin-create-account").into(), &theme)
                                .on_click(move |_, window, cx| {
                                    entity.update(cx, |v, cx| v.go(Mode::SignUp, window, cx))
                                }),
                        )
                        .into_any_element(),
                }
            };
            let session = self.session.clone();
            form = form.child(
                div()
                    .flex()
                    .justify_between()
                    .gap_2()
                    .text_sm()
                    .child(switch)
                    .child(
                        link(
                            "change-server",
                            t!("core-signin-change-server").into(),
                            &theme,
                        )
                        .on_click(move |_, _, cx| session.update(cx, |s, cx| s.change_server(cx))),
                    ),
            );
        }
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(theme.rail)
            .child(form)
    }
}

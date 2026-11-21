//! The server address screen and the sign-in screen.

use gpui_kit::TestSupportExt;
use gpui_kit::component::Disableable;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::session::{Phase, Session};
use crate::theme::{self, c};

pub struct SignInView {
    session: Entity<Session>,
    address: Entity<InputState>,
    username: Entity<InputState>,
    password: Entity<InputState>,
    _subscriptions: Vec<Subscription>,
}

impl SignInView {
    pub fn new(session: Entity<Session>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let address =
            cx.new(|cx| InputState::new(window, cx).placeholder("https://chat.example.com"));
        let username = cx.new(|cx| InputState::new(window, cx).placeholder("Username"));
        let password = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Password")
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
        let subscriptions = vec![
            enter(&address, window, cx),
            enter(&username, window, cx),
            enter(&password, window, cx),
            cx.observe(&session, |_, _, cx| cx.notify()),
        ];
        Self {
            session,
            address,
            username,
            password,
            _subscriptions: subscriptions,
        }
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
        } else {
            let username = self.username.read(cx).value().to_string();
            let password = self.password.read(cx).value().to_string();
            self.session
                .update(cx, |s, cx| s.sign_in(&username, &password, cx));
        }
    }
}

impl Render for SignInView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (busy, error, choosing, server) = {
            let session = self.session.read(cx);
            (
                session.busy,
                session.error.clone(),
                matches!(session.phase, Phase::ChooseServer),
                session.server().map(|s| s.origin()).unwrap_or_default(),
            )
        };

        let title = if choosing {
            "Connect to IRCord"
        } else {
            "Welcome back"
        };
        let subtitle = if choosing {
            "Enter the address of your IRCord server.".to_string()
        } else {
            format!("Sign in to {server}")
        };

        let mut form = div()
            .id("sign-in-form")
            .w(px(400.))
            .p_8()
            .rounded_lg()
            .bg(c(theme::SIDEBAR))
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .text_2xl()
                    .font_weight(FontWeight::BOLD)
                    .text_color(c(theme::TEXT))
                    .child(title),
            )
            .child(div().text_sm().text_color(c(theme::MUTED)).child(subtitle));

        if choosing {
            form = form.child(field(
                "Server address",
                Input::new(&self.address)
                    .id("address")
                    .aria_label("Server address"),
            ));
        } else {
            form = form
                .child(field(
                    "Username",
                    Input::new(&self.username)
                        .id("username")
                        .aria_label("Username"),
                ))
                .child(field(
                    "Password",
                    Input::new(&self.password)
                        .id("password")
                        .aria_label("Password"),
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
                    .text_color(c(theme::DANGER))
                    .child(error),
            );
        }
        let primary = Button::new("submit")
            .primary()
            .label(if choosing {
                if busy { "Checking…" } else { "Continue" }
            } else if busy {
                "Signing in…"
            } else {
                "Sign in"
            })
            .disabled(busy)
            .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx)));
        form = form.child(primary);
        if !choosing {
            let session = self.session.clone();
            let session2 = self.session.clone();
            form = form.child(
                div()
                    .flex()
                    .justify_between()
                    .text_sm()
                    .child(
                        div()
                            .id("sign-up")
                            .text_color(c(theme::LINK))
                            .cursor_pointer()
                            .child("Create an account")
                            .on_click(move |_, _, cx| {
                                if let Some(server) = session.read(cx).server().map(|s| s.origin())
                                {
                                    cx.open_url(&server);
                                }
                            }),
                    )
                    .child(
                        div()
                            .id("change-server")
                            .text_color(c(theme::LINK))
                            .cursor_pointer()
                            .child("Change server")
                            .on_click(move |_, _, cx| {
                                session2.update(cx, |s, cx| s.change_server(cx))
                            }),
                    ),
            );
        }
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(c(theme::RAIL))
            .child(form)
    }
}

fn field(label: &'static str, input: impl IntoElement) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .text_xs()
                .font_weight(FontWeight::BOLD)
                .text_color(c(theme::MUTED))
                .child(label.to_uppercase()),
        )
        .child(input)
}

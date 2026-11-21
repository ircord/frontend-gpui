//! What shows between signing in and the first bootstrap: the server being
//! reached, or why it cannot be, with a way to sign out.

use gpui_kit::TestSupportExt;
use gpui_kit::prelude::*;
use gpui_kit::*;
use ircord_client::store::Connection;

use crate::session::{Session, SessionEvent};
use crate::theme;

pub struct LoadingView {
    session: Entity<Session>,
    _subscription: Subscription,
}

impl LoadingView {
    pub fn new(session: Entity<Session>, cx: &mut Context<Self>) -> Self {
        let subscription = cx.subscribe(&session, |_, _, event: &SessionEvent, cx| {
            if matches!(event, SessionEvent::Connection | SessionEvent::Phase) {
                cx.notify();
            }
        });
        Self {
            session,
            _subscription: subscription,
        }
    }
}

/// Whether the signed-in client has its first bootstrap yet.
pub fn bootstrapped(session: &Session) -> bool {
    session
        .client()
        .is_some_and(|client| !client.read().user.id.is_empty())
}

impl Render for LoadingView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme::get(cx).clone();
        let session = self.session.read(cx);
        let server = session.server().map(|s| s.origin()).unwrap_or_default();
        let offline = session
            .client()
            .and_then(|client| match &client.read().connection {
                Some(Connection::Offline { reason, retry_ms }) => {
                    Some((reason.clone(), retry_ms.div_ceil(1000)))
                }
                _ => None,
            });
        let status = match offline {
            Some((reason, seconds)) => t!(
                "core-loading-offline",
                server = server,
                reason = reason,
                seconds = seconds
            ),
            None => t!("core-loading-connecting", server = server),
        };
        let session = self.session.clone();
        div()
            .id("loading")
            .test_support()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_4()
            .bg(theme.rail)
            .child(
                div()
                    .id("loading-status")
                    .test_support()
                    .role(Role::Status)
                    .aria_label(SharedString::from(status.clone()))
                    .text_color(theme.muted)
                    .child(status),
            )
            .child(
                div()
                    .id("loading-sign-out")
                    .test_support()
                    .role(Role::Button)
                    .aria_label(t!("core-sign-out"))
                    .text_sm()
                    .text_color(theme.link)
                    .cursor_pointer()
                    .child(t!("core-sign-out"))
                    .on_click(move |_, _, cx| session.update(cx, |s, cx| s.sign_out(cx))),
            )
    }
}

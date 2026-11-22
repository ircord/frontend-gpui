//! Message text: quotes, lists, code, pills, Markdown on send (package T1).
//!
//! The drawing is `message/body.rs`; this module holds what the text does:
//! Markdown typed in the box goes out as IRC formatting, and channel names,
//! `irc://` links and message links open inside the app. A channel you are
//! not in asks before joining, since a typo or an issue reference would
//! create it.

use std::rc::Rc;

use gpui_kit::*;
use ircord_client::api::Method;
use ircord_client::contract::{self as c, ConversationKind};
use ircord_client::format::links::{MessageLink, parse_irc_url};
use ircord_client::format::markdown::markdown_to_irc;
use ircord_client::store::Changes;

use crate::registry::{self, ComposerCtx, SendFilter, SendOutcome};
use crate::session::Session;
use crate::ui;

pub fn init(cx: &mut App) {
    registry::register(cx, |r| {
        r.composer
            .send_filters
            .add(crate::composer::emoticons::send_filter());
        r.composer.send_filters.add(SendFilter {
            id: "text-markdown".into(),
            // After commands (which see what was typed), before length checks.
            order: 200,
            filter: Rc::new(|_: &ComposerCtx, text, _, _| SendOutcome::Continue(wire_text(&text))),
        });
    });
}

/// What a send puts on the wire: Markdown shortcuts as IRC formatting. A
/// command other than `/me` is left as typed; `//` sends a line that starts
/// with `/`.
pub fn wire_text(text: &str) -> String {
    if text.starts_with('/') && !text.starts_with("//") && !text.starts_with("/me ") {
        return text.to_string();
    }
    markdown_to_irc(text)
}

/// The joined channel `name` on a network, if any.
pub fn joined_channel(session: &Session, network: &str, name: &str) -> Option<String> {
    let client = session.client()?;
    let w = client.read();
    let bare = name.trim_start_matches('#');
    w.conversations
        .iter()
        .find(|c| {
            c.network_id == network
                && c.kind == ConversationKind::Channel
                && c.party.is_none()
                && c.name.eq_ignore_ascii_case(bare)
        })
        .map(|c| c.id.clone())
}

/// Open the channel `name` on a network, or ask and join it.
pub fn open_channel(
    session: &Entity<Session>,
    network: &str,
    name: &str,
    window: &mut Window,
    cx: &mut App,
) {
    if let Some(id) = joined_channel(session.read(cx), network, name) {
        session.update(cx, |s, cx| s.select_conversation(&id, cx));
        return;
    }
    let network_name = session
        .read(cx)
        .client()
        .and_then(|c| c.read().network(network).map(|n| n.name.clone()))
        .unwrap_or_default();
    let channel = if name.starts_with(['#', '&', '!', '+']) {
        name.to_string()
    } else {
        format!("#{name}")
    };
    let (session, network) = (session.clone(), network.to_string());
    ui::confirm::confirm(
        window,
        cx,
        ui::confirm::Confirm {
            title: t!(
                "text-join-title",
                channel = channel.clone(),
                network = network_name
            )
            .into(),
            body: t!("text-join-body").into(),
            ok: t!("text-join-ok").into(),
            danger: false,
        },
        move |window, cx| {
            // It may have been joined while the question was open.
            if let Some(id) = joined_channel(session.read(cx), &network, &channel) {
                session.update(cx, |s, cx| s.select_conversation(&id, cx));
                return;
            }
            let body = c::ConversationInputBody {
                network_id: network.clone(),
                name: Some(channel.clone()),
                ..Default::default()
            };
            start(&session, body, channel.clone(), window, cx);
        },
    );
}

/// Create (join or open) a conversation and show it.
fn start(
    session: &Entity<Session>,
    body: c::ConversationInputBody,
    what: String,
    window: &mut Window,
    cx: &mut App,
) {
    let Some(client) = session.read(cx).client().cloned() else {
        return;
    };
    let request = client.request(move |api| async move {
        api.call::<_, c::Conversation>(Method::POST, "/api/v1/conversations", &body)
            .await
    });
    let session = session.clone();
    window
        .spawn(cx, async move |cx| {
            let result = request.await;
            cx.update(|window, cx| match result {
                Ok(Ok(conversation)) => {
                    let id = conversation.id.clone();
                    client.change(move |w| {
                        if w.conversation(&conversation.id).is_some() {
                            return Changes::default();
                        }
                        w.conversations.push(conversation);
                        Changes {
                            sidebar: true,
                            ..Default::default()
                        }
                    });
                    session.update(cx, |s, cx| s.select_conversation(&id, cx));
                }
                Ok(Err(e)) => ui::toast::error(
                    window,
                    cx,
                    t!("text-join-failed", target = what, error = e.to_string()),
                ),
                Err(e) => ui::toast::error(
                    window,
                    cx,
                    t!("text-join-failed", target = what, error = e.to_string()),
                ),
            })
            .ok();
        })
        .detach();
}

/// Open an `irc://` link: a channel or a person on a network you have, by
/// its host.
pub fn open_irc_link(session: &Entity<Session>, url: &str, window: &mut Window, cx: &mut App) {
    let Some(link) = parse_irc_url(url) else {
        ui::toast::error(window, cx, t!("text-irc-invalid"));
        return;
    };
    let network = session.read(cx).client().and_then(|c| {
        c.read()
            .networks
            .iter()
            .find(|n| n.host.eq_ignore_ascii_case(&link.host))
            .map(|n| n.id.clone())
    });
    let Some(network) = network else {
        ui::toast::info(window, cx, t!("text-irc-unknown-network", host = link.host));
        return;
    };
    if let Some(channel) = &link.channel {
        open_channel(session, &network, channel, window, cx);
    } else if let Some(nick) = link.nick {
        let body = c::ConversationInputBody {
            network_id: network,
            nickname: Some(nick.clone()),
            ..Default::default()
        };
        start(session, body, nick, window, cx);
    } else {
        session.update(cx, |s, cx| s.select_network(&network, cx));
    }
}

/// Open the conversation a message link points to.
pub fn open_message_link(
    session: &Entity<Session>,
    link: &MessageLink,
    window: &mut Window,
    cx: &mut App,
) {
    let known = session
        .read(cx)
        .client()
        .is_some_and(|c| c.read().conversation(&link.conversation_id).is_some());
    if known {
        session.update(cx, |s, cx| s.select_conversation(&link.conversation_id, cx));
    } else {
        ui::toast::info(window, cx, t!("text-message-link-missing"));
    }
}

#[cfg(test)]
mod tests {
    use super::wire_text;

    #[test]
    fn markdown_goes_out_as_irc_but_commands_stay() {
        assert_eq!(wire_text("**hi**"), "\x02hi\x02");
        assert_eq!(wire_text("/me *waves*"), "/me \x1dwaves\x1d");
        assert_eq!(wire_text("//**x**"), "//\x02x\x02");
        assert_eq!(wire_text("/topic **x**"), "/topic **x**");
    }
}

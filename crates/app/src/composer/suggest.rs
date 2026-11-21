//! Suggestions while typing (package C1): `/` commands, `@` people and
//! `#` channels, each a provider registered by `features/commands.rs`. The
//! matching itself is `ircord_client::commands::complete`.

use gpui_kit::{App, SharedString};
use ircord_client::commands::catalog::{self, Place};
use ircord_client::commands::complete::{self, TokenKind};
use ircord_client::features::Context as Allowed;
use ircord_client::irc::channel_rights_for;
use ircord_client::store::Connection;

use crate::features::commands::{args_text, description};
use crate::registry::{ComposerCtx, Suggestion};

/// How many people or channels the list shows.
const LIMIT: usize = 12;

fn token(ctx: &ComposerCtx, text: &str, kind: TokenKind, cx: &App) -> Option<complete::Token> {
    let (_, caret) = crate::composer::typed(&ctx.composer, cx);
    complete::token(text, caret).filter(|t| t.kind == kind)
}

/// `/jo` → `/join`, with its arguments and what it does.
pub fn commands(ctx: &ComposerCtx, text: &str, cx: &App) -> Vec<Suggestion> {
    let Some(token) = token(ctx, text, TokenKind::Command, cx) else {
        return Vec::new();
    };
    let session = ctx.session.read(cx);
    let Some(client) = session.client() else {
        return Vec::new();
    };
    let w = client.read();
    let allowed = Allowed::of(
        &w,
        &ctx.conversation,
        w.connection == Some(Connection::Live),
    );
    let place = Place {
        channel: allowed.conversation.is_some_and(|c| {
            c.kind == ircord_client::contract::ConversationKind::Channel && c.party.is_none()
        }),
        operator: channel_rights_for(allowed.own_modes(), allowed.conversation, allowed.network).op,
    };
    catalog::matching(&token.query, place)
        .into_iter()
        .map(|spec| {
            let args = args_text(spec.name);
            let detail = if args.is_empty() {
                description(spec.name).to_string()
            } else {
                format!("{args} \u{2014} {}", description(spec.name))
            };
            Suggestion {
                id: format!("command-{}", spec.name).into(),
                label: format!("/{}", spec.name).into(),
                detail: Some(detail.into()),
                replace: token.range.clone(),
                insert: format!("/{} ", spec.name),
            }
        })
        .collect()
}

/// `@bo` → `@bob`, people in this conversation.
pub fn people(ctx: &ComposerCtx, text: &str, cx: &App) -> Vec<Suggestion> {
    let Some(token) = token(ctx, text, TokenKind::Member, cx) else {
        return Vec::new();
    };
    let session = ctx.session.read(cx);
    let Some(client) = session.client() else {
        return Vec::new();
    };
    let w = client.read();
    let Some(conversation) = w.conversation(&ctx.conversation) else {
        return Vec::new();
    };
    complete::members(&w, conversation, &token.query)
        .into_iter()
        .take(LIMIT)
        .map(|m| Suggestion {
            id: format!("person-{}", m.nick).into(),
            label: format!("@{}", m.nick).into(),
            detail: (!m.display_name.is_empty() && m.display_name != m.nick)
                .then(|| SharedString::from(m.display_name.clone())),
            replace: token.range.clone(),
            insert: format!("@{} ", m.nick),
        })
        .collect()
}

/// `#ge` → `#general`, channels you are in on this network.
pub fn channels(ctx: &ComposerCtx, text: &str, cx: &App) -> Vec<Suggestion> {
    let Some(token) = token(ctx, text, TokenKind::Channel, cx) else {
        return Vec::new();
    };
    let session = ctx.session.read(cx);
    let Some(client) = session.client() else {
        return Vec::new();
    };
    let w = client.read();
    let Some(network) = w
        .conversation(&ctx.conversation)
        .map(|c| c.network_id.clone())
    else {
        return Vec::new();
    };
    complete::channels(&w, &network, &token.query)
        .into_iter()
        .take(LIMIT)
        .map(|c| Suggestion {
            id: format!("channel-{}", c.name).into(),
            label: format!("#{}", c.name).into(),
            detail: (!c.topic.is_empty()).then(|| SharedString::from(c.topic.clone())),
            replace: token.range.clone(),
            insert: format!("#{} ", c.name),
        })
        .collect()
}

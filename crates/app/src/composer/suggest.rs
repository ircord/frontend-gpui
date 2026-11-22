//! Suggestions while typing (package C1): `/` commands, `@` people and
//! `#` channels, each a provider registered by `features/commands.rs`. The
//! matching itself is `ircord_client::commands::complete`.
//!
//! The rows look like the web's: commands under "Commands" with their
//! usage in monospace and what they do below it; people under "Members
//! matching @m" with their picture, their name, and their `@nick` at the
//! right.

use gpui_kit::{App, SharedString};
use ircord_client::commands::catalog;
use ircord_client::commands::complete::{self, TokenKind};
use ircord_client::features::Context as Allowed;
use ircord_client::store::{Connection, people};

use crate::features::commands::{args_text, description, place};
use crate::registry::{ComposerCtx, Suggestion, SuggestionLook};

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
    let place = place(&allowed);
    let heading = SharedString::from(t!("composer-commands"));
    catalog::matching(&token.query, place)
        .into_iter()
        .map(|spec| {
            let args = args_text(spec.name);
            let usage = if args.is_empty() {
                format!("/{}", spec.name)
            } else {
                format!("/{} {args}", spec.name)
            };
            Suggestion {
                id: format!("command-{}", spec.name).into(),
                label: usage.into(),
                detail: Some(description(spec.name).to_string().into()),
                replace: token.range.clone(),
                insert: format!("/{} ", spec.name),
                heading: Some(heading.clone()),
                look: SuggestionLook::Command { name: spec.name },
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
    let heading = SharedString::from(t!("composer-people", query = token.query.as_str()));
    complete::members(&w, conversation, &token.query)
        .into_iter()
        .take(LIMIT)
        .map(|m| Suggestion {
            id: format!("person-{}", m.nick).into(),
            label: people::name(m).to_string().into(),
            detail: Some(format!("@{}", m.nick).into()),
            replace: token.range.clone(),
            insert: format!("@{} ", m.nick),
            heading: Some(heading.clone()),
            look: SuggestionLook::Person {
                member: m.id.clone(),
            },
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
    let heading = SharedString::from(t!("composer-channels", query = token.query.as_str()));
    complete::channels(&w, &network, &token.query)
        .into_iter()
        .take(LIMIT)
        .map(|c| Suggestion {
            id: format!("channel-{}", c.name).into(),
            label: format!("#{}", c.name).into(),
            detail: (!c.topic.is_empty()).then(|| SharedString::from(c.topic.clone())),
            replace: token.range.clone(),
            insert: format!("#{} ", c.name),
            heading: Some(heading.clone()),
            look: SuggestionLook::Inline,
        })
        .collect()
}

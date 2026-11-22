//! Autocomplete while typing, from the web client's
//! `useComposerAutocomplete.ts`: `/` commands, `@` people and `#` channels,
//! and IRC-style Tab completion of nicks, which adds the `@` that makes a
//! nick a mention. Emoji (`:name`) belong to package E1.

use std::collections::HashSet;
use std::ops::Range;

use ircord_contract::{Conversation, ConversationKind, Member, MemberStatus};

use super::catalog::{self, FieldKind, Suggest};
use crate::store::Workspace;

/// What the text before the caret is completing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenKind {
    Command,
    Member,
    Channel,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    pub kind: TokenKind,
    /// What is typed after the `/`, `@` or `#`.
    pub query: String,
    /// The bytes to replace, sigil included.
    pub range: Range<usize>,
}

/// The token ending at `caret` (a byte offset into `text`), if any.
pub fn token(text: &str, caret: usize) -> Option<Token> {
    let caret = caret.min(text.len());
    if !text.is_char_boundary(caret) {
        return None;
    }
    let before = &text[..caret];
    if let Some(name) = before.strip_prefix('/')
        && name.chars().all(|c| c.is_alphanumeric() || c == '_')
    {
        return Some(Token {
            kind: TokenKind::Command,
            query: name.to_string(),
            range: 0..caret,
        });
    }
    // The word before the caret, starting after whitespace or at the start.
    let start = before
        .char_indices()
        .rev()
        .find(|(_, c)| c.is_whitespace())
        .map_or(0, |(i, c)| i + c.len_utf8());
    let word = &before[start..];
    let (kind, query) = if let Some(q) = word.strip_prefix('@') {
        // `/unignore @ju` completes from the ignore list with Tab instead.
        if is_unignore(before) || q.contains('@') || q.chars().count() > 100 {
            return None;
        }
        (TokenKind::Member, q)
    } else {
        let q = word.strip_prefix('#')?;
        if q.contains([',', '#', ':']) || q.chars().count() > 49 {
            return None;
        }
        (TokenKind::Channel, q)
    };
    Some(Token {
        kind,
        query: query.to_string(),
        range: start..caret,
    })
}

fn is_unignore(before: &str) -> bool {
    before
        .get(..10)
        .is_some_and(|s| s.eq_ignore_ascii_case("/unignore "))
        && before[10..].split_whitespace().count() <= 1
        && !before.ends_with(char::is_whitespace)
}

/// Members of `conversation` matching `query` by nick or display name,
/// yourself left out: prefix matches first, then online before away, then
/// by nick.
pub fn members<'a>(w: &'a Workspace, conversation: &Conversation, query: &str) -> Vec<&'a Member> {
    rank(
        w,
        &conversation.network_id,
        conversation.member_ids.iter(),
        query,
    )
}

/// The members of `ids` on `network` matching `query`, ranked as
/// [`members`] ranks them.
fn rank<'a, 'b>(
    w: &'a Workspace,
    network: &str,
    ids: impl Iterator<Item = &'b String>,
    query: &str,
) -> Vec<&'a Member> {
    let q = query.to_lowercase();
    let mut found: Vec<(u8, u8, &Member)> = ids
        .filter(|id| !w.is_self(id, network))
        .filter_map(|id| w.member(id, network))
        .filter_map(|m| {
            let nick = m.nick.to_lowercase();
            let name = m.display_name.to_lowercase();
            let rank = if nick.starts_with(&q) {
                0
            } else if !name.is_empty() && name.starts_with(&q) {
                1
            } else if nick.contains(&q) || (!name.is_empty() && name.contains(&q)) {
                2
            } else {
                return None;
            };
            let presence = match m.status {
                MemberStatus::Online => 0,
                MemberStatus::Away => 1,
                _ => 2,
            };
            Some((rank, presence, m))
        })
        .collect();
    found.sort_by(|a, b| {
        (a.0, a.1)
            .cmp(&(b.0, b.1))
            .then_with(|| a.2.nick.to_lowercase().cmp(&b.2.nick.to_lowercase()))
    });
    found.into_iter().map(|(_, _, m)| m).collect()
}

/// Everyone on `network` (the web's `networkPeople`) matching `query`, but
/// not the members of `outside` when given: the people an invitation to
/// that channel could still reach.
pub fn network_members<'a>(
    w: &'a Workspace,
    network: &str,
    outside: Option<&Conversation>,
    query: &str,
) -> Vec<&'a Member> {
    let left_out: &[String] = match outside {
        Some(c) => &c.member_ids,
        None => &[],
    };
    let prefix = format!("{network}:");
    let mut seen = HashSet::new();
    let ids = w
        .conversations
        .iter()
        .filter(|c| c.network_id == network)
        .flat_map(|c| c.member_ids.iter())
        .chain(w.members.keys().filter(|id| id.starts_with(&prefix)))
        .filter(|id| !left_out.contains(*id) && seen.insert(id.as_str()));
    rank(w, network, ids, query)
}

/// The channel `name` on `network`, with or without `#` and in any case: the
/// target of an invitation typed after the nickname.
pub fn channel_named<'a>(w: &'a Workspace, network: &str, name: &str) -> Option<&'a Conversation> {
    let name = name.trim_start_matches('#');
    w.conversations.iter().find(|c| {
        c.network_id == network
            && c.kind == ConversationKind::Channel
            && c.party.is_none()
            && c.name.trim_start_matches('#').eq_ignore_ascii_case(name)
    })
}

/// A nickname argument being typed: the field it fills and the word.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Argument {
    /// Where its suggestions come from: `Nickname` or `Invitee`.
    pub suggest: Suggest,
    /// What is typed in the word.
    pub query: String,
    /// The bytes of the word, to replace with a nick.
    pub range: Range<usize>,
    /// For `/invite`: the channel typed after the nick, without `#`.
    pub channel: Option<String>,
}

/// The nickname argument of a slash command that the caret is in. A field
/// takes one word and the last field takes the rest of the line, so the
/// field is found by the word's position. Nothing for an empty word, or for
/// the command's own word.
pub fn argument(text: &str, caret: usize) -> Option<Argument> {
    let caret = caret.min(text.len());
    if !text.is_char_boundary(caret) || !text.starts_with('/') {
        return None;
    }
    let before = &text[..caret];
    let start = before
        .char_indices()
        .rev()
        .find(|(_, c)| c.is_whitespace())
        .map_or(0, |(i, c)| i + c.len_utf8());
    let query = &before[start..];
    if start == 0 || query.is_empty() {
        return None;
    }
    let words: Vec<&str> = text[..start].split_whitespace().collect();
    let (command, args) = words.split_first()?;
    let spec = catalog::resolve(command)?;
    let index = args.len().min(spec.fields.len().checked_sub(1)?);
    let suggest = spec.fields[index]
        .suggest
        .filter(|s| matches!(s, Suggest::Nickname | Suggest::Invitee))?;
    // The channel word sits after the nick, past the one under the caret.
    let channel = spec
        .fields
        .get(index + 1)
        .is_some_and(|f| f.kind == FieldKind::Channel)
        .then(|| text.split_whitespace().nth(args.len() + 2))
        .flatten()
        .map(|word| word.trim_start_matches('#').to_string());
    Some(Argument {
        suggest,
        query: query.to_string(),
        range: start..caret,
        channel,
    })
}

/// Channels you are in on `network` whose names contain `query`: prefix
/// matches first, then those with mentions, then unread, then by name.
pub fn channels<'a>(w: &'a Workspace, network: &str, query: &str) -> Vec<&'a Conversation> {
    let q = query.to_lowercase();
    let mut found: Vec<&Conversation> = w
        .conversations
        .iter()
        .filter(|c| {
            c.network_id == network
                && c.kind == ConversationKind::Channel
                && c.party.is_none()
                && c.name.to_lowercase().contains(&q)
        })
        .collect();
    found.sort_by_key(|c| {
        (
            !c.name.to_lowercase().starts_with(&q),
            c.mention_count == 0,
            c.unread_count == 0,
            c.name.to_lowercase(),
        )
    });
    found
}

/// Where repeated Tab presses cycle through the nicks one word matched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TabCycle {
    pub start: usize,
    pub matches: Vec<String>,
    pub index: usize,
    /// The caret after the last completion; Tab there cycles on.
    pub end: usize,
}

/// One Tab press: the new text and caret, and the cycle to keep for the
/// next press. `ju⇥` becomes `@jules: ` at the start of the box and
/// `@jules ` elsewhere; in a command (`/msg ju⇥`) the nick stays bare.
/// `candidates` gives the nicks a word matches, best first.
pub fn tab(
    text: &str,
    caret: usize,
    cycle: Option<TabCycle>,
    candidates: impl FnOnce(&str) -> Vec<String>,
) -> Option<(String, usize, TabCycle)> {
    let caret = caret.min(text.len());
    if !text.is_char_boundary(caret) {
        return None;
    }
    let before = &text[..caret];
    let mut cycle = match cycle {
        Some(mut c) if c.end == caret && !c.matches.is_empty() => {
            c.index = (c.index + 1) % c.matches.len();
            c
        }
        _ => {
            let start = before
                .char_indices()
                .rev()
                .find(|(_, c)| c.is_whitespace())
                .map_or(0, |(i, c)| i + c.len_utf8());
            let word = &before[start..];
            if word.is_empty() {
                return None;
            }
            let matches = candidates(word.strip_prefix('@').unwrap_or(word));
            if matches.is_empty() {
                return None;
            }
            TabCycle {
                start,
                matches,
                index: 0,
                end: 0,
            }
        }
    };
    let nick = &cycle.matches[cycle.index];
    let mention = if text.starts_with('/') {
        nick.clone()
    } else {
        format!("@{nick}")
    };
    let insert = if cycle.start == 0 {
        format!("{mention}: ")
    } else {
        format!("{mention} ")
    };
    let next = format!("{}{insert}{}", &text[..cycle.start], &text[caret..]);
    cycle.end = cycle.start + insert.len();
    Some((next, cycle.end, cycle))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::SnapshotBuilder;

    #[test]
    fn finds_the_token_before_the_caret() {
        let t = token("/jo", 3).unwrap();
        assert_eq!(
            (t.kind, t.query.as_str(), t.range),
            (TokenKind::Command, "jo", 0..3)
        );
        let t = token("hi @bo there", 6).unwrap();
        assert_eq!(
            (t.kind, t.query.as_str(), t.range),
            (TokenKind::Member, "bo", 3..6)
        );
        let t = token("see #ge", 7).unwrap();
        assert_eq!((t.kind, t.query.as_str()), (TokenKind::Channel, "ge"));
        assert_eq!(token("/join #a", 8).unwrap().kind, TokenKind::Channel);
        assert!(token("plain words", 11).is_none());
        assert!(token("/join ", 6).is_none());
        assert!(token("a@b", 3).is_none());
        assert!(token("/unignore @ju", 13).is_none());
    }

    #[test]
    fn people_and_channels_rank_like_the_web_client() {
        let w = SnapshotBuilder::new()
            .member("m-bo", "bo")
            .member("m-abob", "abob")
            .member("m-bob", "bob")
            .channel("c-a", "alpha")
            .channel("c-b", "beta")
            .with_conversation(|c| c.unread_count = 2)
            .channel("c-ab", "alphabet")
            .with_conversation(|c| c.mention_count = 1)
            .in_conversation("c-a", "m-bo", "")
            .in_conversation("c-a", "m-abob", "")
            .in_conversation("c-a", "m-bob", "")
            .workspace();
        let c = w.conversation("c-a").unwrap();
        let nicks: Vec<_> = members(&w, c, "bo")
            .iter()
            .map(|m| m.nick.as_str())
            .collect();
        assert_eq!(nicks, ["bo", "bob", "abob"]);
        assert!(members(&w, c, "me").is_empty(), "you are left out");
        let names: Vec<_> = channels(&w, "n1", "a")
            .iter()
            .map(|c| c.name.as_str())
            .collect();
        assert_eq!(names, ["alphabet", "alpha", "beta"]);
    }

    #[test]
    fn nickname_arguments_find_their_field_by_position() {
        let a = argument("/kick bo", 8).unwrap();
        assert_eq!(
            (a.suggest, a.query.as_str(), a.range, a.channel),
            (Suggest::Nickname, "bo", 6..8, None)
        );
        let a = argument("/w bo", 5).unwrap();
        assert_eq!((a.suggest, a.range), (Suggest::Nickname, 3..5), "an alias");
        assert!(
            argument("/kick ", 6).is_none(),
            "an empty word offers nothing"
        );
        assert!(argument("/kick", 5).is_none(), "the command's own word");
        assert!(argument("hi /kick bo", 11).is_none(), "not at the start");
        assert!(
            argument("/kick bob why", 14).is_none(),
            "the reason takes no suggestions"
        );
        assert!(argument("/join bo", 8).is_none(), "no nickname field");
    }

    #[test]
    fn invite_reads_the_channel_after_the_nick() {
        let a = argument("/invite bo #general", 10).unwrap();
        assert_eq!(
            (a.suggest, a.query.as_str(), a.range, a.channel.as_deref()),
            (Suggest::Invitee, "bo", 8..10, Some("general"))
        );
        let a = argument("/invite bo", 10).unwrap();
        assert_eq!(a.channel, None, "no channel typed: the one you are in");
        assert!(
            argument("/invite bo #general", 19).is_none(),
            "the channel field takes no nickname"
        );
    }

    #[test]
    fn invitees_are_people_on_the_network_outside_the_channel() {
        let w = SnapshotBuilder::new()
            .member("m-bo", "bo")
            .member("m-bob", "bob")
            .member("m-cy", "cy")
            .channel("c-a", "alpha")
            .channel("c-b", "beta")
            .in_conversation("c-a", "m-bo", "")
            .in_conversation("c-b", "m-cy", "")
            .in_conversation("c-b", "m-bob", "")
            .workspace();
        let alpha = w.conversation("c-a").unwrap();
        let nicks = |found: Vec<&Member>| found.iter().map(|m| m.nick.clone()).collect::<Vec<_>>();
        assert_eq!(
            nicks(network_members(&w, "n1", Some(alpha), "")),
            ["bob", "cy"],
            "bo is already in alpha"
        );
        assert_eq!(
            nicks(network_members(&w, "n1", None, "")),
            ["bo", "bob", "cy"],
            "no channel: nobody is left out"
        );
        assert_eq!(
            channel_named(&w, "n1", "#ALPHA").map(|c| c.id.as_str()),
            Some("c-a")
        );
        assert!(channel_named(&w, "n1", "gamma").is_none());
    }

    #[test]
    fn tab_adds_the_at_and_cycles() {
        let nicks = |_: &str| vec!["jules".to_string(), "julia".to_string()];
        let (text, caret, cycle) = tab("ju", 2, None, nicks).unwrap();
        assert_eq!((text.as_str(), caret), ("@jules: ", 8));
        let (text, caret, _) = tab(&text, caret, Some(cycle), nicks).unwrap();
        assert_eq!((text.as_str(), caret), ("@julia: ", 8));
        let (text, _, _) = tab("hi ju", 5, None, nicks).unwrap();
        assert_eq!(text, "hi @jules ");
        let (text, _, _) = tab("/msg ju", 7, None, nicks).unwrap();
        assert_eq!(text, "/msg jules ");
        assert!(tab("hi ", 3, None, nicks).is_none());
        assert!(tab("zz", 2, None, |_| Vec::new()).is_none());
    }
}

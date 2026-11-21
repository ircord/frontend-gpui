//! Autocomplete while typing, from the web client's
//! `useComposerAutocomplete.ts`: `/` commands, `@` people and `#` channels,
//! and IRC-style Tab completion of nicks, which adds the `@` that makes a
//! nick a mention. Emoji (`:name`) belong to package E1.

use std::ops::Range;

use ircord_contract::{Conversation, ConversationKind, Member, MemberStatus};

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
    let q = query.to_lowercase();
    let mut found: Vec<(u8, u8, &Member)> = conversation
        .member_ids
        .iter()
        .filter(|id| !w.is_self(id, &conversation.network_id))
        .filter_map(|id| w.member(id, &conversation.network_id))
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

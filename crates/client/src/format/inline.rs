//! The pieces of a line of message text that become interactive: links,
//! `@` mentions and `#channels`, as the web client's `MessageRuns.vue` finds
//! them. Text inside monospace stays literal.
//!
//! Whether an `@word` is a pill depends on who is in the conversation, so
//! this only finds the candidates; the app keeps those naming a member or a
//! group ([`super::mentions::group_mention`]).

use std::ops::Range;

use super::Formatted;
use super::mentions::is_nick_char;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Token {
    /// A link and the URL it opens.
    Link(String),
    /// `@word`: the word, without the `@`.
    Mention(String),
    /// `#name`, with the `#`.
    Channel(String),
}

/// Characters that may come right before an `@` mention.
const MENTION_OPENERS: &str = "([{<\"'“‘«*,;";

/// Every token in a line, in order, without overlaps.
pub fn tokens(f: &Formatted) -> Vec<(Range<usize>, Token)> {
    let literal: Vec<Range<usize>> = f
        .runs
        .iter()
        .filter(|(_, s)| s.mono)
        .map(|(r, _)| r.clone())
        .collect();
    let overlaps = |r: &Range<usize>, others: &[Range<usize>]| {
        others.iter().any(|o| r.start < o.end && o.start < r.end)
    };
    let mut out: Vec<(Range<usize>, Token)> = f
        .links
        .iter()
        .filter(|(r, _)| !overlaps(r, &literal))
        .map(|(r, url)| (r.clone(), Token::Link(url.clone())))
        .collect();
    let taken: Vec<Range<usize>> = f
        .links
        .iter()
        .map(|(r, _)| r.clone())
        .chain(literal.iter().cloned())
        .collect();
    let text = &f.text;
    for (at, c) in text.char_indices() {
        let before = text[..at].chars().next_back();
        let found = match c {
            '@' if before.is_none_or(|b| b.is_whitespace() || MENTION_OPENERS.contains(b)) => {
                mention_at(text, at)
            }
            '#' if before.is_none_or(|b| b.is_whitespace() || b == '(') => channel_at(text, at),
            _ => None,
        };
        if let Some((range, token)) = found
            && !overlaps(&range, &taken)
        {
            out.push((range, token));
        }
    }
    out.sort_by_key(|(r, _)| r.start);
    out
}

fn mention_at(text: &str, at: usize) -> Option<(Range<usize>, Token)> {
    let rest = &text[at + 1..];
    let len: usize = rest
        .chars()
        .take_while(|c| is_nick_char(*c))
        .map(char::len_utf8)
        .sum();
    (len > 0).then(|| (at..at + 1 + len, Token::Mention(rest[..len].to_string())))
}

/// `#[^\s,#:]*[^\s,#:.!?)'"]`, and not only digits (an issue number).
fn channel_at(text: &str, at: usize) -> Option<(Range<usize>, Token)> {
    let rest = &text[at + 1..];
    let run = rest
        .find(|c: char| c.is_whitespace() || ",#:".contains(c))
        .unwrap_or(rest.len());
    let name = rest[..run].trim_end_matches(['.', '!', '?', ')', '\'', '"']);
    if name.is_empty() || name.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let end = at + 1 + name.len();
    Some((at..end, Token::Channel(text[at..end].to_string())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::parse;

    fn found(text: &str) -> Vec<(String, Token)> {
        let f = parse(text);
        tokens(&f)
            .into_iter()
            .map(|(r, t)| (f.text[r].to_string(), t))
            .collect()
    }

    #[test]
    fn mentions_channels_and_links() {
        assert_eq!(
            found("hi @alice, see #dev. and https://x.test/@bob (#rust)"),
            [
                ("@alice".into(), Token::Mention("alice".into())),
                ("#dev".into(), Token::Channel("#dev".into())),
                (
                    "https://x.test/@bob".into(),
                    Token::Link("https://x.test/@bob".into())
                ),
                ("#rust".into(), Token::Channel("#rust".into())),
            ]
        );
    }

    #[test]
    fn not_tokens() {
        // Addresses, issue numbers, words with # inside, and monospace.
        assert!(found("mail bob@alice.example").is_empty());
        assert!(found("fixes #123 and a#b").is_empty());
        assert!(found("\x11@alice #dev\x11").is_empty());
        assert_eq!(
            found("irc://irc.libera.chat/#rust"),
            [(
                "irc://irc.libera.chat/#rust".into(),
                Token::Link("irc://irc.libera.chat/#rust".into())
            )]
        );
    }
}

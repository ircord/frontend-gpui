//! The pieces of a line of message text that become interactive: links,
//! `@` mentions and `#channels`, as the web client's `MessageRuns.vue` finds
//! them. Text inside monospace stays literal.
//!
//! Whether an `@word` is a pill depends on who is in the conversation, so
//! this only finds the candidates; the app keeps those naming a member or a
//! group ([`super::mentions::group_mention`]).
//!
//! Masked links, `[text](url)`, are found per paragraph ([`masked_links`]):
//! inside fenced code they stay as typed, so the blocks come first.

use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::ops::Range;

use url::Url;

use super::Formatted;
use super::blocks::Mapped;
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

// ---- Masked links ---------------------------------------------------------

/// A paragraph with its masked links, `[text](url)`, drawn as their words.
#[derive(Debug, Clone, PartialEq)]
pub struct Masked {
    /// The paragraph without the brackets and addresses of its masked links.
    pub text: Mapped,
    /// Its formatting and links; a masked link's words open its address.
    pub formatted: Formatted,
    /// Where the masked links' words are in `text`.
    pub masked: Vec<Range<usize>>,
}

/// Finds `[text](url)` in a paragraph and draws it as `text` opening `url`.
///
/// Only absolute `http`, `https` and `mailto` addresses are masked, never
/// inside monospace, and the link opens the address in its canonical form.
/// When a word of the text names an address or a domain other than the
/// link's host, or the address carries user info (`https://bank@evil`), the
/// link is not masked: the typed text stays and its address is an ordinary
/// link. The message itself is unchanged, so plain IRC readers see
/// `[text](url)`. `text` may hold formatting, nested brackets and `\]`; the
/// address may hold balanced parentheses; a space between `]` and `(` makes
/// no link. The cases both clients share are in `testdata/masked-links.json`.
/// Linear in the paragraph.
pub fn masked_links(text: &Mapped, source: &Formatted) -> Masked {
    let plain = text.formatted(source);
    if !plain.text.contains("](") {
        return Masked {
            text: text.clone(),
            formatted: plain,
            masked: Vec::new(),
        };
    }
    let s = plain.text.as_str();
    let b = s.as_bytes();
    let mono: Vec<Range<usize>> = plain
        .runs
        .iter()
        .filter(|(_, st)| st.mono)
        .map(|(r, _)| r.clone())
        .collect();
    let brackets = pairs(b, b'[', b']', true, |c| c == b'\n');
    let parens = pairs(b, b'(', b')', false, |c| c.is_ascii_whitespace());
    let mut keep = vec![true; s.len()];
    let mut found: Vec<Found> = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let open = i;
        i += 1;
        if b[open] != b'[' {
            continue;
        }
        let Some(&close) = brackets.get(&open) else {
            continue;
        };
        if b.get(close + 1) != Some(&b'(') {
            continue;
        }
        let Some(&end_paren) = parens.get(&(close + 1)) else {
            continue;
        };
        let address = close + 2..end_paren;
        let end = end_paren + 1;
        if address.is_empty() || mono.iter().any(|m| m.start < end && open < m.end) {
            continue;
        }
        let Some(target) = Target::parse(&s[address.clone()]) else {
            continue;
        };
        let shown = unescape(&s[open + 1..close]);
        if shown.trim().is_empty() {
            continue;
        }
        if target.user_info || names_other_host(&shown, &target.host) {
            // Not masked; a link inside its text may still be.
            found.push(Found {
                words: None,
                address,
                url: target.url,
            });
            continue;
        }
        keep[open] = false;
        keep[close..end].iter_mut().for_each(|k| *k = false);
        let mut j = open + 1;
        while j < close {
            if b[j] == b'\\' && matches!(b.get(j + 1), Some(b'[' | b']' | b'\\')) {
                keep[j] = false;
                j += 1;
            }
            j += 1;
        }
        found.push(Found {
            words: Some(open..end),
            address,
            url: target.url,
        });
        i = end;
    }
    if found.is_empty() {
        return Masked {
            text: text.clone(),
            formatted: plain,
            masked: Vec::new(),
        };
    }
    // Each byte's place in the masked text.
    let mut place = Vec::with_capacity(s.len() + 1);
    let mut kept = Mapped::default();
    for (k, &byte_kept) in keep.iter().enumerate() {
        place.push(kept.at.len());
        if byte_kept {
            kept.at.push(text.at[k]);
        }
    }
    place.push(kept.at.len());
    kept.text = s
        .char_indices()
        .filter(|(k, _)| keep[*k])
        .map(|(_, c)| c)
        .collect();
    let moved = |r: &Range<usize>| place[r.start]..place[r.end];
    let mut formatted = kept.formatted(source);
    // Links found on the source that a masked link, or an unmasked one's
    // address, replaces.
    let replaced: Vec<Range<usize>> = found
        .iter()
        .map(|f| moved(f.words.as_ref().unwrap_or(&f.address)))
        .collect();
    formatted
        .links
        .retain(|(r, _)| !replaced.iter().any(|s| r.start < s.end && s.start < r.end));
    let mut masked = Vec::new();
    for (f, range) in found.into_iter().zip(replaced) {
        if f.words.is_some() {
            masked.push(range.clone());
        }
        formatted.links.push((range, f.url));
    }
    formatted.links.sort_by_key(|(r, _)| r.start);
    Masked {
        text: kept,
        formatted,
        masked,
    }
}

/// A `[words](address)` found in a paragraph.
struct Found {
    /// The whole link, when it is masked.
    words: Option<Range<usize>>,
    address: Range<usize>,
    /// The address in its canonical form.
    url: String,
}

/// Where each `open` byte's matching `close` is, as a depth count from it
/// would find it, in one pass: a stack, emptied at every `stop` byte. With
/// `escapes`, a backslash takes the byte after it out of the count.
fn pairs(
    b: &[u8],
    open: u8,
    close: u8,
    escapes: bool,
    stop: impl Fn(u8) -> bool,
) -> HashMap<usize, usize> {
    let mut out = HashMap::new();
    let mut stack = Vec::new();
    let mut j = 0;
    while j < b.len() {
        let c = b[j];
        if escapes && c == b'\\' {
            j += 2;
            continue;
        }
        if stop(c) {
            stack.clear();
        } else if c == open {
            stack.push(j);
        } else if c == close
            && let Some(o) = stack.pop()
        {
            out.insert(o, j);
        }
        j += 1;
    }
    out
}

fn unescape(words: &str) -> String {
    let mut out = String::with_capacity(words.len());
    let mut chars = words.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\'
            && let Some(&next) = chars.peek()
            && matches!(next, '[' | ']' | '\\')
        {
            continue;
        }
        out.push(c);
    }
    out
}

/// An address a masked link may open: an absolute `http` or `https` URL, or
/// a `mailto` address.
struct Target {
    /// The canonical URL, which also percent-encodes controls.
    url: String,
    /// The URL's host, or the mail domain, as [`bare_host`] writes it.
    host: String,
    /// Whether the URL names a user (`https://bank.test@evil.test`).
    user_info: bool,
}

impl Target {
    fn parse(url: &str) -> Option<Target> {
        let parsed = Url::parse(url).ok()?;
        let (host, user_info) = match parsed.scheme() {
            "http" | "https" => (
                bare_host(parsed.host_str()?),
                !parsed.username().is_empty() || parsed.password().is_some(),
            ),
            "mailto" => (mail_domain(parsed.path())?, false),
            _ => return None,
        };
        Some(Target {
            url: parsed.as_str().to_string(),
            host,
            user_info,
        })
    }
}

/// The domain of `user@domain`.
fn mail_domain(address: &str) -> Option<String> {
    let (user, domain) = address.rsplit_once('@')?;
    if user.is_empty() {
        return None;
    }
    normal_host(domain)
}

/// A host name as `Url` writes it (lowercase, IDNs in punycode, look-alike
/// dots as dots), without one trailing dot.
fn normal_host(host: &str) -> Option<String> {
    Url::parse(&format!("http://{host}/"))
        .ok()?
        .host_str()
        .map(bare_host)
}

fn bare_host(host: &str) -> String {
    let host = host.to_lowercase();
    host.strip_suffix('.').map(str::to_string).unwrap_or(host)
}

/// Whether any word of a link's text names an address or a domain whose
/// host is not `host`. Invisible characters are dropped and look-alike dots
/// and fullwidth letters read as their plain forms first; words split at any
/// character an address cannot hold.
fn names_other_host(words: &str, host: &str) -> bool {
    let plain: String = words
        .chars()
        .filter(|&c| !invisible(c))
        .map(|c| match c {
            '\u{3002}' | '\u{ff0e}' | '\u{ff61}' => '.',
            '\u{ff01}'..='\u{ff5e}' => char::from_u32(c as u32 - 0xfee0).unwrap_or(c),
            _ => c,
        })
        .collect();
    plain
        .split(|c: char| !(c.is_alphanumeric() || ".-@:/_".contains(c)))
        .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()))
        .filter(|w| !w.is_empty())
        .filter_map(word_host)
        .any(|h| h != host)
}

/// Characters that draw nothing: zero-width spaces and joiners, direction
/// controls, the soft hyphen, variation selectors and the byte-order mark.
fn invisible(c: char) -> bool {
    matches!(
        c,
        '\u{ad}'
            | '\u{34f}'
            | '\u{61c}'
            | '\u{180e}'
            | '\u{200b}'..='\u{200f}'
            | '\u{202a}'..='\u{202e}'
            | '\u{2060}'..='\u{206f}'
            | '\u{fe00}'..='\u{fe0f}'
            | '\u{feff}'
    ) || c.is_control()
}

/// The host a word names: `https://a.example/x` and `mailto:b@a.example`
/// (an address of any scheme: an empty host when it names none), bare
/// `a.example`, `a.example/x` and `b@a.example`, and an IPv4 address.
/// `None` for other words.
fn word_host(word: &str) -> Option<String> {
    let lower = word.to_ascii_lowercase();
    if lower.contains("://") || lower.starts_with("mailto:") {
        let host = Url::parse(word).ok().and_then(|u| match u.scheme() {
            "mailto" => mail_domain(u.path()),
            _ => u.host_str().map(bare_host),
        });
        return Some(host.unwrap_or_default());
    }
    let name = word.rsplit_once('@').map_or(word, |(_, d)| d);
    let name = &name[..name.find(['/', ':', '?', '#']).unwrap_or(name.len())];
    if name.parse::<Ipv4Addr>().is_ok() {
        return Some(name.to_string());
    }
    let labels: Vec<&str> = name.split('.').collect();
    let dotted = labels.len() >= 2
        && labels
            .iter()
            .all(|l| !l.is_empty() && l.chars().all(|c| c.is_alphanumeric() || c == '-'))
        && labels
            .last()
            .is_some_and(|tld| tld.chars().count() >= 2 && tld.chars().all(char::is_alphabetic));
    if !dotted {
        return None;
    }
    Some(normal_host(name).unwrap_or_default())
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

    /// What a message shows and the links a click opens, as the app draws
    /// it: its blocks, then masked links in each paragraph.
    fn masked(text: &str) -> (String, Vec<(String, String)>, Masked) {
        use crate::format::blocks::{Block, message_blocks};
        let source = parse(text);
        let blocks = message_blocks(&Mapped::new(&source.text), false);
        let paragraph = match blocks.as_slice() {
            [Block::Text(t)] => t.clone(),
            [Block::Code { code, .. }] => {
                let m = Masked {
                    text: code.clone(),
                    formatted: Formatted {
                        text: code.text.clone(),
                        ..Default::default()
                    },
                    masked: Vec::new(),
                };
                return (code.text.clone(), Vec::new(), m);
            }
            other => panic!("{other:?}"),
        };
        let m = masked_links(&paragraph, &source);
        let links = tokens(&m.formatted)
            .into_iter()
            .filter_map(|(r, t)| match t {
                Token::Link(url) => Some((m.formatted.text[r].to_string(), url)),
                _ => None,
            })
            .collect();
        (m.formatted.text.clone(), links, m)
    }

    /// The masked-link cases, the same file in both clients.
    #[test]
    fn masked_link_cases() {
        #[derive(serde::Deserialize)]
        struct Case {
            name: String,
            input: String,
            visible: String,
            targets: Vec<String>,
            ordinary: Option<String>,
        }
        let cases: Vec<Case> =
            serde_json::from_str(include_str!("testdata/masked-links.json")).unwrap();
        assert!(cases.len() > 50);
        for case in cases {
            let (visible, links, m) = masked(&case.input);
            assert_eq!(visible, case.visible, "{}", case.name);
            let shown = |(words, url): &(String, String)| {
                m.masked
                    .iter()
                    .any(|r| m.formatted.text[r.clone()] == *words)
                    .then(|| url.clone())
            };
            let targets: Vec<String> = links.iter().filter_map(shown).collect();
            assert_eq!(targets, case.targets, "{}", case.name);
            if let Some(url) = &case.ordinary {
                // The typed address is an ordinary link to the real one.
                assert!(
                    links
                        .iter()
                        .any(|link| link.1 == *url && shown(link).is_none()),
                    "{}: {links:?}",
                    case.name
                );
            }
        }
    }

    #[test]
    fn masked_links_keep_formatting_and_sources() {
        let (visible, _, m) = masked("go [\x02bold\x02 docs](https://example.com) now");
        assert_eq!(visible, "go bold docs now");
        assert_eq!(m.masked, vec![3..12]);
        let bold = m.formatted.runs.iter().find(|(_, s)| s.bold).unwrap();
        assert_eq!(bold.0, 3..7);
        // " now" still comes from the end of the source text.
        assert_eq!(
            m.text.at.last(),
            Some(
                &(parse("go [\x02bold\x02 docs](https://example.com) now")
                    .text
                    .len()
                    - 1)
            )
        );
        assert_eq!(m.text.project(&(0..2)), Some(0..2));
    }

    #[test]
    fn masked_links_in_list_items_and_not_in_code() {
        use crate::format::blocks::{Block, message_blocks};
        let source = parse("- [docs](https://example.com)\n```\n[a](https://example.com)\n```");
        let blocks = message_blocks(&Mapped::new(&source.text), false);
        let Block::List(list) = &blocks[0] else {
            panic!("{blocks:?}")
        };
        let item = masked_links(&list.items[0].text, &source);
        assert_eq!(item.formatted.text, "docs");
        assert_eq!(
            item.formatted.links,
            vec![(0..4, "https://example.com/".to_string())]
        );
        // The words keep their places in the message: "docs" is bytes 3..7.
        assert_eq!(item.text.at, vec![3, 4, 5, 6]);
        let Block::Code { code, .. } = &blocks[1] else {
            panic!("{blocks:?}")
        };
        assert_eq!(code.text, "[a](https://example.com)");
    }

    /// The box sends `[text](url)` as typed, with emphasis inside as IRC codes,
    /// so plain IRC readers see the brackets and the address.
    #[test]
    fn masked_links_travel_as_typed() {
        use crate::format::markdown::markdown_to_irc;
        assert_eq!(
            markdown_to_irc("[**the** docs](https://example.com/a_b_c)"),
            "[\x02the\x02 docs](https://example.com/a_b_c)"
        );
    }

    /// A table cell is a paragraph of its own, so a masked link works there.
    #[test]
    fn masked_links_in_table_cells() {
        use crate::format::blocks::{Block, message_blocks};
        let source =
            parse("| site | note |\n| --- | --- |\n| [docs](https://example.com/docs) | x |");
        let blocks = message_blocks(&Mapped::new(&source.text), false);
        let Block::Table(table) = &blocks[0] else {
            panic!("{blocks:?}")
        };
        let cell = masked_links(&table.rows[0][0], &source);
        assert_eq!(cell.formatted.text, "docs");
        assert_eq!(cell.masked, vec![0..4]);
        assert_eq!(cell.formatted.links[0].1, "https://example.com/docs");
    }
}

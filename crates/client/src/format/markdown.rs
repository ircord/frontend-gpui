//! The box's Discord-style shortcuts as IRC formatting codes, so every IRC
//! client sees the formatting: `**bold**`, `*italic*` or `_italic_`,
//! `__underline__`, `~~strike~~` and `` `inline code` `` (IRC monospace). A
//! port of the web client's `markdownToIrc` and `ircToMarkdown`.
//!
//! Fenced blocks are sent untouched. Nothing inside code, URLs or words such
//! as snake_case is converted.

use super::blocks::{Mapped, Segment, split_code_blocks};

const BOLD: char = '\x02';
const ITALIC: char = '\x1d';
const UNDERLINE: char = '\x1f';
const STRIKE: char = '\x1e';
const MONO: char = '\x11';

/// The text a send puts on the wire.
pub fn markdown_to_irc(text: &str) -> String {
    split_code_blocks(&Mapped::new(text))
        .iter()
        .map(|segment| match segment {
            Segment::Code { raw, .. } => raw.text.clone(),
            Segment::Text { raw } => inline_to_irc(&raw.text),
        })
        .collect()
}

/// A piece of a line: text the emphasis rules may change, or inline code and
/// URLs they leave alone.
enum Piece {
    Text(String),
    Kept(String),
}

fn inline_to_irc(text: &str) -> String {
    // NUL is never valid in an IRC message.
    let text = text.replace('\0', "");
    // Inline code and URLs are set aside behind placeholders while the
    // emphasis rules run, so emphasis may still wrap inline code.
    let mut kept = Vec::new();
    let mut protected = String::new();
    for piece in protect(&text) {
        match piece {
            Piece::Text(t) => protected.push_str(&t),
            Piece::Kept(k) => {
                protected.push('\0');
                protected.push_str(&kept.len().to_string());
                protected.push('\0');
                kept.push(k);
            }
        }
    }
    let mut out = pair(&protected, "**", BOLD);
    out = pair(&out, "__", UNDERLINE);
    out = pair(&out, "~~", STRIKE);
    out = single(&out, '*', true);
    out = single(&out, '_', false);
    restore(&out, &kept)
}

/// `` `[^`\n]+` `` and `(?:https?|ircs?)://\S+`, left to right.
fn protect(text: &str) -> Vec<Piece> {
    let mut pieces = Vec::new();
    let mut plain = String::new();
    let mut i = 0;
    while i < text.len() {
        let rest = &text[i..];
        if let Some(after) = rest.strip_prefix('`')
            && let Some(end) = after.find(['`', '\n'])
            && end > 0
            && after.as_bytes()[end] == b'`'
        {
            pieces.push(Piece::Text(std::mem::take(&mut plain)));
            pieces.push(Piece::Kept(format!("{MONO}{}{MONO}", &after[..end])));
            i += end + 2;
            continue;
        }
        let scheme = ["https://", "http://", "ircs://", "irc://"]
            .into_iter()
            .find(|s| rest.starts_with(s));
        if let Some(scheme) = scheme {
            let len = rest[scheme.len()..]
                .find(char::is_whitespace)
                .unwrap_or(rest.len() - scheme.len());
            if len > 0 {
                pieces.push(Piece::Text(std::mem::take(&mut plain)));
                pieces.push(Piece::Kept(rest[..scheme.len() + len].to_string()));
                i += scheme.len() + len;
                continue;
            }
        }
        let c = rest.chars().next().unwrap();
        plain.push(c);
        i += c.len_utf8();
    }
    pieces.push(Piece::Text(plain));
    pieces
}

fn restore(text: &str, kept: &[String]) -> String {
    let mut out = String::new();
    let mut parts = text.split('\0');
    if let Some(first) = parts.next() {
        out.push_str(first);
    }
    while let (Some(index), Some(after)) = (parts.next(), parts.next()) {
        if let Some(k) = index.parse::<usize>().ok().and_then(|i| kept.get(i)) {
            out.push_str(k);
        }
        out.push_str(after);
    }
    out
}

/// `MM([^M\n]+)MM` with a two-character marker, as a global replace does it.
fn pair(text: &str, marker: &str, code: char) -> String {
    let m = marker.chars().next().unwrap();
    let mut out = String::new();
    let mut i = 0;
    while i < text.len() {
        let rest = &text[i..];
        if let Some(after) = rest.strip_prefix(marker) {
            let inner = after.find([m, '\n']).unwrap_or(after.len());
            if inner > 0 && after[inner..].starts_with(marker) {
                out.push(code);
                out.push_str(&after[..inner]);
                out.push(code);
                i += marker.len() * 2 + inner;
                continue;
            }
        }
        let c = rest.chars().next().unwrap();
        out.push(c);
        i += c.len_utf8();
    }
    out
}

fn is_word(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// `(^|[^\w*])\*([^*\s][^*\n]*?)\*(?![\w*])` for `*`, and
/// `(^|[^\w])_([^_\s][^_\n]*?)_(?!\w)` for `_`: a single marker that does not
/// sit inside a word.
fn single(text: &str, marker: char, star: bool) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    // The character before the next candidate, as the replace left it.
    let mut before: Option<char> = None;
    while i < chars.len() {
        let c = chars[i];
        let opens = c == marker
            && before.is_none_or(|b| !(is_word(b) || (star && b == '*')))
            && chars
                .get(i + 1)
                .is_some_and(|n| *n != marker && !n.is_whitespace());
        if opens {
            // The text may not hold the marker, so the first one after it
            // closes, unless a word character follows.
            let mut close = None;
            for (j, &d) in chars.iter().enumerate().skip(i + 2) {
                if d == '\n' {
                    break;
                }
                if d == marker {
                    let next = chars.get(j + 1);
                    if next.is_none_or(|n| !(is_word(*n) || (star && *n == '*'))) {
                        close = Some(j);
                    }
                    break;
                }
            }
            if let Some(end) = close {
                out.push(ITALIC);
                out.extend(&chars[i + 1..end]);
                out.push(ITALIC);
                before = Some(chars[end]);
                i = end + 1;
                continue;
            }
        }
        out.push(c);
        before = Some(c);
        i += 1;
    }
    out
}

/// The box's Markdown for a message's IRC formatting, the inverse of
/// [`markdown_to_irc`], so an edit starts from what was typed. Colours have no
/// Markdown form and are dropped.
pub fn irc_to_markdown(text: &str) -> String {
    let f = super::parse(text);
    if f.runs.is_empty() && f.text == text {
        return text.to_string();
    }
    let mut out = String::new();
    let mut at = 0;
    for (range, style) in &f.runs {
        out.push_str(&f.text[at..range.start]);
        let mut piece = f.text[range.clone()].to_string();
        if style.mono {
            piece = format!("`{piece}`");
        }
        if style.strike {
            piece = format!("~~{piece}~~");
        }
        if style.underline {
            piece = format!("__{piece}__");
        }
        if style.italic {
            piece = format!("*{piece}*");
        }
        if style.bold {
            piece = format!("**{piece}**");
        }
        out.push_str(&piece);
        at = range.end;
    }
    out.push_str(&f.text[at..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emphasis_becomes_irc_codes() {
        assert_eq!(
            markdown_to_irc("**b** *i* _i_ __u__ ~~s~~"),
            "\x02b\x02 \x1di\x1d \x1di\x1d \x1fu\x1f \x1es\x1e"
        );
        assert_eq!(
            markdown_to_irc("run `cargo **x**` now"),
            "run \x11cargo **x**\x11 now"
        );
        assert_eq!(markdown_to_irc("**`code`**"), "\x02\x11code\x11\x02");
    }

    #[test]
    fn words_urls_and_code_stay() {
        assert_eq!(
            markdown_to_irc("snake_case_name and 2*3*4"),
            "snake_case_name and 2*3*4"
        );
        assert_eq!(
            markdown_to_irc("see https://x.test/a_b_c and **https://x.test/**"),
            "see https://x.test/a_b_c and **https://x.test/**"
        );
        assert_eq!(
            markdown_to_irc("```\n**keep**\n```\n**go**"),
            "```\n**keep**\n```\n\x02go\x02"
        );
        assert_eq!(markdown_to_irc("* item\n* item"), "* item\n* item");
        assert_eq!(markdown_to_irc("a\0b"), "ab");
    }

    #[test]
    fn markdown_round_trips() {
        for text in ["**b** and *i*", "__u__ ~~s~~ `c`", "plain"] {
            assert_eq!(irc_to_markdown(&markdown_to_irc(text)), text);
        }
        assert_eq!(irc_to_markdown("\x034red\x03"), "red");
    }
}

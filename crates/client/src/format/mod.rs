//! Message text: mIRC formatting codes and links, a port of the web client's
//! `parseFormatting` (`shared/utils/irc.ts`). The result is plain text plus
//! styled byte ranges, which the app turns into GPUI text runs.
//!
//! The rest of the web client's text rules live beside it: fenced code,
//! quotes and lists ([`blocks`]), `@nick` and group mentions ([`mentions`]),
//! the pieces of a line that become pills or links ([`inline`]), message and
//! `irc://` links ([`links`]), Markdown typed in the box ([`markdown`]) and
//! code highlighting ([`highlight`]).

pub mod blocks;
pub mod clipboard;
pub mod highlight;
pub mod inline;
pub mod links;
pub mod markdown;
pub mod mentions;

use std::ops::Range;

/// mIRC colours 0–98 (ircdocs.horse/specs/formatting); 99 means "default".
pub const IRC_COLORS: [u32; 99] = [
    0xffffff, 0x000000, 0x00007f, 0x009300, 0xff0000, 0x7f0000, 0x9c009c, 0xfc7f00, 0xffff00,
    0x00fc00, 0x009393, 0x00ffff, 0x0000fc, 0xff00ff, 0x7f7f7f, 0xd2d2d2, 0x470000, 0x472100,
    0x474700, 0x324700, 0x004700, 0x00472c, 0x004747, 0x002747, 0x000047, 0x2e0047, 0x470047,
    0x47002a, 0x740000, 0x743a00, 0x747400, 0x517400, 0x007400, 0x007449, 0x007474, 0x004074,
    0x000074, 0x4b0074, 0x740074, 0x740045, 0xb50000, 0xb56300, 0xb5b500, 0x7db500, 0x00b500,
    0x00b571, 0x00b5b5, 0x0063b5, 0x0000b5, 0x7500b5, 0xb500b5, 0xb5006b, 0xff0000, 0xff8c00,
    0xffff00, 0xb2ff00, 0x00ff00, 0x00ffa0, 0x00ffff, 0x008cff, 0x0000ff, 0xa500ff, 0xff00ff,
    0xff0098, 0xff5959, 0xffb459, 0xffff71, 0xcfff60, 0x6fff6f, 0x65ffc9, 0x6dffff, 0x59b4ff,
    0x5959ff, 0xc459ff, 0xff66ff, 0xff59bc, 0xff9c9c, 0xffd39c, 0xffff9c, 0xe2ff9c, 0x9cff9c,
    0x9cffdb, 0x9cffff, 0x9cd3ff, 0x9c9cff, 0xdc9cff, 0xff9cff, 0xff94d3, 0x000000, 0x131313,
    0x282828, 0x363636, 0x4d4d4d, 0x656565, 0x818181, 0x9f9f9f, 0xbcbcbc, 0xe2e2e2, 0xffffff,
];

/// A colour: an RGB value, or the theme's text / background colour when reversed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Color {
    Rgb(u32),
    ThemeText,
    ThemeBackground,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Style {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    pub mono: bool,
    pub fg: Option<Color>,
    pub bg: Option<Color>,
}

impl Style {
    pub fn is_plain(&self) -> bool {
        *self == Style::default()
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Formatted {
    /// The text with formatting codes removed.
    pub text: String,
    /// Styled ranges of `text`; unstyled stretches are left out.
    pub runs: Vec<(Range<usize>, Style)>,
    /// Link ranges of `text` and the URL each opens.
    pub links: Vec<(Range<usize>, String)>,
}

pub fn parse(input: &str) -> Formatted {
    let mut out = Formatted::default();
    let mut style = Style::default();
    let mut reverse = false;
    let mut start = 0;
    let bytes = input.as_bytes();
    let mut i = 0;

    let flush = |out: &mut Formatted, start: &mut usize, style: Style, reverse: bool| {
        let end = out.text.len();
        if end > *start {
            let mut s = style;
            if reverse {
                s.fg = Some(style.bg.unwrap_or(Color::ThemeBackground));
                s.bg = Some(style.fg.unwrap_or(Color::ThemeText));
            }
            if !s.is_plain() {
                out.runs.push((*start..end, s));
            }
        }
        *start = end;
    };

    while i < bytes.len() {
        let b = bytes[i];
        match b {
            0x02 | 0x1d | 0x1f | 0x1e | 0x11 => {
                flush(&mut out, &mut start, style, reverse);
                match b {
                    0x02 => style.bold = !style.bold,
                    0x1d => style.italic = !style.italic,
                    0x1f => style.underline = !style.underline,
                    0x1e => style.strike = !style.strike,
                    _ => style.mono = !style.mono,
                }
                i += 1;
            }
            0x16 => {
                flush(&mut out, &mut start, style, reverse);
                reverse = !reverse;
                i += 1;
            }
            0x0f => {
                flush(&mut out, &mut start, style, reverse);
                style = Style::default();
                reverse = false;
                i += 1;
            }
            0x03 => {
                flush(&mut out, &mut start, style, reverse);
                i += 1;
                let (fg, n) = digits(&bytes[i..]);
                match fg {
                    None => {
                        style.fg = None;
                        style.bg = None;
                    }
                    Some(fg) => {
                        i += n;
                        style.fg = palette(fg);
                        if bytes.get(i) == Some(&b',') {
                            let (bg, m) = digits(&bytes[i + 1..]);
                            if let Some(bg) = bg {
                                i += 1 + m;
                                style.bg = palette(bg);
                            }
                        }
                    }
                }
            }
            0x04 => {
                flush(&mut out, &mut start, style, reverse);
                i += 1;
                match hex(&bytes[i..]) {
                    None => {
                        style.fg = None;
                        style.bg = None;
                    }
                    Some(fg) => {
                        i += 6;
                        style.fg = Some(Color::Rgb(fg));
                        if bytes.get(i) == Some(&b',')
                            && let Some(bg) = hex(&bytes[i + 1..])
                        {
                            i += 7;
                            style.bg = Some(Color::Rgb(bg));
                        }
                    }
                }
            }
            _ => {
                // Copy one whole UTF-8 character.
                let len = utf8_len(b);
                let end = (i + len).min(bytes.len());
                out.text.push_str(&input[i..end]);
                i = end;
            }
        }
    }
    flush(&mut out, &mut start, style, reverse);
    out.links = find_links(&out.text);
    out
}

fn utf8_len(first: u8) -> usize {
    match first {
        0x00..=0x7f => 1,
        0xc0..=0xdf => 2,
        0xe0..=0xef => 3,
        _ => 4,
    }
}

fn digits(b: &[u8]) -> (Option<usize>, usize) {
    let n = b.iter().take(2).take_while(|c| c.is_ascii_digit()).count();
    if n == 0 {
        return (None, 0);
    }
    let value = std::str::from_utf8(&b[..n]).unwrap().parse().unwrap();
    (Some(value), n)
}

fn palette(index: usize) -> Option<Color> {
    IRC_COLORS.get(index).map(|c| Color::Rgb(*c))
}

fn hex(b: &[u8]) -> Option<u32> {
    if b.len() < 6 || !b[..6].iter().all(u8::is_ascii_hexdigit) {
        return None;
    }
    u32::from_str_radix(std::str::from_utf8(&b[..6]).ok()?, 16).ok()
}

/// Text without formatting codes, for excerpts, search and notifications.
pub fn strip(input: &str) -> String {
    parse(input).text
}

/// `http://`, `https://`, `irc://`, `ircs://` and `www.` links, without
/// trailing punctuation.
pub fn find_links(text: &str) -> Vec<(Range<usize>, String)> {
    let mut links = Vec::new();
    let mut i = 0;
    while i < text.len() {
        let rest = &text[i..];
        let at_word_start = i == 0
            || text[..i]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_whitespace() || "([{<\"'`".contains(c));
        let scheme = ["https://", "http://", "ircs://", "irc://", "www."]
            .into_iter()
            .find(|p| {
                rest.len() > p.len()
                    && rest.as_bytes()[..p.len()].eq_ignore_ascii_case(p.as_bytes())
            });
        let Some(scheme) = scheme.filter(|_| at_word_start) else {
            i += rest.chars().next().map_or(1, char::len_utf8);
            continue;
        };
        let mut end = i + rest
            .find(|c: char| c.is_whitespace() || c == '<' || c == '>' || c == '"')
            .unwrap_or(rest.len());
        // Leave sentence punctuation and unbalanced closing brackets outside the link.
        loop {
            let candidate = &text[i..end];
            let Some(last) = candidate.chars().next_back() else {
                break;
            };
            let unbalanced = |open: char, close: char| {
                last == close && candidate.matches(open).count() < candidate.matches(close).count()
            };
            if ".,;:!?'".contains(last) || unbalanced('(', ')') || unbalanced('[', ']') {
                end -= last.len_utf8();
            } else {
                break;
            }
        }
        if end - i > scheme.len() {
            let raw = &text[i..end];
            let url = if scheme == "www." {
                format!("https://{raw}")
            } else {
                raw.to_string()
            };
            links.push((i..end, url));
        }
        i = end.max(i + 1);
    }
    links
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_has_no_runs() {
        let f = parse("hello world");
        assert_eq!(f.text, "hello world");
        assert!(f.runs.is_empty());
    }

    #[test]
    fn toggles_and_reset() {
        let f = parse("a\x02bold\x02 \x1ditalic\x0f plain");
        assert_eq!(f.text, "abold italic plain");
        assert_eq!(f.runs.len(), 2);
        assert_eq!(f.runs[0].0, 1..5);
        assert!(f.runs[0].1.bold);
        assert_eq!(&f.text[f.runs[1].0.clone()], "italic");
        assert!(f.runs[1].1.italic);
    }

    #[test]
    fn colours_with_and_without_background() {
        let f = parse("\x034red\x03 \x0312,1blue on black\x03,x");
        assert_eq!(f.text, "red blue on black,x");
        assert_eq!(f.runs[0].1.fg, Some(Color::Rgb(0xff0000)));
        assert_eq!(f.runs[1].1.fg, Some(Color::Rgb(0x0000fc)));
        assert_eq!(f.runs[1].1.bg, Some(Color::Rgb(0x000000)));
        // A comma not followed by digits is text.
        assert!(f.text.ends_with(",x"));
        // 99 is "default colour".
        assert_eq!(parse("\x0399x").runs, vec![]);
    }

    #[test]
    fn hex_colours_and_reverse() {
        let f = parse("\x04ff8800orange\x0f \x16rev");
        assert_eq!(f.runs[0].1.fg, Some(Color::Rgb(0xff8800)));
        assert_eq!(f.runs[1].1.fg, Some(Color::ThemeBackground));
        assert_eq!(f.runs[1].1.bg, Some(Color::ThemeText));
    }

    #[test]
    fn keeps_unicode_intact() {
        let f = parse("\x02héllo 🎉\x02 ünd");
        assert_eq!(f.text, "héllo 🎉 ünd");
        assert_eq!(&f.text[f.runs[0].0.clone()], "héllo 🎉");
    }

    #[test]
    fn finds_links_without_trailing_punctuation() {
        let links =
            find_links("see https://example.com/a(b). and (www.ircord.com) or http://x.y/z?q=1,");
        let urls: Vec<&str> = links.iter().map(|(_, u)| u.as_str()).collect();
        assert_eq!(
            urls,
            [
                "https://example.com/a(b)",
                "https://www.ircord.com",
                "http://x.y/z?q=1"
            ]
        );
        assert!(find_links("nohttps://x").is_empty());
        assert!(find_links("https://").is_empty());
    }

    #[test]
    fn links_survive_formatting() {
        let f = parse("\x02go\x02 https://ircord.com now");
        assert_eq!(f.links.len(), 1);
        assert_eq!(&f.text[f.links[0].0.clone()], "https://ircord.com");
    }
}

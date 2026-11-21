//! `@nick` and group mentions, a port of the web client's `shared/mentions.ts`
//! and `shared/group-mentions.ts`. The backend applies the same rules
//! (`internal/notify/mentions.go`, `groups.go`); `testdata/mentions.json` is a
//! copy of its `internal/notify/testdata/mentions.json`, which both follow.
//!
//! A name mention needs the `@`: "hey @alice" mentions alice, "Alice is here"
//! does not. Group mentions (`@here`, `@channel`, `@everyone`, `@owner`,
//! `@operator` or `@oper`, `@voiced`) are an IRCord convention: the text goes
//! out as typed and plain IRC clients see words.

/// May come right before the `@`: anything else makes it an address or a path.
const OPENERS: &str = "([{<\"'“‘«*,;";

/// Go's `unicode.IsSpace`, spelled out so it cannot drift from the backend:
/// U+0085 counts, U+FEFF does not.
fn is_space(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n'
            | '\u{0b}'
            | '\u{0c}'
            | '\r'
            | ' '
            | '\u{85}'
            | '\u{a0}'
            | '\u{1680}'
            | '\u{2000}'
            ..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}'
    )
}

/// Combining marks (Unicode category M) in the blocks text uses most.
fn is_mark(c: char) -> bool {
    matches!(c,
        '\u{0300}'..='\u{036f}'
        | '\u{0483}'..='\u{0489}'
        | '\u{0591}'..='\u{05bd}'
        | '\u{0610}'..='\u{061a}'
        | '\u{064b}'..='\u{065f}'
        | '\u{0900}'..='\u{0903}'
        | '\u{093a}'..='\u{094f}'
        | '\u{1ab0}'..='\u{1aff}'
        | '\u{1dc0}'..='\u{1dff}'
        | '\u{20d0}'..='\u{20ff}'
        | '\u{302a}'..='\u{302f}'
        | '\u{3099}'..='\u{309a}'
        | '\u{fe00}'..='\u{fe0f}'
        | '\u{fe20}'..='\u{fe2f}')
}

fn letter_or_digit(c: char) -> bool {
    c.is_alphabetic() || c.is_numeric()
}

/// A character a nick can hold: the RFC 1459 set, and letters, digits and
/// marks of other scripts.
pub fn is_nick_char(c: char) -> bool {
    if c.is_ascii() {
        c.is_ascii_alphanumeric() || "-[]\\`^_{|}".contains(c)
    } else {
        letter_or_digit(c) || is_mark(c)
    }
}

/// One character folded under the network's CASEMAPPING (`ascii`, `rfc1459`
/// when missing, `strict-rfc1459`, anything else by Unicode lower case).
pub fn fold_char(c: char, case_mapping: &str) -> char {
    let mode = case_mapping.to_ascii_lowercase();
    if !mode.is_empty() && !["ascii", "rfc1459", "strict-rfc1459"].contains(&mode.as_str()) {
        return c.to_lowercase().next().unwrap_or(c);
    }
    if c.is_ascii_uppercase() {
        return c.to_ascii_lowercase();
    }
    if mode == "ascii" {
        return c;
    }
    match c {
        '[' => '{',
        ']' => '}',
        '\\' => '|',
        '~' if mode != "strict-rfc1459" => '^',
        c => c,
    }
}

/// A nick folded for comparing under the network's CASEMAPPING.
pub fn fold(nick: &str, case_mapping: &str) -> String {
    nick.chars().map(|c| fold_char(c, case_mapping)).collect()
}

/// Whether text, with IRC formatting already removed, holds an explicit
/// `@nick` mention. The mention must be a whole nick: "@alice123",
/// "@alice-" and "@alice.example" do not mention alice, and a mention inside
/// a URL does not count.
pub fn nick_mentioned(text: &str, nick: &str, case_mapping: &str) -> bool {
    if nick.is_empty() {
        return false;
    }
    let want: Vec<char> = nick.chars().map(|c| fold_char(c, case_mapping)).collect();
    let chars: Vec<char> = text.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        if c != '@' {
            continue;
        }
        if i > 0 && !is_space(chars[i - 1]) && !OPENERS.contains(chars[i - 1]) {
            continue;
        }
        let end = i + 1 + want.len();
        if end > chars.len()
            || chars[i + 1..end]
                .iter()
                .map(|c| fold_char(*c, case_mapping))
                .ne(want.iter().copied())
        {
            continue;
        }
        if continues_nick(&chars[end..]) || in_url(&chars[..i]) {
            continue;
        }
        return true;
    }
    false
}

/// Whether the text right after a candidate mention makes it part of a
/// longer nick or address.
fn continues_nick(rest: &[char]) -> bool {
    let Some(&c) = rest.first() else {
        return false;
    };
    if is_nick_char(c) || c == '@' {
        return true;
    }
    c == '.' && rest.get(1).is_some_and(|n| letter_or_digit(*n))
}

/// Whether the word that ends at the mention contains `://`.
fn in_url(before: &[char]) -> bool {
    let start = before
        .iter()
        .rposition(|c| is_space(*c))
        .map_or(0, |i| i + 1);
    before[start..].iter().collect::<String>().contains("://")
}

/// Whether one of your highlight words is in the text: a whole word, any case,
/// no `@` needed.
pub fn word_highlighted(text: &str, word: &str) -> bool {
    if word.is_empty() {
        return false;
    }
    let text = text.to_lowercase();
    let word = word.to_lowercase();
    let boundary =
        |c: Option<char>| c.is_none_or(|c| !(c.is_ascii_alphanumeric() || c == '_' || c == '-'));
    text.match_indices(&word).any(|(at, _)| {
        boundary(text[..at].chars().next_back()) && boundary(text[at + word.len()..].chars().next())
    })
}

/// An `@` mention of your nick, or one of your highlight words. Takes the raw
/// text, formatting codes included.
pub fn is_highlight(text: &str, nick: &str, case_mapping: &str, words: &[String]) -> bool {
    let plain = super::strip(text);
    nick_mentioned(&plain, nick, case_mapping) || words.iter().any(|w| word_highlighted(&plain, w))
}

// ---- Group mentions -------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Group {
    Here,
    Channel,
    Everyone,
    Owner,
    Operator,
    Voiced,
}

impl Group {
    /// The name the composer offers and the pill shows, without the `@`.
    pub fn name(self) -> &'static str {
        match self {
            Group::Here => "here",
            Group::Channel => "channel",
            Group::Everyone => "everyone",
            Group::Owner => "owner",
            Group::Operator => "operator",
            Group::Voiced => "voiced",
        }
    }

    /// Reaches the whole channel at once, so the channel's setting decides
    /// who may use it.
    pub fn channel_wide(self) -> bool {
        matches!(self, Group::Here | Group::Channel | Group::Everyone)
    }
}

/// In the order the regular expression tries them, so `@operator` is not
/// read as `@oper`.
const GROUP_TOKENS: [(&str, Group); 7] = [
    ("here", Group::Here),
    ("channel", Group::Channel),
    ("everyone", Group::Everyone),
    ("owner", Group::Owner),
    ("operator", Group::Operator),
    ("oper", Group::Operator),
    ("voiced", Group::Voiced),
];

fn word_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-'
}

/// Group mentions in text with their byte ranges, whole words only.
pub fn groups_in(text: &str) -> Vec<(std::ops::Range<usize>, Group)> {
    let mut found = Vec::new();
    for (at, _) in text.match_indices('@') {
        let rest = &text[at + 1..];
        let Some((token, group)) = GROUP_TOKENS.iter().find(|(token, _)| {
            rest.len() >= token.len()
                && rest.as_bytes()[..token.len()].eq_ignore_ascii_case(token.as_bytes())
        }) else {
            continue;
        };
        let end = at + 1 + token.len();
        let before = text[..at].chars().next_back();
        let after = text[end..].chars().next();
        if before.is_some_and(word_char) || after.is_some_and(word_char) {
            continue;
        }
        found.push((at..end, *group));
    }
    found
}

/// Whether a whole word like `@here` or `@oper` is a group mention.
pub fn group_mention(word: &str) -> Option<Group> {
    let groups = groups_in(word);
    match groups.as_slice() {
        [(range, group)] if *range == (0..word.len()) => Some(*group),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct NickCase {
        text: String,
        nick: String,
        #[serde(default)]
        case_mapping: String,
        want: bool,
    }

    #[derive(serde::Deserialize)]
    struct HighlightCase {
        text: String,
        nick: String,
        words: Vec<String>,
        want: bool,
    }

    #[derive(serde::Deserialize)]
    struct Cases {
        nick: Vec<NickCase>,
        highlight: Vec<HighlightCase>,
    }

    fn cases() -> Cases {
        serde_json::from_str(include_str!("testdata/mentions.json")).unwrap()
    }

    #[test]
    fn backend_nick_mentions() {
        let cases = cases();
        assert!(cases.nick.len() > 50);
        for case in cases.nick {
            assert_eq!(
                nick_mentioned(&case.text, &case.nick, &case.case_mapping),
                case.want,
                "{:?} for {:?} ({})",
                case.text,
                case.nick,
                case.case_mapping
            );
        }
    }

    #[test]
    fn backend_highlights() {
        for case in cases().highlight {
            assert_eq!(
                is_highlight(&case.text, &case.nick, "", &case.words),
                case.want,
                "{:?} for {:?} {:?}",
                case.text,
                case.nick,
                case.words
            );
        }
    }

    #[test]
    fn group_mentions_are_whole_words() {
        let groups: Vec<Group> = groups_in("@here @OPER @operators x@everyone @voiced!")
            .into_iter()
            .map(|(_, g)| g)
            .collect();
        assert_eq!(groups, [Group::Here, Group::Operator, Group::Voiced]);
        assert_eq!(group_mention("@operator"), Some(Group::Operator));
        assert_eq!(group_mention("@channel"), Some(Group::Channel));
        assert_eq!(group_mention("@here!"), None);
        assert_eq!(group_mention("@alice"), None);
    }
}

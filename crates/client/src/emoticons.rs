//! Typed emoticons such as `:)`, `:D` and `<3`, which the message box turns
//! into emoji once the word they stand in ends. Ported from the web client
//! (`shared/utils/emoticons.ts` at 62b672a): the same table and the same
//! rules about what counts as an emoticon.

/// Each emoticon as typed, and its emoji. Forms that also appear in ordinary
/// text ("D:" for a drive, "B)" or "8)" in a list) are left out.
pub const EMOTICONS: &[(&str, &str)] = &[
    (":)", "🙂"),
    (":-)", "🙂"),
    ("=)", "🙂"),
    (":(", "🙁"),
    (":-(", "🙁"),
    ("=(", "🙁"),
    (":D", "😄"),
    (":-D", "😄"),
    ("=D", "😄"),
    (":P", "😛"),
    (":p", "😛"),
    (":-P", "😛"),
    (":-p", "😛"),
    ("=P", "😛"),
    (";P", "😜"),
    (";p", "😜"),
    (";-P", "😜"),
    (";-p", "😜"),
    (":O", "😮"),
    (":o", "😮"),
    (":-O", "😮"),
    (":-o", "😮"),
    (";)", "😉"),
    (";-)", "😉"),
    (":X", "🤐"),
    (":x", "🤐"),
    (":-X", "🤐"),
    (":-x", "🤐"),
    ("<3", "❤️"),
    ("</3", "💔"),
    (":'(", "😢"),
    (":'-(", "😢"),
    (":/", "😕"),
    (":-/", "😕"),
    (":|", "😐"),
    (":-|", "😐"),
    ("XD", "😆"),
    ("xD", "😆"),
    ("xd", "😆"),
    (":*", "😘"),
    (":-*", "😘"),
    (">:(", "😠"),
    (">:-(", "😠"),
];

/// An emoticon that a typed word finished, as [`finished`] reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Emoticon {
    /// Where it starts in the text, a byte offset.
    pub start: usize,
    /// As typed, without the whitespace after it.
    pub typed: &'static str,
    pub emoji: &'static str,
}

/// The emoticon that ends `before` and starts it or follows whitespace, as
/// `(start, typed, emoji)`. Where two fit (`:-(` inside `>:-(`), the one that
/// starts earliest, which is the longer, wins.
fn ending_at(before: &str) -> Option<(usize, &'static str, &'static str)> {
    let mut best: Option<(usize, &'static str, &'static str)> = None;
    for &(typed, emoji) in EMOTICONS {
        if !before.ends_with(typed) {
            continue;
        }
        let start = before.len() - typed.len();
        if start > 0 && !before[..start].ends_with(char::is_whitespace) {
            continue;
        }
        match best {
            Some((earliest, _, _)) if earliest <= start => {}
            _ => best = Some((start, typed, emoji)),
        }
    }
    best
}

/// Whether `index` falls inside code: an open ``` fence (fences open at the
/// start of a line) or an open `inline code` span on the same line.
pub fn inside_code(text: &str, index: usize) -> bool {
    let Some(before) = text.get(..index) else {
        return false;
    };
    let fences = before
        .match_indices("```")
        .filter(|(at, _)| *at == 0 || before.as_bytes()[at - 1] == b'\n')
        .count();
    if fences % 2 == 1 {
        return true;
    }
    let line = &before[before.rfind('\n').map_or(0, |at| at + 1)..];
    let line = line.strip_prefix("```").unwrap_or(line);
    line.matches('`').count() % 2 == 1
}

/// The emoticon that whitespace typed at `caret` just finished, if any. An
/// emoticon inside code, a URL or a longer word stays as typed: it must start
/// the text or follow whitespace.
pub fn finished(text: &str, caret: usize) -> Option<Emoticon> {
    let before = text.get(..caret)?;
    let space = before.chars().next_back().filter(|c| c.is_whitespace())?;
    let (start, typed, emoji) = ending_at(&before[..before.len() - space.len_utf8()])?;
    if inside_code(text, start) {
        return None;
    }
    Some(Emoticon {
        start,
        typed,
        emoji,
    })
}

/// Whether `typed` is one of the table's forms, such as `:-D`. The emoji
/// search leaves these alone.
pub fn is_emoticon(typed: &str) -> bool {
    EMOTICONS.iter().any(|&(form, _)| form == typed)
}

/// The text with an emoticon that ends it turned into its emoji. Sending
/// finishes the last word, so `see you :P` sends `see you 😛`.
pub fn convert_trailing(text: &str) -> String {
    match ending_at(text) {
        Some((start, _, emoji)) if !inside_code(text, start) => {
            format!("{}{emoji}", &text[..start])
        }
        _ => text.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str) -> Option<Emoticon> {
        finished(text, text.len())
    }

    fn found(start: usize, typed: &'static str, emoji: &'static str) -> Option<Emoticon> {
        Some(Emoticon {
            start,
            typed,
            emoji,
        })
    }

    #[test]
    fn the_emoticons_from_the_request_convert_after_a_space() {
        for (typed, emoji) in [
            (":)", "🙂"),
            (":(", "🙁"),
            (":D", "😄"),
            (":P", "😛"),
            (":O", "😮"),
            (";)", "😉"),
            (":X", "🤐"),
            ("<3", "❤️"),
        ] {
            assert_eq!(at(&format!("{typed} ")), found(0, typed, emoji), "{typed}");
        }
    }

    #[test]
    fn every_listed_emoticon_converts_after_a_space_or_a_line_break() {
        for &(typed, emoji) in EMOTICONS {
            assert_eq!(
                at(&format!("hi {typed} ")),
                found(3, typed, emoji),
                "{typed}"
            );
            assert_eq!(
                at(&format!("hi {typed}\n")),
                found(3, typed, emoji),
                "{typed}"
            );
        }
    }

    #[test]
    fn the_longest_emoticon_wins() {
        assert_eq!(at("hi :-D "), found(3, ":-D", "😄"));
        assert_eq!(at("a </3 "), found(2, "</3", "💔"));
        assert_eq!(at("hi >:-( "), found(3, ">:-(", "😠"));
    }

    #[test]
    fn the_caret_can_sit_before_later_text() {
        assert_eq!(finished("a :D  b", 5), found(2, ":D", "😄"));
    }

    #[test]
    fn an_emoticon_needs_the_whitespace_typed_right_after_it() {
        assert_eq!(at(":)"), None);
        assert_eq!(at(":)  "), None);
    }

    #[test]
    fn emoticons_inside_a_word_or_url_stay_as_typed() {
        assert_eq!(at("word:( "), None);
        assert_eq!(at("https://x.dev/:) "), None);
        assert_eq!(at("see http:/ "), None);
        assert_eq!(at("ratio 1:D "), None);
        assert_eq!(at("a<3 "), None);
    }

    #[test]
    fn emoticons_inside_code_stay_as_typed() {
        assert_eq!(at("```\ncode :) "), None);
        assert_eq!(at("```js\n:D "), None);
        assert_eq!(at("hi `code :) "), None);
        assert_eq!(at("a `b` c :) ").map(|e| e.emoji), Some("🙂"));
        assert_eq!(at("```\nx\n```\n:D ").map(|e| e.emoji), Some("😄"));
    }

    #[test]
    fn look_alikes_from_ordinary_text_are_not_emoticons() {
        assert_eq!(at("D: "), None);
        assert_eq!(at("B) "), None);
        assert_eq!(at(":S "), None);
    }

    #[test]
    fn inline_code_is_counted_per_line() {
        assert!(!inside_code("`a\nb", 4));
        assert!(inside_code("x `a", 4));
    }

    #[test]
    fn the_emoji_search_leaves_emoticons_alone() {
        assert!(is_emoticon(":-D"));
        assert!(is_emoticon(":-p"));
        assert!(!is_emoticon(":-1"), "a shortcode still searches");
        assert!(!is_emoticon(":joy"));
    }

    #[test]
    fn sending_converts_a_final_emoticon_once() {
        assert_eq!(convert_trailing("hi :)"), "hi 🙂");
        assert_eq!(convert_trailing("<3"), "❤️");
        assert_eq!(convert_trailing("/me waves :D"), "/me waves 😄");
        assert_eq!(convert_trailing("hi 🙂"), "hi 🙂");
        assert_eq!(convert_trailing("hi :) "), "hi :) ");
        assert_eq!(convert_trailing("foo:D"), "foo:D");
        assert_eq!(convert_trailing("see `:)"), "see `:)");
        assert_eq!(convert_trailing("```\n:D"), "```\n:D");
    }
}

//! The rules of Ircoid's high score table, as the server keeps them (and as
//! the web's `hash-field-scores.ts` writes them for Hash Field): whether a
//! score places, what a name may be, and a default name clipped to fit.
//!
//! The server has the last word: these let the game ask for a name only when
//! the score places, and refuse an impossible name before sending it.

use ircord_contract::IrcoidEntry;

use super::MAX_SCORE;

/// Entries in the table.
pub const TABLE_SIZE: usize = 10;
/// The longest name, in characters.
pub const MAX_NAME: usize = 24;

/// Every score is a positive multiple of 10 up to [`MAX_SCORE`].
pub fn valid_score(score: u32) -> bool {
    score > 0 && score <= MAX_SCORE && score.is_multiple_of(10)
}

/// Whether a finished game's score makes `table` (best first): ties keep the
/// earlier entry.
pub fn places_in(table: &[IrcoidEntry], score: u32) -> bool {
    if !valid_score(score) {
        return false;
    }
    table.len() < TABLE_SIZE
        || table
            .last()
            .is_some_and(|last| i64::from(score) > last.score)
}

/// Why a name was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameError {
    /// Empty, or longer than [`MAX_NAME`].
    Length,
    /// A character the table cannot show.
    Characters,
}

/// Whether the table can show `c`: letters, marks, numbers, punctuation,
/// symbols and spaces, and the joiner inside emoji. Controls, format
/// characters (bidi overrides and the like), private use, non-characters and
/// U+FFFD are refused.
fn showable(c: char) -> bool {
    let n = c as u32;
    if c == '\u{200d}' {
        return true;
    }
    if c.is_control() && !c.is_whitespace() {
        return false;
    }
    let refused = [
        0x00ad..=0x00ad,
        0x0600..=0x0605,
        0x061c..=0x061c,
        0x06dd..=0x06dd,
        0x070f..=0x070f,
        0x180e..=0x180e,
        0x200b..=0x200f,
        0x2028..=0x202e,
        0x2060..=0x206f,
        0xd800..=0xdfff,
        0xe000..=0xf8ff,
        0xfdd0..=0xfdef,
        0xfeff..=0xfeff,
        0xfff9..=0xfffd,
        0xe0001..=0xe007f,
        0xf0000..=0x10ffff,
    ];
    if refused.iter().any(|r| r.contains(&n)) || n & 0xfffe == 0xfffe {
        return false;
    }
    // Control-like whitespace (line and paragraph breaks) is folded away;
    // ordinary whitespace is allowed.
    true
}

/// A table name: trimmed, runs of whitespace folded to one space, 1 to
/// [`MAX_NAME`] characters.
pub fn table_name(raw: &str) -> Result<String, NameError> {
    if !raw.chars().all(showable) {
        return Err(NameError::Characters);
    }
    let name = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    let count = name.chars().count();
    if count == 0 || count > MAX_NAME {
        return Err(NameError::Length);
    }
    Ok(name)
}

/// The first [`MAX_NAME`] characters of `name`.
pub fn clip_name(name: &str) -> String {
    name.trim().chars().take(MAX_NAME).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(scores: &[i64]) -> Vec<IrcoidEntry> {
        scores
            .iter()
            .enumerate()
            .map(|(i, score)| IrcoidEntry {
                rank: i as i64 + 1,
                name: format!("p{i}"),
                score: *score,
            })
            .collect()
    }

    #[test]
    fn a_short_table_takes_any_valid_score() {
        assert!(places_in(&table(&[225357, 2102]), 10));
        assert!(!places_in(&table(&[]), 0));
        assert!(!places_in(&table(&[]), 15), "not a multiple of 10");
        assert!(!places_in(&table(&[]), MAX_SCORE + 10));
    }

    #[test]
    fn a_full_table_takes_only_a_better_score_and_ties_do_not_place() {
        let full = table(&[900, 800, 700, 600, 500, 400, 300, 200, 100, 50]);
        assert!(places_in(&full, 60));
        assert!(!places_in(&full, 50), "a tie keeps the earlier entry");
        assert!(!places_in(&full, 40));
    }

    #[test]
    fn names_are_folded_and_checked() {
        assert_eq!(
            table_name("  Peter   Petermann ").unwrap(),
            "Peter Petermann"
        );
        assert_eq!(
            table_name("👩‍💻 dev").unwrap(),
            "👩‍💻 dev",
            "emoji with a joiner"
        );
        assert_eq!(table_name("Zoë ~ @op!").unwrap(), "Zoë ~ @op!");
        assert_eq!(table_name("bad\u{7}name"), Err(NameError::Characters));
        assert_eq!(table_name("evil\u{202e}txt"), Err(NameError::Characters));
        assert_eq!(table_name("x\u{fffd}"), Err(NameError::Characters));
        assert_eq!(table_name("   "), Err(NameError::Length));
        assert_eq!(table_name(&"a".repeat(24)).unwrap().len(), 24);
        assert_eq!(table_name(&"a".repeat(25)), Err(NameError::Length));
    }

    #[test]
    fn a_default_name_is_clipped_by_characters() {
        let long = "é".repeat(30);
        assert_eq!(clip_name(&long).chars().count(), MAX_NAME);
        assert_eq!(clip_name(" me "), "me");
    }
}

//! The `/status` arguments shared by the composer and other command views.

use chrono::{DateTime, Duration, SecondsFormat, Utc};
use icu_properties::CodePointSetData;
use icu_properties::props::{ExtendedPictographic, RegionalIndicator};
use ircord_contract::ProfileUpdate;
use unicode_segmentation::UnicodeSegmentation;

use crate::store::people::{STATUS_EMOJI_BYTES, STATUS_TEXT_BYTES};

#[derive(Debug, PartialEq)]
pub enum Command {
    Edit,
    Clear,
    Set(Status),
}

#[derive(Debug, PartialEq)]
pub struct Status {
    text: String,
    emoji: String,
    expires_at: String,
}

impl From<Status> for ProfileUpdate {
    fn from(status: Status) -> Self {
        Self {
            status_text: Some(status.text),
            status_emoji: Some(status.emoji),
            status_expires_at: Some(status.expires_at),
            ..Default::default()
        }
    }
}

/// An empty invocation opens the editor. Explicit arguments change only the
/// network profile's status, using elapsed time for any deadline.
pub fn parse(args: &str, now: DateTime<Utc>) -> Option<Command> {
    let args = args.trim();
    match args {
        "" => return Some(Command::Edit),
        "clear" => return Some(Command::Clear),
        _ => {}
    }
    if args.chars().any(char::is_control) {
        return None;
    }

    let mut emoji = None;
    let mut duration = None;
    let mut message = Vec::new();
    let mut words = args.split_whitespace();
    while let Some(word) = words.next() {
        let flag = match word {
            "--emoji" => &mut emoji,
            "--for" => &mut duration,
            _ if word.starts_with("--") => return None,
            _ => {
                message.push(word);
                continue;
            }
        };
        let value = words.next().filter(|value| !value.starts_with("--"))?;
        if flag.replace(value).is_some() {
            return None;
        }
    }
    let message = if let Some(first) = message.first().copied().filter(|word| is_emoji(word)) {
        if emoji.replace(first).is_some() {
            return None;
        }
        &message[1..]
    } else {
        &message
    };
    let text = message.join(" ");
    let emoji = emoji.unwrap_or_default();
    if text.is_empty() || text.len() > STATUS_TEXT_BYTES || emoji.len() > STATUS_EMOJI_BYTES {
        return None;
    }
    let expires_at = match duration {
        None | Some("never") => String::new(),
        Some(duration) => now
            .checked_add_signed(elapsed(duration)?)?
            .to_rfc3339_opts(SecondsFormat::Millis, true),
    };
    Some(Command::Set(Status {
        text,
        emoji: emoji.to_owned(),
        expires_at,
    }))
}

fn is_emoji(word: &str) -> bool {
    if word.graphemes(true).count() != 1 {
        return false;
    }
    let pictographic = CodePointSetData::new::<ExtendedPictographic>();
    let regional = CodePointSetData::new::<RegionalIndicator>();
    word.chars().any(|c| pictographic.contains(c))
        || (word.chars().count() == 2 && word.chars().all(|c| regional.contains(c)))
        || word.strip_suffix('\u{20e3}').is_some_and(|key| {
            let key = key.strip_suffix('\u{fe0f}').unwrap_or(key);
            key.len() == 1 && "0123456789#*".contains(key)
        })
}

fn elapsed(value: &str) -> Option<Duration> {
    let seconds_per_unit = match value.as_bytes().last()? {
        b'm' => 60,
        b'h' => 60 * 60,
        b'd' => 24 * 60 * 60,
        _ => return None,
    };
    let n = &value[..value.len() - 1];
    if n.is_empty() || !n.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let seconds = n.parse::<i64>().ok()?.checked_mul(seconds_per_unit)?;
    (1..=365 * 24 * 60 * 60)
        .contains(&seconds)
        .then(|| Duration::seconds(seconds))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(args: &str) -> Option<Command> {
        let now = DateTime::parse_from_rfc3339("2026-11-14T12:00:00Z")
            .unwrap()
            .to_utc();
        parse(args, now)
    }

    fn update(args: &str) -> ProfileUpdate {
        let Some(Command::Set(update)) = parsed(args) else {
            panic!("valid status: {args}")
        };
        update.into()
    }

    #[test]
    fn edit_clear_and_set_have_distinct_arguments() {
        assert_eq!(parsed("  "), Some(Command::Edit));
        assert_eq!(parsed("clear"), Some(Command::Clear));
        for emoji in ["☕", "☕️", "🇩🇰", "👩🏽‍💻", "1️⃣"] {
            assert_eq!(
                update(&format!("{emoji} In a meeting"))
                    .status_emoji
                    .as_deref(),
                Some(emoji)
            );
        }
        let update = update("--emoji :coffee: Busy --for 1h");
        assert_eq!(
            update,
            ProfileUpdate {
                status_emoji: Some(":coffee:".into()),
                status_text: Some("Busy".into()),
                status_expires_at: Some("2026-11-14T13:00:00.000Z".into()),
                ..Default::default()
            }
        );
    }

    #[test]
    fn plain_words_and_numbers_stay_in_the_message() {
        for message in ["1 meeting", "A", "☕coffee break"] {
            let update = update(message);
            assert_eq!(update.status_text.as_deref(), Some(message));
            assert_eq!(update.status_emoji.as_deref(), Some(""));
        }
        assert_eq!(
            update("Busy --for never").status_expires_at.as_deref(),
            Some("")
        );
        assert_eq!(
            update("Busy --for 365d").status_expires_at.as_deref(),
            Some("2027-11-14T12:00:00.000Z")
        );
    }

    #[test]
    fn rejects_bad_flags_out_of_range_deadlines_and_excess_bytes() {
        for args in [
            "--emoji :coffee:",
            "Busy\nnow",
            "☕ Busy --emoji x",
            "Busy --unknown x",
            "Busy --for",
            "Busy --for 1h --for 2h",
            "Busy --for 0m",
            "Busy --for +1h",
            "Busy --for 366d",
            "Busy --for 999999999999999999d",
        ] {
            assert!(parsed(args).is_none(), "{args:?}");
        }
        assert!(parsed(&"é".repeat(100)).is_some());
        assert!(parsed(&"é".repeat(101)).is_none());
        assert!(parsed(&format!("Busy --emoji {}", "é".repeat(33))).is_none());
    }
}

//! The text a copied stretch of the timeline puts on the clipboard: a day
//! line, then one log line per message or event, as the web client's
//! `selectionLog` writes it.
//!
//! ```text
//! --- Tuesday, November 24, 2026 ---
//! [19:10] <karbowiak> that should be implemented already
//! [19:10] * karbowiak waves
//! [19:11] *** bob joined
//! ```
//!
//! The timeline draws each message as separate pieces of selectable text, and
//! the window asks each piece what it adds. [`contribution`] answers, so the
//! pieces together read as [`selection_log`] does.

use std::collections::BTreeMap;

use chrono::{DateTime, TimeZone};

use crate::format;

/// What kind of line a person said.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Say {
    /// `<nick> text`
    #[default]
    Message,
    /// `* nick text`
    Action,
    /// `-nick- text`
    Notice,
}

/// What a line says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Body {
    Said {
        say: Say,
        nick: String,
        text: String,
    },
    /// `*** text`: a join, part, topic change and the like.
    Event(String),
}

impl Default for Body {
    fn default() -> Self {
        Body::Event(String::new())
    }
}

/// One message or event, ready to be copied.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Entry {
    pub id: String,
    /// When it was sent, in milliseconds since the epoch.
    pub at: i64,
    /// Orders messages that share a millisecond, as the timeline does.
    pub position: i64,
    /// The local day as a key, to tell where a selection crosses midnight.
    pub day: String,
    /// The day as `Tuesday, November 24, 2026`: absolute, since "Today" is
    /// wrong once pasted.
    pub day_label: String,
    /// The short time shown in the timeline, such as `19:10`.
    pub time: String,
    pub body: Body,
}

impl Entry {
    /// An entry sent at `at` ms, its day taken in `tz`.
    pub fn new<Tz: TimeZone>(id: &str, at: i64, time: &str, body: Body, tz: &Tz) -> Entry
    where
        Tz::Offset: std::fmt::Display,
    {
        let local: DateTime<Tz> = tz
            .timestamp_millis_opt(at)
            .single()
            .unwrap_or_else(|| tz.timestamp_millis_opt(0).unwrap());
        Entry {
            id: id.to_string(),
            at,
            position: 0,
            day: local.format("%Y-%m-%d").to_string(),
            day_label: local.format("%A, %B %-d, %Y").to_string(),
            time: time.to_string(),
            body,
        }
    }

    /// `[19:10] <nick> text`.
    pub fn line(&self) -> String {
        let at = format!("[{}]", self.time);
        match &self.body {
            Body::Event(text) => format!("{at} *** {text}"),
            Body::Said { say, nick, text } => match say {
                Say::Message => format!("{at} <{nick}> {text}"),
                Say::Action => format!("{at} * {nick} {text}"),
                Say::Notice => format!("{at} -{nick}- {text}"),
            },
        }
    }
}

/// The text of a message as one log line holds it: IRC formatting removed,
/// line breaks as ` ↵ `, then each uploaded file's URL the text does not
/// already show. A GIF reads as its URL.
pub fn message_text<'a>(
    text: &str,
    gif: Option<&str>,
    files: impl IntoIterator<Item = &'a str>,
) -> String {
    let plain = format::parse(gif.unwrap_or(text))
        .text
        .replace("\r\n", " ↵ ")
        .replace('\n', " ↵ ");
    let mut out = plain.clone();
    for url in files {
        if !url.is_empty() && !plain.contains(url) {
            out.push(' ');
            out.push_str(url);
        }
    }
    out
}

fn heading(entry: &Entry) -> String {
    format!("--- {} ---", entry.day_label)
}

/// A day line, then one line per entry, with another day line wherever the
/// selection crosses midnight. `entries` are in the order they read.
pub fn selection_log(entries: &[Entry]) -> String {
    let mut lines = Vec::with_capacity(entries.len());
    let mut day = "";
    for entry in entries {
        if entry.day != day {
            lines.push(heading(entry));
            day = &entry.day;
        }
        lines.push(entry.line());
    }
    lines.join("\n")
}

/// One piece of selected text in the timeline: a paragraph of a message, or
/// the text of an event. A piece stands for `entries` (a folded run of
/// events stands for several) and `key` tells it from the message's other
/// pieces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pick {
    pub key: String,
    pub entries: Vec<Entry>,
    /// The part of the piece that is selected, as plain copying would give it.
    pub selected: String,
}

impl Pick {
    /// Reading order: the timeline's `(time, position, id)`.
    fn unit(&self) -> (i64, i64, &str) {
        let first = self.entries.first();
        (
            first.map_or(0, |e| e.at),
            first.map_or(0, |e| e.position),
            first.map_or("", |e| e.id.as_str()),
        )
    }
}

/// What piece `me` of `picks` adds to the clipboard.
///
/// Selecting inside one message or event copies what is selected, as any
/// text does. Selecting across several copies each of them whole as a log
/// line. The window orders its pieces itself, which a message's pieces and
/// events do not follow, so the first piece in reading order writes the whole
/// log and the others add nothing.
pub fn contribution(picks: &[Pick], me: &str) -> String {
    let Some(mine) = picks.iter().find(|p| p.key == me) else {
        return String::new();
    };
    // One piece for each message or event: its first.
    let mut units: BTreeMap<(i64, i64, &str), &Pick> = BTreeMap::new();
    for pick in picks {
        let slot = units.entry(pick.unit()).or_insert(pick);
        if pick.key < slot.key {
            *slot = pick;
        }
    }
    if units.len() <= 1 {
        return mine.selected.clone();
    }
    if units.values().next().is_none_or(|first| first.key != me) {
        return String::new();
    }
    let entries: Vec<Entry> = units
        .values()
        .flat_map(|p| p.entries.iter().cloned())
        .collect();
    selection_log(&entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    const NOV24_1910: i64 = 1_795_547_400_000;

    fn said(id: &str, at: i64, time: &str, nick: &str, say: Say, text: &str) -> Entry {
        Entry::new(
            id,
            at,
            time,
            Body::Said {
                say,
                nick: nick.into(),
                text: text.into(),
            },
            &Utc,
        )
    }

    fn event(id: &str, at: i64, time: &str, text: &str) -> Entry {
        Entry::new(id, at, time, Body::Event(text.into()), &Utc)
    }

    fn pick(key: &str, entries: Vec<Entry>, selected: &str) -> Pick {
        Pick {
            key: key.into(),
            entries,
            selected: selected.into(),
        }
    }

    #[test]
    fn the_web_clients_example_reads_the_same() {
        let at = |s: &str| {
            let d = chrono::DateTime::parse_from_rfc3339(s).unwrap();
            d.timestamp_millis()
        };
        let a = said(
            "1",
            at("2026-11-24T19:10:05Z"),
            "19:10",
            "karbowiak",
            Say::Message,
            "that should be implemented already",
        );
        let b = said(
            "2",
            at("2026-11-24T19:10:40Z"),
            "19:10",
            "karbowiak",
            Say::Message,
            "the go backend exposes it",
        );
        assert_eq!(
            selection_log(&[a, b]),
            "--- Tuesday, November 24, 2026 ---\n\
             [19:10] <karbowiak> that should be implemented already\n\
             [19:10] <karbowiak> the go backend exposes it"
        );
    }

    #[test]
    fn writes_actions_notices_and_events() {
        let log = selection_log(&[
            said("1", NOV24_1910, "19:10", "bob", Say::Action, "waves"),
            said("2", NOV24_1910, "19:10", "bob", Say::Notice, "note"),
            event("3", NOV24_1910, "19:11", "bob joined"),
        ]);
        let lines: Vec<&str> = log.lines().collect();
        assert_eq!(lines[1], "[19:10] * bob waves");
        assert_eq!(lines[2], "[19:10] -bob- note");
        assert_eq!(lines[3], "[19:11] *** bob joined");
    }

    #[test]
    fn repeats_the_day_line_across_midnight() {
        let late = chrono::DateTime::parse_from_rfc3339("2026-11-23T23:59:00Z")
            .unwrap()
            .timestamp_millis();
        let early = chrono::DateTime::parse_from_rfc3339("2026-11-24T00:01:00Z")
            .unwrap()
            .timestamp_millis();
        let log = selection_log(&[
            said("1", late, "23:59", "ann", Say::Message, "night"),
            said("2", early, "00:01", "ann", Say::Message, "morning"),
            said("3", early + 1000, "00:01", "bob", Say::Message, "hi"),
        ]);
        assert_eq!(
            log,
            "--- Monday, November 23, 2026 ---\n[23:59] <ann> night\n\
             --- Tuesday, November 24, 2026 ---\n[00:01] <ann> morning\n[00:01] <bob> hi"
        );
    }

    #[test]
    fn day_follows_the_time_zone() {
        let tz = chrono::FixedOffset::east_opt(2 * 3600).unwrap();
        let at = chrono::DateTime::parse_from_rfc3339("2026-11-23T23:30:00Z")
            .unwrap()
            .timestamp_millis();
        let e = Entry::new("1", at, "01:30", Body::Event("x".into()), &tz);
        assert_eq!(e.day, "2026-11-24");
        assert_eq!(e.day_label, "Tuesday, November 24, 2026");
    }

    #[test]
    fn message_text_flattens_formatting_breaks_and_files() {
        assert_eq!(
            message_text("hi \x02there\x02\nfriend", None, []),
            "hi there ↵ friend"
        );
        assert_eq!(
            message_text(
                "see https://x/a.png",
                None,
                ["https://x/a.png", "https://x/b.png"]
            ),
            "see https://x/a.png https://x/b.png"
        );
        assert_eq!(
            message_text("ignored", Some("https://g/1.gif"), []),
            "https://g/1.gif"
        );
    }

    #[test]
    fn the_first_piece_writes_the_whole_log() {
        let a = said("1", NOV24_1910, "19:10", "ann", Say::Message, "one");
        let b = said("2", NOV24_1910 + 1000, "19:10", "bob", Say::Message, "two");
        let e = event("3", NOV24_1910 + 2000, "19:10", "cy joined");
        // The second message has two paragraphs.
        let picks = vec![
            pick("e3", vec![e.clone()], "cy joined"),
            pick("m2-p1", vec![b.clone()], "more"),
            pick("m1-p0", vec![a.clone()], "one"),
            pick("m2-p0", vec![b.clone()], "two"),
        ];
        assert_eq!(contribution(&picks, "m1-p0"), selection_log(&[a, b, e]));
        for key in ["m2-p0", "m2-p1", "e3"] {
            assert_eq!(contribution(&picks, key), "", "{key}");
        }
    }

    #[test]
    fn a_selection_across_midnight_has_a_day_line_for_each_day() {
        let day1 = said(
            "1",
            NOV24_1910 - 86_400_000,
            "19:10",
            "ann",
            Say::Message,
            "a",
        );
        let day2 = said("2", NOV24_1910, "19:10", "ann", Say::Message, "b");
        let picks = vec![pick("m2", vec![day2], "b"), pick("m1", vec![day1], "a")];
        assert_eq!(
            contribution(&picks, "m1"),
            "--- Monday, November 23, 2026 ---\n[19:10] <ann> a\n\
             --- Tuesday, November 24, 2026 ---\n[19:10] <ann> b"
        );
        assert_eq!(contribution(&picks, "m2"), "");
    }

    #[test]
    fn messages_sharing_a_millisecond_read_in_position_order() {
        let mut first = said("k9", NOV24_1910, "19:10", "bob", Say::Message, "first");
        first.position = 1;
        let mut second = said("a1", NOV24_1910, "19:10", "ann", Say::Message, "second");
        second.position = 2;
        let picks = vec![
            pick("b", vec![second], "second"),
            pick("a", vec![first], "first"),
        ];
        assert_eq!(
            contribution(&picks, "a"),
            "--- Tuesday, November 24, 2026 ---\n[19:10] <bob> first\n[19:10] <ann> second"
        );
        assert_eq!(contribution(&picks, "b"), "");
    }

    #[test]
    fn a_selection_inside_one_message_copies_as_selected() {
        let a = said(
            "1",
            NOV24_1910,
            "19:10",
            "ann",
            Say::Message,
            "one two three",
        );
        let picks = vec![pick("m1-p0", vec![a], "two")];
        assert_eq!(contribution(&picks, "m1-p0"), "two");
    }

    #[test]
    fn a_folded_run_copies_every_event() {
        let a = said("1", NOV24_1910, "19:10", "ann", Say::Message, "hi");
        let run = vec![
            event("2", NOV24_1910 + 1000, "19:11", "bob joined"),
            event("3", NOV24_1910 + 2000, "19:11", "cy joined"),
        ];
        let picks = vec![pick("m1", vec![a], "hi"), pick("fold", run, "19:11")];
        assert_eq!(
            contribution(&picks, "m1"),
            "--- Tuesday, November 24, 2026 ---\n[19:10] <ann> hi\n\
             [19:11] *** bob joined\n[19:11] *** cy joined"
        );
    }
}

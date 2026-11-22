//! Message search filters (package FD1), as the web client has them in
//! `shared/utils/search-filter.ts` and `composables/search/search-source.ts`:
//! free text plus `from:`, `mentions:`, `in:`, `has:`, `before:`, `after:`,
//! `on:` and `author:`. People and conversations are kept by ID; dates are
//! local calendar days (`YYYY-MM-DD`) and become instants at local midnight
//! only when the server is asked.
//!
//! Also here: the request's query pairs, where results come from (retained
//! messages or the IRC server's own history), and the recent searches kept
//! on this device.

use std::collections::HashMap;
use std::io;

use chrono::{Days, Local, NaiveDate, SecondsFormat, TimeZone};
use ircord_contract as c;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::config::ConfigDir;

/// Kinds of content `has:` picks, in the order the filters list them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Has {
    Link,
    Image,
    Gif,
    Video,
    File,
}

impl Has {
    pub const ALL: [Has; 5] = [Has::Link, Has::Image, Has::Gif, Has::Video, Has::File];

    pub fn as_str(self) -> &'static str {
        match self {
            Has::Link => "link",
            Has::Image => "image",
            Has::Gif => "gif",
            Has::Video => "video",
            Has::File => "file",
        }
    }

    pub fn parse(value: &str) -> Option<Has> {
        Has::ALL
            .into_iter()
            .find(|h| h.as_str().eq_ignore_ascii_case(value))
    }
}

/// `author:`: sent by people or by bots.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Author {
    User,
    Bot,
}

impl Author {
    pub fn as_str(self) -> &'static str {
        match self {
            Author::User => "user",
            Author::Bot => "bot",
        }
    }

    pub fn parse(value: &str) -> Option<Author> {
        match value.to_ascii_lowercase().as_str() {
            "user" | "person" | "people" => Some(Author::User),
            "bot" | "bots" => Some(Author::Bot),
            _ => None,
        }
    }
}

/// Everything a search is narrowed by besides its text.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Filters {
    /// Member IDs.
    pub from: Vec<String>,
    /// Member IDs.
    pub mentions: Vec<String>,
    /// Conversation IDs.
    #[serde(rename = "in")]
    pub in_: Vec<String>,
    pub has: Vec<Has>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub on: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author: Option<Author>,
}

/// The token keys, in the order the field offers them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Key {
    From,
    Mentions,
    In,
    Has,
    Before,
    After,
    On,
    Author,
}

impl Key {
    pub const ALL: [Key; 8] = [
        Key::From,
        Key::Mentions,
        Key::In,
        Key::Has,
        Key::Before,
        Key::After,
        Key::On,
        Key::Author,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Key::From => "from",
            Key::Mentions => "mentions",
            Key::In => "in",
            Key::Has => "has",
            Key::Before => "before",
            Key::After => "after",
            Key::On => "on",
            Key::Author => "author",
        }
    }

    pub fn parse(value: &str) -> Option<Key> {
        Key::ALL
            .into_iter()
            .find(|k| k.as_str().eq_ignore_ascii_case(value))
    }

    /// Whether the value is a calendar day.
    pub fn is_date(self) -> bool {
        matches!(self, Key::Before | Key::After | Key::On)
    }
}

/// One filter value, shown as a removable chip.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Chip {
    From(String),
    Mentions(String),
    In(String),
    Has(Has),
    Before(String),
    After(String),
    On(String),
    Author(Author),
}

impl Chip {
    pub fn key(&self) -> Key {
        match self {
            Chip::From(_) => Key::From,
            Chip::Mentions(_) => Key::Mentions,
            Chip::In(_) => Key::In,
            Chip::Has(_) => Key::Has,
            Chip::Before(_) => Key::Before,
            Chip::After(_) => Key::After,
            Chip::On(_) => Key::On,
            Chip::Author(_) => Key::Author,
        }
    }

    /// The ID, kind, day or author it holds.
    pub fn value(&self) -> &str {
        match self {
            Chip::From(v)
            | Chip::Mentions(v)
            | Chip::In(v)
            | Chip::Before(v)
            | Chip::After(v)
            | Chip::On(v) => v,
            Chip::Has(h) => h.as_str(),
            Chip::Author(a) => a.as_str(),
        }
    }
}

impl Filters {
    /// How many values are set.
    pub fn count(&self) -> usize {
        self.from.len()
            + self.mentions.len()
            + self.in_.len()
            + self.has.len()
            + [&self.before, &self.after, &self.on]
                .iter()
                .filter(|d| d.is_some())
                .count()
            + usize::from(self.author.is_some())
    }

    pub fn is_empty(&self) -> bool {
        self.count() == 0
    }

    /// One chip per value, in the order the field shows them.
    pub fn chips(&self) -> Vec<Chip> {
        let mut chips: Vec<Chip> = Vec::new();
        chips.extend(self.from.iter().cloned().map(Chip::From));
        chips.extend(self.mentions.iter().cloned().map(Chip::Mentions));
        chips.extend(self.in_.iter().cloned().map(Chip::In));
        chips.extend(self.has.iter().copied().map(Chip::Has));
        chips.extend(self.before.clone().map(Chip::Before));
        chips.extend(self.after.clone().map(Chip::After));
        chips.extend(self.on.clone().map(Chip::On));
        chips.extend(self.author.map(Chip::Author));
        chips
    }

    pub fn without(&self, chip: &Chip) -> Filters {
        let mut next = self.clone();
        match chip {
            Chip::From(v) => next.from.retain(|x| x != v),
            Chip::Mentions(v) => next.mentions.retain(|x| x != v),
            Chip::In(v) => next.in_.retain(|x| x != v),
            Chip::Has(h) => next.has.retain(|x| x != h),
            Chip::Before(_) => next.before = None,
            Chip::After(_) => next.after = None,
            Chip::On(_) => next.on = None,
            Chip::Author(_) => next.author = None,
        }
        next
    }

    /// Add `chip`, once. A single day replaces a range and a range replaces
    /// a single day.
    pub fn with(&self, chip: Chip) -> Filters {
        let mut next = self.without(&chip);
        match chip {
            Chip::From(v) => next.from.push(v),
            Chip::Mentions(v) => next.mentions.push(v),
            Chip::In(v) => next.in_.push(v),
            Chip::Has(h) => next.has.push(h),
            Chip::Author(a) => next.author = Some(a),
            Chip::On(v) => {
                next.on = Some(v);
                next.before = None;
                next.after = None;
            }
            Chip::Before(v) => {
                next.before = Some(v);
                next.on = None;
            }
            Chip::After(v) => {
                next.after = Some(v);
                next.on = None;
            }
        }
        next
    }

    /// The same filters searching only `conversation` (a conversation's own
    /// search, where there is no choice of where).
    pub fn scoped(&self, conversation: &str) -> Filters {
        Filters {
            in_: vec![conversation.to_string()],
            ..self.clone()
        }
    }
}

/// A filter token being typed: `from:jul` is key `From`, value `jul`,
/// starting at byte `start`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Token {
    pub key: Key,
    pub value: String,
    pub start: usize,
}

/// A `key:value` word at `start`, if `word` is one.
fn token_at(word: &str, start: usize) -> Option<Token> {
    let (key, value) = word.split_once(':')?;
    Some(Token {
        key: Key::parse(key)?,
        value: value.to_string(),
        start,
    })
}

/// The token being typed at the end of `text`, such as `from:jul`.
pub fn active_token(text: &str) -> Option<Token> {
    if text.ends_with(char::is_whitespace) {
        return None;
    }
    let start = text.rfind(char::is_whitespace).map_or(0, |i| {
        i + text[i..].chars().next().map_or(1, char::len_utf8)
    });
    token_at(&text[start..], start)
}

/// A token just finished by a space: `has:image ` → key `Has`, value `image`.
pub fn finished_token(text: &str) -> Option<Token> {
    let trimmed = text.trim_end();
    if trimmed.len() == text.len() || trimmed.is_empty() {
        return None;
    }
    active_token(trimmed).filter(|t| !t.value.is_empty())
}

/// What has been typed, without a filter token still being written.
pub fn typed_text(text: &str) -> &str {
    match active_token(text) {
        Some(token) => text[..token.start].trim(),
        None => text.trim(),
    }
}

/// `text` without the token at `token.start` and what follows it.
pub fn without_token<'a>(text: &'a str, token: &Token) -> &'a str {
    &text[..token.start]
}

/// A calendar day, `YYYY-MM-DD`.
pub fn is_day(value: &str) -> bool {
    parse_day(value).is_some()
}

pub fn parse_day(value: &str) -> Option<NaiveDate> {
    let b = value.as_bytes();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return None;
    }
    NaiveDate::parse_from_str(value, "%Y-%m-%d").ok()
}

/// `days` before `today`, as a filter value.
pub fn days_before(today: NaiveDate, days: u64) -> String {
    today
        .checked_sub_days(Days::new(days))
        .unwrap_or(today)
        .format("%Y-%m-%d")
        .to_string()
}

/// Two characters of text, or any filter, make a search.
pub fn can_search(text: &str, filters: &Filters) -> bool {
    text.trim().chars().count() >= 2 || !filters.is_empty()
}

/// Search retained messages, or ask the IRC server's own history.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Source {
    #[default]
    Retained,
    Upstream,
}

impl Source {
    pub fn as_str(self) -> &'static str {
        match self {
            Source::Retained => "retained",
            Source::Upstream => "upstream",
        }
    }
}

/// Server history can be searched in exactly one conversation; anything
/// else is retained.
pub fn effective_source(requested: Source, conversations: usize) -> Source {
    if requested == Source::Upstream && conversations == 1 {
        Source::Upstream
    } else {
        Source::Retained
    }
}

/// One request to `GET /api/v1/search`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Query {
    pub text: String,
    pub filters: Filters,
    pub cursor: Option<String>,
    /// What was asked for; [`Query::pairs`] sends what applies.
    pub source: Source,
    pub limit: Option<u32>,
}

impl Query {
    pub fn new(text: impl Into<String>, filters: Filters, source: Source) -> Self {
        Self {
            text: text.into(),
            filters,
            source,
            ..Default::default()
        }
    }

    /// The query pairs, with days at local midnight.
    pub fn pairs(&self) -> Vec<(&'static str, String)> {
        self.pairs_in(&Local)
    }

    /// The query pairs, with days at midnight in `zone`. Lists repeat their
    /// key (`from=a&from=b`), as the backend reads them.
    pub fn pairs_in<Tz: TimeZone>(&self, zone: &Tz) -> Vec<(&'static str, String)> {
        let f = &self.filters;
        let mut pairs: Vec<(&'static str, String)> = Vec::new();
        let text = self.text.trim();
        if !text.is_empty() {
            pairs.push(("q", text.to_string()));
        }
        pairs.extend(f.from.iter().map(|v| ("from", v.clone())));
        pairs.extend(f.mentions.iter().map(|v| ("mentions", v.clone())));
        pairs.extend(f.in_.iter().map(|v| ("conversationId", v.clone())));
        pairs.extend(f.has.iter().map(|v| ("has", v.as_str().to_string())));
        let day = |value: &Option<String>, next: bool| {
            value
                .as_deref()
                .and_then(parse_day)
                .and_then(|d| midnight(d, next, zone))
        };
        let (before, after) = if f.on.is_some() {
            (day(&f.on, true), day(&f.on, false))
        } else {
            (day(&f.before, false), day(&f.after, true))
        };
        pairs.extend(before.map(|v| ("before", v)));
        pairs.extend(after.map(|v| ("after", v)));
        pairs.extend(f.author.map(|a| ("author", a.as_str().to_string())));
        pairs.extend(self.cursor.clone().map(|c| ("cursor", c)));
        pairs.extend(self.limit.map(|l| ("limit", l.to_string())));
        let source = effective_source(self.source, f.in_.len());
        pairs.push(("source", source.as_str().to_string()));
        pairs
    }
}

/// The instant `day` (or the day after it) starts in `zone`, in UTC.
fn midnight<Tz: TimeZone>(day: NaiveDate, next: bool, zone: &Tz) -> Option<String> {
    let day = if next { day.succ_opt()? } else { day };
    let start = zone
        .from_local_datetime(&day.and_hms_opt(0, 0, 0)?)
        .earliest()?;
    Some(
        start
            .with_timezone(&chrono::Utc)
            .to_rfc3339_opts(SecondsFormat::Secs, true),
    )
}

/// What a page of results says about how complete it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Coverage {
    /// Results are limited to messages saved on this device.
    Offline,
    /// Retained messages only; server history may hold more.
    Retained,
    /// The server's own history: a bounded set.
    Upstream,
    /// Server history was asked for, but retained messages were searched.
    Fallback(c::SearchSourceReason),
}

/// The note to show over a page of results, if any.
pub fn coverage(page: &c::SearchPage) -> Option<Coverage> {
    use c::{SearchSourceActual as Actual, SearchSourceCoverage as Covered};
    let source = page.source.as_ref()?;
    if source.completeness != "unknown" {
        return None;
    }
    match (source.actual, source.coverage) {
        (Actual::Upstream, Covered::UpstreamMinusBounded) => Some(Coverage::Upstream),
        (Actual::Retained, Covered::RetainedMinusIndexed) => Some(match source.reason {
            Some(reason) if reason != c::SearchSourceReason::Unknown => Coverage::Fallback(reason),
            _ => Coverage::Retained,
        }),
        _ => None,
    }
}

/// A bounded server-history page: its count is "in this response", not a total.
pub fn is_bounded(page: &c::SearchPage) -> bool {
    coverage(page) == Some(Coverage::Upstream)
}

/// Whether a result can open in the timeline. A hit from the server's own
/// history that IRCord does not retain cannot.
pub fn is_jumpable(result: &c::SearchResult) -> bool {
    result.context.as_ref().is_none_or(|c| c.available)
}

/// Who sent a result: the member's name when known, else the nickname the
/// server gave for an ephemeral hit.
pub fn result_author(result: &c::SearchResult, member: Option<String>) -> Option<String> {
    if member.is_some() {
        return member;
    }
    let ephemeral = result
        .context
        .as_ref()
        .is_some_and(|c| c.identity == c::SearchContextIdentity::Ephemeral);
    if ephemeral {
        result.sender_nick.clone()
    } else {
        None
    }
}

/// A past search, kept on this device to run again.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RecentSearch {
    pub text: String,
    pub filters: Filters,
}

pub const MAX_RECENT: usize = 8;

/// Read recent searches from whatever was stored, dropping what does not fit.
pub fn sanitize_recent(value: &Value) -> Vec<RecentSearch> {
    let Some(list) = value.as_array() else {
        return Vec::new();
    };
    let strings = |v: Option<&Value>| -> Vec<String> {
        v.and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .take(20)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default()
    };
    let day = |v: Option<&Value>| {
        v.and_then(Value::as_str)
            .filter(|d| is_day(d))
            .map(str::to_string)
    };
    list.iter()
        .filter_map(Value::as_object)
        .map(|r| {
            let empty = serde_json::Map::new();
            let f = r
                .get("filters")
                .and_then(Value::as_object)
                .unwrap_or(&empty);
            RecentSearch {
                text: r
                    .get("text")
                    .and_then(Value::as_str)
                    .map(|t| t.chars().take(200).collect())
                    .unwrap_or_default(),
                filters: Filters {
                    from: strings(f.get("from")),
                    mentions: strings(f.get("mentions")),
                    in_: strings(f.get("in")),
                    has: strings(f.get("has"))
                        .iter()
                        .filter_map(|h| Has::parse(h))
                        .collect(),
                    before: day(f.get("before")),
                    after: day(f.get("after")),
                    on: day(f.get("on")),
                    author: f
                        .get("author")
                        .and_then(Value::as_str)
                        .and_then(|a| match a {
                            "user" => Some(Author::User),
                            "bot" => Some(Author::Bot),
                            _ => None,
                        }),
                },
            }
        })
        .filter(|r| !r.text.is_empty() || !r.filters.is_empty())
        .take(MAX_RECENT)
        .collect()
}

/// The device state section that holds recent searches, by account.
const SECTION: &str = "search";

/// The recent searches kept on this device for `account`.
pub fn load_recent(config: &ConfigDir, account: &str) -> Vec<RecentSearch> {
    let all: HashMap<String, Value> = config.section(SECTION);
    all.get(account).map(sanitize_recent).unwrap_or_default()
}

/// Keep `list` as `account`'s recent searches, leaving other accounts' alone.
pub fn save_recent(config: &ConfigDir, account: &str, list: &[RecentSearch]) -> io::Result<()> {
    let mut all: HashMap<String, Value> = config.section(SECTION);
    all.insert(
        account.to_string(),
        serde_json::to_value(list).map_err(io::Error::other)?,
    );
    config.save_section(SECTION, &all)
}

/// `entry` first, without repeating the same search.
pub fn remember(list: &[RecentSearch], entry: RecentSearch) -> Vec<RecentSearch> {
    let mut next = vec![entry.clone()];
    next.extend(list.iter().filter(|r| **r != entry).cloned());
    next.truncate(MAX_RECENT);
    next
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::FixedOffset;
    use serde_json::json;

    fn pairs(q: &Query) -> Vec<(&'static str, String)> {
        // Central European winter time, so local midnight is 23:00 UTC.
        q.pairs_in(&FixedOffset::east_opt(3600).unwrap())
    }

    fn get<'a>(pairs: &'a [(&'static str, String)], key: &str) -> Vec<&'a str> {
        pairs
            .iter()
            .filter(|(k, _)| *k == key)
            .map(|(_, v)| v.as_str())
            .collect()
    }

    #[test]
    fn counts_chips_in_field_order() {
        let f = Filters {
            from: vec!["m1".into()],
            in_: vec!["c1".into()],
            has: vec![Has::Image],
            on: Some("2026-11-20".into()),
            author: Some(Author::Bot),
            ..Default::default()
        };
        assert_eq!(f.count(), 5);
        assert_eq!(
            f.chips(),
            vec![
                Chip::From("m1".into()),
                Chip::In("c1".into()),
                Chip::Has(Has::Image),
                Chip::On("2026-11-20".into()),
                Chip::Author(Author::Bot),
            ]
        );
        assert!(Filters::default().is_empty());
    }

    #[test]
    fn with_and_without_a_chip() {
        let f = Filters::default()
            .with(Chip::From("a".into()))
            .with(Chip::From("b".into()))
            .with(Chip::From("a".into()));
        assert_eq!(f.from, ["b", "a"], "adding twice keeps one, last");
        let f = f.without(&Chip::From("b".into()));
        assert_eq!(f.from, ["a"]);
        // A single day replaces a range and a range replaces a single day.
        let f = Filters::default()
            .with(Chip::Before("2026-11-10".into()))
            .with(Chip::After("2026-11-01".into()));
        let on = f.with(Chip::On("2026-11-05".into()));
        assert_eq!((&on.before, &on.after), (&None, &None));
        let range = on.with(Chip::After("2026-11-02".into()));
        assert_eq!(range.on, None);
        assert_eq!(range.after.as_deref(), Some("2026-11-02"));
        let author = Filters::default()
            .with(Chip::Author(Author::User))
            .with(Chip::Author(Author::Bot));
        assert_eq!(author.author, Some(Author::Bot));
        assert_eq!(author.without(&Chip::Author(Author::Bot)).author, None);
    }

    #[test]
    fn the_token_being_typed() {
        assert_eq!(
            active_token("hello from:jul"),
            Some(Token {
                key: Key::From,
                value: "jul".into(),
                start: 6
            })
        );
        assert_eq!(active_token("FROM:").map(|t| t.key), Some(Key::From));
        assert_eq!(active_token("has:image "), None, "a space ends it");
        assert_eq!(active_token("hello"), None);
        assert_eq!(active_token("http://x"), None, "not a key");
        assert_eq!(active_token("x:y"), None);
        assert_eq!(typed_text("  hello in:gen"), "hello");
        assert_eq!(typed_text(" hello "), "hello");
        let done = finished_token("cats has:image ").unwrap();
        assert_eq!(
            (done.key, done.value.as_str(), done.start),
            (Key::Has, "image", 5)
        );
        assert_eq!(finished_token("cats has: "), None, "no value yet");
        assert_eq!(finished_token("cats has:image"), None, "still typing");
        assert_eq!(without_token("cats has:image ", &done), "cats ");
        let unicode = active_token("café\u{a0}on:2026").unwrap();
        assert_eq!(&"café\u{a0}on:2026"[unicode.start..], "on:2026");
    }

    #[test]
    fn days() {
        assert!(is_day("2026-02-28"));
        assert!(!is_day("2026-02-30"));
        assert!(!is_day("2026-2-28"));
        assert!(!is_day("yesterday"));
        let today = NaiveDate::from_ymd_opt(2026, 3, 1).unwrap();
        assert_eq!(days_before(today, 1), "2026-02-28");
        assert_eq!(days_before(today, 7), "2026-02-22");
    }

    #[test]
    fn what_the_server_is_asked() {
        let f = Filters {
            from: vec!["m1".into(), "m2".into()],
            mentions: vec!["m3".into()],
            in_: vec!["c1".into()],
            has: vec![Has::Link, Has::File],
            before: Some("2026-11-10".into()),
            after: Some("2026-11-01".into()),
            author: Some(Author::User),
            ..Default::default()
        };
        let q = Query {
            text: "  hello world ".into(),
            filters: f,
            cursor: Some("next".into()),
            ..Default::default()
        };
        let p = pairs(&q);
        assert_eq!(get(&p, "q"), ["hello world"]);
        assert_eq!(get(&p, "from"), ["m1", "m2"]);
        assert_eq!(get(&p, "mentions"), ["m3"]);
        assert_eq!(get(&p, "conversationId"), ["c1"]);
        assert_eq!(get(&p, "has"), ["link", "file"]);
        // before: midnight starting that day; after: the day after it, as the
        // web client asks ("after the 1st" starts on the 2nd).
        assert_eq!(get(&p, "before"), ["2026-11-09T23:00:00Z"]);
        assert_eq!(get(&p, "after"), ["2026-11-01T23:00:00Z"]);
        assert_eq!(get(&p, "author"), ["user"]);
        assert_eq!(get(&p, "cursor"), ["next"]);
        assert_eq!(get(&p, "source"), ["retained"]);

        let on = Query::new(
            "",
            Filters::default().with(Chip::On("2026-11-05".into())),
            Source::Retained,
        );
        let p = pairs(&on);
        assert!(get(&p, "q").is_empty(), "no text, no q");
        assert_eq!(get(&p, "after"), ["2026-11-04T23:00:00Z"]);
        assert_eq!(get(&p, "before"), ["2026-11-05T23:00:00Z"]);
    }

    #[test]
    fn source_resets_to_retained_outside_one_conversation() {
        assert_eq!(effective_source(Source::Upstream, 1), Source::Upstream);
        assert_eq!(effective_source(Source::Upstream, 0), Source::Retained);
        assert_eq!(effective_source(Source::Upstream, 2), Source::Retained);
        assert_eq!(effective_source(Source::Retained, 1), Source::Retained);
        let one = Query::new("hi", Filters::default().scoped("c1"), Source::Upstream);
        assert_eq!(get(&pairs(&one), "source"), ["upstream"]);
        let all = Query::new("hi", Filters::default(), Source::Upstream);
        assert_eq!(get(&pairs(&all), "source"), ["retained"]);
    }

    #[test]
    fn enough_to_search() {
        assert!(!can_search("a", &Filters::default()));
        assert!(!can_search("  a  ", &Filters::default()));
        assert!(can_search("ab", &Filters::default()));
        assert!(
            can_search("é✓", &Filters::default()),
            "characters, not bytes"
        );
        assert!(can_search(
            "",
            &Filters::default().with(Chip::Has(Has::Gif))
        ));
    }

    fn page(source: serde_json::Value) -> c::SearchPage {
        serde_json::from_value(json!({"items": [], "source": source})).unwrap()
    }

    #[test]
    fn coverage_uses_the_reported_source_and_labels_a_fallback() {
        let fallback = page(json!({"requested": "upstream", "actual": "retained",
            "coverage": "retained-indexed", "completeness": "unknown",
            "reason": "upstream-unavailable"}));
        assert_eq!(
            coverage(&fallback),
            Some(Coverage::Fallback(
                c::SearchSourceReason::UpstreamMinusUnavailable
            ))
        );
        let upstream = page(json!({"requested": "upstream", "actual": "upstream",
            "coverage": "upstream-bounded", "completeness": "unknown"}));
        assert_eq!(coverage(&upstream), Some(Coverage::Upstream));
        assert!(is_bounded(&upstream));
        // An unknown reason gets the generic retained note.
        let future = page(json!({"requested": "upstream", "actual": "retained",
            "coverage": "retained-indexed", "completeness": "unknown",
            "reason": "future-reason"}));
        assert_eq!(coverage(&future), Some(Coverage::Retained));
        // An old server without the metadata makes no claim.
        assert_eq!(coverage(&c::SearchPage::default()), None);
        let complete = page(json!({"requested": "retained", "actual": "retained",
            "coverage": "retained-indexed", "completeness": "complete"}));
        assert_eq!(coverage(&complete), None);
    }

    #[test]
    fn ephemeral_hits_cannot_open_and_use_only_the_supplied_nickname() {
        let ephemeral: c::SearchResult = serde_json::from_value(json!({
            "context": {"available": false, "identity": "ephemeral"},
            "senderNick": "Mira",
            "conversation": {}, "message": {}
        }))
        .unwrap();
        assert!(!is_jumpable(&ephemeral));
        assert_eq!(result_author(&ephemeral, None).as_deref(), Some("Mira"));
        assert_eq!(
            result_author(&ephemeral, Some("Bob".into())).as_deref(),
            Some("Bob")
        );
        let mut nameless = ephemeral.clone();
        nameless.sender_nick = None;
        assert_eq!(result_author(&nameless, None), None);
        assert!(
            is_jumpable(&c::SearchResult::default()),
            "no context: jumpable"
        );
        let retained: c::SearchResult = serde_json::from_value(json!({
            "context": {"available": true, "identity": "retained"},
            "conversation": {}, "message": {}
        }))
        .unwrap();
        assert!(is_jumpable(&retained));
        assert_eq!(result_author(&retained, None), None);
    }

    #[test]
    fn recent_searches_are_sanitized() {
        let stored = json!([
            {"text": "hello", "filters": {"from": ["m1", 4], "has": ["image", "sound"],
                "on": "2026-11-31", "before": "2026-11-20", "author": "robot"}},
            {"text": "", "filters": {}},
            "junk",
            {"filters": {"in": ["c1"]}},
            {"text": "x".repeat(300)}
        ]);
        let recent = sanitize_recent(&stored);
        assert_eq!(recent.len(), 3, "{recent:?}");
        assert_eq!(recent[0].text, "hello");
        assert_eq!(recent[0].filters.from, ["m1"]);
        assert_eq!(recent[0].filters.has, [Has::Image]);
        assert_eq!(recent[0].filters.on, None, "not a real day");
        assert_eq!(recent[0].filters.before.as_deref(), Some("2026-11-20"));
        assert_eq!(recent[0].filters.author, None);
        assert_eq!(recent[1].filters.in_, ["c1"]);
        assert_eq!(recent[2].text.chars().count(), 200);
        assert!(sanitize_recent(&json!({"text": "x"})).is_empty());
        let many: Vec<Value> = (0..20).map(|i| json!({"text": format!("q{i}")})).collect();
        assert_eq!(sanitize_recent(&Value::Array(many)).len(), MAX_RECENT);
    }

    #[test]
    fn recent_searches_round_trip_through_storage() {
        let entry = RecentSearch {
            text: "cats".into(),
            filters: Filters::default()
                .with(Chip::Has(Has::Gif))
                .with(Chip::Author(Author::Bot))
                .with(Chip::After("2026-01-02".into())),
        };
        let stored = serde_json::to_value(vec![entry.clone()]).unwrap();
        assert_eq!(sanitize_recent(&stored), [entry]);
    }

    #[test]
    fn remembering_puts_newest_first_without_repeats() {
        let a = RecentSearch {
            text: "a".into(),
            ..Default::default()
        };
        let b = RecentSearch {
            text: "b".into(),
            ..Default::default()
        };
        let list = remember(&[], a.clone());
        let list = remember(&list, b.clone());
        let list = remember(&list, a.clone());
        assert_eq!(list, [a.clone(), b]);
        let mut list = Vec::new();
        for i in 0..12 {
            list = remember(
                &list,
                RecentSearch {
                    text: format!("q{i}"),
                    ..Default::default()
                },
            );
        }
        assert_eq!(list.len(), MAX_RECENT);
        assert_eq!(list[0].text, "q11");
    }

    #[test]
    fn recent_searches_are_kept_per_account() {
        let dir = tempfile::tempdir().unwrap();
        let config = ConfigDir::at(dir.path());
        assert!(load_recent(&config, "u1").is_empty());
        let mine = vec![RecentSearch {
            text: "cats".into(),
            ..Default::default()
        }];
        save_recent(&config, "u1", &mine).unwrap();
        save_recent(&config, "u2", &[]).unwrap();
        assert_eq!(load_recent(&config, "u1"), mine);
        assert!(load_recent(&config, "u2").is_empty());
    }

    #[test]
    fn keys_and_kinds_parse() {
        assert_eq!(Key::parse("Mentions"), Some(Key::Mentions));
        assert_eq!(Key::parse("nope"), None);
        assert!(Key::On.is_date() && !Key::Has.is_date());
        assert_eq!(Has::parse("GIF"), Some(Has::Gif));
        assert_eq!(Author::parse("bots"), Some(Author::Bot));
        assert_eq!(Chip::Has(Has::Video).value(), "video");
        assert_eq!(Chip::In("c1".into()).key(), Key::In);
    }
}

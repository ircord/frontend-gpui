//! Shared links: `/links`, and link media on a message (`/conversations/{id}/messages/{messageId}/link-media`, used by L1).
//!
//! Owned by package FD3. The links view and the channel gallery read the
//! server's link catalog with [`Api::list_links`]: links from your own
//! stored channel history, grouped by what they point to, newest post
//! first. The rest of this file is what those views decide without a
//! window, after `shared/utils/link-catalog.ts` in the web client: the
//! filter and its query, the `in:`/`type:`/`host:`… tokens typed into the
//! filter box, the host and date checks, a card's title, and paging.

use chrono::{NaiveDate, SecondsFormat, TimeZone};
use ircord_contract as c;
use ircord_contract::{LinkCard, LinkCardKind, LinkSource};

#[allow(unused_imports)]
use super::{Api, ApiError, Method, segment};

/// Cards asked for per page, as the web client does.
pub const PAGE: i64 = 30;
/// The server keeps at most this many channels in one filter.
pub const MAX_CHANNELS: usize = 50;

/// Every kind the catalog sorts links into, in the order filters list them.
pub const KINDS: [LinkCardKind; 6] = [
    LinkCardKind::Link,
    LinkCardKind::Image,
    LinkCardKind::Gif,
    LinkCardKind::Video,
    LinkCardKind::Audio,
    LinkCardKind::Youtube,
];

/// The kinds the channel gallery shows.
pub const PICTURES: [LinkCardKind; 2] = [LinkCardKind::Image, LinkCardKind::Gif];

impl Api {
    /// What the server found behind `url`, a link in the text of a message
    /// you can read: media from its cache, a file, a card or a player.
    /// Content it could not or would not fetch comes back as a card with a
    /// reason, not as an error.
    pub async fn resolve_link_media(
        &self,
        conversation: &str,
        message: &str,
        url: &str,
    ) -> Result<c::LinkMediaResponse, ApiError> {
        let path = format!(
            "/api/v1/conversations/{}/messages/{}/link-media",
            segment(conversation),
            segment(message)
        );
        let body = c::LinkMediaInputBody {
            url: url.to_string(),
            widget: None,
        };
        self.call(Method::POST, &path, &body).await
    }

    /// One page of the link catalog for `filter`, from `cursor` (a page's
    /// `next_cursor`) or from the newest.
    pub async fn list_links(
        &self,
        filter: &LinkFilter,
        cursor: Option<&str>,
    ) -> Result<c::LinkPage, ApiError> {
        self.get_query("/api/v1/links", &filter.query(cursor, PAGE))
            .await
    }
}

/// What the links view (or the gallery) asks the catalog for.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct LinkFilter {
    /// Channels to keep links from; none keeps every channel.
    pub conversations: Vec<String>,
    /// A network to keep links from, used only when no channel is chosen.
    pub network: String,
    pub kinds: Vec<LinkCardKind>,
    /// Text the link or its message contains.
    pub q: String,
    /// A host already checked by [`host_filter`].
    pub host: String,
    /// RFC 3339 times from [`date_range`].
    pub since: Option<String>,
    pub until: Option<String>,
}

impl LinkFilter {
    /// The links of one channel's pictures, for its gallery.
    pub fn pictures(conversation: &str) -> Self {
        Self {
            conversations: vec![conversation.to_string()],
            kinds: PICTURES.to_vec(),
            ..Self::default()
        }
    }

    /// The query string, with repeated `conversation` and `kind`.
    pub fn query(&self, cursor: Option<&str>, limit: i64) -> Vec<(&'static str, String)> {
        let mut q: Vec<(&'static str, String)> = Vec::new();
        for id in &self.conversations {
            q.push(("conversation", id.clone()));
        }
        if self.conversations.is_empty() && !self.network.is_empty() {
            q.push(("network", self.network.clone()));
        }
        for kind in &self.kinds {
            if let Some(value) = kind_value(*kind) {
                q.push(("kind", value.to_string()));
            }
        }
        for (key, value) in [("q", &self.q), ("host", &self.host)] {
            if !value.is_empty() {
                q.push((key, value.clone()));
            }
        }
        for (key, value) in [("since", &self.since), ("until", &self.until)] {
            if let Some(value) = value {
                q.push((key, value.clone()));
            }
        }
        if let Some(cursor) = cursor {
            q.push(("cursor", cursor.to_string()));
        }
        q.push(("limit", limit.to_string()));
        q
    }

    /// Whether anything beyond the channels narrows the list, so an empty
    /// list means "nothing matches" rather than "nothing posted".
    pub fn narrowed(&self) -> bool {
        !self.q.is_empty()
            || !self.kinds.is_empty()
            || (self.conversations.is_empty() && !self.network.is_empty())
            || !self.host.is_empty()
            || self.since.is_some()
            || self.until.is_some()
    }
}

/// The value the server knows a kind by.
pub fn kind_value(kind: LinkCardKind) -> Option<&'static str> {
    Some(match kind {
        LinkCardKind::Link => "link",
        LinkCardKind::Image => "image",
        LinkCardKind::Gif => "gif",
        LinkCardKind::Video => "video",
        LinkCardKind::Audio => "audio",
        LinkCardKind::Youtube => "youtube",
        LinkCardKind::Unknown => return None,
    })
}

/// A kind from what is typed after `type:`: the server's value or the
/// plural the filters show (`images`, `pages`).
pub fn parse_kind(typed: &str) -> Option<LinkCardKind> {
    let typed = typed.trim().to_lowercase();
    KINDS.into_iter().find(|k| {
        let value = kind_value(*k).unwrap_or_default();
        typed == value
            || typed == format!("{value}s")
            || (*k == LinkCardKind::Link && (typed == "page" || typed == "pages"))
    })
}

/// Why a typed host cannot be used.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HostError;

/// `example.com` from what was typed as a host: a name, an IP address or a
/// whole `https://` link. Empty input keeps every host.
pub fn host_filter(raw: &str) -> Result<String, HostError> {
    let mut host = raw.trim().to_string();
    if host.is_empty() {
        return Ok(host);
    }
    let lower = host.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        host = url::Url::parse(&host)
            .ok()
            .and_then(|u| u.host_str().map(str::to_string))
            .ok_or(HostError)?;
    }
    if let Some(inner) = host.strip_prefix('[').and_then(|h| h.strip_suffix(']')) {
        host = inner.to_string();
    }
    if let Some(trimmed) = host.strip_suffix('.') {
        host = trimmed.to_string();
    }
    if host.is_empty() || host.len() > 253 {
        return Err(HostError);
    }
    if host.contains(':') {
        let shaped = host
            .chars()
            .all(|ch| ch.is_ascii_hexdigit() || ch == ':' || ch == '.');
        return if shaped && host.parse::<std::net::Ipv6Addr>().is_ok() {
            Ok(host.to_lowercase())
        } else {
            Err(HostError)
        };
    }
    let label_ok = |label: &str| {
        let chars: Vec<char> = label.chars().collect();
        let inner =
            |ch: char| ch.is_alphanumeric() || ch == '-' || (!ch.is_ascii() && !ch.is_whitespace());
        !chars.is_empty()
            && chars.len() <= 63
            && chars[0].is_alphanumeric()
            && chars.iter().all(|ch| inner(*ch))
            && chars.last().is_some_and(|ch| *ch != '-')
    };
    if !host.split('.').all(label_ok) || url::Url::parse(&format!("http://{host}/")).is_err() {
        return Err(HostError);
    }
    Ok(host.to_lowercase())
}

/// Why typed days cannot be used.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DateError {
    /// Not a day written as `2026-11-08`.
    Format,
    /// The last day is before the first.
    Order,
}

/// The times from the start of day `from` to the end of day `to` (both
/// included) in the local time zone; either may be empty.
pub fn date_range(from: &str, to: &str) -> Result<(Option<String>, Option<String>), DateError> {
    date_range_in(from, to, &chrono::Local)
}

/// [`date_range`] in the time zone `tz`.
pub fn date_range_in<Tz: TimeZone>(
    from: &str,
    to: &str,
    tz: &Tz,
) -> Result<(Option<String>, Option<String>), DateError> {
    let day =
        |text: &str, offset: i64| -> Result<Option<chrono::DateTime<chrono::Utc>>, DateError> {
            let text = text.trim();
            if text.is_empty() {
                return Ok(None);
            }
            let shaped = text.len() == 10
                && text.char_indices().all(|(i, ch)| {
                    if i == 4 || i == 7 {
                        ch == '-'
                    } else {
                        ch.is_ascii_digit()
                    }
                });
            let date = shaped
                .then(|| NaiveDate::parse_from_str(text, "%Y-%m-%d").ok())
                .flatten()
                .and_then(|d| d.checked_add_signed(chrono::Duration::days(offset)))
                .ok_or(DateError::Format)?;
            let midnight = date.and_hms_opt(0, 0, 0).ok_or(DateError::Format)?;
            let local = tz
                .from_local_datetime(&midnight)
                .earliest()
                .ok_or(DateError::Format)?;
            Ok(Some(local.with_timezone(&chrono::Utc)))
        };
    let start = day(from, 0)?;
    let end = day(to, 1)?;
    if let (Some(start), Some(end)) = (start, end)
        && end <= start
    {
        return Err(DateError::Order);
    }
    let text = |t: chrono::DateTime<chrono::Utc>| t.to_rfc3339_opts(SecondsFormat::Millis, true);
    Ok((start.map(text), end.map(text)))
}

/// Whether a list ending at `until` can still gain posts.
pub fn range_open(until: Option<&str>, now_ms: i64) -> bool {
    until.is_none_or(|until| {
        chrono::DateTime::parse_from_rfc3339(until).map_or(true, |t| t.timestamp_millis() > now_ms)
    })
}

/// `example.com/page?x=1`: the link without its scheme and `www.`, for a
/// card whose page has no title.
pub fn link_title(url: &str) -> String {
    let Ok(parsed) = url::Url::parse(url) else {
        return url.to_string();
    };
    let Some(host) = parsed.host_str() else {
        return url.to_string();
    };
    let path = if parsed.path() == "/" {
        String::new()
    } else {
        percent_decode(parsed.path())
    };
    let query = parsed.query().map(|q| format!("?{q}")).unwrap_or_default();
    format!("{}{path}{query}", host.strip_prefix("www.").unwrap_or(host))
}

fn percent_decode(path: &str) -> String {
    let bytes = path.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let escaped = (bytes[i] == b'%')
            .then(|| bytes.get(i + 1..i + 3))
            .flatten()
            .and_then(|hex| std::str::from_utf8(hex).ok())
            .and_then(|hex| u8::from_str_radix(hex, 16).ok());
        if let Some(byte) = escaped {
            out.push(byte);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).unwrap_or_else(|_| path.to_string())
}

/// Only `http(s)` links are opened from a card.
pub fn openable(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    (lower.starts_with("http://") || lower.starts_with("https://"))
        && url.len() > "https://".len()
        && !url.chars().any(char::is_whitespace)
}

/// What a post said besides its links, for the line under a card's title.
pub fn remark(excerpt: &str) -> String {
    let words: Vec<&str> = excerpt
        .split_whitespace()
        .filter(|w| {
            let lower = w.to_ascii_lowercase();
            !(lower.starts_with("http://") || lower.starts_with("https://"))
        })
        .collect();
    words
        .join(" ")
        .trim_end_matches(|ch: char| ch.is_whitespace() || matches!(ch, ':' | '–' | '—' | '-'))
        .trim()
        .to_string()
}

/// Add an older page's cards to those listed, leaving out any already
/// listed (a link reposted while paging moves to the top).
pub fn merge(items: &mut Vec<LinkCard>, page: Vec<LinkCard>) {
    for card in page {
        if !items.iter().any(|c| c.identity == card.identity) {
            items.push(card);
        }
    }
}

/// Why a source's media is covered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cover {
    /// Its channel is marked as sensitive.
    Nsfw,
    /// Its channel's classification is not known.
    Unknown,
}

/// Whether a source's media is covered until shown: `allowed` when your
/// settings show sensitive media on its network.
pub fn cover(source: &LinkSource, allowed: bool) -> Option<Cover> {
    if allowed {
        None
    } else if !source.nsfw_known {
        Some(Cover::Unknown)
    } else if source.nsfw {
        Some(Cover::Nsfw)
    } else {
        None
    }
}

/// A filter typed into the links filter box as `key:value`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Token {
    /// `in:#general`: a channel by name.
    In(String),
    /// `network:libera`: a network by name.
    Network(String),
    /// `type:images`.
    Type(String),
    /// `host:example.com` (or `site:`).
    Host(String),
    /// `since:2026-11-08`.
    Since(String),
    /// `until:2026-11-08`.
    Until(String),
}

/// The keys the filter box knows, as typed before the colon.
pub const TOKEN_KEYS: [&str; 6] = ["in", "network", "type", "host", "since", "until"];

fn token(word: &str) -> Option<Token> {
    let (key, value) = word.split_once(':')?;
    if value.is_empty() {
        return None;
    }
    let value = value.to_string();
    Some(match key.to_ascii_lowercase().as_str() {
        "in" => Token::In(value),
        "network" => Token::Network(value),
        "type" => Token::Type(value),
        "host" | "site" => Token::Host(value),
        "since" => Token::Since(value),
        "until" => Token::Until(value),
        _ => return None,
    })
}

/// What is typed in the filter box: the finished `key:value` tokens, each
/// followed by a space, and the text to search for. A token still being
/// written at the end (`type:im`) is in neither, so the list does not jump
/// while it is typed. `rest` is the box's text without the finished tokens.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Typed {
    pub tokens: Vec<Token>,
    pub text: String,
    pub rest: String,
}

pub fn parse_typed(input: &str) -> Typed {
    let finished = input.is_empty() || input.ends_with(char::is_whitespace);
    let words: Vec<&str> = input.split_whitespace().collect();
    let pending = !finished && words.last().is_some_and(|w| is_key(w));
    let done = if pending {
        &words[..words.len() - 1]
    } else {
        &words[..]
    };
    let mut typed = Typed::default();
    let mut kept: Vec<&str> = Vec::new();
    for word in done {
        match token(word) {
            Some(t) => typed.tokens.push(t),
            None => kept.push(word),
        }
    }
    typed.text = kept.join(" ");
    if pending {
        kept.extend(words.last());
    }
    typed.rest = kept.join(" ");
    if finished && !typed.rest.is_empty() {
        typed.rest.push(' ');
    }
    typed
}

/// Whether `word` starts with a key the filter box knows.
fn is_key(word: &str) -> bool {
    word.split_once(':').is_some_and(|(key, _)| {
        let key = key.to_ascii_lowercase();
        TOKEN_KEYS.contains(&key.as_str()) || key == "site"
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_query_repeats_channels_and_kinds() {
        let filter = LinkFilter {
            conversations: vec!["c1".into(), "c2".into()],
            network: "n1".into(),
            kinds: vec![LinkCardKind::Image, LinkCardKind::Gif],
            q: "cats".into(),
            host: "example.com".into(),
            since: Some("2026-11-08T00:00:00.000Z".into()),
            until: None,
        };
        let q = filter.query(Some("next"), 30);
        assert_eq!(
            q,
            vec![
                ("conversation", "c1".to_string()),
                ("conversation", "c2".to_string()),
                ("kind", "image".to_string()),
                ("kind", "gif".to_string()),
                ("q", "cats".to_string()),
                ("host", "example.com".to_string()),
                ("since", "2026-11-08T00:00:00.000Z".to_string()),
                ("cursor", "next".to_string()),
                ("limit", "30".to_string()),
            ],
            "a network is left out when channels are chosen"
        );
        let all = LinkFilter {
            network: "n1".into(),
            ..Default::default()
        };
        assert!(all.query(None, 30).contains(&("network", "n1".to_string())));
        assert!(all.narrowed());
        assert!(LinkFilter::pictures("c1").narrowed());
        assert!(!LinkFilter::default().narrowed());
    }

    #[test]
    fn kinds_parse_from_values_and_plurals() {
        assert_eq!(parse_kind("images"), Some(LinkCardKind::Image));
        assert_eq!(parse_kind("GIF"), Some(LinkCardKind::Gif));
        assert_eq!(parse_kind("pages"), Some(LinkCardKind::Link));
        assert_eq!(parse_kind("youtube"), Some(LinkCardKind::Youtube));
        assert_eq!(parse_kind("pictures"), None);
    }

    #[test]
    fn hosts_are_checked_like_the_web_client() {
        assert_eq!(host_filter(""), Ok(String::new()));
        assert_eq!(host_filter(" Example.COM "), Ok("example.com".into()));
        assert_eq!(
            host_filter("https://www.example.com/a?b"),
            Ok("www.example.com".into())
        );
        assert_eq!(host_filter("example.com."), Ok("example.com".into()));
        assert_eq!(host_filter("[::1]"), Ok("::1".into()));
        assert_eq!(host_filter("bücher.de"), Ok("bücher.de".into()));
        assert_eq!(host_filter("exa mple.com"), Err(HostError));
        assert_eq!(host_filter("-bad.com"), Err(HostError));
        assert_eq!(host_filter("bad-.com"), Err(HostError));
        assert_eq!(host_filter("a..b"), Err(HostError));
        assert_eq!(host_filter("zz::zz"), Err(HostError));
        assert_eq!(host_filter(&"a".repeat(254)), Err(HostError));
    }

    #[test]
    fn days_become_a_range_including_both() {
        let utc = chrono::Utc;
        assert_eq!(date_range_in("", "", &utc), Ok((None, None)));
        assert_eq!(
            date_range_in("2026-11-08", "2026-11-08", &utc),
            Ok((
                Some("2026-11-08T00:00:00.000Z".into()),
                Some("2026-11-09T00:00:00.000Z".into())
            ))
        );
        let plus_two = chrono::FixedOffset::east_opt(2 * 3600).unwrap();
        assert_eq!(
            date_range_in("2026-11-08", "", &plus_two),
            Ok((Some("2026-11-07T22:00:00.000Z".into()), None))
        );
        assert_eq!(date_range_in("8.11.2026", "", &utc), Err(DateError::Format));
        assert_eq!(
            date_range_in("2026-02-31", "", &utc),
            Err(DateError::Format)
        );
        assert_eq!(
            date_range_in("2026-11-09", "2026-11-08", &utc),
            Err(DateError::Order)
        );
    }

    #[test]
    fn a_range_that_ended_gains_no_posts() {
        assert!(range_open(None, 0));
        assert!(range_open(Some("2026-11-09T00:00:00.000Z"), 0));
        assert!(!range_open(Some("2026-11-09T00:00:00.000Z"), i64::MAX));
    }

    #[test]
    fn titles_drop_the_scheme_and_www() {
        assert_eq!(link_title("https://www.example.com/"), "example.com");
        assert_eq!(
            link_title("https://example.com/a%20b?x=1"),
            "example.com/a b?x=1"
        );
        assert_eq!(link_title("not a link"), "not a link");
    }

    #[test]
    fn only_web_links_open() {
        assert!(openable("https://example.com/x"));
        assert!(openable("HTTP://example.com"));
        assert!(!openable("javascript:alert(1)"));
        assert!(!openable("https://"));
        assert!(!openable("https://a b"));
    }

    #[test]
    fn remarks_leave_out_the_links() {
        assert_eq!(
            remark("look at this: https://example.com/x"),
            "look at this"
        );
        assert_eq!(remark("https://example.com"), "");
        assert_eq!(remark("a https://x.y b"), "a b");
    }

    #[test]
    fn older_pages_skip_cards_already_listed() {
        let card = |id: &str| LinkCard {
            identity: id.into(),
            ..Default::default()
        };
        let mut items = vec![card("a"), card("b")];
        merge(&mut items, vec![card("b"), card("c")]);
        let ids: Vec<_> = items.iter().map(|c| c.identity.as_str()).collect();
        assert_eq!(ids, ["a", "b", "c"]);
    }

    #[test]
    fn sensitive_sources_are_covered_unless_allowed() {
        let source = |known: bool, nsfw: bool| LinkSource {
            nsfw_known: known,
            nsfw,
            ..Default::default()
        };
        assert_eq!(cover(&source(true, false), false), None);
        assert_eq!(cover(&source(true, true), false), Some(Cover::Nsfw));
        assert_eq!(cover(&source(false, false), false), Some(Cover::Unknown));
        assert_eq!(cover(&source(true, true), true), None);
    }

    #[test]
    fn typed_tokens_are_taken_once_finished() {
        let t = parse_typed("cats in:#general type:images ");
        assert_eq!(
            t.tokens,
            vec![Token::In("#general".into()), Token::Type("images".into())]
        );
        assert_eq!(t.text, "cats");
        assert_eq!(t.rest, "cats ");

        let t = parse_typed("cats type:im");
        assert!(t.tokens.is_empty(), "a token being written is not taken");
        assert_eq!(t.text, "cats", "nor searched for");
        assert_eq!(t.rest, "cats type:im");

        let t = parse_typed("host:example.com dogs");
        assert_eq!(t.tokens, vec![Token::Host("example.com".into())]);
        assert_eq!(t.text, "dogs");

        let t = parse_typed("ratio 16:9 ");
        assert!(t.tokens.is_empty(), "unknown keys are text");
        assert_eq!(t.text, "ratio 16:9");
    }
}

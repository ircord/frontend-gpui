//! Search the encrypted archive in pages, decrypting candidates as needed.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result, bail};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use ircord_contract::{
    Member, MemberRole, Message, MessageKind, SearchPage, SearchResult, Snapshot,
};
use serde::{Deserialize, Serialize};
use unicode_normalization::{UnicodeNormalization, char::is_combining_mark};
use unicode_script::{Script, UnicodeScript};

use super::{Archive, blocking, retention_start, time_ns};
use crate::format::{blocks::text_outside_code, find_links};
use crate::search_filter::{Author, Filters, Has, Query};

pub(crate) const SEARCH_CURSOR: &str = "offline-search:";
const DEFAULT_LIMIT: u32 = 50;
const MAX_LIMIT: u32 = 100;

struct CancelScan(Arc<AtomicBool>);

impl Drop for CancelScan {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

#[derive(Serialize, Deserialize)]
struct Position {
    at: i64,
    id: String,
    conversation: String,
}

#[derive(Serialize, Deserialize)]
struct Continuation {
    query: String,
    position: Position,
}

impl Archive {
    pub(crate) async fn search(&self, query: &Query) -> Result<Option<SearchPage>> {
        let archive = self.clone();
        let query = query.clone();
        let cancelled = Arc::new(AtomicBool::new(false));
        let _cancel_on_drop = CancelScan(cancelled.clone());
        blocking(move || {
            if cancelled.load(Ordering::Relaxed) {
                return Ok(None);
            }
            archive.read(|db, projection| {
                let snapshot = &projection.snapshot;
                let conversations: HashMap<_, _> = snapshot
                    .conversations
                    .iter()
                    .map(|conversation| (conversation.id.as_str(), conversation))
                    .collect();
                if query
                    .filters
                    .in_
                    .iter()
                    .any(|id| !conversations.contains_key(id.as_str()))
                {
                    return Ok(SearchPage::default());
                }
                let signature = signature(&query)?;
                let boundary = match query.cursor.as_deref() {
                    Some(cursor) => {
                        let Some(position) = position(db, cursor, &signature)? else {
                            return Ok(SearchPage::default());
                        };
                        position
                    }
                    None => Position {
                        at: i64::MAX,
                        id: String::new(),
                        conversation: String::new(),
                    },
                };
                let matcher = Matcher::new(&query, snapshot);
                let scopes = &query.filters.in_;
                let placeholders = vec!["?"; scopes.len()].join(",");
                let scope = if scopes.is_empty() {
                    String::new()
                } else {
                    format!(" AND conversation IN ({placeholders})")
                };
                let sql = format!(
                    "SELECT at,id,conversation,payload FROM messages
                 WHERE (at>=? OR pinned=1) AND at>=? AND at<? AND (at,id,conversation)<(?,?,?){scope}
                 ORDER BY at DESC,id DESC,conversation DESC"
                );
                let cutoff = retention_start();
                let remaining: [&dyn rusqlite::ToSql; 6] = [
                    &cutoff,
                    &matcher.after,
                    &matcher.before,
                    &boundary.at,
                    &boundary.id,
                    &boundary.conversation,
                ];
                let params = remaining
                    .into_iter()
                    .chain(scopes.iter().map(|id| id as &dyn rusqlite::ToSql));
                let mut statement = db.prepare(&sql)?;
                let mut rows = statement.query(rusqlite::params_from_iter(params))?;
                let limit = query.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT) as usize;
                let mut items = Vec::new();
                let mut last = None;
                let mut next_cursor = None;
                while !cancelled.load(Ordering::Relaxed)
                    && let Some(row) = rows.next()?
                {
                    let conversation_id: String = row.get(2)?;
                    let Some(conversation) = conversations.get(conversation_id.as_str()) else {
                        continue;
                    };
                    let message = archive.open::<Message>(b"message", row.get(3)?)?;
                    if !matcher.matches(&message) {
                        continue;
                    }
                    if items.len() == limit {
                        let continuation = Continuation {
                            query: signature,
                            position: last.expect("a nonempty results page has a cursor"),
                        };
                        next_cursor = Some(format!(
                            "{SEARCH_CURSOR}{}",
                            URL_SAFE_NO_PAD.encode(serde_json::to_vec(&continuation)?)
                        ));
                        break;
                    }
                    last = Some(Position {
                        at: row.get(0)?,
                        id: row.get(1)?,
                        conversation: conversation_id,
                    });
                    items.push(SearchResult {
                        message,
                        conversation: (**conversation).clone(),
                        ..Default::default()
                    });
                }
                Ok(SearchPage {
                    items,
                    next_cursor,
                    source: None,
                })
            })
        })
        .await
    }
}

fn signature(query: &Query) -> Result<String> {
    let key = serde_json::to_vec(&(query.text.trim(), &query.filters))?;
    Ok(URL_SAFE_NO_PAD.encode(ring::digest::digest(&ring::digest::SHA256, &key)))
}

fn position(db: &rusqlite::Connection, cursor: &str, signature: &str) -> Result<Option<Position>> {
    use rusqlite::OptionalExtension;

    if let Some(encoded) = cursor.strip_prefix(SEARCH_CURSOR) {
        let continuation: Continuation = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(encoded)?)?;
        if continuation.query != signature {
            bail!("offline search cursor belongs to a different query");
        }
        return Ok(Some(continuation.position));
    }
    let wire: serde_json::Value = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(cursor)?)?;
    // Both server cursor versions carry a stable message ID. Read its archive
    // position so paging can continue when the server disconnects mid-search.
    let id = match wire.get("v").and_then(serde_json::Value::as_u64) {
        Some(1) => wire
            .get("id")
            .and_then(serde_json::Value::as_str)
            .context("missing search ID")?
            .to_owned(),
        Some(2) => {
            let encoded = wire
                .get("i")
                .and_then(serde_json::Value::as_str)
                .context("missing search ID")?;
            String::from_utf8(URL_SAFE_NO_PAD.decode(encoded)?)?
        }
        _ => return Ok(None),
    };
    Ok(db
        .query_row(
            "SELECT at,conversation FROM messages WHERE id=?1 ORDER BY at DESC LIMIT 1",
            [&id],
            |row| {
                Ok(Position {
                    at: row.get(0)?,
                    id: id.clone(),
                    conversation: row.get(1)?,
                })
            },
        )
        .optional()?)
}

struct Matcher<'a> {
    filters: &'a Filters,
    text_required: bool,
    terms: Vec<String>,
    names: Vec<String>,
    members: HashMap<&'a str, &'a Member>,
    after: i64,
    before: i64,
}

impl<'a> Matcher<'a> {
    fn new(query: &'a Query, snapshot: &'a Snapshot) -> Self {
        let members: HashMap<_, _> = snapshot
            .members
            .iter()
            .chain([&snapshot.user])
            .map(|member| (member.id.as_str(), member))
            .collect();
        let names = query
            .filters
            .mentions
            .iter()
            .filter_map(|id| members.get(id.as_str()))
            .filter(|member| !member.nick.is_empty())
            .map(|member| member.nick.to_lowercase())
            .collect();
        let pairs = query.pairs();
        let bound = |key| {
            pairs
                .iter()
                .find(|(name, _)| *name == key)
                .and_then(|(_, at)| time_ns(at))
        };
        Self {
            filters: &query.filters,
            text_required: !query.text.trim().is_empty(),
            terms: tokens(&query.text),
            names,
            members,
            after: bound("after").unwrap_or(i64::MIN),
            before: bound("before").unwrap_or(i64::MAX),
        }
    }

    fn matches(&self, message: &Message) -> bool {
        let filters = self.filters;
        if message.deleted == Some(true)
            || !matches!(
                message.kind,
                MessageKind::Message | MessageKind::Action | MessageKind::Notice
            )
            || (!filters.from.is_empty() && !filters.from.contains(&message.sender_id))
        {
            return false;
        }
        if let Some(author) = filters.author {
            let bot = self
                .members
                .get(message.sender_id.as_str())
                .is_some_and(|member| member.role == MemberRole::Bot);
            if bot != (author == Author::Bot) {
                return false;
            }
        }
        if !filters.mentions.is_empty() {
            let text = message.text.to_lowercase();
            if !self.names.iter().any(|nick| mentions(&text, nick)) {
                return false;
            }
        }
        if !filters.has.is_empty()
            && !content(message)
                .iter()
                .any(|kind| filters.has.contains(kind))
        {
            return false;
        }
        !self.text_required || phrase(&message.text, &self.terms)
    }
}

fn tokens(text: &str) -> Vec<String> {
    let mut folded = String::new();
    let mut latin = false;
    for letter in text.chars().flat_map(char::to_lowercase) {
        if latin && is_combining_mark(letter) {
            continue;
        }
        latin = letter.script() == Script::Latin;
        if latin {
            folded.extend(
                std::iter::once(letter)
                    .nfd()
                    .filter(|c| !is_combining_mark(*c)),
            );
        } else {
            folded.push(letter);
        }
    }
    folded.split(|c: char| !c.is_alphanumeric() && !matches!(c, '\u{e000}'..='\u{f8ff}' | '\u{f0000}'..='\u{ffffd}' | '\u{100000}'..='\u{10fffd}'))
        .filter(|word| !word.is_empty()).map(str::to_owned).collect()
}

fn phrase(text: &str, terms: &[String]) -> bool {
    let Some((last, prefix)) = terms.split_last() else {
        return false;
    };
    tokens(text)
        .windows(terms.len())
        .any(|word| word[..prefix.len()] == *prefix && word[prefix.len()].starts_with(last))
}

fn mentions(text: &str, nick: &str) -> bool {
    let separator = |c: char| !c.is_alphanumeric() && !matches!(c, '_' | '-');
    text.match_indices(nick).any(|(at, _)| {
        text[..at].chars().next_back().is_none_or(separator)
            && text[at + nick.len()..].chars().next().is_none_or(separator)
    })
}

fn content(message: &Message) -> Vec<Has> {
    let mut kinds = Vec::new();
    if message.gif.is_some() {
        kinds.push(Has::Gif);
    }
    for attachment in message
        .attachments
        .as_deref()
        .unwrap_or(message.attachment.as_slice())
    {
        let mime = attachment
            .content_type
            .as_deref()
            .unwrap_or_default()
            .split(';')
            .next()
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase();
        kinds.push(match mime.as_str() {
            "image/gif" => Has::Gif,
            image if image.starts_with("image/") => Has::Image,
            video if video.starts_with("video/") => Has::Video,
            _ => Has::File,
        });
    }
    let visible = text_outside_code(&message.text);
    for (span, _) in find_links(&visible) {
        let Ok(url) = url::Url::parse(&visible[span]) else {
            continue;
        };
        if !matches!(url.scheme(), "https" | "http") {
            continue;
        }
        kinds.push(Has::Link);
        let host = url.host_str().unwrap_or_default();
        let domain = |name: &str| {
            host == name
                || host
                    .strip_suffix(name)
                    .is_some_and(|prefix| prefix.ends_with('.'))
        };
        let path = url.path().to_ascii_lowercase();
        let extension = path.rsplit_once('.').map_or("", |(_, ext)| ext);
        if matches!(extension, "gif" | "apng") || domain("tenor.com") || domain("giphy.com") {
            kinds.push(Has::Gif);
        } else if matches!(extension, "png" | "jpg" | "jpeg" | "webp" | "avif" | "heic") {
            kinds.push(Has::Image);
        } else if matches!(extension, "mp4" | "webm" | "mov" | "m4v")
            || ["youtube.com", "youtu.be", "vimeo.com"]
                .into_iter()
                .any(domain)
        {
            kinds.push(Has::Video);
        }
    }
    kinds
}

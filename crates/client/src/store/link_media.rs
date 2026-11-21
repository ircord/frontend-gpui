//! Link previews and the media found behind links (package L1).
//!
//! Every link in a message's text is resolved once through the server
//! (`Api::resolve_link_media`), which answers with media from its cache, a
//! file, a card or a player. A link to a message in this app is instead
//! looked up in your own history (`Api::locate_message`). Answers are kept
//! here per message and link, so the timeline draws each from the store;
//! media whose cached copy expires is asked for again. The rules for what a
//! post card shows follow `shared/utils/link-media.ts` in the web client.

use std::collections::HashMap;
use std::time::Duration;

use ircord_contract::event::ServerEvent;
use ircord_contract::{
    LinkMediaCardReason, LinkMediaFileReason, LinkMediaResponse, LinkMediaResponseKind, Message,
    Snapshot,
};

use super::{Changes, Workspace, parse_time};
use crate::api::{Api, ApiError};

/// The topic this sub-state reports in [`Changes`], with the message's ID.
pub const TOPIC: &str = "link_media";

/// A busy server is asked again this many times, waiting longer each time,
/// before its answer stands.
const BUSY_RETRIES: u32 = 4;
const BUSY_WAIT: Duration = Duration::from_millis(750);

/// A preview that failed is asked for again after this long.
const RETRY_FAILED_MS: i64 = 60_000;

#[derive(Debug, Clone, PartialEq)]
pub enum Preview {
    /// Asked; no answer yet.
    Loading,
    /// The server's answer, and when its cached media stops working.
    Ready {
        response: Box<LinkMediaResponse>,
        expires_ms: Option<i64>,
    },
    /// The request failed (`at_ms`): the card says so, and the link still
    /// opens in the browser.
    Failed { at_ms: i64 },
}

/// A message a link points to.
#[derive(Debug, Clone, PartialEq)]
pub enum Linked {
    Loading,
    Found(Box<Message>),
    /// You never received it, or it is in a conversation you left.
    Missing,
    Failed,
}

#[derive(Debug, Clone, Default)]
pub struct State {
    /// By message ID and link.
    previews: HashMap<(String, String), Preview>,
    /// Messages links point to that are not in a loaded timeline, by the ID
    /// the link carries.
    linked: HashMap<String, Linked>,
}

/// Called for every fresh event after the workspace's own handling. An
/// edit that changes a message's links asks for the new ones by their own
/// key, so no event clears an answer.
pub(super) fn apply(_w: &mut Workspace, _event: &ServerEvent, _now_ms: i64) -> Changes {
    Changes::default()
}

/// Called when a bootstrap replaces the workspace, before it is replaced.
pub(super) fn reset(w: &mut Workspace, _snapshot: &Snapshot) -> Changes {
    w.link_media = State::default();
    Changes::topic(TOPIC)
}

impl Workspace {
    /// What is known of `url` in `message`.
    pub fn link_preview(&self, message: &str, url: &str) -> Option<&Preview> {
        self.link_media
            .previews
            .get(&(message.to_string(), url.to_string()))
    }

    /// Whether `url` in `message` should be resolved now: it was never
    /// asked for, its media expired, or it failed a while ago. Marks it as
    /// loading when so.
    pub fn begin_link_preview(&mut self, message: &str, url: &str, now_ms: i64) -> bool {
        let key = (message.to_string(), url.to_string());
        let ask = match self.link_media.previews.get(&key) {
            None => true,
            Some(Preview::Loading) => false,
            Some(Preview::Ready { expires_ms, .. }) => expires_ms.is_some_and(|e| e <= now_ms),
            Some(Preview::Failed { at_ms }) => now_ms - at_ms >= RETRY_FAILED_MS,
        };
        if ask {
            self.link_media.previews.insert(key, Preview::Loading);
        }
        ask
    }

    /// The server's answer for `url` in `message`.
    pub fn link_preview_answered(
        &mut self,
        message: &str,
        url: &str,
        result: Result<LinkMediaResponse, ApiError>,
        now_ms: i64,
    ) -> Changes {
        let preview = match result {
            Ok(response) => Preview::Ready {
                expires_ms: expiry(&response),
                response: Box::new(response),
            },
            Err(_) => Preview::Failed { at_ms: now_ms },
        };
        self.link_media
            .previews
            .insert((message.to_string(), url.to_string()), preview);
        Changes::topic_id(TOPIC, message)
    }

    /// The message a link carrying `id` points to: from a loaded timeline
    /// (so edits show at once), else what the server answered.
    pub fn linked_message(&self, conversation: &str, id: &str) -> Option<Linked> {
        if let Some(found) = self.loaded_linked(conversation, id) {
            return Some(Linked::Found(Box::new(found.clone())));
        }
        self.link_media.linked.get(id).cloned()
    }

    fn loaded_linked(&self, conversation: &str, id: &str) -> Option<&Message> {
        let c = self.conversation(conversation)?;
        self.timelines
            .get(conversation)?
            .entries
            .iter()
            .map(|e| &e.message)
            .find(|m| is_linked_message(&c.network_id, m, id))
    }

    /// Whether the message a link carrying `id` points to should be looked
    /// up now. Marks it as loading when so.
    pub fn begin_linked_message(&mut self, conversation: &str, id: &str) -> bool {
        if self.loaded_linked(conversation, id).is_some() || self.link_media.linked.contains_key(id)
        {
            return false;
        }
        self.link_media
            .linked
            .insert(id.to_string(), Linked::Loading);
        true
    }

    /// The server's answer to looking up the message a link points to.
    pub fn linked_message_answered(
        &mut self,
        message: &str,
        id: &str,
        result: Result<Message, ApiError>,
    ) -> Changes {
        let linked = match result {
            Ok(m) if self.conversation(&m.conversation_id).is_some() => Linked::Found(Box::new(m)),
            Ok(_) | Err(ApiError::Http { status: 404, .. }) => Linked::Missing,
            Err(_) => Linked::Failed,
        };
        self.link_media.linked.insert(id.to_string(), linked);
        Changes::topic_id(TOPIC, message)
    }
}

/// Resolve `url` in `message`, asking again while the server is busy, as
/// the web client does.
pub async fn resolve(
    api: Api,
    conversation: String,
    message: String,
    url: String,
) -> Result<LinkMediaResponse, ApiError> {
    let mut attempt = 0;
    loop {
        let response = api
            .resolve_link_media(&conversation, &message, &url)
            .await?;
        let busy = response.kind == LinkMediaResponseKind::Card
            && response.card.as_ref().and_then(|c| c.reason) == Some(LinkMediaCardReason::Busy);
        if !busy || attempt >= BUSY_RETRIES {
            return Ok(response);
        }
        attempt += 1;
        tokio::time::sleep(BUSY_WAIT * attempt).await;
    }
}

/// When the first expiring copy in an answer stops working.
fn expiry(r: &LinkMediaResponse) -> Option<i64> {
    let mut times: Vec<&str> = r
        .assets
        .iter()
        .filter_map(|a| a.expires_at.as_deref())
        .collect();
    times.extend(r.file.as_ref().and_then(|f| f.expires_at.as_deref()));
    times.extend(r.card.as_ref().and_then(|c| c.expires_at.as_deref()));
    times.extend(r.widget.as_ref().and_then(|w| w.expires_at.as_deref()));
    times.into_iter().map(parse_time).filter(|t| *t > 0).min()
}

/// The ID every reader on a network stores for a message with an upstream
/// msgid (`sharedMessageId` in `message-links.ts`).
pub fn shared_message_id(network_id: &str, m: &Message) -> Option<String> {
    m.upstream_id
        .as_deref()
        .filter(|u| !u.is_empty() && !u.starts_with('_'))
        .map(|u| format!("{network_id}:{u}"))
}

/// Whether `m` is the message a link carrying `id` names, by either ID.
pub fn is_linked_message(network_id: &str, m: &Message, id: &str) -> bool {
    m.id == id || shared_message_id(network_id, m).as_deref() == Some(id)
}

/// Only `http(s)` links without a user name or password are fetched or
/// opened (`publicMediaURL`).
pub fn public_url(raw: &str) -> Option<String> {
    let url = url::Url::parse(raw).ok()?;
    (matches!(url.scheme(), "http" | "https")
        && url.username().is_empty()
        && url.password().is_none())
    .then(|| url.to_string())
}

/// Why a preview shows no content, as a text ID suffix
/// (`link-previews-reason-…`); unknown reasons read as unavailable.
pub fn reason_key(
    card: Option<LinkMediaCardReason>,
    file: Option<LinkMediaFileReason>,
) -> Option<&'static str> {
    use LinkMediaCardReason as C;
    use LinkMediaFileReason as F;
    if let Some(reason) = card {
        return Some(match reason {
            C::UnsupportedType => "unsupported-type",
            C::TypeMismatch => "type-mismatch",
            C::TooLarge => "too-large",
            C::InvalidFile => "invalid-file",
            C::FetchFailed => "fetch-failed",
            C::Busy => "busy",
            C::StorageUnavailable => "storage-unavailable",
            C::NoOembed => "no-oembed",
            C::Restricted => "restricted",
            C::Unknown => "unavailable",
        });
    }
    file.map(|reason| match reason {
        F::UnsupportedType => "unsupported-type",
        F::TypeMismatch => "type-mismatch",
        F::TooLarge => "too-large",
        F::InvalidFile => "invalid-file",
        F::FetchFailed => "fetch-failed",
        F::StorageUnavailable => "storage-unavailable",
        F::NoOembed => "no-oembed",
        F::Restricted => "restricted",
        F::Unknown => "unavailable",
    })
}

/// The kind of file a detected content type names, as a text ID suffix
/// (`link-previews-type-…`): only the detected type, never the extension
/// (`linkFileTypeKey`).
pub fn file_type_key(content_type: &str) -> Option<&'static str> {
    let t = content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    Some(match t.as_str() {
        "application/octet-stream" => "file",
        "application/pdf" => "pdf",
        "application/msword" => "doc",
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document" => "docx",
        "application/zip" | "application/x-zip-compressed" => "zip",
        "application/vnd.rar" | "application/x-rar-compressed" => "rar",
        "application/x-7z-compressed" => "seven-zip",
        "application/x-tar" => "tar",
        "application/gzip" | "application/x-gzip" => "gzip",
        "text/plain" => "text",
        _ => return None,
    })
}

/// What a post card (a social post, an image page) shows above its media.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PostText {
    /// Only when it says something the caption does not.
    pub title: String,
    pub caption: String,
    pub name: String,
    pub handle: String,
}

/// After `nativePostText`: a byline, the title only when it says something
/// (not Instagram's "Post by name" or TikTok's truncated caption), and the
/// caption with runs of blank lines collapsed.
pub fn post_text(r: &LinkMediaResponse) -> PostText {
    let mut caption = String::new();
    for line in r.description.as_deref().unwrap_or("").split('\n') {
        caption.push_str(line.trim_end_matches([' ', '\t']));
        caption.push('\n');
    }
    while caption.contains("\n\n\n") {
        caption = caption.replace("\n\n\n", "\n\n");
    }
    let caption = caption.trim().to_string();
    let mut title = r.title.as_deref().unwrap_or("").trim().to_string();
    let generic = generic_title(&title);
    let truncated = title.trim_end_matches("...").trim_end_matches('…').trim();
    if generic.is_some()
        || title.eq_ignore_ascii_case("tiktok video")
        || (!truncated.is_empty() && caption.starts_with(truncated))
    {
        title.clear();
    }
    let mut name = r.author.as_deref().unwrap_or("").trim().to_string();
    let path = url::Url::parse(&r.url)
        .map(|u| u.path().to_string())
        .unwrap_or_default();
    let handle;
    if r.provider == "reddit" {
        let mut parts = path.trim_start_matches('/').split('/');
        handle = match (parts.next(), parts.next()) {
            (Some(r), Some(sub)) if r.eq_ignore_ascii_case("r") && !sub.is_empty() => {
                format!("r/{sub}")
            }
            _ => String::new(),
        };
        if !name.is_empty() {
            name = format!("u/{}", name.trim_start_matches("u/"));
        }
    } else {
        let from_path = (r.provider == "tiktok")
            .then(|| path.strip_prefix("/@"))
            .flatten()
            .map(|p| p.split('/').next().unwrap_or("").to_string())
            .filter(|p| !p.is_empty());
        let word = |s: &str| {
            !s.is_empty()
                && s.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.')
        };
        let raw = generic
            .or(from_path)
            .or_else(|| word(&name).then(|| name.clone()))
            .unwrap_or_default();
        handle = if raw.is_empty() {
            String::new()
        } else {
            format!("@{}", raw.trim_start_matches('@'))
        };
        if !handle.is_empty() && name.trim_start_matches('@') == &handle[1..] {
            name.clear();
        }
    }
    PostText {
        title,
        caption,
        name,
        handle,
    }
}

/// `Photos by name` and friends: the name, when the title is only that.
fn generic_title(title: &str) -> Option<String> {
    let lower = title.to_ascii_lowercase();
    let (head, rest) = lower.split_once(" by ")?;
    let head = head.strip_suffix('s').unwrap_or(head);
    let name = &title[head.len() + (lower.len() - head.len() - rest.len())..];
    (["post", "photo", "video", "reel", "carousel"].contains(&head)
        && !name.is_empty()
        && !name.contains(char::is_whitespace))
    .then(|| name.to_string())
}

/// `raw` made absolute against the server's `origin` when it is a path on
/// the server (`/media/a.jpg`), as a browser would; anything else as it is.
pub fn resolve_url(raw: &str, origin: Option<&str>) -> String {
    if url::Url::parse(raw).is_ok() {
        return raw.to_string();
    }
    origin
        .and_then(|o| url::Url::parse(o).ok())
        .and_then(|base| base.join(raw).ok())
        .map(|u| u.to_string())
        .unwrap_or_else(|| raw.to_string())
}

/// One colour scheme of a link's artwork tile, as `0xRRGGBB` and an
/// opacity for the two glows (`POSTER_PALETTES` in the web's
/// `utils/satori-card.ts`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Palette {
    pub glow_a: (u32, f32),
    pub glow_b: (u32, f32),
    pub dot: u32,
    pub bar_start: u32,
    pub bar_end: u32,
}

/// The tile's dark backdrop, its title and its quieter text.
pub const ARTWORK_BACKDROP: u32 = 0x141111;
pub const ARTWORK_TITLE: u32 = 0xf7f2f4;
pub const ARTWORK_TEXT: u32 = 0xb4a8af;

pub const PALETTES: [Palette; 5] = [
    Palette {
        glow_a: (0xe06c53, 0.34),
        glow_b: (0xf59e0b, 0.20),
        dot: 0xe06c53,
        bar_start: 0xf43f5e,
        bar_end: 0xf59e0b,
    },
    Palette {
        glow_a: (0xf43f5e, 0.32),
        glow_b: (0xfb923c, 0.22),
        dot: 0xf43f5e,
        bar_start: 0xfb7185,
        bar_end: 0xfb923c,
    },
    Palette {
        glow_a: (0xea580c, 0.32),
        glow_b: (0xf43f5e, 0.22),
        dot: 0xfb923c,
        bar_start: 0xf97316,
        bar_end: 0xf43f5e,
    },
    Palette {
        glow_a: (0xd946ef, 0.24),
        glow_b: (0xf43f5e, 0.24),
        dot: 0xf472b6,
        bar_start: 0xec4899,
        bar_end: 0xf59e0b,
    },
    Palette {
        glow_a: (0xf59e0b, 0.28),
        glow_b: (0xe06c53, 0.24),
        dot: 0xfbbf24,
        bar_start: 0xf59e0b,
        bar_end: 0xe06c53,
    },
];

/// What the generated artwork of a link without a picture shows: the web
/// draws it with Satori (`SatoriLinkGraphic.vue`), the app with its own
/// elements, from the same text and colours.
#[derive(Debug, Clone, PartialEq)]
pub struct Artwork {
    /// The site's name, or the link's host without `www.`.
    pub host: String,
    pub title: String,
    pub description: String,
    pub palette: Palette,
}

/// FNV-1a over UTF-16 code units, as the web's `hashString`, so a site gets
/// the same colours in both clients.
fn fnv1a(text: &str) -> u32 {
    text.encode_utf16().fold(2_166_136_261u32, |h, unit| {
        (h ^ u32::from(unit)).wrapping_mul(16_777_619)
    })
}

/// Whitespace collapsed and cut to `max` characters with an ellipsis.
fn clean(text: &str, max: usize) -> String {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() <= max {
        return collapsed;
    }
    let cut: String = collapsed.chars().take(max - 1).collect();
    format!("{}…", cut.trim_end())
}

/// The artwork for `url` with what its preview said about it.
pub fn artwork(url: &str, site: &str, title: &str, description: &str) -> Artwork {
    let parsed = url::Url::parse(url).ok();
    let host = match (site.trim(), &parsed) {
        (s, _) if !s.is_empty() => s.to_string(),
        (_, Some(u)) => u
            .host_str()
            .map(|h| h.strip_prefix("www.").unwrap_or(h).to_string())
            .unwrap_or_else(|| "link".into()),
        _ => "link".into(),
    };
    let path = parsed
        .as_ref()
        .map(|u| u.path())
        .filter(|p| *p != "/")
        .map(|p| clean(p, 52))
        .unwrap_or_default();
    let title = Some(clean(title, 92))
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| host.clone());
    let description = Some(clean(description, 120))
        .filter(|d| !d.is_empty())
        .unwrap_or_else(|| {
            if path.is_empty() {
                url.to_string()
            } else {
                format!("{host}{path}")
            }
        });
    let seed = if host.is_empty() { url } else { &host };
    let palette = PALETTES[(fnv1a(seed) % PALETTES.len() as u32) as usize];
    Artwork {
        host,
        title,
        description,
        palette,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ircord_contract::{LinkMediaAsset, LinkMediaCard};

    #[test]
    fn artwork_colours_match_the_web() {
        // Hashes from the web's `hashString` run in Node.
        assert_eq!(fnv1a("github.com"), 1_228_281_881);
        assert_eq!(fnv1a("musicforprogramming.net"), 982_306_389);
        assert_eq!(fnv1a("ä😀"), 3_800_846_462);
        let a = artwork("https://github.com/zed-industries/zed", "", "", "");
        assert_eq!(a.palette, PALETTES[1]);
        let a = artwork("https://example.com/", "", "", "");
        assert_eq!(a.palette, PALETTES[3]);
    }

    #[test]
    fn artwork_falls_back_to_the_host_and_path() {
        let a = artwork("https://www.example.com/a/b", " ", "", "  ");
        assert_eq!(a.host, "example.com");
        assert_eq!(a.title, "example.com");
        assert_eq!(a.description, "example.com/a/b");
        let a = artwork("https://example.com/", "Example", "", "");
        assert_eq!((a.host.as_str(), a.title.as_str()), ("Example", "Example"));
        assert_eq!(a.description, "https://example.com/");
        let long = "word ".repeat(40);
        let a = artwork("https://example.com", "", &long, "x\n  y");
        assert_eq!(a.title.chars().count(), 92);
        assert!(a.title.ends_with("word w…"), "as the web cuts it");
        assert_eq!(a.description, "x y");
    }

    #[test]
    fn relative_media_resolves_against_the_server() {
        let origin = Some("https://chat.example.com");
        assert_eq!(
            resolve_url("/media/weekend.jpg", origin),
            "https://chat.example.com/media/weekend.jpg"
        );
        assert_eq!(
            resolve_url("https://cdn.example.org/a.png", origin),
            "https://cdn.example.org/a.png"
        );
        assert_eq!(resolve_url("/media/a.jpg", None), "/media/a.jpg");
    }

    fn post(
        provider: &str,
        url: &str,
        title: &str,
        author: &str,
        caption: &str,
    ) -> LinkMediaResponse {
        LinkMediaResponse {
            kind: LinkMediaResponseKind::Media,
            provider: provider.into(),
            url: url.into(),
            title: Some(title.into()),
            author: Some(author.into()),
            description: Some(caption.into()),
            ..Default::default()
        }
    }

    #[test]
    fn post_text_after_the_web_client() {
        // Instagram's generic title names the account; the title is dropped.
        let t = post_text(&post(
            "instagram",
            "https://www.instagram.com/p/HbBy-ExIyF/",
            "Post by nssmagazine",
            "nssmagazine",
            "Spring  \n\n\n\nlooks",
        ));
        assert_eq!(t.title, "");
        assert_eq!(t.handle, "@nssmagazine");
        assert_eq!(t.name, "");
        assert_eq!(t.caption, "Spring\n\nlooks");
        // A plural generic title counts too.
        assert_eq!(
            generic_title("Photos by Someone").as_deref(),
            Some("Someone")
        );
        assert_eq!(generic_title("Notes by someone"), None);

        // TikTok: the handle from the path, the truncated caption as title is dropped.
        let t = post_text(&post(
            "tiktok",
            "https://www.tiktok.com/@cooking/video/123",
            "Best pasta ever…",
            "Chef Ana",
            "Best pasta ever, with garlic",
        ));
        assert_eq!((t.title.as_str(), t.handle.as_str()), ("", "@cooking"));
        assert_eq!(t.name, "Chef Ana");

        // Reddit: the subreddit as handle, the author as u/name.
        let t = post_text(&post(
            "reddit",
            "https://www.reddit.com/r/rust/comments/abc/hello/",
            "Hello Rustaceans",
            "ferris",
            "",
        ));
        assert_eq!(t.title, "Hello Rustaceans");
        assert_eq!((t.handle.as_str(), t.name.as_str()), ("r/rust", "u/ferris"));
    }

    #[test]
    fn reasons_and_types() {
        assert_eq!(
            reason_key(Some(LinkMediaCardReason::NoOembed), None),
            Some("no-oembed")
        );
        assert_eq!(
            reason_key(Some(LinkMediaCardReason::Unknown), None),
            Some("unavailable")
        );
        assert_eq!(
            reason_key(None, Some(LinkMediaFileReason::TooLarge)),
            Some("too-large")
        );
        assert_eq!(reason_key(None, None), None);
        assert_eq!(
            file_type_key("application/pdf; charset=binary"),
            Some("pdf")
        );
        assert_eq!(
            file_type_key("application/x-7z-compressed"),
            Some("seven-zip")
        );
        assert_eq!(file_type_key("image/png"), None);
        assert_eq!(
            public_url("https://a.example/x").as_deref(),
            Some("https://a.example/x")
        );
        assert_eq!(public_url("https://u:p@a.example/"), None);
        assert_eq!(public_url("javascript:alert(1)"), None);
    }

    #[test]
    fn previews_are_asked_once_and_again_when_they_expire() {
        let mut w = Workspace::from_snapshot(crate::store::tests_support::snapshot(0));
        assert!(w.begin_link_preview("m1", "https://a.example/", 0));
        assert!(
            !w.begin_link_preview("m1", "https://a.example/", 0),
            "already asked"
        );
        let response = LinkMediaResponse {
            kind: LinkMediaResponseKind::Media,
            assets: vec![LinkMediaAsset {
                url: "https://cdn.example/a.png".into(),
                expires_at: Some("2026-11-21T10:00:00Z".into()),
                ..Default::default()
            }],
            ..Default::default()
        };
        let changes = w.link_preview_answered("m1", "https://a.example/", Ok(response), 0);
        assert!(changes.touches_id(TOPIC, "m1"));
        let expires = parse_time("2026-11-21T10:00:00Z");
        assert!(!w.begin_link_preview("m1", "https://a.example/", expires - 1));
        assert!(
            w.begin_link_preview("m1", "https://a.example/", expires),
            "expired media is asked again"
        );

        assert!(w.begin_link_preview("m2", "https://b.example/", 0));
        w.link_preview_answered(
            "m2",
            "https://b.example/",
            Err(ApiError::Network("down".into())),
            0,
        );
        assert!(matches!(
            w.link_preview("m2", "https://b.example/"),
            Some(Preview::Failed { .. })
        ));
        assert!(!w.begin_link_preview("m2", "https://b.example/", 1000));
        assert!(w.begin_link_preview("m2", "https://b.example/", RETRY_FAILED_MS));
        let card = LinkMediaResponse {
            kind: LinkMediaResponseKind::Card,
            card: Some(LinkMediaCard::default()),
            ..Default::default()
        };
        w.link_preview_answered("m2", "https://b.example/", Ok(card), 0);
        assert!(
            !w.begin_link_preview("m2", "https://b.example/", i64::MAX),
            "a card does not expire"
        );
    }

    #[test]
    fn linked_messages_by_either_id() {
        let mut w = Workspace::from_snapshot(crate::store::tests_support::snapshot(0));
        let m = Message {
            id: "local".into(),
            upstream_id: Some("abc".into()),
            ..Default::default()
        };
        assert!(is_linked_message("n1", &m, "local"));
        assert!(is_linked_message("n1", &m, "n1:abc"));
        assert!(!is_linked_message("n2", &m, "n1:abc"));
        assert!(w.begin_linked_message("c", "x"));
        assert!(!w.begin_linked_message("c", "x"));
        w.linked_message_answered(
            "m1",
            "x",
            Err(ApiError::Http {
                status: 404,
                detail: String::new(),
            }),
        );
        assert_eq!(w.linked_message("c", "x"), Some(Linked::Missing));
        // A message in a conversation you are not in is missing too.
        assert!(w.begin_linked_message("c", "y"));
        w.linked_message_answered("m1", "y", Ok(Message::default()));
        assert_eq!(w.linked_message("c", "y"), Some(Linked::Missing));
    }
}

//! Custom emoji: `/emotes`, `/emotes/{id}`, `/emotes/discover`, `/emotes/imports`,
//! and GIF search through Klipy ([`klipy`]).
//!
//! Owned by package E1, S5. Add calls in this `impl Api` block with the
//! helpers in `api/mod.rs` and the types in `ircord_contract`.

#[allow(unused_imports)]
use super::{Api, ApiError, Method, segment};
use ircord_contract as c;

impl Api {
    /// The instance's shared emote catalog (`GET /api/v1/emotes`).
    pub async fn emotes(&self) -> Result<c::EmoteCatalog, ApiError> {
        self.get("/api/v1/emotes").await
    }

    /// The bundled workspace emoji and the cached 7TV global set
    /// (`GET /api/v1/app/emoji/catalog`). Public.
    pub async fn app_emoji_catalog(&self) -> Result<c::AppEmojiCatalog, ApiError> {
        self.get("/api/v1/app/emoji/catalog").await
    }

    /// `POST /api/v1/emotes`: upload a picture as a new emote, as
    /// `multipart/form-data`.
    pub async fn upload_emote(&self, emote: &NewEmote) -> Result<c::Emote, ApiError> {
        let boundary = format!("ircord-{}", crate::client::random_id());
        let mut body = Vec::with_capacity(emote.bytes.len() + 1024);
        let mut text = |name: &str, value: &str| {
            body.extend_from_slice(
                format!(
                    "--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
                )
                .as_bytes(),
            );
        };
        text("shortcode", &emote.shortcode);
        if !emote.aliases.is_empty() {
            text("aliases", &emote.aliases.join(","));
        }
        let mut file = |name: &str, filename: &str, content_type: &str, bytes: &[u8]| {
            body.extend_from_slice(
                format!(
                    "--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"; filename=\"{}\"\r\nContent-Type: {content_type}\r\n\r\n",
                    quoted_name(filename)
                )
                .as_bytes(),
            );
            body.extend_from_slice(bytes);
            body.extend_from_slice(b"\r\n");
        };
        file("file", &emote.file_name, &emote.content_type, &emote.bytes);
        if let Some(still) = &emote.still {
            file("still", "still.png", "image/png", still);
        }
        body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
        let request = self
            .request(Method::POST, "/api/v1/emotes")
            .header(
                reqwest::header::CONTENT_TYPE,
                format!("multipart/form-data; boundary={boundary}"),
            )
            .body(body);
        self.json(request).await
    }

    /// `POST /api/v1/emotes/imports`: copy an emote from 7TV, FrankerFaceZ
    /// or emoji.gg onto the server.
    pub async fn import_emote(&self, import: &c::EmoteImport) -> Result<c::Emote, ApiError> {
        self.call(Method::POST, "/api/v1/emotes/imports", import)
            .await
    }

    /// `PATCH /api/v1/emotes/{id}`: rename, re-alias, enable or disable.
    pub async fn update_emote(
        &self,
        id: &str,
        patch: &c::EmotePatch,
    ) -> Result<c::Emote, ApiError> {
        self.call(
            Method::PATCH,
            &format!("/api/v1/emotes/{}", segment(id)),
            patch,
        )
        .await
    }

    /// `DELETE /api/v1/emotes/{id}`: remove an emote for everyone.
    pub async fn delete_emote(&self, id: &str) -> Result<(), ApiError> {
        self.delete(&format!("/api/v1/emotes/{}", segment(id)))
            .await
    }

    /// `GET /api/v1/emotes/discover`: one page of the providers' emoji.
    pub async fn discover_emotes(&self, query: &Discover) -> Result<c::DiscoveryResult, ApiError> {
        let mut params: Vec<(&str, String)> = vec![("limit", query.limit.to_string())];
        for (name, value) in [
            ("source", &query.source),
            ("query", &query.query),
            ("animation", &query.animation),
            ("cursor", &query.cursor),
        ] {
            let value = value.trim();
            if !value.is_empty() {
                params.push((name, value.to_string()));
            }
        }
        self.get_query("/api/v1/emotes/discover", &params).await
    }

    /// `GET /api/v1/emotes/discover/{provider}/{id}/preview`: a discovered
    /// emoji's picture, by the item's `previewUrl`. The server serves it only
    /// to a signed-in account, so it is fetched here with the session rather
    /// than through `img(url)`; `still` asks for the first frame
    /// (`?static=true`), as the web's tiles do. The server makes stills only
    /// from GIFs and refuses one for an animated PNG (which emoji.gg lists as
    /// still); that one comes whole, and is drawn on its first frame.
    pub async fn discovered_picture(
        &self,
        preview_url: &str,
        still: bool,
    ) -> Result<Picture, ApiError> {
        if !is_discovery_preview(preview_url) {
            return Err(ApiError::Decode(format!(
                "{preview_url} is not a discovery preview on this server"
            )));
        }
        let mut request = self.request(Method::GET, preview_url);
        if still {
            request = request.query(&[("static", "true")]);
        }
        let response = match self.send(request).await {
            Err(ApiError::Http { status: 422, .. }) if still => {
                self.send(self.request(Method::GET, preview_url)).await?
            }
            other => other?,
        };
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let bytes = response
            .bytes()
            .await
            .map_err(|e| ApiError::Network(e.to_string()))?;
        if bytes.len() > PICTURE_MAX {
            return Err(ApiError::Decode(format!(
                "the preview is larger than {PICTURE_MAX} bytes"
            )));
        }
        let content_type = image_type(&bytes)
            .map(str::to_string)
            .unwrap_or(content_type);
        Ok(Picture {
            content_type,
            bytes: bytes.to_vec(),
        })
    }
}

/// A picture read from the server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Picture {
    pub content_type: String,
    pub bytes: Vec<u8>,
}

/// The largest discovery preview read.
pub const PICTURE_MAX: usize = 8 << 20;

/// Whether `url` is a discovery preview path on the signed-in server (the
/// only kind the backend hands out), the one place the session is sent for
/// a picture.
pub fn is_discovery_preview(url: &str) -> bool {
    url.strip_prefix("/api/v1/emotes/discover/")
        .and_then(|rest| rest.strip_suffix("/preview"))
        .is_some_and(|middle| {
            let mut parts = middle.split('/');
            let ok = |p: Option<&str>| {
                p.is_some_and(|p| {
                    !p.is_empty()
                        && p != ".."
                        && p.chars().all(|ch| {
                            ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-' | '%')
                        })
                })
            };
            ok(parts.next()) && ok(parts.next()) && parts.next().is_none()
        })
}

/// A picture to upload as an emote.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NewEmote {
    pub file_name: String,
    /// One of the catalog's `types`, from [`image_type`].
    pub content_type: String,
    pub bytes: Vec<u8>,
    /// A still PNG of the first frame, for viewers with animation off. The
    /// server keeps it only for an animated WebP or PNG; it takes a GIF's
    /// first frame itself.
    pub still: Option<Vec<u8>>,
    pub shortcode: String,
    pub aliases: Vec<String>,
}

/// A discovery page to ask for. Empty strings are left out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Discover {
    pub source: String,
    pub query: String,
    /// `all`, `static` or `animated`.
    pub animation: String,
    pub cursor: String,
    pub limit: u32,
}

impl Default for Discover {
    fn default() -> Self {
        Self {
            source: String::new(),
            query: String::new(),
            animation: "all".into(),
            cursor: String::new(),
            limit: DISCOVER_PAGE,
        }
    }
}

/// Discovery results per page, as the web client asks.
pub const DISCOVER_PAGE: u32 = 30;

/// The longest shortcode the server takes.
pub const SHORTCODE_MAX: usize = 32;

/// `Content-Disposition` wants a quoted name without quotes or line breaks.
fn quoted_name(name: &str) -> String {
    name.chars()
        .map(|ch| match ch {
            '"' | '\r' | '\n' | '\\' => '_',
            ch => ch,
        })
        .collect()
}

/// The picture's type from its first bytes: PNG, GIF, WebP or JPEG, the
/// types the server takes.
pub fn image_type(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else {
        None
    }
}

/// A shortcode made from a file or emoji name, as the web client suggests
/// one: lowercase, every run of other characters one `_`, at most 32.
pub fn suggest_shortcode(name: &str) -> String {
    let mut out = String::new();
    let mut gap = false;
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            out.push(ch.to_ascii_lowercase());
            gap = false;
        } else if !gap {
            out.push('_');
            gap = true;
        }
    }
    out.chars().take(SHORTCODE_MAX).collect()
}

/// The aliases typed as `a, b, c`: trimmed, empty ones left out.
pub fn split_aliases(text: &str) -> Vec<String> {
    text.split(',')
        .map(str::trim)
        .filter(|a| !a.is_empty())
        .map(str::to_string)
        .collect()
}

/// An emote link or ID pasted for import: a 7TV emote (its page or its
/// 26-character ID) or a FrankerFaceZ one (its page, API URL, `ffz:` ID or
/// bare number), as the web client's `parseProviderEmote`.
pub fn parse_provider_link(text: &str) -> Option<(c::EmoteImportProvider, String)> {
    let text = text.trim();
    let without_scheme = |t: &str| -> Option<String> {
        let rest = t
            .strip_prefix("https://")
            .or_else(|| t.strip_prefix("http://"))?;
        Some(rest.strip_prefix("www.").unwrap_or(rest).to_string())
    };
    let trailing = |t: &str| t.strip_suffix('/').unwrap_or(t).to_string();

    // 7TV.
    let seven = match without_scheme(text) {
        Some(rest) => rest
            .strip_prefix("7tv.app/emotes/")
            .or_else(|| rest.strip_prefix("7tv.io/v3/emotes/"))
            .map(trailing),
        None => Some(trailing(text)),
    };
    if let Some(id) = seven
        && id.len() == 26
        && id.chars().all(|ch| {
            let ch = ch.to_ascii_uppercase();
            ch.is_ascii_digit() || (ch.is_ascii_uppercase() && !matches!(ch, 'I' | 'L' | 'O' | 'U'))
        })
    {
        return Some((c::EmoteImportProvider::V7tv, id.to_ascii_uppercase()));
    }

    // FrankerFaceZ.
    let ffz = match without_scheme(text) {
        Some(rest) => rest
            .strip_prefix("frankerfacez.com/emoticon/")
            .or_else(|| {
                // Only the API host, not `www.api…`.
                text.strip_prefix("https://")
                    .or_else(|| text.strip_prefix("http://"))?
                    .strip_prefix("api.frankerfacez.com/v1/emote/")
            })
            .map(trailing),
        None => Some(trailing(text.strip_prefix("ffz:").unwrap_or(text))),
    }?;
    let (digits, rest) = ffz.split_at(
        ffz.find(|ch: char| !ch.is_ascii_digit())
            .unwrap_or(ffz.len()),
    );
    let slug_ok = rest.is_empty()
        || rest.strip_prefix('-').is_some_and(|slug| {
            slug.chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
        });
    if (1..=10).contains(&digits.len()) && slug_ok {
        return Some((c::EmoteImportProvider::Ffz, digits.to_string()));
    }
    None
}

/// The provider's name as people know it.
pub fn provider_name(source: c::EmoteSource) -> Option<&'static str> {
    match source {
        c::EmoteSource::V7tv => Some("7TV"),
        c::EmoteSource::Ffz => Some("FrankerFaceZ"),
        c::EmoteSource::EmojiGg => Some("emoji.gg"),
        _ => None,
    }
}

/// The source of a discovered emoji, for importing it.
pub fn import_provider(provider: &str) -> c::EmoteImportProvider {
    match provider {
        "7tv" => c::EmoteImportProvider::V7tv,
        "ffz" => c::EmoteImportProvider::Ffz,
        "emoji.gg" => c::EmoteImportProvider::EmojiGg,
        _ => c::EmoteImportProvider::Unknown,
    }
}

/// Whether `emote` is a server emote that came from this discovered emoji.
pub fn is_installed(item: &c::DiscoveryItem, catalog: &[c::Emote]) -> bool {
    item.installed
        || catalog.iter().any(|e| {
            e.provider_id.as_deref() == Some(item.provider_id.as_str())
                && import_provider(&item.provider) != c::EmoteImportProvider::Unknown
                && serde_json::to_value(e.source).ok()
                    == Some(serde_json::Value::String(item.provider.clone()))
        })
}

/// Whether `emote` matches what was typed in the search: its shortcode,
/// name, creator, source or an alias contain it, ignoring case.
pub fn emote_matches(emote: &c::Emote, query: &str) -> bool {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return true;
    }
    let source = serde_json::to_value(emote.source)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default();
    [
        Some(emote.shortcode.as_str()),
        Some(emote.name.as_str()),
        emote.creator.as_deref(),
        Some(source.as_str()),
    ]
    .into_iter()
    .flatten()
    .chain(emote.aliases.iter().flatten().map(String::as_str))
    .any(|v| v.to_lowercase().contains(&query))
}

#[cfg(test)]
mod emote_tests {
    use super::*;

    #[test]
    fn provider_links() {
        let seven = Some((
            c::EmoteImportProvider::V7tv,
            "01GA29CZ2R000C36HNE7Z0DQXD".to_string(),
        ));
        assert_eq!(
            parse_provider_link("https://7tv.app/emotes/01GA29CZ2R000C36HNE7Z0DQXD"),
            seven
        );
        assert_eq!(
            parse_provider_link(" https://www.7tv.app/emotes/01ga29cz2r000c36hne7z0dqxd/ "),
            seven
        );
        assert_eq!(
            parse_provider_link("https://7tv.io/v3/emotes/01GA29CZ2R000C36HNE7Z0DQXD"),
            seven
        );
        assert_eq!(parse_provider_link("01GA29CZ2R000C36HNE7Z0DQXD"), seven);
        let ffz = Some((c::EmoteImportProvider::Ffz, "128054".to_string()));
        assert_eq!(
            parse_provider_link("https://www.frankerfacez.com/emoticon/128054-OMEGALUL"),
            ffz
        );
        assert_eq!(
            parse_provider_link("https://api.frankerfacez.com/v1/emote/128054"),
            ffz
        );
        assert_eq!(parse_provider_link("ffz:128054"), ffz);
        assert_eq!(parse_provider_link("128054"), ffz);
        for wrong in [
            "",
            "https://example.com/emotes/01GA29CZ2R000C36HNE7Z0DQXD",
            "01GA29CZ2R000C36HNE7Z0DQXU",
            "12345678901",
            "ffz:abc",
            "https://7tv.app/emotes/short",
        ] {
            assert_eq!(parse_provider_link(wrong), None, "{wrong}");
        }
    }

    #[test]
    fn shortcodes_and_aliases() {
        assert_eq!(suggest_shortcode("Party Parrot.v2"), "party_parrot_v2");
        assert_eq!(suggest_shortcode("ÄÖ--x"), "_x");
        assert_eq!(suggest_shortcode(&"a".repeat(40)).len(), 32);
        assert_eq!(split_aliases(" a, ,b ,c,"), vec!["a", "b", "c"]);
        assert!(split_aliases("  ").is_empty());
    }

    #[test]
    fn picture_types() {
        assert_eq!(image_type(b"\x89PNG\r\n\x1a\n...."), Some("image/png"));
        assert_eq!(image_type(b"GIF89a.."), Some("image/gif"));
        assert_eq!(image_type(b"RIFF\0\0\0\0WEBPVP8 "), Some("image/webp"));
        assert_eq!(image_type(&[0xFF, 0xD8, 0xFF, 0xE0]), Some("image/jpeg"));
        assert_eq!(image_type(b"<svg"), None);
    }

    #[test]
    fn discovery_preview_paths() {
        assert!(is_discovery_preview(
            "/api/v1/emotes/discover/emoji.gg/739853-oof/preview"
        ));
        for wrong in [
            "https://cdn.example/emotes/discover/emoji.gg/1/preview",
            "//evil.example/api/v1/emotes/discover/emoji.gg/1/preview",
            "/api/v1/emotes/discover/emoji.gg/../../me/preview",
            "/api/v1/emotes/discover/emoji.gg/1/2/preview",
            "/api/v1/emotes/discover/emoji.gg//preview",
            "/api/v1/emotes/discover/emoji.gg/1/preview?x=1",
            "/api/v1/emotes",
        ] {
            assert!(!is_discovery_preview(wrong), "{wrong}");
        }
    }

    #[test]
    fn search_and_installed() {
        let emote: c::Emote = serde_json::from_value(serde_json::json!({
            "id": "e1", "name": "PartyParrot", "shortcode": "parrot", "aliases": ["birb"],
            "retired": [], "enabled": true, "animated": true, "url": "/u", "staticUrl": "/s",
            "width": 32, "height": 32, "source": "7tv", "providerId": "P1", "creator": "Ann",
            "canManage": true, "createdAt": "", "updatedAt": ""
        }))
        .unwrap();
        for q in ["", "PARR", "birb", "ann", "7tv", "party"] {
            assert!(emote_matches(&emote, q), "{q}");
        }
        assert!(!emote_matches(&emote, "cat"));
        let mut item = c::DiscoveryItem {
            provider: "7tv".into(),
            provider_id: "P1".into(),
            ..Default::default()
        };
        assert!(is_installed(&item, std::slice::from_ref(&emote)));
        item.provider = "emoji.gg".into();
        assert!(!is_installed(&item, std::slice::from_ref(&emote)));
        item.installed = true;
        assert!(is_installed(&item, &[]));
    }
}

/// GIF search through Klipy, the way the web client does it: requests go
/// straight to Klipy with the instance's public app key
/// (`app/utils/klipy.ts`, `shared/utils/media.ts`). The key is not served by
/// the backend; the app is built with it (`IRCORD_KLIPY_KEY`).
pub mod klipy {
    use std::time::Duration;

    use ircord_contract as c;
    use serde_json::Value;

    /// Klipy's API, before `/{key}/gifs/...`.
    pub const BASE: &str = "https://api.klipy.com/api/v1";

    /// The hosts Klipy serves pictures from; the backend refuses a GIF from
    /// anywhere else (kept in step with `shared/utils/gif-hosts.ts`).
    pub const MEDIA_HOSTS: [&str; 3] = ["static.klipy.com", "media.klipy.com", "cdn.klipy.com"];

    /// Results per page, as the web client asks.
    pub const PER_PAGE: u32 = 24;

    #[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
    pub enum KlipyError {
        /// No app key: this build cannot search GIFs.
        #[error("GIF search is not configured.")]
        NotConfigured,
        #[error("Could not reach Klipy.")]
        Unreachable,
        /// 429.
        #[error("GIF search is busy.")]
        Busy,
        /// 401 or 403.
        #[error("Klipy did not accept the app key.")]
        KeyRejected,
        #[error("Klipy could not load these GIFs.")]
        LoadFailed,
        #[error("Klipy returned an unreadable response.")]
        Unreadable,
        /// A GIF whose pictures are not on Klipy's hosts.
        #[error("Klipy returned a GIF that cannot be displayed safely.")]
        UnsafeGif,
    }

    /// One page of results; `next_page` is `None` on the last.
    #[derive(Debug, Clone, Default, PartialEq)]
    pub struct GifPage {
        pub items: Vec<c::GIF>,
        pub next_page: Option<u32>,
    }

    #[derive(Debug, Clone, PartialEq)]
    pub struct Category {
        pub name: String,
        pub query: String,
        pub preview_url: String,
    }

    /// Where to reach Klipy and with which key.
    #[derive(Debug, Clone)]
    pub struct Klipy {
        base: String,
        key: String,
        locale: String,
        http: reqwest::Client,
    }

    impl Klipy {
        pub fn new(base: impl Into<String>, key: impl Into<String>) -> Self {
            let http = reqwest::Client::builder()
                .user_agent(crate::api::user_agent())
                .connect_timeout(Duration::from_secs(10))
                .timeout(Duration::from_secs(12))
                .build()
                .expect("the HTTP client configuration is valid");
            Self {
                base: base.into().trim_end_matches('/').to_string(),
                key: key.into(),
                locale: "en".into(),
                http,
            }
        }

        /// The language results are ranked for, such as `en` or `de_DE`.
        pub fn locale(mut self, locale: impl Into<String>) -> Self {
            self.locale = locale.into();
            self
        }

        pub fn configured(&self) -> bool {
            !self.key.is_empty()
        }

        async fn request(
            &self,
            method: reqwest::Method,
            path: &str,
            query: &[(&str, String)],
            body: Option<Value>,
        ) -> Result<Value, KlipyError> {
            if self.key.is_empty() {
                return Err(KlipyError::NotConfigured);
            }
            let url = format!(
                "{}/{}/gifs/{path}",
                self.base,
                crate::api::segment(&self.key)
            );
            let mut request = self.http.request(method, url).query(query);
            if let Some(body) = body {
                request = request.json(&body);
            }
            let response = request.send().await.map_err(|_| KlipyError::Unreachable)?;
            let status = response.status().as_u16();
            if !(200..300).contains(&status) {
                return Err(match status {
                    429 => KlipyError::Busy,
                    401 | 403 => KlipyError::KeyRejected,
                    _ => KlipyError::LoadFailed,
                });
            }
            response.json().await.map_err(|_| KlipyError::Unreadable)
        }

        /// Trending GIFs for an empty `query`, else matches; `page` from 1.
        pub async fn gifs(&self, query: &str, page: u32) -> Result<GifPage, KlipyError> {
            let q: String = query.trim().chars().take(100).collect();
            let mut params = vec![
                ("per_page", PER_PAGE.to_string()),
                ("page", page.to_string()),
                ("locale", language(&self.locale).to_string()),
                ("content_filter", "high".to_string()),
                ("format_filter", "gif,webp,jpg".to_string()),
            ];
            let path = if q.is_empty() {
                "trending"
            } else {
                params.push(("q", q));
                "search"
            };
            normalize_page(
                &self
                    .request(reqwest::Method::GET, path, &params, None)
                    .await?,
            )
        }

        /// The browsing categories, at most 16.
        pub async fn categories(&self) -> Result<Vec<Category>, KlipyError> {
            let params = [("locale", self.locale.replace('-', "_"))];
            let value = self
                .request(reqwest::Method::GET, "categories", &params, None)
                .await?;
            Ok(categories(&value))
        }

        /// Tell Klipy a GIF was shared, as its terms ask. Never fails the send.
        pub async fn share(&self, gif: &c::GIF, query: &str) {
            let path = format!("share/{}", crate::api::segment(&gif.slug));
            let body = if query.is_empty() {
                serde_json::json!({})
            } else {
                serde_json::json!({ "q": query })
            };
            let _ = self
                .request(reqwest::Method::POST, &path, &[], Some(body))
                .await;
        }
    }

    /// `de-DE` → `de`.
    fn language(locale: &str) -> &str {
        locale
            .split(['-', '_'])
            .next()
            .filter(|l| !l.is_empty())
            .unwrap_or("en")
    }

    /// Whether `url` is an https picture on one of Klipy's hosts.
    pub fn is_media_url(url: &str) -> bool {
        if url.len() > 4096 {
            return false;
        }
        let Ok(u) = url::Url::parse(url) else {
            return false;
        };
        u.scheme() == "https"
            && u.username().is_empty()
            && u.password().is_none()
            && u.port().is_none()
            && u.host_str().is_some_and(|h| MEDIA_HOSTS.contains(&h))
    }

    /// What the backend accepts as a message's GIF (`irc.ValidGIF`).
    pub fn valid(gif: &c::GIF) -> bool {
        let slug_ok = !gif.slug.is_empty()
            && gif.slug.len() <= 180
            && gif
                .slug
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-');
        gif.provider == "klipy"
            && !gif.id.is_empty()
            && gif.id.len() <= 100
            && slug_ok
            && gif.title.chars().count() <= 200
            && is_media_url(&gif.url)
            && is_media_url(&gif.preview_url)
            && is_media_url(&gif.still_url)
            && (1..=8192).contains(&gif.width)
            && (1..=8192).contains(&gif.height)
    }

    fn text(v: &Value) -> Option<String> {
        match v {
            Value::String(s) => Some(s.clone()),
            Value::Number(n) => Some(n.to_string()),
            _ => None,
        }
    }

    /// The first of `sizes` × `formats` (in that order of preference) that
    /// the file has: `(url, width, height)`.
    fn pick(file: &Value, choices: &[(&str, &str)]) -> Option<(String, i64, i64)> {
        choices.iter().find_map(|(size, format)| {
            let v = file.get(size)?.get(format)?;
            let url = v.get("url")?.as_str()?.to_string();
            let w = v.get("width").and_then(Value::as_i64).unwrap_or(320);
            let h = v.get("height").and_then(Value::as_i64).unwrap_or(240);
            Some((url, w, h))
        })
    }

    /// Read a page of Klipy rows, as `normalizeKlipyPage` does.
    pub fn normalize_page(response: &Value) -> Result<GifPage, KlipyError> {
        if response.get("result") == Some(&Value::Bool(false)) {
            return Err(KlipyError::LoadFailed);
        }
        let data = response.get("data").ok_or(KlipyError::LoadFailed)?;
        let (wrapper, rows) = match data {
            Value::Array(rows) => (None, rows),
            Value::Object(_) => (
                Some(data),
                data.get("data")
                    .and_then(Value::as_array)
                    .ok_or(KlipyError::Unreadable)?,
            ),
            _ => return Err(KlipyError::LoadFailed),
        };
        let mut items = Vec::new();
        for row in rows {
            let empty = Value::Null;
            let file = row.get("file").unwrap_or(&empty);
            let preview = pick(
                file,
                &[
                    ("sm", "webp"),
                    ("xs", "webp"),
                    ("sm", "gif"),
                    ("md", "webp"),
                    ("hd", "webp"),
                    ("hd", "gif"),
                ],
            );
            let full = pick(file, &[("hd", "gif"), ("md", "gif"), ("hd", "webp")])
                .or_else(|| preview.clone());
            let still = pick(
                file,
                &[("sm", "jpg"), ("xs", "jpg"), ("hd", "jpg"), ("sm", "png")],
            )
            .or_else(|| preview.clone());
            let slug = row.get("slug").and_then(text).unwrap_or_default();
            let gif = c::GIF {
                id: row.get("id").and_then(text).unwrap_or_else(|| slug.clone()),
                slug,
                title: row
                    .get("title")
                    .and_then(text)
                    .unwrap_or_else(|| "GIF".into())
                    .chars()
                    .take(200)
                    .collect(),
                url: full.as_ref().map(|f| f.0.clone()).unwrap_or_default(),
                preview_url: preview.as_ref().map(|f| f.0.clone()).unwrap_or_default(),
                still_url: still.map(|f| f.0).unwrap_or_default(),
                width: full.as_ref().map_or(320, |f| f.1),
                height: full.as_ref().map_or(240, |f| f.2),
                provider: "klipy".into(),
            };
            if !valid(&gif) {
                return Err(KlipyError::UnsafeGif);
            }
            items.push(gif);
        }
        let page = wrapper
            .and_then(|w| w.get("current_page"))
            .and_then(Value::as_u64)
            .unwrap_or(1) as u32;
        let has_next = wrapper
            .and_then(|w| w.get("has_next"))
            .and_then(Value::as_bool)
            .unwrap_or_else(|| {
                wrapper
                    .and_then(|w| w.get("next_page_url"))
                    .is_some_and(|u| u.as_str().is_some_and(|s| !s.is_empty()))
            });
        let last = wrapper
            .and_then(|w| w.get("last_page"))
            .and_then(Value::as_u64)
            .unwrap_or(0) as u32;
        Ok(GifPage {
            items,
            next_page: (has_next || (last > 0 && page < last)).then_some(page + 1),
        })
    }

    fn categories(value: &Value) -> Vec<Category> {
        let Some(rows) = value
            .get("data")
            .and_then(|d| d.get("categories"))
            .and_then(Value::as_array)
        else {
            return Vec::new();
        };
        rows.iter()
            .take(16)
            .filter_map(|row| {
                Some(Category {
                    name: row.get("category").and_then(text)?,
                    query: row.get("query").and_then(text)?,
                    preview_url: row.get("preview_url").and_then(text).unwrap_or_default(),
                })
            })
            .collect()
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use serde_json::json;

        fn row(id: &str, host: &str) -> Value {
            json!({
                "id": id, "slug": format!("cat-{id}"), "title": "Cat",
                "file": {
                    "hd": {"gif": {"url": format!("https://{host}/{id}/hd.gif"), "width": 498, "height": 280}},
                    "sm": {"webp": {"url": format!("https://{host}/{id}/sm.webp"), "width": 220, "height": 124},
                           "jpg": {"url": format!("https://{host}/{id}/sm.jpg"), "width": 220, "height": 124}}
                }
            })
        }

        #[test]
        fn a_page_picks_the_full_preview_and_still_pictures() {
            let page = normalize_page(&json!({
                "result": true,
                "data": {"data": [row("1", "static.klipy.com")], "current_page": 1, "has_next": true}
            }))
            .unwrap();
            let gif = &page.items[0];
            assert_eq!(gif.url, "https://static.klipy.com/1/hd.gif");
            assert_eq!(gif.preview_url, "https://static.klipy.com/1/sm.webp");
            assert_eq!(gif.still_url, "https://static.klipy.com/1/sm.jpg");
            assert_eq!((gif.width, gif.height), (498, 280));
            assert_eq!(gif.provider, "klipy");
            assert_eq!(page.next_page, Some(2));
        }

        #[test]
        fn the_last_page_has_no_next() {
            let page = normalize_page(&json!({
                "data": {"data": [], "current_page": 3, "last_page": 3}
            }))
            .unwrap();
            assert_eq!(page.next_page, None);
        }

        #[test]
        fn pictures_off_klipys_hosts_are_refused() {
            let err = normalize_page(&json!({"data": [row("1", "evil.example")]})).unwrap_err();
            assert_eq!(err, KlipyError::UnsafeGif);
            assert!(!is_media_url("http://static.klipy.com/a.gif"));
            assert!(!is_media_url("https://user@static.klipy.com/a.gif"));
            assert!(is_media_url("https://media.klipy.com/a.gif"));
        }

        #[test]
        fn a_failed_answer_is_an_error() {
            assert_eq!(
                normalize_page(&json!({"result": false})).unwrap_err(),
                KlipyError::LoadFailed
            );
            assert_eq!(
                normalize_page(&json!({"data": {"rows": []}})).unwrap_err(),
                KlipyError::Unreadable
            );
        }

        #[test]
        fn categories_keep_sixteen() {
            let rows: Vec<Value> = (0..20)
                .map(|i| json!({"category": format!("c{i}"), "query": format!("q{i}"), "preview_url": "https://static.klipy.com/p.gif"}))
                .collect();
            let found = categories(&json!({"data": {"categories": rows}}));
            assert_eq!(found.len(), 16);
            assert_eq!(found[0].query, "q0");
        }

        #[test]
        fn languages_drop_the_region() {
            assert_eq!(language("de-DE"), "de");
            assert_eq!(language("pt_BR"), "pt");
            assert_eq!(language(""), "en");
        }
    }
}

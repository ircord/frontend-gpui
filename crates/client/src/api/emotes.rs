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

//! HTTP commands. Every request carries the session cookie and the server's
//! own origin, as the backend's origin check requires of a native client.
//!
//! [`Api`] holds the connection and the generic helpers ([`Api::get`],
//! [`Api::call`], [`Api::delete`] and friends); the calls themselves live in
//! one file per area of the contract, each an `impl Api` block, so a feature
//! adds its calls to its own area's file. Paths are built with [`segment`],
//! which escapes an ID for use in a path.

use std::sync::{Arc, RwLock};
use std::time::Duration;

use ircord_contract as c;
pub use reqwest::Method;
use reqwest::{StatusCode, header};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::address::ServerAddress;

pub mod admin;
mod app_data;
pub mod auth;
pub mod channels;
mod conversations;
mod devices;
pub mod emotes;
mod inbox;
pub mod links;
mod messages;
pub mod networks;
pub mod notifications;
mod parties;
pub mod people;
mod saved;
pub mod search;
pub mod uploads;

pub const SESSION_COOKIE: &str = "ircord_session";

/// `IRCord-GPUI/0.1 (Windows)`: the device list shows the OS name.
pub fn user_agent() -> String {
    let os = match std::env::consts::OS {
        "windows" => "Windows",
        "macos" => "Mac OS X",
        "linux" => "Linux",
        other => other,
    };
    format!("IRCord-GPUI/{} ({os})", env!("CARGO_PKG_VERSION"))
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum ApiError {
    /// The session is missing or expired.
    #[error("You are signed out.")]
    Unauthorized,
    #[error("{detail}")]
    Http { status: u16, detail: String },
    #[error("Could not reach the server: {0}")]
    Network(String),
    #[error("The server sent something this app does not understand: {0}")]
    Decode(String),
}

impl ApiError {
    fn network(e: reqwest::Error) -> Self {
        ApiError::Network(e.to_string())
    }
}

#[derive(Clone)]
pub struct Api {
    http: reqwest::Client,
    server: ServerAddress,
    cookie: Arc<RwLock<Option<String>>>,
}

impl Api {
    pub fn new(server: ServerAddress) -> Self {
        let http = reqwest::Client::builder()
            .user_agent(user_agent())
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("the HTTP client configuration is valid");
        Self {
            http,
            server,
            cookie: Arc::default(),
        }
    }

    pub fn server(&self) -> &ServerAddress {
        &self.server
    }

    /// The `ircord_session=...` cookie pair, if signed in.
    pub fn cookie(&self) -> Option<String> {
        self.cookie.read().unwrap().clone()
    }

    pub fn set_cookie(&self, cookie: Option<String>) {
        *self.cookie.write().unwrap() = cookie;
    }

    pub(crate) fn request(&self, method: Method, path: &str) -> reqwest::RequestBuilder {
        let mut request = self
            .http
            .request(method, self.server.join(path))
            .header(header::ORIGIN, self.server.origin())
            .header(header::ACCEPT, "application/json");
        if let Some(cookie) = self.cookie() {
            request = request.header(header::COOKIE, cookie);
        }
        request
    }

    pub(crate) async fn send(
        &self,
        request: reqwest::RequestBuilder,
    ) -> Result<reqwest::Response, ApiError> {
        let response = request.send().await.map_err(ApiError::network)?;
        let status = response.status();
        if status == StatusCode::UNAUTHORIZED {
            return Err(ApiError::Unauthorized);
        }
        if !status.is_success() {
            let detail = response
                .json::<c::ErrorModel>()
                .await
                .ok()
                .and_then(|e| e.detail.or(e.title))
                .unwrap_or_else(|| format!("The server answered {status}."));
            return Err(ApiError::Http {
                status: status.as_u16(),
                detail,
            });
        }
        Ok(response)
    }

    pub(crate) async fn json<T: DeserializeOwned>(
        &self,
        request: reqwest::RequestBuilder,
    ) -> Result<T, ApiError> {
        let response = self.send(request).await?;
        let bytes = response.bytes().await.map_err(ApiError::network)?;
        serde_json::from_slice(&bytes).map_err(|e| ApiError::Decode(e.to_string()))
    }

    /// `GET path` and decode the JSON answer.
    pub async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T, ApiError> {
        self.json(self.request(Method::GET, path)).await
    }

    /// `GET path?query` and decode the JSON answer.
    pub async fn get_query<T: DeserializeOwned, Q: Serialize + ?Sized>(
        &self,
        path: &str,
        query: &Q,
    ) -> Result<T, ApiError> {
        self.json(self.request(Method::GET, path).query(query))
            .await
    }

    /// Send `body` as JSON with `method` and decode the JSON answer.
    pub async fn call<B: Serialize + ?Sized, T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: &B,
    ) -> Result<T, ApiError> {
        self.json(self.request(method, path).json(body)).await
    }

    /// Send `body` as JSON with `method`; the answer's body is ignored.
    pub async fn call_empty<B: Serialize + ?Sized>(
        &self,
        method: Method,
        path: &str,
        body: &B,
    ) -> Result<(), ApiError> {
        self.send(self.request(method, path).json(body))
            .await
            .map(drop)
    }

    /// `DELETE path`; the answer's body is ignored.
    pub async fn delete(&self, path: &str) -> Result<(), ApiError> {
        self.send(self.request(Method::DELETE, path))
            .await
            .map(drop)
    }
}

/// Fetch a public file (an image to show inline), at most `max` bytes.
/// Returns the status, the content type and the body.
pub async fn fetch(url: &str, max: usize) -> Result<(u16, Option<String>, Vec<u8>), String> {
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    let client = CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .user_agent(user_agent())
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(60))
            .redirect(reqwest::redirect::Policy::limited(5))
            .build()
            .expect("the HTTP client configuration is valid")
    });
    let mut response = client.get(url).send().await.map_err(|e| e.to_string())?;
    let status = response.status().as_u16();
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|e| e.to_string())? {
        if body.len() + chunk.len() > max {
            return Err(format!("{url} is larger than {max} bytes"));
        }
        body.extend_from_slice(&chunk);
    }
    Ok((status, content_type, body))
}

/// `ircord_session=abc; Path=/; HttpOnly` → `ircord_session=abc`.
pub(crate) fn session_cookie(set_cookie: &str) -> Option<String> {
    let pair = set_cookie.split(';').next()?.trim();
    let (name, value) = pair.split_once('=')?;
    (name == SESSION_COOKIE && !value.is_empty()).then(|| pair.to_string())
}

/// Escape one path segment: `net/#chan nel` → `net%2F%23chan%20nel`.
pub fn segment(segment: &str) -> String {
    url::form_urlencoded::byte_serialize(segment.as_bytes())
        .collect::<String>()
        .replace('+', "%20")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_the_session_cookie() {
        assert_eq!(
            session_cookie("ircord_session=abc123; Path=/; HttpOnly; SameSite=Lax").as_deref(),
            Some("ircord_session=abc123")
        );
        assert_eq!(session_cookie("other=1; Path=/"), None);
        assert_eq!(session_cookie("ircord_session=; Max-Age=0"), None);
    }

    #[test]
    fn path_segments_are_escaped() {
        assert_eq!(segment("net/#chan nel"), "net%2F%23chan%20nel");
    }

    #[test]
    fn user_agent_names_the_platform() {
        assert!(user_agent().starts_with("IRCord-GPUI/"));
    }
}

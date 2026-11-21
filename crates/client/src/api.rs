//! HTTP commands. Every request carries the session cookie and the server's
//! own origin, as the backend's origin check requires of a native client.

use std::sync::{Arc, RwLock};
use std::time::Duration;

use ircord_contract as c;
use reqwest::{Method, StatusCode, header};
use serde::de::DeserializeOwned;

use crate::address::ServerAddress;

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

    fn request(&self, method: Method, path: &str) -> reqwest::RequestBuilder {
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

    async fn send(&self, request: reqwest::RequestBuilder) -> Result<reqwest::Response, ApiError> {
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

    async fn json<T: DeserializeOwned>(&self, request: reqwest::RequestBuilder) -> Result<T, ApiError> {
        let response = self.send(request).await?;
        let bytes = response.bytes().await.map_err(ApiError::network)?;
        serde_json::from_slice(&bytes).map_err(|e| ApiError::Decode(e.to_string()))
    }

    /// Check that the address is an IRCord server: `GET /api/v1/auth/session`
    /// answers without a session with `{"account": null}`.
    pub async fn verify_server(&self) -> Result<(), ApiError> {
        let failed = || ApiError::Http {
            status: 0,
            detail: "Could not find an IRCord server at this address. Check the address and try again."
                .into(),
        };
        let response = self
            .http
            .get(self.server.join("/api/v1/auth/session"))
            .header(header::ORIGIN, self.server.origin())
            .timeout(Duration::from_secs(8))
            .send()
            .await
            .map_err(|_| failed())?;
        if !response.status().is_success() {
            return Err(failed());
        }
        let value: serde_json::Value = response.json().await.map_err(|_| failed())?;
        match value.get("account") {
            Some(_) => Ok(()),
            None => Err(failed()),
        }
    }

    /// Sign in and keep the session cookie. Returns the cookie pair to store.
    pub async fn login(&self, username: &str, password: &str) -> Result<String, ApiError> {
        let body = c::Credentials {
            username: username.to_string(),
            password: password.to_string(),
        };
        let response = self
            .send(self.request(Method::POST, "/api/v1/auth/login").json(&body))
            .await
            .map_err(|e| match e {
                ApiError::Unauthorized => ApiError::Http {
                    status: 401,
                    detail: "The username or password is not right.".into(),
                },
                e => e,
            })?;
        let cookie = response
            .headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .find_map(session_cookie)
            .ok_or_else(|| ApiError::Decode("the sign-in response set no session".into()))?;
        self.set_cookie(Some(cookie.clone()));
        Ok(cookie)
    }

    pub async fn logout(&self) -> Result<(), ApiError> {
        let result = self.send(self.request(Method::POST, "/api/v1/auth/logout")).await;
        self.set_cookie(None);
        match result {
            Ok(_) | Err(ApiError::Unauthorized) => Ok(()),
            Err(e) => Err(e),
        }
    }

    pub async fn bootstrap(&self) -> Result<c::Snapshot, ApiError> {
        self.json(self.request(Method::GET, "/api/v1/bootstrap")).await
    }

    /// The newest page, or the page before `before` (a `nextCursor`).
    pub async fn messages(
        &self,
        conversation: &str,
        before: Option<&str>,
        limit: u32,
    ) -> Result<c::MessagePage, ApiError> {
        let path = format!("/api/v1/conversations/{}/messages", encode(conversation));
        let mut request = self
            .request(Method::GET, &path)
            .query(&[("limit", limit.to_string())]);
        if let Some(before) = before {
            request = request.query(&[("before", before)]);
        }
        self.json(request).await
    }

    pub async fn send_message(
        &self,
        conversation: &str,
        body: &c::MessageInputBody,
    ) -> Result<c::CommandResponse, ApiError> {
        let path = format!("/api/v1/conversations/{}/messages", encode(conversation));
        self.json(self.request(Method::POST, &path).json(body)).await
    }

    pub async fn retry_send(&self, client_id: &str) -> Result<c::CommandResponse, ApiError> {
        let path = format!("/api/v1/commands/{}/retry", encode(client_id));
        let body = c::RetrySendInputBody {
            accept_duplicate: false,
        };
        self.json(self.request(Method::POST, &path).json(&body)).await
    }

    pub async fn mark_read(&self, conversation: &str) -> Result<(), ApiError> {
        let path = format!("/api/v1/conversations/{}", encode(conversation));
        let body = c::ConversationPatchBody {
            read: Some(true),
            ..Default::default()
        };
        self.send(self.request(Method::PATCH, &path).json(&body))
            .await
            .map(drop)
    }

    pub async fn report_activity(&self, report: &c::Report) -> Result<(), ApiError> {
        self.send(self.request(Method::POST, "/api/v1/activity").json(report))
            .await
            .map(drop)
    }
}

/// `ircord_session=abc; Path=/; HttpOnly` → `ircord_session=abc`.
fn session_cookie(set_cookie: &str) -> Option<String> {
    let pair = set_cookie.split(';').next()?.trim();
    let (name, value) = pair.split_once('=')?;
    (name == SESSION_COOKIE && !value.is_empty()).then(|| pair.to_string())
}

fn encode(segment: &str) -> String {
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
        assert_eq!(encode("net/#chan nel"), "net%2F%23chan%20nel");
    }

    #[test]
    fn user_agent_names_the_platform() {
        assert!(user_agent().starts_with("IRCord-GPUI/"));
    }
}

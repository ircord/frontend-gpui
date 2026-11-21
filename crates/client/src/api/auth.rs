//! Sign-in, sessions and the account: `/api/v1/auth/*` and `/api/v1/bootstrap`.
//!
//! Package S0 added the account, password, account deletion and the
//! server's release notes (`/api/v1/changelog`); S4 adds sign-up,
//! registration and onboarding (`/auth/signup`, `/auth/registration`,
//! `/auth/username`, `/auth/onboarding/complete`).

use ircord_contract as c;
use reqwest::header;
use std::time::Duration;

use super::{Api, ApiError, Method, session_cookie};

/// `GET /api/v1/auth/session`: the signed-in account, `null` when signed out.
#[derive(Debug, Clone, Default, serde::Deserialize)]
struct SessionAnswer {
    #[serde(default)]
    account: Option<c::Account>,
}

impl Api {
    /// Check that the address is an IRCord server: `GET /api/v1/auth/session`
    /// answers without a session with `{"account": null}`.
    pub async fn verify_server(&self) -> Result<(), ApiError> {
        let failed = || ApiError::Http {
            status: 0,
            detail:
                "Could not find an IRCord server at this address. Check the address and try again."
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
        let result = self
            .send(self.request(Method::POST, "/api/v1/auth/logout"))
            .await;
        self.set_cookie(None);
        match result {
            Ok(_) | Err(ApiError::Unauthorized) => Ok(()),
            Err(e) => Err(e),
        }
    }

    pub async fn bootstrap(&self) -> Result<c::Snapshot, ApiError> {
        self.json(self.request(Method::GET, "/api/v1/bootstrap"))
            .await
    }

    /// The signed-in account (username, when it was created and when it
    /// expires without a visit); `None` when the server says nobody is.
    pub async fn account(&self) -> Result<Option<c::Account>, ApiError> {
        let answer: SessionAnswer = self.get("/api/v1/auth/session").await?;
        Ok(answer.account)
    }

    /// Change the password. A wrong `current` password is a 403 whose detail
    /// says so.
    pub async fn change_password(&self, current: &str, password: &str) -> Result<(), ApiError> {
        let body = c::PasswordChange {
            current: current.to_string(),
            password: password.to_string(),
        };
        self.call_empty(Method::POST, "/api/v1/auth/password", &body)
            .await
    }

    /// Delete the account for good. Needs its password; 403 means it is wrong.
    /// The session ends with it.
    pub async fn delete_account(&self, password: &str) -> Result<(), ApiError> {
        let body = c::AccountDeletion {
            password: password.to_string(),
        };
        self.call_empty(Method::DELETE, "/api/v1/auth/account", &body)
            .await?;
        self.set_cookie(None);
        Ok(())
    }

    /// The server's version and its release notes, newest first.
    pub async fn changelog(&self) -> Result<c::BackendChangelog, ApiError> {
        self.get("/api/v1/changelog").await
    }
}

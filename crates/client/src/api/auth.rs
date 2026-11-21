//! Sign-in, sessions and the account: `/api/v1/auth/*` and `/api/v1/bootstrap`.
//!
//! Package S0 adds password and account deletion here, S4 sign-up,
//! registration and onboarding (`/auth/signup`, `/auth/registration`,
//! `/auth/username`, `/auth/onboarding/complete`).

use ircord_contract as c;
use reqwest::header;
use std::time::Duration;

use super::{Api, ApiError, Method, session_cookie};

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
}

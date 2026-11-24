//! Sign-in, sessions and the account: `/api/v1/auth/*` and `/api/v1/bootstrap`.
//!
//! Package S0 added the account, password, account deletion and the
//! server's release notes (`/api/v1/changelog`); S4 adds sign-up,
//! registration and onboarding (`/auth/signup`, `/auth/registration`,
//! `/auth/username`, `/auth/onboarding/complete`).

use ircord_contract as c;
use reqwest::header;
use std::time::Duration;

use super::{Api, ApiError, Method, segment, session_cookie};
use crate::store::{Changes, Workspace, people};

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

    // ---- Sign-up and onboarding (S4) ----

    /// `GET /auth/registration`: whether this server takes new accounts and
    /// whether they need a registration key.
    pub async fn registration(&self) -> Result<c::RegistrationPolicy, ApiError> {
        self.get("/api/v1/auth/registration").await
    }

    /// `GET /auth/username?name=`: whether a username can be had, with the
    /// server's reason when not. A server that requires a registration key
    /// answers only with it (403 without).
    pub async fn check_username(
        &self,
        name: &str,
        key: Option<&str>,
    ) -> Result<c::UsernameCheck, ApiError> {
        let mut request = self
            .request(Method::GET, "/api/v1/auth/username")
            .query(&[("name", name)]);
        if let Some(key) = key.filter(|k| !k.is_empty()) {
            request = request.header("X-Registration-Key", key);
        }
        self.json(request).await
    }

    /// Create an account and keep its session. Returns the cookie pair to
    /// store and the new account (with the network awaiting its profile
    /// setup, if the server provisioned one).
    pub async fn signup(
        &self,
        body: &c::SignupCredentials,
    ) -> Result<(String, c::Account), ApiError> {
        let response = self
            .send(self.request(Method::POST, "/api/v1/auth/signup").json(body))
            .await?;
        let cookie = response
            .headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .find_map(session_cookie)
            .ok_or_else(|| ApiError::Decode("the sign-up response set no session".into()))?;
        self.set_cookie(Some(cookie.clone()));
        let bytes = response.bytes().await.map_err(ApiError::network)?;
        let result: c::LoginResult =
            serde_json::from_slice(&bytes).map_err(|e| ApiError::Decode(e.to_string()))?;
        Ok((cookie, result.account))
    }

    /// `POST /auth/onboarding/complete`: the first-run setup is done or
    /// skipped; the account stops naming an onboarding network.
    pub async fn complete_onboarding(&self) -> Result<(), ApiError> {
        self.send(self.request(Method::POST, "/api/v1/auth/onboarding/complete"))
            .await
            .map(drop)
    }

    /// `GET /networks/{id}/profile`: your profile on one network.
    pub async fn network_profile(&self, network: &str) -> Result<c::Member, ApiError> {
        self.get(&format!("/api/v1/networks/{}/profile", segment(network)))
            .await
    }

    /// `PATCH /profile`: the account's own profile, used while it has no
    /// network to keep one on.
    pub async fn update_profile(&self, update: &c::ProfileUpdate) -> Result<c::Member, ApiError> {
        self.call(Method::PATCH, "/api/v1/profile", update).await
    }

    /// The default pictures this server's web client ships, from
    /// `/avatars/defaults/manifest.json`; empty when it ships none.
    pub async fn default_avatars(&self) -> Result<Vec<DefaultAvatar>, ApiError> {
        let value: serde_json::Value = self.get(DEFAULT_AVATAR_MANIFEST).await?;
        Ok(parse_avatar_manifest(&value))
    }

    /// The server's version and its release notes, newest first.
    pub async fn changelog(&self) -> Result<c::BackendChangelog, ApiError> {
        self.get("/api/v1/changelog").await
    }
}

// ---- Saving what the server answers ----

/// A profile the server saved for `network` (or the account's, for `None`):
/// it replaces yours there and wherever else you show.
pub fn profile_saved(w: &mut Workspace, network: Option<&str>, member: c::Member) -> Changes {
    if let Some(n) = network.and_then(|id| w.networks.iter_mut().find(|n| n.id == id))
        && n.profile.as_ref().is_none_or(|p| p.id == member.id)
    {
        n.profile = Some(member.clone());
    }
    let mut changes = people::put_member(w, member);
    changes.sidebar = true;
    changes
}

// ---- Sign-up checks ----

/// The shortest password the sign-up form offers to send.
pub const MIN_PASSWORD: usize = 8;

/// Why a sign-up password is not sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PasswordProblem {
    Short,
    Spaces,
}

/// What is wrong with a new password, if anything: at least eight
/// characters, no spaces (it is also the IRC password).
pub fn password_problem(password: &str) -> Option<PasswordProblem> {
    if password.chars().count() < MIN_PASSWORD {
        Some(PasswordProblem::Short)
    } else if password.chars().any(char::is_whitespace) {
        Some(PasswordProblem::Spaces)
    } else {
        None
    }
}

// ---- Default pictures (the web client's `shared/utils/default-avatars.ts`) ----

/// Where the web client serves its default pictures.
pub const DEFAULT_AVATAR_PATH: &str = "/avatars/defaults/";
const DEFAULT_AVATAR_MANIFEST: &str = "/avatars/defaults/manifest.json";

/// One default picture. A chosen one is saved as its URL in the profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefaultAvatar {
    pub id: String,
    pub file: String,
    pub category: String,
    /// False for pictures that can be picked but are never suggested.
    pub suggest: bool,
}

impl DefaultAvatar {
    /// The path of its image on the server.
    pub fn path(&self) -> String {
        format!("{DEFAULT_AVATAR_PATH}{}", self.file)
    }
}

pub(crate) fn avatar_file(file: &str) -> bool {
    let Some((stem, ext)) = file.rsplit_once('.') else {
        return false;
    };
    !stem.is_empty()
        && stem
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        && ["png", "jpg", "jpeg", "webp"].contains(&ext.to_ascii_lowercase().as_str())
}

fn avatar_id(id: &str) -> bool {
    (1..=64).contains(&id.len())
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// The usable entries of a manifest, in its order; malformed or repeated
/// ones are skipped.
pub fn parse_avatar_manifest(value: &serde_json::Value) -> Vec<DefaultAvatar> {
    let Some(list) = value.get("avatars").and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    let mut seen = std::collections::HashSet::new();
    let mut avatars = Vec::new();
    for entry in list {
        let text = |key: &str| entry.get(key).and_then(|v| v.as_str());
        let (Some(id), Some(file)) = (text("id"), text("file")) else {
            continue;
        };
        if !avatar_id(id) || !avatar_file(file) || !seen.insert(id.to_string()) {
            continue;
        }
        avatars.push(DefaultAvatar {
            id: id.to_string(),
            file: file.to_string(),
            category: text("category").unwrap_or_default().to_string(),
            suggest: entry.get("suggest").and_then(|v| v.as_bool()) != Some(false),
        });
    }
    avatars
}

/// 32-bit FNV-1a over the UTF-8 bytes of `text`.
pub fn fnv1a(text: &str) -> u32 {
    text.bytes().fold(0x811c_9dc5u32, |hash, byte| {
        (hash ^ u32::from(byte)).wrapping_mul(0x0100_0193)
    })
}

/// Which of `count` entries `text` picks: FNV-1a finished with the murmur3
/// mixer, so every bit of every character counts.
pub fn avatar_index(text: &str, count: usize) -> usize {
    let mut h = fnv1a(text);
    h ^= h >> 16;
    h = h.wrapping_mul(0x85eb_ca6b);
    h ^= h >> 13;
    h = h.wrapping_mul(0xc2b2_ae35);
    h ^= h >> 16;
    h as usize % count.max(1)
}

/// The picture suggested for a username: the same name (any case) gets the
/// same one while the set stays the same.
pub fn suggest_avatar<'a>(
    username: &str,
    avatars: &'a [DefaultAvatar],
) -> Option<&'a DefaultAvatar> {
    let name = username.trim().to_lowercase();
    let candidates: Vec<&DefaultAvatar> = avatars.iter().filter(|a| a.suggest).collect();
    if name.is_empty() || candidates.is_empty() {
        return None;
    }
    Some(candidates[avatar_index(&name, candidates.len())])
}

/// The default a saved avatar URL points at on `origin`, if it is one.
pub fn default_avatar_for<'a>(
    url: Option<&str>,
    avatars: &'a [DefaultAvatar],
    origin: &str,
) -> Option<&'a DefaultAvatar> {
    let url = url.filter(|u| !u.is_empty())?;
    let path = if url.starts_with('/') {
        url
    } else {
        url.strip_prefix(origin.trim_end_matches('/'))?
    };
    let file = path.strip_prefix(DEFAULT_AVATAR_PATH)?;
    avatars.iter().find(|a| a.file == file)
}

// ---- The channel guide at onboarding ----

/// Whether the guide offers anything to choose beyond what everyone joins
/// and what you are in (the web client's `guideHasChoices`); the rest of
/// the guide's helpers are in `api::channels`.
pub fn guide_has_choices(
    guide: &c::ChannelGuideView,
    joined: &std::collections::HashSet<String>,
) -> bool {
    guide.available
        && guide.entries.iter().any(|c| {
            c.state != c::ChannelGuideChannelState::Everyone
                && !joined.contains(&super::channels::guide_key(&c.channel))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn suggestions_match_the_web_client() {
        // Values from the web client's own functions (Node, Math.imul).
        assert_eq!(fnv1a("alice"), 2267157479);
        assert_eq!(fnv1a("zoë"), 1125401212);
        for (name, of48, of7) in [
            ("alice", 30, 5),
            ("bob", 22, 4),
            ("user1", 3, 6),
            ("userq", 4, 0),
            ("zoë", 34, 4),
        ] {
            assert_eq!(avatar_index(name, 48), of48, "{name}");
            assert_eq!(avatar_index(name, 7), of7, "{name}");
        }
        let avatars: Vec<DefaultAvatar> = (1..=7)
            .map(|i| DefaultAvatar {
                id: format!("a{i}"),
                file: format!("a{i}.png"),
                category: String::new(),
                suggest: true,
            })
            .collect();
        assert_eq!(suggest_avatar(" Alice ", &avatars).unwrap().id, "a6");
        assert_eq!(suggest_avatar("", &avatars), None);
    }

    #[test]
    fn the_manifest_skips_what_is_malformed_or_repeated() {
        let list = parse_avatar_manifest(&json!({"avatars": [
            {"id": "a1", "file": "a1.png", "category": "cat"},
            {"id": "a1", "file": "again.png"},
            {"id": "bad id", "file": "x.png"},
            {"id": "a2", "file": "../a2.png"},
            {"id": "a3", "file": "a3.JPG", "suggest": false},
            {"id": "a4"}
        ]}));
        let ids: Vec<_> = list.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(ids, ["a1", "a3"]);
        assert!(!list[1].suggest);
        assert_eq!(list[0].path(), "/avatars/defaults/a1.png");
        assert!(parse_avatar_manifest(&json!({"nope": 1})).is_empty());
        assert_eq!(
            default_avatar_for(
                Some("https://x.test/avatars/defaults/a3.JPG"),
                &list,
                "https://x.test"
            )
            .map(|a| a.id.as_str()),
            Some("a3")
        );
        assert_eq!(
            default_avatar_for(
                Some("https://y.test/avatars/defaults/a1.png"),
                &list,
                "https://x.test"
            ),
            None
        );
    }

    #[test]
    fn passwords_need_eight_characters_and_no_spaces() {
        assert_eq!(password_problem("short"), Some(PasswordProblem::Short));
        assert_eq!(
            password_problem("long enough"),
            Some(PasswordProblem::Spaces)
        );
        assert_eq!(password_problem("longenough"), None);
    }

    #[test]
    fn the_guide_has_choices_beyond_everyone_and_what_you_are_in() {
        let guide: c::ChannelGuideView = serde_json::from_value(json!({
            "available": true, "revision": 1, "groups": [],
            "entries": [
                {"channel": "#lobby", "state": "everyone", "nsfw": false},
                {"channel": "#Chess", "state": "suggested", "nsfw": false}
            ]
        }))
        .unwrap();
        let none = std::collections::HashSet::new();
        assert!(guide_has_choices(&guide, &none));
        let all: std::collections::HashSet<String> = ["#chess"].map(String::from).into();
        assert!(!guide_has_choices(&guide, &all));
    }

    #[test]
    fn signs_up_checks_names_and_completes_onboarding_against_the_canned_server() {
        use crate::client::runtime;
        use crate::testing::{CannedServer, Reply};
        let rt = runtime().unwrap();
        let server = CannedServer::start(&rt);
        server.route(
            "GET",
            "/api/v1/auth/registration",
            200,
            json!({"registrationEnabled": true, "registrationKeyRequired": true}),
        );
        server.route_fn("GET", "/api/v1/auth/username", |r| {
            let name = r.query_param("name").unwrap_or_default();
            let available = name != "taken" && r.header("x-registration-key") == Some("k1");
            Reply::json(
                200,
                json!({"available": available, "reason": (!available).then_some("No.")}),
            )
        });
        server.route_reply(
            "POST",
            "/api/v1/auth/signup",
            Reply::json(
                201,
                json!({"account": {"id": "a1", "username": "newbie", "onboardingNetworkId": "ircord",
                    "createdAt": "2026-11-21T00:00:00Z", "expiresAt": "2027-11-21T00:00:00Z",
                    "lastSeenAt": "2026-11-21T00:00:00Z"}}),
            )
            .with_header("Set-Cookie", "ircord_session=fresh; Path=/; HttpOnly"),
        );
        server.route_reply("POST", "/api/v1/auth/onboarding/complete", Reply::empty());
        let api = server.api();
        let policy = rt.block_on(api.registration()).unwrap();
        assert!(policy.registration_enabled && policy.registration_key_required);
        assert!(
            rt.block_on(api.check_username("newbie", Some("k1")))
                .unwrap()
                .available
        );
        let taken = rt
            .block_on(api.check_username("taken", Some("k1")))
            .unwrap();
        assert_eq!(
            (taken.available, taken.reason.as_deref()),
            (false, Some("No."))
        );
        let body = c::SignupCredentials {
            username: "newbie".into(),
            password: "longenough".into(),
            registration_key: Some("k1".into()),
            display_name: None,
        };
        let (cookie, account) = rt.block_on(api.signup(&body)).unwrap();
        assert_eq!(cookie, "ircord_session=fresh");
        assert_eq!(api.cookie().as_deref(), Some("ircord_session=fresh"));
        assert_eq!(account.onboarding_network_id.as_deref(), Some("ircord"));
        let sent = server.requests_to("POST", "/api/v1/auth/signup");
        assert_eq!(sent[0].json()["registrationKey"], "k1");
        assert!(sent[0].json().get("displayName").is_none());
        rt.block_on(api.complete_onboarding()).unwrap();
        let done = server.requests_to("POST", "/api/v1/auth/onboarding/complete");
        assert_eq!(done[0].header("cookie"), Some("ircord_session=fresh"));
    }
}

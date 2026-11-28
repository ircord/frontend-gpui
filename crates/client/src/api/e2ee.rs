//! End-to-end encryption: `/api/v1/e2ee/…` (experimental). The backend
//! keeps a key directory, an ordered log per MLS group, which group each
//! direct conversation uses, and a short-lived relay for device approval.
//! Every key, package, commit, Welcome and relayed message is opaque to it;
//! these calls move bytes, and [`crate::e2ee`] decides what they mean.
//!
//! Byte fields travel as standard base64, as the backend's Go `[]byte`
//! encodes them; [`bytes`] and [`encode`] convert.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use ircord_contract as c;
use serde_json::json;

use super::{Api, ApiError, Method, segment};

/// Bytes as the backend sends them.
pub fn bytes(text: &str) -> Result<Vec<u8>, ApiError> {
    STANDARD
        .decode(text)
        .map_err(|e| ApiError::Decode(format!("base64: {e}")))
}

/// Bytes as the backend takes them.
pub fn encode(bytes: &[u8]) -> String {
    STANDARD.encode(bytes)
}

impl Api {
    /// Your identity key and devices.
    pub async fn e2ee_directory(&self) -> Result<c::E2EEOwnDirectory, ApiError> {
        self.get("/api/v1/e2ee/identity").await
    }

    /// Publish your identity key; with `reset`, replace a different one
    /// (starting fresh).
    pub async fn e2ee_publish_identity(
        &self,
        identity_key: &[u8],
        reset: bool,
    ) -> Result<c::E2EEIdentity, ApiError> {
        let body = json!({"identityKey": encode(identity_key), "reset": reset});
        self.call(Method::PUT, "/api/v1/e2ee/identity", &body).await
    }

    pub async fn e2ee_publish_device(
        &self,
        device: &str,
        generation: i64,
        certificate: &[u8],
        last_resort: Option<&[u8]>,
        label: &str,
    ) -> Result<(), ApiError> {
        let mut body = json!({
            "generation": generation,
            "certificate": encode(certificate),
            "label": label,
        });
        if let Some(lr) = last_resort {
            body["lastResort"] = json!(encode(lr));
        }
        let path = format!("/api/v1/e2ee/devices/{}", segment(device));
        self.call_empty(Method::PUT, &path, &body).await
    }

    pub async fn e2ee_delete_device(&self, device: &str) -> Result<(), ApiError> {
        self.delete(&format!("/api/v1/e2ee/devices/{}", segment(device)))
            .await
    }

    /// Add single-use key packages; answers the stock now.
    pub async fn e2ee_add_key_packages(
        &self,
        device: &str,
        packages: &[Vec<u8>],
    ) -> Result<i64, ApiError> {
        let body = json!({"keyPackages": packages.iter().map(|p| encode(p)).collect::<Vec<_>>()});
        let path = format!("/api/v1/e2ee/devices/{}/key-packages", segment(device));
        let stock: c::E2EEStock = self.call(Method::POST, &path, &body).await?;
        Ok(stock.key_packages)
    }

    /// Someone's identity key and devices, by account.
    pub async fn e2ee_account(&self, account: &str) -> Result<c::E2EEPeerDirectory, ApiError> {
        self.get(&format!("/api/v1/e2ee/accounts/{}", segment(account)))
            .await
    }

    /// One key package for each of someone's devices (your own account too).
    pub async fn e2ee_claim(&self, account: &str) -> Result<c::E2EEClaim, ApiError> {
        let path = format!(
            "/api/v1/e2ee/accounts/{}/key-packages/claim",
            segment(account)
        );
        self.call(Method::POST, &path, &json!({})).await
    }

    pub async fn e2ee_create_group(
        &self,
        body: &c::E2EECreateGroupInputBody,
    ) -> Result<c::E2EEGroup, ApiError> {
        self.call(Method::POST, "/api/v1/e2ee/groups", body).await
    }

    pub async fn e2ee_group(&self, group: &str) -> Result<c::E2EEGroup, ApiError> {
        self.get(&format!("/api/v1/e2ee/groups/{}", segment(group)))
            .await
    }

    /// Post a commit made in `body.epoch`. A 409 means another commit won.
    pub async fn e2ee_commit(
        &self,
        group: &str,
        body: &c::E2EECommitInputBody,
    ) -> Result<i64, ApiError> {
        let path = format!("/api/v1/e2ee/groups/{}/commits", segment(group));
        let epoch: c::E2EEEpoch = self.call(Method::POST, &path, body).await?;
        Ok(epoch.epoch)
    }

    /// Commits from epoch `from` on, oldest first.
    pub async fn e2ee_commits(
        &self,
        group: &str,
        from: i64,
    ) -> Result<Vec<c::E2EECommit>, ApiError> {
        let path = format!("/api/v1/e2ee/groups/{}/commits", segment(group));
        let page: c::ItemsE2EECommit = self
            .get_query(
                &path,
                &[("from", from.to_string()), ("limit", "100".into())],
            )
            .await?;
        Ok(page.items)
    }

    pub async fn e2ee_welcomes(&self, device: &str) -> Result<Vec<c::E2EEWelcome>, ApiError> {
        let path = format!("/api/v1/e2ee/devices/{}/welcomes", segment(device));
        let page: c::ItemsE2EEWelcome = self.get(&path).await?;
        Ok(page.items)
    }

    pub async fn e2ee_delete_welcome(&self, id: i64) -> Result<(), ApiError> {
        self.delete(&format!("/api/v1/e2ee/welcomes/{id}")).await
    }

    pub async fn e2ee_conversation(&self, id: &str) -> Result<c::E2EEConversation, ApiError> {
        self.get(&format!("/api/v1/e2ee/conversations/{}", segment(id)))
            .await
    }

    /// Bind a direct conversation to a group: the backend then refuses
    /// plaintext into it. `replaces` names the group a new one replaces.
    pub async fn e2ee_bind(
        &self,
        id: &str,
        group: &str,
        peer: &str,
        replaces: Option<&str>,
    ) -> Result<c::E2EEConversation, ApiError> {
        let body = c::E2EEBindInputBody {
            group_id: group.to_string(),
            peer: peer.to_string(),
            replaces: replaces.map(str::to_string),
        };
        let path = format!("/api/v1/e2ee/conversations/{}", segment(id));
        self.call(Method::PUT, &path, &body).await
    }

    /// Turn encryption off in a conversation whose current group is `group`.
    pub async fn e2ee_turn_off(&self, id: &str, group: &str) -> Result<(), ApiError> {
        let path = format!(
            "/api/v1/e2ee/conversations/{}?groupId={}",
            segment(id),
            segment(group)
        );
        self.delete(&path).await
    }

    pub async fn e2ee_relay(&self, from: &str, to: &str, payload: &[u8]) -> Result<i64, ApiError> {
        let body = c::E2EEApprovalInputBody {
            from_device: from.to_string(),
            to_device: to.to_string(),
            payload: encode(payload),
        };
        let id: c::E2EEApprovalID = self
            .call(Method::POST, "/api/v1/e2ee/approvals", &body)
            .await?;
        Ok(id.id)
    }

    pub async fn e2ee_approvals(&self, device: &str) -> Result<Vec<c::E2EEApproval>, ApiError> {
        let path = format!("/api/v1/e2ee/devices/{}/approvals", segment(device));
        let page: c::ItemsE2EEApproval = self.get(&path).await?;
        Ok(page.items)
    }

    pub async fn e2ee_delete_approval(&self, id: i64) -> Result<(), ApiError> {
        self.delete(&format!("/api/v1/e2ee/approvals/{id}")).await
    }

    /// Publish this account's signed device list. A 409 means a newer list
    /// is published already.
    pub async fn e2ee_put_device_list(&self, list: &[u8], counter: i64) -> Result<(), ApiError> {
        let body = c::E2EEDeviceList {
            list: encode(list),
            counter,
        };
        let _: c::E2EEDeviceList = self
            .call(Method::PUT, "/api/v1/e2ee/device-list", &body)
            .await?;
        Ok(())
    }

    // ---- Encrypted chats carried over IRC (direct chats across servers, groups) ----

    /// Your encrypted chats carried over IRC, with invitations not answered yet.
    pub async fn e2ee_chats(&self) -> Result<Vec<c::E2EEChat>, ApiError> {
        let page: c::ItemsE2EEChat = self.get("/api/v1/e2ee/chats").await?;
        Ok(page.items)
    }

    pub async fn e2ee_create_chat(
        &self,
        body: &c::E2EECreateChatInputBody,
    ) -> Result<c::E2EECreatedChat, ApiError> {
        self.call(Method::POST, "/api/v1/e2ee/chats", body).await
    }

    /// Enter a group's channel (or bind the direct conversation) after an invitation.
    pub async fn e2ee_join_chat(&self, group: &str) -> Result<c::E2EEChat, ApiError> {
        let path = format!("/api/v1/e2ee/chats/{}/join", segment(group));
        self.call(Method::POST, &path, &json!({})).await
    }

    pub async fn e2ee_decline_chat(&self, group: &str) -> Result<(), ApiError> {
        let path = format!("/api/v1/e2ee/chats/{}/decline", segment(group));
        self.call_empty(Method::POST, &path, &json!({})).await
    }

    /// Leave a chat; with `close`, the founder closes a group for everyone.
    pub async fn e2ee_leave_chat(&self, group: &str, close: bool) -> Result<(), ApiError> {
        let mut path = format!("/api/v1/e2ee/chats/{}", segment(group));
        if close {
            path.push_str("?close=true");
        }
        self.delete(&path).await
    }

    /// Send a commit; answers once it settled, whether it came first.
    pub async fn e2ee_chat_commit(
        &self,
        group: &str,
        body: &c::E2EEChatCommitInputBody,
    ) -> Result<c::E2EEChatCommitResult, ApiError> {
        let path = format!("/api/v1/e2ee/chats/{}/commits", segment(group));
        self.call(Method::POST, &path, body).await
    }

    /// The commits this server saw from epoch `from` on, by epoch and, within
    /// one, in position order.
    pub async fn e2ee_chat_commits(
        &self,
        group: &str,
        from: i64,
    ) -> Result<Vec<c::E2EEChatCommit>, ApiError> {
        let path = format!("/api/v1/e2ee/chats/{}/commits", segment(group));
        let page: c::ItemsE2EEChatCommit = self
            .get_query(
                &path,
                &[("from", from.to_string()), ("limit", "100".into())],
            )
            .await?;
        Ok(page.items)
    }

    pub async fn e2ee_chat_welcomes(
        &self,
        device: &str,
    ) -> Result<Vec<c::E2EEChatWelcome>, ApiError> {
        let path = format!("/api/v1/e2ee/devices/{}/chat-welcomes", segment(device));
        let page: c::ItemsE2EEChatWelcome = self.get(&path).await?;
        Ok(page.items)
    }

    pub async fn e2ee_delete_chat_welcome(&self, id: i64) -> Result<(), ApiError> {
        self.delete(&format!("/api/v1/e2ee/chat-welcomes/{id}"))
            .await
    }

    /// Download an encrypted file, at most `max` bytes: from this server
    /// with the session, or from another IRCord server's upload address over
    /// HTTPS without any credentials (a file someone on another server sent).
    pub async fn e2ee_download(&self, url: &str, max: usize) -> Result<Vec<u8>, ApiError> {
        let target = self.file_url(url);
        let parsed = url::Url::parse(&target).ok();
        let own = parsed
            .as_ref()
            .is_some_and(|u| u.origin().ascii_serialization() == self.server().origin());
        if !own {
            return foreign_download(parsed, max).await;
        }
        let path = url::Url::parse(&target)
            .map(|u| match u.query() {
                Some(q) => format!("{}?{q}", u.path()),
                None => u.path().to_string(),
            })
            .unwrap_or_default();
        let mut response = self.send(self.request(Method::GET, &path)).await?;
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(ApiError::network)? {
            if body.len() + chunk.len() > max {
                return Err(ApiError::Http {
                    status: 413,
                    detail: "The file is too large to open here.".into(),
                });
            }
            body.extend_from_slice(&chunk);
        }
        Ok(body)
    }
}

/// Fetch an encrypted file another IRCord server holds: only its public
/// upload address (`/files/…`), over HTTPS (plain HTTP only to this
/// machine, for a local server), with no session, no cookies and no
/// redirects. The file is sealed; its key is in the message.
async fn foreign_download(url: Option<url::Url>, max: usize) -> Result<Vec<u8>, ApiError> {
    let refused = || ApiError::Http {
        status: 400,
        detail: "An encrypted file must come from an IRCord server's upload address.".into(),
    };
    let url = url.ok_or_else(refused)?;
    let local = url.host().is_some_and(|h| match h {
        url::Host::Domain(d) => d == "localhost",
        url::Host::Ipv4(ip) => ip.is_loopback(),
        url::Host::Ipv6(ip) => ip.is_loopback(),
    });
    let secure = url.scheme() == "https" || (url.scheme() == "http" && local);
    if !secure || !url.path().starts_with("/files/") || !url.username().is_empty() {
        return Err(refused());
    }
    let http = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(ApiError::network)?;
    let mut response = http.get(url).send().await.map_err(ApiError::network)?;
    if !response.status().is_success() {
        return Err(ApiError::Http {
            status: response.status().as_u16(),
            detail: "The other server did not hand out the file.".into(),
        });
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(ApiError::network)? {
        if body.len() + chunk.len() > max {
            return Err(ApiError::Http {
                status: 413,
                detail: "The file is too large to open here.".into(),
            });
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

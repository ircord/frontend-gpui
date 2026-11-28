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

    /// Download an encrypted file from this server, at most `max` bytes.
    pub async fn e2ee_download(&self, url: &str, max: usize) -> Result<Vec<u8>, ApiError> {
        let target = self.file_url(url);
        let own = url::Url::parse(&target)
            .ok()
            .is_some_and(|u| u.origin().ascii_serialization() == self.server().origin());
        if !own {
            return Err(ApiError::Http {
                status: 400,
                detail: "An encrypted file must come from this server.".into(),
            });
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

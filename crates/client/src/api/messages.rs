//! Messages: `/conversations/{id}/messages`, `/messages/{id}`, `/commands/*`
//! and `/outbox`.
//!
//! Editing, deleting, reactions (`/conversations/{id}/reactions`),
//! discarding a failed send and reading the outbox are package A1's.

use ircord_contract as c;

use super::{Api, ApiError, Method, segment};

impl Api {
    /// The newest page, or the page before `before` (a `nextCursor`).
    pub async fn messages(
        &self,
        conversation: &str,
        before: Option<&str>,
        limit: u32,
    ) -> Result<c::MessagePage, ApiError> {
        let path = format!("/api/v1/conversations/{}/messages", segment(conversation));
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
        let path = format!("/api/v1/conversations/{}/messages", segment(conversation));
        self.json(self.request(Method::POST, &path).json(body))
            .await
    }

    pub async fn retry_send(&self, client_id: &str) -> Result<c::CommandResponse, ApiError> {
        let path = format!("/api/v1/commands/{}/retry", segment(client_id));
        let body = c::RetrySendInputBody {
            accept_duplicate: false,
        };
        self.json(self.request(Method::POST, &path).json(&body))
            .await
    }

    /// Replace the text of one of your messages for everyone.
    pub async fn edit_message(
        &self,
        conversation: &str,
        message: &str,
        text: &str,
    ) -> Result<c::EditResponse, ApiError> {
        let path = format!(
            "/api/v1/conversations/{}/messages/{}",
            segment(conversation),
            segment(message)
        );
        let body = c::EditInputBody {
            text: text.to_string(),
        };
        self.json(self.request(Method::PATCH, &path).json(&body))
            .await
    }

    /// Delete a message for everyone. `reason` and `delete_files` are for a
    /// server owner moderating.
    pub async fn delete_message(
        &self,
        conversation: &str,
        message: &str,
        reason: Option<&str>,
        delete_files: bool,
    ) -> Result<c::RedactResponse, ApiError> {
        let path = format!(
            "/api/v1/conversations/{}/messages/{}",
            segment(conversation),
            segment(message)
        );
        let mut request = self.request(Method::DELETE, &path);
        if let Some(reason) = reason.filter(|r| !r.is_empty()) {
            request = request.query(&[("reason", reason)]);
        }
        if delete_files {
            request = request.query(&[("deleteFiles", "true")]);
        }
        self.json(request).await
    }

    /// Add your reaction with `emoji`, or take it back if you already reacted
    /// with it. The answer is the message as it now stands.
    pub async fn toggle_reaction(
        &self,
        conversation: &str,
        message: &str,
        emoji: &str,
    ) -> Result<c::Message, ApiError> {
        let path = format!("/api/v1/conversations/{}/reactions", segment(conversation));
        let body = c::ReactionInputBody {
            emoji: emoji.to_string(),
            message_id: message.to_string(),
        };
        self.json(self.request(Method::POST, &path).json(&body))
            .await
    }

    /// Give up on a retained send.
    pub async fn discard_send(&self, client_id: &str) -> Result<(), ApiError> {
        self.delete(&format!("/api/v1/commands/{}", segment(client_id)))
            .await
    }

    /// One page of the account's unresolved sends, oldest first; pass the last
    /// entry's `createdAt` and `clientId` for the next.
    pub async fn outbox(&self, after: Option<(&str, &str)>) -> Result<c::OutboxPage, ApiError> {
        let mut request = self.request(Method::GET, "/api/v1/outbox");
        if let Some((at, id)) = after {
            request = request.query(&[("after", at), ("afterId", id)]);
        }
        self.json(request).await
    }

    /// Every unresolved send, following the pages.
    pub async fn outbox_all(&self) -> Result<Vec<c::OutboxEntry>, ApiError> {
        let mut all: Vec<c::OutboxEntry> = Vec::new();
        loop {
            let after = all
                .last()
                .map(|e| (e.created_at.clone(), e.client_id.clone()));
            let page = self
                .outbox(after.as_ref().map(|(a, b)| (a.as_str(), b.as_str())))
                .await?;
            let more = page.more && !page.items.is_empty();
            all.extend(page.items);
            if !more {
                return Ok(all);
            }
        }
    }
}

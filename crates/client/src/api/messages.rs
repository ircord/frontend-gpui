//! Messages: `/conversations/{id}/messages`, `/messages/{id}`, `/commands/*`
//! and `/outbox`.
//!
//! Package A1 adds editing, deleting, reactions (`/conversations/{id}/reactions`),
//! discarding a failed send and reading the outbox.

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
}

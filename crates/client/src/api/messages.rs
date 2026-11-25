//! Messages: `/conversations/{id}/messages`, `/messages/{id}`, `/commands/*`
//! and `/outbox`.
//!
//! Reading around a message and locating one are package R1's.
//! Editing, deleting, reactions (`/conversations/{id}/reactions`),
//! discarding a failed send and reading the outbox are package A1's.

use ircord_contract as c;

use super::{Api, ApiError, Method, segment};

impl Api {
    /// The newest page, or the page before a server cursor or a message
    /// that marks the edge of the in-memory timeline.
    pub async fn messages(
        &self,
        conversation: &str,
        before: Option<&str>,
        limit: u32,
    ) -> Result<c::MessagePage, ApiError> {
        let message_id = before
            .and_then(|value| value.strip_prefix("message:"))
            .and_then(|value| url::form_urlencoded::parse(value.as_bytes()).next())
            .map(|(id, _)| id.into_owned());
        let boundary = match message_id.as_deref() {
            Some(id) => {
                let around = self.messages_around(conversation, id, 1).await?;
                let Some(cursor) = around.next_cursor else {
                    return Ok(c::MessagePage {
                        cursor: around.cursor,
                        ..Default::default()
                    });
                };
                Some(cursor)
            }
            None => None,
        };
        let path = format!("/api/v1/conversations/{}/messages", segment(conversation));
        let mut request = self
            .request(Method::GET, &path)
            .query(&[("limit", limit.to_string())]);
        if let Some(before) = boundary.as_deref().or(before) {
            request = request.query(&[("before", before)]);
        }
        self.json(request).await
    }

    /// A window of history centred on `message`, for jumping to an old
    /// message and for paging towards the present (`hasNewer`).
    pub async fn messages_around(
        &self,
        conversation: &str,
        message: &str,
        limit: u32,
    ) -> Result<c::MessagePage, ApiError> {
        let path = format!("/api/v1/conversations/{}/messages", segment(conversation));
        let request = self.request(Method::GET, &path).query(&[
            ("limit", limit.to_string()),
            ("around", message.to_string()),
        ]);
        self.json(request).await
    }

    /// Your copy of a message, found by its ID or its shared ID; its
    /// `conversationId` names the conversation it is in.
    pub async fn locate_message(&self, message: &str) -> Result<c::Message, ApiError> {
        let path = format!("/api/v1/messages/{}", segment(message));
        self.json(self.request(Method::GET, &path)).await
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

#[cfg(test)]
mod tests {
    use crate::testing::{CannedServer, Reply};

    #[test]
    fn pages_before_a_trimmed_message_using_the_servers_cursor() {
        let runtime = crate::client::runtime().unwrap();
        let server = CannedServer::start(&runtime);
        let path = "/api/v1/conversations/c1/messages";
        server.route_fn("GET", path, |request| {
            if request.query_param("around").as_deref() == Some("m&42") {
                Reply::json(
                    200,
                    serde_json::json!({"items": [], "nextCursor": "older+/"}),
                )
            } else if request.query_param("before").as_deref() == Some("older+/") {
                Reply::json(200, serde_json::json!({"items": [], "nextCursor": "next"}))
            } else {
                Reply::problem(400, "Expected a message ID or the server cursor.")
            }
        });
        let page = runtime
            .block_on(server.api().messages("c1", Some("message:m%2642"), 20))
            .unwrap();
        assert_eq!(page.next_cursor.as_deref(), Some("next"));
        assert_eq!(server.requests_to("GET", path).len(), 2);
    }
}

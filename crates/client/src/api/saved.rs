//! Saved messages: `/saved-messages`, `/saved-messages/{messageId}`, `/saved-message-ids`.
//!
//! Owned by package FD2. Add calls in this `impl Api` block with the
//! helpers in `api/mod.rs` and the types in `ircord_contract`.

use ircord_contract::{ItemsSavedMessage, SaveInputBody, SavedItem, SavedPage};

use super::{Api, ApiError, Method, segment};

impl Api {
    /// One page of saved messages, newest saved first, keeping only those
    /// whose text contains `query` when it is not empty.
    pub async fn saved_messages(
        &self,
        query: &str,
        cursor: Option<&str>,
        limit: u32,
    ) -> Result<SavedPage, ApiError> {
        let mut params = vec![("limit", limit.to_string())];
        if !query.trim().is_empty() {
            params.push(("q", query.trim().to_string()));
        }
        if let Some(cursor) = cursor {
            params.push(("cursor", cursor.to_string()));
        }
        self.get_query("/api/v1/saved-messages", &params).await
    }

    /// Which messages are saved (references only).
    pub async fn saved_message_ids(&self) -> Result<ItemsSavedMessage, ApiError> {
        self.get("/api/v1/saved-message-ids").await
    }

    /// Save `message` from `conversation` for later.
    pub async fn save_message(
        &self,
        conversation: &str,
        message: &str,
    ) -> Result<SavedItem, ApiError> {
        let path = format!("/api/v1/saved-messages/{}", segment(message));
        let body = SaveInputBody {
            conversation_id: conversation.to_string(),
        };
        self.call(Method::PUT, &path, &body).await
    }

    /// Remove a saved message.
    pub async fn unsave_message(&self, message: &str) -> Result<(), ApiError> {
        let path = format!("/api/v1/saved-messages/{}", segment(message));
        self.delete(&path).await
    }
}

//! Conversations: `PATCH`/`DELETE /conversations/{id}`, `POST /conversations`,
//! `/conversations/{id}/attract`, `/conversations/{id}/transfer`.
//!
//! Read state, favourites, closing a conversation and attract live here; R1
//! and N1 add their calls. Channel administration is in `channels.rs`.

use ircord_contract as c;

use super::{Api, ApiError, Method, segment};

impl Api {
    pub async fn mark_read(&self, conversation: &str) -> Result<(), ApiError> {
        let path = format!("/api/v1/conversations/{}", segment(conversation));
        let body = c::ConversationPatchBody {
            read: Some(true),
            ..Default::default()
        };
        self.send(self.request(Method::PATCH, &path).json(&body))
            .await
            .map(drop)
    }
}

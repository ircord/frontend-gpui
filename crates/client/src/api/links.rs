//! Shared links: `/links`, and link media on a message (`/conversations/{id}/messages/{messageId}/link-media`, used by L1).
//!
//! Owned by package FD3. Add calls in this `impl Api` block with the
//! helpers in `api/mod.rs` and the types in `ircord_contract`.

use ircord_contract as c;

#[allow(unused_imports)]
use super::{Api, ApiError, Method, segment};

impl Api {
    /// What the server found behind `url`, a link in the text of a message
    /// you can read: media from its cache, a file, a card or a player.
    /// Content it could not or would not fetch comes back as a card with a
    /// reason, not as an error.
    pub async fn resolve_link_media(
        &self,
        conversation: &str,
        message: &str,
        url: &str,
    ) -> Result<c::LinkMediaResponse, ApiError> {
        let path = format!(
            "/api/v1/conversations/{}/messages/{}/link-media",
            segment(conversation),
            segment(message)
        );
        let body = c::LinkMediaInputBody {
            url: url.to_string(),
            widget: None,
        };
        self.call(Method::POST, &path, &body).await
    }
}

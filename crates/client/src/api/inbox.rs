//! The inbox: `/inbox`, `/inbox/summary`, `/inbox/{messageId}/dismissed`.
//!
//! Owned by package FD2. Add calls in this `impl Api` block with the
//! helpers in `api/mod.rs` and the types in `ircord_contract`.

use ircord_contract::{InboxPage, InboxSummary};

use super::{Api, ApiError, Method, segment};

impl Api {
    /// One page of the inbox: `filter` is `all`, `unread`, `mentions`,
    /// `replies`, `direct` or `dismissed`; `cursor` continues an earlier page.
    pub async fn inbox(
        &self,
        filter: &str,
        cursor: Option<&str>,
        limit: u32,
    ) -> Result<InboxPage, ApiError> {
        let mut query = vec![("filter", filter.to_string()), ("limit", limit.to_string())];
        if let Some(cursor) = cursor {
            query.push(("cursor", cursor.to_string()));
        }
        self.get_query("/api/v1/inbox", &query).await
    }

    /// How many undismissed inbox items are still unread: the badge.
    pub async fn inbox_summary(&self) -> Result<InboxSummary, ApiError> {
        self.get("/api/v1/inbox/summary").await
    }

    /// Dismiss an inbox item, or restore a dismissed one. Neither marks its
    /// conversation read.
    pub async fn set_inbox_dismissed(
        &self,
        message: &str,
        dismissed: bool,
    ) -> Result<(), ApiError> {
        let method = if dismissed {
            Method::PUT
        } else {
            Method::DELETE
        };
        let path = format!("/api/v1/inbox/{}/dismissed", segment(message));
        self.send(self.request(method, &path)).await.map(drop)
    }
}

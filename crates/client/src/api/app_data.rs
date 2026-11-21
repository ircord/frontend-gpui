//! The account's documents: `/app/preferences`, `/app/appearance`, `/app/themes`, `/app/library`, `/app-data/{kind}`, `/app/emoji/catalog`, `/changelog`. The revisioned ones are driven through `crate::documents`.
//!
//! Owned by package S1, S2, E1. Add calls in this `impl Api` block with the
//! helpers in `api/mod.rs` and the types in `ircord_contract`.

#[allow(unused_imports)]
use super::{Api, ApiError, Method, segment};

// ---- S1: personal themes ----

use ircord_contract::{SaveAppThemeInputBody, Theme, Themes};

impl Api {
    /// Add or replace one personal theme; answers the whole library.
    /// `expected` is the `updatedAt` the editor loaded (`None` for a new
    /// theme); the server refuses with 412 when the stored one moved on,
    /// unless `force`.
    pub async fn save_theme(
        &self,
        theme: &Theme,
        expected: Option<&str>,
        force: bool,
    ) -> Result<Themes, ApiError> {
        let id = theme.id.as_deref().unwrap_or_default();
        let path = format!("/api/v1/app/themes/{}", segment(id));
        let body = SaveAppThemeInputBody {
            expected: expected.map(str::to_string),
            force: force.then_some(true),
            theme: theme.clone(),
        };
        self.call(Method::PUT, &path, &body).await
    }

    /// Remove one personal theme; answers the whole library.
    pub async fn delete_theme(&self, id: &str) -> Result<Themes, ApiError> {
        let path = format!("/api/v1/app/themes/{}", segment(id));
        self.json(self.request(Method::DELETE, &path)).await
    }
}

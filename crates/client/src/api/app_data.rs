//! The account's documents: `/app/preferences`, `/app/appearance`, `/app/themes`, `/app/library`, `/app-data/{kind}`, `/app/emoji/catalog`, `/changelog`. The revisioned ones are driven through `crate::documents`.
//!
//! Owned by package S1, S2, E1. Add calls in this `impl Api` block with the
//! helpers in `api/mod.rs` and the types in `ircord_contract`.

#[allow(unused_imports)]
use super::{Api, ApiError, Method, segment};
use ircord_contract as c;

// ---- S1: personal themes ----

use ircord_contract::{SaveAppThemeInputBody, Theme, Themes};

const BACKGROUND_MAX_BYTES: usize = 25 << 20;

impl Api {
    /// Read a theme's image from this instance without following redirects.
    pub async fn theme_background(&self, path: &str) -> Result<Vec<u8>, ApiError> {
        if !crate::store::appearance::background::is_upload_path(path) {
            return Err(ApiError::Decode("Invalid theme background path.".into()));
        }
        let mut response = self
            .send(
                self.request(Method::GET, path)
                    .header(reqwest::header::ACCEPT, "image/*"),
            )
            .await?;
        let too_large = || ApiError::Decode("The theme background is too large.".into());
        if response
            .content_length()
            .is_some_and(|len| len > BACKGROUND_MAX_BYTES as u64)
        {
            return Err(too_large());
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(ApiError::network)? {
            if bytes.len() + chunk.len() > BACKGROUND_MAX_BYTES {
                return Err(too_large());
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes)
    }

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

// ---- E1: the picker library ----

impl Api {
    /// Add emoji picks to the picker library (E1); answers the library.
    pub async fn update_library(&self, patch: &c::LibraryPatch) -> Result<c::Library, ApiError> {
        self.call(Method::PATCH, "/api/v1/app/library", patch).await
    }

    /// Save a GIF to the picker library, or move it to the front (E1).
    pub async fn save_gif(&self, gif: &c::SavePickerGIF) -> Result<c::Library, ApiError> {
        self.call(Method::POST, "/api/v1/app/library/gifs", gif)
            .await
    }

    /// Take a saved GIF out of the picker library (E1).
    pub async fn delete_gif(&self, id: &str) -> Result<c::Library, ApiError> {
        let path = format!("/api/v1/app/library/gifs/{}", segment(id));
        self.json(self.request(Method::DELETE, &path)).await
    }
}

//! The account's documents: `/app/preferences`, `/app/appearance`, `/app/themes`, `/app/library`, `/app-data/{kind}`, `/app/emoji/catalog`, `/changelog`. The revisioned ones are driven through `crate::documents`.
//!
//! Owned by package S1, S2, E1. Add calls in this `impl Api` block with the
//! helpers in `api/mod.rs` and the types in `ircord_contract`.

#[allow(unused_imports)]
use super::{Api, ApiError, Method, segment};
use ircord_contract as c;

// ---- S1: personal themes ----

use crate::store::appearance::background::{MAX_IMAGE_BYTES, is_upload_path};
use ircord_contract::{SaveAppThemeInputBody, Theme, ThemeConflict, Themes};

#[derive(Debug)]
pub enum ThemeSave {
    Saved(Themes),
    Conflict(Box<Theme>),
}

impl Api {
    /// Read a theme's image from this instance without following redirects.
    pub async fn theme_background(&self, path: &str) -> Result<Vec<u8>, ApiError> {
        if !is_upload_path(path) {
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
            .is_some_and(|len| len > MAX_IMAGE_BYTES as u64)
        {
            return Err(too_large());
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(ApiError::network)? {
            if bytes.len() + chunk.len() > MAX_IMAGE_BYTES {
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
    ) -> Result<ThemeSave, ApiError> {
        let id = theme.id.as_deref().unwrap_or_default();
        let path = format!("/api/v1/app/themes/{}", segment(id));
        let body = SaveAppThemeInputBody {
            expected: expected.map(str::to_string),
            force: force.then_some(true),
            theme: theme.clone(),
        };
        let response = self
            .request(Method::PUT, &path)
            .json(&body)
            .send()
            .await
            .map_err(ApiError::network)?;
        if response.status() == reqwest::StatusCode::PRECONDITION_FAILED {
            let conflict: ThemeConflict = Self::read_json(response).await?;
            let theirs = conflict.data.theirs;
            if theirs.id.as_deref() == Some(id)
                && theirs
                    .updated_at
                    .as_ref()
                    .is_some_and(|stamp| !stamp.is_empty())
            {
                return Ok(ThemeSave::Conflict(Box::new(theirs)));
            }
            return Err(ApiError::Http {
                status: 412,
                detail: conflict.detail.or(conflict.title).unwrap_or_else(|| {
                    "The server didn't provide the current version of this theme.".into()
                }),
            });
        }
        let themes = Self::read_json(Self::check_response(response).await?).await?;
        Ok(ThemeSave::Saved(themes))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::runtime;
    use crate::testing::CannedServer;
    use serde_json::json;

    #[test]
    fn saving_a_theme_preserves_the_conflicting_revision_and_auth_errors() {
        const PATH: &str = "/api/v1/app/themes/t-saved";
        let runtime = runtime().unwrap();
        let server = CannedServer::start(&runtime);
        let api = server.api();
        let draft = Theme {
            id: Some("t-saved".into()),
            name: Some("Draft".into()),
            ..Default::default()
        };
        server.route(
            "PUT",
            PATH,
            412,
            json!({"data": {"theirs": {
                "id": "t-saved", "name": "The saved copy", "updatedAt": "new-revision"
            }}}),
        );
        let result = runtime.block_on(api.save_theme(&draft, Some("old-revision"), false));
        assert!(matches!(result, Ok(ThemeSave::Conflict(saved))
            if saved.updated_at.as_deref() == Some("new-revision")
                && saved.name.as_deref() == Some("The saved copy")));

        server.route("PUT", PATH, 412, json!({"detail": "Theme changed"}));
        let result = runtime.block_on(api.save_theme(&draft, Some("old-revision"), false));
        assert!(matches!(result, Err(ApiError::Http { status: 412, detail })
            if detail == "Theme changed"));

        server.route("PUT", PATH, 401, json!({}));
        assert!(matches!(
            runtime.block_on(api.save_theme(&draft, Some("old-revision"), false)),
            Err(ApiError::Unauthorized)
        ));
    }
}

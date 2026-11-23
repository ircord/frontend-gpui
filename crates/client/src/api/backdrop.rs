//! A personal theme's background picture: an upload on the signed-in server.

use super::{Api, ApiError, Method};
use crate::store::appearance::is_upload_path;

/// A theme's picture is not read past this size.
const PICTURE_MAX: usize = 25 << 20;

impl Api {
    /// An upload a theme names as its background (`/files/…` or
    /// `/api/v1/uploads/…`), with the session, so only this server is asked.
    /// A path that is not an upload is refused before any request.
    pub async fn theme_picture(&self, path: &str) -> Result<Vec<u8>, ApiError> {
        if !is_upload_path(path) {
            return Err(ApiError::Decode(format!("{path} is not an upload")));
        }
        let response = self.send(self.request(Method::GET, path)).await?;
        let bytes = response
            .bytes()
            .await
            .map_err(|e| ApiError::Network(e.to_string()))?;
        if bytes.len() > PICTURE_MAX {
            return Err(ApiError::Decode(format!(
                "the picture is larger than {PICTURE_MAX} bytes"
            )));
        }
        Ok(bytes.to_vec())
    }
}

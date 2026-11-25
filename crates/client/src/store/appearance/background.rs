//! Size and path constraints for a theme background upload.

pub const MAX_IMAGE_BYTES: usize = 25 << 20;

/// Store the instance path from an upload response, as Theme Studio does.
/// Any host in the response is discarded. Background readers use their own server.
pub(super) fn instance_upload_path(address: &str, server: &url::Url) -> Option<String> {
    let url = server.join(address).ok()?;
    (matches!(url.scheme(), "http" | "https")
        && url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none()
        && is_upload_path(url.path()))
    .then(|| url.path().to_string())
}

/// Match the server's allowed background paths. The stored path is always
/// relative to the signed-in server, and never carries a query or fragment.
pub(crate) fn is_upload_path(path: &str) -> bool {
    let part = |text: &str, max, punctuation: &[u8]| {
        !text.is_empty()
            && text.len() <= max
            && text
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || punctuation.contains(&byte))
    };
    if let Some(id) = path.strip_prefix("/api/v1/uploads/") {
        return part(id, 80, b"-");
    }
    let Some(rest) = path.strip_prefix("/files/") else {
        return false;
    };
    let mut fields = rest.split('/');
    let (Some(id), Some(token), Some(name), None) =
        (fields.next(), fields.next(), fields.next(), fields.next())
    else {
        return false;
    };
    part(id, 80, b"-_")
        && part(token, 200, b"-_")
        && part(name, 400, b"._~%$&+,:;=@-")
        && !matches!(
            name.to_ascii_lowercase().as_str(),
            "." | ".." | "%2e" | "%2e%2e" | ".%2e" | "%2e."
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_instance_upload_paths_can_be_loaded() {
        for path in ["/files/a/_b/a%20b.webp", "/api/v1/uploads/u-1"] {
            assert!(is_upload_path(path));
        }
        for path in [
            "//elsewhere.example/files/a/b/a.png",
            "https://elsewhere.example/files/a/b/a.png",
            "/files/a/b/%2e%2e",
            "/files/a/b/a.png?query",
            "/api/v1/uploads/a/another",
            "/api/v1/account",
        ] {
            assert!(!is_upload_path(path), "{path}");
        }
    }

    #[test]
    fn saved_upload_paths_discard_the_response_origin_but_keep_an_escaped_name() {
        let server = url::Url::parse("https://chat.example.org").unwrap();
        assert_eq!(
            instance_upload_path(
                "https://files.example.org/files/id/tok/my%20photo.png",
                &server
            )
            .as_deref(),
            Some("/files/id/tok/my%20photo.png")
        );
        assert_eq!(
            instance_upload_path("/files/id/tok/a.png?different", &server),
            None
        );
        assert_eq!(
            instance_upload_path("file:///files/id/tok/a.png", &server),
            None
        );
    }
}

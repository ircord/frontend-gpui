//! Uploads: `/uploads`, `/uploads/{id}`, `/networks/{id}/uploads`, `/files/*`.
//!
//! Owned by package U1. A batch of files becomes one message: each file is
//! uploaded on its own ([`Api::upload_file`], with progress and a stall
//! watchdog), then [`batch_message`] names every upload in a send whose text
//! is their URLs, so other IRC clients can open the same files. The checks
//! before a file joins a batch ([`check`]) follow `useUploads.ts` in the web
//! client.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures::StreamExt as _;
use ircord_contract as c;
use reqwest::header;

use super::{Api, ApiError, Method, segment};

/// At most this many files wait to be sent in one conversation.
pub const MAX_FILES: usize = 10;

/// An upload that makes no progress for this long is given up, however long
/// the whole upload takes (the web client's `UPLOAD_STALL_MS`). The backend
/// still limits the time to read one request.
pub const STALL: Duration = Duration::from_secs(60);

/// How long one upload may take in all; the stall watchdog ends it sooner.
const UPLOAD_TIMEOUT: Duration = Duration::from_secs(60 * 60);

/// The size of each piece of the body, so progress moves smoothly.
const CHUNK: usize = 64 * 1024;

/// A file read into memory, ready to upload.
#[derive(Debug, Clone)]
pub struct FileData {
    pub name: String,
    pub content_type: String,
    pub bytes: Arc<Vec<u8>>,
}

impl FileData {
    pub fn new(name: impl Into<String>, content_type: impl Into<String>, bytes: Vec<u8>) -> Self {
        Self {
            name: name.into(),
            content_type: content_type.into(),
            bytes: Arc::new(bytes),
        }
    }

    /// A file named `name`, its type guessed from the name.
    pub fn named(name: impl Into<String>, bytes: Vec<u8>) -> Self {
        let name = name.into();
        let content_type = content_type_for(&name).to_string();
        Self::new(name, content_type, bytes)
    }

    pub fn size(&self) -> u64 {
        self.bytes.len() as u64
    }

    pub fn is_image(&self) -> bool {
        self.content_type.starts_with("image/")
    }
}

/// Bytes of one upload sent so far, shared between the upload and the view.
#[derive(Debug, Clone, Default)]
pub struct Progress(Arc<AtomicU64>);

impl Progress {
    pub fn sent(&self) -> u64 {
        self.0.load(Ordering::Relaxed)
    }

    /// The fraction sent, 0 to 1.
    pub fn fraction(&self, total: u64) -> f32 {
        if total == 0 {
            return 0.0;
        }
        (self.sent() as f32 / total as f32).clamp(0.0, 1.0)
    }

    fn add(&self, n: u64) {
        self.0.fetch_add(n, Ordering::Relaxed);
    }

    fn reset(&self) {
        self.0.store(0, Ordering::Relaxed);
    }
}

/// Why an upload did not finish.
#[derive(Debug, Clone, thiserror::Error)]
pub enum UploadError {
    /// The connection broke before the server answered.
    #[error("The upload was interrupted.")]
    Interrupted,
    /// Nothing moved for [`STALL`].
    #[error("The upload stopped making progress.")]
    Stalled,
    /// The server refused it (too large, over quota, not allowed); the
    /// detail is the server's own wording.
    #[error("{0}")]
    Refused(ApiError),
}

impl From<ApiError> for UploadError {
    fn from(e: ApiError) -> Self {
        match e {
            ApiError::Network(_) => UploadError::Interrupted,
            other => UploadError::Refused(other),
        }
    }
}

/// Why a file cannot join the batch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rejection {
    /// The server takes no uploads (no `capabilities.upload`).
    Unavailable,
    /// [`MAX_FILES`] are already waiting.
    TooMany,
    /// Not one of the server's types.
    Unsupported,
    /// Larger than the server's limit, in bytes.
    TooLarge(i64),
}

/// Whether `content_type` matches one of the server's patterns (`image/*`,
/// `application/pdf`).
pub fn accepts(types: &[String], content_type: &str) -> bool {
    types
        .iter()
        .any(|pattern| match pattern.strip_suffix("/*") {
            Some(prefix) => content_type
                .strip_prefix(prefix)
                .is_some_and(|rest| rest.starts_with('/')),
            None => pattern == content_type,
        })
}

/// Whether a file of `content_type` and `size` may join a batch that already
/// holds `queued` files.
pub fn check(
    capability: Option<&c::UploadCapability>,
    queued: usize,
    content_type: &str,
    size: u64,
) -> Result<(), Rejection> {
    let Some(limits) = capability else {
        return Err(Rejection::Unavailable);
    };
    if queued >= MAX_FILES {
        return Err(Rejection::TooMany);
    }
    if !accepts(&limits.types, content_type) {
        return Err(Rejection::Unsupported);
    }
    if limits.max_bytes > 0 && size > limits.max_bytes as u64 {
        return Err(Rejection::TooLarge(limits.max_bytes));
    }
    Ok(())
}

/// The content type for a file name, from its extension. The server sniffs
/// the bytes itself; this decides only what the batch accepts and previews.
pub fn content_type_for(name: &str) -> &'static str {
    let ext = name
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" | "jfif" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "bmp" => "image/bmp",
        "heic" => "image/heic",
        "svg" => "image/svg+xml",
        "mp4" | "m4v" => "video/mp4",
        "webm" => "video/webm",
        "mov" => "video/quicktime",
        "mkv" => "video/x-matroska",
        "mp3" => "audio/mpeg",
        "m4a" => "audio/mp4",
        "ogg" | "oga" => "audio/ogg",
        "opus" => "audio/opus",
        "wav" => "audio/wav",
        "flac" => "audio/flac",
        "pdf" => "application/pdf",
        "zip" => "application/zip",
        "gz" => "application/gzip",
        "7z" => "application/x-7z-compressed",
        "json" => "application/json",
        "txt" | "log" | "md" => "text/plain",
        "csv" => "text/csv",
        _ => "application/octet-stream",
    }
}

/// A human size: `2.5 MB`, `120 kB`, `900 B` (decimal units, as the web).
pub fn human_size(bytes: u64) -> String {
    let b = bytes as f64;
    if b >= 1e9 {
        format!("{:.1} GB", b / 1e9)
    } else if b >= 1e6 {
        format!("{:.1} MB", b / 1e6)
    } else if b >= 1e3 {
        format!("{:.0} kB", b / 1e3)
    } else {
        format!("{bytes} B")
    }
}

/// The send for a finished batch: the uploads' absolute URLs as the text, in
/// order, and their IDs as the attachments.
pub fn batch_message(
    client_id: String,
    uploads: &[(c::Upload, String)],
    reply_to: Option<String>,
) -> c::MessageInputBody {
    c::MessageInputBody {
        client_id,
        text: uploads
            .iter()
            .map(|(_, url)| url.as_str())
            .collect::<Vec<_>>()
            .join(" "),
        attachments: Some(uploads.iter().map(|(u, _)| u.id.clone()).collect()),
        kind: Some(c::MessageInputBodyKind::Message),
        reply_to_id: reply_to,
        ..Default::default()
    }
}

/// `Content-Disposition` wants a quoted name without quotes or line breaks.
fn quoted_name(name: &str) -> String {
    name.chars()
        .map(|ch| match ch {
            '"' | '\r' | '\n' | '\\' => '_',
            ch => ch,
        })
        .collect()
}

impl Api {
    /// An upload's URL as others open it: absolute on this server.
    pub fn file_url(&self, url: &str) -> String {
        url::Url::parse(url)
            .map(|u| u.to_string())
            .unwrap_or_else(|_| self.server().join(url).to_string())
    }

    /// `POST /api/v1/uploads?networkId=`: upload one file as
    /// `multipart/form-data`, counting the bytes sent into `progress`. Gives
    /// up with [`UploadError::Stalled`] when nothing moves for `stall` (use
    /// [`STALL`]). Dropping the future cancels the upload.
    pub async fn upload_file(
        &self,
        network: &str,
        file: &FileData,
        progress: &Progress,
        stall: Duration,
    ) -> Result<c::Upload, UploadError> {
        progress.reset();
        let boundary = format!("ircord-{}", crate::client::random_id());
        let head = format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{}\"\r\nContent-Type: {}\r\n\r\n",
            quoted_name(&file.name),
            file.content_type
        )
        .into_bytes();
        let tail = format!("\r\n--{boundary}--\r\n").into_bytes();
        let total = head.len() + file.bytes.len() + tail.len();

        let moved = Arc::new(Mutex::new(Instant::now()));
        let mut pieces: Vec<Vec<u8>> = vec![head];
        pieces.extend(file.bytes.chunks(CHUNK).map(<[u8]>::to_vec));
        pieces.push(tail);
        let counted = {
            let (progress, moved) = (progress.clone(), moved.clone());
            let file_len = file.bytes.len() as u64;
            let mut file_sent = 0u64;
            futures::stream::iter(pieces).map(move |piece| {
                *moved.lock().unwrap() = Instant::now();
                // Only the file's own bytes count, so 100% means the file is sent.
                let n = (piece.len() as u64).min(file_len - file_sent);
                file_sent += n;
                progress.add(n);
                Ok::<_, std::io::Error>(piece)
            })
        };

        let request = self
            .request(Method::POST, "/api/v1/uploads")
            .query(&[("networkId", network)])
            .header(
                header::CONTENT_TYPE,
                format!("multipart/form-data; boundary={boundary}"),
            )
            .header(header::CONTENT_LENGTH, total)
            .timeout(UPLOAD_TIMEOUT)
            .body(reqwest::Body::wrap_stream(counted));
        let send = self.json::<c::Upload>(request);
        let tick = (stall / 4).min(Duration::from_secs(1));
        let watchdog = async {
            loop {
                tokio::time::sleep(tick).await;
                if moved.lock().unwrap().elapsed() >= stall {
                    break;
                }
            }
        };
        tokio::select! {
            result = send => result.map_err(UploadError::from),
            _ = watchdog => Err(UploadError::Stalled),
        }
    }

    /// `DELETE /api/v1/uploads/{id}`: forget an upload that was never sent.
    pub async fn delete_upload(&self, id: &str) -> Result<(), ApiError> {
        self.delete(&format!("/api/v1/uploads/{}", segment(id)))
            .await
    }

    /// `GET /api/v1/networks/{id}/uploads`: where this network's files go.
    pub async fn upload_provider(&self, network: &str) -> Result<c::Provider, ApiError> {
        self.get(&format!("/api/v1/networks/{}/uploads", segment(network)))
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn capability(types: &[&str], max: i64) -> c::UploadCapability {
        c::UploadCapability {
            max_bytes: max,
            types: types.iter().map(|t| t.to_string()).collect(),
        }
    }

    #[test]
    fn patterns_match_like_the_web() {
        let types = ["image/*".to_string(), "application/pdf".to_string()];
        assert!(accepts(&types, "image/png"));
        assert!(accepts(&types, "application/pdf"));
        assert!(!accepts(&types, "imagex/png"));
        assert!(!accepts(&types, "video/mp4"));
    }

    #[test]
    fn checks_before_a_file_joins() {
        let cap = capability(&["image/*"], 1000);
        assert_eq!(check(None, 0, "image/png", 1), Err(Rejection::Unavailable));
        assert_eq!(check(Some(&cap), 0, "image/png", 1000), Ok(()));
        assert_eq!(
            check(Some(&cap), 0, "image/png", 1001),
            Err(Rejection::TooLarge(1000))
        );
        assert_eq!(
            check(Some(&cap), 0, "text/plain", 1),
            Err(Rejection::Unsupported)
        );
        assert_eq!(
            check(Some(&cap), MAX_FILES, "image/png", 1),
            Err(Rejection::TooMany)
        );
    }

    #[test]
    fn a_batch_is_its_urls_in_order() {
        let upload = |id: &str| c::Upload {
            id: id.into(),
            ..Default::default()
        };
        let body = batch_message(
            "k".into(),
            &[
                (upload("a"), "https://x/files/a/t/one.png".into()),
                (upload("b"), "https://x/files/b/t/two.pdf".into()),
            ],
            None,
        );
        assert_eq!(
            body.text,
            "https://x/files/a/t/one.png https://x/files/b/t/two.pdf"
        );
        assert_eq!(body.attachments, Some(vec!["a".into(), "b".into()]));
    }

    #[test]
    fn types_and_sizes() {
        assert_eq!(content_type_for("Cat.JPG"), "image/jpeg");
        assert_eq!(content_type_for("noext"), "application/octet-stream");
        assert_eq!(human_size(2_500_000), "2.5 MB");
        assert_eq!(human_size(900), "900 B");
        assert_eq!(quoted_name("a\"b\r\n.png"), "a_b__.png");
    }
}

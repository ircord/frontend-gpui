//! Automatically retain previewable images from authorized archived messages.
//! File bytes survive a history refresh. Access still requires a current
//! reference in that session's archive.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Result, bail};
use http::header;
use ircord_contract::Message;
use ring::hmac;
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use tokio::sync::broadcast;
use tokio::time::{Instant, sleep_until};

use super::{Archive, Signal, blocking, now_ns, retention_start};
use crate::api::{Api, Method};
use crate::format::{blocks::text_outside_code, find_links};

const MAX_FILE: usize = 32 << 20;
const BUDGET: i64 = 256 << 20;
const RETRY_NS: i64 = 60 * 60 * 1_000_000_000;

struct Candidate {
    key: Vec<u8>,
    conversation: String,
    message: String,
    url: String,
    at: i64,
}

impl Archive {
    pub(super) fn media_path(&self) -> PathBuf {
        self.0.path.with_extension("media.sqlite3")
    }

    pub(crate) async fn watch_media(self, api: Api, mut signals: broadcast::Receiver<Signal>) {
        let mut live = false;
        let mut next = Instant::now();
        loop {
            tokio::select! {
                signal = signals.recv() => match signal {
                    Ok(Signal::Live(_)) => { live = true; next = Instant::now(); }
                    Ok(Signal::Offline) => live = false,
                    Err(broadcast::error::RecvError::Closed) => return,
                    _ => {}
                },
                _ = self.0.media_changed.notified(), if live => {
                    next = next.min(Instant::now() + Duration::from_millis(250));
                },
                _ = sleep_until(next), if live => {
                    let cache = self.clone();
                    let pending = blocking(move || cache.next_media()).await;
                    match pending {
                        Ok(Some(candidate)) => {
                            let download = fetch(&api, &candidate).await;
                            let cache = self.clone();
                            if let Err(error) = blocking(move || cache.record(candidate, download.ok(), BUDGET)).await {
                                log::debug!("save offline media: {error}");
                            }
                            next = Instant::now() + Duration::from_millis(250);
                        }
                        Ok(None) => next = Instant::now() + Duration::from_secs(60),
                        Err(error) => {
                            log::debug!("scan offline media: {error}");
                            next = Instant::now() + Duration::from_secs(60);
                        }
                    }
                }
            }
        }
    }

    pub(crate) async fn media(&self, url: &str, max: usize) -> Option<(Option<String>, Vec<u8>)> {
        let key = self.file_key(url)?;
        let archive = self.clone();
        blocking(move || archive.read(|db, _| {
            if !has_ref(db, &key)? || !archive.media_path().is_file() {
                return Ok(None);
            }
            let files = Connection::open_with_flags(archive.media_path(), OpenFlags::SQLITE_OPEN_READ_ONLY)?;
            let row: Option<(Option<String>, Vec<u8>)> = files.query_row(
                "SELECT content_type,payload FROM files WHERE key=?1 AND size<=?2 AND payload IS NOT NULL",
                params![key, i64::try_from(max)?],
                |row| Ok((row.get(0)?, row.get(1)?)),
            ).optional()?;
            row.map(|(content, bytes)| Ok((content, archive.open_bytes(&key, bytes)?))).transpose()
        }).map(Option::flatten)).await.ok().flatten()
    }

    fn file_key(&self, raw: &str) -> Option<Vec<u8>> {
        let url = url::Url::parse(raw).ok()?;
        if !matches!(url.scheme(), "http" | "https")
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return None;
        }
        Some(
            hmac::sign(&self.0.media_key, url.as_str().as_bytes())
                .as_ref()
                .to_vec(),
        )
    }

    fn next_media(&self) -> Result<Option<Candidate>> {
        self.read(|db, _| {
            let files = self.open_files()?;
            files.execute("ATTACH DATABASE ?1 AS history", [self.0.path.to_string_lossy().as_ref()])?;
            files.execute(
                "DELETE FROM files WHERE NOT EXISTS (
                    SELECT 1 FROM history.media_refs r JOIN history.messages m ON (m.conversation,m.id)=(r.conversation,r.message)
                    WHERE r.key=files.key AND (m.at>=?1 OR m.pinned=1))",
                [retention_start()],
            )?;
            drop(files);
            db.execute("ATTACH DATABASE ?1 AS media", [self.media_path().to_string_lossy().as_ref()])?;
            let row = db.query_row(
                "SELECT r.key,r.conversation,r.message,r.payload,m.at FROM media_refs r
                 JOIN messages m ON (m.conversation,m.id)=(r.conversation,r.message)
                 LEFT JOIN media.files f ON f.key=r.key
                 WHERE (m.at>=?1 OR m.pinned=1) AND (f.key IS NULL OR (f.payload IS NULL AND f.retry<=?2))
                 ORDER BY m.pinned DESC,m.at DESC LIMIT 1",
                params![retention_start(), now_ns()],
                |row| Ok((row.get::<_, Vec<u8>>(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
            ).optional()?;
            row.map(|(key, conversation, message, payload, at)| {
                let url = self.open(&key, payload)?;
                Ok(Candidate { key, conversation, message, url, at })
            }).transpose()
        }).map(Option::flatten)
    }

    fn open_files(&self) -> Result<Connection> {
        let path = self.media_path();
        if !path.is_file() {
            let parent = path.parent().expect("the archive is in a directory");
            let staging = tempfile::NamedTempFile::new_in(parent)?;
            staging.persist_noclobber(&path)?;
        }
        let db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        db.pragma_update(None, "secure_delete", true)?;
        db.execute_batch(
            "CREATE TABLE IF NOT EXISTS files(key BLOB PRIMARY KEY, content_type TEXT, payload BLOB,
                 size INTEGER NOT NULL DEFAULT 0, stored INTEGER NOT NULL DEFAULT 0, retry INTEGER NOT NULL DEFAULT 0) WITHOUT ROWID;",
        )?;
        Ok(db)
    }

    pub(super) fn attach_files(&self, db: &Connection) -> Result<()> {
        let attached: bool = db.query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_database_list WHERE name='media')",
            [],
            |row| row.get(0),
        )?;
        if !attached {
            drop(self.open_files()?);
            db.execute(
                "ATTACH DATABASE ?1 AS media",
                [self.media_path().to_string_lossy().as_ref()],
            )?;
        }
        Ok(())
    }

    fn record(&self, candidate: Candidate, download: Option<Download>, budget: i64) -> Result<()> {
        self.read(|db, _| {
            if !has_ref(db, &candidate.key)? {
                return Ok(());
            }
            let mut files = self.open_files()?;
            files.execute("ATTACH DATABASE ?1 AS history", [self.0.path.to_string_lossy().as_ref()])?;
            let tx = files.transaction()?;
            if let Some((content_type, bytes)) = download {
                let size = i64::try_from(bytes.len())?;
                let mut used: i64 = tx.query_row("SELECT coalesce(sum(size),0) FROM files", [], |row| row.get(0))?;
                let pinned: bool = db.query_row(
                    "SELECT EXISTS(SELECT 1 FROM media_refs r JOIN messages m ON (m.conversation,m.id)=(r.conversation,r.message) WHERE r.key=?1 AND m.pinned=1)",
                    [&candidate.key], |row| row.get(0),
                )?;
                let evictable = "FROM files WHERE payload IS NOT NULL AND (stored<=?1 OR ?2) AND NOT EXISTS(
                    SELECT 1 FROM history.media_refs r JOIN history.messages m ON (m.conversation,m.id)=(r.conversation,r.message)
                    WHERE r.key=files.key AND m.pinned=1)";
                let reclaimable: i64 = tx.query_row(
                    &format!("SELECT coalesce(sum(size),0) {evictable}"), params![candidate.at, pinned], |row| row.get(0),
                )?;
                let fits = used - reclaimable + size <= budget;
                while fits && used + size > budget {
                    let (key, removed): (Vec<u8>, i64) = tx.query_row(
                        &format!("SELECT key,size {evictable} ORDER BY stored,key LIMIT 1"),
                        params![candidate.at, pinned], |row| Ok((row.get(0)?, row.get(1)?)),
                    )?;
                    // Suppress automatically evicted files so the queue cannot download them again in a loop.
                    tx.execute("UPDATE files SET payload=NULL,size=0,retry=?2 WHERE key=?1", params![key, i64::MAX])?;
                    used -= removed;
                }
                if fits {
                    tx.execute("INSERT OR REPLACE INTO files VALUES (?1,?2,?3,?4,?5,0)",
                        params![candidate.key, content_type, self.seal_bytes(&candidate.key, bytes)?, size, candidate.at])?;
                } else {
                    tx.execute("INSERT OR REPLACE INTO files(key,retry) VALUES (?1,?2)", params![candidate.key, i64::MAX])?;
                }
            } else {
                tx.execute("INSERT OR REPLACE INTO files(key,retry) VALUES (?1,?2)", params![candidate.key, now_ns() + RETRY_NS])?;
            }
            tx.commit()?;
            Ok(())
        })?;
        Ok(())
    }
}

pub(super) fn create_refs(db: &Connection) -> Result<()> {
    db.execute_batch(
        "CREATE TABLE media_refs(conversation TEXT NOT NULL,message TEXT NOT NULL,key BLOB NOT NULL,payload BLOB NOT NULL,
             PRIMARY KEY(conversation,message,key));
         CREATE INDEX media_keys ON media_refs(key);
         CREATE TRIGGER message_deleted AFTER DELETE ON messages BEGIN
             DELETE FROM media_refs WHERE conversation=old.conversation AND message=old.id;
         END;",
    )?;
    Ok(())
}

pub(super) fn store_refs(
    db: &Connection,
    archive: &Archive,
    message: &Message,
    pinned: bool,
) -> Result<()> {
    db.prepare_cached("DELETE FROM media_refs WHERE conversation=?1 AND message=?2")?
        .execute(params![message.conversation_id, message.id])?;
    if message.deleted == Some(true) {
        return Ok(());
    }
    let insert = |raw: &str| -> Result<()> {
        if let Some(key) = archive.file_key(raw) {
            db.prepare_cached("INSERT OR IGNORE INTO media_refs VALUES (?1,?2,?3,?4)")?
                .execute(params![
                    message.conversation_id,
                    message.id,
                    key,
                    archive.seal(&key, &raw)?
                ])?;
        }
        Ok(())
    };
    for attachment in message
        .attachments
        .as_deref()
        .unwrap_or(message.attachment.as_slice())
    {
        if pinned
            || attachment
                .content_type
                .as_deref()
                .is_some_and(|mime| mime.starts_with("image/"))
            || image_path(&attachment.name)
        {
            insert(&attachment.url)?;
        }
    }
    if let Some(gif) = &message.gif {
        insert(&gif.url)?;
        insert(&gif.still_url)?;
    }
    let outside = text_outside_code(&message.text);
    for (span, _) in find_links(&outside) {
        let raw = &outside[span];
        if url::Url::parse(raw)
            .ok()
            .is_some_and(|url| image_path(url.path()))
        {
            insert(raw)?;
        }
    }
    Ok(())
}

fn image_path(raw: &str) -> bool {
    raw.rsplit_once('.').is_some_and(|(_, suffix)| {
        matches!(
            suffix.to_ascii_lowercase().as_str(),
            "png" | "jpg" | "jpeg" | "webp" | "gif" | "avif" | "apng"
        )
    })
}

fn has_ref(db: &Connection, key: &[u8]) -> rusqlite::Result<bool> {
    db.query_row(
        "SELECT EXISTS(SELECT 1 FROM media_refs r JOIN messages m ON (m.conversation,m.id)=(r.conversation,r.message)
         WHERE r.key=?1 AND (m.at>=?2 OR m.pinned=1))",
        params![key, retention_start()], |row| row.get(0),
    )
}

type Download = (Option<String>, Vec<u8>);

async fn fetch(api: &Api, candidate: &Candidate) -> Result<Download> {
    let request = api
        .request(Method::GET, "/api/v1/offline/media")
        .header(header::ACCEPT, "*/*")
        .query(&[
            ("conversationId", &candidate.conversation),
            ("messageId", &candidate.message),
            ("url", &candidate.url),
        ])
        .timeout(Duration::from_secs(125));
    let mut response = api.send(request).await?;
    if response
        .content_length()
        .is_some_and(|length| length > MAX_FILE as u64)
    {
        bail!("offline media exceeds the file limit");
    }
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if bytes.len() + chunk.len() > MAX_FILE {
            bail!("offline media exceeds the file limit");
        }
        bytes.extend_from_slice(&chunk);
    }
    if bytes.is_empty() {
        bail!("offline media was empty");
    }
    Ok((content_type, bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ircord_contract::Attachment;
    use serde_json::json;

    use crate::Client;
    use crate::archive::{RETENTION, Staging};
    use crate::client::runtime;
    use crate::config::ConfigDir;
    use crate::testing::{CANNED_COOKIE, CannedServer, Reply, SnapshotBuilder, frame, wait_live};

    fn picture(id: &str, url: &str, hours_ago: i64) -> Message {
        Message {
            id: id.into(),
            conversation_id: "c".into(),
            created_at: (chrono::Utc::now() - chrono::TimeDelta::hours(hours_ago)).to_rfc3339(),
            attachment: Some(Attachment {
                name: "photo.png".into(),
                url: url.into(),
                content_type: Some("image/png".into()),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    #[test]
    fn downloads_only_via_the_signed_in_backend_and_invalidates_deleted_images() {
        let runtime = runtime().unwrap();
        let server = CannedServer::start(&runtime);
        let snapshot = SnapshotBuilder::new()
            .channel("c", "general")
            .cursor(4)
            .build();
        server.snapshot(&snapshot);
        let url = server.address().join("/files/photo.png").to_string();
        let original = picture("message", &url, 1);
        server.route(
            "GET",
            "/api/v1/offline/snapshot",
            200,
            json!({
                "state": snapshot,
                "cursor": 4,
                "since": (chrono::Utc::now() - RETENTION).to_rfc3339(),
                "items": [original],
            }),
        );
        server.route(
            "GET",
            "/api/v1/events/replay",
            200,
            json!({"valid": true, "items": []}),
        );
        let bytes = b"bytes returned by the approved media route".to_vec();
        server.route_reply(
            "GET",
            "/api/v1/offline/media",
            Reply::bytes(200, "image/png", bytes.clone()),
        );
        server.route("GET", "/files/photo.png", 503, json!({}));
        let temp = tempfile::tempdir().unwrap();
        let api = server
            .api()
            .with_offline_archive(&ConfigDir::at(temp.path()));
        let archive = api.archive.clone().unwrap();
        let (client, _updates) = Client::start(runtime.clone(), api.clone());
        wait_live(&client);

        runtime.block_on(async {
            tokio::time::timeout(Duration::from_secs(5), async {
                while archive.media(&url, 1024).await.is_none() {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            })
            .await
            .expect("the authorized image is downloaded");
            assert_eq!(
                api.fetch_chat_file(&url, 1024).await.unwrap(),
                (200, Some("image/png".into()), bytes.clone())
            );
            assert!(archive.media(&url, 1).await.is_none());
            let encrypted = std::fs::read(archive.media_path()).unwrap();
            assert!(!encrypted.windows(bytes.len()).any(|window| window == bytes));
            let other = Archive::new(
                temp.path(),
                &server.address().origin(),
                "ircord_session=another",
            );
            assert!(other.media(&url, 1024).await.is_none());
            let deleted = Message {
                deleted: Some(true),
                attachment: None,
                ..original
            };
            server.push(frame(5, "message.updated", &deleted));
            tokio::time::timeout(Duration::from_secs(5), async {
                while archive
                    .snapshot()
                    .await
                    .is_none_or(|state| state.cursor != 5)
                {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            })
            .await
            .unwrap();
            assert!(archive.media(&url, 1024).await.is_none());
            assert_eq!(api.fetch_chat_file(&url, 1024).await.unwrap().0, 503);
        });
        let requests = server.requests_to("GET", "/api/v1/offline/media");
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].header("cookie"), Some(CANNED_COOKIE));
        assert_eq!(
            requests[0].query_param("conversationId").as_deref(),
            Some("c")
        );
        assert_eq!(
            requests[0].query_param("messageId").as_deref(),
            Some("message")
        );
        assert_eq!(
            requests[0].query_param("url").as_deref(),
            Some(url.as_str())
        );
        assert_eq!(
            server.requests_to("GET", "/files/photo.png")[0].header("cookie"),
            None
        );
    }

    #[test]
    fn quota_keeps_recent_images_without_repeatedly_fetching_older_ones() {
        let runtime = runtime().unwrap();
        let temp = tempfile::tempdir().unwrap();
        let archive = Archive::new(temp.path(), "https://example.test", CANNED_COOKIE);
        let snapshot = SnapshotBuilder::new()
            .channel("c", "general")
            .cursor(7)
            .build();
        let new = "https://example.test/new.png";
        let old = "https://example.test/old.png";
        let mut stage = Staging::begin(archive.clone(), snapshot, retention_start()).unwrap();
        stage
            .append(vec![picture("new", new, 1), picture("old", old, 2)], 7)
            .unwrap();
        stage.finish(&[], 7).unwrap();
        stage.publish().unwrap();

        for url in [new, old] {
            let candidate = archive.next_media().unwrap().unwrap();
            assert_eq!(candidate.url, url);
            archive
                .record(candidate, Some((None, vec![1, 2, 3, 4])), 4)
                .unwrap();
        }
        assert!(archive.next_media().unwrap().is_none());
        assert!(runtime.block_on(archive.media(new, 8)).is_some());
        assert!(runtime.block_on(archive.media(old, 8)).is_none());
        runtime.block_on(archive.forget());
        assert!(!archive.media_path().exists());
    }

    #[test]
    fn opening_the_previous_archive_format_keeps_downloaded_media() {
        let runtime = runtime().unwrap();
        let temp = tempfile::tempdir().unwrap();
        let archive = Archive::new(temp.path(), "https://example.test", CANNED_COOKIE);
        let snapshot = SnapshotBuilder::new()
            .channel("c", "general")
            .cursor(7)
            .build();
        let url = "https://example.test/saved.png";
        let mut stage = Staging::begin(archive.clone(), snapshot, retention_start()).unwrap();
        stage.append(vec![picture("saved", url, 1)], 7).unwrap();
        stage.finish(&[], 7).unwrap();
        stage.publish().unwrap();
        let image = archive.next_media().unwrap().unwrap();
        archive
            .record(image, Some((None, vec![1, 2, 3, 4])), 4)
            .unwrap();
        let db = Connection::open(&archive.0.path).unwrap();
        db.execute_batch("ALTER TABLE messages DROP COLUMN pinned; PRAGMA user_version=2;")
            .unwrap();
        drop(db);

        let status = runtime.block_on(archive.pin_status("c", "saved"));
        assert!(status.available && status.saved && !status.pinned);
        assert_eq!(
            runtime.block_on(archive.media(url, 8)).unwrap().1,
            [1, 2, 3, 4]
        );
        let db = archive.open_database().unwrap();
        assert_eq!(
            db.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
                .unwrap(),
            super::super::SCHEMA_VERSION
        );
    }

    #[test]
    fn pinning_old_files_protects_them_from_eviction_and_refresh_without_resurrecting_revocations()
    {
        let runtime = runtime().unwrap();
        let server = CannedServer::start(&runtime);
        let snapshot = SnapshotBuilder::new()
            .channel("c", "general")
            .cursor(7)
            .build();
        let owner = snapshot.user.id.clone();
        let temp = tempfile::tempdir().unwrap();
        let api = server
            .api()
            .with_offline_archive(&ConfigDir::at(temp.path()));
        let archive = api.archive.clone().unwrap();
        let new = "https://example.test/new.png";
        let old = "https://example.test/old.pdf";
        let doc = Message {
            attachment: Some(Attachment {
                name: "old.pdf".into(),
                url: old.into(),
                content_type: Some("application/pdf".into()),
                ..Default::default()
            }),
            ..picture("old", old, 192)
        };
        let recent = picture("new", new, 1);
        let mut stage =
            Staging::begin(archive.clone(), snapshot.clone(), retention_start()).unwrap();
        stage.append(vec![recent.clone()], 7).unwrap();
        stage.finish(&[], 7).unwrap();
        stage.publish().unwrap();
        // Open a refresh before pinning, to verify that it cannot replace the new preference.
        let before_pin = runtime.block_on(archive.pinned_refs()).unwrap().0;
        let mut stale =
            Staging::begin(archive.clone(), snapshot.clone(), retention_start()).unwrap();
        stale.finish(&[], 7).unwrap();
        server.route(
            "POST",
            "/api/v1/offline/pinned",
            200,
            json!({"items": [doc], "cursor": 7}),
        );
        server.route(
            "GET",
            "/api/v1/offline/snapshot",
            200,
            json!({
                "state": snapshot, "cursor": 7,
                "since": (chrono::Utc::now() - RETENTION).to_rfc3339(), "items": [recent]
            }),
        );
        server.route(
            "GET",
            "/api/v1/events/replay",
            200,
            json!({"items": [], "valid": true}),
        );

        let status = runtime
            .block_on(api.set_offline_pin("c", "old", true))
            .unwrap();
        assert!(status.pinned);
        assert!(!status.saved);
        assert!(stale.publish_at(before_pin).is_err());
        for url in [old, new] {
            let candidate = archive.next_media().unwrap().unwrap();
            assert_eq!(candidate.url, url);
            archive
                .record(candidate, Some((None, vec![1, 2, 3, 4])), 4)
                .unwrap();
        }
        runtime.block_on(async {
            assert!(api.offline_pin_status("c", "old").await.saved);
            assert!(archive.media(new, 8).await.is_none());
            assert_eq!(
                archive.history_around("c", "old", 1).await.unwrap().items,
                vec![doc.clone()]
            );
            archive.download(&api, &owner).await.unwrap();
            assert!(api.offline_pin_status("c", "old").await.saved);

            let status = api.set_offline_pin("c", "old", false).await.unwrap();
            assert!(!status.pinned);
            assert!(archive.history_around("c", "old", 1).await.is_none());
            api.set_offline_pin("c", "old", true).await.unwrap();
            server.route(
                "POST",
                "/api/v1/offline/pinned",
                200,
                json!({"items": [], "cursor": 7}),
            );
            archive.download(&api, &owner).await.unwrap();
            assert!(archive.history_around("c", "old", 1).await.is_none());
            assert!(archive.media(old, 8).await.is_none());
        });
        let requests = server.requests_to("POST", "/api/v1/offline/pinned");
        assert!(requests.iter().all(|request| request.json()["items"]
            == json!([{"conversationId": "c", "messageId": "old"}])));
    }
}

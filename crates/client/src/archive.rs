//! The account's retained message archive. Download into a private staging
//! file so pagination, a disconnect or closing the app cannot publish a
//! partial snapshot. HTTP and SQLite work stay off the UI thread.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use ircord_contract::{Message, MessagePage, OfflineSnapshot, Snapshot};
use ring::{aead, digest, hmac, rand};
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::sync::mpsc;

use crate::api::{Api, ApiError};

const RETENTION: chrono::TimeDelta = chrono::TimeDelta::days(7);
const SNAPSHOT_PAGE_SIZE: u16 = 500;
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(15 * 60);
const MIN_DOWNLOAD_INTERVAL: Duration = Duration::from_secs(15 * 60);
pub(crate) const CURSOR_PREFIX: &str = "offline:";

#[derive(Clone)]
pub(crate) struct Archive(Arc<Cache>);

struct Cache {
    path: PathBuf,
    key: aead::LessSafeKey,
    /// Serialize access to the final file and fence a sync after sign-out.
    revoked: Mutex<bool>,
}

impl Archive {
    pub(crate) fn new(config: &Path, origin: &str, cookie: &str) -> Self {
        let cookie_key = hmac::Key::new(hmac::HMAC_SHA256, cookie.as_bytes());
        let tag = hmac::sign(
            &cookie_key,
            format!("ircord-offline-v1:{origin}").as_bytes(),
        );
        let file = hmac::sign(
            &cookie_key,
            format!("ircord-offline-file-v1:{origin}").as_bytes(),
        );
        let file = digest::digest(&digest::SHA256, file.as_ref());
        let file: String = file
            .as_ref()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        let key = aead::UnboundKey::new(&aead::CHACHA20_POLY1305, tag.as_ref())
            .expect("HMAC-SHA256 supplies a 32-byte key");
        Self(Arc::new(Cache {
            path: config.join("offline").join(format!("{file}.sqlite3")),
            key: aead::LessSafeKey::new(key),
            revoked: Mutex::new(false),
        }))
    }

    pub(crate) async fn watch(self, api: Api, mut connected: mpsc::Receiver<String>) {
        let mut last_attempt: Option<tokio::time::Instant> = None;
        while let Some(owner) = connected.recv().await {
            if last_attempt.is_some_and(|last| last.elapsed() < MIN_DOWNLOAD_INTERVAL) {
                continue;
            }
            last_attempt = Some(tokio::time::Instant::now());
            if let Err(error) = self.download(&api, &owner).await {
                log::debug!("refresh offline archive: {error}");
            }
        }
    }

    pub(crate) async fn download(&self, api: &Api, owner: &str) -> Result<()> {
        tokio::time::timeout(DOWNLOAD_TIMEOUT, self.download_pages(api, owner))
            .await
            .context("the offline snapshot download timed out")?
    }

    async fn download_pages(&self, api: &Api, owner: &str) -> Result<()> {
        let mut page = snapshot_page(api, None).await?;
        let Some(state) = page.state.take() else {
            bail!("the offline snapshot has no workspace");
        };
        if state.demo
            || state.user.id.is_empty()
            || state.user.id != owner
            || state.cursor != page.cursor
        {
            bail!("the offline snapshot does not match the signed-in workspace");
        }
        let cache = self.clone();
        let cutoff = time_ns(&page.since).context("the offline snapshot has no retention date")?;
        let mut stage = blocking(move || Staging::begin(cache, &state, cutoff)).await?;
        let mut seen = HashSet::new();
        loop {
            let next = page.next_cursor.take();
            stage = blocking(move || {
                stage.append(page.items)?;
                Ok(stage)
            })
            .await?;
            let Some(next) = next.filter(|value| !value.is_empty()) else {
                return blocking(move || stage.publish()).await;
            };
            if !seen.insert(next.clone()) {
                bail!("the offline snapshot repeated a page");
            }
            page = snapshot_page(api, Some(&next)).await?;
        }
    }

    pub(crate) async fn snapshot(&self) -> Option<Snapshot> {
        let cache = self.clone();
        blocking(move || cache.read(|_, snapshot| Ok(snapshot)))
            .await
            .ok()
            .flatten()
    }

    /// A newest or exclusive older page. The boundary is a message ID and
    /// stays usable when an offline timeline returns to the online API.
    pub(crate) async fn history(
        &self,
        conversation: &str,
        before: Option<&str>,
        limit: u32,
    ) -> Option<MessagePage> {
        let cache = self.clone();
        let conversation = conversation.to_owned();
        let before = before.map(str::to_owned);
        blocking(move || {
            cache
                .read(|db, snapshot| {
                    if !snapshot.conversations.iter().any(|c| c.id == conversation) {
                        return Ok(None);
                    }
                    let mut boundary_stmt = db.prepare_cached(
                        "SELECT at, position FROM messages WHERE conversation=?1 AND id=?2",
                    )?;
                    let boundary = match before.as_deref() {
                        Some(id) => {
                            let Some((at, position)) = boundary_stmt
                                .query_row(params![conversation, id], |row| {
                                    Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
                                })
                                .optional()?
                            else {
                                return Ok(None);
                            };
                            (at, position, id)
                        }
                        None => (i64::MAX, i64::MAX, ""),
                    };
                    let mut query = db.prepare_cached(
                        "SELECT payload FROM messages
                     WHERE conversation=?1 AND at>=?2 AND (at,position,id)<(?3,?4,?5)
                     ORDER BY at DESC,position DESC,id DESC LIMIT ?6",
                    )?;
                    let limit = limit.max(1);
                    let mut rows = query.query(params![
                        conversation,
                        retention_start(),
                        boundary.0,
                        boundary.1,
                        boundary.2,
                        i64::from(limit) + 1,
                    ])?;
                    let mut items = Vec::new();
                    while let Some(row) = rows.next()? {
                        let payload: Vec<u8> = row.get(0)?;
                        items.push(cache.open::<Message>(b"message", payload)?);
                    }
                    let more = items.len() > limit as usize;
                    items.truncate(limit as usize);
                    let next_cursor = items
                        .last()
                        .filter(|_| more)
                        .map(|message| format!("{CURSOR_PREFIX}{}", message.id));
                    items.reverse();
                    Ok(Some(MessagePage {
                        items,
                        next_cursor,
                        ..Default::default()
                    }))
                })
                .map(Option::flatten)
        })
        .await
        .ok()
        .flatten()
    }

    pub(crate) async fn forget(&self) {
        let cache = self.clone();
        if let Err(error) = blocking(move || {
            *cache.0.revoked.lock().unwrap() = true;
            match fs::remove_file(&cache.0.path) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
                _ => Ok(()),
            }
        })
        .await
        {
            log::warn!("forget offline archive: {error}");
        }
    }

    fn read<T>(&self, read: impl FnOnce(&Connection, Snapshot) -> Result<T>) -> Result<Option<T>> {
        let revoked = self.0.revoked.lock().unwrap();
        if *revoked || !self.0.path.is_file() {
            return Ok(None);
        }
        let db = Connection::open_with_flags(&self.0.path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let payload: Option<Vec<u8>> = db
            .query_row(
                "SELECT payload FROM workspace WHERE downloaded>=?1",
                [retention_start()],
                |row| row.get(0),
            )
            .optional()?;
        let Some(payload) = payload else {
            return Ok(None);
        };
        // A cache from another sign-in cannot authenticate with this key.
        let snapshot = self.open::<Snapshot>(b"workspace", payload)?;
        read(&db, snapshot).map(Some)
    }

    fn seal(&self, kind: &[u8], value: &impl Serialize) -> Result<Vec<u8>> {
        let mut nonce = [0; aead::NONCE_LEN];
        use rand::SecureRandom;
        rand::SystemRandom::new()
            .fill(&mut nonce)
            .map_err(|_| anyhow::anyhow!("could not obtain randomness for the offline archive"))?;
        let mut bytes = serde_json::to_vec(value)?;
        self.0
            .key
            .seal_in_place_append_tag(
                aead::Nonce::assume_unique_for_key(nonce),
                aead::Aad::from(kind),
                &mut bytes,
            )
            .map_err(|_| anyhow::anyhow!("could not encrypt offline data"))?;
        let mut sealed = nonce.to_vec();
        sealed.extend(bytes);
        Ok(sealed)
    }

    fn open<T: DeserializeOwned>(&self, kind: &[u8], mut sealed: Vec<u8>) -> Result<T> {
        let nonce = sealed
            .get(..aead::NONCE_LEN)
            .context("truncated offline data")?;
        let nonce = aead::Nonce::try_assume_unique_for_key(nonce)
            .map_err(|_| anyhow::anyhow!("invalid offline nonce"))?;
        let bytes = self
            .0
            .key
            .open_within(nonce, aead::Aad::from(kind), &mut sealed, aead::NONCE_LEN..)
            .map_err(|_| anyhow::anyhow!("the offline archive could not be authenticated"))?;
        Ok(serde_json::from_slice(bytes)?)
    }
}

struct Staging {
    db: Connection,
    path: tempfile::TempPath,
    archive: Archive,
    cutoff: i64,
    conversations: HashSet<String>,
}

impl Staging {
    fn begin(archive: Archive, snapshot: &Snapshot, cutoff: i64) -> Result<Self> {
        let dir = archive
            .0
            .path
            .parent()
            .context("offline archive directory")?;
        fs::create_dir_all(dir)?;
        let path = tempfile::Builder::new()
            .prefix(".snapshot-")
            .tempfile_in(dir)?
            .into_temp_path();
        let db = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        // Staging is never opened by readers and is discarded on failure.
        db.execute_batch(
            "PRAGMA journal_mode=OFF;
             PRAGMA synchronous=OFF;
             CREATE TABLE workspace(downloaded INTEGER NOT NULL, payload BLOB NOT NULL);
             CREATE TABLE messages(conversation TEXT NOT NULL, id TEXT NOT NULL,
                 at INTEGER NOT NULL, position INTEGER NOT NULL, payload BLOB NOT NULL,
                 PRIMARY KEY(conversation,id));
             CREATE INDEX messages_history ON messages(conversation,at,position,id);",
        )?;
        db.execute(
            "INSERT INTO workspace VALUES (?1,?2)",
            params![now_ns(), archive.seal(b"workspace", snapshot)?],
        )?;
        Ok(Self {
            db,
            path,
            archive,
            cutoff,
            conversations: snapshot
                .conversations
                .iter()
                .map(|c| c.id.clone())
                .collect(),
        })
    }

    fn append(&mut self, items: Vec<Message>) -> Result<()> {
        let tx = self.db.transaction()?;
        {
            let mut insert =
                tx.prepare_cached("INSERT OR REPLACE INTO messages VALUES (?1,?2,?3,?4,?5)")?;
            for message in items {
                if message.id.is_empty() || !self.conversations.contains(&message.conversation_id) {
                    continue;
                }
                let Some(at) = time_ns(&message.created_at).filter(|at| *at >= self.cutoff) else {
                    continue;
                };
                insert.execute(params![
                    message.conversation_id,
                    message.id,
                    at,
                    message.position.unwrap_or_default(),
                    self.archive.seal(b"message", &message)?
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    fn publish(self) -> Result<()> {
        let Self {
            db, path, archive, ..
        } = self;
        db.close().map_err(|(_, e)| e)?;
        fs::OpenOptions::new().write(true).open(&path)?.sync_all()?;
        let revoked = archive.0.revoked.lock().unwrap();
        if !*revoked {
            path.persist(&archive.0.path)?;
        }
        Ok(())
    }
}

async fn snapshot_page(api: &Api, before: Option<&str>) -> Result<OfflineSnapshot, ApiError> {
    #[derive(Serialize)]
    struct Query<'a> {
        #[serde(skip_serializing_if = "Option::is_none")]
        before: Option<&'a str>,
        limit: u16,
    }
    api.get_query(
        "/api/v1/offline/snapshot",
        &Query {
            before,
            limit: SNAPSHOT_PAGE_SIZE,
        },
    )
    .await
}

async fn blocking<T: Send + 'static>(
    run: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T> {
    tokio::task::spawn_blocking(run).await?
}

fn time_ns(value: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(value)
        .ok()?
        .timestamp_nanos_opt()
}

fn now_ns() -> i64 {
    chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
}

fn retention_start() -> i64 {
    now_ns()
        - RETENTION
            .num_nanoseconds()
            .expect("seven days fits in i64 nanoseconds")
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;
    use serde_json::json;

    use crate::Client;
    use crate::client::runtime;
    use crate::config::ConfigDir;
    use crate::events::Update;
    use crate::testing::{CANNED_COOKIE, CannedServer, Reply, SnapshotBuilder};

    fn message(id: &str, hours_ago: i64) -> Message {
        Message {
            id: id.to_owned(),
            conversation_id: "c".into(),
            text: format!("Private archive example {id}"),
            created_at: (chrono::Utc::now() - chrono::TimeDelta::hours(hours_ago)).to_rfc3339(),
            ..Message::default()
        }
    }

    #[test]
    fn complete_snapshot_restores_the_workspace_and_pages_while_offline() {
        let rt = runtime().unwrap();
        let server = CannedServer::start(&rt);
        let snapshot = SnapshotBuilder::new()
            .network("n", "Home")
            .channel("c", "general")
            .cursor(7)
            .build();
        let owner = snapshot.user.id.clone();
        let since = (chrono::Utc::now() - RETENTION).to_rfc3339();
        server.route_fn("GET", "/api/v1/offline/snapshot", move |request| {
            let page = if request.query_param("before").as_deref() == Some("second") {
                OfflineSnapshot {
                    cursor: 7,
                    items: vec![message("older", 2), message("expired", 180)],
                    since: since.clone(),
                    ..Default::default()
                }
            } else {
                OfflineSnapshot {
                    state: Some(snapshot.clone()),
                    cursor: 7,
                    items: vec![message("newer", 1)],
                    next_cursor: Some("second".into()),
                    since: since.clone(),
                }
            };
            Reply::json(200, serde_json::to_value(page).unwrap())
        });
        server.route("GET", "/api/v1/bootstrap", 503, json!({}));
        server.route("GET", "/api/v1/conversations/c/messages", 503, json!({}));
        let temp = tempfile::tempdir().unwrap();
        let config = ConfigDir::at(temp.path());
        let api = Api::new(server.address());
        api.set_cookie(Some(CANNED_COOKIE.into()));
        let api = api.with_offline_archive(&config);
        let archive = api.archive.clone().unwrap();
        rt.block_on(async {
            archive.download(&api, &owner).await.unwrap();
            let first = api.messages("c", None, 1).await.unwrap();
            assert_eq!(first.items[0].id, "newer");
            let older = api
                .messages("c", first.next_cursor.as_deref(), 1)
                .await
                .unwrap();
            assert_eq!(older.items[0].id, "older");
            assert!(older.next_cursor.is_none());

            let (client, mut updates) = Client::start(rt.clone(), api.clone());
            tokio::time::timeout(Duration::from_secs(2), async {
                while let Some(update) = updates.next().await {
                    if matches!(update, Update::Changed(changes) if changes.reset) {
                        assert_eq!(client.read().user.id, owner);
                        assert_eq!(client.read().cursor, 7);
                        return;
                    }
                }
                panic!("the client stopped before restoring the workspace");
            })
            .await
            .unwrap();
            // Authentication and permission failures always win over saved copies.
            server.route("GET", "/api/v1/conversations/c/messages", 401, json!({}));
            assert!(matches!(
                api.messages("c", None, 1).await,
                Err(ApiError::Unauthorized)
            ));
            archive.forget().await;
            assert!(archive.snapshot().await.is_none());
        });
    }

    #[test]
    fn failed_or_revoked_refresh_leaves_no_partial_snapshot() {
        let rt = runtime().unwrap();
        let temp = tempfile::tempdir().unwrap();
        let origin = "https://example.test";
        let archive = Archive::new(temp.path(), origin, CANNED_COOKIE);
        let snapshot = SnapshotBuilder::new()
            .network("n", "Home")
            .channel("c", "general")
            .cursor(1)
            .build();
        let mut first = Staging::begin(archive.clone(), &snapshot, retention_start()).unwrap();
        first.append(vec![message("initial", 1)]).unwrap();
        first.publish().unwrap();

        let later = Snapshot {
            cursor: 2,
            ..snapshot
        };
        let interrupted = Staging::begin(archive.clone(), &later, retention_start()).unwrap();
        drop(interrupted);
        let revoked = Staging::begin(archive.clone(), &later, retention_start()).unwrap();
        rt.block_on(async {
            assert_eq!(archive.snapshot().await.unwrap().cursor, 1);
            let other_session = Archive::new(temp.path(), origin, "ircord_session=other");
            assert!(other_session.snapshot().await.is_none());
            let other_origin = Archive::new(temp.path(), "https://other.test", CANNED_COOKIE);
            assert!(other_origin.snapshot().await.is_none());
            let bytes = fs::read(&archive.0.path).unwrap();
            assert!(!String::from_utf8_lossy(&bytes).contains("Private archive example"));
            archive.forget().await;
        });
        revoked.publish().unwrap();
        assert!(!archive.0.path.exists());
    }
}

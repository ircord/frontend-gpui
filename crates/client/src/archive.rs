//! The account's retained message archive. Download into a private staging
//! file so pagination, a disconnect or closing the app cannot publish a
//! partial snapshot. HTTP and SQLite work stay off the UI thread.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use ircord_contract::event::ServerEvent;
use ircord_contract::{Message, OfflineSnapshot, Snapshot};
use ring::{aead, digest, hmac, rand};
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::api::{Api, ApiError};

mod history;
mod media;
mod replay;
mod search;
pub(crate) use replay::{EVENT_BUFFER, Signal};
use replay::{Projection, SCHEMA_VERSION, read_replay};
pub(crate) use search::SEARCH_CURSOR;

const RETENTION: chrono::TimeDelta = chrono::TimeDelta::days(7);
const SNAPSHOT_PAGE_SIZE: u16 = 500;
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(15 * 60);
pub(crate) const CURSOR_PREFIX: &str = "offline:";

#[derive(Clone)]
pub(crate) struct Archive(Arc<Cache>);

struct Cache {
    path: PathBuf,
    key: aead::LessSafeKey,
    media_key: hmac::Key,
    media_changed: tokio::sync::Notify,
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
        let media = hmac::sign(
            &cookie_key,
            format!("ircord-offline-media-v1:{origin}").as_bytes(),
        );
        Self(Arc::new(Cache {
            path: config.join("offline").join(format!("{file}.sqlite3")),
            key: aead::LessSafeKey::new(key),
            media_key: hmac::Key::new(hmac::HMAC_SHA256, media.as_ref()),
            media_changed: tokio::sync::Notify::new(),
            revoked: Mutex::new(false),
        }))
    }

    async fn download(&self, api: &Api, owner: &str) -> Result<bool> {
        tokio::time::timeout(DOWNLOAD_TIMEOUT, self.download_pages(api, owner))
            .await
            .context("the offline snapshot download timed out")?
    }

    async fn download_pages(&self, api: &Api, owner: &str) -> Result<bool> {
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
        let base = state.cursor;
        let mut latest_page = base;
        let cache = self.clone();
        let cutoff = time_ns(&page.since).context("the offline snapshot has no retention date")?;
        let mut stage = blocking(move || Staging::begin(cache, state, cutoff)).await?;
        let mut seen = HashSet::new();
        loop {
            if page.cursor < latest_page || time_ns(&page.since) != Some(cutoff) {
                bail!("the offline snapshot changed while downloading");
            }
            latest_page = page.cursor;
            let next = page.next_cursor.take();
            stage = blocking(move || {
                stage.append(page.items, page.cursor)?;
                Ok(stage)
            })
            .await?;
            let Some(next) = next.filter(|value| !value.is_empty()) else {
                break;
            };
            if !seen.insert(next.clone()) {
                bail!("the offline snapshot repeated a page");
            }
            page = snapshot_page(api, Some(&next)).await?;
        }
        let Some(events) = read_replay(api, base).await? else {
            bail!("the server could not replay changes made during the download");
        };
        blocking(move || {
            let changed = stage.finish(&events, latest_page)?;
            stage.publish()?;
            Ok(changed)
        })
        .await
    }

    pub(crate) async fn snapshot(&self) -> Option<Snapshot> {
        let cache = self.clone();
        blocking(move || cache.read(|_, projection| Ok(projection.snapshot)))
            .await
            .ok()
            .flatten()
    }

    pub(crate) async fn forget(&self) {
        let cache = self.clone();
        if let Err(error) = blocking(move || {
            let mut revoked = cache.0.revoked.lock().unwrap();
            *revoked = true;
            let mut result = Ok(());
            for path in [&cache.0.path, &cache.media_path()] {
                if let Err(error) = fs::remove_file(path)
                    && error.kind() != std::io::ErrorKind::NotFound
                {
                    result = Err(error.into());
                }
            }
            result
        })
        .await
        {
            log::warn!("forget offline archive: {error}");
        }
    }

    fn read<T>(
        &self,
        read: impl FnOnce(&Connection, Projection) -> Result<T>,
    ) -> Result<Option<T>> {
        let revoked = self.0.revoked.lock().unwrap();
        if *revoked || !self.0.path.is_file() {
            return Ok(None);
        }
        let db = Connection::open_with_flags(&self.0.path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let Some(projection) = self.load(&db)? else {
            return Ok(None);
        };
        read(&db, projection).map(Some)
    }

    fn load(&self, db: &Connection) -> Result<Option<Projection>> {
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
        self.open(b"workspace", payload).map(Some)
    }

    fn seal(&self, kind: &[u8], value: &impl Serialize) -> Result<Vec<u8>> {
        self.seal_bytes(kind, serde_json::to_vec(value)?)
    }

    fn seal_bytes(&self, kind: &[u8], mut bytes: Vec<u8>) -> Result<Vec<u8>> {
        let mut nonce = [0; aead::NONCE_LEN];
        use rand::SecureRandom;
        rand::SystemRandom::new()
            .fill(&mut nonce)
            .map_err(|_| anyhow::anyhow!("could not obtain randomness for the offline archive"))?;
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

    fn open<T: DeserializeOwned>(&self, kind: &[u8], sealed: Vec<u8>) -> Result<T> {
        Ok(serde_json::from_slice(&self.open_bytes(kind, sealed)?)?)
    }

    fn open_bytes(&self, kind: &[u8], mut sealed: Vec<u8>) -> Result<Vec<u8>> {
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
        let size = bytes.len();
        sealed.truncate(size);
        Ok(sealed)
    }
}

struct Staging {
    db: Connection,
    path: tempfile::TempPath,
    archive: Archive,
    cutoff: i64,
    projection: Projection,
}

impl Staging {
    fn begin(archive: Archive, snapshot: Snapshot, cutoff: i64) -> Result<Self> {
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
                 at INTEGER NOT NULL, position INTEGER NOT NULL, payload BLOB NOT NULL, seq INTEGER NOT NULL,
                 PRIMARY KEY(conversation,id));
             CREATE INDEX messages_history ON messages(conversation,at,position,id);
             CREATE INDEX messages_retention ON messages(at);",
        )?;
        db.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        media::create_refs(&db)?;
        Ok(Self {
            db,
            path,
            archive,
            cutoff,
            projection: Projection::new(snapshot),
        })
    }

    fn append(&mut self, items: Vec<Message>, seq: i64) -> Result<()> {
        let tx = self.db.transaction()?;
        for message in items {
            replay::put_message(&tx, &self.archive, &message, seq, self.cutoff)?;
        }
        tx.commit()?;
        Ok(())
    }

    fn finish(&mut self, events: &[Arc<ServerEvent>], latest_page: i64) -> Result<bool> {
        let tx = self.db.transaction()?;
        let changed = self
            .projection
            .apply(&tx, &self.archive, events)?
            .context("the offline snapshot replay contained a gap")?;
        if self.projection.snapshot.cursor < latest_page {
            bail!("the offline snapshot replay was incomplete");
        }
        self.projection.retain_conversations(&tx)?;
        self.projection.save(&tx, &self.archive)?;
        tx.commit()?;
        Ok(changed)
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
            archive.0.media_changed.notify_one();
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
    use crate::testing::{CANNED_COOKIE, CannedServer, Reply, SnapshotBuilder, frame, wait_live};

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
                    cursor: 9,
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
        let mut out_of_date = message("older", 2);
        out_of_date.text = "Old edit from replay".into();
        server.route(
            "GET",
            "/api/v1/events/replay",
            200,
            json!({
                "valid": true,
                "items": [
                    {"seq": 8, "type": "message.updated", "data": out_of_date},
                    {"seq": 9, "type": "future.event", "data": {}}
                ]
            }),
        );
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
            assert_eq!(older.items[0].text, "Private archive example older");
            assert!(older.next_cursor.is_none());

            let (client, mut updates) = Client::start(rt.clone(), api.clone());
            tokio::time::timeout(Duration::from_secs(2), async {
                while let Some(update) = updates.next().await {
                    if matches!(update, Update::Changed(changes) if changes.reset) {
                        assert_eq!(client.read().user.id, owner);
                        assert_eq!(client.read().cursor, 9);
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
        let mut first =
            Staging::begin(archive.clone(), snapshot.clone(), retention_start()).unwrap();
        first.append(vec![message("initial", 1)], 1).unwrap();
        first.finish(&[], 1).unwrap();
        first.publish().unwrap();

        let later = Snapshot {
            cursor: 2,
            ..snapshot
        };
        let mut interrupted =
            Staging::begin(archive.clone(), later.clone(), retention_start()).unwrap();
        let gap = ServerEvent::decode(&frame(4, "message.created", &message("gap", 1))).unwrap();
        assert!(interrupted.finish(&[Arc::new(gap)], 4).is_err());
        drop(interrupted);
        let revoked = Staging::begin(archive.clone(), later, retention_start()).unwrap();
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

    #[test]
    fn live_events_update_saved_messages_and_remove_revoked_conversations() {
        let rt = runtime().unwrap();
        let server = CannedServer::start(&rt);
        let snapshot = SnapshotBuilder::new()
            .channel("c", "general")
            .cursor(7)
            .build();
        server.snapshot(&snapshot);
        let original = message("initial", 1);
        server.route(
            "GET",
            "/api/v1/offline/snapshot",
            200,
            json!({
                "state": snapshot,
                "cursor": 7,
                "since": (chrono::Utc::now() - RETENTION).to_rfc3339(),
                "items": [original]
            }),
        );
        server.route(
            "GET",
            "/api/v1/events/replay",
            200,
            json!({"valid": true, "items": []}),
        );
        let temp = tempfile::tempdir().unwrap();
        let api = server
            .api()
            .with_offline_archive(&ConfigDir::at(temp.path()));
        let archive = api.archive.clone().unwrap();
        let (client, _updates) = Client::start(rt.clone(), api);
        wait_live(&client);
        let mut deleted = original;
        deleted.text = "Message deleted".into();
        deleted.deleted = Some(true);
        let live = message("live", 0);
        rt.block_on(async {
            wait_for_cursor(&archive, 7).await;
            server.push(frame(8, "message.created", &live));
            server.push(frame(9, "message.updated", &deleted));
            wait_for_cursor(&archive, 9).await;
            let saved = archive.history("c", None, 10).await.unwrap();
            assert_eq!(saved.items, vec![deleted, live]);
            server.push(frame(10, "conversation.removed", &json!({"id": "c"})));
            wait_for_cursor(&archive, 10).await;
            assert!(archive.history("c", None, 10).await.is_none());
        });
        let remaining = archive
            .read(|db, _| {
                Ok(db.query_row("SELECT COUNT(*) FROM messages", [], |row| {
                    row.get::<_, i64>(0)
                })?)
            })
            .unwrap();
        assert_eq!(remaining, Some(0));
    }

    #[test]
    fn reconnect_replays_an_existing_archive_without_downloading_it_again() {
        let rt = runtime().unwrap();
        let server = CannedServer::start(&rt);
        let old = SnapshotBuilder::new()
            .channel("c", "general")
            .cursor(7)
            .build();
        let current = Snapshot {
            cursor: 8,
            ..old.clone()
        };
        server.snapshot(&current);
        let missed = message("missed", 0);
        server.route(
            "GET",
            "/api/v1/events/replay",
            200,
            json!({
                "valid": true,
                "items": [{"seq": 8, "type": "message.created", "data": missed}]
            }),
        );
        let temp = tempfile::tempdir().unwrap();
        let api = server
            .api()
            .with_offline_archive(&ConfigDir::at(temp.path()));
        let archive = api.archive.clone().unwrap();
        let mut stage = Staging::begin(archive.clone(), old, retention_start()).unwrap();
        stage.finish(&[], 7).unwrap();
        stage.publish().unwrap();
        let (client, _updates) = Client::start(rt.clone(), api);
        wait_live(&client);
        rt.block_on(async {
            wait_for_cursor(&archive, 8).await;
            assert_eq!(
                archive.history("c", None, 10).await.unwrap().items,
                vec![missed]
            );
        });
        let request = server.requests_to("GET", "/api/v1/events/replay");
        assert_eq!(request[0].query_param("after").as_deref(), Some("7"));
        assert!(
            server
                .requests_to("GET", "/api/v1/offline/snapshot")
                .is_empty()
        );
    }

    async fn wait_for_cursor(archive: &Archive, cursor: i64) {
        tokio::time::timeout(Duration::from_secs(5), async {
            while archive
                .snapshot()
                .await
                .is_none_or(|snapshot| snapshot.cursor < cursor)
            {
                tokio::time::sleep(Duration::from_millis(15)).await;
            }
        })
        .await
        .expect("the archive catches up to the expected cursor");
    }

    #[test]
    fn offline_search_pages_and_opens_matches_with_their_surrounding_messages() {
        use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
        use ircord_contract::{MemberRole, MessageKind};

        use crate::search_filter::{Author, Chip, Filters, Has, Query, Source};

        let rt = runtime().unwrap();
        let server = CannedServer::start(&rt);
        let mut snapshot = SnapshotBuilder::new()
            .channel("c", "general")
            .channel("elsewhere", "other")
            .member("bot", "helper")
            .member("alice", "Al-ice")
            .cursor(7)
            .build();
        snapshot
            .members
            .iter_mut()
            .find(|m| m.id == "bot")
            .unwrap()
            .role = MemberRole::Bot;
        let hit = |id, hours| Message {
            kind: MessageKind::Message,
            sender_id: "bot".into(),
            text: "Café au lait, @Al-ice. https://example.test/photo.webp".into(),
            ..message(id, hours)
        };
        let newest = hit("newest", 1);
        let pivot = hit("pivot", 4);
        let oldest = hit("oldest", 6);
        let wrong_phrase = Message {
            text: "Café chaud au lait, Al-ice https://example.test/photo.webp".into(),
            ..hit("words", 2)
        };
        let code_link = Message {
            text: "Café au lait, Al-ice\n```\nhttps://example.test/photo.webp\n```".into(),
            ..hit("code", 5)
        };
        let deleted = Message {
            deleted: Some(true),
            ..hit("deleted", 2)
        };
        let other = Message {
            conversation_id: "elsewhere".into(),
            ..hit("elsewhere", 3)
        };
        server.route("GET", "/api/v1/search", 503, json!({}));
        server.route("GET", "/api/v1/conversations/c/messages", 503, json!({}));
        let temp = tempfile::tempdir().unwrap();
        let api = server
            .api()
            .with_offline_archive(&ConfigDir::at(temp.path()));
        let archive = api.archive.clone().unwrap();
        let mut stage = Staging::begin(archive, snapshot, retention_start()).unwrap();
        stage
            .append(
                vec![
                    newest.clone(),
                    pivot.clone(),
                    oldest.clone(),
                    wrong_phrase,
                    code_link,
                    deleted,
                    other,
                ],
                7,
            )
            .unwrap();
        stage.finish(&[], 7).unwrap();
        stage.publish().unwrap();

        let filters = Filters::default()
            .scoped("c")
            .with(Chip::From("bot".into()))
            .with(Chip::Mentions("alice".into()))
            .with(Chip::Author(Author::Bot))
            .with(Chip::Has(Has::Image));
        let mut query = Query::new("cafe au la", filters, Source::Retained);
        query.limit = Some(1);
        rt.block_on(async {
            let first = api.search_with_archive(&query).await.unwrap();
            assert!(first.archived);
            assert_eq!(first.page.items[0].message, newest);
            let saved_cursor = first.page.next_cursor;

            // A server cursor already in the view can serve as a local boundary.
            query.cursor = Some(
                URL_SAFE_NO_PAD.encode(
                    serde_json::to_vec(&json!({
                        "v": 2, "a": 0, "i": URL_SAFE_NO_PAD.encode("newest"), "s": "server-scope"
                    }))
                    .unwrap(),
                ),
            );
            let continued = api.search_with_archive(&query).await.unwrap();
            assert!(continued.archived);
            assert_eq!(continued.page.items[0].message, pivot);

            // The offline cursor remains local even if HTTP comes back.
            server.route("GET", "/api/v1/search", 200, json!({"items": []}));
            query.cursor = saved_cursor;
            assert_eq!(
                api.search_with_archive(&query).await.unwrap().page.items[0].message,
                pivot
            );
            query.text = "different query".into();
            assert!(matches!(
                api.search_with_archive(&query).await,
                Err(ApiError::ArchivedSearchUnavailable)
            ));
            query.cursor = None;
            assert!(!api.search_with_archive(&query).await.unwrap().archived);
            server.route("GET", "/api/v1/search", 401, json!({}));
            assert!(matches!(
                api.search_with_archive(&query).await,
                Err(ApiError::Unauthorized)
            ));

            let jump = api.messages_around("c", "pivot", 1).await.unwrap();
            assert_eq!(jump.items, vec![pivot]);
            assert_eq!(jump.has_newer, Some(true));
            assert_eq!(jump.next_cursor.as_deref(), Some("offline:pivot"));
            let older = api
                .messages("c", jump.next_cursor.as_deref(), 20)
                .await
                .unwrap();
            assert_eq!(older.items.first(), Some(&oldest));
            assert!(older.next_cursor.is_none());
            assert!(api.messages_around("elsewhere", "pivot", 10).await.is_err());
        });
    }
}

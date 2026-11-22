//! User-kept attachments and the original messages they belong to.

use std::sync::atomic::Ordering;

use anyhow::{Context, Result, bail};
use ircord_contract::{Message, OfflineMessageRef, OfflinePinned};
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use serde::Serialize;

use super::replay::{SCHEMA_VERSION, put_message};
use super::{Archive, blocking, now_ns, retention_start};
use crate::api::{Api, ApiError, Method};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OfflinePin {
    pub available: bool,
    pub pinned: bool,
    pub saved: bool,
    pub failed: bool,
}

impl Archive {
    /// Open under the archive lock, upgrading existing copies in place.
    pub(super) fn open_database(&self) -> Result<Connection> {
        let mut db = Connection::open_with_flags(&self.0.path, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        let version: i64 = db.pragma_query_value(None, "user_version", |row| row.get(0))?;
        if version == SCHEMA_VERSION {
            return Ok(db);
        }
        if !(0..SCHEMA_VERSION).contains(&version) {
            bail!("unknown offline archive format");
        }
        let tx = db.transaction()?;
        if version == 0 {
            tx.execute_batch("ALTER TABLE messages ADD COLUMN seq INTEGER NOT NULL DEFAULT 0;")?;
        }
        tx.execute_batch("ALTER TABLE messages ADD COLUMN pinned INTEGER NOT NULL DEFAULT 0;")?;
        if version < 2 {
            super::media::create_refs(&tx)?;
            let mut query = tx.prepare("SELECT payload FROM messages")?;
            let mut rows = query.query([])?;
            while let Some(row) = rows.next()? {
                let message: Message = self.open(b"message", row.get(0)?)?;
                super::media::store_refs(&tx, self, &message, false)?;
            }
        }
        tx.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        tx.commit()?;
        Ok(db)
    }

    pub(super) async fn pinned_refs(&self) -> Result<(u64, Vec<OfflineMessageRef>)> {
        let archive = self.clone();
        blocking(move || {
            let revoked = archive.0.revoked.lock().unwrap();
            let revision = archive.0.pin_revision.load(Ordering::Relaxed);
            if *revoked || !archive.0.path.is_file() {
                return Ok((revision, Vec::new()));
            }
            let db = archive.open_database()?;
            let mut query = db.prepare("SELECT conversation,id FROM messages WHERE pinned=1")?;
            let refs = query
                .query_map([], |row| {
                    Ok(OfflineMessageRef {
                        conversation_id: row.get(0)?,
                        message_id: row.get(1)?,
                    })
                })?
                .collect::<rusqlite::Result<_>>()?;
            Ok((revision, refs))
        })
        .await
    }

    pub(crate) async fn pin_status(&self, conversation: &str, message: &str) -> OfflinePin {
        let archive = self.clone();
        let target = (conversation.to_owned(), message.to_owned());
        blocking(move || {
            archive.read(|db, projection| {
                if !projection
                    .snapshot
                    .conversations
                    .iter()
                    .any(|c| c.id == target.0)
                {
                    return Ok(OfflinePin::default());
                }
                archive.status(db, &target.0, &target.1)
            })
        })
        .await
        .ok()
        .flatten()
        .unwrap_or_default()
    }

    pub(crate) async fn set_pin(
        &self,
        conversation: &str,
        message: &str,
        value: bool,
        online: Option<(i64, Message)>,
    ) -> Result<OfflinePin> {
        let archive = self.clone();
        let target = (conversation.to_owned(), message.to_owned());
        blocking(move || {
            let revoked = archive.0.revoked.lock().unwrap();
            if *revoked || !archive.0.path.is_file() {
                bail!("the offline archive is unavailable");
            }
            let mut db = archive.open_database()?;
            let projection = archive
                .load(&db)?
                .context("the offline archive is unavailable")?;
            if !projection
                .snapshot
                .conversations
                .iter()
                .any(|c| c.id == target.0)
            {
                bail!("the conversation is unavailable offline");
            }
            let saved: Option<(i64, Vec<u8>)> = db
                .query_row(
                    "SELECT seq,payload FROM messages WHERE conversation=?1 AND id=?2",
                    params![target.0, target.1],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?;
            let on_device = online.is_none();
            let (seq, message) = match (online, saved) {
                (Some((seq, message)), saved)
                    if saved.as_ref().is_none_or(|(old, _)| *old <= seq) =>
                {
                    (seq, message)
                }
                (_, Some((seq, payload))) => (seq, archive.open(b"message", payload)?),
                _ => bail!("the message is unavailable offline"),
            };
            if message.deleted == Some(true)
                || message.conversation_id != target.0
                || message.id != target.1
                || (value && on_device && !archive.status(&db, &target.0, &target.1)?.saved)
            {
                bail!("the message files are unavailable offline");
            }
            archive.attach_files(&db)?;
            let tx = db.transaction()?;
            put_message(&tx, &archive, &message, seq, retention_start(), Some(value))?;
            if value {
                let count: i64 = tx.query_row(
                    "SELECT count(*) FROM media_refs WHERE conversation=?1 AND message=?2",
                    params![target.0, target.1],
                    |row| row.get(0),
                )?;
                if count == 0 {
                    bail!("the message has no files that can be kept offline");
                }
                tx.execute(
                    "UPDATE media.files SET retry=0 WHERE payload IS NULL AND key IN (
                    SELECT key FROM media_refs WHERE conversation=?1 AND message=?2)",
                    params![target.0, target.1],
                )?;
            }
            tx.commit()?;
            archive.0.pin_revision.fetch_add(1, Ordering::Relaxed);
            archive.0.media_changed.notify_one();
            archive.status_in_attached(&db, &target.0, &target.1)
        })
        .await
    }

    fn status(&self, db: &Connection, conversation: &str, message: &str) -> Result<OfflinePin> {
        if !self.media_path().is_file() {
            return Ok(OfflinePin {
                available: true,
                pinned: is_pinned(db, conversation, message)?,
                ..OfflinePin::default()
            });
        }
        self.attach_files(db)?;
        self.status_in_attached(db, conversation, message)
    }

    fn status_in_attached(
        &self,
        db: &Connection,
        conversation: &str,
        message: &str,
    ) -> Result<OfflinePin> {
        let (total, saved, failed): (i64, i64, i64) = db.query_row(
            "SELECT count(*), count(CASE WHEN f.size>0 THEN 1 END), count(CASE WHEN f.size=0 AND f.retry>?3 THEN 1 END)
             FROM media_refs r LEFT JOIN media.files f ON f.key=r.key
             WHERE r.conversation=?1 AND r.message=?2",
            params![conversation, message, now_ns()], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;
        Ok(OfflinePin {
            available: true,
            pinned: is_pinned(db, conversation, message)?,
            saved: total > 0 && saved == total,
            failed: failed > 0,
        })
    }
}

fn is_pinned(db: &Connection, conversation: &str, message: &str) -> rusqlite::Result<bool> {
    db.query_row(
        "SELECT EXISTS(SELECT 1 FROM messages WHERE conversation=?1 AND id=?2 AND pinned=1)",
        params![conversation, message],
        |row| row.get(0),
    )
}

pub(super) async fn reconcile(
    api: &Api,
    refs: &[OfflineMessageRef],
) -> Result<OfflinePinned, ApiError> {
    #[derive(Serialize)]
    struct Request<'a> {
        items: &'a [OfflineMessageRef],
    }
    api.call(
        Method::POST,
        "/api/v1/offline/pinned",
        &Request { items: refs },
    )
    .await
}

impl Api {
    pub async fn offline_pin_status(&self, conversation: &str, message: &str) -> OfflinePin {
        match &self.archive {
            Some(archive) => archive.pin_status(conversation, message).await,
            None => OfflinePin::default(),
        }
    }

    pub async fn set_offline_pin(
        &self,
        conversation: &str,
        message: &str,
        value: bool,
    ) -> Result<OfflinePin, ApiError> {
        let archive = self
            .archive
            .as_ref()
            .ok_or(ApiError::OfflineFilesUnavailable)?;
        let online = if value {
            let target = OfflineMessageRef {
                conversation_id: conversation.to_owned(),
                message_id: message.to_owned(),
            };
            match reconcile(self, &[target]).await {
                Ok(page) => Some((
                    page.cursor,
                    page.items
                        .into_iter()
                        .next()
                        .ok_or(ApiError::OfflineFilesUnavailable)?,
                )),
                Err(error) if error.unreachable() => None,
                Err(error) => return Err(error),
            }
        } else {
            None
        };
        archive
            .set_pin(conversation, message, value, online)
            .await
            .map_err(|_| ApiError::OfflineFilesUnavailable)
    }
}

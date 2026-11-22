//! Use the archived conversation's order for exclusive pages and jump windows.

use anyhow::Result;
use ircord_contract::{Message, MessagePage};
use rusqlite::{Connection, OptionalExtension, params};

use super::{Archive, CURSOR_PREFIX, blocking, retention_start};

type Boundary<'a> = (i64, i64, &'a str);

impl Archive {
    pub(crate) async fn history(
        &self,
        conversation: &str,
        before: Option<&str>,
        limit: u32,
    ) -> Option<MessagePage> {
        self.history_page(conversation, before, false, limit).await
    }

    pub(crate) async fn history_around(
        &self,
        conversation: &str,
        message: &str,
        limit: u32,
    ) -> Option<MessagePage> {
        self.history_page(conversation, Some(message), true, limit)
            .await
    }

    async fn history_page(
        &self,
        conversation: &str,
        id: Option<&str>,
        around: bool,
        limit: u32,
    ) -> Option<MessagePage> {
        let cache = self.clone();
        let conversation = conversation.to_owned();
        let id = id.map(str::to_owned);
        blocking(move || {
            cache.read(|db, projection| {
                if !projection.snapshot.conversations.iter().any(|c| c.id == conversation) {
                    return Ok(None);
                }
                let cutoff = retention_start();
                let anchor = match id.as_deref() {
                    Some(id) => {
                        let Some(anchor) = db.query_row(
                            "SELECT at, position, payload FROM messages WHERE conversation=?1 AND id=?2 AND at>=?3",
                            params![conversation, id, cutoff],
                            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?, row.get::<_, Vec<u8>>(2)?)),
                        ).optional()? else { return Ok(None) };
                        Some(anchor)
                    }
                    None => None,
                };
                let boundary = match (&anchor, id.as_deref()) {
                    (Some((at, position, _)), Some(id)) => (*at, *position, id),
                    _ => (i64::MAX, i64::MAX, ""),
                };
                let limit = limit.max(1) as usize;
                let before = if around { (limit - 1) / 2 } else { limit };
                let mut rows = history_rows(db, &conversation, cutoff, boundary, before + 1, false)?;
                let has_older = rows.len() > before;
                rows.truncate(before);
                rows.reverse();
                let mut has_newer = false;
                if let Some((_, _, payload)) = anchor.filter(|_| around) {
                    rows.push(payload);
                    let room = limit - rows.len();
                    let mut newer = history_rows(db, &conversation, cutoff, boundary, room + 1, true)?;
                    has_newer = newer.len() > room;
                    newer.truncate(room);
                    rows.extend(newer);
                }
                let items: Vec<Message> = rows.into_iter()
                    .map(|payload| cache.open(b"message", payload))
                    .collect::<Result<_>>()?;
                let next_cursor = items.first().filter(|_| has_older)
                    .map(|message| format!("{CURSOR_PREFIX}{}", message.id));
                Ok(Some(MessagePage {
                    items,
                    next_cursor,
                    has_newer: Some(has_newer),
                    cursor: Some(projection.snapshot.cursor),
                    ..Default::default()
                }))
            }).map(Option::flatten)
        }).await.ok().flatten()
    }
}

fn history_rows(
    db: &Connection,
    conversation: &str,
    cutoff: i64,
    boundary: Boundary<'_>,
    limit: usize,
    newer: bool,
) -> Result<Vec<Vec<u8>>> {
    let sql = if newer {
        "SELECT payload FROM messages WHERE conversation=?1 AND at>=?2 AND (at,position,id)>(?3,?4,?5)
         ORDER BY at,position,id LIMIT ?6"
    } else {
        "SELECT payload FROM messages WHERE conversation=?1 AND at>=?2 AND (at,position,id)<(?3,?4,?5)
         ORDER BY at DESC,position DESC,id DESC LIMIT ?6"
    };
    let mut query = db.prepare_cached(sql)?;
    let rows = query.query_map(
        params![
            conversation,
            cutoff,
            boundary.0,
            boundary.1,
            boundary.2,
            i64::try_from(limit)?
        ],
        |row| row.get(0),
    )?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

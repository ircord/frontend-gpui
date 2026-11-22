//! A single writer keeps the downloaded archive in step with the account
//! stream. If the bounded socket feed drops events, recover with the same
//! authenticated replay endpoint the web client uses.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Result, bail};
use ircord_contract::event::{Durable, ServerEvent};
use ircord_contract::{Message, Snapshot};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast::{self, error::RecvError, error::TryRecvError};
use tokio::time::{Instant, sleep, sleep_until};

use super::{Archive, blocking, now_ns, retention_start, time_ns};
use crate::api::Api;
use crate::events::now_ms;
use crate::store::{Workspace, parties};

pub(crate) const SCHEMA_VERSION: i64 = 3;
pub(crate) const EVENT_BUFFER: usize = 512;
const BATCH_SIZE: usize = 100;
const BATCH_WAIT: Duration = Duration::from_millis(50);
const REPLAY_INTERVAL: Duration = Duration::from_secs(3);
const RETRY_DELAY: Duration = Duration::from_secs(60);
const HISTORY_REFRESH_DELAY: Duration = Duration::from_secs(120);

#[derive(Clone)]
pub(crate) enum Signal {
    Live(String),
    Offline,
    Event(Arc<ServerEvent>),
}

impl Archive {
    pub(crate) async fn watch(self, api: Api, mut signals: broadcast::Receiver<Signal>) {
        let mut state = SyncState::new();
        loop {
            tokio::select! {
                _ = sleep_until(state.next_sync), if state.live && state.pending => {
                    match state.owner.as_deref() {
                        Some(owner) => match self.sync(&api, owner, state.full, state.next_snapshot).await {
                            Ok(result) => state.synced(result),
                            Err(error) => {
                                log::debug!("refresh offline archive: {error}");
                                state.next_sync = Instant::now() + RETRY_DELAY;
                            }
                        },
                        None => state.pending = false,
                    }
                }
                signal = signals.recv() => match signal {
                    Ok(Signal::Event(first)) => {
                        sleep(BATCH_WAIT).await;
                        let mut batch = vec![first];
                        let mut closed = false;
                        while batch.len() < BATCH_SIZE {
                            match signals.try_recv() {
                                Ok(Signal::Event(event)) => batch.push(event),
                                Ok(signal) => state.notice(signal),
                                Err(TryRecvError::Lagged(_)) => { state.pending = true; break; }
                                Err(TryRecvError::Closed) => { closed = true; break; }
                                Err(TryRecvError::Empty) => break,
                            }
                        }
                        if let Some(owner) = &state.owner {
                            match self.apply(owner, batch).await {
                                Ok(Some(history_changed)) => {
                                    state.pending |= history_changed;
                                    state.full |= history_changed;
                                }
                                Ok(None) => state.pending = true,
                                Err(error) => {
                                    log::debug!("apply offline archive events: {error}");
                                    state.pending = true;
                                    state.full = true;
                                }
                            }
                        }
                        if closed { return; }
                    }
                    Ok(signal) => state.notice(signal),
                    Err(RecvError::Lagged(_)) => state.pending = true,
                    Err(RecvError::Closed) => return,
                },
            }
        }
    }

    async fn sync(
        &self,
        api: &Api,
        owner: &str,
        full: bool,
        next_snapshot: Instant,
    ) -> Result<SyncResult> {
        if !full {
            let archive = self.clone();
            let cursor =
                blocking(move || archive.read(|_, projection| Ok(projection.snapshot.cursor)))
                    .await;
            if let Ok(Some(cursor)) = cursor
                && let Some(events) = read_replay(api, cursor).await?
                && let Some(false) = self.apply(owner, events).await?
            {
                return Ok(SyncResult::Replayed);
            }
        }
        if Instant::now() < next_snapshot {
            return Ok(SyncResult::WaitingForSnapshot);
        }
        self.download(api, owner).await.map(SyncResult::Downloaded)
    }

    async fn apply(&self, owner: &str, events: Vec<Arc<ServerEvent>>) -> Result<Option<bool>> {
        let archive = self.clone();
        let owner = owner.to_owned();
        blocking(move || {
            let revoked = archive.0.revoked.lock().unwrap();
            if *revoked || !archive.0.path.is_file() {
                return Ok(None);
            }
            let mut db = archive.open_database()?;
            db.pragma_update(None, "secure_delete", true)?;
            let tx = db.transaction()?;
            let Some(mut projection) = archive.load(&tx)? else {
                return Ok(None);
            };
            if projection.snapshot.user.id != owner {
                return Ok(None);
            }
            let previous = projection.snapshot.cursor;
            let Some(history_changed) = projection.apply(&tx, &archive, &events)? else {
                return Ok(None);
            };
            tx.execute(
                "DELETE FROM messages WHERE at<?1 AND pinned=0",
                [retention_start()],
            )?;
            if projection.snapshot.cursor != previous {
                projection.save(&tx, &archive)?;
            } else {
                tx.execute("UPDATE workspace SET downloaded=?1", [now_ns()])?;
            }
            tx.commit()?;
            archive.0.media_changed.notify_one();
            Ok(Some(history_changed))
        })
        .await
    }
}

enum SyncResult {
    Replayed,
    Downloaded(bool),
    WaitingForSnapshot,
}

struct SyncState {
    owner: Option<String>,
    live: bool,
    pending: bool,
    full: bool,
    next_sync: Instant,
    next_snapshot: Instant,
}

impl SyncState {
    fn new() -> Self {
        Self {
            owner: None,
            live: false,
            pending: false,
            full: false,
            next_sync: Instant::now(),
            next_snapshot: Instant::now(),
        }
    }

    fn notice(&mut self, signal: Signal) {
        match signal {
            Signal::Live(owner) => {
                self.owner = Some(owner);
                self.live = true;
                self.pending = true;
            }
            Signal::Offline => self.live = false,
            Signal::Event(_) => unreachable!("the event belongs in a batch"),
        }
    }

    fn synced(&mut self, result: SyncResult) {
        let now = Instant::now();
        if matches!(result, SyncResult::Downloaded(_)) {
            self.next_snapshot = now + HISTORY_REFRESH_DELAY;
        }
        self.full = matches!(
            result,
            SyncResult::WaitingForSnapshot | SyncResult::Downloaded(true)
        );
        self.pending = self.full;
        self.next_sync = if self.full {
            self.next_snapshot
        } else {
            now + REPLAY_INTERVAL
        };
    }
}

/// The original workspace fields stay readable from older archive files.
#[derive(Serialize, Deserialize)]
pub(super) struct Projection {
    #[serde(flatten)]
    pub snapshot: Snapshot,
    #[serde(default)]
    party_fences: parties::State,
}

impl Projection {
    pub(super) fn new(snapshot: Snapshot) -> Self {
        let mut workspace = Workspace::from_snapshot(snapshot.clone());
        for party in snapshot
            .conversations
            .iter()
            .filter_map(|c| c.party.as_ref())
        {
            parties::put_party(&mut workspace, party);
        }
        Self {
            snapshot,
            party_fences: workspace.parties,
        }
    }

    /// Apply one contiguous prefix of new events or ask the caller to roll
    /// back and recover. The normal workspace reducer also owns party fences.
    pub(super) fn apply(
        &mut self,
        db: &Connection,
        archive: &Archive,
        events: &[Arc<ServerEvent>],
    ) -> Result<Option<bool>> {
        let mut workspace = Workspace::from_snapshot(self.snapshot.clone());
        workspace.parties = self.party_fences.clone();
        let cutoff = retention_start();
        let mut history_changed = false;
        for event in events {
            let Some(seq) = event.seq().filter(|seq| *seq > workspace.cursor) else {
                continue;
            };
            let changes = workspace.apply(event, now_ms());
            if changes.resync {
                return Ok(None);
            }
            for id in changes.timelines {
                if workspace.conversation(&id).is_none() {
                    db.execute(
                        "DELETE FROM messages WHERE conversation=?1 AND seq<=?2",
                        params![id, seq],
                    )?;
                }
            }
            if let ServerEvent::Durable { event, .. } = event.as_ref() {
                match event {
                    Durable::MessageCreated(message) | Durable::MessageUpdated(message)
                        if workspace.conversation(&message.conversation_id).is_some() =>
                    {
                        put_message(db, archive, message, seq, cutoff, None)?;
                    }
                    Durable::HistorySynced(_) => history_changed = true,
                    _ => {}
                }
            }
        }
        self.snapshot = Snapshot {
            version: self.snapshot.version,
            capabilities: workspace.capabilities,
            conversations: workspace.conversations,
            cursor: workspace.cursor,
            demo: workspace.demo,
            invitations: workspace.invitations,
            members: workspace.members.into_values().collect(),
            networks: workspace.networks,
            user: workspace.user,
        };
        self.party_fences = workspace.parties;
        Ok(Some(history_changed))
    }

    pub(super) fn retain_conversations(&self, db: &Connection) -> Result<()> {
        let mut query = db.prepare("SELECT DISTINCT conversation FROM messages")?;
        let seen = query
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let known: std::collections::HashSet<_> = self
            .snapshot
            .conversations
            .iter()
            .map(|c| c.id.as_str())
            .collect();
        for id in seen {
            if !known.contains(id.as_str()) {
                db.execute("DELETE FROM messages WHERE conversation=?1", [id])?;
            }
        }
        Ok(())
    }

    pub(super) fn save(&self, db: &Connection, archive: &Archive) -> Result<()> {
        db.execute("DELETE FROM workspace", [])?;
        db.execute(
            "INSERT INTO workspace VALUES (?1,?2)",
            params![now_ns(), archive.seal(b"workspace", self)?],
        )?;
        Ok(())
    }
}

pub(super) fn put_message(
    db: &Connection,
    archive: &Archive,
    message: &Message,
    seq: i64,
    cutoff: i64,
    pin: Option<bool>,
) -> Result<()> {
    if message.id.is_empty() || message.conversation_id.is_empty() {
        return Ok(());
    }
    let previous: bool = db
        .prepare_cached(
            "SELECT EXISTS(SELECT 1 FROM messages WHERE conversation=?1 AND id=?2 AND pinned=1)",
        )?
        .query_row(params![message.conversation_id, message.id], |row| {
            row.get(0)
        })?;
    let pinned = pin.unwrap_or(previous) && message.deleted != Some(true);
    let Some(at) = time_ns(&message.created_at) else {
        return Ok(());
    };
    if at < cutoff && !pinned {
        if previous {
            db.execute(
                "DELETE FROM messages WHERE conversation=?1 AND id=?2 AND seq<=?3",
                params![message.conversation_id, message.id, seq],
            )?;
        }
        return Ok(());
    }
    let changed = db
        .prepare_cached(
            "INSERT INTO messages(conversation,id,at,position,payload,seq,pinned) VALUES (?1,?2,?3,?4,?5,?6,?7)
         ON CONFLICT(conversation,id) DO UPDATE SET at=excluded.at,position=excluded.position,
            payload=excluded.payload,seq=excluded.seq,pinned=excluded.pinned WHERE excluded.seq>=messages.seq",
        )?
        .execute(params![
            message.conversation_id,
            message.id,
            at,
            message.position.unwrap_or_default(),
            archive.seal(b"message", message)?,
            seq,
            pinned,
        ])?;
    if changed != 0 {
        super::media::store_refs(db, archive, message, pinned)?;
    }
    Ok(())
}

pub(super) async fn read_replay(api: &Api, after: i64) -> Result<Option<Vec<Arc<ServerEvent>>>> {
    #[derive(Deserialize)]
    struct Response {
        valid: bool,
        items: Vec<serde_json::Value>,
    }
    let response: Response = api
        .get_query("/api/v1/events/replay", &[("after", after)])
        .await?;
    if !response.valid {
        return Ok(None);
    }
    let mut events = Vec::with_capacity(response.items.len());
    for item in response.items {
        let event = ServerEvent::decode_value(item)?;
        if event.seq().is_none() {
            bail!("the archive replay contained an unsequenced event");
        }
        events.push(Arc::new(event));
    }
    Ok(Some(events))
}

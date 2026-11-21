//! The event client: one task owns the socket.
//!
//! 1. Bootstrap if there is no workspace yet (or after a resync).
//! 2. Open `GET /api/v1/events?sync=1`, wait for `session.ready`.
//! 3. Send `session.resume` with the stored cursor and `sync: true`.
//! 4. Apply the replay; `session.synced` means live.
//! 5. On `session.resync` or a gap in sequence numbers, bootstrap again.
//! 6. On any disconnect, wait and reconnect with the web client's back-off
//!    (`realtime-recovery.ts`): `min(15 s, 0.8 s × 2^min(n, 5))` plus jitter.

use std::future::Future;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use futures::channel::mpsc::UnboundedSender;
use ircord_contract::event::{ServerEvent, resume_frame};
use ircord_contract::{Alert, Snapshot};
use tokio::sync::{Notify, mpsc};

use crate::api::ApiError;
use crate::store::{Changes, Connection, Workspace};

/// What the client tells the app.
#[derive(Debug, Clone, PartialEq)]
pub enum Update {
    Changed(Changes),
    Connection(Connection),
    /// The server routed a notification to this device.
    Alert(Alert),
    /// The session ended or was refused; the app shows the sign-in screen.
    SignedOut,
}

pub trait Transport: Send + Sync + 'static {
    type Conn: Conn;
    fn bootstrap(&self) -> impl Future<Output = Result<Snapshot, ApiError>> + Send;
    fn connect(&self) -> impl Future<Output = Result<Self::Conn, ApiError>> + Send;
}

pub trait Conn: Send + 'static {
    /// The next text frame; `None` when the socket closed.
    fn recv(&mut self) -> impl Future<Output = Option<Result<String, String>>> + Send;
    fn send(&mut self, text: String) -> impl Future<Output = Result<(), String>> + Send;
}

#[derive(Debug, Clone)]
pub struct Timing {
    pub ready: Duration,
    pub backoff_base: Duration,
    pub backoff_max: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Timing {
            ready: Duration::from_secs(10),
            backoff_base: Duration::from_millis(800),
            backoff_max: Duration::from_secs(15),
        }
    }
}

impl Timing {
    pub fn backoff(&self, attempts: u32) -> Duration {
        let exp = self.backoff_base * 2u32.pow(attempts.min(5));
        let jitter = Duration::from_millis(rand::random_range(0..250));
        exp.min(self.backoff_max) + jitter.min(self.backoff_base)
    }
}

/// State the event task shares with the rest of the client.
pub struct Shared {
    pub store: Arc<RwLock<Workspace>>,
    pub updates: UnboundedSender<Update>,
    /// The connection ID from the latest `notification.routing`, for activity reports.
    pub routing: Mutex<Option<String>>,
    pub routing_changed: Notify,
    pub tab: String,
    pub incarnation: String,
}

impl Shared {
    fn emit(&self, update: Update) {
        let _ = self.updates.unbounded_send(update);
    }

    fn status(&self, connection: Connection) {
        let mut store = self.store.write().unwrap();
        if store.connection.as_ref() != Some(&connection) {
            store.connection = Some(connection.clone());
            drop(store);
            self.emit(Update::Connection(connection));
        }
    }
}

pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

enum End {
    /// The socket dropped or failed: back off and reconnect.
    Lost(String),
    /// The server asked for, or the client detected, a resync.
    Resync,
    /// The session is gone.
    SignedOut,
    /// The client was dropped.
    Stopped,
}

/// Run until the session ends or the client is dropped. `outgoing` carries
/// frames to send (typing).
pub async fn run<T: Transport>(
    transport: T,
    shared: Arc<Shared>,
    mut outgoing: mpsc::UnboundedReceiver<String>,
    timing: Timing,
) {
    let mut need_bootstrap = true;
    let mut attempts: u32 = 0;
    loop {
        let end = session(&transport, &shared, &mut outgoing, &timing, &mut need_bootstrap, &mut attempts).await;
        match end {
            End::Stopped => return,
            End::SignedOut => {
                shared.emit(Update::SignedOut);
                return;
            }
            End::Resync => {
                need_bootstrap = true;
            }
            End::Lost(reason) => {
                let wait = timing.backoff(attempts);
                attempts += 1;
                log::info!("event socket lost ({reason}); retrying in {wait:?}");
                shared.status(Connection::Offline {
                    reason,
                    retry_ms: wait.as_millis() as u64,
                });
                // Frames queued while offline (typing) are stale; drop them.
                let deadline = tokio::time::sleep(wait);
                tokio::pin!(deadline);
                loop {
                    tokio::select! {
                        _ = &mut deadline => break,
                        frame = outgoing.recv() => if frame.is_none() { return },
                    }
                }
            }
        }
    }
}

async fn session<T: Transport>(
    transport: &T,
    shared: &Shared,
    outgoing: &mut mpsc::UnboundedReceiver<String>,
    timing: &Timing,
    need_bootstrap: &mut bool,
    attempts: &mut u32,
) -> End {
    shared.status(Connection::Connecting);
    if *need_bootstrap {
        match transport.bootstrap().await {
            Ok(snapshot) => {
                let changes = shared.store.write().unwrap().reset(snapshot);
                shared.emit(Update::Changed(changes));
                *need_bootstrap = false;
            }
            Err(ApiError::Unauthorized) => return End::SignedOut,
            Err(e) => return End::Lost(e.to_string()),
        }
    }
    let mut conn = match transport.connect().await {
        Ok(conn) => conn,
        Err(ApiError::Unauthorized) => return End::SignedOut,
        Err(e) => return End::Lost(e.to_string()),
    };
    match tokio::time::timeout(timing.ready, conn.recv()).await {
        Ok(Some(Ok(text))) => match ServerEvent::decode(&text) {
            Ok(ServerEvent::Ready(_)) => {}
            Ok(ServerEvent::Resync(_)) => return End::Resync,
            _ => return End::Lost("the server did not start the session".into()),
        },
        Ok(Some(Err(e))) => return End::Lost(e),
        Ok(None) => return End::Lost("the server closed the connection".into()),
        Err(_) => return End::Lost("the server did not answer in time".into()),
    }
    let cursor = shared.store.read().unwrap().cursor;
    if let Err(e) = conn
        .send(resume_frame(cursor, &shared.tab, &shared.incarnation))
        .await
    {
        return End::Lost(e);
    }
    shared.status(Connection::Replaying);
    loop {
        tokio::select! {
            frame = conn.recv() => {
                let text = match frame {
                    None => return End::Lost("the connection closed".into()),
                    Some(Err(e)) => return End::Lost(e),
                    Some(Ok(text)) => text,
                };
                let Ok(event) = ServerEvent::decode(&text) else {
                    log::warn!("ignoring a frame that is not JSON");
                    continue;
                };
                match &event {
                    ServerEvent::Synced(_) => {
                        *attempts = 0;
                        shared.status(Connection::Live);
                    }
                    ServerEvent::Alert(alert) => shared.emit(Update::Alert(alert.clone())),
                    ServerEvent::Routing(r) => {
                        *shared.routing.lock().unwrap() = r.connection_id.clone();
                        shared.routing_changed.notify_one();
                    }
                    _ => {}
                }
                let changes = shared.store.write().unwrap().apply(&event, now_ms());
                if changes.resync {
                    return End::Resync;
                }
                if !changes.is_empty() {
                    shared.emit(Update::Changed(changes));
                }
            }
            frame = outgoing.recv() => match frame {
                None => return End::Stopped,
                Some(text) => if let Err(e) = conn.send(text).await {
                    return End::Lost(e);
                },
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    /// A scripted server: each `connect` takes the next list of frames.
    #[derive(Clone, Default)]
    struct Fake {
        snapshots: Arc<Mutex<VecDeque<Result<Snapshot, ApiError>>>>,
        sockets: Arc<Mutex<VecDeque<Vec<String>>>>,
        sent: Arc<Mutex<Vec<String>>>,
    }

    struct FakeConn {
        frames: VecDeque<String>,
        sent: Arc<Mutex<Vec<String>>>,
        resumed: bool,
    }

    impl Transport for Fake {
        type Conn = FakeConn;
        async fn bootstrap(&self) -> Result<Snapshot, ApiError> {
            self.snapshots
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(Err(ApiError::Network("no more snapshots".into())))
        }
        async fn connect(&self) -> Result<FakeConn, ApiError> {
            let frames = self
                .sockets
                .lock()
                .unwrap()
                .pop_front()
                .ok_or(ApiError::Unauthorized)?;
            Ok(FakeConn {
                frames: frames.into(),
                sent: self.sent.clone(),
                resumed: false,
            })
        }
    }

    impl Conn for FakeConn {
        async fn recv(&mut self) -> Option<Result<String, String>> {
            // Hold everything after session.ready until the client resumes.
            if self.resumed || self.frames.front().is_some_and(|f| f.contains("session.ready")) {
                self.frames.pop_front().map(Ok)
            } else {
                Some(Err("frame before resume".into()))
            }
        }
        async fn send(&mut self, text: String) -> Result<(), String> {
            self.resumed |= text.contains("session.resume");
            self.sent.lock().unwrap().push(text);
            Ok(())
        }
    }

    fn snapshot(cursor: i64) -> Snapshot {
        crate::store::tests_support::snapshot(cursor)
    }

    fn ready() -> String {
        r#"{"v":1,"type":"session.ready","cursor":0,"sync":true}"#.into()
    }

    fn message(seq: i64, id: &str) -> String {
        format!(
            r#"{{"v":1,"seq":{seq},"type":"message.created","data":{{"id":"{id}","conversationId":"c-a",
            "createdAt":"2026-11-21T10:00:0{seq}Z","kind":"message","senderId":"m-bob","text":"t{seq}","reactions":[]}}}}"#
        )
    }

    fn synced(cursor: i64) -> String {
        format!(r#"{{"v":1,"type":"session.synced","cursor":{cursor}}}"#)
    }

    fn fast() -> Timing {
        Timing {
            ready: Duration::from_millis(200),
            backoff_base: Duration::from_millis(1),
            backoff_max: Duration::from_millis(5),
        }
    }

    async fn drive(fake: Fake) -> (Arc<Shared>, Vec<Update>) {
        let (tx, rx) = futures::channel::mpsc::unbounded();
        let shared = Arc::new(Shared {
            store: Arc::default(),
            updates: tx,
            routing: Mutex::new(None),
            routing_changed: Notify::new(),
            tab: "tab".into(),
            incarnation: "inc".into(),
        });
        let (_out_tx, out_rx) = mpsc::unbounded_channel();
        // The fake answers Unauthorized once its scripted sockets run out, which ends the run.
        run(fake, shared.clone(), out_rx, fast()).await;
        let mut rx = rx;
        let mut updates = Vec::new();
        while let Ok(update) = rx.try_recv() {
            updates.push(update);
        }
        (shared, updates)
    }

    #[tokio::test]
    async fn resumes_from_the_bootstrap_cursor_then_replays_and_goes_live() {
        let fake = Fake::default();
        fake.snapshots.lock().unwrap().push_back(Ok(snapshot(5)));
        fake.sockets
            .lock()
            .unwrap()
            .push_back(vec![ready(), message(6, "a"), synced(6), message(7, "b")]);
        let (shared, updates) = drive(fake.clone()).await;
        let resume: serde_json::Value =
            serde_json::from_str(&fake.sent.lock().unwrap()[0]).unwrap();
        assert_eq!(resume["after"], 5);
        assert_eq!(resume["sync"], true);
        assert_eq!(shared.store.read().unwrap().cursor, 7);
        assert!(updates.contains(&Update::Connection(Connection::Live)));
        assert_eq!(updates.last(), Some(&Update::SignedOut));
    }

    #[tokio::test]
    async fn reconnects_resume_from_the_last_event_seen() {
        let fake = Fake::default();
        fake.snapshots.lock().unwrap().push_back(Ok(snapshot(1)));
        {
            let mut s = fake.sockets.lock().unwrap();
            s.push_back(vec![ready(), synced(1), message(2, "a")]);
            s.push_back(vec![ready(), message(3, "b"), synced(3)]);
        }
        let (shared, updates) = drive(fake.clone()).await;
        let sent = fake.sent.lock().unwrap();
        let after: Vec<i64> = sent
            .iter()
            .map(|s| serde_json::from_str::<serde_json::Value>(s).unwrap()["after"].as_i64().unwrap())
            .collect();
        assert_eq!(after, [1, 2]);
        assert_eq!(shared.store.read().unwrap().cursor, 3);
        assert!(updates
            .iter()
            .any(|u| matches!(u, Update::Connection(Connection::Offline { .. }))));
    }

    #[tokio::test]
    async fn resync_and_gaps_bootstrap_again() {
        let fake = Fake::default();
        {
            let mut s = fake.snapshots.lock().unwrap();
            s.push_back(Ok(snapshot(1)));
            s.push_back(Ok(snapshot(40)));
            s.push_back(Ok(snapshot(90)));
        }
        {
            let mut s = fake.sockets.lock().unwrap();
            // The server says the cursor is too old.
            s.push_back(vec![
                ready(),
                r#"{"v":1,"type":"session.resync","reason":"cursor is outside retained events"}"#.into(),
            ]);
            // Then a sequence gap the client notices itself.
            s.push_back(vec![ready(), synced(40), message(42, "gap")]);
            s.push_back(vec![ready(), synced(90)]);
        }
        let (shared, _) = drive(fake.clone()).await;
        let sent = fake.sent.lock().unwrap();
        let after: Vec<i64> = sent
            .iter()
            .map(|s| serde_json::from_str::<serde_json::Value>(s).unwrap()["after"].as_i64().unwrap())
            .collect();
        assert_eq!(after, [1, 40, 90]);
        assert_eq!(shared.store.read().unwrap().cursor, 90);
    }

    #[tokio::test]
    async fn an_expired_session_signs_out() {
        let fake = Fake::default();
        fake.snapshots
            .lock()
            .unwrap()
            .push_back(Err(ApiError::Unauthorized));
        let (_, updates) = drive(fake).await;
        assert_eq!(updates.last(), Some(&Update::SignedOut));
    }

    #[test]
    fn backoff_grows_and_caps() {
        let t = Timing::default();
        assert!(t.backoff(0) >= Duration::from_millis(800));
        assert!(t.backoff(0) < Duration::from_millis(1050));
        assert!(t.backoff(3) >= Duration::from_millis(6400));
        assert!(t.backoff(10) <= Duration::from_millis(15_250));
    }
}

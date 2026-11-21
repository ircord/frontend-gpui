//! The client handle the app holds: it owns a small Tokio runtime, the shared
//! workspace, the event task and the HTTP commands.

use std::sync::{Arc, Mutex, RwLock, RwLockReadGuard};
use std::time::{Duration, Instant};

use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use ircord_contract as c;
use tokio::runtime::Runtime;
use tokio::sync::{Notify, mpsc};

use crate::api::{Api, ApiError};
use crate::credentials::CredentialStore;
use crate::events::{self, Shared, Timing, Update};
use crate::socket::WsTransport;
use crate::store::{Changes, PendingSend, SendState, Workspace};

/// How many messages one history page asks for.
pub const PAGE_SIZE: u32 = 60;
/// How often a typing indicator is renewed while the user keeps typing.
const TYPING_RENEW: Duration = Duration::from_secs(3);
/// How often an active window renews its reading lease (the server's lasts 60 s).
const ACTIVITY_RENEW: Duration = Duration::from_secs(30);

pub fn runtime() -> std::io::Result<Arc<Runtime>> {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .thread_name("ircord-net")
        .enable_all()
        .build()
        .map(Arc::new)
}

/// Sign in and keep the session in `store`.
pub async fn sign_in(
    api: &Api,
    store: &dyn CredentialStore,
    username: &str,
    password: &str,
) -> Result<(), String> {
    let cookie = api
        .login(username, password)
        .await
        .map_err(|e| e.to_string())?;
    store
        .save(api.server(), &cookie)
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[derive(Default)]
struct Activity {
    active: bool,
    viewed: Option<String>,
    reported_active: Option<bool>,
    reported_connection: Option<String>,
    sequence: i64,
}

#[derive(Default)]
struct TypingState {
    conversation: Option<String>,
    last_sent: Option<Instant>,
}

pub struct Client {
    runtime: Arc<Runtime>,
    api: Api,
    shared: Arc<Shared>,
    socket: mpsc::UnboundedSender<String>,
    updates: UnboundedSender<Update>,
    activity: Arc<Mutex<Activity>>,
    activity_changed: Arc<Notify>,
    typing: Mutex<TypingState>,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}

impl Drop for Client {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}

impl Client {
    /// Start with a signed-in `api`: bootstrap, open the socket, keep it open.
    pub fn start(runtime: Arc<Runtime>, api: Api) -> (Client, UnboundedReceiver<Update>) {
        Self::start_with(runtime, api, Timing::default())
    }

    pub fn start_with(
        runtime: Arc<Runtime>,
        api: Api,
        timing: Timing,
    ) -> (Client, UnboundedReceiver<Update>) {
        let (updates, rx) = unbounded();
        let shared = Arc::new(Shared {
            store: Arc::new(RwLock::new(Workspace::default())),
            updates: updates.clone(),
            routing: Mutex::new(None),
            routing_changed: Notify::new(),
            tab: random_id(),
            incarnation: random_id(),
        });
        let (socket, outgoing) = mpsc::unbounded_channel();
        let events = runtime.spawn(events::run(
            WsTransport { api: api.clone() },
            shared.clone(),
            outgoing,
            timing,
        ));
        let activity = Arc::new(Mutex::new(Activity::default()));
        let activity_changed = Arc::new(Notify::new());
        let reporter = runtime.spawn(report_activity(
            api.clone(),
            shared.clone(),
            activity.clone(),
            activity_changed.clone(),
        ));
        let client = Client {
            runtime,
            api,
            shared,
            socket,
            updates,
            activity,
            activity_changed,
            typing: Mutex::default(),
            tasks: vec![events, reporter],
        };
        (client, rx)
    }

    pub fn api(&self) -> &Api {
        &self.api
    }

    pub fn runtime(&self) -> &Arc<Runtime> {
        &self.runtime
    }

    /// Read the workspace. Hold the guard only while drawing.
    pub fn read(&self) -> RwLockReadGuard<'_, Workspace> {
        self.shared.store.read().unwrap()
    }

    /// Change the workspace locally and tell the app what changed.
    pub fn change(&self, f: impl FnOnce(&mut Workspace) -> Changes) {
        let changes = f(&mut self.shared.store.write().unwrap());
        if !changes.is_empty() {
            let _ = self.updates.unbounded_send(Update::Changed(changes));
        }
    }

    /// Show a conversation: load its newest page unless it is already loaded.
    pub fn open(&self, conversation: &str) {
        let loaded = self
            .read()
            .timelines
            .get(conversation)
            .is_some_and(|t| t.loaded);
        if !loaded {
            self.refresh(conversation);
        }
    }

    /// Re-read the newest page.
    pub fn refresh(&self, conversation: &str) {
        self.shared.store.write().unwrap().begin_load(conversation);
        let api = self.api.clone();
        let store = self.shared.store.clone();
        let updates = self.updates.clone();
        let id = conversation.to_string();
        self.runtime.spawn(async move {
            match api.messages(&id, None, PAGE_SIZE).await {
                Ok(page) => {
                    let changes = store.write().unwrap().load_newest(&id, page);
                    let _ = updates.unbounded_send(Update::Changed(changes));
                }
                Err(ApiError::Unauthorized) => {
                    let _ = updates.unbounded_send(Update::SignedOut);
                }
                Err(e) => {
                    let mut changes = store.write().unwrap().load_newest(
                        &id,
                        c::MessagePage {
                            sync_error: Some(e.to_string()),
                            next_cursor: None,
                            ..Default::default()
                        },
                    );
                    changes.notices.push(e.to_string());
                    let _ = updates.unbounded_send(Update::Changed(changes));
                }
            }
        });
    }

    /// Load the page before the oldest message shown, if there is one.
    pub fn load_older(&self, conversation: &str) {
        let Some(cursor) = self.shared.store.write().unwrap().begin_older(conversation) else {
            return;
        };
        let api = self.api.clone();
        let store = self.shared.store.clone();
        let updates = self.updates.clone();
        let id = conversation.to_string();
        self.runtime.spawn(async move {
            let result = api.messages(&id, Some(&cursor), PAGE_SIZE).await;
            let mut changes = store
                .write()
                .unwrap()
                .load_older(&id, result.as_ref().ok().cloned());
            if let Err(e) = result {
                changes.notices.push(e.to_string());
            }
            let _ = updates.unbounded_send(Update::Changed(changes));
        });
    }

    /// Send what the user typed. `/me text` sends an action.
    pub fn send(&self, conversation: &str, text: &str) {
        let text = text.trim_end_matches(['\n', '\r']);
        if text.trim().is_empty() {
            return;
        }
        let (action, text) = match text.strip_prefix("/me ") {
            Some(rest) => (true, rest.to_string()),
            None => (
                false,
                text.strip_prefix("//")
                    .map_or(text.to_string(), |t| format!("/{t}")),
            ),
        };
        let pending = PendingSend {
            client_id: random_id(),
            conversation_id: conversation.to_string(),
            text,
            action,
            state: SendState::Sending,
        };
        self.stop_typing();
        self.change(|w| w.queue_send(pending.clone()));
        self.post(pending, false);
    }

    /// Send a failed message again.
    pub fn retry(&self, client_id: &str) {
        let Some(pending) = self.shared.store.write().unwrap().retry_send(client_id) else {
            return;
        };
        self.change(|_| Changes {
            timelines: [pending.conversation_id.clone()].into(),
            ..Default::default()
        });
        self.post(pending, true);
    }

    fn post(&self, pending: PendingSend, retry: bool) {
        let api = self.api.clone();
        let store = self.shared.store.clone();
        let updates = self.updates.clone();
        self.runtime.spawn(async move {
            let result = if retry {
                match api.retry_send(&pending.client_id).await {
                    // The server never saw the first attempt: send it fresh.
                    Err(ApiError::Http { status: 404, .. }) => send_fresh(&api, &pending).await,
                    other => other,
                }
            } else {
                send_fresh(&api, &pending).await
            };
            let changes = match result {
                Ok(response) => store.write().unwrap().send_answered(&response),
                Err(e) => store
                    .write()
                    .unwrap()
                    .fail_send(&pending.client_id, &e.to_string())
                    .unwrap_or_default(),
            };
            let _ = updates.unbounded_send(Update::Changed(changes));
        });
    }

    /// Mark a conversation read if it has anything unread.
    pub fn mark_read(&self, conversation: &str) {
        let unread = self
            .read()
            .conversation(conversation)
            .is_some_and(|c| c.unread_count > 0 || c.mention_count > 0 || c.unread_from.is_some());
        if !unread {
            return;
        }
        self.change(|w| w.mark_read(conversation));
        let api = self.api.clone();
        let id = conversation.to_string();
        let updates = self.updates.clone();
        self.runtime.spawn(async move {
            if let Err(e) = api.mark_read(&id).await {
                log::warn!("mark read failed: {e}");
                if matches!(e, ApiError::Unauthorized) {
                    let _ = updates.unbounded_send(Update::SignedOut);
                }
            }
        });
    }

    /// The composer changed. Sends `typing.set` where the network supports it.
    pub fn typing(&self, conversation: &str, has_text: bool) {
        if !has_text {
            self.stop_typing();
            return;
        }
        let supported = {
            let w = self.read();
            w.conversation(conversation)
                .and_then(|c| w.network(&c.network_id))
                .is_some_and(|n| n.features.typing)
        };
        if !supported {
            return;
        }
        let mut t = self.typing.lock().unwrap();
        let fresh = t.conversation.as_deref() != Some(conversation)
            || t.last_sent.is_none_or(|at| at.elapsed() >= TYPING_RENEW);
        if fresh {
            if let Some(previous) = t.conversation.take()
                && previous != conversation
            {
                let _ = self.socket.send(c::event::typing_frame(
                    &previous,
                    c::TypingCommandState::Done,
                ));
            }
            let _ = self.socket.send(c::event::typing_frame(
                conversation,
                c::TypingCommandState::Active,
            ));
            t.conversation = Some(conversation.to_string());
            t.last_sent = Some(Instant::now());
        }
    }

    fn stop_typing(&self) {
        let mut t = self.typing.lock().unwrap();
        if let Some(conversation) = t.conversation.take() {
            let _ = self.socket.send(c::event::typing_frame(
                &conversation,
                c::TypingCommandState::Done,
            ));
        }
        t.last_sent = None;
    }

    /// The window gained or lost focus, or the viewed conversation changed.
    pub fn set_activity(&self, active: bool, viewed: Option<&str>) {
        let mut a = self.activity.lock().unwrap();
        a.active = active;
        a.viewed = viewed.map(str::to_string);
        drop(a);
        self.activity_changed.notify_one();
    }

    /// Drop expired typing indicators. Call about once a second.
    pub fn tick(&self) {
        self.change(|w| w.expire_typing(events::now_ms()));
    }

    /// Sign out on the server and forget the stored session.
    pub fn sign_out(&self, store: Arc<dyn CredentialStore>) -> tokio::task::JoinHandle<()> {
        let api = self.api.clone();
        self.runtime.spawn(async move {
            if let Err(e) = api.logout().await {
                log::warn!("sign out: {e}");
            }
            if let Err(e) = store.clear(api.server()) {
                log::warn!("forget session: {e}");
            }
        })
    }
}

async fn send_fresh(api: &Api, pending: &PendingSend) -> Result<c::CommandResponse, ApiError> {
    let body = c::MessageInputBody {
        client_id: pending.client_id.clone(),
        text: pending.text.clone(),
        kind: Some(if pending.action {
            c::MessageInputBodyKind::Action
        } else {
            c::MessageInputBodyKind::Message
        }),
        ..Default::default()
    };
    api.send_message(&pending.conversation_id, &body).await
}

/// Report activity whenever focus, the viewed conversation or the routing
/// connection changes, and renew every 30 seconds while active.
async fn report_activity(
    api: Api,
    shared: Arc<Shared>,
    activity: Arc<Mutex<Activity>>,
    changed: Arc<Notify>,
) {
    loop {
        tokio::select! {
            _ = changed.notified() => {}
            _ = shared.routing_changed.notified() => {}
            _ = tokio::time::sleep(ACTIVITY_RENEW) => {}
        }
        let Some(connection) = shared.routing.lock().unwrap().clone() else {
            continue;
        };
        let report = {
            let mut a = activity.lock().unwrap();
            let new_connection = a.reported_connection.as_deref() != Some(connection.as_str());
            let signal = if new_connection || a.reported_active != Some(a.active) {
                if a.active {
                    c::ReportSignal::Engagement
                } else {
                    c::ReportSignal::Inactive
                }
            } else if a.active {
                c::ReportSignal::Renewal
            } else {
                continue;
            };
            a.sequence += 1;
            a.reported_active = Some(a.active);
            a.reported_connection = Some(connection.clone());
            c::Report {
                active: a.active,
                can_alert: Some(true),
                connection_id: connection,
                form_factor: Some(c::ReportFormFactor::Desktop),
                incarnation: shared.incarnation.clone(),
                permission: c::ReportPermission::Granted,
                sequence: a.sequence,
                signal,
                tab: shared.tab.clone(),
                viewed_conversation_id: if a.active { a.viewed.clone() } else { None },
            }
        };
        if let Err(e) = api.report_activity(&report).await {
            log::debug!("activity report: {e}");
        }
    }
}

/// 128 random bits as hex: client IDs for sends, tab and incarnation IDs.
pub fn random_id() -> String {
    format!("{:032x}", rand::random::<u128>())
}

//! The client handle the app holds: it owns a small Tokio runtime, the shared
//! workspace, the event task and the HTTP commands.
//!
//! A feature does not add a method here. It runs a request with
//! [`Client::call`] (or [`Handle::call`] from a background task), which runs
//! it off the UI thread and applies the result to the workspace in one step;
//! it reacts to what changed with [`Client::watch`]; and it keeps a
//! revisioned server document with [`Client::watch_document`] and
//! [`Client::edit_document`] (the specs are in [`crate::documents`]).

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock, RwLockReadGuard};
use std::time::{Duration, Instant};

use futures::StreamExt;
use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use ircord_contract as c;
use ircord_contract::event::Document;
use tokio::runtime::Runtime;
use tokio::sync::{Notify, mpsc};

use crate::api::{Api, ApiError};
use crate::credentials::CredentialStore;
use crate::events::{self, Shared, Timing, Update};
use crate::socket::WsTransport;
use crate::store::documents::document_id;
use crate::store::{Changes, PendingSend, Revisioned, SendState, Workspace, topic};

/// How long an edit to a revisioned document waits before it is saved, so a
/// burst of edits is one request.
pub const DOCUMENT_SAVE_DELAY: Duration = Duration::from_millis(400);

/// A boxed future, as [`DocumentSpec`]'s calls return.
pub type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;

/// A watcher sees every set of changes before the app does, on the network thread.
type Watcher = Arc<dyn Fn(&Handle, &Changes) + Send + Sync>;

/// How one revisioned document is read, written and kept in the workspace.
/// Declare it as a `static` in the feature that owns the document.
pub struct DocumentSpec<T: 'static> {
    pub document: Document,
    /// Where the document lives in the workspace (a field of the feature's sub-state).
    pub state: fn(&mut Workspace) -> &mut Revisioned<T>,
    pub read: fn(Api) -> BoxFuture<Result<T, ApiError>>,
    /// Store an edit; the answer is the document as the server stored it.
    pub write: fn(Api, T) -> BoxFuture<Result<T, ApiError>>,
}

impl<T> DocumentSpec<T> {
    /// What every change to this document's state reports.
    fn changes(&self) -> Changes {
        Changes::topic_id(topic::DOCUMENTS, document_id(self.document))
    }
}

/// A cheap, cloneable handle to the client's workspace and server, for
/// background work and watchers. [`Client`] hands one out with [`Client::handle`].
#[derive(Clone)]
pub struct Handle {
    runtime: tokio::runtime::Handle,
    api: Api,
    store: Arc<RwLock<Workspace>>,
    updates: UnboundedSender<Update>,
    watchers: Arc<Mutex<Vec<Watcher>>>,
}

impl Handle {
    pub fn api(&self) -> &Api {
        &self.api
    }

    /// Read the workspace. Do not hold the guard across an `await`.
    pub fn read(&self) -> RwLockReadGuard<'_, Workspace> {
        self.store.read().unwrap()
    }

    /// Change the workspace and tell watchers and the app what changed.
    pub fn change(&self, f: impl FnOnce(&mut Workspace) -> Changes) {
        let changes = f(&mut self.store.write().unwrap());
        if !changes.is_empty() {
            self.emit(Update::Changed(changes));
        }
    }

    /// Tell the app, running the watchers first on this thread for changes.
    /// The workspace lock must not be held.
    fn emit(&self, update: Update) {
        if let Update::Changed(changes) = &update {
            let watchers = self.watchers.lock().unwrap().clone();
            for watcher in watchers {
                watcher(self, changes);
            }
        }
        let _ = self.updates.unbounded_send(update);
    }

    /// Run `request` against the server off the UI thread, then apply its
    /// result to the workspace with `apply` and report what that changed.
    ///
    /// `apply` also sees failures, so it can record them; an expired session
    /// additionally signs the app out.
    pub fn call<T, Fut>(
        &self,
        request: impl FnOnce(Api) -> Fut + Send + 'static,
        apply: impl FnOnce(&mut Workspace, Result<T, ApiError>) -> Changes + Send + 'static,
    ) -> tokio::task::JoinHandle<()>
    where
        T: Send + 'static,
        Fut: Future<Output = Result<T, ApiError>> + Send + 'static,
    {
        let handle = self.clone();
        self.runtime.spawn(async move {
            let result = request(handle.api.clone()).await;
            if matches!(result, Err(ApiError::Unauthorized)) {
                let _ = handle.updates.unbounded_send(Update::SignedOut);
            }
            handle.change(|w| apply(w, result));
        })
    }

    /// Run `request` off the UI thread and hand back its result, for answers
    /// a view keeps itself (a search, a whois). Awaiting the handle works on
    /// any executor. An expired session also signs the app out.
    pub fn request<T, Fut>(
        &self,
        request: impl FnOnce(Api) -> Fut + Send + 'static,
    ) -> tokio::task::JoinHandle<Result<T, ApiError>>
    where
        T: Send + 'static,
        Fut: Future<Output = Result<T, ApiError>> + Send + 'static,
    {
        let api = self.api.clone();
        let updates = self.clone();
        self.runtime.spawn(async move {
            let result = request(api).await;
            if matches!(result, Err(ApiError::Unauthorized)) {
                updates.emit(Update::SignedOut);
            }
            result
        })
    }

    /// Call `f` with every set of changes from now on, before the app sees it.
    /// It runs on the network thread: keep it short and spawn real work with
    /// [`Handle::call`].
    pub fn watch(&self, f: impl Fn(&Handle, &Changes) + Send + Sync + 'static) {
        self.watchers.lock().unwrap().push(Arc::new(f));
    }

    /// Keep a revisioned document: read it now, and again whenever the server
    /// announces a newer revision or the workspace is bootstrapped again.
    pub fn watch_document<T: Clone + Send + Sync + 'static>(&self, spec: &'static DocumentSpec<T>) {
        self.watch(move |handle, changes| {
            let id = document_id(spec.document);
            if changes.touches_id(topic::REVISIONS, id) {
                let mut store = handle.store.write().unwrap();
                (spec.state)(&mut store).remote();
                drop(store);
                handle.sync_document(spec);
            }
        });
        self.sync_document(spec);
    }

    /// Show an edit to a revisioned document now and save it shortly.
    pub fn edit_document<T: Clone + Send + Sync + 'static>(
        &self,
        spec: &'static DocumentSpec<T>,
        value: T,
    ) {
        self.change(|w| {
            (spec.state)(w).edit(value);
            spec.changes()
        });
        let handle = self.clone();
        self.runtime.spawn(async move {
            tokio::time::sleep(DOCUMENT_SAVE_DELAY).await;
            handle.sync_document(spec);
        });
    }

    /// Start whatever the document needs next: a save of a waiting edit, or a read.
    fn sync_document<T: Clone + Send + Sync + 'static>(&self, spec: &'static DocumentSpec<T>) {
        let next = {
            let mut store = self.store.write().unwrap();
            let state = (spec.state)(&mut store);
            match state.begin_save() {
                Some(value) => Some(Ok(value)),
                None => state.begin_read().then_some(Err(())),
            }
        };
        let Some(next) = next else { return };
        let handle = self.clone();
        self.runtime.spawn(async move {
            let api = handle.api.clone();
            let unauthorized = |r: &Result<T, ApiError>| matches!(r, Err(ApiError::Unauthorized));
            match next {
                Ok(value) => {
                    let result = (spec.write)(api, value).await;
                    if unauthorized(&result) {
                        let _ = handle.updates.unbounded_send(Update::SignedOut);
                    }
                    let failed = result.as_ref().err().map(|e| e.to_string());
                    handle.change(|w| {
                        (spec.state)(w).save_done(result.map_err(|e| e.to_string()));
                        let mut changes = spec.changes();
                        changes.notices.extend(failed);
                        changes
                    });
                }
                Err(()) => {
                    let result = (spec.read)(api).await;
                    if unauthorized(&result) {
                        let _ = handle.updates.unbounded_send(Update::SignedOut);
                    }
                    let read = result.is_ok();
                    handle.change(|w| {
                        (spec.state)(w).read_done(result.map_err(|e| e.to_string()));
                        spec.changes()
                    });
                    // A failed read waits for the next event or edit instead of
                    // retrying at once; an edit made meanwhile goes on, and so
                    // does a read after an event that arrived during this one.
                    let next = {
                        let mut store = handle.store.write().unwrap();
                        let state = (spec.state)(&mut store);
                        state.needs_save() || (read && state.needs_read())
                    };
                    if !next {
                        return;
                    }
                }
            }
            handle.sync_document(spec);
        });
    }
}

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
    handle: Handle,
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
        // Updates go straight to the app from the thread that made them, as
        // the app's tests rely on; the watchers run first on that thread.
        // Only the event task's updates pass through a forwarder, because
        // its shared state predates the watchers.
        let (app, rx) = unbounded();
        let (from_events, internal) = unbounded();
        let watchers: Arc<Mutex<Vec<Watcher>>> = Arc::default();
        let shared = Arc::new(Shared {
            store: Arc::new(RwLock::new(Workspace::default())),
            updates: from_events,
            routing: Mutex::new(None),
            routing_changed: Notify::new(),
            tab: random_id(),
            incarnation: random_id(),
            read_aloud: AtomicBool::new(false),
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
        let handle = Handle {
            runtime: runtime.handle().clone(),
            api: api.clone(),
            store: shared.store.clone(),
            updates: app,
            watchers,
        };
        let forwarder = runtime.spawn(forward(handle.clone(), internal));
        let client = Client {
            runtime,
            api,
            shared,
            socket,
            handle,
            activity,
            activity_changed,
            typing: Mutex::default(),
            tasks: vec![events, reporter, forwarder],
        };
        (client, rx)
    }

    pub fn api(&self) -> &Api {
        &self.api
    }

    pub fn set_read_aloud(&self, enabled: bool) {
        self.shared.read_aloud.store(enabled, Ordering::Relaxed);
    }

    pub fn runtime(&self) -> &Arc<Runtime> {
        &self.runtime
    }

    /// A cloneable handle for background work.
    pub fn handle(&self) -> &Handle {
        &self.handle
    }

    /// See [`Handle::call`].
    pub fn call<T, Fut>(
        &self,
        request: impl FnOnce(Api) -> Fut + Send + 'static,
        apply: impl FnOnce(&mut Workspace, Result<T, ApiError>) -> Changes + Send + 'static,
    ) -> tokio::task::JoinHandle<()>
    where
        T: Send + 'static,
        Fut: Future<Output = Result<T, ApiError>> + Send + 'static,
    {
        self.handle.call(request, apply)
    }

    /// See [`Handle::request`].
    pub fn request<T, Fut>(
        &self,
        request: impl FnOnce(Api) -> Fut + Send + 'static,
    ) -> tokio::task::JoinHandle<Result<T, ApiError>>
    where
        T: Send + 'static,
        Fut: Future<Output = Result<T, ApiError>> + Send + 'static,
    {
        self.handle.request(request)
    }

    /// See [`Handle::watch`].
    pub fn watch(&self, f: impl Fn(&Handle, &Changes) + Send + Sync + 'static) {
        self.handle.watch(f)
    }

    /// See [`Handle::watch_document`].
    pub fn watch_document<T: Clone + Send + Sync + 'static>(&self, spec: &'static DocumentSpec<T>) {
        self.handle.watch_document(spec)
    }

    /// See [`Handle::edit_document`].
    pub fn edit_document<T: Clone + Send + Sync + 'static>(
        &self,
        spec: &'static DocumentSpec<T>,
        value: T,
    ) {
        self.handle.edit_document(spec, value)
    }

    /// Read the workspace. Hold the guard only while drawing.
    pub fn read(&self) -> RwLockReadGuard<'_, Workspace> {
        self.shared.store.read().unwrap()
    }

    /// Change the workspace locally and tell the app what changed.
    pub fn change(&self, f: impl FnOnce(&mut Workspace) -> Changes) {
        self.handle.change(f)
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
        let updates = self.handle.clone();
        let id = conversation.to_string();
        self.runtime.spawn(async move {
            match api.messages(&id, None, PAGE_SIZE).await {
                Ok(page) => {
                    let changes = store.write().unwrap().load_newest(&id, page);
                    updates.emit(Update::Changed(changes));
                }
                Err(ApiError::Unauthorized) => {
                    updates.emit(Update::SignedOut);
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
                    updates.emit(Update::Changed(changes));
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
        let updates = self.handle.clone();
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
            updates.emit(Update::Changed(changes));
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
        let updates = self.handle.clone();
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
            updates.emit(Update::Changed(changes));
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
        let updates = self.handle.clone();
        self.runtime.spawn(async move {
            if let Err(e) = api.mark_read(&id).await {
                log::warn!("mark read failed: {e}");
                if matches!(e, ApiError::Unauthorized) {
                    updates.emit(Update::SignedOut);
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

/// Pass the event task's updates to the app, letting the watchers see changes first.
async fn forward(handle: Handle, mut internal: UnboundedReceiver<Update>) {
    while let Some(update) = internal.next().await {
        handle.emit(update);
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

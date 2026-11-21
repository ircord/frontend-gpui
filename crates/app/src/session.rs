//! The app-wide model: which screen is showing, the signed-in client, the
//! route the main pane shows and the right-hand panel. Windows come and go
//! (the tray keeps the app running on Windows); the session outlives them.

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use gpui_kit::prelude::*;
use gpui_kit::*;
use ircord_client::api::Api;
use ircord_client::client::sign_in;
use ircord_client::config::{AppState, ConfigDir};
use ircord_client::credentials::CredentialStore;
use ircord_client::events::Update;
use ircord_client::store::Changes;
use ircord_client::{Client, ServerAddress};
use tokio::runtime::Runtime;

use crate::route::{History, Route};

/// The panel ID that closes the right-hand panel, the member list beside a
/// channel included (P1's "hide member list").
pub const NO_PANEL: &str = "none";

pub enum Phase {
    /// No server chosen yet.
    ChooseServer,
    /// A server is chosen; nobody is signed in.
    SignIn { server: ServerAddress },
    SignedIn {
        server: ServerAddress,
        client: Rc<Client>,
    },
}

/// What changed, for the views to decide whether to redraw.
#[derive(Clone, Debug)]
pub enum SessionEvent {
    /// The screen changed (signed in, signed out, server chosen).
    Phase,
    /// The selected network or conversation changed.
    Selection,
    /// The main pane's route or the right-hand panel changed.
    Route,
    /// The workspace changed.
    Changed(Rc<Changes>),
    /// The connection state changed.
    Connection,
    /// Show a message to the user.
    Notice(String),
}

pub struct Session {
    pub runtime: Arc<Runtime>,
    pub config: ConfigDir,
    pub credentials: Arc<dyn CredentialStore>,
    pub phase: Phase,
    pub network: Option<String>,
    /// The conversation last opened. It stays set while another route (a
    /// search, the inbox) shows, so going back returns to it.
    pub conversation: Option<String>,
    pub history: History,
    /// The right-hand panel's ID, if one other than the default (the member
    /// list beside a channel) is open.
    pub panel: Option<SharedString>,
    /// Alert IDs of shown notifications → the conversation each opens.
    pub alerts: HashMap<String, String>,
    pub window_active: bool,
    pub state: AppState,
    /// Set while signing in or checking a server, to disable the form.
    pub busy: bool,
    pub error: Option<String>,
    _updates: Option<Task<()>>,
    _ticker: Task<()>,
}

impl EventEmitter<SessionEvent> for Session {}

impl Session {
    pub fn new(
        runtime: Arc<Runtime>,
        config: ConfigDir,
        credentials: Arc<dyn CredentialStore>,
        cx: &mut Context<Self>,
    ) -> Self {
        let state = config.state();
        let ticker = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let alive = this.update(cx, |this, _| {
                    if let Phase::SignedIn { client, .. } = &this.phase {
                        client.tick();
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        });
        let mut session = Session {
            runtime,
            config,
            credentials,
            phase: Phase::ChooseServer,
            network: None,
            conversation: state.last_conversation.clone(),
            history: History::default(),
            panel: None,
            alerts: HashMap::new(),
            window_active: true,
            state,
            busy: false,
            error: None,
            _updates: None,
            _ticker: ticker,
        };
        session.resume(cx);
        session
    }

    /// Pick up where the last launch left off.
    fn resume(&mut self, cx: &mut Context<Self>) {
        let server = match self.config.server() {
            Ok(Some(server)) => server,
            Ok(None) => return,
            Err(e) => {
                self.error = Some(t!("core-saved-server-unreadable", error = e));
                return;
            }
        };
        match self.credentials.load(&server) {
            Ok(Some(cookie)) => self.start(server, cookie, cx),
            Ok(None) => self.phase = Phase::SignIn { server },
            Err(e) => {
                self.error = Some(e.to_string());
                self.phase = Phase::SignIn { server };
            }
        }
    }

    pub fn client(&self) -> Option<&Rc<Client>> {
        match &self.phase {
            Phase::SignedIn { client, .. } => Some(client),
            _ => None,
        }
    }

    pub fn server(&self) -> Option<&ServerAddress> {
        match &self.phase {
            Phase::ChooseServer => None,
            Phase::SignIn { server } | Phase::SignedIn { server, .. } => Some(server),
        }
    }

    fn set_phase(&mut self, phase: Phase, cx: &mut Context<Self>) {
        if !matches!(phase, Phase::SignedIn { .. }) {
            self.history = History::default();
            self.panel = None;
        }
        self.phase = phase;
        self.busy = false;
        cx.emit(SessionEvent::Phase);
        cx.notify();
    }

    /// Check an address and remember it.
    pub fn choose_server(&mut self, address: &str, cx: &mut Context<Self>) {
        let server = match ServerAddress::parse(address) {
            Ok(server) => server,
            Err(e) => {
                self.error = Some(e.to_string());
                cx.notify();
                return;
            }
        };
        self.busy = true;
        self.error = None;
        cx.notify();
        let api = Api::new(server.clone());
        let check = self.runtime.spawn(async move { api.verify_server().await });
        cx.spawn(async move |this, cx| {
            let result = check.await.map_err(|e| e.to_string());
            this.update(cx, |this, cx| match result {
                Ok(Ok(())) => {
                    if let Err(e) = this.config.save_server(&server) {
                        log::warn!("save server: {e}");
                    }
                    this.error = None;
                    this.set_phase(Phase::SignIn { server }, cx);
                }
                Ok(Err(e)) => {
                    this.busy = false;
                    this.error = Some(e.to_string());
                    cx.notify();
                }
                Err(e) => {
                    this.busy = false;
                    this.error = Some(e);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    pub fn change_server(&mut self, cx: &mut Context<Self>) {
        self.error = None;
        if let Err(e) = self.config.forget_server() {
            log::warn!("forget server: {e}");
        }
        self.set_phase(Phase::ChooseServer, cx);
    }

    pub fn sign_in(&mut self, username: &str, password: &str, cx: &mut Context<Self>) {
        let Some(server) = self.server().cloned() else {
            return;
        };
        if username.trim().is_empty() || password.is_empty() {
            self.error = Some(t!("core-sign-in-missing").into());
            cx.notify();
            return;
        }
        self.busy = true;
        self.error = None;
        cx.notify();
        let api = Api::new(server.clone());
        let store = self.credentials.clone();
        let (username, password) = (username.trim().to_string(), password.to_string());
        let task = self.runtime.spawn(async move {
            sign_in(&api, store.as_ref(), &username, &password)
                .await
                .map(|()| api.cookie().unwrap_or_default())
        });
        cx.spawn(async move |this, cx| {
            let result = task.await.map_err(|e| e.to_string()).and_then(|r| r);
            this.update(cx, |this, cx| match result {
                Ok(cookie) => this.start(server, cookie, cx),
                Err(e) => {
                    this.busy = false;
                    this.error = Some(e);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    fn start(&mut self, server: ServerAddress, cookie: String, cx: &mut Context<Self>) {
        let api = Api::new(server.clone());
        api.set_cookie(Some(cookie));
        let (client, mut updates) = Client::start(self.runtime.clone(), api);
        let client = Rc::new(client);
        self._updates = Some(cx.spawn(async move |this, cx| {
            while let Some(update) = updates.next().await {
                if this
                    .update(cx, |this, cx| this.on_update(update, cx))
                    .is_err()
                {
                    break;
                }
            }
        }));
        self.error = None;
        self.set_phase(Phase::SignedIn { server, client }, cx);
        self.report_activity();
    }

    fn on_update(&mut self, update: Update, cx: &mut Context<Self>) {
        match update {
            Update::Changed(changes) => {
                if changes.reset {
                    self.fix_selection(cx);
                }
                for notice in &changes.notices {
                    cx.emit(SessionEvent::Notice(notice.clone()));
                }
                // Reload history the server re-synced, if it is showing.
                if let (Some(open), Some(client)) = (&self.conversation, self.client())
                    && changes.refresh.contains(open)
                {
                    client.refresh(open);
                }
                if let Some(open) = &self.conversation
                    && changes.timelines.contains(open)
                    && self.window_active
                    && let Some(client) = self.client()
                {
                    client.mark_read(open);
                }
                cx.emit(SessionEvent::Changed(Rc::new(changes)));
            }
            Update::Connection(_) => {
                cx.emit(SessionEvent::Connection);
                cx.notify();
            }
            Update::Alert(alert) => crate::notify::show(self, alert, cx),
            Update::SignedOut => {
                if let Some(server) = self.server().cloned() {
                    if let Err(e) = self.credentials.clear(&server) {
                        log::warn!("forget session: {e}");
                    }
                    self.error = Some(t!("core-session-ended").into());
                    self._updates = None;
                    self.set_phase(Phase::SignIn { server }, cx);
                }
            }
        }
    }

    /// After a bootstrap: keep the selection if it still exists, else pick one.
    fn fix_selection(&mut self, cx: &mut Context<Self>) {
        let Some(client) = self.client().cloned() else {
            return;
        };
        let w = client.read();
        let conversation = self
            .conversation
            .as_ref()
            .and_then(|id| w.conversation(id))
            .map(|c| (c.network_id.clone(), c.id.clone()));
        let (network, conversation) = match conversation {
            Some((n, c)) => (Some(n), Some(c)),
            None => {
                let network = self
                    .network
                    .clone()
                    .filter(|n| w.network(n).is_some())
                    .or_else(|| w.networks.first().map(|n| n.id.clone()));
                let first = network
                    .as_ref()
                    .and_then(|n| w.sidebar(n).first().map(|c| c.id.clone()));
                (network, first)
            }
        };
        drop(w);
        let changed = self.network != network || self.conversation != conversation;
        self.network = network;
        self.conversation = conversation;
        if let Some(id) = &self.conversation {
            client.open(id);
        }
        let w = client.read();
        self.history.retain(|route| match route {
            Route::Conversation(id) => w.conversation(id).is_some(),
            Route::ServerWindow(id) => w.network(id).is_some(),
            _ => true,
        });
        drop(w);
        if self.history.current().is_none()
            && let Some(id) = &self.conversation
        {
            self.history.replace(Route::Conversation(id.clone()));
        }
        if changed {
            self.save_state();
            cx.emit(SessionEvent::Selection);
        }
    }

    pub fn select_network(&mut self, network: &str, cx: &mut Context<Self>) {
        let Some(client) = self.client().cloned() else {
            return;
        };
        let first = client.read().sidebar(network).first().map(|c| c.id.clone());
        self.network = Some(network.to_string());
        self.conversation = None;
        match first {
            Some(id) => self.select_conversation(&id, cx),
            None => {
                cx.emit(SessionEvent::Selection);
                cx.notify();
            }
        }
    }

    pub fn select_conversation(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(client) = self.client().cloned() else {
            return;
        };
        let network = client.read().conversation(id).map(|c| c.network_id.clone());
        let Some(network) = network else {
            return;
        };
        self.network = Some(network);
        self.conversation = Some(id.to_string());
        self.history.push(Route::Conversation(id.to_string()));
        client.open(id);
        if self.window_active {
            client.mark_read(id);
        }
        self.save_state();
        self.report_activity();
        cx.emit(SessionEvent::Selection);
        cx.notify();
    }

    /// What the main pane shows.
    pub fn route(&self) -> Option<Route> {
        self.history
            .current()
            .cloned()
            .or_else(|| self.conversation.clone().map(Route::Conversation))
    }

    /// Show `route` in the main pane.
    pub fn navigate(&mut self, route: Route, cx: &mut Context<Self>) {
        if let Route::Conversation(id) = &route {
            self.select_conversation(id, cx);
            return;
        }
        if self.history.push(route) {
            cx.emit(SessionEvent::Route);
            cx.notify();
        }
    }

    pub fn go_back(&mut self, cx: &mut Context<Self>) {
        if let Some(route) = self.history.back().cloned() {
            self.show(route, cx);
        }
    }

    pub fn go_forward(&mut self, cx: &mut Context<Self>) {
        if let Some(route) = self.history.forward().cloned() {
            self.show(route, cx);
        }
    }

    /// Show a route the history already moved to.
    fn show(&mut self, route: Route, cx: &mut Context<Self>) {
        if let Route::Conversation(id) = &route
            && let Some(client) = self.client().cloned()
        {
            let network = client.read().conversation(id).map(|c| c.network_id.clone());
            if let Some(network) = network {
                self.network = Some(network);
                self.conversation = Some(id.clone());
                client.open(id);
                if self.window_active {
                    client.mark_read(id);
                }
                self.save_state();
                self.report_activity();
                cx.emit(SessionEvent::Selection);
            }
        }
        cx.emit(SessionEvent::Route);
        cx.notify();
    }

    /// Open the right-hand panel `id`, or close it with `None`.
    pub fn set_panel(&mut self, id: Option<SharedString>, cx: &mut Context<Self>) {
        if self.panel != id {
            self.panel = id;
            cx.emit(SessionEvent::Route);
            cx.notify();
        }
    }

    pub fn set_window_active(&mut self, active: bool) {
        if self.window_active == active {
            return;
        }
        self.window_active = active;
        if active && let (Some(client), Some(id)) = (self.client(), &self.conversation) {
            client.mark_read(id);
        }
        self.report_activity();
    }

    fn report_activity(&self) {
        if let Some(client) = self.client() {
            client.set_activity(self.window_active, self.conversation.as_deref());
        }
    }

    pub fn send(&mut self, text: &str) {
        if let (Some(client), Some(id)) = (self.client(), &self.conversation) {
            client.send(id, text);
        }
    }

    pub fn sign_out(&mut self, cx: &mut Context<Self>) {
        let Some(server) = self.server().cloned() else {
            return;
        };
        if let Some(client) = self.client() {
            let done = client.sign_out(self.credentials.clone());
            cx.background_spawn(async move {
                let _ = done.await;
            })
            .detach();
        }
        self._updates = None;
        self.set_phase(Phase::SignIn { server }, cx);
    }

    fn save_state(&mut self) {
        self.state.last_conversation = self.conversation.clone();
        if let Err(e) = self.config.save_state(&self.state) {
            log::warn!("save state: {e}");
        }
    }

    pub fn save_window_size(&mut self, width: f32, height: f32) {
        self.state.window_width = Some(width);
        self.state.window_height = Some(height);
        self.save_state();
    }
}

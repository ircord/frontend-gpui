//! Per-device opt-in and bounded playback of live highlights.

use std::collections::VecDeque;
use std::io;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::{Context, Task};
use ircord_client::Client;
use ircord_client::api::speech::{Endpoint, Error};
use ircord_client::config::ConfigDir;
use ircord_client::credentials::CredentialStore;
use ircord_client::store::read_aloud::Highlight;
use ircord_platform::audio::Track;
use tokio::runtime::Runtime;
use tokio::task::AbortHandle;

use crate::session::{Session, SessionEvent};

const DEVICE_SECTION: &str = "read-aloud";
const MAX_WAITING: usize = 5;
const MAX_TEXT: usize = 300;
const ERROR_INTERVAL: Duration = Duration::from_secs(60);

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Target {
    scope: String,
    endpoint: Endpoint,
}

pub(crate) fn scope_for(session: &Session, endpoint: &Endpoint) -> Option<String> {
    let server = session.server()?.origin();
    let w = session.client()?.read();
    let account = &w.user.id;
    (!account.is_empty()).then(|| format!("{server}\n{account}\n{}", endpoint.origin()))
}

pub(crate) fn target(session: &Session) -> Option<Target> {
    let endpoint = {
        let w = session.client()?.read();
        let settings = w.documents.preferences.value()?.preferences.as_ref()?;
        Endpoint::new(settings.speech.as_ref()?).ok()?
    };
    let scope = scope_for(session, &endpoint)?;
    Some(Target { scope, endpoint })
}

pub(crate) struct Reader {
    allowed: Vec<String>,
    target: Option<Target>,
    waiting: VecDeque<String>,
    task: Option<Task<()>>,
    download: Option<AbortHandle>,
    player: Option<Rc<Track>>,
    last_error: Option<Instant>,
}

impl Reader {
    pub(crate) fn new(config: &ConfigDir) -> Self {
        Self {
            allowed: config.section(DEVICE_SECTION),
            target: None,
            waiting: VecDeque::new(),
            task: None,
            download: None,
            player: None,
            last_error: None,
        }
    }

    pub(crate) fn enabled(&self) -> bool {
        self.target.is_some()
    }

    pub(crate) fn change(
        &mut self,
        target: &Target,
        enabled: bool,
        config: &ConfigDir,
    ) -> io::Result<()> {
        let mut allowed = self.allowed.clone();
        allowed.retain(|key| key != &target.scope);
        if enabled {
            allowed.push(target.scope.clone());
            if allowed.len() > 30 {
                allowed.remove(0);
            }
        }
        let saved = config.save_section(DEVICE_SECTION, &allowed);
        if saved.is_ok() || !enabled {
            self.allowed = allowed;
        }
        saved
    }

    pub(crate) fn sync(&mut self, target: Option<Target>, client: Option<&Client>) {
        let next = target.filter(|target| self.allowed.contains(&target.scope));
        if self.target != next {
            self.cancel();
            self.target = next;
        }
        if let Some(client) = client {
            client.set_read_aloud(self.enabled());
        }
    }

    fn cancel(&mut self) {
        if let Some(download) = self.download.take() {
            download.abort();
        }
        if let Some(player) = self.player.take() {
            player.pause();
        }
        self.waiting.clear();
        self.task = None;
        self.last_error = None;
    }

    pub(crate) fn incoming(
        &mut self,
        highlight: Highlight,
        runtime: &Arc<Runtime>,
        keys: &Arc<dyn CredentialStore>,
        cx: &mut Context<Session>,
    ) {
        if !self.enabled() {
            return;
        }
        if self.waiting.len() == MAX_WAITING {
            self.waiting.pop_front();
        }
        self.waiting.push_back(line(highlight));
        self.start(runtime, keys, cx);
    }

    fn start(
        &mut self,
        runtime: &Arc<Runtime>,
        keys: &Arc<dyn CredentialStore>,
        cx: &mut Context<Session>,
    ) {
        if self.task.is_some() {
            return;
        }
        let Some(target) = self.target.clone() else {
            return;
        };
        let Some(text) = self.waiting.pop_front() else {
            return;
        };
        let (runtime, keys) = (runtime.clone(), keys.clone());
        let request_keys = keys.clone();
        let download = runtime.spawn(async move {
            let key = tokio::task::spawn_blocking(move || {
                request_keys.speech_key(&target.scope).ok().flatten()
            })
            .await
            .ok()
            .flatten()
            .unwrap_or_default();
            target.endpoint.synthesize(&text, &key).await
        });
        self.download = Some(download.abort_handle());
        self.task = Some(cx.spawn(async move |session, cx| {
            let response = download.await.unwrap_or(Err(Error::Network));
            let player = session.update(cx, |session, cx| session.read_aloud.play(response, cx));
            if let Ok(Some(player)) = player {
                while !player.ended() {
                    cx.background_executor()
                        .timer(Duration::from_millis(100))
                        .await;
                }
            }
            let _ = session.update(cx, |session, cx| {
                session.read_aloud.player = None;
                session.read_aloud.task = None;
                session.read_aloud.start(&runtime, &keys, cx);
            });
        }));
    }

    fn play(
        &mut self,
        response: Result<Vec<u8>, Error>,
        cx: &mut Context<Session>,
    ) -> Option<Rc<Track>> {
        self.download = None;
        if let Ok(bytes) = response
            && let Some(output) = crate::media::sound::output(cx)
            && let Ok(player) = output.track(bytes)
        {
            let player = Rc::new(player);
            player.play();
            self.player = Some(player.clone());
            Some(player)
        } else {
            if self
                .last_error
                .is_none_or(|last| last.elapsed() >= ERROR_INTERVAL)
            {
                self.last_error = Some(Instant::now());
                cx.emit(SessionEvent::Notice(t!("speech-highlight-failed").into()));
            }
            None
        }
    }
}

fn line(highlight: Highlight) -> String {
    let from = highlight.sender.as_deref().unwrap_or(t!("speech-someone"));
    let body = if highlight.gif {
        t!("speech-sent-gif")
    } else {
        &highlight.text
    };
    let line = match highlight.channel {
        Some(channel) => t!(
            "speech-channel-line",
            sender = from,
            channel = channel,
            body = body
        ),
        None => t!("speech-direct-line", sender = from, body = body),
    };
    if let Some((offset, _)) = line.char_indices().nth(MAX_TEXT - 1)
        && line[offset..].chars().count() > 1
    {
        format!("{}…", line[..offset].trim_end())
    } else {
        line
    }
}

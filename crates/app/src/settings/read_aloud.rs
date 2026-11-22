//! The account's speech service and the key and sample on this device.

use std::rc::Rc;
use std::time::Duration;

use gpui_kit::TestSupportExt;
use gpui_kit::component::Disableable;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::prelude::*;
use gpui_kit::*;
use ircord_client::api::speech::{self, Endpoint};
use ircord_client::contract::SpeechService;
use ircord_client::store::topic;
use ircord_platform::audio::Track;

use crate::session::{Session, SessionEvent};
use crate::theme;
use crate::ui;

type Notice = Result<SharedString, SharedString>;

pub(super) struct ReadAloudView {
    session: Entity<Session>,
    url: Entity<InputState>,
    model: Entity<InputState>,
    voice: Entity<InputState>,
    key: Entity<InputState>,
    stored: Option<SpeechService>,
    notice: Option<Notice>,
    task: Option<Task<()>>,
    sample: Option<Rc<Track>>,
    playback: Option<Task<()>>,
    _changes: Subscription,
}

impl ReadAloudView {
    pub(super) fn new(
        session: Entity<Session>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let url = cx.new(|cx| InputState::new(window, cx));
        let model = cx.new(|cx| InputState::new(window, cx));
        let voice = cx.new(|cx| InputState::new(window, cx));
        let key = cx.new(|cx| InputState::new(window, cx).masked(true));
        let changes = cx.subscribe_in(
            &session,
            window,
            |this, _, event: &SessionEvent, window, cx| match event {
                SessionEvent::Phase => {
                    this.task = None;
                    this.stop();
                    this.notice = None;
                    this.stored = None;
                    this.key.update(cx, |key, cx| key.set_value("", window, cx));
                    this.fill(&SpeechService::default(), window, cx);
                }
                SessionEvent::Changed(changes)
                    if changes.reset || changes.touches(topic::DOCUMENTS) =>
                {
                    cx.notify();
                }
                _ => {}
            },
        );
        Self {
            session,
            url,
            model,
            voice,
            key,
            stored: None,
            notice: None,
            task: None,
            sample: None,
            playback: None,
            _changes: changes,
        }
    }

    fn draft(&self, cx: &App) -> SpeechService {
        let value = |input: &Entity<InputState>| {
            let text = input.read(cx).value();
            let text = text.trim();
            (!text.is_empty()).then(|| text.to_string())
        };
        SpeechService {
            base_url: value(&self.url),
            model: value(&self.model),
            voice: value(&self.voice),
        }
    }

    fn fill(&self, service: &SpeechService, window: &mut Window, cx: &mut App) {
        for (field, value) in [
            (&self.url, &service.base_url),
            (&self.model, &service.model),
            (&self.voice, &service.voice),
        ] {
            let text = value.clone().unwrap_or_default();
            field.update(cx, |input, cx| input.set_value(text, window, cx));
        }
    }

    fn sync(&mut self, window: &mut Window, cx: &mut App) -> bool {
        let Some(current) = self.session.read(cx).client().and_then(|client| {
            let w = client.read();
            let settings = w.documents.preferences.value()?;
            Some(
                settings
                    .preferences
                    .as_ref()
                    .and_then(|p| p.speech.clone())
                    .unwrap_or_default(),
            )
        }) else {
            return false;
        };
        if self.stored.as_ref() != Some(&current) {
            if self.stored.as_ref().is_none_or(|s| *s == self.draft(cx)) {
                self.fill(&current, window, cx);
            }
            self.stored = Some(current);
        }
        true
    }

    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let service = self.draft(cx);
        let change = if service.base_url.is_none() {
            None
        } else {
            match Endpoint::new(&service) {
                Ok(_) => Some(service),
                Err(error) => {
                    self.say(Err(problem(error)), cx);
                    return;
                }
            }
        };
        let Some(client) = self.session.read(cx).client().cloned() else {
            return;
        };
        self.fill(
            change.as_ref().unwrap_or(&SpeechService::default()),
            window,
            cx,
        );
        super::messages::edit_preferences(&client, |prefs| prefs.speech = change);
        self.stored = Some(self.draft(cx));
        self.notice = None;
        cx.notify();
    }

    fn scope(&self, endpoint: &Endpoint, cx: &App) -> Option<String> {
        let session = self.session.read(cx);
        let server = session.server()?.origin();
        let client = session.client()?;
        let w = client.read();
        let account = &w.user.id;
        (!account.is_empty()).then(|| format!("{server}\n{account}\n{}", endpoint.origin()))
    }

    fn change_key(&mut self, clear: bool, window: &mut Window, cx: &mut Context<Self>) {
        let endpoint = match Endpoint::new(&self.draft(cx)) {
            Ok(endpoint) => endpoint,
            Err(error) => {
                self.say(Err(problem(error)), cx);
                return;
            }
        };
        let Some(scope) = self.scope(&endpoint, cx) else {
            return;
        };
        let key = self.key.read(cx).value().trim().to_string();
        if !clear && (key.is_empty() || !speech::valid_key(&key)) {
            self.say(Err(problem(speech::Error::Key)), cx);
            return;
        }
        let session = self.session.read(cx);
        let store = session.credentials.clone();
        let work = session.runtime.spawn_blocking(move || {
            store
                .save_speech_key(&scope, (!clear).then_some(key.as_str()))
                .is_ok()
        });
        self.notice = None;
        self.task = Some(cx.spawn_in(window, async move |this, cx| {
            let ok = work.await.unwrap_or(false);
            let _ = this.update_in(cx, |this, window, cx| {
                this.task = None;
                if ok {
                    this.key
                        .update(cx, |input, cx| input.set_value("", window, cx));
                    let text = if clear {
                        t!("speech-key-removed")
                    } else {
                        t!("speech-key-saved")
                    };
                    this.say(Ok(text.into()), cx);
                } else {
                    this.say(Err(t!("speech-key-unavailable").into()), cx);
                }
            });
        }));
        cx.notify();
    }

    fn play(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let endpoint = match Endpoint::new(&self.draft(cx)) {
            Ok(endpoint) => endpoint,
            Err(error) => {
                self.say(Err(problem(error)), cx);
                return;
            }
        };
        let Some(scope) = self.scope(&endpoint, cx) else {
            return;
        };
        self.stop();
        let key = self.key.read(cx).value().trim().to_string();
        let session = self.session.read(cx);
        let store = session.credentials.clone();
        let runtime = session.runtime.clone();
        let work = runtime.spawn(async move {
            let key = if key.is_empty() {
                tokio::task::spawn_blocking(move || store.speech_key(&scope).ok().flatten())
                    .await
                    .ok()
                    .flatten()
                    .unwrap_or_default()
            } else {
                key
            };
            endpoint.synthesize(t!("speech-sample-text"), &key).await
        });
        self.notice = None;
        self.task = Some(cx.spawn_in(window, async move |this, cx| {
            let audio = work.await.unwrap_or(Err(speech::Error::Network));
            let _ = this.update(cx, |this, cx| {
                this.task = None;
                let result = audio.map_err(problem).and_then(|bytes| {
                    let output = crate::media::sound::output(cx).ok_or(t!("speech-no-output"))?;
                    output
                        .track(bytes)
                        .map_err(|_| t!("speech-invalid-audio").into())
                });
                match result {
                    Ok(sample) => this.play_sample(sample, cx),
                    Err(error) => this.say(Err(error), cx),
                }
            });
        }));
        cx.notify();
    }

    fn play_sample(&mut self, sample: Track, cx: &mut Context<Self>) {
        let sample = Rc::new(sample);
        sample.play();
        self.sample = Some(sample.clone());
        self.playback = Some(cx.spawn(async move |this, cx| {
            while !sample.ended() {
                cx.background_executor()
                    .timer(Duration::from_millis(100))
                    .await;
            }
            let _ = this.update(cx, |this, cx| {
                this.sample = None;
                this.playback = None;
                this.say(Ok(t!("speech-sample-played").into()), cx);
            });
        }));
        self.say(Ok(t!("speech-sample-playing").into()), cx);
    }

    fn stop(&mut self) {
        self.playback = None;
        if let Some(sample) = self.sample.take() {
            sample.pause();
        }
    }

    fn say(&mut self, notice: Notice, cx: &mut Context<Self>) {
        self.notice = Some(notice);
        cx.notify();
    }
}

fn problem(error: speech::Error) -> SharedString {
    match error {
        speech::Error::Address => t!("speech-invalid-address").into(),
        speech::Error::ModelOrVoice => t!("speech-invalid-model-voice").into(),
        speech::Error::Key => t!("speech-invalid-key").into(),
        speech::Error::Input => t!("speech-invalid-input").into(),
        speech::Error::Network => t!("speech-unreachable").into(),
        speech::Error::TooLarge => t!("speech-audio-too-large").into(),
        speech::Error::Http(status) => t!("speech-request-failed", status = status).into(),
    }
}

impl Render for ReadAloudView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme::get(cx).clone();
        let loaded = self.sync(window, cx);
        let disabled = !loaded || self.task.is_some() || self.sample.is_some();
        let fields = [
            ("speech-url", t!("speech-url"), &self.url),
            ("speech-model", t!("speech-model"), &self.model),
            ("speech-voice", t!("speech-voice"), &self.voice),
            ("speech-key", t!("speech-key"), &self.key),
        ];
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .text_sm()
                    .text_color(theme.muted)
                    .child(t!("speech-service-help")),
            )
            .children(fields.map(|(id, label, input)| {
                ui::field::field(
                    label,
                    Input::new(input)
                        .id(id)
                        .aria_label(label)
                        .disabled(disabled),
                    &theme,
                )
            }))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .child(
                        Button::new("speech-save")
                            .label(t!("speech-save"))
                            .disabled(disabled)
                            .on_click(cx.listener(|this, _, window, cx| this.save(window, cx))),
                    )
                    .child(
                        Button::new("speech-save-key")
                            .label(t!("speech-save-key"))
                            .disabled(disabled)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.change_key(false, window, cx)
                            })),
                    )
                    .child(
                        Button::new("speech-remove-key")
                            .ghost()
                            .label(t!("speech-remove-key"))
                            .disabled(disabled)
                            .on_click(
                                cx.listener(|this, _, window, cx| {
                                    this.change_key(true, window, cx)
                                }),
                            ),
                    )
                    .child(
                        Button::new("speech-play-sample")
                            .label(t!("speech-play-sample"))
                            .disabled(disabled)
                            .on_click(cx.listener(|this, _, window, cx| this.play(window, cx))),
                    )
                    .when(self.sample.is_some(), |d| {
                        d.child(
                            Button::new("speech-stop-sample")
                                .ghost()
                                .label(t!("speech-stop-sample"))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.stop();
                                    this.notice = None;
                                    cx.notify();
                                })),
                        )
                    }),
            )
            .when_some(self.notice.clone(), |d, notice| {
                let (text, role, color) = match notice {
                    Ok(text) => (text, Role::Status, theme.muted),
                    Err(text) => (text, Role::Alert, theme.danger),
                };
                d.child(
                    div()
                        .id("speech-status")
                        .test_support()
                        .role(role)
                        .aria_label(text.clone())
                        .text_sm()
                        .text_color(color)
                        .child(text),
                )
            })
    }
}

//! Voice channels: the server's rooms at the end of the sidebar's list, and
//! the call's controls at its foot, as the web client's `VoiceChannels.vue`
//! and `VoiceControls.vue` show them.
//!
//! - Shown when the server offers voice (`capabilities.voice`). The rooms
//!   are read when the workspace arrives and again on every `voice.updated`.
//! - A room lists who is in it; your own call lists the media server's view
//!   instead, with who is speaking. Clicking a room joins it, silent.
//! - The controls open and close the microphone, deafen, open Settings →
//!   Voice and leave.
//!
//! The rules live in `ircord_client::voice::Voice`; this file runs the HTTP
//! calls and the media connection they ask for. The media layer is
//! `ircord_call::LiveKit` unless a test installs another with
//! [`use_connector`]. Device settings are `settings::voice`.

use std::sync::Arc;

use gpui_kit::TestSupportExt;
use gpui_kit::prelude::*;
use gpui_kit::*;
use ircord_client::contract::Room;
use ircord_client::store::topic;
use ircord_client::voice::{
    AudioSettings, Block, Call, CallState, Connect, Connector, Hangup, Joined, MediaEvent, SECTION,
    Voice, VoiceError,
};

use crate::registry::{self, SidebarCtx, Slot};
use crate::session::{Session, SessionEvent};
use crate::theme::{self, Theme};
use crate::ui;
use crate::ui::focus::Reachable;
use crate::ui::icon::{IconName, LucideIcon};

pub fn init(cx: &mut App) {
    registry::register(cx, |r| {
        r.sidebar_list
            .add(Slot::new("voice-channels", |ctx: &SidebarCtx, _, cx| {
                let model = model(&ctx.session, cx);
                model
                    .read(cx)
                    .enabled(cx)
                    .then(|| rooms(&model, cx).into_any_element())
            }));
        // Above the connection indicator.
        r.sidebar_footer.add(
            Slot::new("voice-call", |ctx: &SidebarCtx, _, cx| {
                let model = model(&ctx.session, cx);
                model
                    .read(cx)
                    .voice
                    .active
                    .is_some()
                    .then(|| controls(&model, cx).into_any_element())
            })
            .order(-10)
            .at_bottom(),
        );
    });
}

/// The media layer calls go through.
struct Media(Arc<dyn Connector>);
impl Global for Media {}

/// Use `connector` for calls from now on (tests use a fake).
pub fn use_connector(cx: &mut App, connector: Arc<dyn Connector>) {
    cx.set_global(Media(connector));
}

/// The media layer: LiveKit on the session's runtime unless another was set.
pub(crate) fn connector(session: &Entity<Session>, cx: &mut App) -> Arc<dyn Connector> {
    if let Some(Media(connector)) = cx.try_global::<Media>() {
        return connector.clone();
    }
    let runtime = session.read(cx).runtime.handle().clone();
    let connector: Arc<dyn Connector> = Arc::new(ircord_call::LiveKit::new(runtime));
    cx.set_global(Media(connector.clone()));
    connector
}

// ---- The model ----

/// The one model, kept for the session it serves.
struct Current(Entity<VoiceModel>);
impl Global for Current {}

/// The voice model of `session`.
pub fn model(session: &Entity<Session>, cx: &mut App) -> Entity<VoiceModel> {
    if let Some(Current(model)) = cx.try_global::<Current>()
        && model.read(cx).session == *session
    {
        return model.clone();
    }
    let model = cx.new(|cx| VoiceModel::new(session.clone(), cx));
    cx.set_global(Current(model.clone()));
    model
}

pub struct VoiceModel {
    session: Entity<Session>,
    pub voice: Voice,
    pub settings: AudioSettings,
    call: Option<Box<dyn Call>>,
    /// Reads the open call's events.
    _events: Option<Task<()>>,
    /// Counts room list requests, so only the newest answer is used.
    rooms_asked: u64,
    _subscription: Subscription,
}

impl VoiceModel {
    fn new(session: Entity<Session>, cx: &mut Context<Self>) -> Self {
        let subscription = cx.subscribe(&session, |this, _, event: &SessionEvent, cx| {
            match event {
                SessionEvent::Changed(changes)
                    if changes.reset || changes.touches(topic::VOICE) =>
                {
                    this.load_rooms(cx)
                }
                SessionEvent::Phase => {
                    // Signed out, or signed in again: no call carries over.
                    this.leave(cx);
                    this.voice.rooms.clear();
                    this.load_rooms(cx);
                }
                _ => {}
            }
        });
        let settings = session.read(cx).config.section::<AudioSettings>(SECTION);
        let mut model = Self {
            session,
            voice: Voice::default(),
            settings,
            call: None,
            _events: None,
            rooms_asked: 0,
            _subscription: subscription,
        };
        model.load_rooms(cx);
        model
    }

    fn client(&self, cx: &App) -> Option<std::rc::Rc<ircord_client::Client>> {
        self.session.read(cx).client().cloned()
    }

    /// Whether the server offers voice to this account.
    pub fn enabled(&self, cx: &App) -> bool {
        self.client(cx)
            .is_some_and(|c| c.read().capabilities.voice == Some(true))
    }

    fn load_rooms(&mut self, cx: &mut Context<Self>) {
        let Some(client) = self.client(cx) else {
            return;
        };
        if client.read().capabilities.voice != Some(true) {
            if let Some(hangup) = self.voice.set_rooms(Vec::new()) {
                self.hang_up(hangup, cx);
            }
            return;
        }
        // Answers can overtake each other; only the newest request's counts.
        self.rooms_asked += 1;
        let asked = self.rooms_asked;
        let task = client.request(|api| async move { api.voice_rooms().await });
        cx.spawn(async move |this, cx| {
            let rooms = task.await;
            let _ = this.update(cx, |this, cx| {
                if asked != this.rooms_asked {
                    return;
                }
                match rooms {
                    Ok(Ok(rooms)) => {
                        if let Some(hangup) = this.voice.set_rooms(rooms) {
                            this.hang_up(hangup, cx);
                        }
                    }
                    Ok(Err(e)) => log::warn!("voice rooms: {e}"),
                    Err(e) => log::warn!("voice rooms: {e}"),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Join the room `id`, silent.
    pub fn join(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(room) = self.voice.rooms.iter().find(|r| r.id == id).cloned() else {
            return;
        };
        let Some(client) = self.client(cx) else {
            return;
        };
        let Some(attempt) = self.voice.join(&room) else {
            return;
        };
        cx.notify();
        let id = room.id.clone();
        let task = client.request(move |api| async move { api.join_voice_room(&id).await });
        cx.spawn(async move |this, cx| {
            let answer = match task.await {
                Ok(answer) => answer,
                Err(e) => Err(ircord_client::api::ApiError::Network(e.to_string())),
            };
            let _ = this.update(cx, |this, cx| {
                match this.voice.joined(attempt, answer) {
                    Joined::Connect(credential) => {
                        this.connect(attempt, room.bitrate_kbps, credential, cx)
                    }
                    Joined::Release(credential) => this.release(credential, cx),
                    Joined::Refused => {}
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn connect(
        &mut self,
        attempt: u64,
        bitrate_kbps: i64,
        credential: ircord_client::contract::Credential,
        cx: &mut Context<Self>,
    ) {
        let connector = connector(&self.session, cx);
        let (events, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        let request = Connect {
            url: credential.url,
            token: credential.token,
            bitrate_kbps,
            settings: self.settings.clone(),
        };
        self.call = Some(connector.connect(request, events));
        self._events = Some(cx.spawn(async move |this, cx| {
            while let Some(event) = receiver.recv().await {
                let alive = this.update(cx, |this, cx| this.media(attempt, event, cx));
                if alive.is_err() {
                    return;
                }
            }
            // The media layer went away without a word (its task ended):
            // the call is over. After a hangup this task is dropped first,
            // and a call that already ended ignores it.
            let _ = this.update(cx, |this, cx| this.media(attempt, MediaEvent::Ended, cx));
        }));
    }

    fn media(&mut self, attempt: u64, event: MediaEvent, cx: &mut Context<Self>) {
        if let Some(hangup) = self.voice.media(attempt, event) {
            self.hang_up(hangup, cx);
        }
        cx.notify();
    }

    /// Leave the call, if there is one.
    pub fn leave(&mut self, cx: &mut Context<Self>) {
        if let Some(hangup) = self.voice.leave() {
            self.hang_up(hangup, cx);
            cx.notify();
        }
    }

    fn hang_up(&mut self, hangup: Hangup, cx: &mut Context<Self>) {
        if let Some(call) = self.call.take() {
            call.leave();
        }
        self._events = None;
        if let Some(credential) = hangup.credential {
            self.release(credential, cx);
        }
    }

    /// Give a reservation back; the server ends it on its own if this fails.
    fn release(&self, credential: String, cx: &App) {
        if let Some(client) = self.client(cx) {
            drop(client.request(move |api| async move { api.leave_voice_room(&credential).await }));
        }
    }

    pub fn toggle_mute(&mut self, cx: &mut Context<Self>) {
        if let Some(open) = self.voice.toggle_mute() {
            match &self.call {
                Some(call) => call.set_microphone(open),
                None => self.voice.changing_microphone = false,
            }
            cx.notify();
        }
    }

    pub fn toggle_deafen(&mut self, cx: &mut Context<Self>) {
        if let Some(deafen) = self.voice.toggle_deafen() {
            if let Some(call) = &self.call {
                if deafen.close_microphone {
                    call.set_microphone(false);
                }
                call.set_deafened(deafen.deafened);
            }
            cx.notify();
        }
    }

    /// Use `settings` from now on, in the call too, and keep them on this device.
    pub fn set_settings(&mut self, settings: AudioSettings, cx: &mut Context<Self>) {
        if settings == self.settings {
            return;
        }
        if let Some(call) = &self.call {
            call.apply(&settings);
        }
        if let Err(e) = self
            .session
            .read(cx)
            .config
            .save_section(SECTION, &settings)
        {
            log::warn!("voice settings: {e}");
        }
        self.settings = settings;
        cx.notify();
    }
}

// ---- The rooms ----

fn rooms(model: &Entity<VoiceModel>, cx: &mut App) -> impl IntoElement + use<> {
    let theme = theme::get(cx).clone();
    let m = model.read(cx);
    let voice = &m.voice;
    let mut list = div()
        .id("voice-channels")
        .test_support()
        .role(Role::Group)
        .aria_label(t!("voice-channels"))
        .flex()
        .flex_col()
        .gap_0p5()
        .child(
            div()
                .pt_3()
                .pb_1()
                .px_1()
                .text_sm()
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme.muted)
                .child(t!("voice-channels")),
        );
    for room in &voice.rooms {
        list = list.child(room_row(model, voice, room, &theme));
        let people = if voice.is_active(&room.id) {
            voice
                .participants
                .iter()
                .map(|p| (p.name.clone(), voice.speaking(&p.identity)))
                .collect::<Vec<_>>()
        } else {
            room.participants
                .iter()
                .map(|name| (name.clone(), false))
                .collect()
        };
        if !people.is_empty() {
            list = list.child(roster(&room.id, &room.name, people, &theme));
        }
    }
    if let Some(error) = voice.error.as_ref().filter(|_| voice.active.is_none()) {
        list = list.child(error_line("voice-join-error", error, &theme));
    }
    list
}

fn room_row(
    model: &Entity<VoiceModel>,
    voice: &Voice,
    room: &Room,
    theme: &Theme,
) -> impl IntoElement + use<> {
    let active = voice.is_active(&room.id);
    let block = (!active).then(|| voice.block(room)).flatten();
    let reason = block.map(|b| match b {
        Block::Full => t!("voice-full-reason"),
        Block::Unavailable => t!("voice-unavailable-reason"),
        Block::OtherCall => t!("voice-other-call-reason"),
    });
    let tag = match block {
        Some(Block::Full) => Some(t!("voice-full")),
        Some(Block::Unavailable) => Some(t!("voice-unavailable")),
        _ => None,
    };
    let label: SharedString = if active {
        t!("voice-in-room", room = room.name).into()
    } else {
        t!("voice-join", room = room.name).into()
    };
    let (model, id) = (model.clone(), room.id.clone());
    div()
        .id(ElementId::Name(format!("voice-room-{}", room.id).into()))
        .test_support()
        .role(Role::Button)
        .reachable(theme)
        .aria_label(label)
        .when_some(reason, |d, reason| {
            d.aria_description(reason)
                .tooltip(ui::tooltip::text(reason))
        })
        .when(active, |d| d.aria_selected(true))
        .px_2()
        .py_1()
        .rounded_md()
        .flex()
        .items_center()
        .gap_2()
        .text_sm()
        .text_color(if active { theme.text } else { theme.muted })
        .when(active, |d| {
            d.bg(theme.selected).font_weight(FontWeight::MEDIUM)
        })
        .when(!active && block.is_none(), |d| {
            d.cursor_pointer().hover({
                let (hover, text) = (theme.hover, theme.text);
                move |d| d.bg(hover).text_color(text)
            })
        })
        .child(ui::icon::icon(
            LucideIcon::Volume2,
            if active { theme.online } else { theme.faint },
        ))
        .child(div().flex_1().min_w_0().truncate().child(room.name.clone()))
        .children(tag.map(|tag| {
            div()
                .id(ElementId::Name(
                    format!("voice-room-{}-tag", room.id).into(),
                ))
                .test_support()
                .aria_label(tag)
                .flex_none()
                .text_xs()
                .text_color(theme.faint)
                .child(tag)
        }))
        .on_click(move |_, _, cx| {
            if block.is_none() && !active {
                model.update(cx, |m, cx| m.join(&id, cx));
            }
        })
}

/// Who is in a room, under it.
fn roster(
    room: &str,
    name: &str,
    people: Vec<(String, bool)>,
    theme: &Theme,
) -> impl IntoElement + use<> {
    let mut list = div()
        .id(ElementId::Name(format!("voice-roster-{room}").into()))
        .test_support()
        .role(Role::List)
        .aria_label(SharedString::from(t!("voice-people", room = name)))
        .ml_5()
        .pl_3()
        .border_l_1()
        .border_color(theme.border)
        .flex()
        .flex_col()
        .gap_0p5()
        .py_0p5();
    for (i, (person, speaking)) in people.into_iter().enumerate() {
        let label: SharedString = if speaking {
            t!("voice-speaking", name = person.as_str()).into()
        } else {
            person.clone().into()
        };
        list = list.child(
            div()
                .id(ElementId::Name(format!("voice-person-{room}-{i}").into()))
                .test_support()
                .role(Role::ListItem)
                .aria_label(label)
                .flex()
                .items_center()
                .gap_2()
                .text_xs()
                .text_color(if speaking { theme.text } else { theme.muted })
                .child(
                    div()
                        .rounded_full()
                        .border_1()
                        .border_color(if speaking {
                            theme.online
                        } else {
                            hsla(0., 0., 0., 0.)
                        })
                        .child(ui::avatar::avatar(&person, None, px(18.), theme)),
                )
                .child(div().min_w_0().truncate().child(person)),
        );
    }
    list
}

fn error_text(error: &VoiceError) -> SharedString {
    match error {
        VoiceError::Join(detail) => detail.clone().into(),
        VoiceError::ConnectFailed => t!("voice-connect-failed").into(),
        VoiceError::ConnectionEnded => t!("voice-connection-ended").into(),
        VoiceError::MicrophoneFailed => t!("voice-microphone-failed").into(),
        VoiceError::AudioUnavailable => t!("voice-audio-unavailable").into(),
    }
}

fn error_line(id: &'static str, error: &VoiceError, theme: &Theme) -> impl IntoElement + use<> {
    let text = error_text(error);
    let calm = matches!(error, VoiceError::ConnectionEnded);
    div()
        .id(id)
        .test_support()
        .role(Role::Status)
        .aria_label(text.clone())
        .px_2()
        .py_1()
        .text_xs()
        .text_color(if calm { theme.muted } else { theme.danger })
        .child(text)
}

// ---- The call ----

fn controls(model: &Entity<VoiceModel>, cx: &mut App) -> impl IntoElement + use<> {
    let theme = theme::get(cx).clone();
    let m = model.read(cx);
    let voice = &m.voice;
    let room = voice
        .active
        .as_ref()
        .map(|r| r.name.clone())
        .unwrap_or_default();
    let status: SharedString = match voice.state {
        CallState::Joining | CallState::Idle => t!("voice-joining").into(),
        CallState::Reconnecting => t!("voice-reconnecting").into(),
        CallState::Connected if voice.deafened => t!("voice-deafened-status").into(),
        CallState::Connected => t!("voice-connected").into(),
    };
    let connected = voice.state == CallState::Connected;
    let can_mute = connected && !voice.changing_microphone && !voice.deafened;
    let can_deafen = connected && !voice.changing_microphone;
    let error = voice.error.clone();
    let (muted, deafened) = (voice.muted, voice.deafened);
    let session = m.session.clone();
    let row = div()
        .flex()
        .items_center()
        .justify_between()
        .gap_0p5()
        .child(toggle(
            "voice-mute",
            if muted {
                LucideIcon::MicOff
            } else {
                LucideIcon::Mic
            },
            if muted {
                t!("voice-unmute")
            } else {
                t!("voice-mute")
            },
            muted,
            can_mute,
            &theme,
            {
                let model = model.clone();
                move |cx| model.update(cx, |m, cx| m.toggle_mute(cx))
            },
        ))
        .child(toggle(
            "voice-deafen",
            if deafened {
                LucideIcon::HeadphoneOff
            } else {
                LucideIcon::Headphones
            },
            if deafened {
                t!("voice-undeafen")
            } else {
                t!("voice-deafen")
            },
            deafened,
            can_deafen,
            &theme,
            {
                let model = model.clone();
                move |cx| model.update(cx, |m, cx| m.toggle_deafen(cx))
            },
        ))
        .child(ui::icon::button(
            "voice-settings",
            IconName::Settings,
            t!("voice-settings"),
            &theme,
            move |_, _, cx| crate::settings::open(&session, Some("voice-microphone"), cx),
        ))
        .child(
            div()
                .id("voice-leave")
                .test_support()
                .role(Role::Button)
                .reachable(&theme)
                .aria_label(t!("voice-leave"))
                .tooltip(ui::tooltip::text(t!("voice-leave")))
                .p_1()
                .rounded_md()
                .cursor_pointer()
                .hover({
                    let hover = theme.hover;
                    move |d| d.bg(hover)
                })
                .child(ui::icon::icon(LucideIcon::PhoneOff, theme.danger))
                .on_click({
                    let model = model.clone();
                    move |_, _, cx| model.update(cx, |m, cx| m.leave(cx))
                }),
        );
    div()
        .id("voice-call")
        .test_support()
        .role(Role::Group)
        .aria_label(t!("voice-call-controls"))
        .mx_2()
        .mb_1()
        .p_2()
        .rounded_md()
        .border_1()
        .border_color(theme.border)
        .bg(theme.main)
        .flex()
        .flex_col()
        .gap_1p5()
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(ui::icon::icon(
                    LucideIcon::AudioLines,
                    if connected {
                        theme.online
                    } else {
                        theme.warning
                    },
                ))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .id("voice-status")
                                .test_support()
                                .role(Role::Status)
                                .aria_label(status.clone())
                                .text_xs()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(theme.text)
                                .truncate()
                                .child(status),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.muted)
                                .truncate()
                                .child(room),
                        ),
                ),
        )
        .child(row)
        .children(error.map(|e| error_line("voice-call-error", &e, &theme)))
}

/// A button that is on or off (mute, deafen).
#[allow(clippy::too_many_arguments)]
fn toggle(
    id: &'static str,
    icon: LucideIcon,
    label: &'static str,
    on: bool,
    enabled: bool,
    theme: &Theme,
    run: impl Fn(&mut App) + 'static,
) -> impl IntoElement {
    div()
        .id(id)
        .test_support()
        .role(Role::Button)
        .reachable(theme)
        .aria_label(label)
        .aria_toggled(if on { Toggled::True } else { Toggled::False })
        .tooltip(ui::tooltip::text(label))
        .p_1()
        .rounded_md()
        .when(on, |d| d.border_1().border_color(theme.control_border))
        .when(enabled, |d| {
            d.cursor_pointer().hover({
                let hover = theme.hover;
                move |d| d.bg(hover)
            })
        })
        .child(ui::icon::icon(
            icon,
            if !enabled {
                theme.faint
            } else if on {
                theme.warning
            } else {
                theme.text
            },
        ))
        .on_click(move |_, _, cx| {
            if enabled {
                run(cx)
            }
        })
}

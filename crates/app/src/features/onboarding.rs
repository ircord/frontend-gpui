//! The first-run setup (package S4), as the web client's `WelcomeDialog.vue`:
//! a new account arriving on the network the server provisioned for it
//! (the account's `onboardingNetworkId`) first chooses how it appears there
//! (picture, nickname, display name, real name, about), then picks channels
//! from the server's channel guide when it offers any. Setup completes only
//! after every requested save succeeds; skipping completes it too.
//!
//! It opens when the workspace first shows for such an account.

use crate::ui::focus::Reachable;
use std::collections::HashSet;
use std::rc::Rc;

use gpui_kit::TestSupportExt;
use gpui_kit::component::Disableable;
use gpui_kit::component::Sizable;
use gpui_kit::component::WindowExt;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::*;
use gpui_kit::*;
use ircord_client::api::ApiError;
use ircord_client::api::auth;
use ircord_client::api::channels::{self, GuideSection, SectionKind};
use ircord_client::api::networks;
use ircord_client::commands::NickError;
use ircord_client::commands::run::{nickname_error, nickname_limit};
use ircord_client::contract::{
    ChannelGuideChannelState, ChannelGuideView, ConversationKind, Member, NetworkUpdate,
    ProfileUpdate,
};

use crate::session::Session;
use crate::settings::profile::{self as pictures, AvatarChoice, Picture};
use crate::theme::{self, Theme};
use crate::ui;
use crate::workspace::WorkspaceView;

pub fn init(cx: &mut App) {
    cx.observe_new(
        |view: &mut WorkspaceView, window: Option<&mut Window>, cx: &mut Context<WorkspaceView>| {
            if let Some(window) = window {
                check(view.session().clone(), window, cx);
            }
        },
    )
    .detach();
}

/// The accounts whose setup was opened this run, so a workspace shown
/// again (after a reconnect) does not open it twice.
#[derive(Default)]
struct Offered(HashSet<String>);
impl Global for Offered {}

/// Ask the server whether the account still has its setup to do, and open
/// it if so.
fn check(session: Entity<Session>, window: &mut Window, cx: &mut App) {
    let Some(client) = session.read(cx).client().cloned() else {
        return;
    };
    let task = client
        .handle()
        .request(|api| async move { api.account().await });
    window
        .spawn(cx, async move |cx| {
            let Ok(Ok(Some(account))) = task.await else {
                return;
            };
            let Some(network) = account.onboarding_network_id.clone() else {
                return;
            };
            cx.update(|window, cx| {
                let offered = cx.default_global::<Offered>();
                if !offered.0.insert(account.id.clone()) {
                    return;
                }
                open(&session, &network, window, cx);
            })
            .ok();
        })
        .detach();
}

/// Open the setup for `network`.
pub fn open(session: &Entity<Session>, network: &str, window: &mut Window, cx: &mut App) {
    let view = cx.new(|cx| Onboarding::new(session.clone(), network.to_string(), window, cx));
    let focus = view.read(cx).nick.clone();
    let first = std::cell::Cell::new(true);
    window.open_dialog(cx, move |dialog, window, cx| {
        if first.replace(false) {
            focus.focus_handle(cx).focus(window, cx);
        }
        let title = match view.read(cx).step {
            Step::Profile => t!("profile-welcome-title"),
            Step::Channels => t!("profile-welcome-channels-title"),
        };
        dialog
            .title(title)
            .w(px(480.))
            .close_button(false)
            .overlay_closable(false)
            .keyboard(false)
            // Enter in a field saves the step; it must not close the dialog.
            .on_ok(|_, _, _| false)
            .child(view.clone())
    });
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Step {
    Profile,
    Channels,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Busy {
    Saving,
    Skipping,
    Retrying,
}

/// What finishing the setup came to.
enum Finished {
    /// Some picked channels could not be joined (the first time).
    JoinFailed(Vec<String>, String),
    Done,
}

pub struct Onboarding {
    session: Entity<Session>,
    network: String,
    pub step: Step,
    pub nick: Entity<InputState>,
    pub name: Entity<InputState>,
    pub realname: Entity<InputState>,
    pub bio: Entity<InputState>,
    saved_nick: String,
    saved_realname: String,
    /// Your profile on the network, once read.
    profile: Option<Member>,
    avatar: AvatarChoice,
    guide: Option<Result<ChannelGuideView, String>>,
    picked: Vec<String>,
    join_failed_before: bool,
    busy: Option<Busy>,
    nick_touched: bool,
    error: Option<String>,
    _subscriptions: Vec<Subscription>,
}

impl Onboarding {
    fn new(
        session: Entity<Session>,
        network: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let (saved_nick, saved_realname, profile) = {
            let s = session.read(cx);
            let w = s.client().map(|c| c.read());
            let n = w.as_ref().and_then(|w| w.network(&network));
            (
                n.map(|n| n.nick.clone()).unwrap_or_default(),
                n.and_then(|n| n.realname.clone()).unwrap_or_default(),
                n.and_then(|n| n.profile.clone()),
            )
        };
        let input = |value: &str, window: &mut Window, cx: &mut Context<Self>| {
            let value = value.to_string();
            cx.new(|cx| {
                let mut s = InputState::new(window, cx);
                s.set_value(value, window, cx);
                s
            })
        };
        let nick = input(&saved_nick, window, cx);
        let name = input(
            &profile
                .as_ref()
                .map(pictures::explicit_name)
                .unwrap_or_default(),
            window,
            cx,
        );
        let realname = input(&saved_realname, window, cx);
        let bio = cx.new(|cx| {
            let mut s = InputState::new(window, cx).placeholder(t!("profile-bio-placeholder"));
            s.set_value(
                profile.as_ref().map(|p| p.bio.clone()).unwrap_or_default(),
                window,
                cx,
            );
            s
        });
        let mut subscriptions = Vec::new();
        for field in [&nick, &name, &realname, &bio] {
            subscriptions.push(cx.subscribe_in(
                field,
                window,
                |this: &mut Self, _, event: &InputEvent, window, cx| match event {
                    InputEvent::Change => cx.notify(),
                    InputEvent::PressEnter { .. } => this.save(window, cx),
                    _ => {}
                },
            ));
        }
        let mut view = Self {
            session,
            network,
            step: Step::Profile,
            nick,
            name,
            realname,
            bio,
            saved_nick,
            saved_realname,
            profile,
            avatar: AvatarChoice::Keep,
            guide: None,
            picked: Vec::new(),
            join_failed_before: false,
            busy: None,
            nick_touched: false,
            error: None,
            _subscriptions: subscriptions,
        };
        if view.profile.is_none() {
            view.load_profile(window, cx);
        }
        view.load_guide(window, cx, |_, _, _| {});
        view
    }

    fn client(&self, cx: &App) -> Option<Rc<ircord_client::Client>> {
        self.session.read(cx).client().cloned()
    }

    fn load_profile(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(client) = self.client(cx) else {
            return;
        };
        let network = self.network.clone();
        let task = client
            .handle()
            .request(move |api| async move { api.network_profile(&network).await });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| {
                match result {
                    Ok(Ok(member)) => {
                        let name = pictures::explicit_name(&member);
                        let bio = member.bio.clone();
                        this.name.update(cx, |s, cx| s.set_value(name, window, cx));
                        this.bio.update(cx, |s, cx| s.set_value(bio, window, cx));
                        this.profile = Some(member);
                    }
                    Ok(Err(e)) => this.error = Some(pictures::error_text(&e)),
                    Err(e) => this.error = Some(e.to_string()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Read the channel guide, then run `then`.
    fn load_guide(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        then: impl FnOnce(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) {
        let Some(client) = self.client(cx) else {
            return;
        };
        let network = self.network.clone();
        let task = client
            .handle()
            .request(move |api| async move { api.channel_guide(&network).await });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| {
                let first = this.guide.is_none() || matches!(this.guide, Some(Err(_)));
                this.guide = Some(match result {
                    Ok(Ok(guide)) => Ok(guide),
                    Ok(Err(e)) => Err(pictures::error_text(&e)),
                    Err(e) => Err(e.to_string()),
                });
                if first && this.step == Step::Profile {
                    this.picked = this.preselected(cx);
                }
                then(this, window, cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn joined(&self, cx: &App) -> HashSet<String> {
        self.client(cx)
            .map(|c| channels::joined(&c.read(), &self.network))
            .unwrap_or_default()
    }

    fn preselected(&self, cx: &App) -> Vec<String> {
        match &self.guide {
            Some(Ok(g)) if g.available => channels::initial_selection(&g.entries, &self.joined(cx)),
            _ => Vec::new(),
        }
    }

    fn has_choices(&self, cx: &App) -> bool {
        matches!(&self.guide, Some(Ok(g)) if auth::guide_has_choices(g, &self.joined(cx)))
    }

    fn nick_problem(&self, cx: &App) -> Option<NickError> {
        let max = self
            .client(cx)
            .and_then(|c| {
                c.read()
                    .network(&self.network)
                    .map(|n| nickname_limit(&n.isupport))
            })
            .unwrap_or(32);
        nickname_error(self.nick.read(cx).value().trim(), max)
    }

    /// The profile the form asks to save, if it changed.
    fn profile_update(&self, cx: &App) -> Option<ProfileUpdate> {
        let saved = self.profile.as_ref()?;
        let name = self.name.read(cx).value().trim().to_string();
        let bio = self.bio.read(cx).value().to_string();
        let mut u = ProfileUpdate::default();
        if name != pictures::explicit_name(saved) {
            u.display_name = Some(name);
        }
        if bio != saved.bio {
            u.bio = Some(bio.clone());
        }
        let origin = self
            .session
            .read(cx)
            .server()
            .map(|s| s.origin())
            .unwrap_or_default();
        self.avatar.patch(&origin, &mut u);
        if u == ProfileUpdate::default() {
            return None;
        }
        // The web client always sends the about line with a change.
        u.bio = Some(bio);
        Some(u)
    }

    fn network_update(&self, cx: &App) -> Option<NetworkUpdate> {
        let nick = self.nick.read(cx).value().trim().to_string();
        let realname = self.realname.read(cx).value().trim().to_string();
        let mut u = NetworkUpdate::default();
        if nick != self.saved_nick {
            u.nick = Some(nick);
        }
        if realname != self.saved_realname {
            u.realname = Some(realname);
        }
        (u != NetworkUpdate::default()).then_some(u)
    }

    /// Save the profile step, then go on.
    pub fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy.is_some() || self.step != Step::Profile || self.profile.is_none() {
            return;
        }
        self.nick_touched = true;
        if self.nick_problem(cx).is_some() {
            self.nick.focus_handle(cx).focus(window, cx);
            cx.notify();
            return;
        }
        let Some(client) = self.client(cx) else {
            return;
        };
        let (profile, network_update) = (self.profile_update(cx), self.network_update(cx));
        self.busy = Some(Busy::Saving);
        self.error = None;
        cx.notify();
        let network = self.network.clone();
        let task = client.handle().request(move |api| async move {
            let member = match profile {
                Some(u) => Some(api.update_network_profile(&network, &u).await?),
                None => None,
            };
            let saved = match network_update {
                Some(u) => Some(api.update_network(&network, &u).await?),
                None => None,
            };
            Ok::<_, ApiError>((member, saved))
        });
        let network = self.network.clone();
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| {
                match result {
                    Ok(Ok((member, saved))) => {
                        if let Some(member) = member {
                            client
                                .change(|w| auth::profile_saved(w, Some(&network), member.clone()));
                            this.profile = Some(member);
                            this.avatar = AvatarChoice::Keep;
                        }
                        if let Some(n) = saved {
                            this.saved_nick = n.nick.clone();
                            this.saved_realname = n.realname.clone().unwrap_or_default();
                            client.change(|w| networks::saved(w, n));
                        }
                        this.next(window, cx);
                    }
                    Ok(Err(e)) => {
                        this.busy = None;
                        this.error = Some(pictures::error_text(&e));
                    }
                    Err(e) => {
                        this.busy = None;
                        this.error = Some(e.to_string());
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// After the profile: the channel step, or the end when the guide
    /// offers nothing to pick. A guide that could not be read shows the
    /// channel step with a way to try again.
    fn next(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.guide.is_none() {
            self.load_guide(window, cx, |this, window, cx| this.next(window, cx));
            return;
        }
        if matches!(self.guide, Some(Err(_))) || self.has_choices(cx) {
            self.busy = None;
            self.step = Step::Channels;
            cx.notify();
            return;
        }
        self.finish(Vec::new(), window, cx);
    }

    /// Join `channels`, complete the setup and open the first channel for
    /// everyone.
    fn finish(&mut self, channels: Vec<String>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(client) = self.client(cx) else {
            return;
        };
        if self.busy.is_none() {
            self.busy = Some(Busy::Saving);
        }
        self.error = None;
        cx.notify();
        let network = self.network.clone();
        let retry = !self.join_failed_before;
        let task = client.handle().request(move |api| async move {
            if !channels.is_empty() {
                let result = api.join_from_guide(&network, &channels).await?;
                // The first time some channels cannot be joined, stay on the
                // channel step with them ticked; after that go on without.
                if !result.failed.is_empty() && retry {
                    let names: Vec<String> =
                        result.failed.iter().map(|f| f.channel.clone()).collect();
                    let keys = names.iter().map(|c| channels::guide_key(c)).collect();
                    return Ok(Finished::JoinFailed(keys, names.join(", ")));
                }
            }
            api.complete_onboarding().await?;
            Ok::<_, ApiError>(Finished::Done)
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| {
                this.busy = None;
                match result {
                    Ok(Ok(Finished::Done)) => this.done(window, cx),
                    Ok(Ok(Finished::JoinFailed(keys, names))) => {
                        this.join_failed_before = true;
                        this.step = Step::Channels;
                        this.picked = keys;
                        this.error = Some(t!("profile-welcome-join-failed", channels = names));
                    }
                    Ok(Err(e)) => this.error = Some(pictures::error_text(&e)),
                    Err(e) => this.error = Some(e.to_string()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn done(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        ui::dialog::close(window, cx);
        // Land in the first channel for everyone, if it is there yet.
        let first = match &self.guide {
            Some(Ok(g)) => g
                .entries
                .iter()
                .find(|c| c.state == ChannelGuideChannelState::Everyone)
                .map(|c| channels::guide_key(&c.channel)),
            _ => None,
        };
        let landing = first.and_then(|key| {
            let client = self.client(cx)?;
            let w = client.read();
            w.conversations
                .iter()
                .find(|c| {
                    c.network_id == self.network
                        && c.kind == ConversationKind::Channel
                        && channels::guide_key(
                            &c.target.clone().unwrap_or_else(|| format!("#{}", c.name)),
                        ) == key
                })
                .map(|c| c.id.clone())
        });
        if let Some(id) = landing {
            self.session
                .update(cx, |s, cx| s.select_conversation(&id, cx));
        }
    }

    /// Skip this step: the profile goes on to the channels; the channels
    /// join the ones that started ticked.
    pub fn skip(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy.is_some() {
            return;
        }
        self.busy = Some(Busy::Skipping);
        self.error = None;
        match self.step {
            Step::Profile => self.next(window, cx),
            Step::Channels => {
                let picked = self.preselected(cx);
                self.finish(picked, window, cx)
            }
        }
        cx.notify();
    }

    fn join_picked(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy.is_some() {
            return;
        }
        let picked = self.picked.clone();
        self.finish(picked, window, cx);
    }

    fn retry_guide(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.busy = Some(Busy::Retrying);
        cx.notify();
        self.load_guide(window, cx, |this, _, cx| {
            this.busy = None;
            this.picked = this.preselected(cx);
        });
    }

    fn render_profile(&mut self, cx: &mut Context<Self>, theme: &Theme) -> AnyElement {
        let entity = cx.entity();
        let busy = self.busy.is_some();
        let network_name = self
            .client(cx)
            .and_then(|c| c.read().network(&self.network).map(|n| n.name.clone()))
            .unwrap_or_else(|| "IRCord".into());
        let avatars = pictures::default_avatars(&self.session, cx);
        let session = self.session.read(cx);
        let saved_url = self.profile.as_ref().and_then(|p| p.avatar_url.clone());
        let pic = pictures::preview(&self.avatar, saved_url.as_deref(), session);
        let origin = session.server().map(|s| s.origin()).unwrap_or_default();
        let nick_now = self.nick.read(cx).value().trim().to_string();
        let name_now = {
            let n = self.name.read(cx).value().trim().to_string();
            if n.is_empty() {
                let real = self.realname.read(cx).value().trim().to_string();
                if !real.is_empty() && real != "IRCord" {
                    real
                } else {
                    nick_now.clone()
                }
            } else {
                n
            }
        };
        let suggestion = (self.avatar == AvatarChoice::Keep
            && saved_url.as_deref().is_none_or(str::is_empty))
        .then(|| {
            let username = if nick_now.is_empty() {
                self.saved_nick.clone()
            } else {
                nick_now.clone()
            };
            auth::suggest_avatar(&username, &avatars).map(|a| a.id.clone())
        })
        .flatten();
        let selected = match &self.avatar {
            AvatarChoice::Default(a) => Some(a.id.clone()),
            AvatarChoice::Keep => auth::default_avatar_for(saved_url.as_deref(), &avatars, &origin)
                .map(|a| a.id.clone()),
            _ => None,
        };
        let e = entity.clone();
        let picker = (!avatars.is_empty()).then(|| {
            pictures::default_picker(
                "onboarding-default",
                &avatars,
                pictures::Marks {
                    selected: selected.as_deref(),
                    suggested: suggestion.as_deref(),
                    disabled: busy,
                },
                session,
                theme,
                move |avatar, _, cx| {
                    let avatar = avatar.clone();
                    e.update(cx, |v, cx| {
                        v.avatar = AvatarChoice::Default(avatar);
                        v.error = None;
                        cx.notify();
                    })
                },
            )
        });
        let has_picture = !matches!(pic, Picture::None);
        let e = entity.clone();
        let upload = Button::new("onboarding-upload")
            .small()
            .label(t!("profile-upload-own"))
            .disabled(busy)
            .on_click(move |_, window, cx| {
                let e = e.clone();
                pictures::pick_picture(window, cx, move |prepared, _, cx| {
                    e.update(cx, |v, cx| {
                        match prepared {
                            Ok(p) => v.avatar = AvatarChoice::Upload(p),
                            Err(err) => v.error = Some(err),
                        }
                        cx.notify();
                    })
                });
            });
        let e = entity.clone();
        let remove = Button::new("onboarding-remove-picture")
            .small()
            .ghost()
            .label(t!("profile-remove"))
            .disabled(busy)
            .on_click(move |_, _, cx| {
                e.update(cx, |v, cx| {
                    let saved = v
                        .profile
                        .as_ref()
                        .and_then(|p| p.avatar_url.clone())
                        .is_some_and(|u| !u.is_empty());
                    v.avatar = if saved {
                        AvatarChoice::Remove
                    } else {
                        AvatarChoice::Keep
                    };
                    cx.notify();
                })
            });
        let nick_problem = (self.nick_touched || !nick_now.is_empty())
            .then(|| self.nick_problem(cx))
            .flatten();
        let nick_help: SharedString = match nick_problem {
            Some(p) => nick_text(p).into(),
            None => t!("profile-welcome-nick-help", network = network_name.clone()).into(),
        };
        let field = |label: SharedString,
                     id: &'static str,
                     input: &Entity<InputState>,
                     help: SharedString,
                     danger: bool| {
            ui::field::field(
                label.clone(),
                Input::new(input).id(id).aria_label(label),
                theme,
            )
            .child(
                div()
                    .id(ElementId::Name(format!("{id}-help").into()))
                    .test_support()
                    .role(if danger { Role::Alert } else { Role::Note })
                    .aria_label(help.clone())
                    .text_xs()
                    .text_color(if danger { theme.danger } else { theme.muted })
                    .child(help),
            )
        };
        let has_choices = self.has_choices(cx);
        let e = entity.clone();
        let skip = Button::new("onboarding-skip")
            .ghost()
            .label(if self.busy == Some(Busy::Skipping) {
                t!("profile-welcome-skipping")
            } else {
                t!("profile-welcome-skip")
            })
            .disabled(busy)
            .on_click(move |_, window, cx| e.update(cx, |v, cx| v.skip(window, cx)));
        let e = entity.clone();
        let save = Button::new("onboarding-save")
            .primary()
            .label(if self.busy == Some(Busy::Saving) {
                t!("profile-saving")
            } else if has_choices {
                t!("profile-welcome-next")
            } else {
                t!("profile-welcome-save")
            })
            .disabled(busy || self.profile.is_none())
            .on_click(move |_, window, cx| e.update(cx, |v, cx| v.save(window, cx)));
        div()
            .id("onboarding-profile")
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .text_sm()
                    .text_color(theme.muted)
                    .child(t!("profile-welcome-intro", network = network_name.clone())),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(pictures::picture(&pic, &name_now, px(56.), theme))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .min_w_0()
                            .child(
                                div()
                                    .id("onboarding-preview-name")
                                    .test_support()
                                    .role(Role::Status)
                                    .aria_label(SharedString::from(name_now.clone()))
                                    .flex()
                                    .gap_2()
                                    .child(
                                        div()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(theme.text)
                                            .child(name_now.clone()),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(theme.muted)
                                            .child(format!("@{nick_now}")),
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .gap_2()
                                    .child(upload)
                                    .when(has_picture, |d| d.child(remove)),
                            ),
                    ),
            )
            .children(picker)
            .child(field(
                t!("profile-welcome-nick").into(),
                "onboarding-nick",
                &self.nick,
                nick_help,
                nick_problem.is_some(),
            ))
            .child(field(
                t!("profile-display-name").into(),
                "onboarding-name",
                &self.name,
                t!("profile-welcome-name-help").into(),
                false,
            ))
            .child(field(
                t!("profile-welcome-realname").into(),
                "onboarding-realname",
                &self.realname,
                t!("profile-welcome-realname-help", network = network_name).into(),
                false,
            ))
            .child(ui::field::field(
                t!("profile-about-you"),
                Input::new(&self.bio)
                    .id("onboarding-bio")
                    .aria_label(t!("profile-about-you")),
                theme,
            ))
            .when_some(self.error.clone(), |d, e| d.child(error(e, theme)))
            .child(div().flex().justify_end().gap_2().child(skip).child(save))
            .into_any_element()
    }

    fn render_channels(&mut self, cx: &mut Context<Self>, theme: &Theme) -> AnyElement {
        let entity = cx.entity();
        let busy = self.busy.is_some();
        let network_name = self
            .client(cx)
            .and_then(|c| c.read().network(&self.network).map(|n| n.name.clone()))
            .unwrap_or_else(|| "IRCord".into());
        let joined = self.joined(cx);
        let mut body = div()
            .id("onboarding-channels")
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .text_sm()
                    .text_color(theme.muted)
                    .child(t!("profile-welcome-channels-intro", network = network_name)),
            );
        match &self.guide {
            Some(Err(_)) => {
                let e = entity.clone();
                body = body.child(
                    div()
                        .id("onboarding-guide-failed")
                        .test_support()
                        .role(Role::Alert)
                        .aria_label(t!("profile-welcome-guide-failed"))
                        .flex()
                        .flex_col()
                        .gap_1()
                        .text_sm()
                        .text_color(theme.danger)
                        .child(t!("profile-welcome-guide-failed"))
                        .child(
                            Button::new("onboarding-retry-guide")
                                .small()
                                .label(t!("profile-welcome-retry-guide"))
                                .disabled(busy)
                                .on_click(move |_, window, cx| {
                                    e.update(cx, |v, cx| v.retry_guide(window, cx))
                                }),
                        ),
                );
            }
            Some(Ok(guide)) => {
                let mut list = div()
                    .id("onboarding-channel-list")
                    .flex()
                    .flex_col()
                    .gap_2()
                    .max_h(px(360.))
                    .overflow_y_scroll();
                for section in channels::guide_sections(&guide.groups, &guide.entries) {
                    list = list.child(self.section(&section, &joined, busy, &entity, theme));
                }
                body = body.child(list);
            }
            None => {
                body = body.child(
                    div()
                        .text_sm()
                        .text_color(theme.muted)
                        .child(t!("profile-loading")),
                )
            }
        }
        let count = self.picked.len();
        let e = entity.clone();
        let skip = Button::new("onboarding-skip")
            .ghost()
            .label(if self.busy == Some(Busy::Skipping) {
                t!("profile-welcome-skipping")
            } else {
                t!("profile-welcome-skip-channels")
            })
            .disabled(busy)
            .on_click(move |_, window, cx| e.update(cx, |v, cx| v.skip(window, cx)));
        let e = entity.clone();
        let join = Button::new("onboarding-join")
            .primary()
            .label(if self.busy == Some(Busy::Saving) {
                t!("profile-welcome-joining").to_string()
            } else if count == 1 {
                t!("profile-welcome-join-one").to_string()
            } else if count > 1 {
                t!("profile-welcome-join-count", count = count)
            } else {
                t!("profile-welcome-continue").to_string()
            })
            .disabled(busy)
            .on_click(move |_, window, cx| e.update(cx, |v, cx| v.join_picked(window, cx)));
        body.when_some(self.error.clone(), |d, e| d.child(error(e, theme)))
            .child(div().flex().justify_end().gap_2().child(skip).child(join))
            .into_any_element()
    }

    fn section(
        &self,
        section: &GuideSection,
        joined: &HashSet<String>,
        busy: bool,
        entity: &Entity<Self>,
        theme: &Theme,
    ) -> AnyElement {
        let heading: SharedString = match &section.kind {
            SectionKind::Everyone => t!("profile-welcome-for-everyone").into(),
            SectionKind::Group(name) => name.clone().into(),
            SectionKind::More => t!("profile-welcome-more-channels").into(),
        };
        let mut group = div()
            .id(ElementId::Name(
                format!("onboarding-section-{}", heading.to_lowercase()).into(),
            ))
            .role(Role::Group)
            .aria_label(heading.clone())
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::BOLD)
                    .text_color(theme.muted)
                    .child(heading.to_uppercase()),
            );
        for channel in &section.channels {
            let key = channels::guide_key(&channel.channel);
            let fixed =
                channel.state == ChannelGuideChannelState::Everyone || joined.contains(&key);
            let on = fixed || self.picked.contains(&key);
            let id = format!("onboarding-channel-{}", key.trim_start_matches('#'));
            let about = channel
                .description
                .clone()
                .filter(|d| !d.is_empty())
                .or_else(|| channel.topic.clone())
                .unwrap_or_default();
            let e = entity.clone();
            let toggled = key.clone();
            let hover = theme.hover;
            group = group.child(
                div()
                    .id(ElementId::Name(id.into()))
                    .test_support()
                    .role(Role::CheckBox)
                    .reachable(theme)
                    .aria_label(SharedString::from(channel.channel.clone()))
                    .aria_toggled(if on { Toggled::True } else { Toggled::False })
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .when(!fixed && !busy, |d| {
                        d.cursor_pointer()
                            .hover(move |d| d.bg(hover))
                            .on_click(move |_, _, cx| {
                                e.update(cx, |v, cx| {
                                    if let Some(i) = v.picked.iter().position(|k| *k == toggled) {
                                        v.picked.remove(i);
                                    } else {
                                        v.picked.push(toggled.clone());
                                    }
                                    cx.notify();
                                })
                            })
                    })
                    .child(
                        div()
                            .size(px(14.))
                            .flex_none()
                            .rounded_sm()
                            .border_1()
                            .border_color(theme.muted)
                            .when(on, |d| d.bg(theme.accent).border_color(theme.accent)),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .min_w_0()
                            .flex_1()
                            .child(
                                div()
                                    .flex()
                                    .gap_2()
                                    .child(
                                        div().text_color(theme.text).child(channel.channel.clone()),
                                    )
                                    .when(
                                        channel.state == ChannelGuideChannelState::Everyone,
                                        |d| d.child(tag(t!("profile-welcome-everyone"), theme)),
                                    )
                                    .when(channel.nsfw, |d| {
                                        d.child(tag(t!("profile-welcome-nsfw"), theme))
                                    }),
                            )
                            .when(!about.is_empty(), |d| {
                                d.child(
                                    div()
                                        .text_xs()
                                        .text_color(theme.muted)
                                        .truncate()
                                        .child(about),
                                )
                            }),
                    ),
            );
        }
        group.into_any_element()
    }
}

fn tag(text: &str, theme: &Theme) -> Div {
    div()
        .px_1()
        .rounded_sm()
        .text_xs()
        .bg(theme.input)
        .text_color(theme.muted)
        .child(text.to_string())
}

fn error(text: String, theme: &Theme) -> impl IntoElement {
    div()
        .id("onboarding-error")
        .test_support()
        .role(Role::Alert)
        .aria_label(SharedString::from(text.clone()))
        .text_sm()
        .text_color(theme.danger)
        .child(text)
}

/// Why a nickname cannot be used.
pub fn nick_text(problem: NickError) -> String {
    match problem {
        NickError::Required => t!("profile-nick-required").into(),
        NickError::Spaces => t!("profile-nick-spaces").into(),
        NickError::Syntax => t!("profile-nick-syntax").into(),
        NickError::TooLong(max) => t!("profile-nick-too-long", maximum = max),
    }
}

impl Render for Onboarding {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme::get(cx).clone();
        match self.step {
            Step::Profile => self.render_profile(cx, &theme),
            Step::Channels => self.render_channels(cx, &theme),
        }
    }
}

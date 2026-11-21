//! Channel settings (package CH2), as the web client's `ChannelSettings.vue`,
//! `ChannelOverview.vue`, `ChannelModes.vue` and `ChannelGuideListing.vue`
//! draw them: a dialog with Overview (the channel at a glance, the topic,
//! what everyone is notified about, who may use group mentions, NSFW, and
//! leaving, which asks first), Modes (the switches and valued modes the
//! network advertises), Registration (`registration.rs`), the channel guide
//! request on this server's own network, and one tab per list mode
//! (`mode_lists.rs`).
//!
//! It opens from the conversation menu, `/settings` and `/banlist`. Topics
//! and modes go out as IRC lines built in `ircord_client::channel_modes`;
//! what you may change follows your rank, and what you may not is shown
//! read-only with the reason.

use std::collections::HashMap;
use std::rc::Rc;

use gpui_kit::TestSupportExt;
use gpui_kit::component::Disableable;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::switch::Switch;
use gpui_kit::prelude::*;
use gpui_kit::*;
use ircord_client::Client;
use ircord_client::channel_modes::{self as modes, Chanmodes, Rank};
use ircord_client::commands::View;
use ircord_client::contract::{self as c, ConversationKind, NetworkStatus};
use ircord_client::features::Context as Allowed;
use ircord_client::format;
use ircord_client::irc::{ChannelRights, Prefixes, channel_rights_for, channel_target};
use ircord_client::store::{Changes, Connection, server_log};

use crate::features::commands;
use crate::features::create_channel::mode_text;
use crate::features::mode_lists::ModeListView;
use crate::features::registration::RegistrationView;
use crate::registry::{self, ConversationCtx, MenuItem};
use crate::session::{Session, SessionEvent};
use crate::theme::Theme;
use crate::ui::icon::IconName;
use crate::{theme, ui};

pub fn init(cx: &mut App) {
    commands::on_view(cx, View::ChannelSettings, |request, window, cx| {
        open(
            &request.session,
            &request.conversation,
            Tab::Overview,
            window,
            cx,
        )
    });
    commands::on_view(cx, View::BanList, |request, window, cx| {
        open(
            &request.session,
            &request.conversation,
            Tab::List('b'),
            window,
            cx,
        )
    });
    registry::register(cx, |r| {
        let m = &mut r.conversation_menu;
        m.add(
            MenuItem::new(
                "channel-settings-open",
                t!("channel-settings-menu-open"),
                |ctx: &ConversationCtx, window, cx| {
                    open(&ctx.session, &ctx.conversation, Tab::Overview, window, cx)
                },
            )
            .icon(IconName::Settings)
            .order(400)
            .when(|ctx: &ConversationCtx, cx| here(&ctx.session, &ctx.conversation, cx).is_some()),
        );
        m.add(
            MenuItem::new(
                "channel-settings-lists",
                t!("channel-settings-menu-lists"),
                |ctx: &ConversationCtx, window, cx| {
                    open(&ctx.session, &ctx.conversation, Tab::List('b'), window, cx)
                },
            )
            .order(401)
            .when(|ctx: &ConversationCtx, cx| {
                here(&ctx.session, &ctx.conversation, cx).is_some_and(|h| h.rights.ban)
            }),
        );
        m.add(
            MenuItem::new(
                "channel-settings-edit-topic",
                t!("channel-settings-menu-topic"),
                |ctx: &ConversationCtx, window, cx| edit_topic(ctx, window, cx),
            )
            .order(402)
            .when(|ctx: &ConversationCtx, cx| {
                here(&ctx.session, &ctx.conversation, cx).is_some_and(|h| h.rights.topic)
            }),
        );
    });
}

// ---- What the settings need to know about the channel ----

/// A channel as the settings see it.
pub(crate) struct Here {
    pub conversation: c::Conversation,
    pub network: Option<c::Network>,
    pub rights: ChannelRights,
    /// Your highest prefix there.
    pub prefix: Option<char>,
    /// The socket is live and the network connected.
    pub connected: bool,
}

impl Here {
    pub fn target(&self) -> String {
        channel_target(&self.conversation)
    }

    pub fn isupport(&self) -> std::collections::BTreeMap<String, String> {
        self.network
            .as_ref()
            .map(|n| n.isupport.clone())
            .unwrap_or_default()
    }

    pub fn network_id(&self) -> &str {
        &self.conversation.network_id
    }

    pub fn network_name(&self) -> String {
        self.network
            .as_ref()
            .map(|n| n.name.clone())
            .unwrap_or_default()
    }
}

/// The channel `conversation`, if it is a channel (not a group).
pub(crate) fn here(session: &Entity<Session>, conversation: &str, cx: &App) -> Option<Here> {
    let client = session.read(cx).client()?;
    let w = client.read();
    let live = w.connection == Some(Connection::Live);
    let allowed = Allowed::of(&w, conversation, live);
    let c = allowed.conversation?;
    if c.kind != ConversationKind::Channel || c.party.is_some() {
        return None;
    }
    let own = allowed.own_modes();
    let rights = channel_rights_for(own, Some(c), allowed.network);
    let prefixes = allowed.network.map_or_else(
        || Prefixes::of(&Default::default()),
        |n| Prefixes::of(&n.isupport),
    );
    let prefix = prefixes.prefixes.iter().copied().find(|p| own.contains(*p));
    Some(Here {
        conversation: c.clone(),
        network: allowed.network.cloned(),
        rights,
        prefix,
        connected: live
            && allowed
                .network
                .is_some_and(|n| n.status == NetworkStatus::Connected),
    })
}

/// Send one IRC line to the channel's network; say why when it cannot go.
/// `false` when it was not sent (not connected).
pub(crate) fn send(
    session: &Entity<Session>,
    conversation: &str,
    line: String,
    window: &mut Window,
    cx: &mut App,
) -> bool {
    let Some(h) = here(session, conversation, cx) else {
        return false;
    };
    let Some(client) = session.read(cx).client().cloned() else {
        return false;
    };
    if !h.connected {
        ui::toast::info(window, cx, t!("channel-settings-not-connected"));
        return false;
    }
    let task = server_log::send_raw(client.handle(), h.network_id(), line);
    window
        .spawn(cx, async move |acx| {
            let result = task
                .await
                .map_err(|e| e.to_string())
                .and_then(|r| r.map_err(|e| e.to_string()));
            if let Err(e) = result {
                let _ = acx.update(|window, cx| ui::toast::error(window, cx, e));
            }
        })
        .detach();
    true
}

/// An ISO time as a day, in this computer's time zone.
pub(crate) fn date(iso: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(iso)
        .map(|d| {
            d.with_timezone(&chrono::Local)
                .format("%-d %b %Y")
                .to_string()
        })
        .unwrap_or_default()
}

/// Put the server's answer (a changed conversation) into the workspace.
pub(crate) fn put(
    w: &mut ircord_client::store::Workspace,
    conversation: c::Conversation,
) -> Changes {
    match w.conversations.iter_mut().find(|x| x.id == conversation.id) {
        Some(existing) => *existing = conversation,
        None => w.conversations.push(conversation),
    }
    Changes {
        sidebar: true,
        members: true,
        ..Default::default()
    }
}

/// A short heading over a group of settings.
pub(crate) fn heading(text: impl Into<SharedString>, theme: &Theme) -> Div {
    let text: SharedString = text.into();
    div()
        .text_xs()
        .font_weight(FontWeight::BOLD)
        .text_color(theme.faint)
        .child(text.to_uppercase())
}

/// A line of help under a control.
pub(crate) fn hint(text: impl Into<SharedString>, theme: &Theme) -> Div {
    let text: SharedString = text.into();
    div().text_xs().text_color(theme.muted).child(text)
}

/// A row of choices, one picked (a segmented control).
pub(crate) fn choices<T: Copy + PartialEq + 'static>(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    options: Vec<(T, SharedString)>,
    picked: T,
    enabled: bool,
    on_pick: impl Fn(T, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let on_pick = Rc::new(on_pick);
    let id: SharedString = id.into();
    let mut row = div()
        .id(ElementId::Name(id.clone()))
        .role(Role::RadioGroup)
        .aria_label(label.into())
        .flex()
        .flex_wrap()
        .gap_1();
    for (i, (value, text)) in options.into_iter().enumerate() {
        let on_pick = on_pick.clone();
        let button = Button::new(ElementId::Name(format!("{id}-{i}").into()))
            .label(text.clone())
            .accessibility_label(text)
            .disabled(!enabled)
            .on_click(move |_, window, cx| on_pick(value, window, cx));
        row = row.child(if value == picked {
            button.primary()
        } else {
            button.ghost()
        });
    }
    row
}

// ---- The dialog ----

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    Overview,
    Modes,
    Registration,
    Guide,
    /// A list mode's entries (`b`, `e`, `I`, `q`…).
    List(char),
}

/// Open the settings of `conversation` on `tab`.
pub fn open(
    session: &Entity<Session>,
    conversation: &str,
    tab: Tab,
    window: &mut Window,
    cx: &mut App,
) {
    let Some(h) = here(session, conversation, cx) else {
        return ui::toast::info(window, cx, t!("channel-settings-channels-only"));
    };
    let view = cx.new(|cx| {
        ChannelSettingsView::new(session.clone(), conversation.to_string(), tab, window, cx)
    });
    ui::dialog::open(
        window,
        cx,
        t!("channel-settings-title", channel = h.target()),
        move |dialog, _, _| {
            dialog
                .w(px(820.))
                // Enter in a field submits that field; it must not close the dialog.
                .on_ok(|_, _, _| false)
                .child(view.clone())
        },
    );
}

pub struct ChannelSettingsView {
    session: Entity<Session>,
    conversation: String,
    tab: Tab,
    topic: Entity<InputState>,
    /// The topic as the server last had it, to tell edits from updates.
    topic_seen: String,
    values: HashMap<char, Entity<InputState>>,
    guide: Option<c::ChannelGuideListing>,
    guide_group: Entity<InputState>,
    guide_description: Entity<InputState>,
    guide_busy: bool,
    guide_error: Option<SharedString>,
    lists: HashMap<char, Entity<ModeListView>>,
    registration: Option<Entity<RegistrationView>>,
    _subscriptions: Vec<Subscription>,
}

impl ChannelSettingsView {
    fn new(
        session: Entity<Session>,
        conversation: String,
        tab: Tab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let h = here(&session, &conversation, cx);
        let topic_seen = h
            .as_ref()
            .map(|h| format::strip(&h.conversation.topic))
            .unwrap_or_default();
        let topic = cx.new(|cx| {
            let mut state =
                InputState::new(window, cx).placeholder(t!("channel-settings-topic-none"));
            state.set_value(topic_seen.clone(), window, cx);
            state
        });
        let line = |placeholder: &'static str, window: &mut Window, cx: &mut Context<Self>| {
            cx.new(|cx| InputState::new(window, cx).placeholder(placeholder))
        };
        let guide_group = line(t!("channel-settings-guide-group-placeholder"), window, cx);
        let guide_description = line(
            t!("channel-settings-guide-description-placeholder"),
            window,
            cx,
        );
        let mut subscriptions = vec![
            cx.subscribe_in(
                &session,
                window,
                |this: &mut Self, _, event: &SessionEvent, window, cx| {
                    if let SessionEvent::Changed(_) = event {
                        this.refresh_topic(window, cx);
                        cx.notify();
                    }
                },
            ),
            cx.subscribe_in(
                &topic,
                window,
                |this: &mut Self, _, event: &InputEvent, window, cx| match event {
                    InputEvent::Change => cx.notify(),
                    InputEvent::PressEnter { .. } => this.save_topic(window, cx),
                    _ => {}
                },
            ),
        ];
        let mut values = HashMap::new();
        if let Some(h) = &h {
            let chanmodes = Chanmodes::parse(&h.isupport());
            for mode in modes::value_modes(&chanmodes) {
                let input = cx.new(|cx| {
                    let mut state = InputState::new(window, cx);
                    // The key is never shown; the others start with their value.
                    if mode != 'k'
                        && let Some(v) = h
                            .conversation
                            .mode_params
                            .as_ref()
                            .and_then(|p| p.get(&mode.to_string()))
                    {
                        state.set_value(v.clone(), window, cx);
                    }
                    state
                });
                subscriptions.push(cx.subscribe_in(
                    &input,
                    window,
                    move |this: &mut Self, _, event: &InputEvent, window, cx| match event {
                        InputEvent::Change => cx.notify(),
                        InputEvent::PressEnter { .. } => this.set_value(mode, window, cx),
                        _ => {}
                    },
                ));
                values.insert(mode, input);
            }
        }
        let mut view = Self {
            session,
            conversation,
            tab: Tab::Overview,
            topic,
            topic_seen,
            values,
            guide: None,
            guide_group,
            guide_description,
            guide_busy: false,
            guide_error: None,
            lists: HashMap::new(),
            registration: None,
            _subscriptions: subscriptions,
        };
        view.load_guide(window, cx);
        view.set_tab(tab, window, cx);
        view
    }

    fn client(&self, cx: &App) -> Option<Rc<Client>> {
        self.session.read(cx).client().cloned()
    }

    fn here(&self, cx: &App) -> Option<Here> {
        here(&self.session, &self.conversation, cx)
    }

    fn set_tab(&mut self, tab: Tab, window: &mut Window, cx: &mut Context<Self>) {
        self.tab = tab;
        match tab {
            Tab::List(mode) if !self.lists.contains_key(&mode) => {
                let (session, conversation) = (self.session.clone(), self.conversation.clone());
                let view = cx.new(|cx| ModeListView::new(session, conversation, mode, window, cx));
                self.lists.insert(mode, view);
            }
            Tab::Registration if self.registration.is_none() => {
                let (session, conversation) = (self.session.clone(), self.conversation.clone());
                self.registration =
                    Some(cx.new(|cx| RegistrationView::new(session, conversation, window, cx)));
            }
            _ => {}
        }
        cx.notify();
    }

    /// The server's topic changed: show it, unless the box holds an edit.
    fn refresh_topic(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(h) = self.here(cx) else {
            return;
        };
        let now = format::strip(&h.conversation.topic);
        if now == self.topic_seen {
            return;
        }
        let typed = self.topic.read(cx).value().to_string();
        if typed == self.topic_seen {
            self.topic
                .update(cx, |s, cx| s.set_value(now.clone(), window, cx));
        }
        self.topic_seen = now;
    }

    fn save_topic(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(h) = self.here(cx) else {
            return;
        };
        let typed = self.topic.read(cx).value().to_string();
        if !h.rights.topic || typed == self.topic_seen {
            return;
        }
        send(
            &self.session,
            &self.conversation,
            modes::topic_line(&h.target(), &typed),
            window,
            cx,
        );
    }

    fn toggle(&mut self, mode: char, on: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(h) = self.here(cx) else {
            return;
        };
        send(
            &self.session,
            &self.conversation,
            modes::toggle_line(&h.target(), mode, on),
            window,
            cx,
        );
    }

    fn set_value(&mut self, mode: char, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(h), Some(input)) = (self.here(cx), self.values.get(&mode)) else {
            return;
        };
        let value = input.read(cx).value().to_string();
        if let Some(line) = modes::set_value_line(&h.target(), mode, &value) {
            send(&self.session, &self.conversation, line, window, cx);
        }
    }

    fn clear_value(&mut self, mode: char, window: &mut Window, cx: &mut Context<Self>) {
        let Some(h) = self.here(cx) else {
            return;
        };
        let chanmodes = Chanmodes::parse(&h.isupport());
        let current = h
            .conversation
            .mode_params
            .as_ref()
            .and_then(|p| p.get(&mode.to_string()))
            .cloned();
        let line = modes::clear_value_line(&h.target(), mode, &chanmodes, current.as_deref());
        send(&self.session, &self.conversation, line, window, cx);
    }

    fn change_shared(&mut self, body: c::ChannelSettingsPatchBody, cx: &mut Context<Self>) {
        let Some(client) = self.client(cx) else {
            return;
        };
        let conversation = self.conversation.clone();
        client.call(
            move |api| async move { api.channel_settings(&conversation, &body).await },
            |w, result| match result {
                Ok(c) => put(w, c),
                Err(e) => Changes {
                    notices: vec![e.to_string()],
                    ..Default::default()
                },
            },
        );
    }

    fn leave(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(h) = self.here(cx) else {
            return;
        };
        let (session, conversation) = (self.session.clone(), self.conversation.clone());
        ui::confirm::confirm(
            window,
            cx,
            ui::confirm::Confirm {
                title: t!("channel-settings-leave-title", channel = h.target()).into(),
                body: t!("channel-settings-leave-body").into(),
                ok: t!("channel-settings-leave").into(),
                danger: true,
            },
            move |window, cx| {
                crate::sidebar::menus::remove_conversation(&session, &conversation, cx);
                // The confirmation has closed; close the settings under it too.
                ui::dialog::close(window, cx);
            },
        );
    }

    // ---- The channel guide ----

    fn load_guide(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(client) = self.client(cx) else {
            return;
        };
        let conversation = self.conversation.clone();
        let task = client.request(move |api| async move { api.guide_listing(&conversation).await });
        cx.spawn_in(window, async move |this, cx| {
            // Without the listing the tab stays hidden; the rest still works.
            if let Ok(Ok(listing)) = task.await {
                this.update_in(cx, |this, window, cx| {
                    this.guide_loaded(listing, window, cx)
                })
                .ok();
            }
        })
        .detach();
    }

    fn guide_loaded(
        &mut self,
        listing: c::ChannelGuideListing,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let group = listing.group.clone().unwrap_or_default();
        let description = listing.description.clone().unwrap_or_default();
        self.guide_group
            .update(cx, |s, cx| s.set_value(group, window, cx));
        self.guide_description
            .update(cx, |s, cx| s.set_value(description, window, cx));
        self.guide = Some(listing);
        cx.notify();
    }

    fn guide_action(&mut self, withdraw: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(client) = self.client(cx) else {
            return;
        };
        if self.guide_busy {
            return;
        }
        let conversation = self.conversation.clone();
        let text = |s: &Entity<InputState>, cx: &App| {
            let v = s.read(cx).value().trim().to_string();
            (!v.is_empty()).then_some(v)
        };
        let body = c::ChannelGuideRequestInput {
            group: text(&self.guide_group, cx),
            description: text(&self.guide_description, cx),
        };
        self.guide_busy = true;
        self.guide_error = None;
        cx.notify();
        let task = client.request(move |api| async move {
            if withdraw {
                api.withdraw_guide_listing(&conversation).await
            } else {
                api.request_guide_listing(&conversation, &body).await
            }
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| {
                this.guide_busy = false;
                match result {
                    Ok(Ok(listing)) => {
                        ui::toast::success(
                            window,
                            cx,
                            if withdraw {
                                t!("channel-settings-guide-withdrawn")
                            } else {
                                t!("channel-settings-guide-sent")
                            },
                        );
                        this.guide_loaded(listing, window, cx);
                    }
                    Ok(Err(e)) => this.guide_error = Some(e.to_string().into()),
                    Err(e) => this.guide_error = Some(e.to_string().into()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    // ---- Drawing ----

    fn tabs(&self, h: &Here) -> Vec<(Tab, SharedString)> {
        let mut tabs = vec![
            (Tab::Overview, t!("channel-settings-tab-overview").into()),
            (Tab::Modes, t!("channel-settings-tab-modes").into()),
            (
                Tab::Registration,
                t!("channel-settings-tab-registration").into(),
            ),
        ];
        if self.guide.as_ref().is_some_and(|g| g.available) {
            tabs.push((Tab::Guide, t!("channel-settings-tab-guide").into()));
        }
        for mode in Chanmodes::parse(&h.isupport()).list.chars() {
            tabs.push((Tab::List(mode), list_label(mode).into()));
        }
        tabs
    }

    fn render_nav(&self, h: &Here, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = theme::get(cx).clone();
        let mut nav = div()
            .id("channel-settings-tabs")
            .test_support()
            .role(Role::TabList)
            .aria_label(t!("channel-settings-label"))
            .flex()
            .flex_col()
            .gap_0p5()
            .w(px(190.))
            .flex_none();
        for (tab, label) in self.tabs(h) {
            let active = tab == self.tab;
            let hover = theme.hover;
            nav = nav.child(
                div()
                    .id(tab_id(tab))
                    .test_support()
                    .role(Role::Tab)
                    .aria_label(label.clone())
                    .px_3()
                    .py_1p5()
                    .rounded_md()
                    .cursor_pointer()
                    .text_sm()
                    .when(active, |d| d.bg(theme.selected).text_color(theme.text))
                    .when(!active, |d| {
                        d.text_color(theme.muted).hover(move |d| d.bg(hover))
                    })
                    .child(label)
                    .on_click(
                        cx.listener(move |this, _, window, cx| this.set_tab(tab, window, cx)),
                    ),
            );
        }
        nav
    }

    fn rank_note(&self, h: &Here, theme: &Theme) -> Option<AnyElement> {
        if matches!(self.tab, Tab::Registration | Tab::Guide) {
            return None;
        }
        let text: SharedString = if h.rights.op {
            let rank = match Rank::of(h.prefix) {
                Rank::Owner => t!("channel-settings-rank-the-owner"),
                Rank::Admin => t!("channel-settings-rank-an-admin"),
                _ => t!("channel-settings-rank-an-operator"),
            };
            t!("channel-settings-can-change-all", rank = rank).into()
        } else if h.rights.ban {
            t!("channel-settings-can-manage-lists").into()
        } else {
            t!(
                "channel-settings-read-only",
                prefix = "@",
                channel = h.target()
            )
            .into()
        };
        let color = if h.rights.op || h.rights.ban {
            theme.online
        } else {
            theme.warning
        };
        Some(
            div()
                .id("channel-settings-rank-note")
                .test_support()
                .role(Role::Status)
                .aria_label(text.clone())
                .px_3()
                .py_1()
                .rounded_full()
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(color)
                .bg(color.opacity(0.12))
                .child(text)
                .into_any_element(),
        )
    }

    fn render_overview(&self, h: &Here, cx: &mut Context<Self>) -> Div {
        let theme = theme::get(cx).clone();
        let c = &h.conversation;
        let fact = |label: &'static str, value: SharedString| {
            div()
                .flex()
                .flex_col()
                .gap_0p5()
                .min_w(px(120.))
                .child(div().text_xs().text_color(theme.faint).child(label))
                .child(div().text_sm().text_color(theme.text).child(value))
        };
        let rank = match Rank::of(h.prefix) {
            Rank::Owner => t!("channel-settings-rank-owner"),
            Rank::Admin => t!("channel-settings-rank-admin"),
            Rank::Operator => t!("channel-settings-rank-operator"),
            Rank::HalfOperator => t!("channel-settings-rank-half-operator"),
            Rank::Voiced => t!("channel-settings-rank-voiced"),
            Rank::Member => t!("channel-settings-rank-member"),
        };
        let mut about = div()
            .id("channel-settings-about")
            .test_support()
            .role(Role::Group)
            .aria_label(t!("channel-settings-about"))
            .flex()
            .flex_wrap()
            .gap_4()
            .p_3()
            .rounded_lg()
            .bg(theme.panel)
            .child(fact(
                t!("channel-settings-network"),
                h.network_name().into(),
            ))
            .child(fact(
                t!("channel-settings-people"),
                c.member_ids.len().to_string().into(),
            ))
            .child(fact(
                t!("channel-settings-modes"),
                format!("+{}", c.modes).into(),
            ))
            .child(fact(t!("channel-settings-your-rank"), rank.into()));
        if let Some(created) = c.created_at.as_deref().map(date).filter(|d| !d.is_empty()) {
            about = about.child(fact(t!("channel-settings-created"), created.into()));
        }
        if let Some(url) = c.url.as_deref().filter(|u| !u.is_empty()) {
            about = about.child(fact(t!("channel-settings-website"), url.to_string().into()));
        }

        // The topic.
        let typed = self.topic.read(cx).value().to_string();
        let limit = modes::topic_limit(&h.isupport());
        let length = typed.chars().count();
        let mut meta = div()
            .flex()
            .items_center()
            .gap_3()
            .text_xs()
            .text_color(theme.faint);
        if let Some(by) = c.topic_set_by.as_deref().filter(|b| !b.is_empty()) {
            let mut text = t!("channel-settings-topic-set-by", person = by);
            if let Some(at) = c
                .topic_set_at
                .as_deref()
                .map(date)
                .filter(|d| !d.is_empty())
            {
                text = format!("{text} · {at}");
            }
            meta = meta.child(text);
        }
        if h.rights.topic {
            let over = length > limit;
            meta = meta
                .child(
                    div()
                        .id("channel-settings-topic-count")
                        .test_support()
                        .role(Role::Status)
                        .aria_label(t!(
                            "channel-settings-topic-count",
                            count = length,
                            limit = limit
                        ))
                        .when(over, |d| d.text_color(theme.danger))
                        .child(format!("{length}/{limit}")),
                )
                .child(div().flex_1())
                .child(
                    Button::new("channel-settings-topic-save")
                        .primary()
                        .label(t!("channel-settings-topic-save"))
                        .disabled(typed == self.topic_seen || over)
                        .on_click(cx.listener(|this, _, window, cx| this.save_topic(window, cx))),
                );
        }
        let topic_hint = if !h.rights.topic && c.modes.contains('t') {
            Some(t!("channel-settings-topic-locked"))
        } else if h.rights.topic {
            Some(t!("channel-settings-topic-formatting"))
        } else {
            None
        };
        let topic = div()
            .flex()
            .flex_col()
            .gap_2()
            .child(heading(t!("channel-settings-topic"), &theme))
            .child(
                Input::new(&self.topic)
                    .id("channel-settings-topic")
                    .aria_label(t!("channel-settings-topic-label"))
                    .disabled(!h.rights.topic),
            )
            .child(meta)
            .children(topic_hint.map(|t| hint(t, &theme)));

        // Settings for everyone in the channel.
        let shared = c.channel_settings.clone();
        let can_share = shared.is_some() && h.rights.op;
        let mut note = match &shared {
            None => t!("channel-settings-shared-unavailable").to_string(),
            Some(_) if h.rights.op => t!("channel-settings-shared-yours", prefix = "@").to_string(),
            Some(_) => t!("channel-settings-shared-operators", prefix = "@").to_string(),
        };
        if let Some(by) = shared.as_ref().and_then(|s| s.updated_by.as_deref()) {
            note.push(' ');
            note.push_str(&t!("channel-settings-shared-changed-by", person = by));
            if let Some(at) = shared
                .as_ref()
                .and_then(|s| s.updated_at.as_deref())
                .map(date)
                .filter(|d| !d.is_empty())
            {
                note.push_str(&format!(" · {at}"));
            }
            note.push('.');
        }
        let own_default = self.client(cx).and_then(|client| {
            let w = client.read();
            w.documents
                .notification_settings
                .value()
                .map(|s| s.channel_default)
        });
        let own_default = match own_default {
            Some(c::SettingsChannelDefault::All) => t!("channel-settings-level-all"),
            Some(c::SettingsChannelDefault::None) => t!("channel-settings-level-mute"),
            _ => t!("channel-settings-level-mentions"),
        };
        use c::ChannelSettingsNotifications as SN;
        use c::ChannelSettingsPatchBodyNotifications as PN;
        let level = match shared.as_ref().map(|s| s.notifications) {
            Some(SN::All) => PN::All,
            Some(SN::Mentions) => PN::Mentions,
            Some(SN::None) => PN::None,
            _ => PN::Default,
        };
        let this = cx.entity().downgrade();
        let notifications = choices(
            "channel-settings-notifications",
            t!("channel-settings-notifications-label"),
            vec![
                (PN::Default, t!("channel-settings-level-default").into()),
                (PN::All, t!("channel-settings-level-all").into()),
                (PN::Mentions, t!("channel-settings-level-mentions").into()),
                (PN::None, t!("channel-settings-level-mute").into()),
            ],
            level,
            can_share,
            move |value, _, cx| {
                if value == level {
                    return;
                }
                this.update(cx, |v, cx| {
                    v.change_shared(
                        c::ChannelSettingsPatchBody {
                            notifications: Some(value),
                            ..Default::default()
                        },
                        cx,
                    )
                })
                .ok();
            },
        );
        use c::ChannelSettingsGroupMentions as SG;
        use c::ChannelSettingsPatchBodyGroupMentions as PG;
        let group = match shared.as_ref().map(|s| s.group_mentions) {
            Some(SG::All) => PG::All,
            Some(SG::Off) => PG::Off,
            _ => PG::Ops,
        };
        let this = cx.entity().downgrade();
        let mentions = choices(
            "channel-settings-group-mentions",
            t!("channel-settings-group-mentions-label"),
            vec![
                (
                    PG::All,
                    t!("channel-settings-group-mentions-everyone").into(),
                ),
                (
                    PG::Ops,
                    t!("channel-settings-group-mentions-operators").into(),
                ),
                (PG::Off, t!("channel-settings-group-mentions-off").into()),
            ],
            group,
            can_share,
            move |value, _, cx| {
                if value == group {
                    return;
                }
                this.update(cx, |v, cx| {
                    v.change_shared(
                        c::ChannelSettingsPatchBody {
                            group_mentions: Some(value),
                            ..Default::default()
                        },
                        cx,
                    )
                })
                .ok();
            },
        );
        let nsfw = shared.as_ref().is_some_and(|s| s.nsfw);
        let this = cx.entity().downgrade();
        let setting = |title: &'static str, about: SharedString, control: AnyElement| {
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap_4()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_0p5()
                        .flex_1()
                        .min_w_0()
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(title),
                        )
                        .child(hint(about, &theme)),
                )
                .child(control)
        };
        let everyone = div()
            .id("channel-settings-shared")
            .test_support()
            .role(Role::Group)
            .aria_label(t!("channel-settings-shared", channel = h.target()))
            .flex()
            .flex_col()
            .gap_3()
            .child(heading(
                t!("channel-settings-shared", channel = h.target()),
                &theme,
            ))
            .child(hint(note, &theme))
            .child(setting(
                t!("channel-settings-notifications"),
                t!("channel-settings-notifications-about", level = own_default).into(),
                notifications.into_any_element(),
            ))
            .child(setting(
                t!("channel-settings-group-mentions"),
                t!("channel-settings-group-mentions-about").into(),
                mentions.into_any_element(),
            ))
            .child(setting(
                t!("channel-settings-nsfw"),
                t!("channel-settings-nsfw-about").into(),
                Switch::new("channel-settings-nsfw")
                    .checked(nsfw)
                    .disabled(!can_share)
                    .accessibility_label(t!("channel-settings-nsfw"))
                    .on_click(move |on: &bool, _, cx| {
                        let on = *on;
                        this.update(cx, |v, cx| {
                            v.change_shared(
                                c::ChannelSettingsPatchBody {
                                    nsfw: Some(on),
                                    ..Default::default()
                                },
                                cx,
                            )
                        })
                        .ok();
                    })
                    .into_any_element(),
            ))
            .child(hint(t!("channel-settings-personal"), &theme));

        let leave = div()
            .flex()
            .items_center()
            .justify_between()
            .gap_4()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(t!("channel-settings-leave-channel")),
                    )
                    .child(hint(t!("channel-settings-leave-about"), &theme)),
            )
            .child(
                Button::new("channel-settings-leave")
                    .danger()
                    .label(t!("channel-settings-leave-channel"))
                    .on_click(cx.listener(|this, _, window, cx| this.leave(window, cx))),
            );

        div()
            .flex()
            .flex_col()
            .gap_6()
            .child(about)
            .child(topic)
            .child(everyone)
            .child(leave)
    }

    fn render_modes(&self, h: &Here, cx: &mut Context<Self>) -> Div {
        let theme = theme::get(cx).clone();
        let chanmodes = Chanmodes::parse(&h.isupport());
        let set = &h.conversation.modes;
        let mut flags = div()
            .id("channel-settings-flags")
            .test_support()
            .role(Role::List)
            .aria_label(t!("channel-settings-flags"))
            .flex()
            .flex_col()
            .gap_2();
        for mode in modes::sorted_flags(&chanmodes) {
            let (label, about) = mode_text(mode);
            let on = set.contains(mode);
            let mut text = div()
                .flex()
                .flex_col()
                .gap_0p5()
                .flex_1()
                .min_w_0()
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .items_baseline()
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(label),
                        )
                        .child(
                            div()
                                .text_xs()
                                .font_family("monospace")
                                .text_color(theme.faint)
                                .child(format!("+{mode}")),
                        ),
                )
                .child(hint(about, &theme));
            if modes::known(mode) && modes::varies(mode) {
                text = text.child(hint(t!("channel-settings-mode-varies"), &theme));
            }
            flags = flags.child(
                div().flex().items_center().gap_4().child(text).child(
                    Switch::new(ElementId::Name(
                        format!("channel-settings-flag-{}", mode_key(mode)).into(),
                    ))
                    .checked(on)
                    .disabled(!h.rights.op)
                    .accessibility_label(t!(
                        "channel-settings-mode-label",
                        mode = mode,
                        label = label
                    ))
                    .on_click(cx.listener(
                        move |this, on: &bool, window, cx| this.toggle(mode, *on, window, cx),
                    )),
                ),
            );
        }
        let mut valued = div().flex().flex_col().gap_3();
        for mode in modes::value_modes(&chanmodes) {
            let Some(input) = self.values.get(&mode) else {
                continue;
            };
            let (label, about) = mode_text(mode);
            let current = h
                .conversation
                .mode_params
                .as_ref()
                .and_then(|p| p.get(&mode.to_string()))
                .cloned();
            let is_set = set.contains(mode);
            let status: Option<SharedString> = match (&current, is_set) {
                // The key is never shown.
                (_, true) if mode == 'k' => Some(t!("channel-settings-mode-set").into()),
                (Some(v), true) if !v.is_empty() => {
                    Some(t!("channel-settings-mode-current", value = v.clone()).into())
                }
                (_, true) => Some(t!("channel-settings-mode-set").into()),
                _ => None,
            };
            let typed = input.read(cx).value().trim().to_string();
            let key = mode_key(mode);
            valued = valued.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .items_baseline()
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(label),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .font_family("monospace")
                                    .text_color(theme.faint)
                                    .child(format!("+{mode}")),
                            ),
                    )
                    .child(hint(about, &theme))
                    .children(status.map(|s| hint(s, &theme)))
                    .when(h.rights.op, |d| {
                        d.child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(
                                    div().flex_1().child(
                                        Input::new(input)
                                            .id(ElementId::Name(
                                                format!("channel-settings-value-{key}").into(),
                                            ))
                                            .aria_label(t!(
                                                "channel-settings-mode-value-for",
                                                mode = mode
                                            )),
                                    ),
                                )
                                .child(
                                    Button::new(ElementId::Name(
                                        format!("channel-settings-value-{key}-set").into(),
                                    ))
                                    .primary()
                                    .label(t!("channel-settings-mode-set-button"))
                                    .disabled(typed.is_empty())
                                    .on_click(cx.listener(
                                        move |this, _, window, cx| this.set_value(mode, window, cx),
                                    )),
                                )
                                .when(is_set, |d| {
                                    d.child(
                                        Button::new(ElementId::Name(
                                            format!("channel-settings-value-{key}-remove").into(),
                                        ))
                                        .ghost()
                                        .label(t!("channel-settings-mode-remove"))
                                        .on_click(
                                            cx.listener(move |this, _, window, cx| {
                                                this.clear_value(mode, window, cx)
                                            }),
                                        ),
                                    )
                                }),
                        )
                    }),
            );
        }
        div()
            .flex()
            .flex_col()
            .gap_5()
            .child(heading(t!("channel-settings-flags"), &theme))
            .child(flags)
            .child(heading(t!("channel-settings-valued"), &theme))
            .child(valued)
            .child(hint(t!("channel-settings-registration-hint"), &theme))
    }

    fn render_guide(&self, h: &Here, cx: &mut Context<Self>) -> Div {
        let theme = theme::get(cx).clone();
        let Some(listing) = &self.guide else {
            return div();
        };
        let status = match listing.status {
            c::ChannelGuideListingStatus::Listed => t!("channel-settings-guide-listed"),
            c::ChannelGuideListingStatus::Pending => t!("channel-settings-guide-pending"),
            _ => t!("channel-settings-guide-none"),
        };
        let mut body = div()
            .flex()
            .flex_col()
            .gap_3()
            .child(hint(
                t!("channel-settings-guide-intro", channel = h.target()),
                &theme,
            ))
            .child(
                div()
                    .id("channel-settings-guide-status")
                    .test_support()
                    .role(Role::Status)
                    .aria_label(status)
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(status),
            );
        if listing.status == c::ChannelGuideListingStatus::Listed {
            return body.child(hint(t!("channel-settings-guide-listed-about"), &theme));
        }
        if !listing.operator {
            return body.child(hint(t!("channel-settings-guide-operators-only"), &theme));
        }
        if !listing.groups.is_empty() {
            body = body.child(hint(
                t!(
                    "channel-settings-guide-groups",
                    groups = listing.groups.join(", ")
                ),
                &theme,
            ));
        }
        let pending = listing.status == c::ChannelGuideListingStatus::Pending;
        body.child(ui::field::field(
            t!("channel-settings-guide-group"),
            Input::new(&self.guide_group)
                .id("channel-settings-guide-group")
                .aria_label(t!("channel-settings-guide-group")),
            &theme,
        ))
        .child(hint(t!("channel-settings-guide-group-about"), &theme))
        .child(ui::field::field(
            t!("channel-settings-guide-description"),
            Input::new(&self.guide_description)
                .id("channel-settings-guide-description")
                .aria_label(t!("channel-settings-guide-description")),
            &theme,
        ))
        .child(hint(t!("channel-settings-guide-description-about"), &theme))
        .children(
            self.guide_error
                .clone()
                .map(|e| div().text_sm().text_color(theme.danger).child(e)),
        )
        .child(
            div()
                .flex()
                .gap_2()
                .child(
                    Button::new("channel-settings-guide-request")
                        .primary()
                        .label(if pending {
                            t!("channel-settings-guide-update")
                        } else {
                            t!("channel-settings-guide-request")
                        })
                        .disabled(self.guide_busy)
                        .on_click(
                            cx.listener(|this, _, window, cx| this.guide_action(false, window, cx)),
                        ),
                )
                .when(pending, |d| {
                    d.child(
                        Button::new("channel-settings-guide-withdraw")
                            .ghost()
                            .label(t!("channel-settings-guide-withdraw"))
                            .disabled(self.guide_busy)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.guide_action(true, window, cx)
                            })),
                    )
                }),
        )
    }
}

impl Render for ChannelSettingsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme::get(cx).clone();
        let Some(h) = self.here(cx) else {
            return div()
                .id("channel-settings-dialog")
                .test_support()
                .child(hint(t!("channel-settings-channels-only"), &theme));
        };
        let content: AnyElement = match self.tab {
            Tab::Overview => self.render_overview(&h, cx).into_any_element(),
            Tab::Modes => self.render_modes(&h, cx).into_any_element(),
            Tab::Guide => self.render_guide(&h, cx).into_any_element(),
            Tab::Registration => self
                .registration
                .clone()
                .map(|v| v.into_any_element())
                .unwrap_or_else(|| div().into_any_element()),
            Tab::List(mode) => self
                .lists
                .get(&mode)
                .cloned()
                .map(|v| v.into_any_element())
                .unwrap_or_else(|| div().into_any_element()),
        };
        let nav = self.render_nav(&h, cx);
        div()
            .id("channel-settings-dialog")
            .test_support()
            .role(Role::Group)
            .aria_label(t!("channel-settings-label"))
            .flex()
            .gap_4()
            .h(px(560.))
            .child(nav)
            .child(
                div()
                    .id("channel-settings-content")
                    .test_support()
                    .role(Role::TabPanel)
                    .aria_label(t!("channel-settings-label"))
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap_4()
                    .pr_2()
                    .overflow_y_scroll()
                    .children(self.rank_note(&h, &theme))
                    .child(content),
            )
    }
}

/// A letter's part of an element ID: `b`, or `upper-i` for `I`, since IDs
/// are matched without regard to case on some platforms.
pub(crate) fn mode_key(mode: char) -> String {
    if mode.is_ascii_uppercase() {
        format!("upper-{}", mode.to_ascii_lowercase())
    } else {
        mode.to_string()
    }
}

fn tab_id(tab: Tab) -> ElementId {
    ElementId::Name(
        match tab {
            Tab::Overview => "channel-settings-tab-overview".to_string(),
            Tab::Modes => "channel-settings-tab-modes".to_string(),
            Tab::Registration => "channel-settings-tab-registration".to_string(),
            Tab::Guide => "channel-settings-tab-guide".to_string(),
            Tab::List(mode) => format!("channel-settings-tab-list-{}", mode_key(mode)),
        }
        .into(),
    )
}

/// A list mode's name: Bans, Ban exceptions, …, or `+X list`.
pub(crate) fn list_label(mode: char) -> String {
    match mode {
        'b' => t!("channel-settings-list-b").to_string(),
        'e' => t!("channel-settings-list-e").to_string(),
        'I' => t!("channel-settings-list-upper-i").to_string(),
        'q' => t!("channel-settings-list-q").to_string(),
        _ => t!("channel-settings-list-other", mode = mode),
    }
}

/// "Edit topic…" from the conversation menu: one line, saved on OK.
fn edit_topic(ctx: &ConversationCtx, window: &mut Window, cx: &mut App) {
    let Some(h) = here(&ctx.session, &ctx.conversation, cx) else {
        return;
    };
    let (session, conversation) = (ctx.session.clone(), ctx.conversation.clone());
    let target = h.target();
    ui::prompt::prompt(
        window,
        cx,
        ui::prompt::Prompt {
            title: t!("channel-settings-topic-title", channel = target.clone()).into(),
            label: t!("channel-settings-topic-label").into(),
            initial: format::strip(&h.conversation.topic),
            ok: t!("channel-settings-topic-save").into(),
        },
        move |text, window, cx| {
            send(
                &session,
                &conversation,
                modes::topic_line(&target, &text),
                window,
                cx,
            );
        },
    );
}

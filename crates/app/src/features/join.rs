//! Joining channels (package CH1), as the web client's `JoinDialog.vue`,
//! `ChannelGuidePicker.vue` and `ChannelGuideNotice.vue` do it.
//!
//! The join dialog shows the server's channel guide (Featured) above the
//! network's channel search (All channels); `/list [filter]`, the Join key,
//! the sidebar's + and the network menu open it. Under the conversation list
//! a one-time notice points members who joined before the server had a
//! guide to it. The rules (sections, what is ticked, names) live in
//! `ircord_client::api::channels`, tested there; this file draws them.

use crate::ui::focus::Reachable;
use std::collections::{HashMap, HashSet};
use std::rc::{Rc, Weak};
use std::time::Duration;

use gpui_kit::TestSupportExt;
use gpui_kit::component::Disableable;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::*;
use gpui_kit::*;
use ircord_client::Client;
use ircord_client::api::channels::{self as ch, SectionKind};
use ircord_client::commands::View;
use ircord_client::contract as c;
use ircord_client::documents::PREFERENCES;
use ircord_client::format;

use crate::features::commands;
use crate::features::shortcuts::JoinChannel;
use crate::registry::{self, MenuItem, NetworkCtx, SidebarCtx, Slot};
use crate::session::Session;
use crate::sidebar::layout;
use crate::theme::Theme;
use crate::ui::icon::IconName;
use crate::{theme, ui};

/// How long typing must pause before the channel list is asked again.
const SEARCH_DELAY: Duration = Duration::from_millis(250);

pub fn init(cx: &mut App) {
    cx.on_action(|_: &JoinChannel, cx| {
        let Some(session) = layout::session(cx) else {
            return;
        };
        layout::with_window(cx, move |window, cx| open(&session, None, "", window, cx));
    });
    commands::on_view(cx, View::ChannelList, |request, window, cx| {
        open(
            &request.session,
            Some(&request.network),
            &request.args,
            window,
            cx,
        )
    });
    registry::register(cx, |r| {
        r.network_menu.add(
            MenuItem::new(
                "network-join-channel",
                t!("channels-menu-join"),
                |n: &NetworkCtx, window, cx| open(&n.session, Some(&n.network), "", window, cx),
            )
            .order(300),
        );
        r.sidebar_footer
            .add(Slot::new("channel-guide-notice", notice).order(-10));
    });
}

/// Open the join dialog for `network` (the selected one when `None`). A
/// `filter` (from `/list` or a typed name) opens the channel search with it.
pub fn open(
    session: &Entity<Session>,
    network: Option<&str>,
    filter: &str,
    window: &mut Window,
    cx: &mut App,
) {
    let Some(network) = network
        .map(str::to_string)
        .or_else(|| session.read(cx).network.clone())
    else {
        return;
    };
    if session.read(cx).client().is_none() {
        return;
    }
    let filter = ch::normalize_filter(filter);
    let view = cx.new(|cx| JoinView::new(session.clone(), network, filter, window, cx));
    let focus = std::cell::Cell::new(true);
    ui::dialog::open(
        window,
        cx,
        t!("channels-join-title"),
        move |dialog, window, cx| {
            if focus.replace(false) && !view.read(cx).featured(cx) {
                let input = view.read(cx).name.clone();
                input.focus_handle(cx).focus(window, cx);
            }
            dialog
                .w(px(560.))
                // Enter joins the typed name; it must not close the dialog as well.
                .on_ok(|_, _, _| false)
                .child(view.clone())
        },
    );
}

enum Load<T> {
    Loading,
    Ready(T),
    Failed(SharedString),
}

pub struct JoinView {
    session: Entity<Session>,
    network: String,
    name: Entity<InputState>,
    /// The filter the list was last asked for.
    query: String,
    round: u64,
    list: Load<Vec<c::ChannelListing>>,
    guide: Load<c::ChannelGuideView>,
    /// Featured (the guide) or All channels (the search).
    show_guide: bool,
    picked: Vec<String>,
    busy: bool,
    error: Option<SharedString>,
    _subscriptions: Vec<Subscription>,
}

impl JoinView {
    fn new(
        session: Entity<Session>,
        network: String,
        filter: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let name = cx.new(|cx| {
            let mut state =
                InputState::new(window, cx).placeholder(t!("channels-join-placeholder"));
            state.set_value(filter.clone(), window, cx);
            state
        });
        let subscriptions = vec![cx.subscribe_in(
            &name,
            window,
            |this: &mut Self, _, event: &InputEvent, window, cx| match event {
                InputEvent::Change => this.typed(cx),
                InputEvent::PressEnter { .. } => this.submit(window, cx),
                _ => {}
            },
        )];
        let mut view = Self {
            session,
            network,
            name,
            query: filter.clone(),
            round: 0,
            list: Load::Loading,
            guide: Load::Loading,
            show_guide: filter.is_empty(),
            picked: Vec::new(),
            busy: false,
            error: None,
            _subscriptions: subscriptions,
        };
        view.load_guide(cx);
        view.load_list(Duration::ZERO, cx);
        view
    }

    fn client(&self, cx: &App) -> Option<Rc<Client>> {
        self.session.read(cx).client().cloned()
    }

    /// Whether the Featured view shows: the guide has channels and it is picked.
    fn featured(&self, _cx: &App) -> bool {
        self.show_guide && self.guide_ready().is_some()
    }

    fn guide_ready(&self) -> Option<&c::ChannelGuideView> {
        match &self.guide {
            Load::Ready(g) if ch::guide_available(g) => Some(g),
            _ => None,
        }
    }

    fn load_guide(&mut self, cx: &mut Context<Self>) {
        let Some(client) = self.client(cx) else {
            return;
        };
        let network = self.network.clone();
        let task = client.request(move |api| async move { api.channel_guide(&network).await });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                this.guide = match result {
                    Ok(Ok(guide)) => Load::Ready(guide),
                    Ok(Err(e)) => Load::Failed(e.to_string().into()),
                    Err(e) => Load::Failed(e.to_string().into()),
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn load_list(&mut self, delay: Duration, cx: &mut Context<Self>) {
        let Some(client) = self.client(cx) else {
            return;
        };
        self.round += 1;
        let round = self.round;
        let (network, query) = (self.network.clone(), self.query.clone());
        let task = client.request(move |api| async move {
            if !delay.is_zero() {
                tokio::time::sleep(delay).await;
            }
            api.channel_list(&network, &query).await
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                if this.round != round {
                    return;
                }
                this.list = match result {
                    Ok(Ok(items)) => Load::Ready(items),
                    Ok(Err(e)) => Load::Failed(e.to_string().into()),
                    Err(e) => Load::Failed(e.to_string().into()),
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn typed(&mut self, cx: &mut Context<Self>) {
        let query = ch::normalize_filter(&self.name.read(cx).value());
        if query == self.query {
            return;
        }
        self.query = query;
        // Keep the rows showing until the new answer replaces them.
        self.load_list(SEARCH_DELAY, cx);
        cx.notify();
    }

    fn set_view(&mut self, guide: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.show_guide = guide;
        self.error = None;
        if !guide {
            self.name.focus_handle(cx).focus(window, cx);
        }
        cx.notify();
    }

    /// Enter or the Join button: the channel typed.
    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(name) = ch::channel_name(&self.name.read(cx).value()) {
            self.join(name, window, cx);
        }
    }

    /// Open the channel if you are in it, or join it.
    fn join(&mut self, channel: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let Some(client) = self.client(cx) else {
            return;
        };
        self.error = None;
        if let Some(id) = ch::existing(&client.read(), &self.network, &channel) {
            self.session
                .update(cx, |s, cx| s.select_conversation(&id, cx));
            ui::dialog::close(window, cx);
            return;
        }
        self.busy = true;
        cx.notify();
        let network = self.network.clone();
        let task =
            client.request(move |api| async move { api.join_channel(&network, &channel).await });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| {
                this.busy = false;
                match result {
                    Ok(Ok(conversation)) => {
                        let id = conversation.id.clone();
                        client.change(|w| ch::remember(w, conversation));
                        this.session
                            .update(cx, |s, cx| s.select_conversation(&id, cx));
                        ui::dialog::close(window, cx);
                    }
                    Ok(Err(e)) => this.error = Some(e.to_string().into()),
                    Err(e) => this.error = Some(e.to_string().into()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn toggle(&mut self, key: String, on: bool, cx: &mut Context<Self>) {
        self.picked.retain(|k| *k != key);
        if on {
            self.picked.push(key);
        }
        cx.notify();
    }

    /// Join the ticked guide channels in one request.
    fn join_picked(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy || self.picked.is_empty() {
            return;
        }
        let Some(client) = self.client(cx) else {
            return;
        };
        self.busy = true;
        self.error = None;
        cx.notify();
        let (network, channels) = (self.network.clone(), self.picked.clone());
        let task = client
            .request(move |api| async move { api.join_from_guide(&network, &channels).await });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| {
                this.busy = false;
                match result {
                    Ok(Ok(result)) => {
                        let first = result.joined.first().map(|c| c.id.clone());
                        for conversation in result.joined {
                            client.change(|w| ch::remember(w, conversation));
                        }
                        if result.failed.is_empty() {
                            if let Some(id) = first {
                                this.session
                                    .update(cx, |s, cx| s.select_conversation(&id, cx));
                            }
                            ui::dialog::close(window, cx);
                        } else {
                            let names: Vec<_> =
                                result.failed.iter().map(|f| f.channel.clone()).collect();
                            this.error = Some(
                                t!("channels-guide-some-failed", channels = names.join(", "))
                                    .into(),
                            );
                            this.picked = names.iter().map(|n| ch::guide_key(n)).collect();
                        }
                    }
                    Ok(Err(e)) => this.error = Some(e.to_string().into()),
                    Err(e) => this.error = Some(e.to_string().into()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn create_instead(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (session, network) = (self.session.clone(), self.network.clone());
        ui::dialog::close(window, cx);
        window.defer(cx, move |window, cx| {
            crate::features::create_channel::open(&session, &network, window, cx)
        });
    }

    fn render_tabs(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = theme::get(cx).clone();
        let tab = |id: &'static str, label: &'static str, active: bool, guide: bool| {
            let hover = theme.accent.opacity(0.1);
            div()
                .id(id)
                .test_support()
                .role(Role::Tab)
                .reachable(&theme)
                .aria_label(label)
                .px_3()
                .py_1()
                .rounded_md()
                .cursor_pointer()
                .text_sm()
                .when(active, |d| d.bg(theme.accent).text_color(theme.on_accent))
                .when(!active, |d| {
                    d.text_color(theme.muted).hover(move |d| d.bg(hover))
                })
                .child(label)
                .on_click(cx.listener(move |this, _, window, cx| this.set_view(guide, window, cx)))
        };
        div()
            .id("join-views")
            .test_support()
            .role(Role::TabList)
            .aria_label(t!("channels-guide-views"))
            .flex()
            .gap_1()
            .p_1()
            .rounded_lg()
            .bg(theme.sidebar)
            .w_full()
            .child(tab(
                "join-view-featured",
                t!("channels-guide-featured"),
                self.show_guide,
                true,
            ))
            .child(tab(
                "join-view-search",
                t!("channels-guide-all"),
                !self.show_guide,
                false,
            ))
    }

    fn render_guide(
        &self,
        guide: &c::ChannelGuideView,
        joined: &HashSet<String>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let theme = theme::get(cx).clone();
        let mut list = div()
            .id("join-guide")
            .test_support()
            .role(Role::List)
            .aria_label(t!("channels-guide-featured"))
            .flex()
            .flex_col()
            .gap_3()
            .max_h(px(400.))
            .overflow_y_scroll();
        for section in ch::guide_sections(&guide.groups, &guide.entries) {
            let heading: SharedString = match &section.kind {
                SectionKind::Everyone => t!("channels-guide-for-everyone").into(),
                SectionKind::More => t!("channels-guide-more").into(),
                SectionKind::Group(name) => name.clone().into(),
            };
            let group_id = match &section.kind {
                SectionKind::Everyone => "join-section-everyone".to_string(),
                SectionKind::More => "join-section-more".to_string(),
                SectionKind::Group(name) => format!("join-section-{}", name.to_lowercase()),
            };
            let mut group = div()
                .id(ElementId::Name(group_id.into()))
                .test_support()
                .role(Role::Group)
                .aria_label(heading.clone())
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .px_2()
                        .text_xs()
                        .font_weight(FontWeight::BOLD)
                        .text_color(theme.faint)
                        .child(heading.to_uppercase()),
                );
            for channel in section.channels {
                group = group.child(self.guide_row(&channel, joined, &theme, cx));
            }
            list = list.child(group);
        }
        list
    }

    fn guide_row(
        &self,
        channel: &c::ChannelGuideChannel,
        joined: &HashSet<String>,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let key = ch::guide_key(&channel.channel);
        let locked = ch::guide_locked(channel, joined);
        let checked = locked || self.picked.contains(&key);
        let is_joined = joined.contains(&key);
        let about: SharedString = match &channel.description {
            Some(d) if !d.is_empty() => d.clone().into(),
            _ => topic_text(channel.topic.as_deref().unwrap_or_default()),
        };
        let status = if is_joined {
            Some(t!("channels-joined"))
        } else if channel.state == c::ChannelGuideChannelState::Everyone {
            Some(t!("channels-guide-everyone-joins"))
        } else {
            None
        };
        let id = row_id("join-guide", &channel.channel);
        let toggle_key = key.clone();
        let mut name = div().flex().items_center().gap_1().min_w_0().child(
            div()
                .text_sm()
                .font_weight(FontWeight::BOLD)
                .truncate()
                .child(channel.channel.clone()),
        );
        if channel.nsfw {
            name = name.child(badge(t!("channels-guide-nsfw"), theme.danger, theme));
        }
        div()
            .flex()
            .items_center()
            .gap_2()
            .p_2()
            .rounded_md()
            .child(
                Checkbox::new(id)
                    .checked(checked)
                    .disabled(locked || self.busy)
                    .accessibility_label(t!(
                        "channels-guide-pick",
                        channel = channel.channel.clone()
                    ))
                    .on_click(cx.listener(move |this, on: &bool, _, cx| {
                        this.toggle(toggle_key.clone(), *on, cx)
                    })),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .child(name)
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.faint)
                            .truncate()
                            .child(about),
                    ),
            )
            .children(status.map(|s| badge(s, theme.accent, theme)))
            .children(
                channel
                    .members
                    .map(|n| member_count("join-guide", &channel.channel, n, theme)),
            )
    }

    fn render_search(
        &self,
        joined: &HashSet<String>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let theme = theme::get(cx).clone();
        let typed = !self
            .name
            .read(cx)
            .value()
            .trim()
            .trim_start_matches('#')
            .is_empty();
        let form = div()
            .flex()
            .items_center()
            .gap_2()
            .child(div().text_lg().text_color(theme.faint).child("#"))
            .child(
                div().flex_1().child(
                    Input::new(&self.name)
                        .id("join-name")
                        .aria_label(t!("channels-join-name")),
                ),
            )
            .child(
                Button::new("join-submit")
                    .primary()
                    .label(t!("channels-join-action"))
                    .disabled(!typed || self.busy)
                    .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx))),
            );
        let mut list = div()
            .id("join-channel-list")
            .test_support()
            .role(Role::List)
            .aria_label(t!("channels-join-list"))
            .flex()
            .flex_col()
            .max_h(px(400.))
            .overflow_y_scroll();
        match &self.list {
            Load::Loading => {
                list = list.child(hint(
                    "join-loading",
                    t!("channels-join-loading").into(),
                    false,
                    &theme,
                ));
            }
            Load::Failed(e) => {
                list = list.child(hint("join-list-error", e.clone(), true, &theme));
            }
            Load::Ready(items) if items.is_empty() => {
                let text = if self.query.is_empty() {
                    t!("channels-join-empty")
                } else {
                    t!("channels-join-no-match")
                };
                list = list.child(hint("join-empty", text.into(), false, &theme));
            }
            Load::Ready(items) => {
                for item in items {
                    list = list.child(self.search_row(item, joined, &theme, cx));
                }
            }
        }
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(form)
            .children(self.error_line(&theme))
            .child(list)
    }

    fn search_row(
        &self,
        item: &c::ChannelListing,
        joined: &HashSet<String>,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let name = item.name.clone();
        let is_joined = joined.contains(&ch::guide_key(&name));
        let topic = if item.topic.trim().is_empty() {
            SharedString::from(t!("channels-join-no-topic"))
        } else {
            topic_text(&item.topic)
        };
        let hover = theme.accent.opacity(0.1);
        div()
            .id(row_id("join-channel", &name))
            .test_support()
            .role(Role::Button)
            .reachable(&theme)
            .aria_label(name.clone())
            .flex()
            .items_center()
            .gap_2()
            .p_2()
            .rounded_md()
            .cursor_pointer()
            .hover(move |d| d.bg(hover))
            .child(div().text_lg().text_color(theme.faint).child("#"))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::BOLD)
                            .child(name.trim_start_matches('#').to_string()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.faint)
                            .truncate()
                            .child(topic),
                    ),
            )
            .when(is_joined, |d| {
                d.child(badge(t!("channels-joined"), theme.accent, theme))
            })
            .child(member_count("join-channel", &item.name, item.users, theme))
            .on_click(cx.listener(move |this, _, window, cx| this.join(name.clone(), window, cx)))
    }

    fn error_line(&self, theme: &Theme) -> Option<impl IntoElement + use<>> {
        self.error
            .clone()
            .map(|e| hint("join-error", e, true, theme))
    }
}

impl Render for JoinView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme::get(cx).clone();
        let Some(client) = self.client(cx) else {
            return div().id("join-dialog").test_support();
        };
        let (network_name, joined) = {
            let w = client.read();
            (
                w.network(&self.network)
                    .map(|n| n.name.clone())
                    .unwrap_or_default(),
                ch::joined(&w, &self.network),
            )
        };
        let mut root = div()
            .id("join-dialog")
            .test_support()
            .role(Role::Group)
            .aria_label(t!("channels-join-title"))
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .text_sm()
                    .text_color(theme.muted)
                    .child(t!("channels-join-description", network = network_name)),
            );
        let guide = match &self.guide {
            Load::Ready(g) if ch::guide_available(g) => Some(g.clone()),
            _ => None,
        };
        if guide.is_some() {
            root = root.child(self.render_tabs(cx));
        }
        match guide.filter(|_| self.show_guide) {
            Some(guide) => {
                let count = self.picked.len();
                let label: SharedString = if self.busy {
                    t!("channels-guide-joining").into()
                } else {
                    t!("channels-guide-join-count", count = count).into()
                };
                root = root
                    .child(self.render_guide(&guide, &joined, cx))
                    .children(self.error_line(&theme))
                    .child(
                        div().flex().justify_end().child(
                            Button::new("join-guide-join")
                                .primary()
                                .label(label)
                                .disabled(count == 0 || self.busy)
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.join_picked(window, cx)),
                                ),
                        ),
                    );
            }
            None => root = root.child(self.render_search(&joined, cx)),
        }
        root.child(
            div()
                .pt_2()
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted)
                        .child(t!("channels-join-create-hint")),
                )
                .child(
                    Button::new("join-create")
                        .ghost()
                        .label(t!("channels-join-create"))
                        .on_click(
                            cx.listener(|this, _, window, cx| this.create_instead(window, cx)),
                        ),
                ),
        )
    }
}

/// `join-channel-rust` for `#Rust`.
fn row_id(prefix: &str, channel: &str) -> ElementId {
    ElementId::Name(
        format!(
            "{prefix}-{}",
            ch::guide_key(channel.trim_start_matches('#'))
        )
        .into(),
    )
}

/// A topic without its IRC colours and formatting.
fn topic_text(topic: &str) -> SharedString {
    format::strip(topic).into()
}

fn badge(text: impl Into<SharedString>, colour: Hsla, theme: &Theme) -> Div {
    let _ = theme;
    div()
        .px_1p5()
        .rounded_sm()
        .text_xs()
        .font_weight(FontWeight::SEMIBOLD)
        .bg(colour.opacity(0.12))
        .text_color(colour)
        .child(text.into())
}

/// How many people are in a channel: the number beside an icon, read out
/// with its meaning.
fn member_count(row: &str, channel: &str, count: i64, theme: &Theme) -> impl IntoElement + use<> {
    let label = t!("channels-user-count", count = count);
    div()
        .id(ElementId::Name(
            format!(
                "{row}-{}-users",
                ch::guide_key(channel.trim_start_matches('#'))
            )
            .into(),
        ))
        .test_support()
        .role(Role::Status)
        .aria_label(label)
        .flex()
        .items_center()
        .justify_end()
        .gap_1()
        .min_w(px(56.))
        .text_xs()
        .text_color(theme.faint)
        .child(ui::icon::icon(IconName::User, theme.faint))
        .child(count.to_string())
}

fn hint(
    id: &'static str,
    text: SharedString,
    error: bool,
    theme: &Theme,
) -> impl IntoElement + use<> {
    div()
        .id(id)
        .test_support()
        .role(if error { Role::Alert } else { Role::Status })
        .aria_label(text.clone())
        .p_2()
        .text_sm()
        .text_color(if error { theme.danger } else { theme.muted })
        .child(text)
}

// ---- The one-time notice about the channel guide ----

/// What the notice needs from the server, per signed-in client.
#[derive(Default)]
struct Notice(Vec<NoticeState>);
impl Global for Notice {}

/// One client's part of [`Notice`]. Kept apart per client so two windows
/// signed in at once do not take turns resetting each other's.
struct NoticeState {
    client: Weak<Client>,
    /// Whether the account still waits for its onboarding (S4 shows the
    /// guide there instead).
    onboarding: Option<Load<bool>>,
    guides: HashMap<String, Load<c::ChannelGuideView>>,
}

/// `client`'s part of the notice state, if it has one yet.
fn notice_state<'a>(cx: &'a mut App, client: &Rc<Client>) -> Option<&'a mut NoticeState> {
    cx.default_global::<Notice>()
        .0
        .iter_mut()
        .find(|s| s.client.upgrade().is_some_and(|c| Rc::ptr_eq(&c, client)))
}

/// Members who joined before owners saved a guide see this once. It never
/// joins anything: opening it shows Featured in the join dialog. Dismissing
/// or opening it is remembered with the synced preferences.
fn notice(ctx: &SidebarCtx, window: &mut Window, cx: &mut App) -> Option<AnyElement> {
    let network = ctx.network.clone()?;
    let client = ctx.session.read(cx).client().cloned()?;
    if notice_state(cx, &client).is_none() {
        let notice = cx.default_global::<Notice>();
        notice.0.retain(|s| s.client.strong_count() > 0);
        notice.0.push(NoticeState {
            client: Rc::downgrade(&client),
            onboarding: None,
            guides: HashMap::new(),
        });
        // The sidebar's watch, shared, so the preferences are read once.
        layout::watch_documents(Some(client.clone()), cx);
    }
    let seen = {
        let w = client.read();
        let prefs = w.documents.preferences.value()?;
        prefs
            .preferences
            .as_ref()
            .and_then(|p| p.channel_guide_seen)
            .unwrap_or(false)
    };
    if seen {
        return None;
    }
    let state = notice_state(cx, &client)?;
    if state.onboarding.is_none() {
        state.onboarding = Some(Load::Loading);
        let owner = client.clone();
        let task = client.request(|api| async move { api.account().await });
        window
            .spawn(cx, async move |cx| {
                let result = task.await;
                cx.update(|window, cx| {
                    let pending =
                        matches!(&result, Ok(Ok(Some(a))) if a.onboarding_network_id.is_some());
                    if let Some(state) = notice_state(cx, &owner) {
                        state.onboarding = Some(Load::Ready(pending));
                    }
                    window.refresh();
                })
                .ok();
            })
            .detach();
    }
    let state = notice_state(cx, &client)?;
    if !state.guides.contains_key(&network) {
        state.guides.insert(network.clone(), Load::Loading);
        let owner = client.clone();
        let id = network.clone();
        let task = client.request(move |api| async move { api.channel_guide(&id).await });
        let key = network.clone();
        window
            .spawn(cx, async move |cx| {
                let result = task.await;
                cx.update(|window, cx| {
                    let loaded = match result {
                        Ok(Ok(guide)) => Load::Ready(guide),
                        Ok(Err(e)) => Load::Failed(e.to_string().into()),
                        Err(e) => Load::Failed(e.to_string().into()),
                    };
                    if let Some(state) = notice_state(cx, &owner) {
                        state.guides.insert(key, loaded);
                    }
                    window.refresh();
                })
                .ok();
            })
            .detach();
    }
    let state = notice_state(cx, &client)?;
    if !matches!(state.onboarding, Some(Load::Ready(false))) {
        return None;
    }
    let Some(Load::Ready(guide)) = state.guides.get(&network) else {
        return None;
    };
    if !ch::guide_available(guide) || guide.revision <= 0 {
        return None;
    }
    let network_name = client
        .read()
        .network(&network)
        .map(|n| n.name.clone())
        .unwrap_or_default();
    let theme = theme::get(cx).clone();
    let (open_session, open_network) = (ctx.session.clone(), network.clone());
    let (open_client, dismiss_client) = (client.clone(), client.clone());
    Some(
        div()
            .id("channel-guide-notice")
            .test_support()
            .role(Role::Region)
            .aria_label(t!("channels-notice-title"))
            .relative()
            .mx_2()
            .mb_2()
            .p_3()
            .pr_8()
            .rounded_lg()
            .bg(theme.main)
            .border_1()
            .border_color(theme.selected)
            .flex()
            .flex_col()
            .gap_1p5()
            .text_sm()
            .child(
                div()
                    .font_weight(FontWeight::BOLD)
                    .text_color(theme.text)
                    .child(t!("channels-notice-title")),
            )
            .child(
                div()
                    .text_color(theme.muted)
                    .child(t!("channels-notice-body", network = network_name)),
            )
            .child(
                div().child(
                    Button::new("channel-guide-notice-open")
                        .primary()
                        .label(t!("channels-notice-action"))
                        .on_click(move |_, window, cx| {
                            mark_seen(&open_client);
                            open(&open_session, Some(&open_network), "", window, cx);
                        }),
                ),
            )
            .child(
                div()
                    .absolute()
                    .top_1p5()
                    .right_1p5()
                    .child(ui::icon::button(
                        "channel-guide-notice-dismiss",
                        IconName::Close,
                        t!("channels-notice-dismiss"),
                        &theme,
                        move |_, _, _| mark_seen(&dismiss_client),
                    )),
            )
            .into_any_element(),
    )
}

fn mark_seen(client: &Client) {
    let mut next = client
        .read()
        .documents
        .preferences
        .value()
        .cloned()
        .unwrap_or_default();
    next.preferences.get_or_insert_default().channel_guide_seen = Some(true);
    client.edit_document(&PREFERENCES, next);
}

//! Administration → Welcome, after `AdminWelcome.vue`: the channel guide new
//! members pick from when they join, and everyone can browse when joining a
//! channel. Owners edit its groups and channels here, see the picker as a
//! new member sees it, and approve, edit or decline channel operators'
//! requests to be listed. Saving never joins or parts anyone.
//!
//! The editing rules (unique groups, the limits, requests already listed)
//! are `GuideDraft` in the client crate.

use std::time::Duration;

use gpui_kit::TestSupportExt;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::*;
use gpui_kit::*;
use ircord_client::api::admin::{self as a, AddError, GuideDraft, MAX_ENTRIES, MAX_GROUPS};
use ircord_client::api::channels::{self as ch, SectionKind};
use ircord_client::contract as c;

use super::{Kind, button, client, error_text, group, hint, notice, when};
use crate::session::Session;
use crate::theme::{self, Theme};
use crate::ui;
use crate::ui::icon::IconName;

const DEBOUNCE: Duration = Duration::from_millis(250);

pub struct WelcomePanel {
    session: Entity<Session>,
    saved: Option<c::AdminChannelGuide>,
    draft: GuideDraft,
    loading: bool,
    saving: bool,
    error: Option<String>,
    /// Another owner saved the guide since it was loaded.
    conflict: bool,
    notice: Option<String>,
    /// The request being declined.
    declining: Option<String>,
    /// One field per group name and per channel description, rebuilt when
    /// the lists change shape.
    group_inputs: Vec<Entity<InputState>>,
    description_inputs: Vec<Entity<InputState>>,
    inputs_stale: bool,
    input_subscriptions: Vec<Subscription>,
    new_group: Entity<InputState>,
    add_channel: Entity<InputState>,
    directory: Vec<c::GuideDirectoryChannel>,
    directory_generation: u64,
    _subscriptions: Vec<Subscription>,
}

impl WelcomePanel {
    pub fn new(session: Entity<Session>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let new_group =
            cx.new(|cx| InputState::new(window, cx).placeholder(t!("admin-guide-new-group")));
        let add_channel = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t!("admin-guide-add-channel-placeholder"))
        });
        let subscriptions = vec![
            cx.subscribe_in(
                &new_group,
                window,
                |this: &mut Self, _, event: &InputEvent, window, cx| {
                    if let InputEvent::PressEnter { .. } = event {
                        this.add_group(window, cx);
                    }
                },
            ),
            cx.subscribe_in(
                &add_channel,
                window,
                |this: &mut Self, input, event: &InputEvent, window, cx| match event {
                    InputEvent::Change => {
                        let text = input.read(cx).value().to_string();
                        this.search_directory(text, cx);
                    }
                    InputEvent::PressEnter { .. } => {
                        let text = input.read(cx).value().to_string();
                        this.add_channel(&text, window, cx);
                    }
                    _ => {}
                },
            ),
        ];
        let mut panel = Self {
            session,
            saved: None,
            draft: GuideDraft::default(),
            loading: false,
            saving: false,
            error: None,
            conflict: false,
            notice: None,
            declining: None,
            group_inputs: Vec::new(),
            description_inputs: Vec::new(),
            inputs_stale: true,
            input_subscriptions: Vec::new(),
            new_group,
            add_channel,
            directory: Vec::new(),
            directory_generation: 0,
            _subscriptions: subscriptions,
        };
        panel.load(cx);
        panel
    }

    pub fn load(&mut self, cx: &mut Context<Self>) {
        let Some(client) = client(&self.session, cx) else {
            return;
        };
        self.loading = true;
        self.error = None;
        self.conflict = false;
        let task = client.request(|api| async move { api.administration_channel_guide().await });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                this.loading = false;
                match result {
                    Ok(Ok(saved)) => this.adopt(saved),
                    Ok(Err(e)) => {
                        super::refused(&client, &e, cx);
                        this.error = Some(error_text(&e));
                    }
                    Err(e) => this.error = Some(e.to_string()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn adopt(&mut self, saved: c::AdminChannelGuide) {
        self.draft = GuideDraft::from_saved(&saved);
        self.saved = Some(saved);
        self.inputs_stale = true;
    }

    /// Save the draft; `done` is said once it is saved.
    fn save(&mut self, done: String, cx: &mut Context<Self>) {
        let (Some(saved), Some(client)) = (&self.saved, client(&self.session, cx)) else {
            return;
        };
        if self.saving {
            return;
        }
        self.saving = true;
        self.error = None;
        self.conflict = false;
        self.notice = None;
        let (revision, guide) = (saved.revision, self.draft.guide());
        let task =
            client.request(move |api| async move { api.save_channel_guide(revision, guide).await });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                this.saving = false;
                match result {
                    Ok(Ok(saved)) => {
                        this.adopt(saved);
                        this.notice = Some(done);
                    }
                    Ok(Err(e)) => {
                        super::refused(&client, &e, cx);
                        this.conflict = a::conflict(&e);
                        this.error = Some(error_text(&e));
                    }
                    Err(e) => this.error = Some(e.to_string()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    // ---- Groups ----

    fn add_group(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.new_group.read(cx).value().to_string();
        if self.draft.add_group(&name) {
            self.new_group
                .update(cx, |s, cx| s.set_value("", window, cx));
            self.inputs_stale = true;
            cx.notify();
        }
    }

    fn shift_group(&mut self, index: usize, by: isize, cx: &mut Context<Self>) {
        a::shift(&mut self.draft.groups, index, by);
        self.inputs_stale = true;
        cx.notify();
    }

    fn remove_group(&mut self, index: usize, cx: &mut Context<Self>) {
        self.draft.remove_group(index);
        self.inputs_stale = true;
        cx.notify();
    }

    // ---- Channels ----

    fn search_directory(&mut self, text: String, cx: &mut Context<Self>) {
        self.directory_generation += 1;
        let generation = self.directory_generation;
        let query = text.trim().to_string();
        let directory = self.saved.as_ref().is_some_and(|s| s.directory);
        if query.is_empty() || !directory {
            self.directory.clear();
            cx.notify();
            return;
        }
        let Some(client) = client(&self.session, cx) else {
            return;
        };
        let settle = super::pause(&self.session, cx, DEBOUNCE);
        cx.spawn(async move |this, cx| {
            let _ = settle.await;
            let still = this
                .update(cx, |this, _| this.directory_generation == generation)
                .unwrap_or(false);
            if !still {
                return;
            }
            let result = client
                .request(move |api| async move { api.guide_directory(&query).await })
                .await;
            this.update(cx, |this, cx| {
                if this.directory_generation != generation {
                    return;
                }
                match result {
                    Ok(Ok(page)) => this.directory = page.items,
                    Ok(Err(e)) => super::refused(&client, &e, cx),
                    Err(_) => {}
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn added(
        &mut self,
        result: Result<String, AddError>,
        cx: &mut Context<Self>,
    ) -> Option<String> {
        match result {
            Ok(channel) => {
                self.inputs_stale = true;
                cx.notify();
                Some(channel)
            }
            Err(AddError::Full) => {
                self.notice = Some(t!("admin-guide-too-many", count = MAX_ENTRIES));
                cx.notify();
                None
            }
            Err(_) => None,
        }
    }

    fn add_channel(&mut self, typed: &str, window: &mut Window, cx: &mut Context<Self>) {
        let result = self.draft.add_channel(typed);
        if self.added(result, cx).is_some() {
            self.directory_generation += 1;
            self.directory.clear();
            self.add_channel
                .update(cx, |s, cx| s.set_value("", window, cx));
        }
    }

    fn shift_entry(&mut self, index: usize, by: isize, cx: &mut Context<Self>) {
        a::shift(&mut self.draft.entries, index, by);
        self.inputs_stale = true;
        cx.notify();
    }

    fn remove_entry(&mut self, index: usize, cx: &mut Context<Self>) {
        if index < self.draft.entries.len() {
            self.draft.entries.remove(index);
            self.inputs_stale = true;
            cx.notify();
        }
    }

    // ---- Requests ----

    fn approve(&mut self, request: &c::ChannelGuideRequestView, cx: &mut Context<Self>) {
        let result = self.draft.apply_request(request);
        if self.added(result, cx).is_some() {
            self.save(
                t!("admin-guide-approved", channel = request.channel.as_str()),
                cx,
            );
        }
    }

    fn edit(&mut self, request: &c::ChannelGuideRequestView, cx: &mut Context<Self>) {
        let result = self.draft.apply_request(request);
        if self.added(result, cx).is_some() {
            self.notice = Some(t!(
                "admin-guide-editing",
                channel = request.channel.as_str()
            ));
        }
    }

    fn decline(&mut self, channel: String, cx: &mut Context<Self>) {
        let Some(client) = client(&self.session, cx) else {
            return;
        };
        self.declining = Some(channel.clone());
        self.error = None;
        let name = channel.clone();
        let task = client.request(move |api| async move { api.decline_guide_request(&name).await });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                this.declining = None;
                match result {
                    Ok(Ok(())) => {
                        if let Some(saved) = &mut this.saved {
                            saved.requests.retain(|r| r.channel != channel);
                        }
                        this.notice = Some(t!("admin-guide-declined", channel = channel.as_str()));
                    }
                    Ok(Err(e)) => {
                        super::refused(&client, &e, cx);
                        this.error = Some(error_text(&e));
                    }
                    Err(e) => this.error = Some(e.to_string()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// Make one field per group and per channel, with what the draft holds.
    fn sync_inputs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !std::mem::take(&mut self.inputs_stale) {
            return;
        }
        self.input_subscriptions.clear();
        let field = |value: String, label: String, window: &mut Window, cx: &mut Context<Self>| {
            cx.new(|cx| {
                let mut s = InputState::new(window, cx).placeholder(label);
                s.set_value(value, window, cx);
                s
            })
        };
        self.group_inputs = self
            .draft
            .groups
            .clone()
            .into_iter()
            .map(|name| field(name, t!("admin-guide-group-name").to_string(), window, cx))
            .collect();
        self.description_inputs = self
            .draft
            .entries
            .clone()
            .into_iter()
            .map(|e| {
                field(
                    e.description.unwrap_or_default(),
                    t!("admin-guide-description-placeholder").to_string(),
                    window,
                    cx,
                )
            })
            .collect();
        for (i, input) in self.group_inputs.clone().into_iter().enumerate() {
            self.input_subscriptions.push(cx.subscribe(
                &input,
                move |this: &mut Self, input, event: &InputEvent, cx| {
                    if let InputEvent::Change = event {
                        let name = input.read(cx).value().to_string();
                        this.draft.rename_group(i, &name);
                        cx.notify();
                    }
                },
            ));
        }
        for (j, input) in self.description_inputs.clone().into_iter().enumerate() {
            self.input_subscriptions.push(cx.subscribe(
                &input,
                move |this: &mut Self, input, event: &InputEvent, cx| {
                    if let InputEvent::Change = event {
                        let text = input.read(cx).value().to_string();
                        if let Some(e) = this.draft.entries.get_mut(j) {
                            e.description = Some(text);
                        }
                        cx.notify();
                    }
                },
            ));
        }
    }
}

fn state_label(state: c::ChannelGuideEntryState) -> &'static str {
    match state {
        c::ChannelGuideEntryState::Everyone => t!("admin-guide-for-everyone"),
        c::ChannelGuideEntryState::Suggested => t!("admin-guide-suggested"),
        _ => t!("admin-guide-listed"),
    }
}

fn warning_text(kind: c::ChannelGuideWarningKind) -> Option<&'static str> {
    Some(match kind {
        c::ChannelGuideWarningKind::Missing => t!("admin-guide-warning-missing"),
        c::ChannelGuideWarningKind::Hidden => t!("admin-guide-warning-hidden"),
        c::ChannelGuideWarningKind::Unregistered => t!("admin-guide-warning-unregistered"),
        _ => return None,
    })
}

/// A small choice among several: the chosen one is highlighted.
fn pill(
    id: String,
    label: impl Into<SharedString>,
    aria: String,
    on: bool,
    theme: &Theme,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let hover = theme.hover;
    div()
        .id(ElementId::Name(id.into()))
        .test_support()
        .role(Role::RadioButton)
        .aria_label(aria)
        .aria_selected(on)
        .px_2p5()
        .py_0p5()
        .rounded_full()
        .text_xs()
        .cursor_pointer()
        .bg(if on { theme.accent } else { theme.input })
        .text_color(if on { theme.on_accent } else { theme.muted })
        .when(!on, |d| d.hover(move |d| d.bg(hover)))
        .child(label.into())
        .on_click(on_click)
        .into_any_element()
}

/// A boxed item of a list; `content` is laid out in a column.
fn card(id: String, label: String, theme: &Theme, content: Div) -> AnyElement {
    div()
        .id(ElementId::Name(id.into()))
        .test_support()
        .role(Role::ListItem)
        .aria_label(label)
        .flex()
        .flex_col()
        .gap_2()
        .px_3p5()
        .py_3()
        .rounded_lg()
        .bg(theme.raised)
        .border_1()
        .border_color(theme.border)
        .child(content.flex().flex_col().gap_2())
        .into_any_element()
}

impl WelcomePanel {
    fn requests(&self, saved: &c::AdminChannelGuide, cx: &mut Context<Self>) -> Option<AnyElement> {
        let theme = theme::get(cx).clone();
        let pending: Vec<c::ChannelGuideRequestView> =
            self.draft.pending(saved).into_iter().cloned().collect();
        if pending.is_empty() {
            return None;
        }
        let busy = self.saving || self.declining.is_some();
        let mut list = div()
            .id("admin-requests")
            .test_support()
            .role(Role::List)
            .aria_label(t!("admin-guide-requests"))
            .flex()
            .flex_col()
            .gap_2();
        for (i, r) in pending.into_iter().enumerate() {
            let members = r
                .members
                .map(|n| {
                    if n == 1 {
                        t!("admin-guide-members-one").to_string()
                    } else {
                        t!("admin-guide-members", count = a::count(n))
                    }
                })
                .unwrap_or_default();
            let about = r
                .description
                .clone()
                .filter(|d| !d.is_empty())
                .or_else(|| r.topic.clone().filter(|t| !t.is_empty()))
                .unwrap_or_else(|| t!("admin-guide-no-description").to_string());
            let asked = t!(
                "admin-guide-requested-by",
                account = r.requester.as_str(),
                time = when(&r.created_at)
            );
            let declining = self.declining.as_deref() == Some(r.channel.as_str());
            let (approve, edit, decline) = (r.clone(), r.clone(), r.channel.clone());
            list = list.child(card(
                format!("admin-request-{i}"),
                format!("{}. {asked}. {about}", r.channel),
                &theme,
                div()
                    .child(
                        div()
                            .flex()
                            .items_baseline()
                            .gap_2()
                            .child(
                                div()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(theme.text)
                                    .child(r.channel.clone()),
                            )
                            .child(div().text_xs().text_color(theme.faint).child(members)),
                    )
                    .child(div().text_xs().text_color(theme.muted).child(asked))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1p5()
                            .text_sm()
                            .text_color(theme.body)
                            .when_some(r.group.clone().filter(|g| !g.is_empty()), |d, g| {
                                d.child(
                                    div()
                                        .px_1p5()
                                        .rounded_sm()
                                        .bg(theme.selected)
                                        .text_xs()
                                        .text_color(theme.accent)
                                        .child(g),
                                )
                            })
                            .child(about),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap_2()
                            .child(button(
                                ElementId::Name(format!("admin-request-{i}-approve").into()),
                                t!("admin-guide-approve"),
                                Some(t!(
                                    "admin-guide-approve-channel",
                                    channel = r.channel.as_str()
                                )),
                                Kind::Primary,
                                !busy,
                                &theme,
                                cx.listener(move |this, _, _, cx| this.approve(&approve, cx)),
                            ))
                            .child(button(
                                ElementId::Name(format!("admin-request-{i}-edit").into()),
                                t!("admin-guide-edit"),
                                None,
                                Kind::Secondary,
                                !busy,
                                &theme,
                                cx.listener(move |this, _, _, cx| this.edit(&edit, cx)),
                            ))
                            .child(button(
                                ElementId::Name(format!("admin-request-{i}-decline").into()),
                                if declining {
                                    t!("admin-guide-declining")
                                } else {
                                    t!("admin-guide-decline")
                                },
                                None,
                                Kind::Ghost,
                                !busy,
                                &theme,
                                cx.listener(move |this, _, _, cx| {
                                    this.decline(decline.clone(), cx)
                                }),
                            )),
                    ),
            ));
        }
        Some(
            group(
                "admin-guide-requests",
                t!("admin-guide-requests"),
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(hint(t!("admin-guide-requests-help"), &theme))
                    .child(list),
                &theme,
            )
            .into_any_element(),
        )
    }

    fn groups(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = theme::get(cx).clone();
        let count = self.draft.groups.len();
        let mut list = div()
            .id("admin-groups")
            .test_support()
            .role(Role::List)
            .aria_label(t!("admin-guide-groups"))
            .flex()
            .flex_col()
            .gap_1p5();
        for (i, input) in self.group_inputs.iter().enumerate() {
            let name = self.draft.groups.get(i).cloned().unwrap_or_default();
            list = list.child(
                div()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .child(
                        div().flex_1().child(
                            Input::new(input)
                                .id(ElementId::Name(format!("admin-group-{i}-name").into()))
                                .aria_label(t!("admin-guide-group-name")),
                        ),
                    )
                    .when(i > 0, |d| {
                        d.child(ui::icon::button(
                            ElementId::Name(format!("admin-group-{i}-up").into()),
                            IconName::ArrowUp,
                            t!("admin-guide-move-up", name = name.as_str()),
                            &theme,
                            cx.listener(move |this, _, _, cx| this.shift_group(i, -1, cx)),
                        ))
                    })
                    .when(i + 1 < count, |d| {
                        d.child(ui::icon::button(
                            ElementId::Name(format!("admin-group-{i}-down").into()),
                            IconName::ArrowDown,
                            t!("admin-guide-move-down", name = name.as_str()),
                            &theme,
                            cx.listener(move |this, _, _, cx| this.shift_group(i, 1, cx)),
                        ))
                    })
                    .child(ui::icon::button(
                        ElementId::Name(format!("admin-group-{i}-remove").into()),
                        IconName::Close,
                        t!("admin-guide-remove", name = name.as_str()),
                        &theme,
                        cx.listener(move |this, _, _, cx| this.remove_group(i, cx)),
                    )),
            );
        }
        let typed = !self.new_group.read(cx).value().trim().is_empty();
        let add = div()
            .flex()
            .items_center()
            .gap_2()
            .child(
                div().flex_1().child(
                    Input::new(&self.new_group)
                        .id("admin-new-group")
                        .aria_label(t!("admin-guide-new-group")),
                ),
            )
            .child(button(
                "admin-add-group",
                t!("admin-guide-add-group"),
                None,
                Kind::Secondary,
                typed && count < MAX_GROUPS,
                &theme,
                cx.listener(|this, _, window, cx| this.add_group(window, cx)),
            ));
        group(
            "admin-guide-groups",
            t!("admin-guide-groups"),
            div()
                .flex()
                .flex_col()
                .gap_2()
                .child(hint(t!("admin-guide-groups-help"), &theme))
                .child(list)
                .child(add),
            &theme,
        )
        .into_any_element()
    }

    fn channels(&self, saved: &c::AdminChannelGuide, cx: &mut Context<Self>) -> AnyElement {
        let theme = theme::get(cx).clone();
        let mut body = div()
            .flex()
            .flex_col()
            .gap_2()
            .child(hint(t!("admin-guide-channels-help"), &theme));
        if !saved.saved {
            let text = if saved.defaults.is_empty() {
                t!("admin-guide-not-saved").to_string()
            } else {
                t!(
                    "admin-guide-not-saved-defaults",
                    channels = saved.defaults.join(", ")
                )
            };
            body = body.child(notice("admin-guide-defaults", text, false, &theme));
        }
        let count = self.draft.entries.len();
        let groups: Vec<String> = self
            .draft
            .groups
            .iter()
            .filter(|g| !g.trim().is_empty())
            .cloned()
            .collect();
        let mut list = div()
            .id("admin-guide-entries")
            .test_support()
            .role(Role::List)
            .aria_label(t!("admin-guide-channels"))
            .flex()
            .flex_col()
            .gap_2();
        for (j, e) in self.draft.entries.iter().enumerate() {
            let channel = e.channel.clone();
            let mut states = div()
                .id(ElementId::Name(format!("admin-guide-{j}-states").into()))
                .test_support()
                .role(Role::RadioGroup)
                .aria_label(t!("admin-guide-state-for", channel = channel.as_str()))
                .flex()
                .flex_wrap()
                .gap_1p5();
            for (key, state) in [
                ("everyone", c::ChannelGuideEntryState::Everyone),
                ("suggested", c::ChannelGuideEntryState::Suggested),
                ("listed", c::ChannelGuideEntryState::Listed),
            ] {
                states = states.child(pill(
                    format!("admin-guide-{j}-state-{key}"),
                    state_label(state),
                    state_label(state).to_string(),
                    e.state == state,
                    &theme,
                    cx.listener(move |this, _, _, cx| {
                        if let Some(e) = this.draft.entries.get_mut(j) {
                            e.state = state;
                        }
                        cx.notify();
                    }),
                ));
            }
            let current = e.group.clone().unwrap_or_default();
            let mut in_group = div()
                .id(ElementId::Name(format!("admin-guide-{j}-groups").into()))
                .test_support()
                .role(Role::RadioGroup)
                .aria_label(t!("admin-guide-group-for", channel = channel.as_str()))
                .flex()
                .flex_wrap()
                .gap_1p5();
            let choices =
                std::iter::once((String::new(), t!("admin-guide-more-channels").to_string()))
                    .chain(groups.iter().map(|g| (g.clone(), g.clone())));
            for (k, (value, label)) in choices.enumerate() {
                let on = current == value;
                in_group = in_group.child(pill(
                    format!("admin-guide-{j}-group-{k}"),
                    label.clone(),
                    label,
                    on,
                    &theme,
                    cx.listener(move |this, _, _, cx| {
                        if let Some(e) = this.draft.entries.get_mut(j) {
                            e.group = Some(value.clone());
                        }
                        cx.notify();
                    }),
                ));
            }
            let warning = a::warning(saved, &channel).and_then(warning_text);
            let mut item = div().child(
                div()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .child(
                        div()
                            .flex_1()
                            .truncate()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.text)
                            .child(channel.clone()),
                    )
                    .when(j > 0, |d| {
                        d.child(ui::icon::button(
                            ElementId::Name(format!("admin-guide-{j}-up").into()),
                            IconName::ArrowUp,
                            t!("admin-guide-move-up", name = channel.as_str()),
                            &theme,
                            cx.listener(move |this, _, _, cx| this.shift_entry(j, -1, cx)),
                        ))
                    })
                    .when(j + 1 < count, |d| {
                        d.child(ui::icon::button(
                            ElementId::Name(format!("admin-guide-{j}-down").into()),
                            IconName::ArrowDown,
                            t!("admin-guide-move-down", name = channel.as_str()),
                            &theme,
                            cx.listener(move |this, _, _, cx| this.shift_entry(j, 1, cx)),
                        ))
                    })
                    .child(ui::icon::button(
                        ElementId::Name(format!("admin-guide-{j}-remove").into()),
                        IconName::Close,
                        t!("admin-guide-remove", name = channel.as_str()),
                        &theme,
                        cx.listener(move |this, _, _, cx| this.remove_entry(j, cx)),
                    )),
            );
            if let Some(text) = warning {
                item = item.child(
                    div()
                        .id(ElementId::Name(format!("admin-guide-{j}-warning").into()))
                        .test_support()
                        .role(Role::Status)
                        .aria_label(text)
                        .text_xs()
                        .text_color(theme.warning)
                        .child(text),
                );
            }
            item = item.child(states).child(in_group);
            if let Some(input) = self.description_inputs.get(j) {
                item = item.child(
                    Input::new(input)
                        .id(ElementId::Name(
                            format!("admin-guide-{j}-description").into(),
                        ))
                        .aria_label(t!(
                            "admin-guide-description-for",
                            channel = channel.as_str()
                        )),
                );
            }
            list = list.child(card(
                format!("admin-guide-{j}"),
                channel.clone(),
                &theme,
                item,
            ));
        }
        body = body.child(list);

        let typed = self.add_channel.read(cx).value().to_string();
        let add = div()
            .flex()
            .items_center()
            .gap_2()
            .child(
                div().flex_1().child(
                    Input::new(&self.add_channel)
                        .id("admin-guide-add-input")
                        .aria_label(t!("admin-guide-add-channel")),
                ),
            )
            .child(button(
                "admin-guide-add",
                t!("admin-guide-add"),
                None,
                Kind::Secondary,
                !typed.trim().is_empty() && count < MAX_ENTRIES,
                &theme,
                cx.listener(move |this, _, window, cx| {
                    let typed = this.add_channel.read(cx).value().to_string();
                    this.add_channel(&typed, window, cx)
                }),
            ));
        body = body.child(add);
        if !self.directory.is_empty() {
            let mut found = div()
                .id("admin-directory")
                .test_support()
                .role(Role::ListBox)
                .aria_label(t!("admin-guide-add-channel"))
                .flex()
                .flex_col()
                .gap_0p5();
            for (k, d) in self.directory.iter().enumerate() {
                let listed = self
                    .draft
                    .entries
                    .iter()
                    .any(|e| ch::guide_key(&e.channel) == ch::guide_key(&d.name));
                let users = if d.users == 1 {
                    t!("admin-guide-members-one").to_string()
                } else {
                    t!("admin-guide-members", count = a::count(d.users))
                };
                let name = d.name.clone();
                let hover = theme.hover;
                found = found.child(
                    div()
                        .id(ElementId::Name(format!("admin-directory-{k}").into()))
                        .test_support()
                        .role(Role::ListBoxOption)
                        .aria_label(format!("{}, {users}", d.name))
                        .flex()
                        .items_center()
                        .gap_2()
                        .px_2()
                        .py_1p5()
                        .rounded_sm()
                        .text_sm()
                        .text_color(theme.muted)
                        .when(listed, |d| d.opacity(0.5))
                        .child(
                            div()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(theme.text)
                                .child(d.name.clone()),
                        )
                        .child(
                            div()
                                .flex_1()
                                .truncate()
                                .text_color(theme.faint)
                                .child(d.topic.clone().unwrap_or_default()),
                        )
                        .child(div().text_xs().text_color(theme.faint).child(users))
                        .when(!listed, |d| {
                            d.cursor_pointer()
                                .hover(move |d| d.bg(hover))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.add_channel(&name, window, cx)
                                }))
                        }),
                );
            }
            body = body.child(found);
        }
        group(
            "admin-guide-channels",
            t!("admin-guide-channels"),
            body,
            &theme,
        )
        .into_any_element()
    }

    fn preview(&self, theme: &Theme) -> AnyElement {
        let (groups, channels) = self.draft.preview();
        let body = if channels.is_empty() {
            notice(
                "admin-preview-empty",
                t!("admin-guide-preview-empty"),
                false,
                theme,
            )
            .into_any_element()
        } else {
            let ticked = ch::initial_selection(&channels, &Default::default());
            let mut list = div()
                .id("admin-preview")
                .test_support()
                .role(Role::List)
                .aria_label(t!("admin-guide-preview"))
                .p_2()
                .rounded_lg()
                .bg(theme.raised)
                .border_1()
                .border_color(theme.border)
                .flex()
                .flex_col()
                .gap_2();
            let mut n = 0;
            for section in ch::guide_sections(&groups, &channels) {
                let title = match &section.kind {
                    SectionKind::Everyone => t!("admin-guide-for-everyone").to_string(),
                    SectionKind::Group(name) => name.clone(),
                    SectionKind::More => t!("admin-guide-more-channels").to_string(),
                };
                let mut part = div().flex().flex_col().gap_0p5().child(
                    div()
                        .px_1()
                        .text_xs()
                        .font_weight(FontWeight::BOLD)
                        .text_color(theme.muted)
                        .child(title.to_uppercase()),
                );
                for channel in &section.channels {
                    let on = channel.state == c::ChannelGuideChannelState::Everyone
                        || ticked.contains(&ch::guide_key(&channel.channel));
                    let label = if on {
                        t!(
                            "admin-guide-preview-ticked",
                            channel = channel.channel.as_str()
                        )
                    } else {
                        channel.channel.clone()
                    };
                    part = part.child(
                        div()
                            .id(ElementId::Name(format!("admin-preview-{n}").into()))
                            .test_support()
                            .role(Role::ListItem)
                            .aria_label(label)
                            .flex()
                            .items_center()
                            .gap_2()
                            .px_1()
                            .py_0p5()
                            .text_sm()
                            .child(
                                div()
                                    .size(px(14.))
                                    .rounded_sm()
                                    .border_1()
                                    .border_color(theme.control_border)
                                    .when(on, |d| d.bg(theme.accent)),
                            )
                            .child(
                                div()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(theme.text)
                                    .child(channel.channel.clone()),
                            )
                            .when_some(channel.description.clone(), |d, about| {
                                d.child(
                                    div()
                                        .flex_1()
                                        .truncate()
                                        .text_color(theme.muted)
                                        .child(about),
                                )
                            }),
                    );
                    n += 1;
                }
                list = list.child(part);
            }
            list.into_any_element()
        };
        group(
            "admin-guide-preview",
            t!("admin-guide-preview"),
            div()
                .flex()
                .flex_col()
                .gap_2()
                .child(hint(t!("admin-guide-preview-help"), theme))
                .child(body),
            theme,
        )
        .into_any_element()
    }
}

impl Render for WelcomePanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_inputs(window, cx);
        let theme = theme::get(cx).clone();
        let mut body = div()
            .flex()
            .flex_col()
            .gap_6()
            .child(hint(t!("admin-guide-intro"), &theme));
        if self.loading && self.saved.is_none() {
            body = body.child(hint(t!("admin-guide-loading"), &theme));
        }
        if let Some(error) = &self.error {
            let text = if self.conflict {
                t!("admin-guide-conflict").to_string()
            } else {
                error.clone()
            };
            body = body.child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().flex_1().min_w_0().child(notice(
                        "admin-guide-error",
                        text,
                        true,
                        &theme,
                    )))
                    .when(self.conflict, |d| {
                        d.child(button(
                            "admin-guide-reload",
                            t!("admin-guide-reload"),
                            None,
                            Kind::Ghost,
                            true,
                            &theme,
                            cx.listener(|this, _, _, cx| this.load(cx)),
                        ))
                    }),
            );
        }
        let Some(saved) = self.saved.clone() else {
            return body;
        };
        if let Some(requests) = self.requests(&saved, cx) {
            body = body.child(requests);
        }
        body = body
            .child(self.groups(cx))
            .child(self.channels(&saved, cx))
            .child(self.preview(&theme));

        let dirty = self.draft.dirty(&saved);
        let mut bar = div().flex().items_center().justify_end().gap_3().py_2();
        if let Some(text) = &self.notice {
            bar = bar.child(div().mr_auto().child(notice(
                "admin-guide-notice",
                text.clone(),
                false,
                &theme,
            )));
        } else if let (Some(at), false) = (&saved.updated_at, dirty) {
            bar = bar.child(div().mr_auto().child(hint(
                t!(
                    "admin-guide-last-saved",
                    account = saved.updated_by.as_deref().unwrap_or_default(),
                    time = when(at)
                ),
                &theme,
            )));
        }
        bar = bar.child(button(
            "admin-guide-save",
            if self.saving {
                t!("admin-guide-saving")
            } else {
                t!("admin-guide-save")
            },
            None,
            Kind::Primary,
            (dirty || !saved.saved) && !self.saving,
            &theme,
            cx.listener(|this, _, _, cx| this.save(t!("admin-guide-saved").to_string(), cx)),
        ));
        body.child(bar)
    }
}

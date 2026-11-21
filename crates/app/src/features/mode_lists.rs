//! Ban, exception, invite-exception and quiet lists with the mask builder
//! (package CH2), as the web client's `ChannelModeList.vue` and
//! `MaskPersonPicker.vue` draw them: one tab of the channel settings per
//! list mode, with its entries, removing one (asked first), and adding one.
//!
//! A mask is built from someone picked with the shared people picker: their
//! nick, username, host or account, ticked; the mask field (under Advanced)
//! stays the one source of truth, so typing a mask by hand and ticking
//! boxes always agree. Before anything is added the app says what the mask
//! becomes, what it covers, how broad it is and who here it matches now.
//! The rules are `ircord_client::channel_modes`, tested there.

use std::rc::Rc;
use std::time::Duration;

use gpui_kit::TestSupportExt;
use gpui_kit::component::Disableable;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::*;
use gpui_kit::*;
use ircord_client::Client;
use ircord_client::api::people::Purpose;
use ircord_client::channel_modes::{
    self as modes, Broad, Extbans, MaskError, MaskPart, MaskPerson, Part, PartKind, Scope, Warning,
};
use ircord_client::contract as c;

use crate::features::channel_settings::{Here, date, heading, here, hint, list_label, send};
use crate::session::{Session, SessionEvent};
use crate::theme::Theme;
use crate::ui::people_picker::{PeoplePicker, Picked, Picker};
use crate::{theme, ui};

/// Mode changes arrive as events; the list is read again shortly after one is sent.
const REFRESH: Duration = Duration::from_millis(400);

pub fn init(_cx: &mut App) {
    // Drawn inside the channel settings (`channel_settings.rs`).
}

enum Load<T> {
    Loading,
    Ready(T),
    Failed(SharedString),
}

/// The person a mask is being built from.
#[derive(Clone)]
struct Chosen {
    person: MaskPerson,
}

pub struct ModeListView {
    session: Entity<Session>,
    conversation: String,
    mode: char,
    entries: Load<Vec<c::ChannelListEntry>>,
    round: u64,
    mask: Entity<InputState>,
    picker: Entity<PeoplePicker>,
    chosen: Option<Chosen>,
    /// The mask field shows (it folds away while building from someone).
    advanced: bool,
    examples: bool,
    _subscriptions: Vec<Subscription>,
}

impl ModeListView {
    pub fn new(
        session: Entity<Session>,
        conversation: String,
        mode: char,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mask = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t!("channel-settings-mask-placeholder"))
        });
        let network = here(&session, &conversation, cx)
            .map(|h| h.network_id().to_string())
            .unwrap_or_default();
        let this = cx.entity().downgrade();
        let picker = cx.new(|cx| {
            PeoplePicker::new(
                &Picker {
                    title: t!("channel-settings-mask-find").into(),
                    session: session.clone(),
                    network,
                    purpose: Purpose::Mask,
                    conversation: Some(conversation.clone()),
                },
                Rc::new(move |picked: &Picked, window, cx| {
                    this.update(cx, |v, cx| v.choose(picked, window, cx)).ok();
                }),
                window,
                cx,
            )
        });
        let subscriptions = vec![
            cx.subscribe_in(
                &mask,
                window,
                |this: &mut Self, _, event: &InputEvent, window, cx| match event {
                    InputEvent::Change => cx.notify(),
                    InputEvent::PressEnter { .. } => this.add(window, cx),
                    _ => {}
                },
            ),
            cx.subscribe(&session, |_, _, event: &SessionEvent, cx| {
                if let SessionEvent::Changed(_) = event {
                    cx.notify();
                }
            }),
        ];
        let mut view = Self {
            session,
            conversation,
            mode,
            entries: Load::Loading,
            round: 0,
            mask,
            picker,
            chosen: None,
            advanced: false,
            examples: false,
            _subscriptions: subscriptions,
        };
        view.load(Duration::ZERO, cx);
        view
    }

    fn client(&self, cx: &App) -> Option<Rc<Client>> {
        self.session.read(cx).client().cloned()
    }

    fn load(&mut self, delay: Duration, cx: &mut Context<Self>) {
        let Some(client) = self.client(cx) else {
            return;
        };
        self.round += 1;
        let round = self.round;
        let (conversation, mode) = (self.conversation.clone(), self.mode);
        let task = client.request(move |api| async move {
            if !delay.is_zero() {
                tokio::time::sleep(delay).await;
            }
            api.mode_list(&conversation, mode).await
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                if this.round != round {
                    return;
                }
                this.entries = match result {
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

    fn typed(&self, cx: &App) -> String {
        self.mask.read(cx).value().to_string()
    }

    fn set_mask(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        self.mask.update(cx, |s, cx| s.set_value(text, window, cx));
        cx.notify();
    }

    /// Someone was picked: start from their default mask.
    fn choose(&mut self, picked: &Picked, window: &mut Window, cx: &mut Context<Self>) {
        let member = self.client(cx).and_then(|client| {
            let w = client.read();
            picked
                .member
                .as_ref()
                .and_then(|id| w.members.get(id))
                .cloned()
        });
        let person = MaskPerson {
            nick: picked.nick.clone(),
            username: member.as_ref().and_then(|m| m.username.clone()),
            host: member.as_ref().and_then(|m| m.host.clone()),
            account: picked
                .account
                .clone()
                .or_else(|| member.as_ref().map(|m| m.account.clone())),
        };
        let prefix = self.account_prefix(cx);
        let mask = modes::default_mask(&person, prefix.as_deref());
        self.chosen = Some(Chosen { person });
        self.set_mask(mask, window, cx);
    }

    fn clear_person(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.chosen = None;
        self.set_mask(String::new(), window, cx);
        let input = self.picker.read(cx).input.clone();
        input.update(cx, |s, cx| s.set_value("", window, cx));
        input.focus_handle(cx).focus(window, cx);
    }

    fn toggle_part(
        &mut self,
        part: MaskPart,
        on: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(chosen) = self.chosen.clone() else {
            return;
        };
        let prefix = self.account_prefix(cx);
        let mask =
            modes::toggled_mask(&self.typed(cx), &chosen.person, part, on, prefix.as_deref());
        self.set_mask(mask, window, cx);
    }

    fn extbans(&self, cx: &App) -> Option<Extbans> {
        here(&self.session, &self.conversation, cx).and_then(|h| Extbans::parse(&h.isupport()))
    }

    fn account_prefix(&self, cx: &App) -> Option<String> {
        modes::account_prefix(self.extbans(cx).as_ref())
    }

    fn use_example(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        self.chosen = None;
        self.advanced = true;
        self.set_mask(text, window, cx);
        self.mask.focus_handle(cx).focus(window, cx);
    }

    fn add(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(h) = here(&self.session, &self.conversation, cx) else {
            return;
        };
        if !h.rights.ban {
            return;
        }
        let preview = modes::preview_mask(&self.typed(cx), self.extbans(cx).as_ref());
        if preview.mask.is_empty() || preview.error.is_some() {
            return;
        }
        let line = modes::list_line(&h.target(), self.mode, &preview.mask, true);
        // Not sent (disconnected): keep what was typed to add it later.
        if !send(&self.session, &self.conversation, line, window, cx) {
            return;
        }
        self.chosen = None;
        self.set_mask(String::new(), window, cx);
        self.load(REFRESH, cx);
    }

    fn remove(&mut self, mask: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(h) = here(&self.session, &self.conversation, cx) else {
            return;
        };
        let this = cx.entity().downgrade();
        let (session, conversation, mode) =
            (self.session.clone(), self.conversation.clone(), self.mode);
        let target = h.target();
        ui::confirm::confirm(
            window,
            cx,
            ui::confirm::Confirm {
                title: t!("channel-settings-list-remove-title", entry = singular(mode)).into(),
                body: mask.clone().into(),
                ok: t!("channel-settings-list-remove").into(),
                danger: false,
            },
            move |window, cx| {
                let line = modes::list_line(&target, mode, &mask, false);
                if send(&session, &conversation, line, window, cx) {
                    this.update(cx, |v, cx| v.load(REFRESH, cx)).ok();
                }
            },
        );
    }

    // ---- Drawing ----

    fn render_entries(&self, h: &Here, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        match &self.entries {
            Load::Loading => hint(t!("channel-settings-list-loading"), theme).into_any_element(),
            Load::Failed(e) => div()
                .id("channel-settings-list-error")
                .test_support()
                .role(Role::Alert)
                .aria_label(e.clone())
                .text_sm()
                .text_color(theme.danger)
                .child(t!("channel-settings-list-failed", error = e.clone()))
                .into_any_element(),
            Load::Ready(items) if items.is_empty() => div()
                .id("channel-settings-list-empty")
                .test_support()
                .role(Role::Status)
                .aria_label(t!("channel-settings-list-empty"))
                .text_sm()
                .text_color(theme.muted)
                .child(t!("channel-settings-list-empty"))
                .into_any_element(),
            Load::Ready(items) => {
                let mut list = div()
                    .id("channel-settings-list-entries")
                    .test_support()
                    .role(Role::List)
                    .aria_label(list_label(self.mode))
                    .flex()
                    .flex_col()
                    .gap_1();
                for (i, entry) in items.iter().enumerate() {
                    let mut meta = Vec::new();
                    if let Some(by) = entry.set_by.as_deref().filter(|b| !b.is_empty()) {
                        meta.push(t!("channel-settings-list-set-by", person = by));
                    }
                    if let Some(at) = entry.set_at.as_deref().map(date).filter(|d| !d.is_empty()) {
                        meta.push(at);
                    }
                    let mask = entry.mask.clone();
                    list = list.child(
                        div()
                            .id(ElementId::Name(
                                format!("channel-settings-list-entry-{i}").into(),
                            ))
                            .test_support()
                            .role(Role::ListItem)
                            .aria_label(entry.mask.clone())
                            .flex()
                            .items_center()
                            .gap_3()
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .bg(theme.panel)
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .flex_1()
                                    .min_w_0()
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_family("monospace")
                                            .truncate()
                                            .child(entry.mask.clone()),
                                    )
                                    .when(!meta.is_empty(), |d| {
                                        d.child(hint(meta.join(" · "), theme))
                                    }),
                            )
                            .when(h.rights.ban, |d| {
                                d.child(
                                    Button::new(ElementId::Name(
                                        format!("channel-settings-list-remove-{i}").into(),
                                    ))
                                    .ghost()
                                    .label(t!("channel-settings-list-remove"))
                                    .accessibility_label(t!(
                                        "channel-settings-list-remove-mask",
                                        mask = mask.clone()
                                    ))
                                    .on_click(cx.listener(
                                        move |this, _, window, cx| {
                                            this.remove(mask.clone(), window, cx)
                                        },
                                    )),
                                )
                            }),
                    );
                }
                list.into_any_element()
            }
        }
    }

    fn render_builder(&self, theme: &Theme, cx: &mut Context<Self>) -> Div {
        let Some(chosen) = &self.chosen else {
            return div()
                .flex()
                .flex_col()
                .gap_2()
                .child(hint(t!("channel-settings-mask-choose"), theme))
                .child(self.picker.clone());
        };
        let p = &chosen.person;
        let prefix = self.account_prefix(cx);
        let typed = self.typed(cx);
        let parts = modes::parts_of(&typed, p, prefix.as_deref());
        let unknown = t!("channel-settings-mask-unknown");
        let mut options: Vec<(MaskPart, &'static str, Option<String>, &'static str)> = vec![
            (
                MaskPart::Nick,
                t!("channel-settings-mask-nick"),
                Some(p.nick.clone()),
                unknown,
            ),
            (
                MaskPart::User,
                t!("channel-settings-mask-username"),
                p.username.clone().filter(|u| !u.is_empty()),
                unknown,
            ),
            (
                MaskPart::Host,
                t!("channel-settings-mask-host"),
                p.host.clone().filter(|h| !h.is_empty()),
                unknown,
            ),
        ];
        if prefix.is_some() {
            let tracked = here(&self.session, &self.conversation, cx).is_some_and(|h| {
                h.network
                    .as_ref()
                    .is_some_and(|n| n.capabilities.iter().any(|c| c == "account-notify"))
            });
            options.push((
                MaskPart::Account,
                t!("channel-settings-mask-account"),
                p.known_account().map(str::to_string),
                if tracked {
                    t!("channel-settings-mask-not-logged-in")
                } else {
                    t!("channel-settings-mask-not-reported")
                },
            ));
        }
        let mut boxes = div()
            .id("channel-settings-mask-parts")
            .test_support()
            .role(Role::Group)
            .aria_label(t!("channel-settings-mask-match-by", nick = p.nick.clone()))
            .flex()
            .flex_col()
            .gap_1();
        for (part, label, value, missing) in options {
            let on = parts.get(part);
            // The last ticked box stays, or the builder would make `*!*@*`.
            let last = on && parts.count() == 1;
            let key = match part {
                MaskPart::Nick => "nick",
                MaskPart::User => "user",
                MaskPart::Host => "host",
                MaskPart::Account => "account",
            };
            boxes = boxes.child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        Checkbox::new(ElementId::Name(
                            format!("channel-settings-mask-part-{key}").into(),
                        ))
                        .checked(on)
                        .disabled(value.is_none() || last)
                        .accessibility_label(label)
                        .on_click(cx.listener(
                            move |this, on: &bool, window, cx| {
                                this.toggle_part(part, *on, window, cx)
                            },
                        )),
                    )
                    .child(div().text_sm().w(px(90.)).child(label))
                    .child(match value {
                        Some(v) => div().text_sm().font_family("monospace").child(v),
                        None => div().text_sm().text_color(theme.faint).child(missing),
                    }),
            );
        }
        let custom = !typed.trim().is_empty() && parts.count() == 0;
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(t!("channel-settings-mask-match-by", nick = p.nick.clone())),
                    )
                    .child(
                        Button::new("channel-settings-mask-clear-person")
                            .ghost()
                            .label(t!("channel-settings-mask-clear"))
                            .accessibility_label(t!(
                                "channel-settings-mask-clear-person",
                                nick = p.nick.clone()
                            ))
                            .on_click(
                                cx.listener(|this, _, window, cx| this.clear_person(window, cx)),
                            ),
                    ),
            )
            .child(boxes)
            .child(hint(t!("channel-settings-mask-unchecked"), theme))
            .when(prefix.is_some(), |d| {
                d.child(hint(t!("channel-settings-mask-account-alone"), theme))
            })
            .when(custom, |d| {
                d.child(hint(
                    t!("channel-settings-mask-custom", nick = p.nick.clone()),
                    theme,
                ))
            })
    }

    fn render_preview(
        &self,
        h: &Here,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let typed = self.typed(cx);
        if typed.trim().is_empty() {
            return None;
        }
        let preview = modes::preview_mask(&typed, self.extbans(cx).as_ref());
        if let Some(error) = preview.error {
            let text = match error {
                MaskError::Spaces => t!("channel-settings-mask-error-spaces"),
                MaskError::Prefix => t!("channel-settings-mask-error-prefix"),
            };
            return Some(
                div()
                    .id("channel-settings-mask-preview")
                    .test_support()
                    .role(Role::Alert)
                    .aria_label(text)
                    .text_sm()
                    .text_color(theme.danger)
                    .child(text)
                    .into_any_element(),
            );
        }
        let scope = preview.scope.as_ref().map(scope_text).unwrap_or_default();
        let line = if preview.expanded {
            t!(
                "channel-settings-mask-adds-expanded",
                mask = preview.mask.clone(),
                typed = typed.trim(),
                scope = scope
            )
        } else {
            t!(
                "channel-settings-mask-adds",
                mask = preview.mask.clone(),
                scope = scope
            )
        };
        let mut out = div().flex().flex_col().gap_1().child(
            div()
                .id("channel-settings-mask-preview")
                .test_support()
                .role(Role::Status)
                .aria_label(line.trim().to_string())
                .text_sm()
                .child(line.trim().to_string()),
        );
        for (i, warning) in preview.warnings.iter().enumerate() {
            let text = warning_text(warning);
            out = out.child(
                div()
                    .id(ElementId::Name(
                        format!("channel-settings-mask-warning-{i}").into(),
                    ))
                    .test_support()
                    .role(Role::Note)
                    .aria_label(text.clone())
                    .text_xs()
                    .text_color(theme.warning)
                    .child(text),
            );
        }
        if preview.extban.is_none() {
            let here = matched_here(&self.session, h, &preview.mask, cx);
            let text = match here.len() {
                0 => t!("channel-settings-mask-matches-none").to_string(),
                1 => t!("channel-settings-mask-matches-one", people = names(&here)),
                n => t!(
                    "channel-settings-mask-matches-many",
                    count = n,
                    people = names(&here)
                ),
            };
            out = out.child(
                div()
                    .id("channel-settings-mask-matches")
                    .test_support()
                    .role(Role::Status)
                    .aria_label(text.clone())
                    .text_xs()
                    .text_color(theme.muted)
                    .child(text),
            );
        }
        Some(out.into_any_element())
    }

    fn render_examples(&self, theme: &Theme, cx: &mut Context<Self>) -> Div {
        let toggle = Button::new("channel-settings-mask-how")
            .ghost()
            .label(t!("channel-settings-mask-how"))
            .on_click(cx.listener(|this, _, _, cx| {
                this.examples = !this.examples;
                cx.notify();
            }));
        let mut out = div()
            .flex()
            .flex_col()
            .gap_2()
            .child(div().flex().child(toggle));
        if !self.examples {
            return out;
        }
        let extbans = self.extbans(cx);
        let account = modes::account_prefix(extbans.as_ref());
        let mute = extbans.as_ref().and_then(Extbans::mute);
        let network = here(&self.session, &self.conversation, cx)
            .map(|h| h.network_name())
            .unwrap_or_default();
        let mut examples: Vec<(String, &'static str)> = vec![
            ("alice".into(), t!("channel-settings-mask-example-nick")),
            (
                "*!*@203.0.113.7".into(),
                t!("channel-settings-mask-example-address"),
            ),
            (
                "*!*@*.example.net".into(),
                t!("channel-settings-mask-example-host"),
            ),
            (
                "*!*alice@*".into(),
                t!("channel-settings-mask-example-username"),
            ),
        ];
        if let Some(a) = &account {
            examples.push((
                format!("{a}alice"),
                t!("channel-settings-mask-example-account"),
            ));
        }
        if mute.is_some() && self.mode == 'b' {
            examples.push(("m:alice".into(), t!("channel-settings-mask-example-mute")));
        }
        out = out
            .child(hint(t!("channel-settings-mask-three-parts"), theme))
            .child(hint(t!("channel-settings-mask-nick-about"), theme))
            .child(hint(t!("channel-settings-mask-user-about"), theme))
            .child(hint(t!("channel-settings-mask-host-about"), theme))
            .child(hint(t!("channel-settings-mask-wildcards"), theme))
            .child(hint(
                match &account {
                    Some(a) => t!(
                        "channel-settings-mask-accounts-supported",
                        network = network,
                        mask = format!("{a}name")
                    ),
                    None => t!(
                        "channel-settings-mask-accounts-unsupported",
                        network = network
                    ),
                },
                theme,
            ));
        let mut list = div()
            .id("channel-settings-mask-examples")
            .test_support()
            .role(Role::List)
            .aria_label(t!("channel-settings-mask-how"))
            .flex()
            .flex_col()
            .gap_1();
        for (i, (text, about)) in examples.into_iter().enumerate() {
            let fill = text.clone();
            list = list.child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        Button::new(ElementId::Name(
                            format!("channel-settings-mask-example-{i}").into(),
                        ))
                        .ghost()
                        .label(text.clone())
                        .accessibility_label(t!(
                            "channel-settings-mask-use-example",
                            example = text
                        ))
                        .on_click(cx.listener(
                            move |this, _, window, cx| this.use_example(fill.clone(), window, cx),
                        )),
                    )
                    .child(hint(about, theme)),
            );
        }
        out.child(list)
            .child(hint(t!("channel-settings-mask-example-only"), theme))
    }
}

impl Render for ModeListView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme::get(cx).clone();
        let Some(h) = here(&self.session, &self.conversation, cx) else {
            return div().into_any_element();
        };
        let mode = self.mode;
        let mut top = div().flex().flex_col().gap_1();
        top = top.child(hint(list_about(mode), &theme));
        if let Some(overrides) = list_overrides(mode) {
            top = top.child(hint(overrides, &theme));
        }
        if mode == 'I' && !h.conversation.modes.contains('i') {
            top = top.child(hint(t!("channel-settings-list-invite-only-off"), &theme));
        }
        let page = div()
            .id("channel-settings-list")
            .test_support()
            .role(Role::Region)
            .aria_label(list_label(mode))
            .flex()
            .flex_col()
            .gap_4()
            .child(top)
            .child(self.render_entries(&h, &theme, cx));
        if !h.rights.ban {
            return page.into_any_element();
        }
        let typed = self.typed(cx);
        let preview = modes::preview_mask(&typed, self.extbans(cx).as_ref());
        let can_add = !preview.mask.is_empty() && preview.error.is_none();
        let show_field = self.advanced || self.chosen.is_none() && !typed.is_empty();
        let mut form = div()
            .id("channel-settings-list-add")
            .test_support()
            .role(Role::Form)
            .aria_label(t!("channel-settings-list-add", entry = singular(mode)))
            .flex()
            .flex_col()
            .gap_3()
            .p_3()
            .rounded_lg()
            .border_1()
            .border_color(theme.border)
            .child(heading(
                t!("channel-settings-list-add", entry = singular(mode)),
                &theme,
            ))
            .child(self.render_builder(&theme, cx))
            .child(
                div().flex().child(
                    Button::new("channel-settings-mask-advanced")
                        .ghost()
                        .label(t!("channel-settings-mask-advanced"))
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.advanced = !this.advanced;
                            if this.advanced {
                                this.mask.focus_handle(cx).focus(window, cx);
                            }
                            cx.notify();
                        })),
                ),
            );
        if show_field {
            form = form
                .child(ui::field::field(
                    t!("channel-settings-mask-for", entry = singular(mode)),
                    Input::new(&self.mask)
                        .id("channel-settings-mask")
                        .aria_label(t!("channel-settings-mask-for", entry = singular(mode))),
                    &theme,
                ))
                .child(hint(t!("channel-settings-mask-short-help"), &theme));
        }
        form = form
            .children(self.render_preview(&h, &theme, cx))
            .child(
                div().flex().child(
                    Button::new("channel-settings-list-add-button")
                        .primary()
                        .label(t!("channel-settings-list-add", entry = singular(mode)))
                        .disabled(!can_add)
                        .on_click(cx.listener(|this, _, window, cx| this.add(window, cx))),
                ),
            )
            .child(self.render_examples(&theme, cx));
        page.child(form).into_any_element()
    }
}

/// One entry of a list, in the middle of a sentence: "ban", "quiet".
fn singular(mode: char) -> &'static str {
    match mode {
        'b' => t!("channel-settings-list-b-one"),
        'e' => t!("channel-settings-list-e-one"),
        'I' => t!("channel-settings-list-upper-i-one"),
        'q' => t!("channel-settings-list-q-one"),
        _ => t!("channel-settings-list-entry"),
    }
}

fn list_about(mode: char) -> &'static str {
    match mode {
        'b' => t!("channel-settings-list-b-about"),
        'e' => t!("channel-settings-list-e-about"),
        'I' => t!("channel-settings-list-upper-i-about"),
        'q' => t!("channel-settings-list-q-about"),
        _ => t!("channel-settings-list-other-about"),
    }
}

/// What a list does and does not override, so bans, ban exceptions and
/// invite exceptions do not read as the same list.
fn list_overrides(mode: char) -> Option<&'static str> {
    match mode {
        'b' => Some(t!("channel-settings-list-b-overrides")),
        'e' => Some(t!("channel-settings-list-e-overrides")),
        'I' => Some(t!("channel-settings-list-upper-i-overrides")),
        _ => None,
    }
}

fn part_text(part: &Part) -> String {
    let value = part.value.clone();
    match (part.kind, part.pattern) {
        (PartKind::Nick, false) => t!("channel-settings-part-nick", value = value),
        (PartKind::Nick, true) => t!("channel-settings-part-nick-pattern", value = value),
        (PartKind::User, false) => t!("channel-settings-part-user", value = value),
        (PartKind::User, true) => t!("channel-settings-part-user-pattern", value = value),
        (PartKind::Host, false) => t!("channel-settings-part-host", value = value),
        (PartKind::Host, true) => t!("channel-settings-part-host-pattern", value = value),
    }
}

/// `anyone with the nick alice and the host h`, or `everyone`.
fn parts_text(parts: &[Part]) -> String {
    let words: Vec<String> = parts.iter().map(part_text).collect();
    let joined = match words.as_slice() {
        [] => return t!("channel-settings-part-everyone").to_string(),
        [one] => one.clone(),
        [first, second] => t!(
            "channel-settings-part-join-two",
            first = first.clone(),
            second = second.clone()
        ),
        [rest @ .., last] => t!(
            "channel-settings-part-join-many",
            first = rest.join(", "),
            last = last.clone()
        ),
    };
    t!("channel-settings-part-anyone-with", parts = joined)
}

fn scope_text(scope: &Scope) -> String {
    match scope {
        Scope::Account { account, negated } => match (account.is_empty(), negated) {
            (false, false) => t!("channel-settings-scope-account", account = account.clone()),
            (false, true) => t!(
                "channel-settings-scope-account-not",
                account = account.clone()
            ),
            (true, false) => t!("channel-settings-scope-any-account").to_string(),
            (true, true) => t!("channel-settings-scope-any-account-not").to_string(),
        },
        Scope::Mute(parts) => t!("channel-settings-scope-mute", parts = parts_text(parts)),
        Scope::Extended { negated: false } => t!("channel-settings-scope-extended").to_string(),
        Scope::Extended { negated: true } => {
            t!("channel-settings-scope-extended-negated").to_string()
        }
        Scope::Matches(parts) => t!("channel-settings-scope-matches", parts = parts_text(parts)),
        Scope::MatchesForward(parts, channel) => t!(
            "channel-settings-scope-matches-forward",
            parts = parts_text(parts),
            channel = channel.clone()
        ),
    }
}

fn warning_text(warning: &Warning) -> String {
    match warning {
        Warning::Broad(Broad::Everyone) => t!("channel-settings-broad-everyone").to_string(),
        Warning::Broad(Broad::Host) => t!("channel-settings-broad-host").to_string(),
        Warning::Broad(Broad::Nick) => t!("channel-settings-broad-nick").to_string(),
        Warning::UnknownExtban => t!("channel-settings-mask-unknown-extban").to_string(),
        Warning::UnknownType(prefix) => {
            t!(
                "channel-settings-mask-unknown-type",
                prefix = prefix.clone()
            )
        }
        Warning::NoExtendedType(prefix) => {
            t!("channel-settings-mask-no-extended-type", prefix = prefix)
        }
    }
}

/// Who in the channel the mask matches now, you marked as you.
fn matched_here(session: &Entity<Session>, h: &Here, mask: &str, cx: &App) -> Vec<String> {
    let Some(client) = session.read(cx).client() else {
        return Vec::new();
    };
    let w = client.read();
    h.conversation
        .member_ids
        .iter()
        .filter_map(|id| {
            let m = w.members.get(id)?;
            let own = w.is_self(id, h.network_id());
            let nick = if own {
                h.network
                    .as_ref()
                    .map_or(m.nick.clone(), |n| n.nick.clone())
            } else {
                m.nick.clone()
            };
            modes::matches_person(mask, &nick, m.username.as_deref(), m.host.as_deref()).then(
                || {
                    if own {
                        t!("channel-settings-mask-you", nick = nick)
                    } else {
                        nick
                    }
                },
            )
        })
        .collect()
}

fn names(people: &[String]) -> String {
    let mut out = people
        .iter()
        .take(8)
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    if people.len() > 8 {
        out.push_str(", …");
    }
    out
}

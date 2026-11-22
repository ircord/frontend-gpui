//! Creating a channel (package CH1), as the web client's
//! `CreateChannelDialog.vue` does it: a name the server checks while you
//! type, a topic, a template (private, open, announcements) that sets the
//! switches, the common switches with the key and user limit, the rest
//! folded away, and a page listing what did not work when the server could
//! not set everything up. Typed changes ask before they are thrown away.
//!
//! The form's rules live in `ircord_client::api::channels::CreateForm`,
//! tested there; this file draws it. [`mode_text`] names channel modes for
//! the channel settings (CH2) too.

use crate::menu_order;
use crate::ui::focus::Reachable;
use std::collections::BTreeMap;
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
use ircord_client::api::channels::{self as ch, Availability, CreateForm, Template};
use ircord_client::contract as c;

use crate::registry::{self, MenuItem, NetworkCtx};
use crate::session::Session;
use crate::theme::Theme;
use crate::ui::icon::IconName;
use crate::{theme, ui};

/// How long typing must pause before the server checks the name.
const CHECK_DELAY: Duration = Duration::from_millis(300);

pub fn init(cx: &mut App) {
    registry::register(cx, |r| {
        r.network_menu.add(
            MenuItem::new(
                "network-create-channel",
                t!("channels-menu-create"),
                |n: &NetworkCtx, window, cx| open(&n.session, &n.network, window, cx),
            )
            .icon(IconName::Plus)
            .order(menu_order::network::CREATE_CHANNEL),
        );
    });
}

/// Open the create dialog for `network`.
pub fn open(session: &Entity<Session>, network: &str, window: &mut Window, cx: &mut App) {
    let Some(client) = session.read(cx).client().cloned() else {
        return;
    };
    if client.read().network(network).is_none() {
        return;
    }
    let session = session.clone();
    let network = network.to_string();
    let view = cx.new(|cx| CreateView::new(session, client, network, window, cx));
    let focus = std::cell::Cell::new(true);
    ui::dialog::open(
        window,
        cx,
        t!("channels-create-title"),
        move |dialog, window, cx| {
            if focus.replace(false) {
                let input = view.read(cx).name.clone();
                input.focus_handle(cx).focus(window, cx);
            }
            let guard = view.downgrade();
            dialog
                .w(px(560.))
                // Enter belongs to the fields: it creates from the name or
                // topic, and must not close the dialog as well.
                .on_ok(|_, _, _| false)
                .on_cancel(move |_, window, cx| {
                    let Some(view) = guard.upgrade() else {
                        return true;
                    };
                    let v = view.read(cx);
                    if v.busy {
                        false
                    } else if v.dirty() {
                        view.update(cx, |v, cx| v.ask_discard(window, cx));
                        false
                    } else {
                        true
                    }
                })
                .footer(view.update(cx, |v, cx| v.actions(cx)))
                .child(view.clone())
        },
    );
}

/// What the server answered when it could not do everything.
struct Partial {
    conversation: c::Conversation,
    warnings: Vec<String>,
}

pub struct CreateView {
    session: Entity<Session>,
    client: Rc<Client>,
    network: String,
    name: Entity<InputState>,
    topic: Entity<InputState>,
    /// Inputs for modes with a parameter, by letter.
    params: BTreeMap<char, Entity<InputState>>,
    form: CreateForm,
    initial: CreateForm,
    availability: Availability,
    round: u64,
    advanced: bool,
    busy: bool,
    error: Option<SharedString>,
    partial: Option<Partial>,
    _subscriptions: Vec<Subscription>,
}

impl CreateView {
    fn new(
        session: Entity<Session>,
        client: Rc<Client>,
        network: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let isupport = client
            .read()
            .network(&network)
            .map(|n| n.isupport.clone())
            .unwrap_or_default();
        let form = CreateForm::new(&isupport);
        let input = |masked: bool, window: &mut Window, cx: &mut Context<Self>| {
            cx.new(|cx| InputState::new(window, cx).masked(masked))
        };
        let name = input(false, window, cx);
        let topic = input(false, window, cx);
        let params: BTreeMap<char, Entity<InputState>> = form
            .modes
            .parameters()
            .into_iter()
            .map(|m| (m, input(m == 'k', window, cx)))
            .collect();
        let mut subscriptions = vec![
            cx.subscribe_in(
                &name,
                window,
                |this: &mut Self, _, event: &InputEvent, window, cx| match event {
                    InputEvent::Change => this.name_changed(cx),
                    InputEvent::PressEnter { .. } => this.submit(window, cx),
                    _ => {}
                },
            ),
            cx.subscribe_in(
                &topic,
                window,
                |this: &mut Self, _, event: &InputEvent, window, cx| match event {
                    InputEvent::Change => this.sync(cx),
                    InputEvent::PressEnter { .. } => this.submit(window, cx),
                    _ => {}
                },
            ),
        ];
        for input in params.values() {
            subscriptions.push(cx.subscribe(
                input,
                |this: &mut Self, _, event: &InputEvent, cx| {
                    if let InputEvent::Change = event {
                        this.sync(cx)
                    }
                },
            ));
        }
        Self {
            session,
            client,
            network,
            name,
            topic,
            params,
            initial: form.clone(),
            form,
            availability: Availability::Idle,
            round: 0,
            advanced: false,
            busy: false,
            error: None,
            partial: None,
            _subscriptions: subscriptions,
        }
    }

    fn dirty(&self) -> bool {
        self.partial.is_none() && self.form.dirty(&self.initial)
    }

    fn network(&self) -> Option<c::Network> {
        self.client.read().network(&self.network).cloned()
    }

    /// Copy the inputs into the form.
    fn sync(&mut self, cx: &mut Context<Self>) {
        self.form.name = self.name.read(cx).value().to_string();
        self.form.topic = self.topic.read(cx).value().to_string();
        for (mode, input) in &self.params {
            self.form
                .values
                .insert(*mode, input.read(cx).value().to_string());
        }
        cx.notify();
    }

    fn name_changed(&mut self, cx: &mut Context<Self>) {
        let before = self.form.channel();
        self.sync(cx);
        let Some(channel) = self.form.channel() else {
            self.round += 1;
            self.availability = Availability::Idle;
            return;
        };
        if before.as_deref() == Some(channel.as_str()) {
            return;
        }
        self.round += 1;
        let round = self.round;
        self.availability = Availability::Checking;
        let network = self.network.clone();
        let task = self.client.request(move |api| async move {
            tokio::time::sleep(CHECK_DELAY).await;
            api.channel_availability(&network, &channel).await
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                if this.round != round {
                    return;
                }
                this.availability = match result {
                    Ok(Ok(status)) => Availability::of(status),
                    _ => Availability::Unknown,
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn set_template(&mut self, template: Template, cx: &mut Context<Self>) {
        self.form.apply_template(template);
        cx.notify();
    }

    fn toggle(&mut self, mode: char, on: bool, cx: &mut Context<Self>) {
        self.form.toggle(mode, on);
        cx.notify();
    }

    fn can_submit(&self) -> bool {
        !self.busy
            && self.partial.is_none()
            && self.network().is_some_and(|n| ch::can_create(&n))
            && self.form.channel().is_some()
            && !matches!(
                self.availability,
                Availability::Unavailable | Availability::Checking
            )
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.sync(cx);
        if !self.can_submit() {
            return;
        }
        let Some(body) = self.form.body() else {
            return;
        };
        self.busy = true;
        self.error = None;
        cx.notify();
        let network = self.network.clone();
        let task = self
            .client
            .request(move |api| async move { api.create_channel(&network, &body).await });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| {
                this.busy = false;
                match result {
                    Ok(Ok(result)) => {
                        let conversation = result.conversation.clone();
                        this.client
                            .change(|w| ch::remember(w, conversation.clone()));
                        if ch::partial(&result) {
                            let warnings = result
                                .warnings
                                .iter()
                                .map(|w| this.form.redact_key(w))
                                .collect();
                            this.partial = Some(Partial {
                                conversation,
                                warnings,
                            });
                        } else {
                            this.open_channel(&conversation.id, window, cx);
                        }
                    }
                    Ok(Err(e)) => this.error = Some(this.form.redact_key(&e.to_string()).into()),
                    Err(e) => this.error = Some(e.to_string().into()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn open_channel(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.session
            .update(cx, |s, cx| s.select_conversation(id, cx));
        ui::dialog::close(window, cx);
    }

    fn cancel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        if self.dirty() {
            self.ask_discard(window, cx)
        } else {
            ui::dialog::close(window, cx)
        }
    }

    fn ask_discard(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        ui::confirm::confirm(
            window,
            cx,
            ui::confirm::Confirm {
                title: t!("channels-discard-title").into(),
                body: t!("channels-discard-body").into(),
                ok: t!("channels-discard").into(),
                danger: true,
            },
            |window, cx| {
                // The question closes itself; then the form under it.
                window.defer(cx, ui::dialog::close);
            },
        );
    }

    fn render_partial(&self, partial: &Partial, theme: &Theme, cx: &mut Context<Self>) -> Div {
        let id = partial.conversation.id.clone();
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .id("create-channel-partial")
                    .test_support()
                    .role(Role::Status)
                    .aria_label(t!("channels-create-partial"))
                    .text_sm()
                    .child(t!("channels-create-partial")),
            )
            .when(!partial.warnings.is_empty(), |d| {
                let mut list = div()
                    .id("create-channel-warnings")
                    .test_support()
                    .role(Role::List)
                    .aria_label(t!("channels-create-warnings"))
                    .flex()
                    .flex_col()
                    .gap_1()
                    .pl_2();
                for (i, warning) in partial.warnings.iter().enumerate() {
                    list = list.child(
                        div()
                            .id(ElementId::NamedInteger(
                                "create-channel-warning".into(),
                                i as u64,
                            ))
                            .test_support()
                            .role(Role::ListItem)
                            .aria_label(warning.clone())
                            .text_sm()
                            .text_color(theme.muted)
                            .child(format!("• {warning}")),
                    );
                }
                d.child(list)
            })
            .child(
                div().flex().justify_end().child(
                    Button::new("create-channel-open")
                        .primary()
                        .label(t!("channels-create-open"))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.open_channel(&id, window, cx)
                        })),
                ),
            )
    }

    /// The error and the buttons, in the dialog's footer so they stay in
    /// view while the form scrolls.
    fn actions(&self, cx: &mut Context<Self>) -> Div {
        let theme = theme::get(cx).clone();
        let mut footer = div().w_full().flex().flex_col().gap_2();
        if self.partial.is_some() {
            return footer;
        }
        if let Some(error) = &self.error {
            footer = footer.child(
                div()
                    .id("create-channel-error")
                    .test_support()
                    .role(Role::Alert)
                    .aria_label(error.clone())
                    .text_sm()
                    .text_color(theme.danger)
                    .child(error.clone()),
            );
        }
        footer.child(
            div()
                .flex()
                .justify_end()
                .gap_2()
                .child(
                    Button::new("create-channel-cancel")
                        .ghost()
                        .label(t!("core-cancel"))
                        .disabled(self.busy)
                        .on_click(cx.listener(|this, _, window, cx| this.cancel(window, cx))),
                )
                .child(
                    Button::new("create-channel-submit")
                        .primary()
                        .label(if self.busy {
                            t!("channels-create-creating")
                        } else {
                            t!("channels-create-submit")
                        })
                        .disabled(!self.can_submit())
                        .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx))),
                ),
        )
    }

    fn mode_box(
        &self,
        mode: char,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let (label, about) = mode_text(mode);
        let title = format!("{label} (+{mode})");
        div()
            .w_full()
            .min_w_0()
            .flex()
            .items_start()
            .gap_2()
            .child(
                Checkbox::new(ElementId::Name(format!("create-mode-{mode}").into()))
                    .checked(self.form.is_on(mode))
                    .disabled(self.busy)
                    .accessibility_label(title.clone())
                    .on_click(
                        cx.listener(move |this, on: &bool, _, cx| this.toggle(mode, *on, cx)),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .text_sm()
                    .child(div().font_weight(FontWeight::BOLD).child(title))
                    .child(div().text_xs().text_color(theme.muted).child(about)),
            )
    }

    fn param_field(
        &self,
        mode: char,
        label: SharedString,
        help: Option<SharedString>,
        theme: &Theme,
    ) -> Option<Div> {
        let input = self.params.get(&mode)?;
        let id = match mode {
            'k' => SharedString::from("create-channel-key"),
            'l' => SharedString::from("create-channel-limit"),
            _ => format!("create-mode-param-{mode}").into(),
        };
        let control = div()
            .flex()
            .flex_col()
            .gap_1()
            .child(Input::new(input).id(id).aria_label(label.clone()))
            .children(help.map(|h| div().text_xs().text_color(theme.muted).child(h)));
        Some(ui::field::field(label, control, theme))
    }
}

impl Render for CreateView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme::get(cx).clone();
        let network = self.network();
        let network_name = network.as_ref().map(|n| n.name.clone()).unwrap_or_default();
        let supported = network.as_ref().is_some_and(ch::can_create);
        let mut root = div()
            .id("create-channel")
            .test_support()
            .role(Role::Group)
            .aria_label(t!("channels-create-title"))
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .text_sm()
                    .text_color(theme.muted)
                    .child(t!("channels-create-description", network = network_name)),
            );
        if !supported {
            root = root.child(
                div()
                    .id("create-channel-unsupported")
                    .test_support()
                    .role(Role::Status)
                    .aria_label(t!("channels-create-unsupported"))
                    .text_sm()
                    .text_color(theme.warning)
                    .child(t!("channels-create-unsupported")),
            );
        }
        if let Some(partial) = &self.partial {
            return root.child(self.render_partial(partial, &theme, cx));
        }

        // Name, with the server's check under it.
        let check = match self.availability {
            Availability::Idle => None,
            Availability::Checking => Some((t!("channels-create-checking"), false)),
            Availability::Available => Some((t!("channels-create-available"), false)),
            Availability::Unavailable => Some((t!("channels-create-unavailable"), true)),
            Availability::Unknown => Some((t!("channels-create-unknown"), false)),
        };
        let name = div()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                Input::new(&self.name)
                    .id("create-channel-name")
                    .aria_label(t!("channels-create-name")),
            )
            .children(check.map(|(text, bad)| {
                div()
                    .id("create-channel-name-note")
                    .test_support()
                    .role(Role::Status)
                    .aria_label(text)
                    .text_xs()
                    .text_color(if bad { theme.danger } else { theme.muted })
                    .child(text)
            }));
        root = root
            .child(ui::field::field(t!("channels-create-name"), name, &theme))
            .child(ui::field::field(
                t!("channels-create-topic"),
                Input::new(&self.topic)
                    .id("create-channel-topic")
                    .aria_label(t!("channels-create-topic")),
                &theme,
            ));

        // The template.
        let mut templates = div()
            .id("create-channel-template")
            .test_support()
            .role(Role::RadioGroup)
            .aria_label(t!("channels-create-template"))
            .flex()
            .gap_1();
        for template in Template::ALL {
            let active = self.form.template == template;
            let (label, about) = template_text(template);
            let hover = theme.accent.opacity(0.06);
            templates = templates.child(
                div()
                    .id(ElementId::Name(
                        format!("create-channel-template-{}", template.wire()).into(),
                    ))
                    .test_support()
                    .role(Role::RadioButton)
                    .reachable(&theme)
                    .aria_label(label)
                    .flex_1()
                    .min_w_0()
                    .p_2()
                    .rounded_md()
                    .cursor_pointer()
                    .border_1()
                    .when(active, |d| {
                        d.border_color(theme.accent).bg(theme.accent.opacity(0.12))
                    })
                    .when(!active, |d| {
                        d.border_color(theme.faint.opacity(0.4))
                            .hover(move |d| d.bg(hover))
                    })
                    .flex()
                    .flex_col()
                    .child(div().text_sm().font_weight(FontWeight::BOLD).child(label))
                    .child(div().text_xs().text_color(theme.muted).child(about))
                    .on_click(cx.listener(move |this, _, _, cx| this.set_template(template, cx))),
            );
        }
        root = root.child(
            section(
                "create-channel-templates",
                t!("channels-create-template"),
                &theme,
            )
            .child(templates),
        );
        let missing = self.form.missing_modes();
        if !missing.is_empty() {
            root = root.child(
                div()
                    .id("create-channel-missing")
                    .test_support()
                    .role(Role::Status)
                    .aria_label(t!("channels-create-missing", modes = missing.clone()))
                    .text_xs()
                    .text_color(theme.muted)
                    .child(t!("channels-create-missing", modes = missing)),
            );
        }

        // The switches.
        let mut settings = section(
            "create-channel-settings",
            t!("channels-create-settings"),
            &theme,
        )
        .child(
            div()
                .text_xs()
                .text_color(theme.muted)
                .child(t!("channels-create-supported")),
        );
        let mut switches = div().flex().flex_wrap().gap_y_2();
        for mode in self.form.modes.common() {
            switches = switches.child(
                div()
                    .w_1_2()
                    .min_w_0()
                    .pr_3()
                    .child(self.mode_box(mode, &theme, cx)),
            );
        }
        let mut values = div().flex().gap_2();
        for (mode, label) in [
            ('k', t!("channels-create-key")),
            ('l', t!("channels-create-limit")),
        ] {
            if let Some(field) = self.param_field(mode, label.into(), None, &theme) {
                values = values.child(field.flex_1().min_w_0());
            }
        }
        settings = settings.child(switches).child(values);
        let advanced = self.form.modes.advanced();
        let other: Vec<char> = self
            .form
            .modes
            .parameters()
            .into_iter()
            .filter(|m| !"kl".contains(*m))
            .collect();
        if !advanced.is_empty() || !other.is_empty() {
            let open = self.advanced;
            settings = settings.child(
                div()
                    .id("create-channel-advanced")
                    .test_support()
                    .role(Role::Button)
                    .reachable(&theme)
                    .aria_label(t!("channels-create-advanced"))
                    .aria_expanded(open)
                    .flex()
                    .items_center()
                    .gap_1()
                    .cursor_pointer()
                    .text_sm()
                    .font_weight(FontWeight::BOLD)
                    .child(ui::icon::icon(
                        if open {
                            IconName::ChevronDown
                        } else {
                            IconName::ChevronRight
                        },
                        theme.muted,
                    ))
                    .child(t!("channels-create-advanced"))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.advanced = !this.advanced;
                        cx.notify();
                    })),
            );
            if open {
                let mut switches = div().flex().flex_wrap().gap_y_2();
                for mode in advanced {
                    switches = switches.child(
                        div()
                            .w_1_2()
                            .min_w_0()
                            .pr_3()
                            .child(self.mode_box(mode, &theme, cx)),
                    );
                }
                settings = settings.child(switches);
                for mode in other {
                    let (label, about) = mode_text(mode);
                    if let Some(field) = self.param_field(
                        mode,
                        format!("{label} (+{mode})").into(),
                        Some(about.into()),
                        &theme,
                    ) {
                        settings = settings.child(field);
                    }
                }
            }
        }
        root = root.child(settings);

        root
    }
}

fn section(id: &'static str, legend: &'static str, theme: &Theme) -> Stateful<Div> {
    div()
        .id(id)
        .role(Role::Group)
        .aria_label(legend)
        .flex()
        .flex_col()
        .gap_2()
        .child(
            div()
                .text_xs()
                .font_weight(FontWeight::BOLD)
                .text_color(theme.faint)
                .child(legend.to_uppercase()),
        )
}

fn template_text(template: Template) -> (&'static str, &'static str) {
    match template {
        Template::Private => (
            t!("channels-template-private"),
            t!("channels-template-private-about"),
        ),
        Template::Open => (
            t!("channels-template-open"),
            t!("channels-template-open-about"),
        ),
        Template::Announcements => (
            t!("channels-template-announcements"),
            t!("channels-template-announcements-about"),
        ),
    }
}

/// A channel mode's name and what it does. What a letter means is up to the
/// server software, so common letters are described and the rest are named
/// server-specific.
pub fn mode_text(mode: char) -> (&'static str, &'static str) {
    match mode {
        'n' => (t!("channels-mode-n"), t!("channels-mode-n-about")),
        't' => (t!("channels-mode-t"), t!("channels-mode-t-about")),
        'm' => (t!("channels-mode-m"), t!("channels-mode-m-about")),
        'i' => (t!("channels-mode-i"), t!("channels-mode-i-about")),
        's' => (t!("channels-mode-s"), t!("channels-mode-s-about")),
        'p' => (t!("channels-mode-p"), t!("channels-mode-p-about")),
        'k' => (t!("channels-mode-k"), t!("channels-mode-k-about")),
        'l' => (t!("channels-mode-l"), t!("channels-mode-l-about")),
        'r' | 'R' => (t!("channels-mode-r"), t!("channels-mode-r-about")),
        'c' => (t!("channels-mode-c"), t!("channels-mode-c-about")),
        'C' => (
            t!("channels-mode-upper-c"),
            t!("channels-mode-upper-c-about"),
        ),
        'S' => (
            t!("channels-mode-upper-s"),
            t!("channels-mode-upper-s-about"),
        ),
        'T' => (
            t!("channels-mode-upper-t"),
            t!("channels-mode-upper-t-about"),
        ),
        'z' => (t!("channels-mode-z"), t!("channels-mode-z-about")),
        'g' => (t!("channels-mode-g"), t!("channels-mode-g-about")),
        'P' => (
            t!("channels-mode-upper-p"),
            t!("channels-mode-upper-p-about"),
        ),
        'Q' => (
            t!("channels-mode-upper-q"),
            t!("channels-mode-upper-q-about"),
        ),
        'F' => (
            t!("channels-mode-upper-f"),
            t!("channels-mode-upper-f-about"),
        ),
        'u' => (t!("channels-mode-u"), t!("channels-mode-u-about")),
        'f' => (t!("channels-mode-f"), t!("channels-mode-f-about")),
        'j' => (t!("channels-mode-j"), t!("channels-mode-j-about")),
        'L' => (
            t!("channels-mode-upper-l"),
            t!("channels-mode-upper-l-about"),
        ),
        _ => (
            t!("channels-mode-server-specific"),
            t!("channels-mode-server-specific-about"),
        ),
    }
}

//! Adding and editing a network (package NW1), as the web client's
//! `NetworkDialog.vue` does it: presets, the server, identity, login and
//! on-connect fieldsets, the server checking a new address before saving,
//! and a question before typed changes are thrown away. An account with no
//! networks gets the quick set-up page instead (nickname first, the rest
//! folded away). The network menu edits, adds and changes your nick and
//! real name; the sidebar's own entries (N1) connect, disconnect and remove.
//!
//! The form's rules live in `ircord_client::api::networks::Form`, tested
//! there; this file draws it.

use crate::ui::focus::Reachable;
use crate::ui::icon::IconName;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::TestSupportExt;
use gpui_kit::component::Disableable;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::switch::Switch;
use gpui_kit::prelude::*;
use gpui_kit::*;
use ircord_client::Client;
use ircord_client::api::networks::{self as nets, Form, Login, Problems};
use ircord_client::commands::NickError;
use ircord_client::contract as c;
use ircord_client::contract::NetworkStatus;

use crate::registry::{self, MenuItem, NetworkCtx, Pane};
use crate::route::{PaneKind, Route};
use crate::session::{Session, SessionEvent};
use crate::theme::Theme;
use crate::{theme, ui};

/// The page an account with no networks lands on.
pub const SETUP_PAGE: &str = "network-setup";

/// How long typing must pause before the server checks an address.
const CHECK_DELAY: Duration = Duration::from_millis(300);

pub fn init(cx: &mut App) {
    cx.observe_new::<Session>(|_, _, cx| {
        cx.subscribe_self(|session: &mut Session, event: &SessionEvent, cx| {
            if let SessionEvent::Changed(changes) = event
                && changes.reset
                && session.route().is_none()
                && session
                    .client()
                    .is_some_and(|c| c.read().networks.is_empty())
            {
                session.navigate(Route::Page(SETUP_PAGE.into()), cx);
            }
            // A network added elsewhere (another device) while the set-up
            // waits untouched, or the one just set up: go to it, which
            // leaves the page once it has a conversation to show.
            if let SessionEvent::Changed(changes) = event
                && changes.sidebar
                && session.route() == Some(Route::Page(SETUP_PAGE.into()))
                && let Some(Setup(form)) = cx.try_global::<Setup>()
                && !form.read(cx).dirty(cx)
                && let Some(first) = session
                    .client()
                    .and_then(|c| c.read().networks.first().map(|n| n.id.clone()))
            {
                session.select_network(&first, cx);
            }
        })
        .detach();
    })
    .detach();
    registry::register(cx, |r| {
        r.panes.add(Pane::new(
            PaneKind::Page(SETUP_PAGE.into()),
            |ctx, window, cx| setup_page(&ctx.session, window, cx).into_any_element(),
        ));
        // Connect, disconnect and remove are the sidebar's (N1), at 500 and 900.
        let menu = &mut r.network_menu;
        menu.add(
            MenuItem::new(
                "network-change-nick",
                t!("networks-menu-change-nick"),
                |n: &NetworkCtx, window, cx| change_self(n.clone(), SelfField::Nick, window, cx),
            )
            .icon(IconName::User)
            .order(530)
            .when(|n, cx| status(n, cx) == Some(NetworkStatus::Connected)),
        );
        menu.add(
            MenuItem::new(
                "network-change-realname",
                t!("networks-menu-change-realname"),
                |n: &NetworkCtx, window, cx| {
                    change_self(n.clone(), SelfField::Realname, window, cx)
                },
            )
            .icon(IconName::CircleUser)
            .order(531)
            .when(|n, cx| {
                with_network(&n.session, &n.network, cx, |n| {
                    n.status == NetworkStatus::Connected && n.features.setname
                })
                .unwrap_or(false)
            }),
        );
        menu.add(
            MenuItem::new(
                "network-edit",
                t!("networks-menu-edit"),
                |n: &NetworkCtx, window, cx| open_edit(&n.session, &n.network, window, cx),
            )
            .icon(IconName::Settings2)
            .order(520),
        );
        menu.add(
            MenuItem::new(
                "network-add",
                t!("networks-menu-add"),
                |n: &NetworkCtx, window, cx| open_add(&n.session, window, cx),
            )
            .icon(IconName::Plus)
            .order(800),
        );
    });
}

fn client(session: &Entity<Session>, cx: &App) -> Option<Rc<Client>> {
    session.read(cx).client().cloned()
}

fn with_network<T>(
    session: &Entity<Session>,
    id: &str,
    cx: &App,
    f: impl FnOnce(&c::Network) -> T,
) -> Option<T> {
    let client = client(session, cx)?;
    let w = client.read();
    w.network(id).map(f)
}

fn status(n: &NetworkCtx, cx: &App) -> Option<NetworkStatus> {
    with_network(&n.session, &n.network, cx, |n| n.status)
}

/// The words for a nickname problem.
pub fn nick_problem(problem: NickError) -> SharedString {
    match problem {
        NickError::Required => t!("networks-nick-required").into(),
        NickError::Spaces => t!("networks-nick-spaces").into(),
        NickError::Syntax => t!("networks-nick-syntax").into(),
        NickError::TooLong(maximum) => t!("networks-nick-too-long", maximum = maximum).into(),
    }
}

// ---- Actions ----

/// Connect or disconnect a network; a failure shows as a toast.
pub fn connect(
    session: &Entity<Session>,
    network: &str,
    up: bool,
    window: &mut Window,
    cx: &mut App,
) {
    let Some(client) = client(session, cx) else {
        return;
    };
    let id = network.to_string();
    let task = client.request(move |api| async move { api.connect_network(&id, up).await });
    window
        .spawn(cx, async move |cx| {
            let result = task.await;
            cx.update(|window, cx| match result {
                Ok(Ok(network)) => client.change(|w| nets::saved(w, network)),
                Ok(Err(e)) => ui::toast::error(window, cx, e.to_string()),
                Err(e) => ui::toast::error(window, cx, e.to_string()),
            })
            .ok();
        })
        .detach();
}

#[derive(Clone, Copy)]
enum SelfField {
    Nick,
    Realname,
}

/// Ask for a new nick or real name and send it to the connected network.
fn change_self(n: NetworkCtx, field: SelfField, window: &mut Window, cx: &mut App) {
    let Some((name, nick, realname, max)) = with_network(&n.session, &n.network, cx, |net| {
        (
            net.name.clone(),
            net.nick.clone(),
            net.realname.clone().unwrap_or_default(),
            ircord_client::commands::run::nickname_limit(&net.isupport),
        )
    }) else {
        return;
    };
    let (title, label, initial) = match field {
        SelfField::Nick => (
            t!("networks-change-nick-title", name = name),
            t!("networks-nick"),
            nick.clone(),
        ),
        SelfField::Realname => (
            t!("networks-change-realname-title", name = name),
            t!("networks-realname"),
            realname.clone(),
        ),
    };
    ui::prompt::prompt(
        window,
        cx,
        ui::prompt::Prompt {
            title: title.into(),
            label: label.into(),
            initial: initial.clone(),
            ok: t!("networks-change").into(),
        },
        move |text, window, cx| {
            let text = text.trim().to_string();
            if text == initial || text.is_empty() && matches!(field, SelfField::Realname) {
                return;
            }
            let update = match field {
                SelfField::Nick => {
                    if let Some(problem) = ircord_client::commands::run::nickname_error(&text, max)
                    {
                        ui::toast::error(window, cx, nick_problem(problem));
                        return;
                    }
                    c::SelfUpdate {
                        nick: Some(text),
                        ..Default::default()
                    }
                }
                SelfField::Realname => c::SelfUpdate {
                    realname: Some(text),
                    ..Default::default()
                },
            };
            let Some(client) = client(&n.session, cx) else {
                return;
            };
            let id = n.network.clone();
            let task =
                client.request(move |api| async move { api.update_self(&id, &update).await });
            window
                .spawn(cx, async move |cx| {
                    let result = task.await;
                    cx.update(|window, cx| match result {
                        Ok(Ok(network)) => client.change(|w| nets::saved(w, network)),
                        Ok(Err(e)) => ui::toast::error(window, cx, e.to_string()),
                        Err(e) => ui::toast::error(window, cx, e.to_string()),
                    })
                    .ok();
                })
                .detach();
        },
    );
}

/// Ask before removing a network; `form` is the edit dialog it came from,
/// closed once the network is gone.
fn confirm_remove(
    session: Entity<Session>,
    network: String,
    form: Option<WeakEntity<NetworkFormView>>,
    window: &mut Window,
    cx: &mut App,
) {
    let Some(name) = with_network(&session, &network, cx, |n| n.name.clone()) else {
        return;
    };
    ui::confirm::confirm(
        window,
        cx,
        ui::confirm::Confirm {
            title: t!("networks-remove-title", name = name).into(),
            body: t!("networks-remove-body").into(),
            ok: t!("networks-remove-confirm").into(),
            danger: true,
        },
        move |window, cx| {
            let Some(client) = client(&session, cx) else {
                return;
            };
            let id = network.clone();
            let task = client.request(move |api| async move { api.remove_network(&id).await });
            let (id, name, form) = (network.clone(), name.clone(), form.clone());
            window
                .spawn(cx, async move |cx| {
                    let result = task.await;
                    cx.update(|window, cx| match result {
                        Ok(Ok(())) => {
                            client.change(|w| nets::removed(w, &id));
                            if form.is_some_and(|f| f.upgrade().is_some()) {
                                ui::dialog::close(window, cx);
                            }
                            ui::toast::success(window, cx, t!("networks-removed", name = name));
                        }
                        Ok(Err(e)) => ui::toast::error(window, cx, e.to_string()),
                        Err(e) => ui::toast::error(window, cx, e.to_string()),
                    })
                    .ok();
                })
                .detach();
        },
    );
}

/// Open the form to add a network. With no networks yet it is the quick set-up.
pub fn open_add(session: &Entity<Session>, window: &mut Window, cx: &mut App) {
    open_dialog(session, None, window, cx);
}

pub fn open_edit(session: &Entity<Session>, network: &str, window: &mut Window, cx: &mut App) {
    open_dialog(session, Some(network.to_string()), window, cx);
}

fn open_dialog(
    session: &Entity<Session>,
    network: Option<String>,
    window: &mut Window,
    cx: &mut App,
) {
    let title = match &network {
        Some(id) => match with_network(session, id, cx, |n| n.name.clone()) {
            Some(name) => t!("networks-edit-title", name = name),
            None => return,
        },
        None => t!("networks-add-title").to_string(),
    };
    let session = session.clone();
    let form = cx.new(|cx| NetworkFormView::new(session, network, Place::Dialog, window, cx));
    // Focus the first field once the dialog is up, as `ui::prompt` does:
    // opening the dialog moves the focus into it.
    let focus = std::cell::Cell::new(true);
    ui::dialog::open(window, cx, title, move |dialog, window, cx| {
        if focus.replace(false) {
            let first = form.read(cx).first_field();
            first.focus_handle(cx).focus(window, cx);
        }
        let guard = form.downgrade();
        dialog
            .w(px(640.))
            .on_cancel(move |_, window, cx| {
                // Escape, the close button or a click outside: ask first
                // when something was typed.
                let Some(form) = guard.upgrade() else {
                    return true;
                };
                if form.read(cx).dirty(cx) {
                    form.update(cx, |f, cx| f.ask_discard(window, cx));
                    false
                } else {
                    true
                }
            })
            .footer(form.update(cx, |f, cx| f.actions(cx)))
            .child(form.clone())
    });
}

// ---- The quick set-up page ----

struct Setup(Entity<NetworkFormView>);
impl Global for Setup {}

fn setup_page(session: &Entity<Session>, window: &mut Window, cx: &mut App) -> impl IntoElement {
    let form = match cx.try_global::<Setup>() {
        Some(Setup(form)) if form.read(cx).session == *session => form.clone(),
        _ => {
            let form =
                cx.new(|cx| NetworkFormView::new(session.clone(), None, Place::Setup, window, cx));
            cx.set_global(Setup(form.clone()));
            form
        }
    };
    let theme = theme::get(cx).clone();
    ui::page::Page::new(
        SETUP_PAGE,
        t!("networks-first-title"),
        div()
            .w_full()
            .flex()
            .justify_center()
            .child(div().w(px(560.)).max_w_full().py_4().child(form)),
    )
    .render(&theme)
}

// ---- The form ----

/// Where the form is shown.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Place {
    Dialog,
    /// The quick set-up page.
    Setup,
}

/// The server's verdict on the address in the form.
#[derive(Clone, Debug, PartialEq)]
pub enum Check {
    /// Nothing valid to check yet.
    Idle,
    Checking,
    Reachable,
    Unreachable(String),
}

struct Fields {
    name: Entity<InputState>,
    host: Entity<InputState>,
    port: Entity<InputState>,
    nick: Entity<InputState>,
    alt_nick: Entity<InputState>,
    username: Entity<InputState>,
    realname: Entity<InputState>,
    account: Entity<InputState>,
    password: Entity<InputState>,
    auto_join: Entity<TextareaState>,
    perform: Entity<TextareaState>,
}

pub struct NetworkFormView {
    session: Entity<Session>,
    /// The network being edited; `None` adds one.
    network: Option<String>,
    place: Place,
    /// The quick set-up shows only the nickname until this is on.
    advanced: bool,
    presets: Vec<c::NetworkPreset>,
    fields: Fields,
    tls: bool,
    login: Login,
    clear_password: bool,
    /// What the form opened with, to tell whether anything was typed.
    initial: Form,
    check: Check,
    check_round: u64,
    error: Option<SharedString>,
    saving: bool,
    /// Problems show once a save was tried, or once a field has text.
    tried: bool,
    /// The quick set-up's network, once added.
    done: Option<String>,
    _subscriptions: Vec<Subscription>,
}

impl NetworkFormView {
    fn new(
        session: Entity<Session>,
        network: Option<String>,
        place: Place,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let (initial, existing) = {
            let client = session.read(cx).client().cloned();
            let w = client.as_ref().map(|c| c.read());
            let existing = network
                .as_deref()
                .and_then(|id| w.as_ref()?.network(id).cloned());
            let nick = match place {
                Place::Setup => String::new(),
                Place::Dialog => w.as_ref().map(|w| w.user.nick.clone()).unwrap_or_default(),
            };
            let initial = existing.as_ref().map_or_else(|| Form::new(&nick), Form::of);
            (initial, existing)
        };
        let line = |value: &str, masked: bool, window: &mut Window, cx: &mut Context<Self>| {
            let value = value.to_string();
            cx.new(|cx| {
                let mut state = InputState::new(window, cx).masked(masked);
                state.set_value(value, window, cx);
                state
            })
        };
        let lines =
            |value: &str, placeholder: &str, window: &mut Window, cx: &mut Context<Self>| {
                let (value, placeholder) = (value.to_string(), placeholder.to_string());
                cx.new(|cx| {
                    let mut state = TextareaState::new(window, cx)
                        .auto_grow(3, 6)
                        .placeholder(placeholder);
                    state.set_value(value, window, cx);
                    state
                })
            };
        let fields = Fields {
            name: line(&initial.name, false, window, cx),
            host: line(&initial.host, false, window, cx),
            port: line(&initial.port, false, window, cx),
            nick: line(&initial.nick, false, window, cx),
            alt_nick: line(&initial.alt_nick, false, window, cx),
            username: line(&initial.username, false, window, cx),
            realname: line(&initial.realname, false, window, cx),
            account: line(&initial.account, false, window, cx),
            password: line("", true, window, cx),
            auto_join: lines(
                &initial.auto_join,
                "#channel\n#private secretkey",
                window,
                cx,
            ),
            perform: lines(&initial.perform, "MODE mynick +x", window, cx),
        };
        fields
            .host
            .update(cx, |s, cx| s.set_placeholder("irc.example.org", window, cx));
        fields.name.update(cx, |s, cx| {
            s.set_placeholder(t!("networks-name-placeholder"), window, cx)
        });
        let mut subscriptions = Vec::new();
        for (input, address) in [
            (&fields.name, false),
            (&fields.host, true),
            (&fields.port, true),
            (&fields.nick, false),
            (&fields.alt_nick, false),
            (&fields.username, false),
            (&fields.realname, false),
            (&fields.account, false),
            (&fields.password, false),
        ] {
            subscriptions.push(cx.subscribe_in(
                input,
                window,
                move |this: &mut Self, _, event: &InputEvent, window, cx| match event {
                    InputEvent::Change => {
                        if address {
                            this.address_changed(cx);
                        }
                        cx.notify();
                    }
                    InputEvent::PressEnter { .. } => this.submit(window, cx),
                    _ => {}
                },
            ));
        }
        for input in [&fields.auto_join, &fields.perform] {
            subscriptions.push(cx.subscribe(input, |_, _, event: &InputEvent, cx| {
                if let InputEvent::Change = event {
                    cx.notify();
                }
            }));
        }
        subscriptions.push(cx.observe(&session, |_, _, cx| cx.notify()));
        let mut this = Self {
            session,
            network,
            place,
            advanced: false,
            presets: Vec::new(),
            fields,
            tls: initial.tls,
            login: initial.login,
            clear_password: false,
            initial,
            check: Check::Idle,
            check_round: 0,
            error: None,
            saving: false,
            tried: false,
            done: None,
            _subscriptions: subscriptions,
        };
        this.advanced = place == Place::Dialog;
        if existing.is_none() {
            this.load_presets(window, cx);
        }
        this.address_changed(cx);
        this
    }

    /// The field the person starts with.
    fn first_field(&self) -> Entity<InputState> {
        match self.place {
            Place::Setup => self.fields.nick.clone(),
            Place::Dialog => self.fields.name.clone(),
        }
    }

    fn client(&self, cx: &App) -> Option<Rc<Client>> {
        self.session.read(cx).client().cloned()
    }

    fn existing(&self, cx: &App) -> Option<c::Network> {
        let client = self.client(cx)?;
        let w = client.read();
        w.network(self.network.as_deref()?).cloned()
    }

    /// The form as typed.
    pub fn form(&self, cx: &App) -> Form {
        let text = |s: &Entity<InputState>| s.read(cx).value().to_string();
        Form {
            name: text(&self.fields.name),
            host: text(&self.fields.host),
            port: text(&self.fields.port),
            tls: self.tls,
            nick: text(&self.fields.nick),
            alt_nick: text(&self.fields.alt_nick),
            username: text(&self.fields.username),
            realname: text(&self.fields.realname),
            login: self.login,
            account: text(&self.fields.account),
            password: text(&self.fields.password),
            clear_password: self.clear_password,
            auto_join: self.fields.auto_join.read(cx).value().to_string(),
            perform: self.fields.perform.read(cx).value().to_string(),
        }
    }

    /// Whether anything differs from what the form opened with.
    pub fn dirty(&self, cx: &App) -> bool {
        !self.saving && self.done.is_none() && self.form(cx) != self.initial
    }

    pub fn check(&self) -> &Check {
        &self.check
    }

    fn load_presets(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(client) = self.client(cx) else {
            self.presets = nets::default_presets();
            return;
        };
        let task = client.request(|api| async move { api.network_presets().await });
        cx.spawn_in(window, async move |this, cx| {
            let page = task.await.ok().and_then(Result::ok);
            this.update_in(cx, |this, window, cx| {
                this.presets = nets::presets(page);
                // The quick set-up starts from the first suggestion.
                if this.place == Place::Setup
                    && !this.dirty_beyond_nick(cx)
                    && let Some(first) = this.presets.first().cloned()
                {
                    this.apply_preset(&first, window, cx);
                    let form = this.form(cx);
                    this.initial = form;
                    let problems = this.problems(cx);
                    this.advanced =
                        problems.name || problems.host || problems.port || problems.password;
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Whether anything but the nickname and display name was typed.
    fn dirty_beyond_nick(&self, cx: &App) -> bool {
        let mut form = self.form(cx);
        form.nick = self.initial.nick.clone();
        form.realname = self.initial.realname.clone();
        form != self.initial
    }

    fn apply_preset(
        &mut self,
        preset: &c::NetworkPreset,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut form = self.form(cx);
        form.apply(preset);
        let set = |input: &Entity<InputState>,
                   value: &str,
                   window: &mut Window,
                   cx: &mut Context<Self>| {
            let value = value.to_string();
            input.update(cx, |s, cx| s.set_value(value, window, cx));
        };
        set(&self.fields.name, &form.name, window, cx);
        set(&self.fields.host, &form.host, window, cx);
        set(&self.fields.port, &form.port, window, cx);
        set(&self.fields.account, "", window, cx);
        set(&self.fields.password, "", window, cx);
        let auto_join = form.auto_join.clone();
        self.fields
            .auto_join
            .update(cx, |s, cx| s.set_value(auto_join, window, cx));
        self.tls = form.tls;
        self.login = form.login;
        self.clear_password = false;
        self.address_changed(cx);
        cx.notify();
    }

    fn problems(&self, cx: &App) -> Problems {
        let existing = self.existing(cx);
        self.form(cx).problems(existing.as_ref(), &self.presets)
    }

    /// The address changed: check it again after a pause, unless it is the
    /// saved one.
    fn address_changed(&mut self, cx: &mut Context<Self>) {
        self.check_round += 1;
        let form = self.form(cx);
        let existing = self.existing(cx);
        if !form.needs_check(existing.as_ref()) {
            self.check = Check::Reachable;
            return;
        }
        let Some((host, port, tls)) = form.endpoint() else {
            self.check = Check::Idle;
            return;
        };
        let Some(client) = self.client(cx) else {
            return;
        };
        self.check = Check::Checking;
        let round = self.check_round;
        let task = client.request(move |api| async move {
            tokio::time::sleep(CHECK_DELAY).await;
            api.check_network(&host, port, tls).await
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                if this.check_round != round {
                    return;
                }
                this.check = match result {
                    Ok(Ok(check)) if check.reachable => Check::Reachable,
                    Ok(Ok(check)) => Check::Unreachable(check.detail),
                    Ok(Err(e)) => Check::Unreachable(t!("networks-check-failed", error = e)),
                    Err(e) => Check::Unreachable(t!("networks-check-failed", error = e)),
                };
                if this.place == Place::Setup && this.check != Check::Reachable {
                    this.advanced = true;
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn set_tls(&mut self, tls: bool, cx: &mut Context<Self>) {
        self.tls = tls;
        self.address_changed(cx);
        cx.notify();
    }

    fn set_login(&mut self, login: Login, cx: &mut Context<Self>) {
        self.login = login;
        cx.notify();
    }

    /// Save: add the network or change it, once nothing is wrong and the
    /// server can reach the address.
    pub fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.saving || self.done.is_some() {
            return;
        }
        self.tried = true;
        let problems = self.problems(cx);
        let existing = self.existing(cx);
        let form = self.form(cx);
        let unchecked = form.needs_check(existing.as_ref()) && self.check != Check::Reachable;
        if problems.any() || unchecked {
            if self.place == Place::Setup && (problems.any_beyond_nick() || unchecked) {
                self.advanced = true;
            }
            cx.notify();
            return;
        }
        let Some(client) = self.client(cx) else {
            return;
        };
        self.error = None;
        self.saving = true;
        cx.notify();
        let task = match &self.network {
            Some(id) => {
                let (id, update) = (id.clone(), form.update());
                client.request(move |api| async move { api.update_network(&id, &update).await })
            }
            None => {
                let config = form.config();
                client.request(move |api| async move { api.add_network(&config).await })
            }
        };
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| {
                this.saving = false;
                match result {
                    Ok(Ok(network)) => {
                        let id = network.id.clone();
                        let name = network.name.clone();
                        client.change(|w| nets::saved(w, network));
                        match this.place {
                            Place::Dialog => ui::dialog::close(window, cx),
                            Place::Setup => {
                                this.done = Some(name);
                                this.session.update(cx, |s, cx| s.select_network(&id, cx));
                            }
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

    /// Cancel: ask first when something was typed.
    fn cancel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.dirty(cx) {
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
                title: t!("networks-discard-title").into(),
                body: t!("networks-discard-body").into(),
                ok: t!("networks-discard").into(),
                danger: true,
            },
            |window, cx| {
                // The question closes itself; then the form under it.
                window.defer(cx, ui::dialog::close);
            },
        );
    }
}

trait Beyond {
    fn any_beyond_nick(&self) -> bool;
}

impl Beyond for Problems {
    fn any_beyond_nick(&self) -> bool {
        Problems {
            nick: None,
            ..self.clone()
        }
        .any()
    }
}

// ---- Drawing ----

fn fieldset(
    id: &'static str,
    legend: SharedString,
    theme: &Theme,
) -> impl ParentElement + IntoElement + use<> {
    div()
        .id(id)
        .test_support()
        .role(Role::Group)
        .aria_label(legend.clone())
        .mt_3()
        .p_3()
        .rounded_lg()
        .bg(theme.sidebar)
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

/// The glyph a network gets from its name, drawn while it is typed: its
/// first letter on the accent's tint, or "?" before there is a name.
fn glyph_preview(name: &str, theme: &Theme) -> impl IntoElement + use<> {
    let initial = name
        .trim()
        .chars()
        .next()
        .map(|c| c.to_uppercase().to_string())
        .unwrap_or_else(|| "?".to_string());
    div()
        .id("network-glyph-preview")
        .test_support()
        .role(Role::Image)
        .aria_label(SharedString::from(t!(
            "networks-glyph-preview",
            initial = initial.clone()
        )))
        .size(px(56.))
        .flex_none()
        .mb(px(2.))
        .rounded(theme.radius_large)
        .bg(theme.accent.opacity(0.25))
        .flex()
        .items_center()
        .justify_center()
        .text_xl()
        .font_weight(FontWeight::BOLD)
        .text_color(theme.text)
        .child(initial)
}

fn hint(text: impl Into<SharedString>, error: bool, theme: &Theme) -> Div {
    div()
        .text_xs()
        .text_color(if error { theme.danger } else { theme.muted })
        .child(text.into())
}

/// A labelled one-line input, with its problem (or help) under it.
fn line_field(
    id: &'static str,
    label: impl Into<SharedString>,
    input: &Entity<InputState>,
    note: Option<(SharedString, bool)>,
    theme: &Theme,
) -> Div {
    let label: SharedString = label.into();
    let control = div()
        .flex()
        .flex_col()
        .gap_1()
        .child(Input::new(input).id(id).aria_label(label.clone()))
        .children(note.map(|(text, error)| {
            div()
                .id(ElementId::Name(format!("{id}-note").into()))
                .test_support()
                .role(if error { Role::Alert } else { Role::Status })
                .aria_label(text.clone())
                .child(hint(text, error, theme))
        }));
    ui::field::field(label, control, theme).flex_1().min_w_0()
}

fn lines_field(
    id: &'static str,
    label: impl Into<SharedString>,
    input: &Entity<TextareaState>,
    theme: &Theme,
) -> Div {
    let label: SharedString = label.into();
    ui::field::field(
        label.clone(),
        div()
            .id(id)
            .test_support()
            .p_1()
            .rounded_md()
            .bg(theme.input)
            .child(Textarea::new(input).aria_label(label)),
        theme,
    )
    .flex_1()
    .min_w_0()
}

fn login_label(login: Login) -> &'static str {
    match login {
        Login::None => t!("networks-login-none"),
        Login::Sasl => t!("networks-login-sasl"),
        Login::NickServ => t!("networks-login-nickserv"),
        Login::ServerPassword => t!("networks-login-server-password"),
    }
}

fn login_help(login: Login) -> &'static str {
    match login {
        Login::None => t!("networks-login-none-help"),
        Login::Sasl => t!("networks-login-sasl-help"),
        Login::NickServ => t!("networks-login-nickserv-help"),
        Login::ServerPassword => t!("networks-login-server-password-help"),
    }
}

impl Render for NetworkFormView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme::get(cx).clone();
        let form = self.form(cx);
        let existing = self.existing(cx);
        let problems = form.problems(existing.as_ref(), &self.presets);
        let password_set = form.password_set(existing.as_ref(), &self.presets);
        let provided = existing.is_none() && password_set;
        // A field's problem shows once it has text, or after a save was tried.
        let show = |problem: bool, typed: &str| problem && (self.tried || !typed.is_empty());
        let nick_note =
            |value: &str, problem: Option<NickError>, help: Option<&'static str>| match problem {
                Some(p) if self.tried || !value.is_empty() => Some((nick_problem(p), true)),
                _ => help.map(|h| (SharedString::from(h), false)),
            };

        let mut root = div()
            .id("network-form")
            .test_support()
            .role(Role::Form)
            .aria_label(match (&existing, self.place) {
                (Some(n), _) => SharedString::from(t!("networks-edit-title", name = n.name)),
                (None, Place::Setup) => t!("networks-first-title").into(),
                (None, Place::Dialog) => t!("networks-add-title").into(),
            })
            .flex()
            .flex_col()
            .text_sm()
            .text_color(theme.text);

        if let Some(name) = &self.done {
            return root.child(
                div()
                    .id("network-setup-done")
                    .test_support()
                    .role(Role::Status)
                    .aria_label(SharedString::from(t!("networks-setup-done", name = name)))
                    .p_4()
                    .text_color(theme.text)
                    .child(t!("networks-setup-done", name = name)),
            );
        }

        // Quick set-up: the nickname first, the rest folded away.
        if self.place == Place::Setup {
            let intro = if form.name.trim().is_empty() {
                t!("networks-intro").to_string()
            } else {
                t!("networks-ready-intro", name = form.name.trim())
            };
            let mut summary = String::new();
            if let Some((host, port, tls)) = form.endpoint() {
                summary = format!(
                    "{host}:{port} · {}",
                    if tls {
                        "TLS"
                    } else {
                        t!("networks-summary-plain")
                    }
                );
                let channels = nets::auto_join(&form.auto_join);
                if !channels.is_empty() {
                    summary.push_str(&format!(" · {}", channels.join(", ")));
                }
                if provided {
                    summary.push_str(&format!(" · {}", t!("networks-summary-password")));
                }
            }
            let advanced = self.advanced;
            root = root
                .child(div().mb_3().text_color(theme.muted).child(intro))
                .child(line_field(
                    "network-nick",
                    t!("networks-nick"),
                    &self.fields.nick,
                    nick_note(&form.nick, problems.nick, Some(t!("networks-nick-help"))),
                    &theme,
                ))
                .child(div().mt_2().child(line_field(
                    "network-realname",
                    t!("networks-display-name"),
                    &self.fields.realname,
                    Some((t!("networks-display-name-help").into(), false)),
                    &theme,
                )))
                .when(!summary.is_empty(), |d| {
                    d.child(
                        div()
                            .id("network-summary")
                            .test_support()
                            .role(Role::Status)
                            .aria_label(SharedString::from(summary.clone()))
                            .mt_3()
                            .text_xs()
                            .text_color(theme.muted)
                            .child(summary.clone()),
                    )
                })
                .child(
                    div()
                        .id("network-advanced")
                        .test_support()
                        .role(Role::Button)
                        .reachable(&theme)
                        .aria_label(t!("networks-connection-settings"))
                        .mt_2()
                        .text_color(theme.link)
                        .cursor_pointer()
                        .child(format!(
                            "{} {}",
                            if advanced { "▾" } else { "▸" },
                            t!("networks-connection-settings")
                        ))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.advanced = !this.advanced;
                            cx.notify();
                        })),
                );
        }

        if self.advanced {
            // Presets, for a new network.
            if existing.is_none() && !self.presets.is_empty() {
                let mut row = div()
                    .id("network-presets")
                    .test_support()
                    .role(Role::Group)
                    .aria_label(t!("networks-presets"))
                    .flex()
                    .flex_wrap()
                    .gap_1p5()
                    .mt_2();
                for (i, preset) in self.presets.iter().enumerate() {
                    let p = preset.clone();
                    let hover = theme.hover;
                    row = row.child(
                        div()
                            .id(ElementId::Name(format!("network-preset-{i}").into()))
                            .test_support()
                            .role(Role::Button)
                            .reachable(&theme)
                            .aria_label(SharedString::from(preset.name.clone()))
                            .flex()
                            .items_center()
                            .gap_2()
                            .pl_1()
                            .pr_2p5()
                            .py_1()
                            .rounded_full()
                            .bg(theme.selected)
                            .hover(move |d| d.bg(hover))
                            .cursor_pointer()
                            .child(
                                div()
                                    .size(px(22.))
                                    .rounded_full()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .bg(theme.accent)
                                    .text_color(theme.on_accent)
                                    .text_xs()
                                    .font_weight(FontWeight::BOLD)
                                    .child(preset.initial.clone()),
                            )
                            .child(preset.name.clone())
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.apply_preset(&p, window, cx)
                            })),
                    );
                }
                root = root.child(row);
            }

            // Server.
            let host_note = show(problems.host, &form.host)
                .then(|| (SharedString::from(t!("networks-problem-host")), true));
            let port_note = show(problems.port, &form.port)
                .then(|| (SharedString::from(t!("networks-problem-port")), true));
            let name_note = (problems.name && self.tried)
                .then(|| (SharedString::from(t!("networks-problem-name")), true));
            let tls = self.tls;
            let this = cx.entity().downgrade();
            root = root.child(
                fieldset("network-server", t!("networks-server").into(), &theme)
                    .child(
                        // As the web's: the network's glyph as it will show
                        // in the tabs, beside its name.
                        div()
                            .flex()
                            .items_end()
                            .gap_3()
                            .child(glyph_preview(&form.name, &theme))
                            .child(line_field(
                                "network-name",
                                t!("networks-name"),
                                &self.fields.name,
                                name_note,
                                &theme,
                            )),
                    )
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(line_field(
                                "network-host",
                                t!("networks-host"),
                                &self.fields.host,
                                host_note,
                                &theme,
                            ))
                            .child(div().w(px(110.)).child(line_field(
                                "network-port",
                                t!("networks-port"),
                                &self.fields.port,
                                port_note,
                                &theme,
                            ))),
                    )
                    .child(
                        div()
                            .mt_1()
                            .pt_3()
                            .border_t_1()
                            .border_color(theme.border)
                            .flex()
                            .items_center()
                            .gap_3()
                            .child(
                                div()
                                    .size(px(32.))
                                    .flex_none()
                                    .rounded_full()
                                    .bg(if tls { theme.online } else { theme.warning }
                                        .opacity(0.16))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(ui::icon::icon(
                                        if tls { IconName::EyeOff } else { IconName::Eye },
                                        if tls { theme.online } else { theme.warning },
                                    )),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .flex()
                                    .flex_col()
                                    .child(
                                        div()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .child(t!("networks-tls")),
                                    )
                                    .child(hint(t!("networks-tls-help"), false, &theme)),
                            )
                            .child(
                                Switch::new("network-tls")
                                    .checked(tls)
                                    .accessibility_label(t!("networks-tls"))
                                    .on_click(move |checked, _, cx| {
                                        this.update(cx, |f, cx| f.set_tls(*checked, cx)).ok();
                                    }),
                            ),
                    ),
            );

            // Identity.
            let mut identity = fieldset("network-identity", t!("networks-identity").into(), &theme);
            let alt_nick = line_field(
                "network-alt-nick",
                t!("networks-alt-nick"),
                &self.fields.alt_nick,
                nick_note(&form.alt_nick, problems.alt_nick, None),
                &theme,
            );
            let username = line_field(
                "network-username",
                t!("networks-username"),
                &self.fields.username,
                None,
                &theme,
            );
            // As the web's: "If taken, use" beside the nickname, the real
            // name beside the username.
            if self.place == Place::Dialog {
                identity =
                    identity
                        .child(
                            div()
                                .flex()
                                .items_start()
                                .gap_2()
                                .child(line_field(
                                    "network-nick",
                                    t!("networks-nick"),
                                    &self.fields.nick,
                                    nick_note(
                                        &form.nick,
                                        problems.nick,
                                        Some(t!("networks-nick-help")),
                                    ),
                                    &theme,
                                ))
                                .child(alt_nick),
                        )
                        .child(div().flex().items_start().gap_2().child(username).child(
                            line_field(
                                "network-realname",
                                t!("networks-realname"),
                                &self.fields.realname,
                                Some((t!("networks-realname-help").into(), false)),
                                &theme,
                            ),
                        ));
            } else {
                identity = identity.child(
                    div()
                        .flex()
                        .items_start()
                        .gap_2()
                        .child(alt_nick)
                        .child(username),
                );
            }
            root = root.child(identity);

            // Login.
            let mut methods = div()
                .id("network-login-method")
                .test_support()
                .role(Role::RadioGroup)
                .aria_label(t!("networks-login-method"))
                .flex()
                .gap_1();
            for login in Login::ALL {
                let active = self.login == login;
                let hover = theme.hover;
                methods = methods.child(
                    div()
                        .id(ElementId::Name(
                            format!("network-login-{}", login.wire()).into(),
                        ))
                        .test_support()
                        .role(Role::RadioButton)
                        .reachable(&theme)
                        .aria_label(login_label(login))
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .cursor_pointer()
                        .when(active, |d| d.bg(theme.accent).text_color(theme.on_accent))
                        .when(!active, |d| {
                            d.bg(theme.selected).hover(move |d| d.bg(hover))
                        })
                        .child(login_label(login))
                        .on_click(cx.listener(move |this, _, _, cx| this.set_login(login, cx))),
                );
            }
            let mut login = fieldset("network-login", t!("networks-login").into(), &theme)
                .child(methods)
                .child(hint(login_help(self.login), false, &theme));
            if self.login != Login::None {
                let password_note = if show(problems.password, "") {
                    Some((SharedString::from(t!("networks-problem-password")), true))
                } else if provided {
                    Some((SharedString::from(t!("networks-password-provided")), false))
                } else if password_set && !self.clear_password {
                    Some((SharedString::from(t!("networks-password-saved")), false))
                } else {
                    None
                };
                let mut row = div().flex().gap_2();
                if self.login == Login::Sasl {
                    let account_note = (problems.account && self.tried)
                        .then(|| (SharedString::from(t!("networks-problem-account")), true));
                    row = row.child(line_field(
                        "network-account",
                        t!("networks-account"),
                        &self.fields.account,
                        account_note,
                        &theme,
                    ));
                }
                row = row.child(line_field(
                    "network-password",
                    t!("networks-password"),
                    &self.fields.password,
                    password_note,
                    &theme,
                ));
                login = login.child(row);
                if existing.is_some() && password_set && form.password.is_empty() {
                    let label = if self.clear_password {
                        t!("networks-keep-password")
                    } else {
                        t!("networks-forget-password")
                    };
                    login = login.child(
                        div()
                            .id("network-forget-password")
                            .test_support()
                            .role(Role::Button)
                            .reachable(&theme)
                            .aria_label(label)
                            .text_xs()
                            .text_color(theme.link)
                            .cursor_pointer()
                            .child(label)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.clear_password = !this.clear_password;
                                cx.notify();
                            })),
                    );
                }
            }
            root = root.child(login);

            // On connect.
            root = root.child(
                fieldset(
                    "network-on-connect",
                    t!("networks-on-connect").into(),
                    &theme,
                )
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .child(lines_field(
                            "network-auto-join",
                            t!("networks-auto-join"),
                            &self.fields.auto_join,
                            &theme,
                        ))
                        .child(lines_field(
                            "network-perform",
                            t!("networks-perform"),
                            &self.fields.perform,
                            &theme,
                        )),
                )
                .child(hint(t!("networks-commands-help"), false, &theme)),
            );
        }

        if self.place == Place::Setup {
            root = root.child(self.actions(cx));
        }
        root
    }
}

impl NetworkFormView {
    /// Remove, Cancel and Save: the dialog's footer, or under the set-up form.
    fn actions(&self, cx: &mut Context<Self>) -> Div {
        let theme = theme::get(cx).clone();
        let form = self.form(cx);
        let existing = self.existing(cx);
        let problems = form.problems(existing.as_ref(), &self.presets);
        let needs_check = form.needs_check(existing.as_ref());
        let mut status = div().flex().flex_col();
        // The error, then the address check.
        if let Some(error) = &self.error {
            status = status.child(
                div()
                    .id("network-form-error")
                    .test_support()
                    .role(Role::Alert)
                    .aria_label(error.clone())
                    .mt_3()
                    .text_color(theme.danger)
                    .child(error.clone()),
            );
        }
        if needs_check {
            let text = match &self.check {
                Check::Checking => Some(SharedString::from(t!("networks-verifying"))),
                Check::Unreachable(detail) => Some(SharedString::from(detail.clone())),
                _ => None,
            };
            if let Some(text) = text {
                let failed = matches!(self.check, Check::Unreachable(_));
                status = status.child(
                    div()
                        .id("network-check")
                        .test_support()
                        .role(Role::Status)
                        .aria_label(text.clone())
                        .mt_3()
                        .flex()
                        .gap_2()
                        .text_xs()
                        .text_color(if failed { theme.danger } else { theme.muted })
                        .child(text)
                        .when(failed, |d| {
                            d.child(
                                div()
                                    .id("network-check-retry")
                                    .test_support()
                                    .role(Role::Button)
                                    .reachable(&theme)
                                    .aria_label(t!("networks-retry-check"))
                                    .text_color(theme.link)
                                    .cursor_pointer()
                                    .child(t!("networks-retry-check"))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.address_changed(cx);
                                        cx.notify();
                                    })),
                            )
                        }),
                );
            }
        }

        let checking = needs_check && self.check == Check::Checking;
        let blocked = needs_check && matches!(self.check, Check::Unreachable(_) | Check::Idle);
        let submit_label = if checking {
            t!("networks-verifying")
        } else if self.saving {
            t!("networks-saving")
        } else if existing.is_some() {
            t!("networks-save")
        } else if self.place == Place::Setup {
            t!("networks-connect-submit")
        } else {
            t!("networks-add-submit")
        };
        let mut actions = div()
            .w_full()
            .when(self.place == Place::Setup, |d| {
                d.mt_4().pt_3().border_t_1().border_color(theme.selected)
            })
            .flex()
            .items_center()
            .gap_2();
        if existing.is_some() {
            let (session, network) = (
                self.session.clone(),
                self.network.clone().unwrap_or_default(),
            );
            let this = cx.entity().downgrade();
            actions = actions.child(
                Button::new("network-remove")
                    .danger()
                    .label(t!("networks-remove"))
                    .on_click(move |_, window, cx| {
                        confirm_remove(
                            session.clone(),
                            network.clone(),
                            Some(this.clone()),
                            window,
                            cx,
                        )
                    }),
            );
        }
        actions = actions
            .child(div().flex_1())
            // The set-up page has nowhere to go back to: it is how a first
            // network is added.
            .when(self.place == Place::Dialog, |d| {
                d.child(
                    Button::new("network-cancel")
                        .ghost()
                        .label(t!("networks-cancel"))
                        .on_click(cx.listener(|this, _, window, cx| this.cancel(window, cx))),
                )
            })
            .child(
                Button::new("network-submit")
                    .primary()
                    .label(submit_label)
                    .disabled(
                        self.saving || checking || (blocked && !problems.host && !problems.port),
                    )
                    .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx))),
            );
        div()
            .w_full()
            .flex()
            .flex_col()
            .gap_2()
            .child(status)
            .child(actions)
    }
}

//! End-to-end encrypted direct messages (experimental, ircord/issues#19).
//!
//! The engine (`ircord_client::e2ee::Engine`) does the work; this file
//! starts one per signed-in client when the server offers encryption, and
//! shows it:
//!
//! - the lock in a direct conversation's header, closed when encrypted, with
//!   a check once the person is verified and a warning when something needs
//!   attention; it opens the encryption panel;
//! - "Start encrypted chat" and "Turn off encryption" in the conversation
//!   menu (turning off asks first and is visible to both people);
//! - the panel: what encryption means here, in plain words, the security
//!   code and its QR code to verify the other person, and a search of this
//!   device's encrypted messages;
//! - banners: the other person started fresh, a message arrived
//!   unencrypted, this device is not approved yet;
//! - in the composer, "Encrypted message to …"; sending, replies and files in
//!   an encrypted conversation go through the engine, never in the clear;
//! - in the timeline, the parts of long messages and events this client does
//!   not show are hidden, a run of messages from before this device was added
//!   is one line, and a message that arrived unencrypted is drawn in red.
//!
//! Device set-up and approval are in Settings → Encryption
//! (`settings/encryption.rs`). Everything is labelled experimental.

use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::{Rc, Weak};

use gpui_kit::TestSupportExt;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::{Disableable, Sizable};
use gpui_kit::prelude::*;
use gpui_kit::*;
use ircord_client::Client;
use ircord_client::e2ee::Engine;
use ircord_client::e2ee::engine as e2ee_rules;
use ircord_client::e2ee::vault::{Unreadable, VaultKey};
use ircord_client::store::e2ee::{self as view, Attention, Blocked, Shown};
use ircord_client::store::people;
use ircord_client::store::{Changes, Workspace};

use crate::menu_order;
use crate::registry::{
    self, BannerCtx, ComposerCtx, ConversationCtx, HeaderCtx, Inline, InlineRenderer, InlineSpan,
    MemberCtx, MenuItem, Panel, PanelCtx, RowFilter, SendFilter, SendOutcome, Slot, SubmitHook,
};
use crate::route::Route;
use crate::session::{Session, SessionEvent};
use crate::theme::{self, Theme};
use crate::timeline::rows::Row;
use crate::ui;
use crate::ui::confirm::{Confirm, confirm};
use crate::ui::focus::Reachable;
use crate::ui::icon::LucideIcon;

/// The right-hand panel's ID.
pub const PANEL: &str = "encryption";

/// One engine per signed-in client.
#[derive(Default)]
struct Engines(Vec<(Weak<Client>, Engine)>);

impl Global for Engines {}

/// The session, for what is drawn without one at hand (inline renderers,
/// the image loader).
#[derive(Default)]
struct Current(Option<WeakEntity<Session>>);

impl Global for Current {}

fn current_session(cx: &App) -> Option<Entity<Session>> {
    cx.try_global::<Current>()?.0.as_ref()?.upgrade()
}

pub fn init(cx: &mut App) {
    cx.default_global::<Engines>();
    cx.default_global::<Current>();
    cx.observe_new::<Session>(|session: &mut Session, _, cx| {
        let weak = cx.weak_entity();
        cx.global_mut::<Current>().0 = Some(weak);
        ensure(session, cx);
        cx.subscribe_self(|session: &mut Session, event: &SessionEvent, cx| {
            let check = match event {
                SessionEvent::Phase => true,
                SessionEvent::Changed(changes) => changes.reset,
                _ => false,
            };
            if check {
                ensure(session, cx);
            }
        })
        .detach();
    })
    .detach();

    registry::register(cx, |r| {
        r.panels.add(Panel::new(PANEL, panel));
        r.header_buttons
            .add(Slot::new("encryption-lock", lock_button).order(60));
        r.conversation_menu.add(
            MenuItem::new(
                "encryption-start",
                t!("encryption-start"),
                |ctx: &ConversationCtx, _, cx| {
                    if let Some(engine) = engine(&ctx.session, cx) {
                        engine.start_chat(&ctx.conversation);
                        open_panel(&ctx.session, cx);
                    }
                },
            )
            // Lucide's lock is not in the menu's icon set: an eye shut, for
            // "nobody else sees it".
            .icon(ui::icon::IconName::EyeOff)
            .when(|ctx, cx| offered(ctx, cx) && !is_on(&ctx.session, &ctx.conversation, cx))
            .unavailable_when(|ctx, cx| blocked_reason(&ctx.session, &ctx.conversation, cx))
            .order(menu_order::conversation::ENCRYPTION),
        );
        r.conversation_menu.add(
            MenuItem::new(
                "encryption-turn-off",
                t!("encryption-turn-off"),
                |ctx: &ConversationCtx, window, cx| {
                    ask_turn_off(&ctx.session, &ctx.conversation, window, cx);
                },
            )
            .icon(ui::icon::IconName::Eye)
            .when(|ctx, cx| is_on(&ctx.session, &ctx.conversation, cx))
            .order(menu_order::conversation::ENCRYPTION),
        );
        r.member_menu.add(
            MenuItem::new(
                "encryption-start-person",
                t!("encryption-start"),
                start_with_person,
            )
            .icon(ui::icon::IconName::EyeOff)
            .when(person_offered)
            .unavailable_when(person_blocked_reason)
            .order(menu_order::member::ENCRYPTED_MESSAGE),
        );
        r.banners.add(Slot::new("encryption", banner).order(5));
        r.composer
            .above
            .add(Slot::new("encryption", composer_note).order(5));
        r.composer.send_filters.add(SendFilter {
            id: "encryption".into(),
            // After commands and Markdown, before the length check (800) and
            // A1's reply and edit (900): an encrypted conversation's text
            // never reaches a filter that sends in the clear.
            order: 750,
            filter: Rc::new(send_filter),
        });
        r.composer.on_submit.add(SubmitHook {
            id: "encryption-files".into(),
            // Before U1's tray, which leaves an encrypted conversation's
            // files to this hook.
            order: -10,
            run: Rc::new(|ctx, _, _, cx| send_tray(&ctx.session, &ctx.conversation, cx)),
        });
        r.row_filters.add(RowFilter {
            id: "encryption".into(),
            order: 5,
            keep: Rc::new(keep_row),
        });
        r.inline_renderers.add(InlineRenderer {
            id: "encryption-unencrypted".into(),
            order: 5,
            spans: Rc::new(unencrypted_spans),
        });
    });
}

// ---- The engine ----

/// This client's engine, if encryption is offered and it started.
pub fn engine(session: &Entity<Session>, cx: &App) -> Option<Engine> {
    let client = session.read(cx).client()?;
    engine_of(client, cx)
}

fn engine_of(client: &Rc<Client>, cx: &App) -> Option<Engine> {
    cx.try_global::<Engines>()?
        .0
        .iter()
        .find(|(c, _)| c.upgrade().is_some_and(|c| Rc::ptr_eq(&c, client)))
        .map(|(_, e)| e.clone())
}

/// The credential store entry that holds this account's vault key.
fn key_name(origin: &str, user: &str) -> String {
    format!("e2ee-vault:{origin}:{user}")
}

/// This account's folder for the vault.
fn vault_dir(session: &Session, origin: &str, user: &str) -> PathBuf {
    let safe = |s: &str| -> String {
        s.chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
            .collect()
    };
    session
        .config
        .path()
        .join("e2ee")
        .join(safe(origin))
        .join(safe(user))
}

/// Start the engine for the signed-in client once the bootstrap says the
/// server offers encryption.
fn ensure(session: &mut Session, cx: &mut Context<Session>) {
    let engines = cx.default_global::<Engines>();
    engines.0.retain(|(c, _)| c.strong_count() > 0);
    let Some(client) = session.client().cloned() else {
        return;
    };
    if engine_of(&client, cx).is_some() {
        return;
    }
    let (offered, user) = {
        let w = client.read();
        (w.capabilities.encryption.is_some(), w.user.id.clone())
    };
    if !offered || user.is_empty() {
        return;
    }
    let origin = client.api().server().origin();
    let name = key_name(&origin, &user);
    let key = match session.credentials.load_secret(&name) {
        Ok(Some(text)) => VaultKey::decode(&text),
        Ok(None) => {
            let key = VaultKey::random();
            match session.credentials.save_secret(&name, &key.encode()) {
                Ok(()) => Some(key),
                Err(e) => {
                    failed(&client, e.to_string());
                    return;
                }
            }
        }
        Err(e) => {
            failed(&client, e.to_string());
            return;
        }
    };
    let Some(key) = key else {
        failed(&client, t!("encryption-key-unreadable").into());
        return;
    };
    let dir = vault_dir(session, &origin, &user);
    let engine = Engine::start(
        client.handle().clone(),
        session.runtime.handle().clone(),
        dir,
        key,
        t!("encryption-device-label", os = os_name()),
        texts(),
    );
    cx.global_mut::<Engines>()
        .0
        .push((Rc::downgrade(&client), engine));
}

fn failed(client: &Rc<Client>, why: String) {
    client.change(|w| {
        w.e2ee.device = view::DeviceState::Failed(why);
        Changes::topic(view::TOPIC)
    });
}

fn os_name() -> &'static str {
    match std::env::consts::OS {
        "windows" => "Windows",
        "macos" => "macOS",
        "linux" => "Linux",
        other => other,
    }
}

/// The lines the engine writes into timelines.
fn texts() -> ircord_client::e2ee::Texts {
    ircord_client::e2ee::Texts {
        control: Box::new(|on, name, mine| match (on, mine) {
            (true, true) => t!("encryption-line-on-you").into(),
            (true, false) => t!("encryption-line-on", name = name),
            (false, true) => t!("encryption-line-off-you").into(),
            (false, false) => t!("encryption-line-off", name = name),
        }),
        unreadable: Box::new(unreadable_text),
    }
}

pub fn unreadable_text(why: &Unreadable) -> String {
    match why {
        Unreadable::BeforeDevice => t!("encryption-unreadable-before").into(),
        Unreadable::OwnLost => t!("encryption-unreadable-own").into(),
        Unreadable::IdentityChanged => t!("encryption-unreadable-identity").into(),
        Unreadable::Forged => t!("encryption-unreadable-forged").into(),
        Unreadable::Failed(reason) => t!("encryption-unreadable-failed", reason = reason),
    }
}

// ---- What a conversation allows ----

fn with<R>(session: &Entity<Session>, cx: &App, f: impl FnOnce(&Workspace) -> R) -> Option<R> {
    let client = session.read(cx).client()?;
    let w = client.read();
    Some(f(&w))
}

fn is_on(session: &Entity<Session>, conversation: &str, cx: &App) -> bool {
    with(session, cx, |w| w.e2ee.is_on(conversation)).unwrap_or(false)
}

/// Whether encryption is a choice here at all: a direct conversation, on a
/// server that offers it.
fn offered(ctx: &ConversationCtx, cx: &App) -> bool {
    with(&ctx.session, cx, |w| {
        w.capabilities.encryption.is_some()
            && w.conversation(&ctx.conversation)
                .is_some_and(|c| c.kind == ircord_client::contract::ConversationKind::Direct)
    })
    .unwrap_or(false)
}

/// The other person's name in a direct conversation: the conversation is
/// named after them.
fn peer_name(session: &Entity<Session>, conversation: &str, cx: &App) -> String {
    with(session, cx, |w| {
        let c = w.conversation(conversation)?;
        let member = c
            .member_ids
            .iter()
            .find(|id| !w.is_self(id, &c.network_id))
            .map(|id| w.display_name(id, &c.network_id));
        Some(member.unwrap_or_else(|| c.name.clone()))
    })
    .flatten()
    .unwrap_or_default()
}

/// Why encryption cannot be started here now, in words.
fn blocked_reason(session: &Entity<Session>, conversation: &str, cx: &App) -> Option<SharedString> {
    if let Some(engine) = engine(session, cx) {
        engine.check_peer(conversation);
    }
    let blocked = with(session, cx, |w| {
        ircord_client::e2ee::engine::blocked(w, conversation)
    })
    .flatten()?;
    let name = peer_name(session, conversation, cx);
    Some(blocked_text(&blocked, &name).into())
}

pub fn blocked_text(blocked: &Blocked, name: &str) -> String {
    match blocked {
        Blocked::Off => t!("encryption-blocked-off").into(),
        Blocked::NotReady => t!("encryption-blocked-not-ready").into(),
        Blocked::NotHere => t!("encryption-blocked-not-here").into(),
        Blocked::Network => t!("encryption-blocked-network").into(),
        Blocked::NoAccount => t!("encryption-blocked-no-account").into(),
        Blocked::PeerNotSetUp => t!("encryption-blocked-peer", name = name),
    }
}

// ---- The person menu's entry ----

/// Someone else, on a server with encryption on, whose direct conversation
/// (if there is one) is not encrypted yet.
fn person_offered(ctx: &MemberCtx, cx: &App) -> bool {
    let Some(network) = super::profile::network_of(ctx, cx) else {
        return false;
    };
    with(&ctx.session, cx, |w| {
        w.capabilities.encryption.is_some()
            && !w.is_self(&ctx.member, &network)
            && w.members.contains_key(&ctx.member)
            && !e2ee_rules::direct_with(w, &network, &ctx.member)
                .is_some_and(|dm| w.e2ee.is_on(&dm))
    })
    .unwrap_or(false)
}

/// Why an encrypted chat with this person cannot start now, in the words
/// the conversation's lock uses.
fn person_blocked_reason(ctx: &MemberCtx, cx: &App) -> Option<SharedString> {
    let network = super::profile::network_of(ctx, cx)?;
    let (blocked, name, dm) = with(&ctx.session, cx, |w| {
        let blocked = e2ee_rules::blocked_with(w, &network, &ctx.member)?;
        let name = w
            .members
            .get(&ctx.member)
            .map(|m| people::name(m).to_string())
            .unwrap_or_default();
        Some((
            blocked,
            name,
            e2ee_rules::direct_with(w, &network, &ctx.member),
        ))
    })
    .flatten()?;
    // Learn whether they have set up encryption, as the lock does.
    if let (Some(dm), Some(engine)) = (dm, engine(&ctx.session, cx)) {
        engine.check_peer(&dm);
    }
    Some(blocked_text(&blocked, &name).into())
}

/// Open (or start) the direct conversation with this person, then start
/// encrypting it and show the panel, as the conversation menu's entry does.
fn start_with_person(ctx: &MemberCtx, window: &mut Window, cx: &mut App) {
    let Some(network) = super::profile::network_of(ctx, cx) else {
        return;
    };
    let Some(nick) = with(&ctx.session, cx, |w| {
        w.members.get(&ctx.member).map(|m| m.nick.clone())
    })
    .flatten() else {
        return;
    };
    let (session, member) = (ctx.session.clone(), ctx.member.clone());
    super::profile::message_then(
        &network,
        Some(member.clone()),
        &nick,
        window,
        cx,
        move |conversation, _, cx| {
            let Some(client) = session.read(cx).client().cloned() else {
                return;
            };
            let conversation = conversation.to_string();
            client.change(|w| e2ee_rules::note_peer(w, &conversation, &member));
            if let Some(engine) = engine(&session, cx) {
                let ready = with(&session, cx, |w| {
                    e2ee_rules::blocked(w, &conversation).is_none()
                })
                .unwrap_or(false);
                if ready {
                    engine.start_chat(&conversation);
                } else {
                    engine.check_peer(&conversation);
                }
            }
            open_panel(&session, cx);
        },
    );
}

/// Whether the composer must leave `conversation`'s files to this feature:
/// U1's tray then sends nothing itself, so no file goes up unencrypted.
pub fn takes_uploads(session: &Entity<Session>, conversation: &str, cx: &App) -> bool {
    guarded(session, conversation, cx)
}

/// Whether nothing may be sent in the clear into `conversation` now
/// (`ircord_client::store::e2ee::guarded`). Asks the backend about the
/// conversation, once, as a side effect.
pub fn guarded(session: &Entity<Session>, conversation: &str, cx: &App) -> bool {
    let Some(client) = session.read(cx).client() else {
        return false;
    };
    ircord_client::e2ee::engine::check_conversation(client.handle(), conversation);
    ircord_client::store::e2ee::guarded(&client.read(), conversation)
}

/// The engine, when this device can send encrypted into `conversation` now;
/// otherwise says why and gives `None`, and the caller sends nothing.
fn sendable(
    session: &Entity<Session>,
    conversation: &str,
    window: &mut Window,
    cx: &mut App,
) -> Option<Engine> {
    let (on, ready, loaded) = with(session, cx, |w| {
        (
            w.e2ee.is_on(conversation),
            w.e2ee.device.ready(),
            w.e2ee.loaded,
        )
    })?;
    let engine = engine(session, cx);
    if on && ready && engine.is_some() {
        return engine;
    }
    let why = if !loaded {
        t!("encryption-checking")
    } else {
        t!("encryption-blocked-not-ready")
    };
    ui::toast::error(window, cx, why);
    None
}

/// For features that send by themselves (GIFs, `/slap`, `/msg`): true, with
/// the reason shown, when `conversation` must not get plaintext. This
/// version sends only text and files encrypted.
pub fn refuses_plaintext(
    session: &Entity<Session>,
    conversation: &str,
    window: &mut Window,
    cx: &mut App,
) -> bool {
    if !guarded(session, conversation, cx) {
        return false;
    }
    ui::toast::error(window, cx, t!("encryption-not-here"));
    true
}

fn open_panel(session: &Entity<Session>, cx: &mut App) {
    session.update(cx, |s, cx| s.set_panel(Some(SharedString::from(PANEL)), cx));
}

fn ask_turn_off(session: &Entity<Session>, conversation: &str, window: &mut Window, cx: &mut App) {
    let name = peer_name(session, conversation, cx);
    let (session, conversation) = (session.clone(), conversation.to_string());
    confirm(
        window,
        cx,
        Confirm {
            title: t!("encryption-turn-off-title").into(),
            body: t!("encryption-turn-off-body", name = name).into(),
            ok: t!("encryption-turn-off-ok").into(),
            danger: true,
        },
        move |_, cx| {
            if let Some(engine) = engine(&session, cx) {
                engine.turn_off(&conversation);
            }
        },
    );
}

// ---- The header's lock ----

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Lock {
    Open,
    Closed,
    Verified,
    Attention,
}

/// What the lock shows for a conversation.
pub fn lock_of(w: &Workspace, conversation: &str) -> Lock {
    match w.e2ee.conversation(conversation) {
        Some(c) if c.attention.is_some() || (c.on && c.unencrypted) => Lock::Attention,
        Some(c) if c.on && c.verified => Lock::Verified,
        Some(c) if c.on => Lock::Closed,
        _ => Lock::Open,
    }
}

fn lock_button(ctx: &HeaderCtx, _window: &mut Window, cx: &mut App) -> Option<AnyElement> {
    let Route::Conversation(id) = &ctx.route else {
        return None;
    };
    // Learn early whether the backend says this conversation is encrypted.
    let _ = guarded(&ctx.session, id, cx);
    let shown = with(&ctx.session, cx, |w| {
        w.capabilities.encryption.is_some()
            && w.conversation(id)
                .is_some_and(|c| c.kind == ircord_client::contract::ConversationKind::Direct)
    })
    .unwrap_or(false);
    if !shown {
        return None;
    }
    Some(watched(&ctx.session, id, Part::Lock, cx).into_any_element())
}

// ---- Views that redraw when encryption changes ----

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Part {
    Lock,
    Banner,
    Composer,
}

#[derive(Default)]
struct Views(HashMap<(Part, String), Entity<Watched>>);

impl Global for Views {}

/// The view for one part of one conversation, made once.
fn watched(
    session: &Entity<Session>,
    conversation: &str,
    part: Part,
    cx: &mut App,
) -> Entity<Watched> {
    let key = (part, conversation.to_string());
    let known = cx.default_global::<Views>().0.get(&key).cloned();
    if let Some(v) = known
        && v.read(cx).session == *session
    {
        return v;
    }
    let view = cx.new(|cx| Watched::new(session.clone(), conversation.to_string(), part, cx));
    cx.global_mut::<Views>().0.insert(key, view.clone());
    view
}

pub struct Watched {
    session: Entity<Session>,
    conversation: String,
    part: Part,
    _subscription: Subscription,
}

impl Watched {
    fn new(
        session: Entity<Session>,
        conversation: String,
        part: Part,
        cx: &mut Context<Self>,
    ) -> Self {
        let subscription = cx.subscribe(&session, |_, _, event: &SessionEvent, cx| match event {
            SessionEvent::Changed(changes) if changes.touches(view::TOPIC) || changes.members => {
                cx.notify()
            }
            SessionEvent::Phase | SessionEvent::Route => cx.notify(),
            _ => {}
        });
        Self {
            session,
            conversation,
            part,
            _subscription: subscription,
        }
    }
}

impl Render for Watched {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme::get(cx).clone();
        match self.part {
            Part::Lock => self.lock(&theme, cx).into_any_element(),
            Part::Banner => self.banner(&theme, cx),
            Part::Composer => self.note(&theme, cx),
        }
    }
}

impl Watched {
    fn lock(&self, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let lock =
            with(&self.session, cx, |w| lock_of(w, &self.conversation)).unwrap_or(Lock::Open);
        let name = peer_name(&self.session, &self.conversation, cx);
        let (icon, label, color): (LucideIcon, SharedString, Hsla) = match lock {
            Lock::Open => (
                LucideIcon::LockOpen,
                t!("encryption-lock-off").into(),
                theme.muted,
            ),
            Lock::Closed => (
                LucideIcon::Lock,
                t!("encryption-lock-on").into(),
                theme.online,
            ),
            Lock::Verified => (
                LucideIcon::ShieldCheck,
                t!("encryption-lock-verified", name = name).into(),
                theme.online,
            ),
            Lock::Attention => (
                LucideIcon::ShieldAlert,
                t!("encryption-lock-attention").into(),
                theme.warning,
            ),
        };
        let session = self.session.clone();
        let open = session.read(cx).panel.as_deref() == Some(PANEL);
        div()
            .id("encryption-lock")
            .test_support()
            .role(Role::Button)
            .reachable(theme)
            .aria_label(label.clone())
            .tooltip(ui::tooltip::text(label))
            .p_1()
            .rounded_md()
            .cursor_pointer()
            .hover({
                let hover = theme.hover;
                move |d| d.bg(hover)
            })
            .child(ui::icon::icon(icon, color))
            .on_click(move |_, _, cx| {
                session.update(cx, |s, cx| {
                    s.set_panel((!open).then(|| SharedString::from(PANEL)), cx)
                });
            })
    }

    fn banner(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let Some((attention, unencrypted, on, ready)) = with(&self.session, cx, |w| {
            let c = w.e2ee.conversation(&self.conversation);
            (
                c.and_then(|c| c.attention.clone()),
                c.is_some_and(|c| c.on && c.unencrypted),
                c.is_some_and(|c| c.on),
                w.e2ee.device.ready(),
            )
        }) else {
            return div().into_any_element();
        };
        let name = peer_name(&self.session, &self.conversation, cx);
        let session = self.session.clone();
        let conversation = self.conversation.clone();
        if let Some(Attention::StartedFresh { .. }) = attention {
            let (s1, c1) = (session.clone(), conversation.clone());
            let (s2, c2) = (session.clone(), conversation.clone());
            return strip("encryption-banner-fresh", theme.warning, theme)
                .child(text_column(
                    t!("encryption-banner-fresh", name = name.clone()),
                    theme,
                ))
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .child(
                            Button::new("encryption-banner-verify")
                                .small()
                                .label(t!("encryption-banner-verify", name = name))
                                .on_click(move |_, _, cx| open_panel(&s1, cx)),
                        )
                        .child(
                            Button::new("encryption-banner-ok")
                                .small()
                                .primary()
                                .label(t!("encryption-banner-ok"))
                                .on_click(move |_, _, cx| {
                                    if let Some(engine) = engine(&s2, cx) {
                                        engine.acknowledge(&c2);
                                    }
                                    let _ = &c1;
                                }),
                        ),
                )
                .test_support()
                .into_any_element();
        }
        if unencrypted {
            return strip("encryption-banner-unencrypted", theme.danger, theme)
                .child(text_column(
                    t!("encryption-banner-unencrypted").into(),
                    theme,
                ))
                .test_support()
                .into_any_element();
        }
        if on && !ready {
            return strip("encryption-banner-not-ready", theme.warning, theme)
                .child(text_column(t!("encryption-banner-not-ready").into(), theme))
                .child(
                    Button::new("encryption-banner-settings")
                        .small()
                        .label(t!("encryption-open-settings"))
                        .on_click(move |_, _, cx| {
                            crate::settings::open(&session, Some("encryption-device"), cx)
                        }),
                )
                .test_support()
                .into_any_element();
        }
        div().into_any_element()
    }

    fn note(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let Some((on, error, busy)) = with(&self.session, cx, |w| {
            (
                w.e2ee.is_on(&self.conversation),
                w.e2ee.errors.get(&self.conversation).cloned(),
                w.e2ee.busy.contains(&self.conversation),
            )
        }) else {
            return div().into_any_element();
        };
        let name = peer_name(&self.session, &self.conversation, cx);
        let mut column = div().flex().flex_col().gap_0p5().px_3().pb_1();
        if on {
            column = column.child(
                div()
                    .id("encryption-composer")
                    .test_support()
                    .role(Role::Status)
                    .aria_label(SharedString::from(t!(
                        "encryption-composer",
                        name = name.clone()
                    )))
                    .flex()
                    .items_center()
                    .gap_1()
                    .text_xs()
                    .text_color(theme.online)
                    .child(ui::icon::icon(LucideIcon::Lock, theme.online))
                    .child(t!("encryption-composer", name = name.clone()))
                    .child(
                        div()
                            .text_color(theme.muted)
                            .child(t!("encryption-experimental-tag")),
                    ),
            );
        }
        if busy {
            column = column.child(
                div()
                    .text_xs()
                    .text_color(theme.muted)
                    .child(t!("encryption-busy")),
            );
        }
        if let Some(error) = error {
            column = column.child(
                div()
                    .id("encryption-composer-error")
                    .test_support()
                    .role(Role::Alert)
                    .aria_label(SharedString::from(error_text(&error, &name)))
                    .text_xs()
                    .text_color(theme.danger)
                    .child(error_text(&error, &name)),
            );
        }
        column.into_any_element()
    }
}

/// The engine's short error codes, in words; anything else as it came.
pub fn error_text(error: &str, name: &str) -> String {
    match error {
        "peer-not-set-up" => t!("encryption-blocked-peer", name = name),
        "not-ready" => t!("encryption-blocked-not-ready").into(),
        "started-fresh" => t!("encryption-error-started-fresh", name = name),
        "not-encrypted" => t!("encryption-error-not-encrypted").into(),
        other => t!("encryption-error", error = other),
    }
}

/// A banner's frame; the caller adds `.test_support()` once it is filled.
fn strip(id: &'static str, tint: Hsla, theme: &Theme) -> Stateful<Div> {
    div()
        .id(id)
        .role(Role::Alert)
        .flex()
        .items_center()
        .gap_3()
        .px_4()
        .py_2()
        .border_b_1()
        .border_color(theme.border)
        .bg(tint.opacity(0.12))
}

fn text_column(text: String, theme: &Theme) -> impl IntoElement {
    div()
        .flex_1()
        .min_w_0()
        .text_sm()
        .text_color(theme.text)
        .child(text)
}

fn banner(ctx: &BannerCtx, _window: &mut Window, cx: &mut App) -> Option<AnyElement> {
    let Some(Route::Conversation(id)) = &ctx.route else {
        return None;
    };
    let relevant = with(&ctx.session, cx, |w| w.e2ee.conversation(id).is_some()).unwrap_or(false);
    relevant.then(|| watched(&ctx.session, id, Part::Banner, cx).into_any_element())
}

fn composer_note(ctx: &ComposerCtx, _window: &mut Window, cx: &mut App) -> Option<AnyElement> {
    let relevant = with(&ctx.session, cx, |w| {
        w.e2ee.conversation(&ctx.conversation).is_some()
            || w.e2ee.errors.contains_key(&ctx.conversation)
    })
    .unwrap_or(false);
    relevant
        .then(|| watched(&ctx.session, &ctx.conversation, Part::Composer, cx).into_any_element())
}

// ---- Sending ----

fn send_filter(ctx: &ComposerCtx, text: String, window: &mut Window, cx: &mut App) -> SendOutcome {
    if !guarded(&ctx.session, &ctx.conversation, cx) {
        return SendOutcome::Continue(text);
    }
    let Some(engine) = sendable(&ctx.session, &ctx.conversation, window, cx) else {
        return SendOutcome::Keep;
    };
    let reply = match crate::features::message_actions::mode(&ctx.conversation, cx) {
        Some(crate::features::message_actions::Mode::Reply { message, .. }) => Some(message),
        Some(crate::features::message_actions::Mode::Edit { .. }) => {
            // Editing goes in the clear on the server's side; an encrypted
            // edit is not in this version.
            ui::toast::error(window, cx, t!("encryption-no-edit"));
            return SendOutcome::Keep;
        }
        None => None,
    };
    if reply.is_some() {
        crate::features::message_actions::cancel(&ctx.conversation, window, cx);
    }
    let text = text.trim_end_matches(['\n', '\r']).to_string();
    if !text.trim().is_empty() {
        engine.send(&ctx.conversation, &text, reply);
    }
    send_tray(&ctx.session, &ctx.conversation, cx);
    SendOutcome::Handled
}

/// Send the files waiting in the upload tray, encrypted.
fn send_tray(session: &Entity<Session>, conversation: &str, cx: &mut App) {
    let ready = with(session, cx, |w| {
        w.e2ee.is_on(conversation) && w.e2ee.device.ready()
    })
    .unwrap_or(false);
    if !ready {
        // Guarded files stay in the tray (U1 sends none of them).
        return;
    }
    let pending = crate::composer::uploads::Uploads::pending(cx, conversation);
    if pending.is_empty() {
        return;
    }
    let Some(engine) = engine(session, cx) else {
        return;
    };
    let Some(network) = with(session, cx, |w| {
        w.conversation(conversation).map(|c| c.network_id.clone())
    })
    .flatten() else {
        return;
    };
    let files: Vec<_> = pending.iter().map(|p| p.file.clone()).collect();
    for p in &pending {
        crate::composer::uploads::remove(session, conversation, p.key, cx);
    }
    let failed = {
        let id = conversation.to_string();
        let handle = engine.handle().clone();
        move |result: Result<(), String>| {
            if let Err(e) = result {
                handle.change(|w| {
                    w.e2ee.errors.insert(id.clone(), e);
                    Changes::topic_id(view::TOPIC, id)
                });
            }
        }
    };
    engine.send_files(conversation, &network, files, failed);
}

// ---- The timeline ----

fn keep_row(row: &Row, w: &Workspace, _cx: &App) -> bool {
    match row {
        Row::Message(m) => !matches!(
            w.e2ee.shown.get(&m.id),
            Some(Shown::Hidden) | Some(Shown::Folded)
        ),
        Row::Pending(p) => !w.e2ee.hidden_sends.contains(&p.send.client_id),
        _ => true,
    }
}

/// A message that arrived in the clear in an encrypted chat: drawn in red.
fn unencrypted_spans(
    row: &crate::timeline::rows::MessageRow,
    theme: &Theme,
    cx: &App,
) -> Vec<InlineSpan> {
    let Some(session) = current_session(cx) else {
        return Vec::new();
    };
    let marked = with(&session, cx, |w| w.e2ee.unencrypted.contains(&row.id)).unwrap_or(false);
    if !marked || row.body.text.is_empty() {
        return Vec::new();
    }
    vec![InlineSpan {
        range: 0..row.body.text.len(),
        inline: Inline::Style(HighlightStyle {
            color: Some(theme.danger),
            underline: Some(UnderlineStyle {
                thickness: px(1.),
                color: Some(theme.danger),
                wavy: true,
            }),
            ..Default::default()
        }),
    }]
}

// ---- The panel ----

fn panel(ctx: &PanelCtx, window: &mut Window, cx: &mut App) -> AnyElement {
    let theme = theme::get(cx).clone();
    let Some(conversation) = ctx.conversation.clone() else {
        return ui::empty::empty(
            "encryption-panel-none",
            t!("encryption-panel-none"),
            None,
            &theme,
        )
        .into_any_element();
    };
    let view = match cx.try_global::<PanelView>() {
        Some(PanelView(view))
            if view.read(cx).session == ctx.session
                && view.read(cx).conversation == conversation =>
        {
            view.clone()
        }
        _ => {
            let session = ctx.session.clone();
            let view = cx.new(|cx| Details::new(session, conversation, window, cx));
            cx.set_global(PanelView(view.clone()));
            view
        }
    };
    view.into_any_element()
}

struct PanelView(Entity<Details>);

impl Global for PanelView {}

pub struct Details {
    session: Entity<Session>,
    conversation: String,
    code: Option<ircord_client::e2ee::engine::Code>,
    asked: bool,
    query: Entity<gpui_kit::component::input::InputState>,
    hits: Vec<(String, String)>,
    _subscriptions: Vec<Subscription>,
}

impl Details {
    fn new(
        session: Entity<Session>,
        conversation: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let subscription = cx.subscribe(
            &session,
            move |this: &mut Self, _, event: &SessionEvent, cx| {
                if let SessionEvent::Changed(changes) = event
                    && changes.touches(view::TOPIC)
                {
                    this.asked = false;
                    cx.notify();
                }
            },
        );
        let query = cx.new(|cx| {
            gpui_kit::component::input::InputState::new(window, cx)
                .placeholder(t!("encryption-panel-search"))
        });
        let search = cx.subscribe(&query, |this: &mut Self, input, event, cx| {
            if let gpui_kit::component::input::InputEvent::Change = event {
                let text = input.read(cx).value().to_string();
                this.search(text, cx);
            }
        });
        Self {
            session,
            conversation,
            code: None,
            asked: false,
            query,
            hits: Vec::new(),
            _subscriptions: vec![subscription, search],
        }
    }

    fn load_code(&mut self, cx: &mut Context<Self>) {
        if self.asked {
            return;
        }
        self.asked = true;
        let Some(engine) = engine(&self.session, cx) else {
            return;
        };
        let conversation = self.conversation.clone();
        let task = self
            .session
            .read(cx)
            .runtime
            .spawn(async move { engine.code(&conversation).await });
        cx.spawn(async move |this, cx| {
            let code = task.await.ok().flatten();
            let _ = this.update(cx, |this, cx| {
                this.code = code;
                cx.notify();
            });
        })
        .detach();
    }

    fn search(&mut self, text: String, cx: &mut Context<Self>) {
        let Some(engine) = engine(&self.session, cx) else {
            return;
        };
        if text.trim().is_empty() {
            self.hits.clear();
            cx.notify();
            return;
        }
        let conversation = self.conversation.clone();
        let task = self
            .session
            .read(cx)
            .runtime
            .spawn(async move { engine.search(&text, 50).await });
        cx.spawn(async move |this, cx| {
            let hits = task.await.unwrap_or_default();
            let _ = this.update(cx, |this, cx| {
                this.hits = hits
                    .into_iter()
                    .filter(|(_, p)| p.conversation == conversation)
                    .map(|(id, p)| (id, p.text))
                    .take(20)
                    .collect();
                cx.notify();
            });
        })
        .detach();
    }
}

impl Render for Details {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme::get(cx).clone();
        let name = peer_name(&self.session, &self.conversation, cx);
        let Some((state, lock, ready)) = with(&self.session, cx, |w| {
            (
                w.e2ee.conversation(&self.conversation).cloned(),
                lock_of(w, &self.conversation),
                w.e2ee.device.ready(),
            )
        }) else {
            return div().into_any_element();
        };
        let on = state.as_ref().is_some_and(|s| s.on);
        if on && ready {
            self.load_code(cx);
        }
        let session = self.session.clone();
        let conversation = self.conversation.clone();
        let mut body = div()
            .id("encryption-panel")
            .test_support()
            .size_full()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap_4()
            .p_4()
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_2()
                    .child(ui::icon::icon(
                        if on {
                            LucideIcon::Lock
                        } else {
                            LucideIcon::LockOpen
                        },
                        if on { theme.online } else { theme.muted },
                    ))
                    .child(
                        div()
                            .text_color(theme.text)
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(t!("encryption-panel-title")),
                    )
                    .child(tag(t!("encryption-experimental"), &theme)),
            )
            .child(
                div()
                    .id("encryption-panel-state")
                    .test_support()
                    .role(Role::Status)
                    .text_sm()
                    .text_color(theme.text)
                    .child(match (on, lock) {
                        (true, Lock::Verified) => {
                            t!("encryption-panel-on-verified", name = name.clone())
                        }
                        (true, _) => t!("encryption-panel-on", name = name.clone()),
                        (false, _) => t!("encryption-panel-off").into(),
                    }),
            );
        if !on {
            let blocked = blocked_reason(&self.session, &self.conversation, cx);
            let (s, c) = (session.clone(), conversation.clone());
            body = body.child(
                Button::new("encryption-panel-start")
                    .primary()
                    .label(t!("encryption-start"))
                    .disabled(blocked.is_some())
                    .on_click(move |_, _, cx| {
                        if let Some(engine) = engine(&s, cx) {
                            engine.start_chat(&c);
                        }
                    }),
            );
            if let Some(why) = blocked {
                body = body.child(
                    div()
                        .id("encryption-panel-blocked")
                        .test_support()
                        .role(Role::Status)
                        .text_sm()
                        .text_color(theme.warning)
                        .child(why),
                );
            }
            return body
                .child(
                    div()
                        .id("encryption-panel-caveat")
                        .test_support()
                        .role(Role::Note)
                        .text_sm()
                        .text_color(theme.muted)
                        .child(t!("encryption-caveat", name = name.clone())),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted)
                        .child(t!("encryption-panel-experimental")),
                )
                .into_any_element();
        }
        body = body
            .child(
                div()
                    .id("encryption-panel-caveat")
                    .test_support()
                    .role(Role::Note)
                    .text_sm()
                    .text_color(theme.muted)
                    .child(t!("encryption-caveat", name = name.clone())),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(theme.muted)
                    .child(t!("encryption-panel-experimental")),
            );
        body = body.child(
            div().child(
                Button::new("encryption-panel-turn-off")
                    .small()
                    .danger()
                    .label(t!("encryption-turn-off"))
                    .on_click({
                        let (s, c) = (session.clone(), conversation.clone());
                        move |_, window, cx| ask_turn_off(&s, &c, window, cx)
                    }),
            ),
        );
        if let Some(code) = self.code.clone() {
            let (s, c) = (session.clone(), conversation.clone());
            let verified = code.verified;
            body = body
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.text)
                        .child(t!("encryption-panel-code")),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(theme.muted)
                        .child(t!("encryption-panel-code-help", name = name.clone())),
                )
                .child(
                    div()
                        .id("encryption-panel-digits")
                        .test_support()
                        .role(Role::Note)
                        .aria_label(SharedString::from(code.groups.join(" ")))
                        .w(px(260.))
                        .flex()
                        .flex_wrap()
                        .gap_y_1()
                        .font_family(theme.mono_font.clone())
                        .text_color(theme.text)
                        .children(
                            code.groups
                                .iter()
                                .map(|g| div().w(px(65.)).child(g.clone())),
                        ),
                )
                .child(
                    Button::new("encryption-panel-verify")
                        .label(if verified {
                            t!("encryption-panel-unverify").to_string()
                        } else {
                            t!("encryption-panel-verify", name = name.clone())
                        })
                        .when(!verified, |b| b.primary())
                        .on_click(move |_, _, cx| {
                            if let Some(engine) = engine(&s, cx) {
                                engine.set_verified(&c, !verified);
                            }
                        }),
                )
                .child(qr_code(
                    "encryption-panel-qr",
                    &code.qr,
                    t!("encryption-panel-qr"),
                    4.0,
                ));
        }
        body = body
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.text)
                    .child(t!("encryption-panel-search-title")),
            )
            .child(gpui_kit::component::input::Input::new(&self.query).small())
            .children(self.hits.iter().enumerate().map(|(i, (id, text))| {
                let (s, c, id) = (session.clone(), conversation.clone(), id.clone());
                div()
                    .id(("encryption-hit", i))
                    .test_support()
                    .role(Role::Button)
                    .reachable(&theme)
                    .aria_label(SharedString::from(text.clone()))
                    .p_1()
                    .rounded_md()
                    .text_sm()
                    .text_color(theme.text)
                    .cursor_pointer()
                    .hover({
                        let hover = theme.hover;
                        move |d| d.bg(hover)
                    })
                    .child(text.chars().take(120).collect::<String>())
                    .on_click(move |_, _, cx| {
                        if let Some(client) = s.read(cx).client().cloned() {
                            client.change(|w| w.focus(&c, &id));
                        }
                    })
            }));
        let _ = window;
        body.into_any_element()
    }
}

fn tag(text: &'static str, theme: &Theme) -> impl IntoElement {
    div()
        .px_1p5()
        .rounded_md()
        .text_xs()
        .text_color(theme.warning)
        .bg(theme.warning.opacity(0.14))
        .child(text)
}

/// A QR code, dark on white whatever the theme, with its quiet zone.
pub fn qr_code(id: &'static str, text: &str, label: &'static str, module: f32) -> impl IntoElement {
    let rows = ircord_client::e2ee::files::qr(text).unwrap_or_default();
    let n = rows.len();
    let quiet = 4.0;
    let side = (n as f32 + 2.0 * quiet) * module;
    div()
        .id(id)
        .test_support()
        .role(Role::Image)
        .aria_label(SharedString::from(label))
        .flex_none()
        .w(px(side))
        .h(px(side))
        .bg(gpui_kit::white())
        .rounded_md()
        .child(
            canvas(
                |_, _, _| {},
                move |bounds, _, window, _| {
                    let origin = bounds.origin;
                    for (y, row) in rows.iter().enumerate() {
                        for (x, dark) in row.iter().enumerate() {
                            if *dark {
                                let at = point(
                                    origin.x + px((x as f32 + quiet) * module),
                                    origin.y + px((y as f32 + quiet) * module),
                                );
                                window.paint_quad(fill(
                                    Bounds::new(at, size(px(module), px(module))),
                                    gpui_kit::black(),
                                ));
                            }
                        }
                    }
                },
            )
            .size_full(),
        )
}

/// Save an encrypted message's file: decrypt it into the downloads folder
/// under a name no other file has, and show it in its folder. It is not
/// opened: a file from someone else must not run on one click. `false` when
/// `url` is not one (open it as usual).
pub fn open_local(url: &str, cx: &mut App) -> bool {
    let Some(open) = ircord_client::e2ee::engine::local_file(url) else {
        return false;
    };
    let Some(session) = current_session(cx) else {
        return true;
    };
    let task = session.read(cx).runtime.spawn(open);
    cx.spawn(async move |cx| {
        let saved = match task.await {
            Ok(Ok(file)) => {
                let dir = dirs::download_dir().unwrap_or_else(std::env::temp_dir);
                let name = std::path::Path::new(&file.name)
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .filter(|n| !n.is_empty())
                    .unwrap_or_else(|| "file".into());
                let path = free_path(&dir, &name);
                std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&path)
                    .and_then(|mut f| std::io::Write::write_all(&mut f, &file.bytes))
                    .map(|_| path)
                    .map_err(|e| e.to_string())
            }
            Ok(Err(e)) => Err(e),
            Err(e) => Err(e.to_string()),
        };
        cx.update(|cx| match saved {
            Ok(path) => cx.reveal_path(&path),
            Err(e) => log::warn!("open an encrypted file: {e}"),
        });
    })
    .detach();
    true
}

/// `dir/name`, or `dir/name (2)`… (before the extension) when taken.
fn free_path(dir: &std::path::Path, name: &str) -> std::path::PathBuf {
    let first = dir.join(name);
    if !first.exists() {
        return first;
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => (stem, format!(".{ext}")),
        _ => (name, String::new()),
    };
    (2..)
        .map(|n| dir.join(format!("{stem} ({n}){ext}")))
        .find(|p| !p.exists())
        .expect("some name is free")
}

#[cfg(test)]
mod tests {
    use super::{Unreadable, error_text, free_path, unreadable_text};

    #[test]
    fn a_saved_file_never_replaces_another() {
        let dir = std::env::temp_dir().join(format!("ircord-free-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), b"x").unwrap();
        assert_eq!(free_path(&dir, "a.txt"), dir.join("a (2).txt"));
        assert_eq!(free_path(&dir, "b.txt"), dir.join("b.txt"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn errors_have_words() {
        assert!(error_text("peer-not-set-up", "Bob").contains("Bob"));
        assert!(!error_text("something odd", "Bob").is_empty());
        assert!(!unreadable_text(&Unreadable::BeforeDevice).is_empty());
    }

    #[test]
    fn a_qr_code_fits_its_box() {
        let rows = ircord_client::e2ee::files::qr("ircord-e2ee:1:00").unwrap();
        assert!(rows.len() < 60);
    }
}

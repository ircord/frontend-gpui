//! Profile cards and the member menu (package P1).
//!
//! The card opens from the member list, a name or picture in the timeline
//! and the member menu. It shows who they are (profile fields, badges, the
//! channels you share), asks the IRC server about them (WHOIS, quiet to
//! them) and, on request, their client (CTCP VERSION, TIME and PING). The
//! member menu messages, mentions, copies, ignores, looks up and invites;
//! server owners mark bots. Ignored people's messages are hidden from the
//! timeline, and an empty direct conversation starts with the person.

use crate::menu_order;
use crate::ui::focus::Reachable;
use std::rc::{Rc, Weak};

use gpui_kit::TestSupportExt;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::{Disableable, Sizable};
use gpui_kit::prelude::*;
use gpui_kit::*;
use ircord_client::Client;
use ircord_client::commands::{self as cmd, Plan};
use ircord_client::contract::{
    CTCPReply, CTCPReplyCommand, ConversationKind, CtcpRequestCommand, Member, MemberDevice,
    MemberRole, MemberStatus,
};
use ircord_client::documents::NOTIFICATION_SETTINGS;
use ircord_client::store::documents::document_id;
use ircord_client::store::people::{self, Device, Lookup};
use ircord_client::store::{Changes, parse_time, timeline, topic};

use crate::composer::ComposerView;
use crate::registry::{
    self, ConversationCtx, MemberCtx, MenuItem, MessageCtx, NetworkCtx, RowFilter, Slot,
};
use crate::route::Route;
use crate::session::{NO_PANEL, Session, SessionEvent};
use crate::theme::{self, Theme};
use crate::timeline::rows::Row;
use crate::ui;
use crate::ui::icon::IconName;
use crate::ui::people_picker::{self, Picker};

/// The device state section P1 keeps (whether the member list is hidden).
const SECTION: &str = "people";

#[derive(Default)]
struct People {
    session: Option<WeakEntity<Session>>,
    composer: Option<WeakEntity<ComposerView>>,
    /// Clients whose notification settings (the ignore list) are watched.
    watched: Vec<Weak<Client>>,
}

impl Global for People {}

pub fn init(cx: &mut App) {
    cx.default_global::<People>();
    cx.observe_new::<Session>(|session: &mut Session, _, cx| {
        let weak = cx.weak_entity();
        cx.global_mut::<People>().session = Some(weak);
        if session.config.section::<Device>(SECTION).members_hidden {
            session.panel = Some(NO_PANEL.into());
        }
        if let Some(client) = session.client().cloned() {
            watch_settings(&client, cx);
        }
        cx.subscribe_self(|session: &mut Session, event: &SessionEvent, cx| {
            if let SessionEvent::Phase = event
                && let Some(client) = session.client().cloned()
            {
                watch_settings(&client, cx);
            }
        })
        .detach();
    })
    .detach();
    cx.observe_new::<ComposerView>(|_, _, cx| {
        let weak = cx.weak_entity();
        cx.global_mut::<People>().composer = Some(weak);
    })
    .detach();

    registry::register(cx, |r| {
        r.open_person = Some(Rc::new(open));
        let menu = &mut r.member_menu;
        menu.add(
            MenuItem::new("people-profile", t!("people-menu-profile"), open)
                .icon(IconName::CircleUser)
                .order(menu_order::member::PROFILE),
        );
        menu.add(
            MenuItem::new(
                "people-message",
                t!("people-menu-message"),
                |ctx, window, cx| {
                    if let Some(p) = resolve(ctx, cx) {
                        message(
                            &p.network,
                            Some(p.member.id.clone()),
                            &p.member.nick,
                            window,
                            cx,
                        );
                    }
                },
            )
            .icon(IconName::Inbox)
            .when(|ctx, cx| resolve(ctx, cx).is_some_and(|p| !p.me))
            .order(menu_order::member::MESSAGE),
        );
        menu.add(
            MenuItem::new(
                "people-mention",
                t!("people-menu-mention"),
                |ctx, window, cx| {
                    if let Some(p) = resolve(ctx, cx) {
                        mention(&p.member.nick, window, cx);
                    }
                },
            )
            .icon(IconName::Asterisk)
            .when(|ctx, cx| {
                let open = ctx.session.read(cx).conversation.clone();
                ctx.conversation.is_some()
                    && ctx.conversation == open
                    && resolve(ctx, cx).is_some_and(|p| !p.me)
            })
            .order(menu_order::member::MENTION),
        );
        menu.add(
            MenuItem::new(
                "people-copy-nick",
                t!("people-menu-copy-nick"),
                |ctx, window, cx| {
                    if let Some(p) = resolve(ctx, cx) {
                        cx.write_to_clipboard(ClipboardItem::new_string(p.member.nick.clone()));
                        ui::toast::info(window, cx, t!("people-copied-nick", nick = p.member.nick));
                    }
                },
            )
            .icon(IconName::Copy)
            .order(menu_order::member::COPY_NICK),
        );
        menu.add(
            MenuItem::new(
                "people-copy-address",
                t!("people-menu-copy-address"),
                |ctx, window, cx| {
                    if let Some(address) = resolve(ctx, cx).and_then(|p| people::address(&p.member))
                    {
                        cx.write_to_clipboard(ClipboardItem::new_string(address.clone()));
                        ui::toast::info(window, cx, t!("people-copied-address", address = address));
                    }
                },
            )
            .icon(IconName::Copy)
            .when(|ctx, cx| resolve(ctx, cx).is_some_and(|p| people::address(&p.member).is_some()))
            .order(menu_order::member::COPY_ADDRESS),
        );
        menu.add(
            MenuItem::new("people-ignore", t!("people-menu-ignore"), toggle_ignore)
                .when(|ctx, cx| resolve(ctx, cx).is_some_and(|p| !p.me && !p.ignored))
                .icon(IconName::EyeOff)
                .order(menu_order::member::IGNORE),
        );
        menu.add(
            MenuItem::new("people-unignore", t!("people-menu-unignore"), toggle_ignore)
                .when(|ctx, cx| resolve(ctx, cx).is_some_and(|p| !p.me && p.ignored))
                .icon(IconName::Bell)
                .order(menu_order::member::IGNORE),
        );
        menu.add(
            MenuItem::new(
                "people-whois",
                t!("people-menu-whois"),
                |ctx, window, cx| {
                    if let Some(p) = resolve(ctx, cx) {
                        whois_in_server_window(
                            &ctx.session,
                            &p.network,
                            &p.member.nick,
                            window,
                            cx,
                        );
                    }
                },
            )
            .icon(IconName::Info)
            .order(menu_order::member::WHOIS),
        );
        menu.add(
            MenuItem::new(
                "people-invite",
                t!("people-menu-invite"),
                |ctx, window, cx| {
                    if let Some(p) = resolve(ctx, cx) {
                        invite(&ctx.session, &p.network, &p.member, window, cx);
                    }
                },
            )
            .icon(IconName::Plus)
            .when(|ctx, cx| resolve(ctx, cx).is_some_and(|p| !p.me))
            .order(menu_order::member::INVITE),
        );
        menu.add(
            MenuItem::new(
                "people-mark-bot",
                t!("people-menu-mark-bot"),
                |ctx, window, cx| set_bot(ctx, true, window, cx),
            )
            .when(|ctx, cx| {
                resolve(ctx, cx)
                    .is_some_and(|p| p.owner && !p.me && p.member.role != MemberRole::Bot)
            })
            .icon(IconName::Bot)
            .order(menu_order::member::BOT),
        );
        menu.add(
            MenuItem::new(
                "people-unmark-bot",
                t!("people-menu-unmark-bot"),
                |ctx, window, cx| set_bot(ctx, false, window, cx),
            )
            .when(|ctx, cx| {
                resolve(ctx, cx)
                    .is_some_and(|p| p.owner && !p.me && p.member.role == MemberRole::Bot)
            })
            .icon(IconName::Bot)
            .order(menu_order::member::BOT),
        );

        r.network_menu.add(
            MenuItem::new(
                "people-new-direct",
                t!("people-menu-new-direct"),
                |ctx: &NetworkCtx, window, cx| new_direct(&ctx.session, &ctx.network, window, cx),
            )
            .icon(IconName::Plus)
            .order(menu_order::network::NEW_DIRECT),
        );

        // The web's people entries in the other menus: inviting someone to a
        // channel, the other person's profile in a direct conversation, and
        // the author of a message.
        r.conversation_menu.add(
            MenuItem::new(
                "people-invite-someone",
                t!("people-menu-invite-someone"),
                |ctx: &ConversationCtx, window, cx| invite_someone(ctx, window, cx),
            )
            .icon(IconName::Plus)
            .order(menu_order::conversation::INVITE)
            .when(|ctx: &ConversationCtx, cx: &App| invitable(ctx, cx).is_some()),
        );
        r.conversation_menu.add(
            MenuItem::new(
                "people-direct-profile",
                t!("people-menu-profile"),
                |ctx: &ConversationCtx, window, cx| {
                    if let Some(card) = partner(ctx, cx) {
                        open(&card, window, cx);
                    }
                },
            )
            .icon(IconName::CircleUser)
            .order(menu_order::conversation::PROFILE)
            .when(|ctx: &ConversationCtx, cx: &App| partner(ctx, cx).is_some()),
        );
        r.message_menu.add(
            MenuItem::new(
                "people-mention-author",
                t!("people-menu-mention"),
                |ctx: &MessageCtx, window, cx| {
                    if let Some(p) = author(ctx, cx).and_then(|card| resolve(&card, cx)) {
                        mention(&p.member.nick, window, cx);
                    }
                },
            )
            .icon(IconName::Asterisk)
            .order(menu_order::message::MENTION)
            .label_with(|ctx: &MessageCtx, cx: &App| {
                author(ctx, cx)
                    .and_then(|card| resolve(&card, cx))
                    .map(|p| t!("people-menu-mention-person", nick = p.member.nick).into())
                    .unwrap_or_else(|| t!("people-menu-mention").into())
            })
            .when(|ctx: &MessageCtx, cx: &App| {
                let open = ctx.session.read(cx).conversation.clone();
                open.as_deref() == Some(ctx.conversation.as_str())
                    && author(ctx, cx)
                        .and_then(|card| resolve(&card, cx))
                        .is_some_and(|p| !p.me)
            }),
        );
        r.message_menu.add(
            MenuItem::new(
                "people-author-profile",
                t!("people-menu-profile"),
                |ctx: &MessageCtx, window, cx| {
                    if let Some(card) = author(ctx, cx) {
                        open(&card, window, cx);
                    }
                },
            )
            .icon(IconName::CircleUser)
            .order(menu_order::message::PROFILE)
            .when(|ctx: &MessageCtx, cx: &App| {
                author(ctx, cx).is_some_and(|card| resolve(&card, cx).is_some())
            }),
        );

        r.row_filters.add(RowFilter {
            id: "people-ignored".into(),
            order: 0,
            keep: Rc::new(|row, w, _| match row {
                Row::Message(m) => match (people::ignore_list(w), w.members.get(&m.sender_id)) {
                    (Some(list), Some(member)) if !m.own => !people::is_ignored(&list, member),
                    _ => true,
                },
                _ => true,
            }),
        });

        r.timeline_start
            .add(Slot::new("people-direct-intro", direct_intro));
    });
}

/// Save whether the member list is hidden, for the next start.
pub fn remember_members_hidden(hidden: bool, cx: &mut App) {
    let Some(session) = session(cx) else { return };
    let config = &session.read(cx).config;
    if let Err(e) = config.save_section(
        SECTION,
        &Device {
            members_hidden: hidden,
        },
    ) {
        log::warn!("save the member list: {e}");
    }
}

fn session(cx: &App) -> Option<Entity<Session>> {
    cx.try_global::<People>()?.session.as_ref()?.upgrade()
}

/// Read the notification settings (the ignore list) once per client.
pub fn watch_settings(client: &Rc<Client>, cx: &mut App) {
    let people = cx.default_global::<People>();
    people.watched.retain(|w| w.strong_count() > 0);
    if people
        .watched
        .iter()
        .any(|w| w.upgrade().is_some_and(|w| Rc::ptr_eq(&w, client)))
    {
        return;
    }
    people.watched.push(Rc::downgrade(client));
    client.watch_document(&NOTIFICATION_SETTINGS);
    // A changed ignore list redraws the timeline (R1's reading topic does).
    let settings = document_id(NOTIFICATION_SETTINGS.document);
    client.watch(move |handle, changes| {
        if !changes.reset && changes.touches_id(topic::DOCUMENTS, settings) {
            handle.change(|_| Changes::topic(timeline::TOPIC));
        }
    });
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

// ---- Who a menu or card is about ----

/// A person as a menu or card sees them.
struct Resolved {
    client: Rc<Client>,
    network: String,
    member: Member,
    /// It is you.
    me: bool,
    ignored: bool,
    /// You may mark bots on this network.
    owner: bool,
}

fn network_of(ctx: &MemberCtx, cx: &App) -> Option<String> {
    let session = ctx.session.read(cx);
    let client = session.client()?;
    let w = client.read();
    ctx.conversation
        .as_ref()
        .and_then(|id| w.conversation(id))
        .map(|c| c.network_id.clone())
        .or_else(|| session.network.clone())
}

fn resolve(ctx: &MemberCtx, cx: &App) -> Option<Resolved> {
    let network = network_of(ctx, cx)?;
    let client = ctx.session.read(cx).client()?.clone();
    let w = client.read();
    let member = people::person(&w, &ctx.member, &network)?.clone();
    let me = w.is_self(&ctx.member, &network);
    let ignored = people::ignore_list(&w).is_some_and(|l| people::is_ignored(&l, &member));
    let owner = people::can_mark_bots(&w, &network);
    drop(w);
    Some(Resolved {
        client,
        network,
        member,
        me,
        ignored,
        owner,
    })
}

// ---- Menu actions ----

/// Open a direct conversation with someone and show it.
pub fn message(
    network: &str,
    member: Option<String>,
    nick: &str,
    window: &mut Window,
    cx: &mut App,
) {
    let Some(session) = session(cx) else { return };
    let Some(client) = session.read(cx).client().cloned() else {
        return;
    };
    let task = cmd::execute(
        &client,
        Plan::Direct {
            network: network.to_string(),
            member,
            nickname: Some(nick.to_string()),
            text: None,
        },
    );
    window
        .spawn(cx, async move |acx| {
            let result = task
                .await
                .map_err(|e| e.to_string())
                .and_then(|r| r.map_err(|e| e.to_string()));
            let _ = acx.update(|window, cx| match result {
                Ok(done) => {
                    if let Some(id) = done.open {
                        ui::dialog::close(window, cx);
                        session.update(cx, |s, cx| s.select_conversation(&id, cx));
                    }
                }
                Err(e) => ui::toast::error(window, cx, e),
            });
        })
        .detach();
}

/// Pick someone on `network` and open a direct conversation with them.
pub fn new_direct(session: &Entity<Session>, network: &str, window: &mut Window, cx: &mut App) {
    let network_id = network.to_string();
    people_picker::open(
        window,
        cx,
        Picker {
            title: t!("people-new-direct-title").into(),
            session: session.clone(),
            network: network.to_string(),
            purpose: ircord_client::api::people::Purpose::Dm,
            conversation: None,
            except: Vec::new(),
        },
        move |picked, window, cx| {
            message(&network_id, picked.member.clone(), &picked.nick, window, cx)
        },
    );
}

/// Put `@nick ` into the box and give it the keyboard.
fn mention(nick: &str, window: &mut Window, cx: &mut App) {
    let Some(composer) = cx
        .try_global::<People>()
        .and_then(|p| p.composer.as_ref()?.upgrade())
    else {
        return;
    };
    composer.update(cx, |c, cx| {
        let text = c.text(cx);
        let gap = if text.is_empty() || text.ends_with(' ') {
            ""
        } else {
            " "
        };
        let next = format!("{text}{gap}@{nick} ");
        c.set_text_at(&next, next.len(), window, cx);
    });
    let input = composer.read(cx).input.clone();
    input.update(cx, |state, cx| state.focus(window, cx));
}

fn toggle_ignore(ctx: &MemberCtx, window: &mut Window, cx: &mut App) {
    let Some(p) = resolve(ctx, cx) else { return };
    let (list, me) = {
        let w = p.client.read();
        (
            people::ignore_list(&w),
            people::me(&w, Some(&p.network)).clone(),
        )
    };
    let Some(list) = list else {
        watch_settings(&p.client, cx);
        ui::toast::info(window, cx, t!("people-settings-loading"));
        return;
    };
    let outcome = people::toggle_ignore(&list, &p.member, &me);
    match outcome.ignore {
        Some(next_list) => {
            let mut next = p
                .client
                .read()
                .documents
                .notification_settings
                .value()
                .cloned()
                .unwrap_or_default();
            next.ignore = Some(next_list);
            p.client.edit_document(&NOTIFICATION_SETTINGS, next);
            let text = if p.ignored {
                t!("people-showing-again", nick = p.member.nick)
            } else {
                t!("people-ignoring", nick = p.member.nick)
            };
            ui::toast::info(window, cx, text);
        }
        None => {
            let text = match outcome.notice {
                ircord_client::commands::ignore::Notice::Covered { masks, .. } => t!(
                    "people-ignore-covered",
                    nick = p.member.nick,
                    masks = masks.join(", ")
                ),
                ircord_client::commands::ignore::Notice::Maximum => {
                    t!("people-ignore-maximum").to_string()
                }
                _ => t!("people-ignore-failed", nick = p.member.nick),
            };
            ui::toast::info(window, cx, text);
        }
    }
}

/// Send WHOIS and show the server window, where the answer appears.
fn whois_in_server_window(
    session: &Entity<Session>,
    network: &str,
    nick: &str,
    window: &mut Window,
    cx: &mut App,
) {
    raw(session, network, format!("WHOIS {nick}"), window, cx);
    session.update(cx, |s, cx| {
        s.navigate(Route::ServerWindow(network.to_string()), cx)
    });
}

/// Send one IRC line on `network`; say why when it cannot go.
fn raw(session: &Entity<Session>, network: &str, line: String, window: &mut Window, cx: &mut App) {
    let Some(client) = session.read(cx).client().cloned() else {
        return;
    };
    let task = cmd::execute(
        &client,
        Plan::Raw {
            network: network.to_string(),
            line,
            then: None,
        },
    );
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
}

/// Invite them to one of your channels on the network they are not in.
/// The other person in a direct conversation, as a member menu would see
/// them.
fn partner(ctx: &ConversationCtx, cx: &App) -> Option<MemberCtx> {
    let client = ctx.session.read(cx).client()?.clone();
    let w = client.read();
    let member = people::direct_partner(&w, &ctx.conversation)?;
    Some(MemberCtx {
        session: ctx.session.clone(),
        member: member.id.clone(),
        conversation: Some(ctx.conversation.clone()),
    })
}

/// Who wrote a message, as a member menu would see them.
fn author(ctx: &MessageCtx, cx: &App) -> Option<MemberCtx> {
    let client = ctx.session.read(cx).client()?.clone();
    let w = client.read();
    let message = w.message(&ctx.conversation, &ctx.message)?;
    Some(MemberCtx {
        session: ctx.session.clone(),
        member: message.sender_id.clone(),
        conversation: Some(ctx.conversation.clone()),
    })
}

/// A channel you are in, as its network and its name for INVITE.
fn invitable(ctx: &ConversationCtx, cx: &App) -> Option<(String, String)> {
    let client = ctx.session.read(cx).client()?.clone();
    let w = client.read();
    let c = w.conversation(&ctx.conversation)?;
    (c.kind == ConversationKind::Channel && c.party.is_none() && c.joined)
        .then(|| (c.network_id.clone(), ircord_client::irc::channel_target(c)))
}

/// Pick someone and invite them to the channel.
fn invite_someone(ctx: &ConversationCtx, window: &mut Window, cx: &mut App) {
    let Some((network, channel)) = invitable(ctx, cx) else {
        return;
    };
    let session = ctx.session.clone();
    people_picker::open(
        window,
        cx,
        Picker {
            title: t!("people-invite-someone-title", channel = channel).into(),
            session: session.clone(),
            network: network.clone(),
            purpose: ircord_client::api::people::Purpose::Dm,
            conversation: None,
            except: Vec::new(),
        },
        move |picked, window, cx| {
            raw(
                &session,
                &network,
                format!("INVITE {} {channel}", picked.nick),
                window,
                cx,
            );
            ui::toast::info(
                window,
                cx,
                t!("people-invited", nick = picked.nick, channel = channel),
            );
        },
    );
}

fn invite(
    session: &Entity<Session>,
    network: &str,
    member: &Member,
    window: &mut Window,
    cx: &mut App,
) {
    let Some(client) = session.read(cx).client().cloned() else {
        return;
    };
    let channels: Vec<String> = {
        let w = client.read();
        let mut list: Vec<String> = w
            .conversations
            .iter()
            .filter(|c| {
                c.network_id == network
                    && c.kind == ConversationKind::Channel
                    && c.party.is_none()
                    && c.joined
                    && !c.member_ids.contains(&member.id)
            })
            .map(|c| c.name.clone())
            .collect();
        list.sort_by_key(|n| n.to_lowercase());
        list
    };
    if channels.is_empty() {
        ui::toast::info(window, cx, t!("people-invite-none", nick = member.nick));
        return;
    }
    let (session, network, nick) = (session.clone(), network.to_string(), member.nick.clone());
    ui::dialog::open(
        window,
        cx,
        t!("people-invite-title", nick = member.nick),
        move |dialog, _, cx| {
            let theme = theme::get(cx).clone();
            let mut list = div()
                .id("people-invite-channels")
                .test_support()
                .role(Role::List)
                .aria_label(t!("people-invite-channels"))
                .flex()
                .flex_col()
                .gap_1();
            for (i, name) in channels.iter().enumerate() {
                let channel = if name.starts_with(['#', '&']) {
                    name.clone()
                } else {
                    format!("#{name}")
                };
                let (session, network, nick) = (session.clone(), network.clone(), nick.clone());
                let label = t!("people-invite-to", channel = channel);
                list = list.child(
                    div()
                        .id(ElementId::Name(format!("people-invite-{i}").into()))
                        .test_support()
                        .role(Role::Button)
                        .reachable(&theme)
                        .aria_label(SharedString::from(label.clone()))
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .cursor_pointer()
                        .hover(|d| d.bg(theme.accent.opacity(0.15)))
                        .child(label)
                        .on_click(move |_, window, cx| {
                            ui::dialog::close(window, cx);
                            raw(
                                &session,
                                &network,
                                format!("INVITE {nick} {channel}"),
                                window,
                                cx,
                            );
                            ui::toast::info(
                                window,
                                cx,
                                t!("people-invited", nick = nick, channel = channel),
                            );
                        }),
                );
            }
            dialog.w(px(360.)).child(list)
        },
    );
}

fn set_bot(ctx: &MemberCtx, bot: bool, window: &mut Window, cx: &mut App) {
    let Some(p) = resolve(ctx, cx) else { return };
    let (network, member, nick) = (
        p.network.clone(),
        p.member.id.clone(),
        p.member.nick.clone(),
    );
    let task = p
        .client
        .handle()
        .request(move |api| async move { api.set_member_bot(&network, &member, bot).await });
    let client = p.client.clone();
    window
        .spawn(cx, async move |acx| {
            let result = task
                .await
                .map_err(|e| e.to_string())
                .and_then(|r| r.map_err(|e| e.to_string()));
            let _ = acx.update(|window, cx| match result {
                Ok(changed) => {
                    client.change(|w| people::put_member(w, changed));
                    let text = if bot {
                        t!("people-marked-bot", nick = nick)
                    } else {
                        t!("people-unmarked-bot", nick = nick)
                    };
                    ui::toast::info(window, cx, text);
                }
                Err(e) => ui::toast::error(window, cx, e),
            });
        })
        .detach();
}

// ---- Lookups ----

fn whois(client: &Rc<Client>, network: &str, nick: &str) {
    let now = now_ms();
    let mut ask = false;
    client.change(|w| {
        ask = people::begin_whois(w, network, nick, now);
        if ask {
            Changes::topic_id(people::TOPIC, people::key(network, nick))
        } else {
            Changes::default()
        }
    });
    if !ask {
        return;
    }
    let (n, k) = (network.to_string(), nick.to_string());
    let (n2, k2) = (n.clone(), k.clone());
    client.call(
        move |api| async move { api.whois(&n, &k).await },
        move |w, result| people::whois_done(w, &n2, &k2, result, now_ms()),
    );
}

fn ask_client(client: &Rc<Client>, network: &str, nick: &str) {
    let mut ask = false;
    client.change(|w| {
        ask = people::begin_ctcp(w, network, nick);
        if ask {
            Changes::topic_id(people::TOPIC, people::key(network, nick))
        } else {
            Changes::default()
        }
    });
    if !ask {
        return;
    }
    let (n, k) = (network.to_string(), nick.to_string());
    let (n2, k2) = (n.clone(), k.clone());
    client.call(
        move |api| async move {
            let asks = [
                CtcpRequestCommand::VERSION,
                CtcpRequestCommand::TIME,
                CtcpRequestCommand::PING,
            ]
            .map(|command| {
                let (api, n, k) = (api.clone(), n.clone(), k.clone());
                async move { api.ctcp(&n, &k, command).await }
            });
            futures::future::try_join_all(asks).await
        },
        move |w, result| people::ctcp_done(w, &n2, &k2, result),
    );
}

// ---- The card ----

/// Open the profile card for a person.
pub fn open(ctx: &MemberCtx, window: &mut Window, cx: &mut App) {
    let Some(network) = network_of(ctx, cx) else {
        return;
    };
    let (session, member) = (ctx.session.clone(), ctx.member.clone());
    let card = cx.new(|cx| ProfileCard::new(session, member, network, cx));
    let title = card.read(cx).title(cx);
    ui::dialog::open(window, cx, title, move |dialog, _, _| {
        dialog.w(px(440.)).child(card.clone())
    });
}

pub struct ProfileCard {
    session: Entity<Session>,
    member: String,
    network: String,
    _subscription: Subscription,
}

impl ProfileCard {
    fn new(
        session: Entity<Session>,
        member: String,
        network: String,
        cx: &mut Context<Self>,
    ) -> Self {
        let subscription = cx.subscribe(&session, |_, _, event: &SessionEvent, cx| {
            if let SessionEvent::Changed(c) = event
                && (c.members || c.touches(people::TOPIC))
            {
                cx.notify();
            }
        });
        let this = Self {
            session,
            member,
            network,
            _subscription: subscription,
        };
        if let Some(client) = this.session.read(cx).client().cloned() {
            let nick = people::person(&client.read(), &this.member, &this.network)
                .map(|m| m.nick.clone())
                .filter(|n| !n.is_empty());
            if let Some(nick) = nick {
                whois(&client, &this.network, &nick);
            }
        }
        this
    }

    fn title(&self, cx: &App) -> SharedString {
        self.session
            .read(cx)
            .client()
            .and_then(|c| {
                people::person(&c.read(), &self.member, &self.network)
                    .map(|m| SharedString::from(people::name(m).to_string()))
            })
            .unwrap_or_else(|| t!("people-profile-title").into())
    }
}

/// "3 minutes ago", for a moment `ms` milliseconds back.
fn ago(ms: i64) -> String {
    let minutes = ms.max(0) / 60_000;
    match minutes {
        0 => t!("people-ago-now").to_string(),
        1..=59 => t!("people-ago-minutes", count = minutes),
        60..=1439 => t!("people-ago-hours", count = minutes / 60),
        _ => t!("people-ago-days", count = minutes / 1440),
    }
}

fn section_label(text: &'static str, theme: &Theme) -> Div {
    div()
        .text_xs()
        .font_weight(FontWeight::BOLD)
        .text_color(theme.faint)
        .child(text.to_uppercase())
}

/// Label left, value right.
fn info_rows(
    id: &'static str,
    label: &'static str,
    rows: Vec<(String, String)>,
    theme: &Theme,
) -> AnyElement {
    let mut list = div()
        .id(id)
        .test_support()
        .role(Role::List)
        .aria_label(label)
        .flex()
        .flex_col();
    for (i, (key, value)) in rows.into_iter().enumerate() {
        list = list.child(
            div()
                .id(ElementId::Name(format!("{id}-{i}").into()))
                .test_support()
                .role(Role::ListItem)
                .aria_label(SharedString::from(format!("{key}: {value}")))
                .flex()
                .justify_between()
                .gap_4()
                .py_1()
                .border_b_1()
                .border_color(theme.rail)
                .child(
                    div()
                        .flex_none()
                        .text_xs()
                        .text_color(theme.muted)
                        .child(key),
                )
                .child(div().min_w_0().text_sm().child(value)),
        );
    }
    list.into_any_element()
}

/// Facts as tiles side by side, the value over its label, as the web's card.
fn fact_tiles(
    id: &'static str,
    label: &'static str,
    facts: Vec<(String, String)>,
    theme: &Theme,
) -> AnyElement {
    let mut tiles = div()
        .id(id)
        .test_support()
        .role(Role::List)
        .aria_label(label)
        .flex_none()
        .flex()
        .flex_wrap()
        .gap_px()
        .rounded(theme.radius_large)
        .overflow_hidden()
        .bg(theme.border);
    for (i, (key, value)) in facts.into_iter().enumerate() {
        tiles = tiles.child(
            div()
                .id(ElementId::Name(format!("{id}-{i}").into()))
                .test_support()
                .role(Role::ListItem)
                .aria_label(SharedString::from(format!("{key}: {value}")))
                .flex_1()
                .min_w(px(110.))
                .flex()
                .flex_col()
                .gap_0p5()
                .px_3()
                .py_2p5()
                .bg(theme.sidebar)
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.text)
                        .child(value),
                )
                .child(div().text_xs().text_color(theme.muted).child(key)),
        );
    }
    tiles.into_any_element()
}

fn badge(id: String, text: SharedString, theme: &Theme) -> AnyElement {
    div()
        .id(ElementId::Name(id.into()))
        .test_support()
        .aria_label(text.clone())
        .px_2()
        .rounded_full()
        .bg(theme.selected)
        .text_color(theme.accent)
        .text_xs()
        .font_weight(FontWeight::BOLD)
        .child(text)
        .into_any_element()
}

/// Their picture, or the initial of their name, with their presence. The
/// picture loads only from this server (`people::picture_url`, the web's
/// #278 rule): an address elsewhere would learn who looks at the card.
pub fn picture(m: &Member, session: &Session, size: Pixels, theme: &Theme) -> AnyElement {
    let server = session.server().map(|s| s.url().clone());
    ui::avatar::person(
        ui::avatar::Person {
            member: m,
            name: people::name(m),
            server: server.as_ref(),
            presence: true,
            key: &format!("person-{}", m.id),
        },
        size,
        theme,
    )
    .into_any_element()
}

fn presence(m: &Member) -> &'static str {
    match m.status {
        MemberStatus::Online => t!("people-online"),
        MemberStatus::Away => t!("people-away"),
        _ => t!("people-offline"),
    }
}

impl Render for ProfileCard {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme::get(cx).clone();
        let session = self.session.read(cx);
        let Some(client) = session.client().cloned() else {
            return div().into_any_element();
        };
        let w = client.read();
        let Some(m) = people::person(&w, &self.member, &self.network).cloned() else {
            return ui::empty::empty(
                "profile-missing",
                t!("people-profile-missing"),
                None,
                &theme,
            )
            .into_any_element();
        };
        let me = w.is_self(&self.member, &self.network);
        let now = now_ms();
        let network_name = w.network(&self.network).map(|n| n.name.clone());
        let owner = people::can_mark_bots(&w, &self.network);
        let whois = w.people.whois(&self.network, &m.nick).cloned();
        let ctcp = w.people.ctcp(&self.network, &m.nick).cloned();
        let channels: Vec<(String, String)> =
            people::shared_channels(&w, &self.member, &self.network)
                .into_iter()
                .map(|c| (c.id.clone(), c.name.clone()))
                .collect();
        drop(w);
        let found = whois.as_ref().and_then(|l| l.done()).filter(|d| d.found);

        let mut card = div()
            .id("profile-card")
            .test_support()
            .role(Role::Group)
            .aria_label(t!("people-profile-title"))
            .flex()
            .flex_col()
            .gap_3()
            .max_h(px(500.))
            .overflow_y_scroll();

        // Who they are.
        let mut badges: Vec<AnyElement> = Vec::new();
        if m.role == MemberRole::Operator {
            badges.push(badge(
                "profile-badge-operator".into(),
                t!("people-badge-operator").into(),
                &theme,
            ));
        }
        if found.is_some_and(|d| d.operator) {
            badges.push(badge(
                "profile-badge-irc-operator".into(),
                t!("people-badge-irc-operator").into(),
                &theme,
            ));
        }
        if m.role == MemberRole::Bot {
            badges.push(badge(
                "profile-badge-bot".into(),
                t!("people-bot").into(),
                &theme,
            ));
        }
        match m.device {
            Some(MemberDevice::Phone) => badges.push(badge(
                "profile-badge-device".into(),
                t!("people-on-phone").into(),
                &theme,
            )),
            Some(MemberDevice::Tablet) => badges.push(badge(
                "profile-badge-device".into(),
                t!("people-on-tablet").into(),
                &theme,
            )),
            _ => {}
        }
        // A banner, the picture over its edge, the name in their colour.
        let colored = crate::message::layout::settings(cx).colored_names;
        let name_color = ui::avatar::name_color(&m, colored, &theme)
            .map(theme::hex)
            .unwrap_or(theme.text);
        let server = self.session.read(cx).server().map(|s| s.url().clone());
        let banner_url = server
            .as_ref()
            .and_then(|s| people::picture_url(m.banner_url.as_deref(), s));
        let accent = m
            .profile_accent_color
            .as_deref()
            .and_then(|c| u32::from_str_radix(c.strip_prefix('#')?, 16).ok())
            .map(theme::hex)
            .unwrap_or(theme.accent);
        let banner = div()
            .id("profile-banner")
            .test_support()
            .h(px(96.))
            .w_full()
            .flex_none()
            .overflow_hidden()
            .rounded(theme.radius_large)
            .bg(accent.opacity(0.3))
            .children(banner_url.map(|url| {
                img(crate::images::still(url.to_string().into()))
                    .size_full()
                    .object_fit(ObjectFit::Cover)
            }));
        let face = div()
            .self_start()
            .mt(px(-56.))
            .ml_3()
            .p_1()
            .flex_none()
            .rounded_full()
            .bg(theme.main)
            .child(ui::avatar::person(
                ui::avatar::Person {
                    member: &m,
                    name: people::name(&m),
                    server: server.as_ref(),
                    presence: true,
                    key: "profile",
                },
                px(ui::avatar::LARGE),
                &theme,
            ));
        let dot = match m.status {
            MemberStatus::Online => theme.online,
            MemberStatus::Away => theme.away,
            _ => theme.offline,
        };
        let mut handle = div()
            .flex()
            .items_center()
            .gap_1p5()
            .text_sm()
            .text_color(theme.muted)
            .child(format!("@{}", m.nick))
            .child(div().size(px(7.)).rounded_full().bg(dot))
            .child(presence(&m));
        if let Some(p) = m.pronouns.as_deref().filter(|p| !p.is_empty()) {
            handle = handle.child(format!("· {p}"));
        }
        let mut names = div()
            .flex()
            .flex_col()
            .gap_0p5()
            .min_w_0()
            .child(
                div()
                    .id("profile-name")
                    .test_support()
                    .aria_label(SharedString::from(people::name(&m).to_string()))
                    .text_2xl()
                    .font_weight(FontWeight::BOLD)
                    .text_color(name_color)
                    .child(people::name(&m).to_string()),
            )
            .child(handle);
        if !badges.is_empty() {
            names = names.child(div().flex().flex_wrap().gap_1().pt_1().children(badges));
        }
        card = card.child(
            div()
                .flex_none()
                .flex()
                .flex_col()
                .gap_2()
                .child(banner)
                .child(face)
                .child(names),
        );
        if let Some(headline) = m.headline.clone().filter(|h| !h.is_empty()) {
            card = card.child(div().text_sm().child(headline));
        }
        if m.status == MemberStatus::Away
            && let Some(away) = m.away_message.clone().filter(|a| !a.is_empty())
        {
            card = card.child(
                div()
                    .id("profile-away")
                    .test_support()
                    .aria_label(SharedString::from(t!(
                        "people-profile-away",
                        message = away
                    )))
                    .text_sm()
                    .text_color(theme.away)
                    .child(t!("people-profile-away", message = away)),
            );
        }
        if let Some(status) = people::status_label(&m, now) {
            card = card.child(
                div()
                    .id("profile-status")
                    .test_support()
                    .aria_label(SharedString::from(status.clone()))
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .bg(theme.accent.opacity(0.15))
                    .text_sm()
                    .child(status),
            );
        }
        if !m.bio.is_empty() {
            card = card.child(
                div()
                    .id("profile-bio")
                    .test_support()
                    .aria_label(SharedString::from(m.bio.clone()))
                    .pl_3()
                    .border_l_2()
                    .border_color(accent)
                    .text_sm()
                    .child(m.bio.clone()),
            );
        }
        if let Some(home) = m.homepage_url.clone().filter(|u| u.starts_with("https://")) {
            let shown = home
                .trim_start_matches("https://")
                .trim_end_matches('/')
                .to_string();
            card = card.child(
                div()
                    .id("profile-homepage")
                    .test_support()
                    .role(Role::Link)
                    .reachable(&theme)
                    .aria_label(SharedString::from(shown.clone()))
                    .text_sm()
                    .text_color(theme.link)
                    .cursor_pointer()
                    .child(shown)
                    .on_click(move |_, _, cx| cx.open_url(&home)),
            );
        }
        if let Some(interests) = m.interests.clone().filter(|i| !i.is_empty()) {
            card =
                card.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(section_label(t!("people-profile-interests"), &theme))
                        .child(div().flex().flex_wrap().gap_1().children(
                            interests.into_iter().map(|i| {
                                div()
                                    .px_2()
                                    .rounded_full()
                                    .bg(theme.accent.opacity(0.15))
                                    .text_xs()
                                    .child(i)
                            }),
                        )),
                );
        }

        // Channels.
        if !channels.is_empty() {
            let label = if me {
                t!("people-profile-your-channels")
            } else {
                t!("people-profile-shared-channels")
            };
            let mut list = div()
                .id("profile-channels")
                .test_support()
                .role(Role::List)
                .aria_label(label)
                .flex()
                .flex_wrap()
                .gap_1();
            for (id, name) in channels {
                let session = self.session.clone();
                let text = format!("#{}", name.trim_start_matches('#'));
                let hover = theme.hover;
                list = list.child(
                    div()
                        .id(ElementId::Name(format!("profile-channel-{id}").into()))
                        .test_support()
                        .role(Role::Link)
                        .reachable(&theme)
                        .aria_label(SharedString::from(text.clone()))
                        .flex()
                        .items_center()
                        .gap_1()
                        .px_2p5()
                        .py_0p5()
                        .rounded_full()
                        .border_1()
                        .border_color(theme.border)
                        .text_sm()
                        .text_color(theme.text)
                        .cursor_pointer()
                        .hover(move |d| d.bg(hover))
                        .child(div().text_color(theme.faint).child("#"))
                        .child(name.trim_start_matches('#').to_string())
                        .on_click(move |_, window, cx| {
                            ui::dialog::close(window, cx);
                            session.update(cx, |s, cx| s.select_conversation(&id, cx));
                        }),
                );
            }
            card = card.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(section_label(label, &theme))
                    .child(list),
            );
        }

        // Profile facts.
        let mut details = Vec::new();
        if let Some(name) = network_name {
            details.push((t!("people-profile-network").to_string(), name));
        }
        if !m.account.is_empty() {
            details.push((t!("people-profile-account").to_string(), m.account.clone()));
        }
        if let Some(real) = m
            .realname
            .clone()
            .filter(|r| !r.is_empty() && *r != m.display_name && r != "IRCord")
        {
            details.push((t!("people-profile-real-name").to_string(), real));
        }
        if let Some(location) = m.location.clone().filter(|l| !l.is_empty()) {
            details.push((t!("people-profile-location").to_string(), location));
        }
        if let Some(langs) = m.languages.clone().filter(|l| !l.is_empty()) {
            details.push((t!("people-profile-languages").to_string(), langs.join(", ")));
        }
        if let Some(tz) = m.timezone.clone().filter(|t| !t.is_empty()) {
            details.push((t!("people-profile-timezone").to_string(), tz));
        }
        if !details.is_empty() {
            card = card.child(fact_tiles(
                "profile-details",
                t!("people-profile-details"),
                details,
                &theme,
            ));
        }

        // What the server says.
        let server = match &whois {
            Some(Lookup::Loading) => Some(
                div()
                    .id("profile-whois-loading")
                    .test_support()
                    .role(Role::Status)
                    .aria_label(t!("people-whois-looking"))
                    .text_sm()
                    .text_color(theme.muted)
                    .child(t!("people-whois-looking"))
                    .into_any_element(),
            ),
            Some(Lookup::Failed(e)) => Some(
                div()
                    .text_sm()
                    .text_color(theme.muted)
                    .child(t!("people-whois-failed", error = e))
                    .into_any_element(),
            ),
            Some(Lookup::Done(d)) if !d.found => Some(
                div()
                    .id("profile-whois-absent")
                    .test_support()
                    .aria_label(t!("people-whois-absent"))
                    .text_sm()
                    .text_color(theme.muted)
                    .child(t!("people-whois-absent"))
                    .into_any_element(),
            ),
            Some(Lookup::Done(d)) => {
                let mut rows = Vec::new();
                let user = d.username.clone().unwrap_or_default();
                let host = d.host.clone().unwrap_or_default();
                if !user.is_empty() || !host.is_empty() {
                    rows.push((
                        t!("people-whois-address").to_string(),
                        format!("{user}@{host}"),
                    ));
                }
                if let Some(s) = d.server.clone().filter(|s| !s.is_empty()) {
                    rows.push((t!("people-whois-server").to_string(), s));
                }
                if let Some(on) = d.signed_on_at.as_deref().map(parse_time).filter(|t| *t > 0) {
                    rows.push((t!("people-whois-connected").to_string(), ago(now - on)));
                }
                if let Some(idle) = d.idle_seconds {
                    rows.push((t!("people-whois-last-active").to_string(), ago(idle * 1000)));
                }
                rows.push((
                    t!("people-whois-connection").to_string(),
                    if d.secure {
                        t!("people-whois-secure").to_string()
                    } else {
                        t!("people-whois-not-secure").to_string()
                    },
                ));
                Some(info_rows(
                    "profile-whois",
                    t!("people-whois-title"),
                    rows,
                    &theme,
                ))
            }
            None => None,
        };
        if let Some(server) = server {
            card = card.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(section_label(t!("people-whois-title"), &theme))
                    .child(server),
            );
        }

        // What their client says, when asked.
        if !me {
            let asking = matches!(ctcp, Some(Lookup::Loading));
            let (client2, network, nick) = (client.clone(), self.network.clone(), m.nick.clone());
            let ask = Button::new("profile-ask-client")
                .outline()
                .small()
                .icon(IconName::RefreshCw)
                .label(if asking {
                    t!("people-ctcp-asking")
                } else if ctcp.is_some() {
                    t!("people-ctcp-ask-again")
                } else {
                    t!("people-ctcp-ask")
                })
                .disabled(asking)
                .on_click(move |_, _, _| ask_client(&client2, &network, &nick));
            let body = match &ctcp {
                Some(Lookup::Done(replies)) => info_rows(
                    "profile-ctcp",
                    t!("people-ctcp-title"),
                    ctcp_rows(replies),
                    &theme,
                ),
                Some(Lookup::Failed(e)) => div()
                    .text_sm()
                    .text_color(theme.danger)
                    .child(e.clone())
                    .into_any_element(),
                _ => div()
                    .text_xs()
                    .text_color(theme.muted)
                    .child(t!("people-ctcp-help"))
                    .into_any_element(),
            };
            card = card.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(section_label(t!("people-ctcp-title"), &theme))
                            .child(ask),
                    )
                    .child(body),
            );
        }

        // What you can do.
        let mut actions = div().flex().gap_2().justify_end();
        if me {
            let session = self.session.clone();
            actions = actions.child(
                Button::new("profile-edit")
                    .ghost()
                    .label(t!("people-profile-edit"))
                    .on_click(move |_, window, cx| {
                        ui::dialog::close(window, cx);
                        session.update(cx, |s, cx| s.navigate(Route::Settings, cx));
                    }),
            );
        } else {
            if owner {
                // Any conversation on the card's network names that network.
                let conversation = client
                    .read()
                    .conversations
                    .iter()
                    .find(|c| c.network_id == self.network)
                    .map(|c| c.id.clone());
                let ctx = MemberCtx {
                    session: self.session.clone(),
                    member: self.member.clone(),
                    conversation,
                };
                let bot = m.role != MemberRole::Bot;
                actions = actions.child(
                    Button::new("profile-bot")
                        .ghost()
                        .label(if bot {
                            t!("people-menu-mark-bot")
                        } else {
                            t!("people-menu-unmark-bot")
                        })
                        .on_click(move |_, window, cx| set_bot(&ctx, bot, window, cx)),
                );
            }
            let (network, member, nick) = (self.network.clone(), m.id.clone(), m.nick.clone());
            actions = actions.w_full().flex_col().child(
                Button::new("profile-message")
                    .primary()
                    .w_full()
                    .label(t!("people-menu-message"))
                    .on_click(move |_, window, cx| {
                        message(&network, Some(member.clone()), &nick, window, cx)
                    }),
            );
        }
        // The actions stay in view below the part that scrolls.
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(card)
            .child(actions)
            .into_any_element()
    }
}

fn ctcp_rows(replies: &[CTCPReply]) -> Vec<(String, String)> {
    let mut rows = Vec::new();
    for command in [
        CTCPReplyCommand::VERSION,
        CTCPReplyCommand::TIME,
        CTCPReplyCommand::PING,
    ] {
        let Some(r) = replies.iter().find(|r| r.command == command) else {
            continue;
        };
        let label = match command {
            CTCPReplyCommand::VERSION => t!("people-ctcp-client"),
            CTCPReplyCommand::TIME => t!("people-ctcp-time"),
            _ => t!("people-ctcp-ping"),
        };
        let value = if !r.answered {
            t!("people-ctcp-no-reply").to_string()
        } else if command == CTCPReplyCommand::PING {
            t!("people-ctcp-ping-ms", ms = r.latency_ms.unwrap_or(0))
        } else {
            r.reply.clone().unwrap_or_default()
        };
        rows.push((label.to_string(), value));
    }
    rows
}

// ---- An empty direct conversation ----

/// The person a direct conversation is with, at the start of its history.
fn direct_intro(ctx: &ConversationCtx, _window: &mut Window, cx: &mut App) -> Option<AnyElement> {
    let theme = theme::get(cx).clone();
    let session = ctx.session.read(cx);
    let client = session.client()?.clone();
    let w = client.read();
    let conversation = w.conversation(&ctx.conversation)?;
    let network = conversation.network_id.clone();
    let m = people::direct_partner(&w, &ctx.conversation)?.clone();
    drop(w);
    let now = now_ms();
    let open_ctx = MemberCtx {
        session: ctx.session.clone(),
        member: m.id.clone(),
        conversation: Some(ctx.conversation.clone()),
    };
    let mut column = div()
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .text_xl()
                .font_weight(FontWeight::BOLD)
                .text_color(theme.text)
                .child(people::name(&m).to_string()),
        )
        .child(div().text_sm().text_color(theme.muted).child(format!(
            "{} · {}",
            m.nick,
            presence(&m)
        )));
    if let Some(status) = people::status_label(&m, now) {
        column = column.child(div().text_sm().text_color(theme.text).child(status));
    }
    if !m.bio.is_empty() {
        column = column.child(div().text_sm().text_color(theme.text).child(m.bio.clone()));
    }
    column = column.child(
        div()
            .text_sm()
            .text_color(theme.muted)
            .child(t!("people-direct-start", name = people::name(&m))),
    );
    let _ = network;
    Some(
        div()
            .id("direct-intro")
            .test_support()
            .role(Role::Group)
            .aria_label(t!("people-direct-intro", name = people::name(&m)))
            .flex()
            .flex_col()
            .gap_3()
            .child(picture(&m, session, px(ui::avatar::LARGE), &theme))
            .child(column)
            .child(
                div().flex().child(
                    Button::new("direct-intro-profile")
                        .outline()
                        .small()
                        .icon(IconName::User)
                        .label(t!("people-menu-profile"))
                        .on_click(move |_, window, cx| open(&open_ctx, window, cx)),
                ),
            )
            .into_any_element(),
    )
}

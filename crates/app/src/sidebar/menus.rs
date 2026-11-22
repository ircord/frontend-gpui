//! The conversation and network menus' entries from package N1, and what
//! they do. Other packages add their own entries (channel settings, jump to
//! a date, edit network) to the same registry lists.
//!
//! Conversation: mark read, favourite, notification level, move up and
//! down, sort A to Z, copy the channel name, leave a channel, close a direct
//! message. Network: mark all read, connect or disconnect, copy the address,
//! remove. Changes are applied locally from the server's answer, and the
//! server's own events confirm them.

use gpui_kit::*;
use ircord_client::api::{Method, segment};
use ircord_client::contract::{
    self as c, ChannelSettingsPatchBody, ChannelSettingsPatchBodyNotifications, Conversation,
    ConversationKind, NetworkStatus, SettingsLevelsValue,
};
use ircord_client::documents;
use ircord_client::features::Context as Allowed;
use ircord_client::irc::{channel_rights_for, channel_target};
use ircord_client::store::sidebar_order::{self as order, Level, Place, Section};
use ircord_client::store::{Changes, Workspace};

use crate::menu_order;
use crate::registry::{self, ConversationCtx, MenuItem, NetworkCtx};
use crate::session::Session;
use crate::ui;
use crate::ui::confirm::Confirm;
use crate::ui::icon::IconName;

pub fn register(cx: &mut App) {
    registry::register(cx, |r| {
        conversation_menu(&mut r.conversation_menu);
        network_menu(&mut r.network_menu);
    });
}

fn conversation_of(ctx: &ConversationCtx, cx: &App) -> Option<Conversation> {
    let session = ctx.session.read(cx);
    let client = session.client()?;
    client.read().conversation(&ctx.conversation).cloned()
}

fn with_conversation(
    ctx: &ConversationCtx,
    cx: &App,
    f: impl FnOnce(&Workspace, &Conversation) -> bool,
) -> bool {
    let session = ctx.session.read(cx);
    let Some(client) = session.client() else {
        return false;
    };
    let w = client.read();
    w.conversation(&ctx.conversation).is_some_and(|c| f(&w, c))
}

type Registered = registry::List<MenuItem<ConversationCtx>>;

fn conversation_menu(m: &mut Registered) {
    m.add(
        MenuItem::new(
            "sidebar-mark-read",
            t!("sidebar-menu-mark-read"),
            |ctx: &ConversationCtx, _, cx| mark_read(&ctx.session, &ctx.conversation, cx),
        )
        .icon(IconName::Check)
        .order(menu_order::conversation::MARK_READ)
        .when(|ctx: &ConversationCtx, cx: &App| {
            with_conversation(ctx, cx, |_, c| order::has_unread(c))
        }),
    );
    m.add(
        MenuItem::new(
            "sidebar-favourite",
            t!("sidebar-menu-favourite"),
            |ctx: &ConversationCtx, _, cx| set_favourite(&ctx.session, &ctx.conversation, true, cx),
        )
        .icon(IconName::Star)
        .order(menu_order::conversation::FAVOURITE)
        .when(|ctx: &ConversationCtx, cx: &App| with_conversation(ctx, cx, |_, c| !c.favorite)),
    );
    m.add(
        MenuItem::new(
            "sidebar-unfavourite",
            t!("sidebar-menu-unfavourite"),
            |ctx: &ConversationCtx, _, cx| {
                set_favourite(&ctx.session, &ctx.conversation, false, cx)
            },
        )
        .icon(IconName::StarOff)
        .order(menu_order::conversation::FAVOURITE)
        .when(|ctx: &ConversationCtx, cx: &App| with_conversation(ctx, cx, |_, c| c.favorite)),
    );
    // Notifications opens the levels, as the web's submenu: a channel's are
    // channel-wide and set by its operators, a heading says so.
    m.add(
        MenuItem::new(
            "sidebar-notifications",
            t!("sidebar-menu-notifications"),
            |_: &ConversationCtx, _, _| {},
        )
        .icon(IconName::Bell)
        .order(menu_order::conversation::NOTIFICATIONS),
    );
    use menu_order::conversation::level;
    let under = "sidebar-notifications";
    m.add(
        MenuItem::heading("sidebar-level-heading", t!("sidebar-menu-level-everyone"))
            .under(under)
            .order(level::HEADING)
            .label_with(|ctx: &ConversationCtx, cx: &App| {
                if with_conversation(ctx, cx, can_set_level) {
                    t!("sidebar-menu-level-everyone").into()
                } else {
                    t!("sidebar-menu-level-operators-only").into()
                }
            })
            .when(|ctx: &ConversationCtx, cx: &App| {
                with_conversation(ctx, cx, |_, c| order::channel_wide(c))
            }),
    );
    let levels = [
        ("sidebar-level-default", level::DEFAULT, None, false),
        (
            "sidebar-level-channel-default",
            level::CHANNEL_DEFAULT,
            None,
            true,
        ),
        ("sidebar-level-all", level::ALL, Some(Level::All), false),
        (
            "sidebar-level-mentions",
            level::MENTIONS,
            Some(Level::Mentions),
            false,
        ),
        ("sidebar-level-none", level::NONE, Some(Level::None), false),
    ];
    for (id, at, choice, channel_only) in levels {
        m.add(
            MenuItem::new(
                id,
                level_label(choice),
                move |ctx: &ConversationCtx, window, cx| {
                    set_level(&ctx.session, &ctx.conversation, choice, window, cx)
                },
            )
            .under(under)
            .order(at)
            .label_with(move |ctx: &ConversationCtx, cx: &App| match choice {
                Some(_) => level_label(choice),
                None => {
                    let session = ctx.session.read(cx);
                    let level = session
                        .client()
                        .and_then(|client| {
                            let w = client.read();
                            w.conversation(&ctx.conversation)
                                .map(|c| order::default_level(&w, c))
                        })
                        .unwrap_or(Level::Mentions);
                    let level = level_text(level);
                    if channel_only {
                        t!("sidebar-menu-level-channel-default", level = level).into()
                    } else {
                        t!("sidebar-menu-level-default", level = level).into()
                    }
                }
            })
            .when(move |ctx: &ConversationCtx, cx: &App| {
                // The default for your own levels, the channel's default for
                // a channel's; the levels themselves for both.
                choice.is_some()
                    || with_conversation(ctx, cx, |_, c| order::channel_wide(c) == channel_only)
            })
            .checked_when(move |ctx: &ConversationCtx, cx: &App| {
                with_conversation(ctx, cx, |w, c| {
                    // Your own levels show a choice once the settings are read.
                    (order::channel_wide(c) || w.documents.notification_settings.is_loaded())
                        && chosen_level(w, c) == choice
                })
            })
            .disabled_when(|ctx: &ConversationCtx, cx: &App| {
                !with_conversation(ctx, cx, can_set_level)
            }),
        );
    }
    m.add(
        MenuItem::new(
            "sidebar-move-up",
            t!("sidebar-menu-move-up"),
            |ctx: &ConversationCtx, _, cx| step(&ctx.session, &ctx.conversation, -1, cx),
        )
        .icon(IconName::ArrowUp)
        .order(menu_order::conversation::MOVE_UP)
        .when(|ctx: &ConversationCtx, cx: &App| {
            with_conversation(ctx, cx, |w, c| order::steps(w, c).0)
        }),
    );
    m.add(
        MenuItem::new(
            "sidebar-move-down",
            t!("sidebar-menu-move-down"),
            |ctx: &ConversationCtx, _, cx| step(&ctx.session, &ctx.conversation, 1, cx),
        )
        .icon(IconName::ArrowDown)
        .order(menu_order::conversation::MOVE_DOWN)
        .when(|ctx: &ConversationCtx, cx: &App| {
            with_conversation(ctx, cx, |w, c| order::steps(w, c).1)
        }),
    );
    m.add(
        MenuItem::new(
            "sidebar-sort",
            t!("sidebar-menu-sort"),
            |ctx: &ConversationCtx, _, cx| {
                if let Some(c) = conversation_of(ctx, cx) {
                    sort_by_name(&ctx.session, &c.network_id, order::section_of(&c), cx)
                }
            },
        )
        .icon(IconName::SortAscending)
        .order(menu_order::conversation::SORT)
        .when(|ctx: &ConversationCtx, cx: &App| {
            with_conversation(ctx, cx, |w, c| {
                order::custom(w, &c.network_id, order::section_of(c))
            })
        }),
    );
    m.add(
        MenuItem::new(
            "sidebar-copy-channel",
            t!("sidebar-menu-copy-channel"),
            |ctx: &ConversationCtx, window, cx| {
                if let Some(c) = conversation_of(ctx, cx) {
                    cx.write_to_clipboard(ClipboardItem::new_string(channel_target(&c)));
                    ui::toast::info(window, cx, t!("sidebar-copied-channel"));
                }
            },
        )
        .icon(IconName::Copy)
        .order(menu_order::conversation::COPY_CHANNEL)
        .when(|ctx: &ConversationCtx, cx: &App| {
            with_conversation(ctx, cx, |_, c| order::channel_wide(c))
        }),
    );
    m.add(
        MenuItem::new(
            "sidebar-leave",
            t!("sidebar-menu-leave"),
            |ctx: &ConversationCtx, window, cx| {
                let Some(c) = conversation_of(ctx, cx) else {
                    return;
                };
                let (session, id) = (ctx.session.clone(), ctx.conversation.clone());
                ui::confirm::confirm(
                    window,
                    cx,
                    Confirm {
                        title: t!("sidebar-leave-title", channel = channel_target(&c)).into(),
                        body: t!("sidebar-leave-body").into(),
                        ok: t!("sidebar-menu-leave").into(),
                        danger: true,
                    },
                    move |_, cx| remove_conversation(&session, &id, cx),
                );
            },
        )
        .icon(IconName::Close)
        .danger()
        .order(menu_order::conversation::LEAVE)
        .when(|ctx: &ConversationCtx, cx: &App| {
            with_conversation(ctx, cx, |_, c| order::channel_wide(c) && c.joined)
        }),
    );
    m.add(
        MenuItem::new(
            "sidebar-close",
            t!("sidebar-menu-close"),
            |ctx: &ConversationCtx, _, cx| remove_conversation(&ctx.session, &ctx.conversation, cx),
        )
        .icon(IconName::Close)
        .danger()
        .order(menu_order::conversation::LEAVE)
        .when(|ctx: &ConversationCtx, cx: &App| {
            with_conversation(ctx, cx, |_, c| {
                c.kind == ConversationKind::Direct && c.party.is_none()
            })
        }),
    );
}

fn network_menu(m: &mut registry::List<MenuItem<NetworkCtx>>) {
    let status = |ctx: &NetworkCtx, cx: &App| -> Option<NetworkStatus> {
        let session = ctx.session.read(cx);
        let client = session.client()?;
        client.read().network(&ctx.network).map(|n| n.status)
    };
    m.add(
        MenuItem::new(
            "sidebar-mark-all-read",
            t!("sidebar-menu-mark-all-read"),
            |ctx: &NetworkCtx, _, cx| {
                mark_network_read(&ctx.session, &ctx.network, cx);
            },
        )
        .icon(IconName::Check)
        .order(menu_order::network::MARK_ALL_READ)
        .when(|ctx: &NetworkCtx, cx: &App| {
            let session = ctx.session.read(cx);
            session
                .client()
                .is_some_and(|c| !order::unread_in(&c.read(), Some(&ctx.network)).is_empty())
        }),
    );
    m.add(
        MenuItem::new("sidebar-connect", t!("sidebar-menu-connect"), |ctx: &NetworkCtx, _, cx| {
            set_connected(&ctx.session, &ctx.network, true, cx)
        })
        .icon(IconName::RefreshCw)
        .order(menu_order::network::CONNECTION)
        .when(move |ctx: &NetworkCtx, cx: &App| {
            matches!(status(ctx, cx), Some(s) if s != NetworkStatus::Connected && s != NetworkStatus::Connecting)
        }),
    );
    m.add(
        MenuItem::new(
            "sidebar-cancel-connect",
            t!("sidebar-menu-cancel-connect"),
            |ctx: &NetworkCtx, _, cx| set_connected(&ctx.session, &ctx.network, false, cx),
        )
        .icon(IconName::Close)
        .order(menu_order::network::CONNECTION)
        .when(move |ctx: &NetworkCtx, cx: &App| status(ctx, cx) == Some(NetworkStatus::Connecting)),
    );
    m.add(
        MenuItem::new(
            "sidebar-disconnect",
            t!("sidebar-menu-disconnect"),
            |ctx: &NetworkCtx, _, cx| set_connected(&ctx.session, &ctx.network, false, cx),
        )
        .icon(IconName::Close)
        .order(menu_order::network::CONNECTION)
        .when(move |ctx: &NetworkCtx, cx: &App| status(ctx, cx) == Some(NetworkStatus::Connected)),
    );
    m.add(
        MenuItem::new(
            "sidebar-copy-address",
            t!("sidebar-menu-copy-address"),
            |ctx: &NetworkCtx, window, cx| {
                let address = {
                    let session = ctx.session.read(cx);
                    session.client().and_then(|c| {
                        c.read()
                            .network(&ctx.network)
                            .map(|n| format!("{}:{}", n.host, n.port))
                    })
                };
                if let Some(address) = address {
                    cx.write_to_clipboard(ClipboardItem::new_string(address));
                    ui::toast::info(window, cx, t!("sidebar-copied-address"));
                }
            },
        )
        .icon(IconName::Copy)
        .order(menu_order::network::COPY_ADDRESS),
    );
    m.add(
        MenuItem::new(
            "sidebar-remove-network",
            t!("sidebar-menu-remove-network"),
            |ctx: &NetworkCtx, window, cx| {
                let name = {
                    let session = ctx.session.read(cx);
                    session
                        .client()
                        .and_then(|c| c.read().network(&ctx.network).map(|n| n.name.clone()))
                        .unwrap_or_default()
                };
                let (session, id) = (ctx.session.clone(), ctx.network.clone());
                ui::confirm::confirm(
                    window,
                    cx,
                    Confirm {
                        title: t!("sidebar-remove-network-title", network = name).into(),
                        body: t!("sidebar-remove-network-body").into(),
                        ok: t!("sidebar-remove-network-ok").into(),
                        danger: true,
                    },
                    move |_, cx| remove_network(&session, &id, cx),
                );
            },
        )
        .icon(IconName::Delete)
        .danger()
        .order(menu_order::network::REMOVE),
    );
}

// ---- What the entries do ----

fn notice(changes: &mut Changes, e: impl std::fmt::Display) {
    changes.notices.push(e.to_string());
}

pub fn mark_read(session: &Entity<Session>, conversation: &str, cx: &App) {
    if let Some(client) = session.read(cx).client() {
        client.mark_read(conversation);
    }
}

/// Mark every conversation of a network read. Returns how many were unread.
pub fn mark_network_read(session: &Entity<Session>, network: &str, cx: &App) -> usize {
    let Some(client) = session.read(cx).client() else {
        return 0;
    };
    let ids: Vec<String> = order::unread_in(&client.read(), Some(network))
        .iter()
        .map(|c| c.id.clone())
        .collect();
    for id in &ids {
        client.mark_read(id);
    }
    ids.len()
}

fn put_conversation(w: &mut Workspace, c: c::Conversation) -> Changes {
    match w.conversations.iter_mut().find(|x| x.id == c.id) {
        Some(existing) => *existing = c,
        None => w.conversations.push(c),
    }
    Changes {
        sidebar: true,
        ..Default::default()
    }
}

pub fn set_favourite(session: &Entity<Session>, conversation: &str, favorite: bool, cx: &App) {
    let Some(client) = session.read(cx).client() else {
        return;
    };
    let path = format!("/api/v1/conversations/{}", segment(conversation));
    let body = c::ConversationPatchBody {
        favorite: Some(favorite),
        ..Default::default()
    };
    client.call(
        move |api| async move {
            api.call::<_, c::Conversation>(Method::PATCH, &path, &body)
                .await
        },
        |w, result| match result {
            Ok(c) => put_conversation(w, c),
            Err(e) => {
                let mut changes = Changes::default();
                notice(&mut changes, e);
                changes
            }
        },
    );
}

/// The level you or the operators chose, `None` for the default.
fn chosen_level(w: &Workspace, c: &Conversation) -> Option<Level> {
    if order::channel_wide(c) {
        order::channel_level(c)
    } else {
        order::own_level(w, c)
    }
}

/// A level's row in the Notifications submenu.
fn level_label(level: Option<Level>) -> SharedString {
    match level {
        Some(Level::All) => t!("sidebar-menu-level-all").into(),
        Some(Level::Mentions) => t!("sidebar-menu-level-mentions").into(),
        Some(Level::None) => t!("sidebar-menu-level-none").into(),
        None => SharedString::default(),
    }
}

/// A level inside "Default (…)".
fn level_text(level: Level) -> String {
    match level {
        Level::All => t!("sidebar-level-all"),
        Level::Mentions => t!("sidebar-level-mentions"),
        Level::None => t!("sidebar-level-none"),
    }
    .to_string()
}

/// Channel levels are channel-wide and set by operators; your own levels
/// need the notification settings read first.
fn can_set_level(w: &Workspace, c: &Conversation) -> bool {
    if order::channel_wide(c) {
        let allowed = Allowed::of(w, &c.id, true);
        let network = w.network(&c.network_id);
        channel_rights_for(allowed.own_modes(), Some(c), network).op
    } else {
        w.documents.notification_settings.is_loaded()
    }
}

pub fn set_level(
    session: &Entity<Session>,
    conversation: &str,
    level: Option<Level>,
    window: &mut Window,
    cx: &mut App,
) {
    let Some(client) = session.read(cx).client().cloned() else {
        return;
    };
    let w = client.read();
    let Some(c) = w.conversation(conversation).cloned() else {
        return;
    };
    if !can_set_level(&w, &c) {
        drop(w);
        return ui::toast::info(window, cx, t!("sidebar-menu-level-operators"));
    }
    if order::channel_wide(&c) {
        drop(w);
        use ChannelSettingsPatchBodyNotifications as N;
        let body = ChannelSettingsPatchBody {
            notifications: Some(match level {
                Some(Level::All) => N::All,
                Some(Level::Mentions) => N::Mentions,
                Some(Level::None) => N::None,
                None => N::Default,
            }),
            ..Default::default()
        };
        let path = format!(
            "/api/v1/conversations/{}/channel-settings",
            segment(conversation)
        );
        client.call(
            move |api| async move {
                api.call::<_, c::Conversation>(Method::PUT, &path, &body)
                    .await
            },
            |w, result| match result {
                Ok(c) => put_conversation(w, c),
                Err(e) => {
                    let mut changes = Changes::default();
                    notice(&mut changes, e);
                    changes
                }
            },
        );
        return;
    }
    let Some(mut settings) = w.documents.notification_settings.value().cloned() else {
        return;
    };
    drop(w);
    match level {
        Some(Level::All) => settings
            .levels
            .insert(c.id.clone(), SettingsLevelsValue::All),
        Some(Level::Mentions) => settings
            .levels
            .insert(c.id.clone(), SettingsLevelsValue::Mentions),
        Some(Level::None) => settings
            .levels
            .insert(c.id.clone(), SettingsLevelsValue::None),
        None => settings.levels.remove(&c.id),
    };
    client.edit_document(&documents::NOTIFICATION_SETTINGS, settings);
}

/// Save a new order, if the preferences were read (an edit before that
/// would overwrite the account's other preferences).
fn save_order(
    session: &Entity<Session>,
    cx: &App,
    next: impl FnOnce(&Workspace, &c::StoredPreferences) -> Option<c::StoredPreferences>,
) {
    let Some(client) = session.read(cx).client() else {
        return;
    };
    let w = client.read();
    let Some(current) = w.documents.preferences.value().cloned() else {
        return;
    };
    let next = next(&w, &current);
    drop(w);
    if let Some(next) = next {
        client.edit_document(&documents::PREFERENCES, next);
    }
}

pub fn step(session: &Entity<Session>, conversation: &str, direction: i32, cx: &App) {
    save_order(session, cx, |w, current| {
        let c = w.conversation(conversation)?;
        order::stepped(w, current, c, direction)
    });
}

/// Drop `id` next to `target` in a section.
pub fn move_to(
    session: &Entity<Session>,
    network: &str,
    section: Section,
    id: &str,
    target: &str,
    place: Place,
    cx: &App,
) {
    save_order(session, cx, |w, current| {
        order::moved(w, current, network, section, id, target, place)
    });
}

pub fn sort_by_name(session: &Entity<Session>, network: &str, section: Section, cx: &App) {
    let key = order::section_key(network, section);
    save_order(session, cx, |_, current| {
        Some(order::with_order(current, &key, None))
    });
}

/// Leave a channel or close a direct message.
pub fn remove_conversation(session: &Entity<Session>, conversation: &str, cx: &App) {
    let Some(client) = session.read(cx).client() else {
        return;
    };
    let path = format!("/api/v1/conversations/{}", segment(conversation));
    let id = conversation.to_string();
    client.call(
        move |api| async move { api.delete(&path).await },
        move |w, result| match result {
            Ok(()) => {
                // A channel stays listed as not joined until the server's
                // event; a direct message goes at once.
                let direct = w
                    .conversation(&id)
                    .is_some_and(|c| c.kind == ConversationKind::Direct);
                if direct {
                    w.conversations.retain(|c| c.id != id);
                } else if let Some(c) = w.conversations.iter_mut().find(|c| c.id == id) {
                    c.joined = false;
                }
                Changes {
                    sidebar: true,
                    ..Default::default()
                }
            }
            Err(e) => {
                let mut changes = Changes::default();
                notice(&mut changes, e);
                changes
            }
        },
    );
}

pub fn set_connected(session: &Entity<Session>, network: &str, connect: bool, cx: &App) {
    let Some(client) = session.read(cx).client() else {
        return;
    };
    let path = format!("/api/v1/networks/{}", segment(network));
    let body = c::NetworkUpdate {
        status: Some(if connect {
            c::NetworkUpdateStatus::Connected
        } else {
            c::NetworkUpdateStatus::Disconnected
        }),
        ..Default::default()
    };
    client.call(
        move |api| async move { api.call::<_, c::Network>(Method::PATCH, &path, &body).await },
        |w, result| match result {
            Ok(n) => {
                match w.networks.iter_mut().find(|x| x.id == n.id) {
                    Some(existing) => *existing = n,
                    None => w.networks.push(n),
                }
                Changes {
                    sidebar: true,
                    ..Default::default()
                }
            }
            Err(e) => {
                let mut changes = Changes::default();
                notice(&mut changes, e);
                changes
            }
        },
    );
}

pub fn remove_network(session: &Entity<Session>, network: &str, cx: &mut App) {
    let Some(client) = session.read(cx).client().cloned() else {
        return;
    };
    let path = format!("/api/v1/networks/{}", segment(network));
    let id = network.to_string();
    let task = client.call(
        move |api| async move { api.delete(&path).await },
        move |w, result| match result {
            Ok(()) => {
                w.networks.retain(|n| n.id != id);
                w.conversations.retain(|c| c.network_id != id);
                Changes {
                    sidebar: true,
                    ..Default::default()
                }
            }
            Err(e) => {
                let mut changes = Changes::default();
                notice(&mut changes, e);
                changes
            }
        },
    );
    // Showing the removed network: move to the first one left.
    let session = session.clone();
    let removed = network.to_string();
    cx.spawn(async move |cx| {
        let _ = task.await;
        session.update(cx, |s, cx| {
            let Some(client) = s.client().cloned() else {
                return;
            };
            let gone = client.read().network(&removed).is_none();
            if gone && s.network.as_deref() == Some(removed.as_str()) {
                let first = client.read().networks.first().map(|n| n.id.clone());
                if let Some(first) = first {
                    s.select_network(&first, cx);
                } else {
                    s.network = None;
                    s.conversation = None;
                    cx.notify();
                }
            }
        });
    })
    .detach();
}

//! The default keys and their actions, and the shortcut help (package N1).
//!
//! The web client's 23 workspace shortcuts (`shared/utils/shortcuts.ts`),
//! Ctrl+1 to 9 for the numbered channels, and back and forward through
//! views. Every key is a [`KeyDefault`] in the registry, so the shortcuts
//! settings (S2) can list and change them, and the help ([`ShortcutHelp`])
//! lists whatever is registered.
//!
//! N1 handles the actions that move around the workspace. The others are
//! named here so their owners only add a handler (`cx.on_action` or an
//! element's `on_action`) and need no key of their own: [`Gifs`] (E1),
//! [`ToggleMembers`] (P1), [`JoinChannel`] (CH1) and [`PauseTheme`] (S1).
//! Jump to unread and to the newest message (R1) and the formatting keys
//! (C1) are registered by their packages; the help files them under their
//! groups by ID. [`OpenInbox`], [`OpenSaved`] and [`OpenSettings`] open
//! their routes, drawn by the panes those packages register.

use gpui_kit::TestSupportExt;
use gpui_kit::component::dialog::Dialog;
use gpui_kit::prelude::*;
use gpui_kit::*;
use ircord_client::store::sidebar_order as order;

use crate::composer::ComposerView;
use crate::registry::{self, KeyDefault};
use crate::route::Route;
use crate::session::Session;
use crate::sidebar::{layout, menus};
use crate::theme;
use crate::ui;

gpui_kit::actions!(
    ircord,
    [
        Search,
        PreviousConversation,
        NextConversation,
        PreviousUnread,
        NextUnread,
        PreviousNetwork,
        NextNetwork,
        OpenInbox,
        OpenSaved,
        JoinChannel,
        GoBack,
        GoForward,
        MarkRead,
        MarkNetworkRead,
        ToggleMembers,
        FocusComposer,
        Gifs,
        OpenSettings,
        PauseTheme,
        ShortcutHelp,
        OpenChannel1,
        OpenChannel2,
        OpenChannel3,
        OpenChannel4,
        OpenChannel5,
        OpenChannel6,
        OpenChannel7,
        OpenChannel8,
        OpenChannel9,
    ]
);

/// The help's groups, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Group {
    GettingAround,
    Reading,
    Writing,
    App,
    /// Keys other packages register.
    Other,
}

/// The group a key ID belongs to in the help.
pub fn group_of(id: &str) -> Group {
    match id {
        "search" | "previous" | "next" | "previous-unread" | "next-unread" | "previous-network"
        | "next-network" | "inbox" | "saved" | "join" | "back" | "forward" => Group::GettingAround,
        "jump-to-unread" | "jump-to-present" | "mark-read" | "mark-network-read"
        | "toggle-members" => Group::Reading,
        "focus-composer" | "gifs" | "format-bold" | "format-italic" | "format-underline" => {
            Group::Writing
        }
        "settings" | "pause-theme" | "help" | "help-question" => Group::App,
        _ => Group::Other,
    }
}

const MAC: bool = cfg!(target_os = "macos");

/// The window's root context. A binding without a context would outrank
/// every control's own keys (Escape in a dialog, arrows in a field), so the
/// workspace keys sit at the bottom of the context stack instead.
const ROOT: Option<&str> = Some("Root");

/// The platform's main modifier: ⌘ on a Mac, Ctrl elsewhere.
const fn m(mac: &'static str, other: &'static str) -> &'static str {
    if MAC { mac } else { other }
}

fn keys() -> Vec<KeyDefault> {
    let back = m("cmd-[", "alt-left");
    let forward = m("cmd-]", "alt-right");
    let mut keys = vec![
        KeyDefault::new(
            "search",
            t!("sidebar-key-search"),
            m("cmd-k", "ctrl-k"),
            ROOT,
            Search,
        ),
        KeyDefault::new(
            "previous",
            t!("sidebar-key-previous"),
            "alt-up",
            ROOT,
            PreviousConversation,
        ),
        KeyDefault::new(
            "next",
            t!("sidebar-key-next"),
            "alt-down",
            ROOT,
            NextConversation,
        ),
        KeyDefault::new(
            "previous-unread",
            t!("sidebar-key-previous-unread"),
            "alt-shift-up",
            ROOT,
            PreviousUnread,
        ),
        KeyDefault::new(
            "next-unread",
            t!("sidebar-key-next-unread"),
            "alt-shift-down",
            ROOT,
            NextUnread,
        ),
        KeyDefault::new(
            "previous-network",
            t!("sidebar-key-previous-network"),
            m("cmd-alt-up", "ctrl-alt-up"),
            ROOT,
            PreviousNetwork,
        ),
        KeyDefault::new(
            "next-network",
            t!("sidebar-key-next-network"),
            m("cmd-alt-down", "ctrl-alt-down"),
            ROOT,
            NextNetwork,
        ),
        KeyDefault::new("inbox", t!("sidebar-key-inbox"), "", ROOT, OpenInbox),
        KeyDefault::new("saved", t!("sidebar-key-saved"), "", ROOT, OpenSaved),
        KeyDefault::new("join", t!("sidebar-key-join"), "", ROOT, JoinChannel),
        KeyDefault::new("back", t!("sidebar-key-back"), back, ROOT, GoBack),
        KeyDefault::new(
            "forward",
            t!("sidebar-key-forward"),
            forward,
            ROOT,
            GoForward,
        ),
        KeyDefault::new(
            "mark-read",
            t!("sidebar-key-mark-read"),
            "escape",
            ROOT,
            MarkRead,
        ),
        KeyDefault::new(
            "mark-network-read",
            t!("sidebar-key-mark-network-read"),
            "shift-escape",
            ROOT,
            MarkNetworkRead,
        ),
        KeyDefault::new(
            "toggle-members",
            t!("sidebar-key-toggle-members"),
            "",
            ROOT,
            ToggleMembers,
        ),
        KeyDefault::new(
            "focus-composer",
            t!("sidebar-key-focus-composer"),
            "alt-shift-c",
            ROOT,
            FocusComposer,
        ),
        KeyDefault::new(
            "gifs",
            t!("sidebar-key-gifs"),
            m("cmd-g", "ctrl-g"),
            ROOT,
            Gifs,
        ),
        KeyDefault::new(
            "settings",
            t!("sidebar-key-settings"),
            m("cmd-,", "ctrl-,"),
            ROOT,
            OpenSettings,
        ),
        KeyDefault::new(
            "pause-theme",
            t!("sidebar-key-pause-theme"),
            "shift-f8",
            ROOT,
            PauseTheme,
        ),
        KeyDefault::new(
            "help",
            t!("sidebar-key-help"),
            m("cmd-/", "ctrl-/"),
            ROOT,
            ShortcutHelp,
        ),
        // “?” types text in a field; it opens the help everywhere else.
        KeyDefault::new(
            "help-question",
            t!("sidebar-key-help"),
            "?",
            Some("!Input"),
            ShortcutHelp,
        ),
    ];
    keys.extend([
        channel_key(0, OpenChannel1),
        channel_key(1, OpenChannel2),
        channel_key(2, OpenChannel3),
        channel_key(3, OpenChannel4),
        channel_key(4, OpenChannel5),
        channel_key(5, OpenChannel6),
        channel_key(6, OpenChannel7),
        channel_key(7, OpenChannel8),
        channel_key(8, OpenChannel9),
    ]);
    for (i, key) in keys.iter_mut().enumerate() {
        key.order = i as i32;
    }
    keys
}

/// Ctrl+1 to 9: the numbered channels.
fn channel_key<A: Action + Clone>(index: usize, action: A) -> KeyDefault {
    const IDS: [&str; 9] = [
        "channel-1",
        "channel-2",
        "channel-3",
        "channel-4",
        "channel-5",
        "channel-6",
        "channel-7",
        "channel-8",
        "channel-9",
    ];
    const KEYS: [&str; 9] = [
        m("cmd-1", "ctrl-1"),
        m("cmd-2", "ctrl-2"),
        m("cmd-3", "ctrl-3"),
        m("cmd-4", "ctrl-4"),
        m("cmd-5", "ctrl-5"),
        m("cmd-6", "ctrl-6"),
        m("cmd-7", "ctrl-7"),
        m("cmd-8", "ctrl-8"),
        m("cmd-9", "ctrl-9"),
    ];
    let label = t!("sidebar-key-channel", number = index + 1);
    KeyDefault::new(IDS[index], label, KEYS[index], ROOT, action)
}

pub fn init(cx: &mut App) {
    crate::sidebar::init(cx);
    cx.default_global::<Composer>();
    cx.observe_new::<ComposerView>(|_, _, cx| {
        let weak = cx.weak_entity();
        cx.global_mut::<Composer>().0 = Some(weak);
    })
    .detach();
    registry::register(cx, |r| {
        for key in keys() {
            r.keys.add(key);
        }
    });

    cx.on_action(|_: &Search, cx| layout::with_window(cx, crate::features::switcher::open));
    cx.on_action(|_: &ShortcutHelp, cx| layout::with_window(cx, open_help));
    cx.on_action(|_: &PreviousConversation, cx| step(-1, cx));
    cx.on_action(|_: &NextConversation, cx| step(1, cx));
    cx.on_action(|_: &PreviousUnread, cx| unread(-1, cx));
    cx.on_action(|_: &NextUnread, cx| unread(1, cx));
    cx.on_action(|_: &PreviousNetwork, cx| network(-1, cx));
    cx.on_action(|_: &NextNetwork, cx| network(1, cx));
    cx.on_action(|_: &OpenInbox, cx| toggle_route(Route::Inbox, cx));
    cx.on_action(|_: &OpenSaved, cx| toggle_route(Route::Saved, cx));
    cx.on_action(|_: &OpenSettings, cx| toggle_route(Route::Settings, cx));
    cx.on_action(|_: &GoBack, cx| with_session(cx, |s, cx| s.go_back(cx)));
    cx.on_action(|_: &GoForward, cx| with_session(cx, |s, cx| s.go_forward(cx)));
    cx.on_action(|_: &MarkRead, cx| mark_read(cx));
    cx.on_action(|_: &MarkNetworkRead, cx| mark_network_read(cx));
    cx.on_action(|_: &FocusComposer, cx| focus_composer(cx));
    cx.on_action(|_: &OpenChannel1, cx| open_numbered(0, cx));
    cx.on_action(|_: &OpenChannel2, cx| open_numbered(1, cx));
    cx.on_action(|_: &OpenChannel3, cx| open_numbered(2, cx));
    cx.on_action(|_: &OpenChannel4, cx| open_numbered(3, cx));
    cx.on_action(|_: &OpenChannel5, cx| open_numbered(4, cx));
    cx.on_action(|_: &OpenChannel6, cx| open_numbered(5, cx));
    cx.on_action(|_: &OpenChannel7, cx| open_numbered(6, cx));
    cx.on_action(|_: &OpenChannel8, cx| open_numbered(7, cx));
    cx.on_action(|_: &OpenChannel9, cx| open_numbered(8, cx));
}

/// The composer, for Focus the message box.
#[derive(Default)]
struct Composer(Option<WeakEntity<ComposerView>>);
impl Global for Composer {}

fn with_session(cx: &mut App, f: impl FnOnce(&mut Session, &mut Context<Session>)) {
    if let Some(session) = layout::session(cx) {
        session.update(cx, f);
    }
}

fn step(direction: i32, cx: &mut App) {
    with_session(cx, |s, cx| {
        let Some(client) = s.client().cloned() else {
            return;
        };
        let Some(network) = s.network.clone() else {
            return;
        };
        let next = order::step_conversation(
            &client.read(),
            &network,
            s.conversation.as_deref(),
            direction,
        );
        if let Some(id) = next {
            s.select_conversation(&id, cx);
        }
    });
}

fn unread(direction: i32, cx: &mut App) {
    let mut nothing = false;
    with_session(cx, |s, cx| {
        let Some(client) = s.client().cloned() else {
            return;
        };
        let next = order::next_unread(&client.read(), s.conversation.as_deref(), direction);
        match next {
            Some(id) => s.select_conversation(&id, cx),
            None => nothing = true,
        }
    });
    if nothing {
        layout::with_window(cx, |window, cx| {
            ui::toast::info(window, cx, t!("sidebar-nothing-unread"))
        });
    }
}

fn network(direction: i32, cx: &mut App) {
    with_session(cx, |s, cx| {
        let Some(client) = s.client().cloned() else {
            return;
        };
        let next = order::next_network(&client.read().networks, s.network.as_deref(), direction)
            .map(|n| n.id.clone());
        if let Some(id) = next {
            s.select_network(&id, cx);
        }
    });
}

/// Open a route, or go back from it when it is already showing.
fn toggle_route(route: Route, cx: &mut App) {
    with_session(cx, |s, cx| {
        if s.route().as_ref() == Some(&route) {
            s.go_back(cx);
        } else {
            s.navigate(route, cx);
        }
    });
}

fn mark_read(cx: &mut App) {
    let Some(session) = layout::session(cx) else {
        return;
    };
    let open = session
        .read(cx)
        .route()
        .and_then(|r| r.conversation().map(str::to_string));
    if let Some(id) = open {
        menus::mark_read(&session, &id, cx);
    }
}

fn mark_network_read(cx: &mut App) {
    let Some(session) = layout::session(cx) else {
        return;
    };
    let network = session.read(cx).network.clone();
    if let Some(network) = network {
        menus::mark_network_read(&session, &network, cx);
    }
}

fn focus_composer(cx: &mut App) {
    let Some(composer) = cx.global::<Composer>().0.as_ref().and_then(|c| c.upgrade()) else {
        return;
    };
    layout::with_window(cx, move |window, cx| {
        let input = composer.read(cx).input.clone();
        input.focus_handle(cx).focus(window, cx);
    });
}

fn open_numbered(index: usize, cx: &mut App) {
    with_session(cx, |s, cx| {
        let Some(client) = s.client().cloned() else {
            return;
        };
        let Some(network) = s.network.clone() else {
            return;
        };
        let target = order::numbered(&client.read(), &network)
            .get(index)
            .cloned();
        if let Some(id) = target {
            s.select_conversation(&id, cx);
        }
    });
}

/// "Ctrl Alt ↑" for `secondary-alt-up` on Windows and Linux, "⌘ ⌥ ↑" on a Mac.
pub fn chord_label(keys: &str, mac: bool) -> String {
    keys.split_whitespace()
        .map(|stroke| {
            let mut parts: Vec<&str> = Vec::new();
            let mut rest = stroke;
            // A trailing "-" is the minus key itself.
            while let Some((modifier, tail)) = rest.split_once('-') {
                if tail.is_empty() {
                    break;
                }
                parts.push(modifier);
                rest = tail;
            }
            let mut words: Vec<String> = parts
                .iter()
                .map(|m| {
                    match (*m, mac) {
                        ("secondary" | "cmd", true) => "⌘",
                        ("secondary" | "ctrl", false) => "Ctrl",
                        ("ctrl", true) => "⌃",
                        ("cmd", false) => "Win",
                        ("alt", true) => "⌥",
                        ("alt", false) => "Alt",
                        ("shift", true) => "⇧",
                        ("shift", false) => "Shift",
                        (other, _) => other,
                    }
                    .to_string()
                })
                .collect();
            words.push(match rest {
                "up" => "↑".to_string(),
                "down" => "↓".to_string(),
                "left" => "←".to_string(),
                "right" => "→".to_string(),
                "pageup" => t!("sidebar-key-name-page-up").to_string(),
                "pagedown" => t!("sidebar-key-name-page-down").to_string(),
                "escape" => t!("sidebar-key-name-escape").to_string(),
                "space" => t!("sidebar-key-name-space").to_string(),
                "enter" => t!("sidebar-key-name-enter").to_string(),
                "tab" => t!("sidebar-key-name-tab").to_string(),
                key => key.to_uppercase(),
            });
            words.join(" ")
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// The help: every registered key by group, with its keys.
pub fn open_help(window: &mut Window, cx: &mut App) {
    ui::dialog::open(
        window,
        cx,
        t!("sidebar-help-title"),
        |dialog: Dialog, _, cx| {
            let theme = theme::get(cx).clone();
            let keys = registry::get(cx).keys.to_vec();
            let groups = [
                (Group::GettingAround, t!("sidebar-help-getting-around")),
                (Group::Reading, t!("sidebar-help-reading")),
                (Group::Writing, t!("sidebar-help-writing")),
                (Group::App, t!("sidebar-help-app")),
                (Group::Other, t!("sidebar-help-other")),
            ];
            let mut body = div()
                .id("shortcut-list")
                .test_support()
                .role(Role::List)
                .aria_label(t!("sidebar-help-title"))
                .flex()
                .flex_col()
                .gap_3()
                .max_h(px(520.))
                .overflow_y_scroll();
            for (group, title) in groups {
                // Both help keys show as one row; the numbered channels as one.
                let mut rows: Vec<(String, SharedString, String)> = Vec::new();
                for key in keys.iter().filter(|k| group_of(&k.id) == group) {
                    if key.id.starts_with("channel-") || key.id == "help-question" {
                        continue;
                    }
                    let mut bound = Vec::new();
                    if key.id == "help" {
                        for other in keys.iter().filter(|k| k.id == "help-question") {
                            bound.extend(registry::keys_of(cx, other));
                        }
                    }
                    bound.extend(registry::keys_of(cx, key));
                    let chords = if bound.is_empty() {
                        t!("sidebar-help-no-key").to_string()
                    } else {
                        bound
                            .iter()
                            .map(|k| chord_label(k, MAC))
                            .collect::<Vec<_>>()
                            .join(", ")
                    };
                    rows.push((key.id.to_string(), key.label.clone(), chords));
                }
                if group == Group::GettingAround {
                    let first = if MAC { "⌘ 1" } else { "Ctrl 1" };
                    rows.insert(
                        1,
                        (
                            "channels".into(),
                            t!("sidebar-help-channels").into(),
                            format!("{first}–9"),
                        ),
                    );
                }
                if rows.is_empty() {
                    continue;
                }
                let mut section = div().flex().flex_col().gap_1().child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::BOLD)
                        .text_color(theme.muted)
                        .child(title.to_uppercase()),
                );
                for (id, label, chords) in rows {
                    section = section.child(
                        div()
                            .id(ElementId::Name(format!("shortcut-{id}").into()))
                            .test_support()
                            .role(Role::ListItem)
                            .aria_label(SharedString::from(format!("{label}: {chords}")))
                            .flex()
                            .justify_between()
                            .gap_4()
                            .text_sm()
                            .child(div().child(label))
                            .child(
                                div()
                                    .px_1p5()
                                    .rounded_sm()
                                    .bg(theme.input)
                                    .text_color(theme.text)
                                    .child(chords),
                            ),
                    );
                }
                body = body.child(section);
            }
            dialog.w(px(520.)).child(body)
        },
    )
}

#[cfg(test)]
mod tests {
    use super::{Group, chord_label, group_of, keys};

    #[test]
    fn chords_read_like_the_web_client() {
        assert_eq!(chord_label("secondary-k", false), "Ctrl K");
        assert_eq!(chord_label("secondary-k", true), "⌘ K");
        assert_eq!(chord_label("secondary-alt-up", false), "Ctrl Alt ↑");
        assert_eq!(chord_label("alt-shift-down", true), "⌥ ⇧ ↓");
        assert_eq!(chord_label("shift-pageup", false), "Shift Page Up");
        assert_eq!(chord_label("escape", false), "Esc");
        assert_eq!(chord_label("secondary--", false), "Ctrl -");
        assert_eq!(chord_label("?", false), "?");
    }

    #[test]
    fn every_key_has_a_group_and_the_web_clients_actions_are_there() {
        let keys = keys();
        let named: Vec<&str> = keys.iter().map(|k| k.id.as_ref()).collect();
        for id in [
            "search",
            "previous",
            "next",
            "previous-unread",
            "next-unread",
            "previous-network",
            "next-network",
            "mark-read",
            "mark-network-read",
            "focus-composer",
            "gifs",
            "toggle-members",
            "inbox",
            "saved",
            "join",
            "settings",
            "pause-theme",
            "help",
        ] {
            assert!(named.contains(&id), "{id} is missing");
            assert_ne!(group_of(id), Group::Other, "{id} has no group");
        }
        // The other five of the web client's 23 are R1's and C1's.
        for id in [
            "jump-to-unread",
            "jump-to-present",
            "format-bold",
            "format-italic",
            "format-underline",
        ] {
            assert!(!named.contains(&id), "{id} belongs to its own package");
            assert_ne!(group_of(id), Group::Other, "{id} has no group");
        }
        assert_eq!(
            named.iter().filter(|id| id.starts_with("channel-")).count(),
            9
        );
    }
}

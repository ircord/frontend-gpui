//! Settings → Shortcuts (package S2): every registered key by group, as the
//! web client's `ShortcutList.vue` in its editable form. Each action shows
//! its keys; a key is removed with its ×, Add key records the next key you
//! press (Esc cancels), a key another action uses asks whether to move it,
//! and an action you changed can be reset, or all of them at once.
//!
//! Your keys are kept in the account's preferences (`shortcuts`), under the
//! web client's action names, recording only the actions you changed, so
//! new defaults still reach you. Whenever the preferences change, the keys
//! are bound again ([`apply`]): the old keys are silenced with GPUI's
//! `NoAction` and the new ones bound after them, and
//! `registry::BoundKeys` tells the help (N1) what is bound now.

use crate::ui::focus::Reachable;
use std::collections::{BTreeMap, HashMap};
use std::rc::{Rc, Weak};

use gpui_kit::TestSupportExt;
use gpui_kit::prelude::*;
use gpui_kit::*;
use ircord_client::Client;
use ircord_client::contract::Shortcut;
use ircord_client::documents::PREFERENCES;
use ircord_client::store::{Workspace, topic};

use crate::features::shortcuts::{Group, chord_label, group_of};
use crate::registry::{self, BoundKeys, KeyDefault, SettingsCtx, SettingsRow, SettingsSection};
use crate::session::{Session, SessionEvent};
use crate::theme::{self, Theme};
use crate::ui;

const MAC: bool = cfg!(target_os = "macos");
/// The server keeps at most this many keys per action.
pub const MAX_KEYS: usize = 3;

pub fn init(cx: &mut App) {
    registry::register(cx, |r| {
        r.settings_sections.add(
            SettingsSection::new("shortcuts", t!("settings-messages-shortcuts-title"), render)
                .order(70)
                .keywords(t!("settings-messages-shortcuts-keywords").split(", "))
                .row(
                    SettingsRow::new("shortcut-list", t!("settings-messages-shortcuts-list"))
                        .description(t!("settings-messages-shortcuts-list-help"))
                        .keywords(t!("settings-messages-shortcuts-keywords")),
                )
                .row(
                    SettingsRow::new(
                        "shortcuts-reset",
                        t!("settings-messages-shortcuts-reset-all"),
                    )
                    .keywords(t!("settings-messages-shortcuts-reset-keywords")),
                ),
        );
    });
    // Bind the person's keys whenever a session's preferences change.
    cx.observe_new::<Session>(|_, _, cx| {
        cx.subscribe_self(|session: &mut Session, event: &SessionEvent, cx| {
            let refresh = match event {
                SessionEvent::Changed(changes) => {
                    changes.reset || changes.topics.contains_key(topic::DOCUMENTS)
                }
                SessionEvent::Phase => true,
                _ => false,
            };
            if refresh {
                let client = session.client().cloned();
                if let Some(client) = &client {
                    watch(client, cx);
                }
                let overrides = client.map(|c| overrides(&c.read())).unwrap_or_default();
                apply(&overrides, cx);
            }
        })
        .detach();
    })
    .detach();
}

/// The client whose preferences are watched, so each client is watched once.
#[derive(Default)]
struct Watched(Option<Weak<Client>>);
impl Global for Watched {}

fn watch(client: &Rc<Client>, cx: &mut App) {
    let watched = cx.default_global::<Watched>();
    if watched
        .0
        .as_ref()
        .and_then(Weak::upgrade)
        .is_some_and(|c| Rc::ptr_eq(&c, client))
    {
        return;
    }
    watched.0 = Some(Rc::downgrade(client));
    client.watch_document(&PREFERENCES);
}

// ---- Chords ----------------------------------------------------------------

/// GPUI key names and the web client's (`KeyboardEvent.key`) for the keys
/// that are not one character.
const NAMES: [(&str, &str); 14] = [
    ("up", "ArrowUp"),
    ("down", "ArrowDown"),
    ("left", "ArrowLeft"),
    ("right", "ArrowRight"),
    ("pageup", "PageUp"),
    ("pagedown", "PageDown"),
    ("escape", "Escape"),
    ("enter", "Enter"),
    ("tab", "Tab"),
    ("backspace", "Backspace"),
    ("delete", "Delete"),
    ("home", "Home"),
    ("end", "End"),
    ("insert", "Insert"),
];

/// One chord as the web client stores it, normalised so equal chords
/// compare equal.
pub fn chord(key: &str, ctrl: bool, alt: bool, shift: bool) -> Shortcut {
    let key = if key.chars().count() == 1 {
        key.to_lowercase()
    } else {
        key.to_string()
    };
    Shortcut {
        key,
        r#mod: ctrl.then_some(true),
        alt: alt.then_some(true),
        shift: shift.then_some(true),
    }
}

fn normal(s: &Shortcut) -> Shortcut {
    chord(
        &s.key,
        s.r#mod == Some(true),
        s.alt == Some(true),
        s.shift == Some(true),
    )
}

/// A GPUI keystroke (`secondary-alt-up`, `ctrl-k`, `?`) as a stored chord.
pub fn from_keys(keys: &str) -> Option<Shortcut> {
    let stroke = keys.split_whitespace().next()?;
    let (mut ctrl, mut alt, mut shift) = (false, false, false);
    let mut rest = stroke;
    while let Some((modifier, tail)) = rest.split_once('-') {
        if tail.is_empty() {
            break;
        }
        match modifier {
            "secondary" | "ctrl" | "cmd" | "platform" | "super" | "win" => ctrl = true,
            "alt" => alt = true,
            "shift" => shift = true,
            _ => {}
        }
        rest = tail;
    }
    let key = match NAMES.iter().find(|(gpui, _)| *gpui == rest) {
        Some((_, web)) => web.to_string(),
        None if rest == "space" => " ".to_string(),
        None if rest.len() > 1 && rest.starts_with('f') && rest[1..].parse::<u8>().is_ok() => {
            rest.to_uppercase()
        }
        None => rest.to_string(),
    };
    Some(chord(&key, ctrl, alt, shift))
}

/// A stored chord as a GPUI keystroke. The web client's `mod` is Ctrl, or ⌘
/// on a Mac.
pub fn to_keys(s: &Shortcut) -> String {
    let mut out = String::new();
    if s.r#mod == Some(true) {
        out.push_str(if MAC { "cmd-" } else { "ctrl-" });
    }
    if s.alt == Some(true) {
        out.push_str("alt-");
    }
    if s.shift == Some(true) {
        out.push_str("shift-");
    }
    let key = match NAMES.iter().find(|(_, web)| *web == s.key) {
        Some((gpui, _)) => gpui.to_string(),
        None if s.key == " " => "space".to_string(),
        None => s.key.to_lowercase(),
    };
    out.push_str(&key);
    out
}

/// A key you pressed as a chord, or `None` for a modifier on its own.
/// Shift is not kept for punctuation, which many layouts need it to type.
pub fn from_keystroke(k: &Keystroke) -> Option<Shortcut> {
    if k.is_modifier_key() || k.key.is_empty() {
        return None;
    }
    let m = &k.modifiers;
    let ctrl = m.control || m.platform;
    let mut key = k.key.clone();
    let mut shift = m.shift;
    let punctuation = key.chars().count() == 1 && !key.chars().all(char::is_alphanumeric);
    if punctuation || (shift && key.chars().count() == 1 && !key.chars().all(char::is_alphabetic)) {
        if let Some(typed) = k.key_char.as_ref().filter(|c| c.chars().count() == 1)
            && !ctrl
            && !m.alt
        {
            key = typed.clone();
        }
        if !key.chars().all(char::is_alphanumeric) {
            shift = false;
        }
    }
    from_keys(&{
        let mut s = String::new();
        if ctrl {
            s.push_str("secondary-");
        }
        if m.alt {
            s.push_str("alt-");
        }
        if shift {
            s.push_str("shift-");
        }
        s.push_str(&key);
        s
    })
}

/// "Ctrl K" or "⌘ K".
pub fn label(s: &Shortcut) -> String {
    chord_label(&to_keys(s), MAC)
}

// ---- Actions ---------------------------------------------------------------

/// The web client's name for a registered key ID: its stored action name.
pub fn action_name(id: &str) -> String {
    match id {
        "format-bold" => "bold".into(),
        "format-italic" => "italic".into(),
        "format-underline" => "underline".into(),
        // "?" and Ctrl / are one action, help, as in the web client.
        "help-question" => "help".into(),
        _ => {
            let mut out = String::new();
            let mut upper = false;
            for c in id.chars() {
                if c == '-' {
                    upper = true;
                } else if upper {
                    out.extend(c.to_uppercase());
                    upper = false;
                } else {
                    out.push(c);
                }
            }
            out
        }
    }
}

/// One action in the list: its stored name and the registered keys that
/// make it up (two for help).
#[derive(Clone)]
pub struct Action {
    pub name: String,
    pub keys: Vec<KeyDefault>,
}

impl Action {
    pub fn label(&self) -> SharedString {
        self.keys[0].label.clone()
    }

    pub fn defaults(&self) -> Vec<Shortcut> {
        let mut out: Vec<Shortcut> = Vec::new();
        // The question mark first, as the web client lists help.
        let mut keys = self.keys.clone();
        keys.sort_by_key(|k| k.id != "help-question");
        for k in keys {
            if let Some(c) = from_keys(k.keys)
                && !k.keys.trim().is_empty()
                && !out.contains(&c)
            {
                out.push(c);
            }
        }
        out
    }
}

/// Every registered key the settings can change, as actions, in order. The
/// numbered channel keys stay fixed, as in the web client.
pub fn actions(keys: &[KeyDefault]) -> Vec<Action> {
    let mut out: Vec<Action> = Vec::new();
    for k in keys {
        if k.id.starts_with("channel-") {
            continue;
        }
        let name = action_name(&k.id);
        match out.iter_mut().find(|a| a.name == name) {
            Some(a) => a.keys.push(k.clone()),
            None => out.push(Action {
                name,
                keys: vec![k.clone()],
            }),
        }
    }
    out
}

pub type Overrides = BTreeMap<String, Vec<Shortcut>>;

/// The actions you changed, from the preferences.
pub fn overrides(w: &Workspace) -> Overrides {
    w.documents
        .preferences
        .value()
        .and_then(|p| p.preferences.as_ref())
        .and_then(|p| p.shortcuts.as_ref())
        .map(|map| {
            map.iter()
                .filter_map(|(name, list)| {
                    Some((name.clone(), list.as_ref()?.iter().map(normal).collect()))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The keys an action has now.
pub fn effective(action: &Action, overrides: &Overrides) -> Vec<Shortcut> {
    overrides
        .get(&action.name)
        .cloned()
        .unwrap_or_else(|| action.defaults())
}

/// Why a chord cannot be given to an action at all.
pub fn reserved(c: &Shortcut, keys: &[KeyDefault]) -> Option<String> {
    let plain = c.r#mod.is_none() && c.alt.is_none();
    if plain && matches!(c.key.as_str(), "Enter" | "Tab") {
        return Some(t!("settings-messages-shortcuts-reserved-composer").into());
    }
    let channel = keys
        .iter()
        .filter(|k| k.id.starts_with("channel-"))
        .find(|k| from_keys(k.keys).as_ref() == Some(c));
    channel.map(|k| {
        t!(
            "settings-messages-shortcuts-reserved-channel",
            action = k.label.as_ref()
        )
    })
}

/// The actions other than `except` that use `c` now.
pub fn users(c: &Shortcut, except: &str, actions: &[Action], overrides: &Overrides) -> Vec<String> {
    actions
        .iter()
        .filter(|a| a.name != except && effective(a, overrides).contains(c))
        .map(|a| a.name.clone())
        .collect()
}

/// `overrides` with `action` set to `list`, keeping only what differs from
/// the defaults.
pub fn with(overrides: &Overrides, action: &Action, list: Vec<Shortcut>) -> Overrides {
    let mut next = overrides.clone();
    if list == action.defaults() {
        next.remove(&action.name);
    } else {
        next.insert(action.name.clone(), list);
    }
    next
}

/// Give `c` to `action`, taking it from `from` (the actions that used it).
pub fn add(
    overrides: &Overrides,
    actions: &[Action],
    action: &Action,
    c: &Shortcut,
    from: &[String],
) -> Overrides {
    let mut next = overrides.clone();
    for name in from {
        if let Some(other) = actions.iter().find(|a| &a.name == name) {
            let list: Vec<Shortcut> = effective(other, &next)
                .into_iter()
                .filter(|x| x != c)
                .collect();
            next = with(&next, other, list);
        }
    }
    let mut list = effective(action, &next);
    if !list.contains(c) {
        list.push(c.clone());
    }
    list.truncate(MAX_KEYS);
    with(&next, action, list)
}

/// Store `overrides` as the account's shortcuts.
fn save(client: &Client, overrides: &Overrides) {
    let mut next = client
        .read()
        .documents
        .preferences
        .value()
        .cloned()
        .unwrap_or_default();
    let map = overrides
        .iter()
        .map(|(name, list)| (name.clone(), Some(list.clone())))
        .collect();
    next.preferences.get_or_insert_default().shortcuts = Some(map);
    client.edit_document(&PREFERENCES, next);
}

// ---- Binding ---------------------------------------------------------------

/// What [`apply`] bound last, and every key it ever bound, which it silences
/// before binding again.
#[derive(Default)]
struct Applied {
    overrides: Overrides,
    ever: Vec<(String, Option<SharedString>)>,
}
impl Global for Applied {}

fn leak(keys: &str) -> &'static str {
    Box::leak(keys.to_string().into_boxed_str())
}

/// Bind every action's keys: yours where you changed them, the defaults
/// elsewhere.
pub fn apply(overrides: &Overrides, cx: &mut App) {
    let applied = cx.default_global::<Applied>();
    if applied.overrides == *overrides {
        return;
    }
    let mut ever = std::mem::take(&mut applied.ever);
    let keys = registry::get(cx).keys.to_vec();
    let actions = actions(&keys);
    for a in &actions {
        if overrides.contains_key(&a.name) {
            for k in &a.keys {
                if !k.keys.trim().is_empty() {
                    ever.push((k.keys.to_string(), k.context.clone()));
                }
            }
        }
    }
    ever.sort();
    ever.dedup();
    let mut bindings: Vec<KeyBinding> = ever
        .iter()
        .map(|(keys, context)| KeyBinding::new(leak(keys), NoAction, context.as_deref()))
        .collect();
    let mut bound: HashMap<SharedString, Vec<String>> = HashMap::new();
    for a in &actions {
        match overrides.get(&a.name) {
            Some(list) => {
                let first = &a.keys[0];
                let strokes: Vec<String> = list.iter().map(to_keys).collect();
                for s in &strokes {
                    bindings.push(first.binding(s));
                    ever.push((s.clone(), first.context.clone()));
                }
                bound.insert(first.id.clone(), strokes);
                for other in &a.keys[1..] {
                    bound.insert(other.id.clone(), Vec::new());
                }
            }
            None => {
                for k in &a.keys {
                    if !k.keys.trim().is_empty() {
                        bindings.push(k.binding(k.keys));
                    }
                }
            }
        }
    }
    ever.sort();
    ever.dedup();
    let applied = cx.global_mut::<Applied>();
    applied.overrides = overrides.clone();
    applied.ever = ever;
    cx.set_global(BoundKeys(bound));
    cx.bind_keys(bindings);
}

// ---- Recording -------------------------------------------------------------

/// The action getting a new key, while keys are recorded for it, and a key
/// that waits for you to move it or try another.
#[derive(Default)]
struct Recording {
    action: Option<String>,
    problem: Option<Problem>,
    _intercept: Option<Subscription>,
}
impl Global for Recording {}

#[derive(Clone)]
enum Problem {
    /// The chord, why it can't be used, and the action it was for.
    Reserved(Shortcut, String, String),
    /// The chord, the actions that use it, and the action it was for.
    Taken(Shortcut, Vec<String>, String),
}

fn stop(cx: &mut App) {
    let r = cx.default_global::<Recording>();
    r.action = None;
    r._intercept = None;
}

fn start(client: Rc<Client>, action: Action, window: &mut Window, cx: &mut App) {
    let name = action.name.clone();
    let weak = Rc::downgrade(&client);
    let intercept = cx.intercept_keystrokes(move |event, window, cx| {
        let Some(client) = weak.upgrade() else {
            return;
        };
        let k = &event.keystroke;
        if k.is_modifier_key() {
            return;
        }
        cx.stop_propagation();
        if k.key == "escape" && !k.modifiers.modified() {
            stop(cx);
            cx.default_global::<Recording>().problem = None;
            window.refresh();
            return;
        }
        let Some(c) = from_keystroke(k) else {
            return;
        };
        record(&client, &action, c, cx);
        window.refresh();
    });
    let r = cx.default_global::<Recording>();
    r.action = Some(name);
    r.problem = None;
    r._intercept = Some(intercept);
    window.refresh();
}

/// A key pressed while recording for `action`.
fn record(client: &Client, action: &Action, c: Shortcut, cx: &mut App) {
    let keys = registry::get(cx).keys.to_vec();
    let all = actions(&keys);
    let current = overrides(&client.read());
    stop(cx);
    if let Some(reason) = reserved(&c, &keys) {
        cx.default_global::<Recording>().problem =
            Some(Problem::Reserved(c, reason, action.name.clone()));
        return;
    }
    let taken = users(&c, &action.name, &all, &current);
    if !taken.is_empty() {
        cx.default_global::<Recording>().problem =
            Some(Problem::Taken(c, taken, action.name.clone()));
        return;
    }
    cx.default_global::<Recording>().problem = None;
    save(client, &add(&current, &all, action, &c, &[]));
}

// ---- The section -----------------------------------------------------------

fn group_title(group: Group) -> &'static str {
    match group {
        Group::GettingAround => t!("sidebar-help-getting-around"),
        Group::Reading => t!("sidebar-help-reading"),
        Group::Writing => t!("sidebar-help-writing"),
        Group::App => t!("sidebar-help-app"),
        Group::Other => t!("sidebar-help-other"),
    }
}

fn small_button(
    id: String,
    label: impl Into<SharedString>,
    theme: &Theme,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let label: SharedString = label.into();
    let hover = theme.hover;
    div()
        .id(ElementId::Name(id.into()))
        .role(Role::Button)
        .aria_label(label.clone())
        .px_2()
        .py_0p5()
        .rounded_md()
        .text_sm()
        .bg(theme.selected)
        .text_color(theme.text)
        .cursor_pointer()
        .hover(move |d| d.bg(hover))
        .child(label)
        .on_click(on_click)
}

fn render(ctx: &SettingsCtx, _window: &mut Window, cx: &mut App) -> AnyElement {
    let theme = theme::get(cx).clone();
    let Some(client) = ctx.session.read(cx).client().cloned() else {
        return div().id("shortcut-settings").into_any_element();
    };
    watch(&client, cx);
    let keys = registry::get(cx).keys.to_vec();
    let all = actions(&keys);
    let current = overrides(&client.read());
    let (recording, problem) = cx
        .try_global::<Recording>()
        .map(|r| (r.action.clone(), r.problem.clone()))
        .unwrap_or_default();
    let label_of = |name: &str| -> String {
        all.iter()
            .find(|a| a.name == name)
            .map(|a| a.label().to_string())
            .unwrap_or_else(|| name.to_string())
    };

    let mut problem_note: Option<(String, AnyElement)> = None;
    if let Some(problem) = problem {
        let for_action = match &problem {
            Problem::Reserved(_, _, name) | Problem::Taken(_, _, name) => name.clone(),
        };
        let mut note = div()
            .id("shortcut-problem")
            .test_support()
            .role(Role::Alert)
            .flex()
            .flex_wrap()
            .items_center()
            .gap_2()
            .p_2()
            .rounded_md()
            .bg(theme.danger.opacity(0.12))
            .text_sm()
            .text_color(theme.text);
        match problem {
            Problem::Reserved(c, reason, _) => {
                let text = t!(
                    "settings-messages-shortcuts-reserved",
                    chord = label(&c),
                    reason = reason
                );
                note = note
                    .aria_label(SharedString::from(text.clone()))
                    .child(text);
            }
            Problem::Taken(c, from, target) => {
                let names: Vec<String> = from.iter().map(|n| label_of(n)).collect();
                let text = t!(
                    "settings-messages-shortcuts-taken",
                    chord = label(&c),
                    actions = names.join(", ")
                );
                note = note
                    .aria_label(SharedString::from(text.clone()))
                    .child(text);
                if let Some(action) = all.iter().find(|a| a.name == target).cloned() {
                    let (client, all2) = (client.clone(), all.clone());
                    note = note.child(
                        small_button(
                            "shortcut-move".into(),
                            t!(
                                "settings-messages-shortcuts-move",
                                action = action.label().as_ref()
                            ),
                            &theme,
                            move |_, window, cx| {
                                let current = overrides(&client.read());
                                save(&client, &add(&current, &all2, &action, &c, &from));
                                cx.default_global::<Recording>().problem = None;
                                window.refresh();
                            },
                        )
                        .test_support()
                        .reachable(&theme),
                    );
                }
            }
        }
        note = note.child(
            small_button(
                "shortcut-dismiss".into(),
                t!("settings-messages-shortcuts-cancel"),
                &theme,
                |_, window, cx| {
                    cx.default_global::<Recording>().problem = None;
                    window.refresh();
                },
            )
            .test_support()
            .reachable(&theme),
        );
        problem_note = Some((for_action, note.into_any_element()));
    }
    let mut list = div()
        .id("shortcut-settings")
        .test_support()
        .role(Role::List)
        .aria_label(t!("settings-messages-shortcuts-list"))
        .flex()
        .flex_col()
        .gap_3();
    let groups = [
        Group::GettingAround,
        Group::Reading,
        Group::Writing,
        Group::App,
        Group::Other,
    ];
    for group in groups {
        let in_group: Vec<&Action> = all
            .iter()
            .filter(|a| group_of(&a.keys[0].id) == group)
            .collect();
        if in_group.is_empty() {
            continue;
        }
        let mut section = div().flex().flex_col().gap_1().child(
            div()
                .text_xs()
                .font_weight(FontWeight::BOLD)
                .text_color(theme.muted)
                .child(group_title(group).to_uppercase()),
        );
        for a in in_group {
            let keys_now = effective(a, &current);
            let changed = current.contains_key(&a.name);
            let action_label = a.label();
            let mut chips = div().flex().flex_wrap().items_center().gap_1();
            if keys_now.is_empty() {
                chips = chips.child(
                    div()
                        .text_sm()
                        .text_color(theme.faint)
                        .child(t!("sidebar-help-no-key")),
                );
            }
            for (i, c) in keys_now.iter().enumerate() {
                let text = label(c);
                let remove = {
                    let (client, a, c) = (client.clone(), a.clone(), c.clone());
                    move |_: &ClickEvent, _: &mut Window, _: &mut App| {
                        let current = overrides(&client.read());
                        let list = effective(&a, &current)
                            .into_iter()
                            .filter(|x| *x != c)
                            .collect();
                        save(&client, &with(&current, &a, list));
                    }
                };
                chips = chips.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .pl_2()
                        .pr_1()
                        .py_0p5()
                        .rounded_md()
                        .border_1()
                        .border_color(theme.rail)
                        .text_sm()
                        .text_color(theme.text)
                        .child(text.clone())
                        .child(
                            div()
                                .id(ElementId::Name(
                                    format!("shortcut-remove-{}-{i}", a.name).into(),
                                ))
                                .test_support()
                                .role(Role::Button)
                                .reachable(&theme)
                                .aria_label(t!(
                                    "settings-messages-shortcuts-remove",
                                    chord = text.as_str(),
                                    action = action_label.as_ref()
                                ))
                                .px_1()
                                .rounded_sm()
                                .text_color(theme.muted)
                                .cursor_pointer()
                                .hover({
                                    let hover = theme.hover;
                                    move |d| d.bg(hover)
                                })
                                .child("×")
                                .on_click(remove),
                        ),
                );
            }
            let is_recording = recording.as_deref() == Some(a.name.as_str());
            let mut controls = div().flex().items_center().gap_1();
            if is_recording {
                controls = controls.child(
                    div()
                        .id(ElementId::Name(
                            format!("shortcut-recording-{}", a.name).into(),
                        ))
                        .test_support()
                        .role(Role::Status)
                        .aria_label(t!(
                            "settings-messages-shortcuts-press",
                            action = action_label.as_ref()
                        ))
                        .text_sm()
                        .text_color(theme.accent)
                        .child(t!(
                            "settings-messages-shortcuts-press",
                            action = action_label.as_ref()
                        )),
                );
                controls = controls.child(
                    small_button(
                        format!("shortcut-cancel-{}", a.name),
                        t!("settings-messages-shortcuts-cancel"),
                        &theme,
                        |_, window, cx| {
                            stop(cx);
                            window.refresh();
                        },
                    )
                    .test_support()
                    .reachable(&theme),
                );
            } else if keys_now.len() < MAX_KEYS {
                let (client, a2) = (client.clone(), a.clone());
                controls = controls.child(
                    small_button(
                        format!("shortcut-add-{}", a.name),
                        t!("settings-messages-shortcuts-add"),
                        &theme,
                        move |_, window, cx| start(client.clone(), a2.clone(), window, cx),
                    )
                    .test_support()
                    .reachable(&theme),
                );
            }
            if changed {
                let (client, a2) = (client.clone(), a.clone());
                controls = controls.child(
                    small_button(
                        format!("shortcut-reset-{}", a.name),
                        t!("settings-messages-shortcuts-reset"),
                        &theme,
                        move |_, _, _| {
                            let mut current = overrides(&client.read());
                            current.remove(&a2.name);
                            save(&client, &current);
                        },
                    )
                    .test_support()
                    .reachable(&theme),
                );
            }
            section = section.child(
                div()
                    .id(ElementId::Name(format!("shortcut-{}", a.name).into()))
                    .test_support()
                    .role(Role::ListItem)
                    .aria_label(SharedString::from(format!(
                        "{action_label}: {}",
                        if keys_now.is_empty() {
                            t!("sidebar-help-no-key").to_string()
                        } else {
                            keys_now.iter().map(label).collect::<Vec<_>>().join(", ")
                        }
                    )))
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_3()
                    .py_0p5()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_color(theme.text)
                            .child(action_label.clone()),
                    )
                    .child(chips)
                    .child(controls),
            );
            if problem_note
                .as_ref()
                .is_some_and(|(name, _)| *name == a.name)
                && let Some((_, note)) = problem_note.take()
            {
                section = section.child(note);
            }
        }
        list = list.child(section);
    }
    let block = super::block(
        ctx,
        "shortcut-list",
        t!("settings-messages-shortcuts-list"),
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .text_sm()
                    .text_color(theme.muted)
                    .child(t!("settings-messages-shortcuts-list-help")),
            )
            .child(list),
        &theme,
    );
    let reset_client = client.clone();
    div()
        .id("shortcut-settings-page")
        .flex()
        .flex_col()
        .gap_3()
        .child(block)
        .child(super::row(
            ctx,
            "shortcuts-reset",
            t!("settings-messages-shortcuts-reset-all"),
            None,
            small_button(
                "shortcuts-reset-all".into(),
                t!("settings-messages-shortcuts-reset-all"),
                &theme,
                move |_, window, cx| {
                    save(&reset_client, &Overrides::new());
                    ui::toast::info(window, cx, t!("settings-messages-shortcuts-defaults-again"));
                },
            )
            .test_support()
            .reachable(&theme),
            &theme,
        ))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::{
        Overrides, Shortcut, action_name, actions, add, chord, from_keys, normal, reserved,
        to_keys, users,
    };
    use crate::registry::KeyDefault;

    fn key(id: &'static str, keys: &'static str) -> KeyDefault {
        gpui_kit::actions!(test, [Nothing]);
        KeyDefault::new(id, id, keys, Some("Root"), Nothing)
    }

    #[test]
    fn chords_go_both_ways_like_the_web_client_stores_them() {
        let c = from_keys("secondary-alt-up").unwrap();
        assert_eq!(c, chord("ArrowUp", true, true, false));
        assert_eq!(
            to_keys(&c),
            if cfg!(target_os = "macos") {
                "cmd-alt-up"
            } else {
                "ctrl-alt-up"
            }
        );
        assert_eq!(from_keys("ctrl-k"), from_keys("secondary-k"));
        assert_eq!(from_keys("shift-f8").unwrap().key, "F8");
        assert_eq!(from_keys("secondary--").unwrap().key, "-");
        assert_eq!(to_keys(&from_keys("escape").unwrap()), "escape");
        assert_eq!(from_keys("?").unwrap(), chord("?", false, false, false));
        let stored: Shortcut =
            serde_json::from_value(serde_json::json!({"key": "K", "mod": true})).unwrap();
        assert_eq!(normal(&stored), from_keys("ctrl-k").unwrap());
    }

    #[test]
    fn action_names_are_the_web_clients() {
        assert_eq!(action_name("previous-unread"), "previousUnread");
        assert_eq!(action_name("format-bold"), "bold");
        assert_eq!(action_name("help-question"), "help");
        assert_eq!(action_name("search"), "search");
    }

    #[test]
    fn help_is_one_action_and_channel_keys_stay_fixed() {
        let keys = [
            key("help", "secondary-/"),
            key("help-question", "?"),
            key("channel-1", "secondary-1"),
            key("inbox", ""),
        ];
        let all = actions(&keys);
        let names: Vec<&str> = all.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, ["help", "inbox"]);
        assert_eq!(
            all[0].defaults(),
            [from_keys("?").unwrap(), from_keys("secondary-/").unwrap()]
        );
        assert!(all[1].defaults().is_empty());
        assert!(reserved(&from_keys("ctrl-1").unwrap(), &keys).is_some());
        assert!(reserved(&from_keys("enter").unwrap(), &keys).is_some());
        assert!(reserved(&from_keys("ctrl-enter").unwrap(), &keys).is_none());
    }

    #[test]
    fn moving_a_key_takes_it_from_the_others_and_defaults_are_not_stored() {
        let keys = [key("next", "alt-down"), key("inbox", "")];
        let all = actions(&keys);
        let down = from_keys("alt-down").unwrap();
        let none = Overrides::new();
        assert_eq!(users(&down, "inbox", &all, &none), ["next"]);
        let moved = add(&none, &all, &all[1], &down, &["next".into()]);
        assert_eq!(moved.get("next"), Some(&Vec::new()));
        assert_eq!(moved.get("inbox"), Some(&vec![down.clone()]));
        // Giving it back to next leaves inbox empty, its default: not stored.
        let back = add(&moved, &all, &all[0], &down, &["inbox".into()]);
        assert!(back.is_empty(), "{back:?}");
    }
}

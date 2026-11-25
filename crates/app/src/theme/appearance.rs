//! Keeps the account's appearance applied: reads this device's copy when a
//! session starts, watches the `appearance` and `themes` documents once
//! signed in, follows the system's light or dark, and recompiles the
//! [`Theme`](super::Theme) (and gpui-kit's own theme, the window's rem size
//! and the motion switch) whenever any of them changes.
//!
//! Settings edit through [`edit`]: with sync on the edit goes to the account
//! (and so to every device), with it off it stays on this device. Package S2
//! edits its message settings in the same document the same way.

use std::collections::HashSet;
use std::rc::Rc;

use gpui_kit::base::Root;
use gpui_kit::component::{Theme as KitTheme, ThemeMode};
use gpui_kit::*;
use ircord_client::Client;
use ircord_client::config::ConfigDir;
use ircord_client::contract::{Appearance, StoredAppearance, Theme as UserTheme, Themes};
use ircord_client::documents::{APPEARANCE, THEMES};
use ircord_client::store::appearance::{self as look, Compiled, Device, Look, Mode};
use ircord_client::store::documents::document_id;
use ircord_client::store::topic;

use crate::session::{Session, SessionEvent};

/// The device state section this module keeps.
const SECTION: &str = "appearance";

#[derive(Default)]
struct State {
    device: Device,
    config: Option<ConfigDir>,
    session: Option<WeakEntity<Session>>,
    system: Option<Mode>,
    /// Clients whose documents are being watched.
    watched: HashSet<usize>,
    compiled: Option<Compiled>,
    _session: Option<Subscription>,
}

impl Global for State {}

pub(super) fn init(cx: &mut App) {
    if cx.has_global::<State>() {
        return;
    }
    cx.set_global(State::default());
    cx.observe_new::<Session>(|_, _, cx| {
        let session = cx.entity();
        cx.defer(move |cx| attach(session, cx));
    })
    .detach();
    cx.observe_new::<Root>(|_, window, cx| {
        let Some(window) = window else {
            return;
        };
        set_rem_size(window, cx);
        cx.observe_window_appearance(window, |_, window, cx| {
            system_changed(mode_of(window.appearance()), cx);
        })
        .detach();
        let system = mode_of(window.appearance());
        cx.defer(move |cx| system_changed(system, cx));
    })
    .detach();
    apply(cx);
}

fn mode_of(appearance: WindowAppearance) -> Mode {
    match appearance {
        WindowAppearance::Dark | WindowAppearance::VibrantDark => Mode::Dark,
        _ => Mode::Light,
    }
}

fn system_changed(mode: Mode, cx: &mut App) {
    let state = cx.global_mut::<State>();
    if state.system != Some(mode) {
        state.system = Some(mode);
        apply(cx);
    }
}

/// A session started: show this device's copy and follow the account.
fn attach(session: Entity<Session>, cx: &mut App) {
    let config = session.read(cx).config.clone();
    let device = config.section::<Device>(SECTION);
    let subscription = cx.subscribe(&session, |session, event: &SessionEvent, cx| {
        on_session(&session, event, cx)
    });
    let state = cx.global_mut::<State>();
    state.device = device;
    state.config = Some(config);
    state.session = Some(session.downgrade());
    state._session = Some(subscription);
    watch(&session, cx);
    apply(cx);
}

fn on_session(session: &Entity<Session>, event: &SessionEvent, cx: &mut App) {
    match event {
        SessionEvent::Phase => {
            watch(session, cx);
            apply(cx);
        }
        SessionEvent::Changed(changes) => {
            if changes.reset {
                watch(session, cx);
            }
            let appearance = changes.touches_id(topic::DOCUMENTS, document_id(APPEARANCE.document));
            let themes = changes.touches_id(topic::DOCUMENTS, document_id(THEMES.document));
            if appearance || themes || changes.reset {
                follow(cx);
                apply(cx);
            }
        }
        _ => {}
    }
}

/// Read the appearance and the theme library, once per client.
fn watch(session: &Entity<Session>, cx: &mut App) {
    let Some(client) = session.read(cx).client().cloned() else {
        return;
    };
    let key = Rc::as_ptr(&client) as usize;
    if cx.global_mut::<State>().watched.insert(key) {
        client.watch_document(&APPEARANCE);
        client.watch_document(&THEMES);
    }
}

/// With sync on, remember the account's copy for the next start.
fn follow(cx: &mut App) {
    let account = account(cx);
    let state = cx.global_mut::<State>();
    if state.device.follow(account.as_ref()) {
        save_device(cx);
    }
}

fn save_device(cx: &App) {
    let state = cx.global::<State>();
    if let Some(config) = &state.config
        && let Err(e) = config.save_section(SECTION, &state.device)
    {
        log::warn!("could not save the appearance on this device: {e}");
    }
}

fn client(cx: &App) -> Option<Rc<Client>> {
    let session = cx.try_global::<State>()?.session.as_ref()?.upgrade()?;
    session.read(cx).client().cloned()
}

/// The account's stored copy, once read.
fn account(cx: &App) -> Option<StoredAppearance> {
    let client = client(cx)?;
    let w = client.read();
    w.documents.appearance.server().cloned()
}

/// The appearance shown: the account's with sync on, this device's with it off.
pub fn current(cx: &App) -> Appearance {
    let Some(state) = cx.try_global::<State>() else {
        return Appearance::default();
    };
    let shown = client(cx).and_then(|c| c.read().documents.appearance.value().cloned());
    state.device.effective(shown.as_ref())
}

/// The interface settings shown, cleaned.
pub fn look(cx: &App) -> Look {
    Look::of(&current(cx))
}

/// The account's personal themes.
pub fn themes(cx: &App) -> Vec<UserTheme> {
    library(cx).map(|l| l.themes).unwrap_or_default()
}

/// The account's theme library, once read.
pub fn library(cx: &App) -> Option<Themes> {
    let client = client(cx)?;
    let w = client.read();
    w.documents.themes.value().cloned()
}

/// Whether this device follows the account's appearance.
pub fn sync(cx: &App) -> bool {
    cx.try_global::<State>().is_none_or(|s| s.device.sync)
}

/// The appearance as drawn now.
pub fn compiled(cx: &App) -> Compiled {
    cx.try_global::<State>()
        .and_then(|s| s.compiled.clone())
        .unwrap_or_else(|| look::compile(&Look::default(), Mode::Dark, &[], false))
}

/// The light or dark the system asks for.
pub fn system_mode(cx: &App) -> Mode {
    cx.try_global::<State>()
        .and_then(|s| s.system)
        .unwrap_or_else(|| mode_of(cx.window_appearance()))
}

/// Change the appearance. Returns false when nothing changed.
pub fn edit(cx: &mut App, change: impl FnOnce(&mut Appearance)) -> bool {
    let before = current(cx);
    let mut next = before.clone();
    change(&mut next);
    if next == before {
        return false;
    }
    let client = client(cx);
    let state = cx.global_mut::<State>();
    state.device.settings = Some(next.clone());
    let to_account = state.device.sync && client.is_some();
    save_device(cx);
    match client.filter(|_| to_account) {
        Some(client) => {
            let updated_at = client
                .read()
                .documents
                .appearance
                .server()
                .and_then(|s| s.updated_at.clone());
            client.edit_document(
                &APPEARANCE,
                StoredAppearance {
                    settings: Some(next),
                    updated_at,
                },
            );
        }
        None => crate::settings::saved(cx),
    }
    apply(cx);
    true
}

/// Follow the account's appearance on this device, or keep this device's own.
pub fn set_sync(cx: &mut App, on: bool) {
    let shown = current(cx);
    let state = cx.global_mut::<State>();
    if state.device.sync == on {
        return;
    }
    state.device.sync = on;
    // Turning sync off keeps the look shown; turning it on adopts the account's.
    state.device.settings = Some(shown);
    save_device(cx);
    if on {
        follow(cx);
    }
    crate::settings::saved(cx);
    apply(cx);
}

/// Store the account's theme library as the server answered it.
pub fn set_library(themes: Themes, cx: &mut App) {
    if let Some(client) = client(cx) {
        client.change(|w| {
            w.documents.themes.read_done(Ok(themes));
            ircord_client::store::Changes::topic_id(topic::DOCUMENTS, document_id(THEMES.document))
        });
    }
}

/// Whether the server lets personal themes use their CSS (an absent
/// capability allows it, as the backends before it did).
pub fn server_allows_css(cx: &App) -> bool {
    client(cx).is_none_or(|c| {
        let w = c.read();
        w.capabilities
            .themes
            .as_ref()
            .is_none_or(|themes| themes.custom_css)
    })
}

/// Whether a personal theme's CSS is applied: the account's switch is on and
/// the server allows CSS.
fn css_allowed(cx: &App) -> bool {
    library(cx).is_some_and(|l| l.custom_css) && server_allows_css(cx)
}

/// Switch the account's use of its themes' custom CSS.
pub fn set_custom_css(cx: &mut App, on: bool) {
    let (Some(client), Some(mut library)) = (client(cx), library(cx)) else {
        return;
    };
    if library.custom_css == on {
        return;
    }
    library.custom_css = on;
    client.edit_document(&THEMES, library);
    crate::settings::saved(cx);
    apply(cx);
}

/// Recompile and, when anything changed, apply to every window.
pub fn apply(cx: &mut App) {
    let look = look(cx);
    let mode = look.resolve_mode(system_mode(cx));
    let compiled = look::compile_with_css(&look, mode, &themes(cx), false, css_allowed(cx));
    super::backdrop::sync(compiled.backdrop.as_ref(), client(cx), cx);
    let state = cx.global_mut::<State>();
    if state.compiled.as_ref() == Some(&compiled) {
        return;
    }
    state.compiled = Some(compiled.clone());
    let mut theme = super::Theme::from_compiled(&compiled);
    apply_css_fonts(&mut theme, &compiled, cx);
    cx.set_reduce_motion(theme.reduced_motion);
    let runtime = state_runtime(cx);
    if !super::fade::start(theme.clone(), runtime, show, cx) {
        show(theme, cx);
    }
    for window in cx.windows() {
        let _ = window.update(cx, |_, window, cx| set_rem_size(window, cx));
    }
}

/// One step of a change: the kit's controls and the tokens, in these colours.
/// A fade calls it each frame; its first frame already has the new fonts,
/// corners and scale.
fn show(theme: super::Theme, cx: &mut App) {
    apply_kit(&theme, cx);
    cx.set_global(theme);
    cx.refresh_windows();
}

/// The runtime that paces a fade: the signed-in session's.
fn state_runtime(cx: &App) -> Option<std::sync::Arc<tokio::runtime::Runtime>> {
    let session = cx.try_global::<State>()?.session.as_ref()?.upgrade()?;
    Some(session.read(cx).runtime.clone())
}

/// Generic family names end a theme's font list: the system's own font stays.
const GENERIC_FAMILIES: [&str; 5] = [
    "sans-serif",
    "system-ui",
    "ui-sans-serif",
    "serif",
    "monospace",
];

/// The first family in `listed` the system has, by its own spelling. A
/// generic name ends the list without a match.
fn first_installed(listed: &[String], installed: &[String]) -> Option<SharedString> {
    for family in listed {
        if GENERIC_FAMILIES
            .iter()
            .any(|g| g.eq_ignore_ascii_case(family))
        {
            return None;
        }
        if let Some(name) = installed.iter().find(|n| n.eq_ignore_ascii_case(family)) {
            return Some(name.clone().into());
        }
    }
    None
}

/// The fonts and sizes a theme's CSS sets (`--font-ui`, `--font-mono`,
/// `--font-message`, `--message-size`, `--group-gap`) over the compiled look.
fn apply_css_fonts(theme: &mut super::Theme, compiled: &Compiled, cx: &App) {
    let Some(css) = &compiled.css else {
        return;
    };
    let installed = cx.text_system().all_font_names();
    if let Some(font) = first_installed(&css.font_ui, &installed) {
        theme.font = font;
    }
    if let Some(font) = first_installed(&css.font_mono, &installed) {
        theme.mono_font = font;
    }
    theme.message_font = first_installed(&css.font_message, &installed);
    theme.message_size = css.message_size;
    theme.group_gap = css.group_gap;
}

fn set_rem_size(window: &mut Window, cx: &App) {
    window.set_rem_size(px(16. * super::get(cx).scale));
}

/// gpui-kit's own controls (fields, buttons, menus, switches) in the same
/// colours, font, corners and motion.
fn apply_kit(theme: &super::Theme, cx: &mut App) {
    if !cx.has_global::<KitTheme>() {
        return;
    }
    let mode = if theme.dark {
        ThemeMode::Dark
    } else {
        ThemeMode::Light
    };
    if KitTheme::global(cx).mode != mode {
        KitTheme::change(mode, None, cx);
    }
    let t = theme.clone();
    KitTheme::update(cx, move |k| {
        k.font_family = t.font.clone();
        k.mono_font_family = t.mono_font.clone();
        k.radius = t.radius;
        k.radius_lg = t.radius_large;
        if t.reduced_motion {
            k.motion.duration_fast = std::time::Duration::ZERO;
            k.motion.duration_normal = std::time::Duration::ZERO;
            k.motion.duration_slow = std::time::Duration::ZERO;
        } else {
            k.motion = Default::default();
        }
        let c = &mut k.colors;
        c.background = t.main;
        c.foreground = t.text;
        c.border = t.border;
        c.input = t.control_border;
        c.ring = t.accent;
        c.caret = t.text;
        c.selection = t.accent.opacity(0.35);
        c.accent = t.hover;
        c.accent_foreground = t.text;
        c.primary = t.accent;
        c.primary_hover = t.accent.opacity(0.9);
        c.primary_active = t.accent.opacity(0.8);
        c.primary_foreground = t.on_accent;
        // Buttons read their own fields, not `primary`: without these a
        // primary button keeps gpui-kit's near-black instead of the accent.
        c.button_primary = t.accent;
        c.button_primary_hover = t.accent.opacity(0.9);
        c.button_primary_active = t.accent.opacity(0.8);
        c.button_primary_foreground = t.on_accent;
        c.secondary = t.raised;
        c.secondary_hover = t.hover;
        c.secondary_active = t.selected;
        c.secondary_foreground = t.text;
        c.muted = t.raised;
        c.muted_foreground = t.muted;
        c.popover = t.panel;
        c.popover_foreground = t.text;
        c.list = t.main;
        c.list_hover = t.hover;
        c.list_active = t.selected;
        c.list_active_border = t.accent;
        c.link = t.link;
        c.link_hover = t.link;
        c.link_active = t.link;
        c.danger = t.danger;
        c.danger_foreground = t.on_accent;
        c.warning = t.warning;
        c.success = t.online;
        c.info = t.accent;
        c.switch = t.control_border;
        c.scrollbar_thumb = t.border;
        c.scrollbar_thumb_hover = t.control_border;
        c.sidebar = t.sidebar;
        c.sidebar_foreground = t.text;
        c.sidebar_border = t.border;
        c.title_bar = t.rail;
        c.title_bar_border = t.border;
        c.window_border = t.border;
    });
}

//! The sidebar's device layout (widths, the folded list, collapsed sections,
//! the unread filter) and the handles the sidebar, the switcher and the
//! shortcuts share: the session, and the main window for dialogs opened
//! from a key.

use std::collections::BTreeSet;
use std::time::Duration;

use gpui_kit::*;
use ircord_client::documents;
use ircord_client::store::sidebar_order::Layout;

use crate::MainView;
use crate::session::Session;

/// The device state section the layout is saved in.
const SECTION: &str = "sidebar";

#[derive(Default)]
struct Shared {
    session: Option<WeakEntity<Session>>,
    window: Option<AnyWindowHandle>,
    layout: Layout,
    /// Bumped by every change; a save runs only for the newest one.
    generation: u64,
    /// (client address, document) pairs already watched.
    watched: BTreeSet<(usize, usize)>,
}

impl Global for Shared {}

pub fn init(cx: &mut App) {
    cx.default_global::<Shared>();
    cx.observe_new::<Session>(|session: &mut Session, _, cx| {
        let layout = session.config.section::<Layout>(SECTION).sanitized();
        let weak = cx.weak_entity();
        let shared = cx.global_mut::<Shared>();
        shared.session = Some(weak);
        shared.layout = layout;
    })
    .detach();
    cx.observe_new::<MainView>(|_, window, cx| {
        if let Some(window) = window {
            cx.global_mut::<Shared>().window = Some(window.window_handle());
        }
    })
    .detach();
}

/// The session the sidebar belongs to.
pub fn session(cx: &App) -> Option<Entity<Session>> {
    cx.try_global::<Shared>()?.session.as_ref()?.upgrade()
}

/// Run `f` with the main window, after the current update (a key handler
/// runs while the window is busy dispatching it).
pub fn with_window(cx: &mut App, f: impl FnOnce(&mut Window, &mut App) + 'static) {
    let Some(handle) = cx.try_global::<Shared>().and_then(|s| s.window) else {
        return;
    };
    cx.defer(move |cx| {
        handle.update(cx, |_, window, cx| f(window, cx)).ok();
    });
}

pub fn get(cx: &App) -> Layout {
    cx.try_global::<Shared>()
        .map(|s| s.layout.clone())
        .unwrap_or_default()
}

/// Change the layout, redraw, and save it shortly (a drag changes it on
/// every mouse move).
pub fn update(cx: &mut App, f: impl FnOnce(&mut Layout)) {
    let shared = cx.default_global::<Shared>();
    let before = shared.layout.clone();
    f(&mut shared.layout);
    shared.layout = shared.layout.clone().sanitized();
    if shared.layout == before {
        return;
    }
    shared.generation += 1;
    let generation = shared.generation;
    cx.refresh_windows();
    cx.spawn(async move |cx| {
        cx.background_executor()
            .timer(Duration::from_millis(300))
            .await;
        cx.update(|cx| {
            let shared = cx.global::<Shared>();
            if shared.generation != generation {
                return;
            }
            let layout = shared.layout.clone();
            if let Some(session) = session(cx)
                && let Err(e) = session.read(cx).config.save_section(SECTION, &layout)
            {
                log::warn!("save the sidebar layout: {e}");
            }
        });
    })
    .detach();
}

/// Read the preferences (the order and network icons), once per client.
pub fn watch_documents(client: Option<std::rc::Rc<ircord_client::Client>>, cx: &mut App) {
    watch(client, documents::PREFERENCES.document, cx, |c| {
        c.watch_document(&documents::PREFERENCES)
    });
}

/// Read the notification settings (your own levels, mute), once per client.
/// Read when a conversation menu first opens rather than at start, so
/// other features see them unread until something needs them.
pub fn watch_notification_settings(
    client: Option<std::rc::Rc<ircord_client::Client>>,
    cx: &mut App,
) {
    watch(client, documents::NOTIFICATION_SETTINGS.document, cx, |c| {
        c.watch_document(&documents::NOTIFICATION_SETTINGS)
    });
}

fn watch(
    client: Option<std::rc::Rc<ircord_client::Client>>,
    document: ircord_client::contract::event::Document,
    cx: &mut App,
    start: impl FnOnce(&ircord_client::Client),
) {
    let Some(client) = client else {
        return;
    };
    let key = (std::rc::Rc::as_ptr(&client) as usize, document as usize);
    if cx.default_global::<Shared>().watched.insert(key) {
        start(&client);
    }
}

//! Checking for a newer IRCord (package B2): Settings → About's "Check for
//! updates", and the prompt after the first check of a start.
//!
//! Only builds with `IRCORD_UPDATES=1` check at all
//! ([`ircord_platform::update::available`]). The check runs on the session's
//! runtime, off the interface thread. Windows and macOS download and install
//! the update, and macOS then restarts IRCord; Linux opens the release page,
//! since a `.deb` cannot replace itself.

use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::base::Root;
use gpui_kit::component::WindowExt;
use gpui_kit::*;
use ircord_platform::update::{self, Checked, Update, Updater};
use tokio::runtime::Runtime;

use crate::session::Session;

/// How long after a start the first check waits.
const FIRST_CHECK: Duration = Duration::from_secs(10);

/// The updater this build uses, and how long after a start it first checks.
struct Installer {
    updater: Arc<dyn Updater>,
    first_check: Duration,
}
impl Global for Installer {}

/// Where the last check or install stands.
#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    Idle,
    Checking,
    UpToDate,
    Available(Update),
    Installing,
    Failed(String),
}

/// The update state the About page shows. One per session.
pub struct Updates {
    runtime: Arc<Runtime>,
    updater: Arc<dyn Updater>,
    session: WeakEntity<Session>,
    pub status: Status,
}

#[derive(Clone)]
struct Tracked(Entity<Updates>);
impl Global for Tracked {}

pub fn init(cx: &mut App) {
    if update::available() {
        match update::System::new(env!("CARGO_PKG_VERSION")) {
            Ok(system) => use_updater(cx, Arc::new(system), FIRST_CHECK),
            Err(e) => log::warn!("updates are off: {e}"),
        }
    }
    cx.observe_new::<Session>(|session, _, cx| {
        let Some((updater, first_check)) = cx
            .try_global::<Installer>()
            .map(|i| (i.updater.clone(), i.first_check))
        else {
            return;
        };
        let runtime = session.runtime.clone();
        let session_handle = cx.weak_entity();
        let updates = cx.new(|_| Updates {
            runtime: runtime.clone(),
            updater,
            session: session_handle,
            status: Status::Idle,
        });
        cx.set_global(Tracked(updates.clone()));
        // The first check waits on the runtime, not on a GPUI timer, which
        // does not fire in the headless tests.
        cx.spawn(async move |_, cx| {
            // The timer is made inside the runtime: it needs its reactor.
            let _ = runtime
                .spawn(async move { tokio::time::sleep(first_check).await })
                .await;
            cx.update(|cx| check(&updates, cx, true));
        })
        .detach();
    })
    .detach();
}

/// The update state of the running session, once one has started; `None` in a
/// build that does not check for updates.
pub fn tracked(cx: &App) -> Option<Entity<Updates>> {
    cx.try_global::<Tracked>().map(|t| t.0.clone())
}

/// Use `updater` for this build's checks, the first one `first_check` after a
/// start. [`init`] installs the platform's; a test puts a fake in its place
/// before the session starts.
pub fn use_updater(cx: &mut App, updater: Arc<dyn Updater>, first_check: Duration) {
    cx.set_global(Installer {
        updater,
        first_check,
    });
}

/// Look for a newer release now. With `prompt`, a release found is offered
/// in a dialog.
pub fn check(updates: &Entity<Updates>, cx: &mut App, prompt: bool) {
    let (runtime, updater) = {
        let state = updates.read(cx);
        if matches!(state.status, Status::Checking | Status::Installing) {
            return;
        }
        (state.runtime.clone(), state.updater.clone())
    };
    updates.update(cx, |u, cx| {
        u.status = Status::Checking;
        cx.notify();
    });
    let task = runtime.spawn_blocking(move || updater.check());
    let updates = updates.clone();
    cx.spawn(async move |cx| {
        let result = match task.await {
            Ok(Ok(checked)) => Ok(checked),
            Ok(Err(e)) => Err(e.to_string()),
            Err(e) => Err(e.to_string()),
        };
        cx.update(|cx| {
            let found = match &result {
                Ok(Checked::Available(update)) => Some(update.clone()),
                _ => None,
            };
            updates.update(cx, |u, cx| {
                u.status = match result {
                    Ok(Checked::UpToDate) => Status::UpToDate,
                    Ok(Checked::Available(update)) => Status::Available(update),
                    Err(e) => {
                        log::warn!("update check: {e}");
                        Status::Failed(e)
                    }
                };
                cx.notify();
            });
            if let (true, Some(update)) = (prompt, found) {
                // A later task, not this update: the window may be mid-update
                // where this runs.
                let updates = updates.clone();
                cx.spawn(async move |cx| {
                    cx.update(|cx| prompt_install(updates, update, cx));
                })
                .detach();
            }
        });
    })
    .detach();
}

/// Ask whether to install `update` now, in the main window.
fn prompt_install(updates: Entity<Updates>, update: Update, cx: &mut App) {
    // The untyped handle: the typed one leases the root view, which the
    // dialog reads.
    let Some(window) = cx
        .windows()
        .into_iter()
        .find(|handle| handle.downcast::<Root>().is_some())
    else {
        return;
    };
    let version = update.version.to_string();
    let current = env!("CARGO_PKG_VERSION").to_string();
    let (ok, body) = if update::installs() {
        (
            t!("updates-install").to_string(),
            t!(
                "updates-prompt-install",
                version = version,
                current = current
            )
            .to_string(),
        )
    } else {
        (
            t!("updates-open-release").to_string(),
            t!(
                "updates-prompt-release",
                version = version,
                current = current
            )
            .to_string(),
        )
    };
    let title = t!("updates-prompt-title").to_string();
    let later = t!("updates-later").to_string();
    let on_ok = Rc::new(move |cx: &mut App| install(&updates, &update, cx));
    let _ = window.update(cx, |_, window, cx| {
        window.open_alert_dialog(cx, move |alert, _, _| {
            let on_ok = on_ok.clone();
            alert
                .title(title.clone())
                .description(body.clone())
                .ok_text(ok.clone())
                .cancel_text(later.clone())
                .show_cancel(true)
                .on_ok(move |_, _, cx| {
                    on_ok(cx);
                    true
                })
        });
    });
}

/// Save what is unsaved before an install that may end this process: the
/// drafts, and the main window's size.
fn save_state(updates: &Entity<Updates>, cx: &mut App) {
    crate::features::drafts::save_now(cx);
    let session = updates.read(cx).session.upgrade();
    let window = cx
        .windows()
        .into_iter()
        .find(|handle| handle.downcast::<Root>().is_some());
    if let (Some(session), Some(window)) = (session, window)
        && let Ok(size) = window.update(cx, |_, window, _| window.bounds().size)
    {
        session.update(cx, |s, _| {
            s.save_window_size(f32::from(size.width), f32::from(size.height));
        });
    }
}

/// Download and install the update, or on Linux open the release page.
pub fn install(updates: &Entity<Updates>, update: &Update, cx: &mut App) {
    if !update::installs() {
        cx.open_url(update::RELEASES_URL);
        return;
    }
    // Saved from a task of its own: the window must not be updated from the
    // click that started the install.
    let (saving, update) = (updates.clone(), update.clone());
    cx.spawn(async move |cx| {
        cx.update(|cx| save_state(&saving, cx));
        cx.update(|cx| start_install(&saving, &update, cx));
    })
    .detach();
}

fn start_install(updates: &Entity<Updates>, update: &Update, cx: &mut App) {
    let (runtime, updater) = {
        let state = updates.read(cx);
        (state.runtime.clone(), state.updater.clone())
    };
    updates.update(cx, |u, cx| {
        u.status = Status::Installing;
        cx.notify();
    });
    let update = update.clone();
    let task = runtime.spawn_blocking(move || updater.install(&update));
    let updates = updates.clone();
    cx.spawn(async move |cx| {
        let result = match task.await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(e)) => Err(e.to_string()),
            Err(e) => Err(e.to_string()),
        };
        cx.update(|cx| match result {
            // Windows' installer ends this process itself; macOS has replaced
            // the app and starts the new copy once this one quits.
            Ok(()) => cx.quit(),
            Err(e) => {
                log::warn!("update install: {e}");
                updates.update(cx, |u, cx| {
                    u.status = Status::Failed(e);
                    cx.notify();
                });
            }
        });
    })
    .detach();
}

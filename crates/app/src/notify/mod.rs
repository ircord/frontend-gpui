//! Native notifications, driven by the server's routing: the server decides
//! which device alerts (`notification.alert`); this app decides how it shows
//! here, as the web client does (`useNotifications.ts`): a toast, the app's
//! chime, or nothing. Clicking a toast opens its conversation.

pub mod picture;

use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use futures::StreamExt;
use futures::channel::mpsc;
use gpui_kit::prelude::*;
use gpui_kit::*;
use ircord_client::config::{APP_ID, ConfigDir};
use ircord_client::contract::{Alert, AlertDelivery, AlertKind};
use ircord_platform::audio::Chime;
use ircord_platform::toast::{self, Clicked, Identity, Toast};

use crate::media::sound;
use crate::session::Session;

/// Whether the system's own toast plays a sound (Windows and macOS). Linux
/// desktops differ on that, so the app plays its chime there instead.
const SYSTEM_TOAST_SOUND: bool = cfg!(any(windows, target_os = "macos"));

/// The app icon, written once to the config directory for the toast.
const ICON: &[u8] = include_bytes!("../../../../packaging/linux/ircord.png");

/// What one alert does on this device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Plan {
    /// Show a toast; `Some(true)` when the toast carries the system's sound.
    pub toast: Option<bool>,
    /// Play this chime through the app.
    pub chime: Option<Chime>,
}

const NOTHING: Plan = Plan {
    toast: None,
    chime: None,
};

/// What `alert` does, given whether the window is active, whether the person
/// is viewing its conversation, and whether the system's toast plays sound.
/// Pure, so the rules are tested as a table without a window.
pub fn plan(alert: &Alert, window_active: bool, viewing: bool, system_toast_sound: bool) -> Plan {
    if viewing || (alert.delivery == AlertDelivery::Unknown && alert.title.is_empty()) {
        return NOTHING;
    }
    let chime = alert.sound.then(|| chime_for(alert.kind));
    if alert.delivery == AlertDelivery::Sound || window_active {
        return Plan { toast: None, chime };
    }
    if system_toast_sound {
        Plan {
            toast: Some(alert.sound),
            chime: None,
        }
    } else {
        Plan {
            toast: Some(false),
            chime,
        }
    }
}

fn chime_for(kind: AlertKind) -> Chime {
    match kind {
        AlertKind::Mention => Chime::Mention,
        _ => Chime::Message,
    }
}

/// The words of one toast, and the chime its alert would play.
struct Note {
    tag: String,
    title: String,
    body: String,
    chime: Option<Chime>,
}

pub fn show(session: &mut Session, alert: Alert, cx: &mut Context<Session>) {
    let viewing = session.window_active
        && session.conversation.as_deref() == Some(alert.conversation_id.as_str());
    let plan = plan(&alert, session.window_active, viewing, SYSTEM_TOAST_SOUND);
    if plan.toast.is_none() && plan.chime.is_none() {
        return;
    }
    let picture = if plan.toast.is_some() {
        remember(session, &alert);
        Some(picture::resolve(session, &alert, cx))
    } else {
        None
    };
    let note = Note {
        tag: alert.alert_id.clone(),
        // The title names the sender only when previews are on.
        title: if alert.preview {
            alert.title.clone()
        } else {
            t!("core-app-name").to_string()
        },
        body: if alert.preview || !alert.body.is_empty() {
            alert.body.clone()
        } else {
            t!("core-new-message").to_string()
        },
        chime: alert.sound.then(|| chime_for(alert.kind)),
    };
    play(cx, plan, note, picture);
}

/// Which toast a click on `alert_id` opens. Old ones are dropped first.
fn remember(session: &mut Session, alert: &Alert) {
    session
        .alerts
        .insert(alert.alert_id.clone(), alert.conversation_id.clone());
    if session.alerts.len() > 200 {
        // Old toasts are long gone from the notification centre.
        let keep = alert.alert_id.clone();
        session.alerts.retain(|id, _| *id == keep);
        session
            .alerts
            .insert(alert.alert_id.clone(), alert.conversation_id.clone());
    }
}

/// Do what `plan` asks: the chime now, and the toast once its picture is in.
fn play(cx: &mut App, plan: Plan, note: Note, picture: Option<Task<Option<PathBuf>>>) {
    if let Some(kind) = plan.chime {
        sound::chime(kind, cx);
    }
    let Some(with_sound) = plan.toast else {
        return;
    };
    // The chime already played here, so a failed toast plays it only when the
    // plan had none.
    let fallback = if plan.chime.is_none() {
        note.chime
    } else {
        None
    };
    let picture = picture.unwrap_or_else(|| Task::ready(None));
    cx.spawn(async move |cx| {
        let picture = picture.await;
        let toast = Toast {
            tag: note.tag,
            title: note.title,
            body: note.body,
            picture,
            sound: with_sound,
        };
        cx.update(|cx| present(cx, toast, fallback));
    })
    .detach();
}

/// Show one toast on the main thread: to the test sink when one is set, else
/// to the system. If the system refuses, GPUI's own notification shows, with
/// the chime when the alert had one.
fn present(cx: &mut App, toast: Toast, fallback: Option<Chime>) {
    if let Some(sink) = cx.try_global::<Sink>().and_then(|s| s.0.clone()) {
        sink(&toast);
        return;
    }
    let (tag, title, body) = (toast.tag.clone(), toast.title.clone(), toast.body.clone());
    if let Err(e) = toast::show(&identity(), toast, clicked(cx)) {
        log::warn!("could not show the notification: {e}");
        cx.show_system_notification(SystemNotification {
            tag: tag.into(),
            title: title.into(),
            body: body.into(),
            actions: Vec::new(),
        });
        if let Some(kind) = fallback {
            sound::chime(kind, cx);
        }
    }
}

/// Show the settings' test notification, through the same path with sound on.
pub fn send_test(cx: &mut App) {
    let plan = Plan {
        toast: Some(SYSTEM_TOAST_SOUND),
        chime: (!SYSTEM_TOAST_SOUND).then_some(Chime::Mention),
    };
    let note = Note {
        tag: "ircord-test-notification".into(),
        title: t!("core-app-name").to_string(),
        body: t!("notifications-test-body").to_string(),
        chime: Some(Chime::Mention),
    };
    play(cx, plan, note, None);
}

/// A test's stand-in for the system: it receives each toast.
type ToastSink = Rc<dyn Fn(&Toast)>;

/// Where toasts go instead of the system, once set. For tests.
#[derive(Default)]
struct Sink(Option<ToastSink>);
impl Global for Sink {}

/// Send every toast to `sink` and nothing to the system.
pub fn set_sink(cx: &mut App, sink: ToastSink) {
    cx.set_global(Sink(Some(sink)));
}

/// The channel a toast click arrives on, from the platform's own thread.
struct Clicks(mpsc::UnboundedSender<String>);
impl Global for Clicks {}

/// The callback the platform calls with a toast's tag when it is clicked.
fn clicked(cx: &App) -> Clicked {
    let sender = cx.try_global::<Clicks>().map(|c| c.0.clone());
    Arc::new(move |tag| {
        if let Some(sender) = &sender {
            let _ = sender.unbounded_send(tag);
        }
    })
}

/// Report a click on the toast for `tag`, as the platform does. For tests.
pub fn click(cx: &App, tag: &str) {
    if let Some(clicks) = cx.try_global::<Clicks>() {
        let _ = clicks.0.unbounded_send(tag.to_string());
    }
}

/// The app as the toast names it: its id, its name and its icon file.
fn identity() -> Identity {
    Identity {
        id: APP_ID.into(),
        name: t!("core-app-name").to_string(),
        icon: icon_file(),
    }
}

/// The app icon as a file in the config directory, written on first use.
fn icon_file() -> Option<PathBuf> {
    let dir = ConfigDir::platform().ok()?.path().join("notify");
    let path = dir.join("icon.png");
    if !path.exists() {
        std::fs::create_dir_all(&dir).ok()?;
        std::fs::write(&path, ICON).ok()?;
    }
    Some(path)
}

/// Give the process its identity (the Windows AppUserModelID). Without it the
/// platform shows no notifications at all. Call before opening windows.
pub fn identify(cx: &App) {
    cx.set_app_identity(APP_ID, t!("core-app-name"));
}

/// Route notification clicks to the conversation and bring the window up.
/// A toast's click comes through a channel, since the platform calls back
/// from its own thread; GPUI's handler stays for macOS and the fallback.
pub fn install(session: Entity<Session>, cx: &mut App) {
    let (tx, mut rx) = mpsc::unbounded();
    cx.set_global(Clicks(tx));
    let from_toast = session.clone();
    cx.spawn(async move |cx| {
        while let Some(tag) = rx.next().await {
            cx.update(|cx| open(&from_toast, &tag, cx));
        }
    })
    .detach();
    cx.on_system_notification_response(move |response, cx| {
        open(&session, response.tag.as_ref(), cx);
    });
}

/// Select the conversation of the alert `tag` and bring the window up.
fn open(session: &Entity<Session>, tag: &str, cx: &mut App) {
    let conversation = session.read(cx).alerts.get(tag).cloned();
    if let Some(id) = conversation {
        session.update(cx, |s, cx| s.select_conversation(&id, cx));
    }
    crate::show_main_window(session.clone(), cx);
}

#[cfg(test)]
mod tests {
    // Not `super::*`: that glob brings in gpui_kit's `test` macro, which
    // shadows `#[test]`.
    use super::{NOTHING, Plan, plan};
    use AlertDelivery::{Os, Sound, Unknown};
    use AlertKind::{Mention, Message};
    use ircord_client::contract::{Alert, AlertDelivery, AlertKind};
    use ircord_platform::audio::Chime;

    fn alert(delivery: AlertDelivery, titled: bool, sound: bool, kind: AlertKind) -> Alert {
        Alert {
            delivery,
            title: if titled { "bob".into() } else { String::new() },
            sound,
            kind,
            ..Default::default()
        }
    }

    const fn toast(with_sound: bool) -> Plan {
        Plan {
            toast: Some(with_sound),
            chime: None,
        }
    }

    const fn chime(kind: Chime) -> Plan {
        Plan {
            toast: None,
            chime: Some(kind),
        }
    }

    const fn both(with_sound: bool, kind: Chime) -> Plan {
        Plan {
            toast: Some(with_sound),
            chime: Some(kind),
        }
    }

    /// `a` under the given window state, checked against `want`.
    fn expect(a: Alert, window_active: bool, viewing: bool, system: bool, want: Plan) {
        let got = plan(&a, window_active, viewing, system);
        assert_eq!(
            got, want,
            "{a:?} active={window_active} viewing={viewing} system={system}"
        );
    }

    #[test]
    fn viewing_shows_nothing() {
        for delivery in [Os, Sound, Unknown] {
            for sound in [false, true] {
                for system in [false, true] {
                    expect(
                        alert(delivery, true, sound, Mention),
                        true,
                        true,
                        system,
                        NOTHING,
                    );
                    expect(
                        alert(delivery, true, sound, Mention),
                        false,
                        true,
                        system,
                        NOTHING,
                    );
                }
            }
        }
    }

    #[test]
    fn a_sound_delivery_is_a_chime_only() {
        expect(
            alert(Sound, true, true, Mention),
            false,
            false,
            true,
            chime(Chime::Mention),
        );
        expect(
            alert(Sound, true, true, Message),
            true,
            false,
            false,
            chime(Chime::Message),
        );
        expect(
            alert(Sound, true, false, Mention),
            false,
            false,
            true,
            NOTHING,
        );
    }

    #[test]
    fn an_os_delivery_in_use_is_a_chime_only() {
        expect(
            alert(Os, true, true, Message),
            true,
            false,
            true,
            chime(Chime::Message),
        );
        expect(alert(Os, true, false, Message), true, false, false, NOTHING);
    }

    #[test]
    fn an_os_delivery_away_is_a_toast() {
        // Windows and macOS: the toast carries the sound, no chime.
        expect(
            alert(Os, true, true, Mention),
            false,
            false,
            true,
            toast(true),
        );
        expect(
            alert(Os, true, false, Mention),
            false,
            false,
            true,
            toast(false),
        );
        // Linux: a silent toast, and the chime from the app.
        expect(
            alert(Os, true, true, Mention),
            false,
            false,
            false,
            both(false, Chime::Mention),
        );
        expect(
            alert(Os, true, false, Message),
            false,
            false,
            false,
            toast(false),
        );
    }

    #[test]
    fn an_unknown_delivery_is_an_os_delivery_with_a_title() {
        expect(
            alert(Unknown, false, true, Mention),
            false,
            false,
            true,
            NOTHING,
        );
        expect(
            alert(Unknown, true, true, Message),
            true,
            false,
            false,
            chime(Chime::Message),
        );
        expect(
            alert(Unknown, true, true, Mention),
            false,
            false,
            true,
            toast(true),
        );
    }
}

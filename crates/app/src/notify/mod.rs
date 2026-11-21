//! Native notifications, driven by the server's routing: the server decides
//! which device alerts (`notification.alert`); this app only shows it.
//! Clicking one opens its conversation.

pub mod picture;

use gpui_kit::prelude::*;
use gpui_kit::*;
use ircord_client::contract::{Alert, AlertDelivery};

use crate::session::Session;

pub fn show(session: &mut Session, alert: Alert, cx: &mut Context<Session>) {
    // `sound` deliveries ask only for a sound; the OS toast plays its own.
    if alert.delivery == AlertDelivery::Unknown && alert.title.is_empty() {
        return;
    }
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
    let body = if alert.preview || !alert.body.is_empty() {
        alert.body.clone()
    } else {
        t!("core-new-message").to_string()
    };
    cx.show_system_notification(SystemNotification {
        tag: alert.alert_id.into(),
        title: alert.title.into(),
        body: body.into(),
        actions: Vec::new(),
    });
}

/// Give the process its identity (the Windows AppUserModelID). Without it the
/// platform shows no notifications at all. Call before opening windows.
pub fn identify(cx: &App) {
    cx.set_app_identity(ircord_client::config::APP_ID, t!("core-app-name"));
}

/// Route notification clicks to the conversation and bring the window up.
pub fn install(session: Entity<Session>, cx: &mut App) {
    cx.on_system_notification_response(move |response, cx| {
        let conversation = session.read(cx).alerts.get(response.tag.as_ref()).cloned();
        if let Some(id) = conversation {
            session.update(cx, |s, cx| s.select_conversation(&id, cx));
        }
        crate::show_main_window(session.clone(), cx);
    });
}

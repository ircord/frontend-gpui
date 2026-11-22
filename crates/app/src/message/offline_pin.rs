//! Keep a message's attachments on this device past the normal archive window.

use std::time::Duration;

use gpui_kit::prelude::*;
use gpui_kit::*;
use ircord_client::api::OfflinePin;

use crate::registry::MessageCtx;
use crate::ui::icon::IconName;
use crate::{theme, ui};

pub(super) fn control(ctx: &MessageCtx, window: &mut Window, cx: &mut App) -> Option<AnyElement> {
    if ctx.session.read(cx).client()?.read().demo {
        return None;
    }
    let key = ElementId::Name(format!("offline-pin-{}-{}", ctx.conversation, ctx.message).into());
    let context = ctx.clone();
    let control = window.use_keyed_state(key, cx, |_, cx| {
        let mut control = PinControl {
            context,
            status: OfflinePin::default(),
            busy: false,
            poll: None,
        };
        control.refresh(cx);
        control
    });
    Some(control.into_any_element())
}

struct PinControl {
    context: MessageCtx,
    status: OfflinePin,
    busy: bool,
    poll: Option<Task<()>>,
}

impl PinControl {
    fn refresh(&mut self, cx: &mut Context<Self>) {
        let Some(client) = self.context.session.read(cx).client() else {
            return;
        };
        let api = client.api().clone();
        let runtime = self.context.session.read(cx).runtime.clone();
        let target = (
            self.context.conversation.clone(),
            self.context.message.clone(),
        );
        self.poll = Some(cx.spawn(async move |this, cx| {
            loop {
                let (api, target) = (api.clone(), target.clone());
                let result = runtime
                    .spawn(async move { api.offline_pin_status(&target.0, &target.1).await })
                    .await;
                let Ok(status) = result else { return };
                if this
                    .update(cx, |this, cx| {
                        if this.status != status {
                            this.status = status;
                            cx.notify();
                        }
                    })
                    .is_err()
                {
                    return;
                }
                if status.available && (!status.pinned || status.saved) {
                    return;
                }
                let wait = Duration::from_secs(if status.available && !status.failed {
                    1
                } else {
                    5
                });
                let _ = runtime
                    .spawn(async move { tokio::time::sleep(wait).await })
                    .await;
            }
        }));
    }

    fn toggle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy || !self.status.available {
            return;
        }
        let Some(client) = self.context.session.read(cx).client() else {
            return;
        };
        let target = (
            self.context.conversation.clone(),
            self.context.message.clone(),
        );
        let keep = !self.status.pinned;
        let request = client.request(move |api| async move {
            api.set_offline_pin(&target.0, &target.1, keep).await
        });
        self.poll = None;
        self.busy = true;
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let result = request.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.busy = false;
                if let Ok(Ok(status)) = result {
                    this.status = status;
                } else {
                    ui::toast::error(window, cx, t!("uploads-offline-unavailable"));
                }
                this.refresh(cx);
                cx.notify();
            });
        })
        .detach();
    }
}

impl Render for PinControl {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme::get(cx).clone();
        let mut controls = div()
            .flex()
            .items_center()
            .gap_1()
            .text_xs()
            .text_color(theme.muted);
        if self.status.available {
            let action = if self.status.pinned {
                t!("uploads-offline-stop-keeping")
            } else {
                t!("uploads-offline-keep")
            };
            let status =
                if self.busy || (self.status.pinned && !self.status.saved && !self.status.failed) {
                    Some(t!("uploads-offline-saving"))
                } else if self.status.pinned && self.status.failed {
                    Some(t!("uploads-offline-unavailable"))
                } else if self.status.saved {
                    Some(t!("uploads-offline-available"))
                } else {
                    None
                };
            controls = controls
                .child(ui::icon::button(
                    ElementId::Name(format!("offline-pin-toggle-{}", self.context.message).into()),
                    if self.status.pinned {
                        IconName::Check
                    } else {
                        IconName::ArrowDown
                    },
                    action,
                    &theme,
                    cx.listener(|this, _, window, cx| this.toggle(window, cx)),
                ))
                .children(status);
        }
        controls
    }
}

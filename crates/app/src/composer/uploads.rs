//! The upload tray and the attach button (package U1), after
//! `useUploads.ts` and `ComposerUploads.vue` in the web client.
//!
//! Files join a per-conversation batch from the attachment menu's file dialog,
//! by dropping them on the drop zone that shows above the box while files are
//! dragged over the window, or by pasting a picture or copied files into the
//! box. The tray above the box shows each with its progress; removing one
//! that is uploading cancels it. Submitting (Enter, also with an empty box)
//! uploads every file, then sends them as one message whose text is their
//! URLs. A failed batch stays in the tray to send again; files that already
//! uploaded are not uploaded twice.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::TestSupportExt;
use gpui_kit::component::Sizable;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::menu::{DropdownMenu, PopupMenuItem};
use gpui_kit::prelude::*;
use gpui_kit::*;
use ircord_client::Client;
use ircord_client::api::uploads::{self, FileData, Progress, Rejection, UploadError};
use ircord_client::contract as c;

use crate::composer::{ComposerView, expressions::Tab};
use crate::features::{klipy, message_actions, shortcuts::Gifs};
use crate::registry::{self, ComposerCtx, Slot, SubmitHook};
use crate::session::Session;
use crate::theme::{self, Theme};
use crate::ui;
use crate::ui::icon::{IconName, LucideIcon};

/// How often the tray redraws while files upload, for the progress bars.
const PROGRESS_TICK: Duration = Duration::from_millis(150);

/// A small picture of a waiting file.
#[derive(Clone)]
pub enum Preview {
    Path(Arc<Path>),
    Image(Arc<Image>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Status {
    Ready,
    Uploading,
    /// Uploaded and sent; leaves the tray.
    Done,
    Failed(String),
}

/// One file waiting in a conversation's batch.
#[derive(Clone)]
pub struct Pending {
    pub key: u64,
    pub file: FileData,
    pub preview: Option<Preview>,
    pub progress: Progress,
    pub status: Status,
    /// Set once the file uploaded; a retry reuses it.
    pub upload: Option<c::Upload>,
    cancel: Option<tokio::task::AbortHandle>,
    cancelled: bool,
}

/// Every conversation's batch.
#[derive(Default)]
pub struct Uploads {
    queues: HashMap<String, Vec<Pending>>,
    sending: HashSet<String>,
    next: u64,
    /// The composer, to redraw it when an upload changes.
    composer: Option<WeakEntity<ComposerView>>,
    /// How long an upload may stand still; [`uploads::STALL`] outside tests.
    stall: Option<Duration>,
}

impl Global for Uploads {}

impl Uploads {
    /// The files waiting in `conversation`.
    pub fn pending(cx: &App, conversation: &str) -> Vec<Pending> {
        cx.try_global::<Uploads>()
            .and_then(|u| u.queues.get(conversation))
            .cloned()
            .unwrap_or_default()
    }

    pub(super) fn has_pending(cx: &App, conversation: &str) -> bool {
        cx.try_global::<Uploads>()
            .and_then(|u| u.queues.get(conversation))
            .is_some_and(|queue| !queue.is_empty())
    }

    /// Whether any conversation has files waiting (quitting would lose them).
    pub fn any_queued(cx: &App) -> bool {
        cx.try_global::<Uploads>()
            .is_some_and(|u| u.queues.values().any(|q| !q.is_empty()))
    }

    /// Give up on an upload that stands still for `stall` (tests shorten it).
    pub fn set_stall(cx: &mut App, stall: Duration) {
        cx.default_global::<Uploads>().stall = Some(stall);
    }
}

pub fn init(cx: &mut App) {
    cx.default_global::<Uploads>();
    registry::register(cx, |r| {
        r.composer
            .above
            .add(Slot::new("upload-drop", drop_zone).order(-10));
        r.composer
            .above
            .add(Slot::new("upload-tray", tray).order(10));
        r.composer
            .buttons
            .add(Slot::new("upload", attach_button).order(40));
        r.composer
            .on_submit
            .add(SubmitHook::new("uploads", |ctx, sent, window, cx| {
                send_all(ctx, sent.is_empty(), window, cx);
            }));
    });
}

fn client(session: &Entity<Session>, cx: &App) -> Option<Rc<Client>> {
    session.read(cx).client().cloned()
}

fn capability(session: &Entity<Session>, cx: &App) -> Option<c::UploadCapability> {
    client(session, cx)?.read().capabilities.upload.clone()
}

fn refresh(cx: &mut App) {
    let composer = cx
        .try_global::<Uploads>()
        .and_then(|u| u.composer.clone())
        .and_then(|c| c.upgrade());
    if let Some(composer) = composer {
        composer.update(cx, |_, cx| cx.notify());
    }
    cx.refresh_windows();
}

/// Words for a file that cannot join the batch.
fn rejection_text(rejection: &Rejection, name: &str) -> SharedString {
    match rejection {
        Rejection::Unavailable => t!("uploads-server-unavailable").into(),
        Rejection::TooMany => t!("uploads-too-many", count = uploads::MAX_FILES).into(),
        Rejection::Unsupported => t!("uploads-unsupported", name = name).into(),
        Rejection::TooLarge(max) => t!(
            "uploads-too-large",
            name = name,
            size = uploads::human_size(*max as u64)
        )
        .into(),
    }
}

fn failure_text(e: &UploadError) -> String {
    match e {
        UploadError::Interrupted => t!("uploads-interrupted").into(),
        UploadError::Stalled => t!("uploads-stalled").into(),
        UploadError::Refused(e) => e.to_string(),
    }
}

/// Add files to `conversation`'s batch, telling the person about any that
/// cannot join.
pub fn add(
    session: &Entity<Session>,
    conversation: &str,
    files: Vec<(FileData, Option<Preview>)>,
    window: &mut Window,
    cx: &mut App,
) {
    let limits = capability(session, cx);
    let mut refused = Vec::new();
    {
        let state = cx.default_global::<Uploads>();
        for (file, preview) in files {
            let queue = state.queues.entry(conversation.to_string()).or_default();
            match uploads::check(
                limits.as_ref(),
                queue.len(),
                &file.content_type,
                file.size(),
            ) {
                Ok(()) => {
                    state.next += 1;
                    queue.push(Pending {
                        key: state.next,
                        preview: preview.filter(|_| file.is_image()),
                        file,
                        progress: Progress::default(),
                        status: Status::Ready,
                        upload: None,
                        cancel: None,
                        cancelled: false,
                    });
                }
                Err(rejection) => {
                    let stop = matches!(rejection, Rejection::Unavailable | Rejection::TooMany);
                    refused.push(rejection_text(&rejection, &file.name));
                    if stop {
                        break;
                    }
                }
            }
        }
    }
    for text in refused {
        ui::toast::error(window, cx, text);
    }
    refresh(cx);
}

/// Take a file out of the batch. One that is uploading is cancelled; one
/// that uploaded but was never sent is deleted on the server.
pub fn remove(session: &Entity<Session>, conversation: &str, key: u64, cx: &mut App) {
    let item = {
        let state = cx.default_global::<Uploads>();
        let Some(queue) = state.queues.get_mut(conversation) else {
            return;
        };
        let Some(at) = queue.iter().position(|p| p.key == key) else {
            return;
        };
        let mut item = queue.remove(at);
        item.cancelled = true;
        item
    };
    if let Some(cancel) = &item.cancel {
        cancel.abort();
    }
    if let (Some(upload), false) = (&item.upload, item.status == Status::Done)
        && let Some(client) = client(session, cx)
    {
        let id = upload.id.clone();
        drop(client.request(move |api| async move { api.delete_upload(&id).await }));
    }
    refresh(cx);
}

fn update_item(cx: &mut App, conversation: &str, key: u64, f: impl FnOnce(&mut Pending)) {
    if let Some(item) = cx
        .default_global::<Uploads>()
        .queues
        .get_mut(conversation)
        .and_then(|q| q.iter_mut().find(|p| p.key == key))
    {
        f(item);
    }
}

/// Upload everything waiting in `conversation`, then send it as one message.
fn send_all(ctx: &ComposerCtx, files_only: bool, window: &mut Window, cx: &mut App) {
    let session = ctx.session.clone();
    let conversation = ctx.conversation.clone();
    if matches!(
        message_actions::mode(&conversation, cx),
        Some(message_actions::Mode::Edit { .. })
    ) {
        return;
    }
    let Some(client) = client(&session, cx) else {
        return;
    };
    let batch: Vec<u64> = {
        let state = cx.default_global::<Uploads>();
        if state.sending.contains(&conversation) {
            return;
        }
        let Some(queue) = state.queues.get_mut(&conversation) else {
            return;
        };
        let mut keys = Vec::new();
        for item in queue.iter_mut().filter(|p| p.status != Status::Done) {
            item.status = Status::Uploading;
            keys.push(item.key);
        }
        if keys.is_empty() {
            return;
        }
        state.sending.insert(conversation.clone());
        keys
    };
    let reply = if files_only {
        message_actions::take_reply(&conversation, cx)
    } else {
        None
    };
    let network = client
        .read()
        .conversation(&conversation)
        .map(|c| c.network_id.clone());
    let stall = cx
        .try_global::<Uploads>()
        .and_then(|u| u.stall)
        .unwrap_or(uploads::STALL);
    refresh(cx);
    ticker(conversation.clone(), cx);

    window
        .spawn(cx, async move |cx| {
            let Some(network) = network else {
                cx.update(|_, cx| {
                    for key in &batch {
                        update_item(cx, &conversation, *key, |p| {
                            p.status = Status::Failed(t!("uploads-conversation-unavailable").into())
                        });
                    }
                    finish(cx, &conversation);
                })
                .ok();
                return;
            };
            for key in &batch {
                let Ok(Some(item)) = cx.update(|_, cx| {
                    let state = cx.try_global::<Uploads>()?;
                    state
                        .queues
                        .get(&conversation)?
                        .iter()
                        .find(|p| p.key == *key)
                        .cloned()
                }) else {
                    continue;
                };
                if item.upload.is_some() || item.cancelled {
                    continue;
                }
                let (api, network, file, progress) = (
                    client.api().clone(),
                    network.clone(),
                    item.file.clone(),
                    item.progress.clone(),
                );
                let task = client
                    .runtime()
                    .spawn(async move { api.upload_file(&network, &file, &progress, stall).await });
                let abort = task.abort_handle();
                cx.update(|_, cx| update_item(cx, &conversation, *key, |p| p.cancel = Some(abort)))
                    .ok();
                let result = task.await;
                cx.update(|_, cx| {
                    update_item(cx, &conversation, *key, |p| {
                        p.cancel = None;
                        match result {
                            Ok(Ok(upload)) => p.upload = Some(upload),
                            Ok(Err(e)) => p.status = Status::Failed(failure_text(&e)),
                            // Cancelled: the item has left the queue already.
                            Err(_) => {}
                        }
                    });
                })
                .ok();
            }
            // Files removed meanwhile are left out.
            let Ok(files) = cx.update(|_, cx| {
                Uploads::pending(cx, &conversation)
                    .into_iter()
                    .filter(|p| batch.contains(&p.key))
                    .collect::<Vec<_>>()
            }) else {
                return;
            };
            if files.iter().any(|p| matches!(p.status, Status::Failed(_))) {
                cx.update(|window, cx| {
                    failed_elsewhere(&session, &conversation, window, cx);
                    finish(cx, &conversation);
                })
                .ok();
                return;
            }
            let uploaded: Vec<(c::Upload, String)> = files
                .iter()
                .filter_map(|p| p.upload.clone())
                .map(|u| {
                    let url = client.api().file_url(&u.url);
                    (u, url)
                })
                .collect();
            if uploaded.is_empty() {
                cx.update(|_, cx| finish(cx, &conversation)).ok();
                return;
            }
            let body = uploads::batch_message(ircord_client::client::random_id(), &uploaded, reply);
            let target = conversation.clone();
            let answer = client
                .request(move |api| async move { api.send_message(&target, &body).await })
                .await;
            let refused = match answer {
                Ok(Ok(response)) => match response.state {
                    c::CommandResponseState::Failed | c::CommandResponseState::Discarded => Some(
                        response
                            .error
                            .clone()
                            .unwrap_or_else(|| t!("uploads-not-sent").into()),
                    ),
                    _ => {
                        client.change(|w| w.send_answered(&response));
                        None
                    }
                },
                Ok(Err(e)) => Some(e.to_string()),
                Err(e) => Some(e.to_string()),
            };
            cx.update(|window, cx| {
                for p in &files {
                    update_item(cx, &conversation, p.key, |item| {
                        item.status = match &refused {
                            None => Status::Done,
                            Some(reason) => Status::Failed(reason.clone()),
                        }
                    });
                }
                if let Some(reason) = &refused {
                    ui::toast::error(window, cx, reason.clone());
                } else if let Some(queue) =
                    cx.default_global::<Uploads>().queues.get_mut(&conversation)
                {
                    queue.retain(|p| p.status != Status::Done);
                }
                finish(cx, &conversation);
            })
            .ok();
        })
        .detach();
}

/// The batch is over: anything still marked uploading is ready again.
fn finish(cx: &mut App, conversation: &str) {
    let state = cx.default_global::<Uploads>();
    state.sending.remove(conversation);
    if let Some(queue) = state.queues.get_mut(conversation) {
        for item in queue.iter_mut().filter(|p| p.status == Status::Uploading) {
            item.status = Status::Ready;
        }
        if queue.is_empty() {
            state.queues.remove(conversation);
        }
    }
    refresh(cx);
}

/// Some files failed while another conversation is open: say where.
fn failed_elsewhere(
    session: &Entity<Session>,
    conversation: &str,
    window: &mut Window,
    cx: &mut App,
) {
    let s = session.read(cx);
    if s.conversation.as_deref() == Some(conversation) {
        return;
    }
    let Some(name) = s
        .client()
        .and_then(|c| c.read().conversation(conversation).map(|c| c.name.clone()))
    else {
        return;
    };
    ui::toast::error(window, cx, t!("uploads-failed-elsewhere", name = name));
}

/// Redraw while `conversation` is sending, for the progress bars.
fn ticker(conversation: String, cx: &mut App) {
    cx.spawn(async move |cx| {
        loop {
            cx.background_executor().timer(PROGRESS_TICK).await;
            let going = cx.update(|cx| {
                let going = cx
                    .try_global::<Uploads>()
                    .is_some_and(|u| u.sending.contains(&conversation));
                if going {
                    refresh(cx);
                }
                going
            });
            if !going {
                break;
            }
        }
    })
    .detach();
}

/// Read picked or dropped files off the UI thread, then add them.
pub fn add_paths(
    session: Entity<Session>,
    conversation: String,
    paths: Vec<PathBuf>,
    window: &mut Window,
    cx: &mut App,
) {
    let read = cx.background_spawn(async move {
        paths
            .into_iter()
            .filter(|p| p.is_file())
            .map(|path| {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "file".into());
                (path.clone(), name, std::fs::read(&path))
            })
            .collect::<Vec<_>>()
    });
    window
        .spawn(cx, async move |cx| {
            let read = read.await;
            cx.update(|window, cx| {
                let mut files = Vec::new();
                for (path, name, bytes) in read {
                    match bytes {
                        Ok(bytes) => files.push((
                            FileData::named(name, bytes),
                            Some(Preview::Path(Arc::from(path.as_path()))),
                        )),
                        Err(e) => ui::toast::error(
                            window,
                            cx,
                            t!("uploads-read-failed", name = name, reason = e.to_string()),
                        ),
                    }
                }
                add(&session, &conversation, files, window, cx);
            })
            .ok();
        })
        .detach();
}

/// A picture or copied files join the batch when the clipboard has no text.
pub(super) fn paste(
    ctx: &ComposerCtx,
    item: &ClipboardItem,
    window: &mut Window,
    cx: &mut App,
) -> bool {
    let mut images = Vec::new();
    let mut paths = Vec::new();
    for entry in item.entries() {
        match entry {
            ClipboardEntry::String(s) if !s.text().is_empty() => return false,
            ClipboardEntry::Image(image) => images.push(image.clone()),
            ClipboardEntry::ExternalPaths(p) => paths.extend(p.paths().iter().cloned()),
            ClipboardEntry::String(_) => {}
        }
    }
    if images.is_empty() && paths.is_empty() {
        return false;
    }
    let (session, conversation) = (&ctx.session, &ctx.conversation);
    if !images.is_empty() {
        let files = images
            .into_iter()
            .enumerate()
            .map(|(i, image)| {
                let ext = image.format.extension();
                let name = if i == 0 {
                    format!("pasted-image.{ext}")
                } else {
                    format!("pasted-image-{}.{ext}", i + 1)
                };
                let file = FileData::named(name, image.bytes.clone());
                (file, Some(Preview::Image(Arc::new(image))))
            })
            .collect();
        add(session, conversation, files, window, cx);
    }
    if !paths.is_empty() {
        add_paths(session.clone(), conversation.clone(), paths, window, cx);
    }
    true
}

/// Open the file dialog and add what was picked.
fn pick(session: Entity<Session>, conversation: String, window: &mut Window, cx: &mut App) {
    let picked = cx.prompt_for_paths(PathPromptOptions {
        files: true,
        directories: false,
        multiple: true,
        prompt: Some(t!("uploads-pick-prompt").into()),
    });
    window
        .spawn(cx, async move |cx| {
            if let Ok(Ok(Some(paths))) = picked.await {
                cx.update(|window, cx| add_paths(session, conversation, paths, window, cx))
                    .ok();
            }
        })
        .detach();
}

fn remember_composer(ctx: &ComposerCtx, cx: &mut App) {
    cx.default_global::<Uploads>().composer = Some(ctx.composer.downgrade());
}

/// The attachment menu beside the box also opens GIFs and slash commands.
fn attach_button(ctx: &ComposerCtx, _window: &mut Window, cx: &mut App) -> Option<AnyElement> {
    remember_composer(ctx, cx);
    let ctx = ctx.clone();
    Some(
        Button::new("composer-attach")
            .ghost()
            .small()
            .size(px(30.))
            .rounded_full()
            .icon(LucideIcon::Paperclip)
            .accessibility_label(t!("uploads-menu"))
            .tooltip(t!("uploads-menu"))
            .dropdown_menu_with_anchor(Anchor::BottomRight, move |menu, _, cx| {
                let limits = capability(&ctx.session, cx);
                let upload =
                    if let Some(limit) = limits.as_ref().filter(|limit| limit.max_bytes > 0) {
                        t!(
                            "uploads-attach-limit",
                            size = uploads::human_size(limit.max_bytes as u64)
                        )
                        .into()
                    } else {
                        SharedString::from(t!("uploads-attach"))
                    };
                let (session, conversation) = (ctx.session.clone(), ctx.conversation.clone());
                let gif = ctx.clone();
                let composer = ctx.composer.clone();
                menu.item(
                    PopupMenuItem::new(upload)
                        .icon(LucideIcon::Upload)
                        .disabled(limits.is_none())
                        .on_click(move |_, window, cx| {
                            if capability(&session, cx).is_none() {
                                ui::toast::error(window, cx, t!("uploads-server-unavailable"));
                                return;
                            }
                            pick(session.clone(), conversation.clone(), window, cx);
                        }),
                )
                .item(
                    PopupMenuItem::new(t!("uploads-menu-gif"))
                        .icon(LucideIcon::Image)
                        .action(Box::new(Gifs))
                        .on_click(move |_, window, cx| klipy::toggle(&gif, Tab::Gifs, window, cx)),
                )
                .item(
                    PopupMenuItem::new(t!("uploads-menu-command"))
                        .icon(LucideIcon::Slash)
                        .on_click(move |_, window, cx| {
                            let input = composer.read(cx).input.clone();
                            input.update(cx, |input, cx| {
                                input.replace_all("/", window, cx);
                                input.set_selected_range(1..1, cx);
                                input.focus(window, cx);
                            });
                        }),
                )
            })
            .into_any_element(),
    )
}

/// While files are dragged over the window, a zone above the box to drop
/// them on.
fn drop_zone(ctx: &ComposerCtx, _window: &mut Window, cx: &mut App) -> Option<AnyElement> {
    remember_composer(ctx, cx);
    if !cx.has_active_drag() {
        return None;
    }
    let theme = theme::get(cx).clone();
    let (session, conversation) = (ctx.session.clone(), ctx.conversation.clone());
    let accent = theme.accent;
    Some(
        div()
            .id("upload-drop-zone")
            .test_support()
            .role(Role::Group)
            .aria_label(t!("uploads-drop"))
            .mb_2()
            .h(px(96.))
            .rounded_lg()
            .border_2()
            .border_dashed()
            .border_color(theme.rail)
            .bg(theme.sidebar)
            .flex()
            .items_center()
            .justify_center()
            .text_color(theme.muted)
            .child(t!("uploads-drop"))
            .drag_over::<ExternalPaths>(move |style, _, _, _| style.border_color(accent))
            .on_drop(move |paths: &ExternalPaths, window, cx| {
                add_paths(
                    session.clone(),
                    conversation.clone(),
                    paths.paths().to_vec(),
                    window,
                    cx,
                );
            })
            .into_any_element(),
    )
}

/// The files waiting to be sent, above the box.
fn tray(ctx: &ComposerCtx, _window: &mut Window, cx: &mut App) -> Option<AnyElement> {
    remember_composer(ctx, cx);
    let items = Uploads::pending(cx, &ctx.conversation);
    if items.is_empty() {
        return None;
    }
    let theme = theme::get(cx).clone();
    let mut list = div()
        .id("upload-tray")
        .test_support()
        .role(Role::List)
        .aria_label(t!("uploads-tray"))
        .mb_2()
        .p_2()
        .rounded_md()
        .bg(theme.input)
        .flex()
        .gap_2()
        .overflow_x_scroll();
    for item in items {
        list = list.child(tray_item(ctx, &item, &theme));
    }
    Some(list.into_any_element())
}

fn tray_item(ctx: &ComposerCtx, item: &Pending, theme: &Theme) -> AnyElement {
    let name = item.file.name.clone();
    let uploading = item.status == Status::Uploading;
    let fraction = item.progress.fraction(item.file.size());
    let meta: SharedString = match &item.status {
        Status::Failed(reason) => reason.clone().into(),
        Status::Uploading => format!("{:.0}%", fraction * 100.0).into(),
        _ => uploads::human_size(item.file.size()).into(),
    };
    let failed = matches!(item.status, Status::Failed(_));
    let picture = match &item.preview {
        Some(Preview::Path(path)) => img(path.clone())
            .size_full()
            .object_fit(ObjectFit::Cover)
            .into_any_element(),
        Some(Preview::Image(image)) => img(image.clone())
            .size_full()
            .object_fit(ObjectFit::Cover)
            .into_any_element(),
        None => ui::icon::icon(IconName::File, theme.muted).into_any_element(),
    };
    let (session, conversation, key) = (ctx.session.clone(), ctx.conversation.clone(), item.key);
    let remove_label = if uploading {
        t!("uploads-cancel-file", name = name)
    } else {
        t!("uploads-remove-file", name = name)
    };
    div()
        .id(ElementId::Name(format!("upload-item-{key}").into()))
        .test_support()
        .role(Role::ListItem)
        .aria_label(SharedString::from(name.clone()))
        .relative()
        .flex_none()
        .w(px(150.))
        .p_1p5()
        .rounded_md()
        .bg(theme.sidebar)
        .overflow_hidden()
        .flex()
        .flex_col()
        .gap_0p5()
        .child(
            div()
                .w_full()
                .h(px(96.))
                .rounded_md()
                .overflow_hidden()
                .bg(theme.rail)
                .flex()
                .items_center()
                .justify_center()
                .child(picture),
        )
        .child(
            div()
                .text_xs()
                .text_color(theme.text)
                .truncate()
                .child(name),
        )
        .child(
            div()
                .id(ElementId::Name(format!("upload-status-{key}").into()))
                .test_support()
                .aria_label(meta.clone())
                .text_xs()
                .truncate()
                .text_color(if failed { theme.danger } else { theme.faint })
                .child(meta),
        )
        .when(uploading, |d| {
            d.child(
                div()
                    .absolute()
                    .left_0()
                    .bottom_0()
                    .h(px(3.))
                    .w(relative(fraction))
                    .bg(theme.accent),
            )
        })
        .child(
            div()
                .absolute()
                .top(px(8.))
                .right(px(8.))
                .rounded_md()
                .bg(theme.sidebar)
                .child(ui::icon::button(
                    ElementId::Name(format!("upload-remove-{key}").into()),
                    IconName::Delete,
                    remove_label,
                    theme,
                    move |_, _, cx| remove(&session, &conversation, key, cx),
                )),
        )
        .into_any_element()
}

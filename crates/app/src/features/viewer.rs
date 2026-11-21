//! The image viewer (package U1), after `MediaViewer.vue`: a dialog filling
//! the window with one picture of a gallery at a time. Left and Right (or
//! the arrow buttons) page through the gallery, `+`, `-` and `0` zoom, and
//! the toolbar downloads, copies the link or opens the file in the browser.
//! Escape closes it.
//!
//! `init` also installs the upload tray (`composer/uploads.rs`), which has
//! no feature module of its own.

use std::path::PathBuf;
use std::sync::Arc;

use gpui_kit::TestSupportExt;
use gpui_kit::component::WindowExt;
use gpui_kit::prelude::*;
use gpui_kit::*;

use tokio::runtime::Runtime;

use crate::session::Session;
use crate::theme::{self, Theme};
use crate::ui;
use crate::ui::icon::IconName;

/// Files downloaded from the viewer or a message are at most this large.
const MAX_DOWNLOAD: usize = 512 << 20;
const ZOOM_STEP: f32 = 1.25;
const MAX_ZOOM: f32 = 8.0;

pub fn init(cx: &mut App) {
    crate::composer::uploads::init(cx);
}

/// A picture, or another file, as the viewer and the message show it.
#[derive(Debug, Clone, PartialEq)]
pub struct Media {
    pub url: String,
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub content_type: String,
    pub size: Option<i64>,
}

impl Media {
    pub fn is_gif(&self) -> bool {
        self.content_type == "image/gif" || {
            let path = self.url.split(['?', '#']).next().unwrap_or("");
            path.to_ascii_lowercase().ends_with(".gif")
        }
    }

    /// The name to save the file as.
    pub fn file_name(&self) -> String {
        if !self.name.is_empty() {
            return self.name.replace(['/', '\\'], "_");
        }
        url::Url::parse(&self.url)
            .ok()
            .and_then(|u| {
                u.path_segments()
                    .and_then(|mut s| s.next_back().map(str::to_string))
            })
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "download".into())
    }
}

/// Open the viewer on `items[index]`.
pub fn open(
    session: &Entity<Session>,
    items: Vec<Media>,
    index: usize,
    window: &mut Window,
    cx: &mut App,
) {
    if items.is_empty() {
        return;
    }
    let runtime = session.read(cx).runtime.clone();
    let state = cx.new(|cx| Viewer {
        runtime,
        index: index.min(items.len() - 1),
        items,
        zoom: 1.0,
        focus: cx.focus_handle(),
    });
    let focus = state.read(cx).focus.clone();
    let size = window.viewport_size();
    let width = (size.width - px(64.)).max(px(320.));
    window.open_dialog(cx, move |dialog, _, _| {
        dialog
            .width(width)
            .title(t!("uploads-viewer-title"))
            .child(state.clone())
    });
    window.focus(&focus, cx);
}

/// The open viewer's state.
pub struct Viewer {
    runtime: Arc<Runtime>,
    items: Vec<Media>,
    index: usize,
    zoom: f32,
    focus: FocusHandle,
}

impl Viewer {
    fn step(&mut self, by: isize, cx: &mut Context<Self>) {
        let n = self.items.len() as isize;
        self.index = ((self.index as isize + by).rem_euclid(n)) as usize;
        self.zoom = 1.0;
        cx.notify();
    }

    fn zoom(&mut self, to: f32, cx: &mut Context<Self>) {
        self.zoom = to.clamp(1.0, MAX_ZOOM);
        cx.notify();
    }
}

/// Save `media` where the person chooses.
pub fn download(runtime: Arc<Runtime>, media: &Media, window: &mut Window, cx: &mut App) {
    let dir = std::env::home_dir()
        .map(|h| {
            let d = h.join("Downloads");
            if d.is_dir() { d } else { h }
        })
        .unwrap_or_else(|| PathBuf::from("."));
    let target = cx.prompt_for_new_path(&dir, Some(&media.file_name()));
    let url = media.url.clone();
    window
        .spawn(cx, async move |cx| {
            let Ok(Ok(Some(path))) = target.await else {
                return;
            };
            let fetched = runtime
                .spawn(async move { ircord_client::api::fetch(&url, MAX_DOWNLOAD).await })
                .await;
            let result = match fetched {
                Ok(Ok((status, _, body))) if (200..300).contains(&status) => {
                    std::fs::write(&path, body).map_err(|e| e.to_string())
                }
                Ok(Ok((status, _, _))) => Err(t!("uploads-download-status", status = status)),
                Ok(Err(e)) => Err(e),
                Err(e) => Err(e.to_string()),
            };
            cx.update(|window, cx| match result {
                Ok(()) => ui::toast::success(
                    window,
                    cx,
                    t!("uploads-downloaded", name = path.display().to_string()),
                ),
                Err(e) => ui::toast::error(window, cx, t!("uploads-download-failed", reason = e)),
            })
            .ok();
        })
        .detach();
}

/// Put `url` on the clipboard and say so.
pub fn copy_link(url: &str, window: &mut Window, cx: &mut App) {
    cx.write_to_clipboard(ClipboardItem::new_string(url.to_string()));
    ui::toast::info(window, cx, t!("uploads-link-copied"));
}

/// The three actions on a piece of media: download, copy link, open.
pub fn actions(id: &str, media: &Media, runtime: Arc<Runtime>, theme: &Theme) -> Div {
    let (a, b, c) = (media.clone(), media.url.clone(), media.url.clone());
    div()
        .flex()
        .gap_0p5()
        .child(ui::icon::button(
            ElementId::Name(format!("{id}-download").into()),
            IconName::ArrowDown,
            t!("uploads-download", name = media.file_name()),
            theme,
            move |_, window, cx| download(runtime.clone(), &a, window, cx),
        ))
        .child(ui::icon::button(
            ElementId::Name(format!("{id}-copy").into()),
            IconName::Copy,
            t!("uploads-copy-link"),
            theme,
            move |_, window, cx| copy_link(&b, window, cx),
        ))
        .child(ui::icon::button(
            ElementId::Name(format!("{id}-open").into()),
            IconName::ExternalLink,
            t!("uploads-open"),
            theme,
            move |_, _, cx| cx.open_url(&c),
        ))
}

impl Focusable for Viewer {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for Viewer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme::get(cx).clone();
        let media = self.items[self.index].clone();
        let count = self.items.len();
        let viewport = window.viewport_size();
        let stage_h = (viewport.height - px(220.)).max(px(200.));
        let stage_w = (viewport.width - px(112.)).max(px(280.));
        // The picture at zoom 1 fits the stage; zooming grows it and the
        // stage scrolls.
        let (w, h) = if media.width > 0 && media.height > 0 {
            let fit = (stage_w / px(media.width as f32))
                .min(stage_h / px(media.height as f32))
                .min(1.0);
            (
                px(media.width as f32 * fit * self.zoom),
                px(media.height as f32 * fit * self.zoom),
            )
        } else {
            (stage_w * self.zoom, stage_h * self.zoom)
        };
        let zoom = self.zoom;
        let position = t!(
            "uploads-viewer-position",
            name = media.file_name(),
            index = self.index + 1,
            count = count
        );
        let zoom_text = format!("{:.0}%", zoom * 100.0);
        let mut toolbar = div()
            .id("viewer-toolbar")
            .test_support()
            .role(Role::Toolbar)
            .aria_label(t!("uploads-viewer-tools"))
            .flex()
            .items_center()
            .gap_1()
            .child(
                div()
                    .id("viewer-position")
                    .test_support()
                    .aria_label(SharedString::from(position.clone()))
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_sm()
                    .text_color(theme.muted)
                    .child(position),
            )
            .child(ui::icon::button(
                "viewer-zoom-out",
                IconName::Minus,
                t!("uploads-zoom-out"),
                &theme,
                cx.listener(move |this, _, _, cx| this.zoom(zoom / ZOOM_STEP, cx)),
            ))
            .child(
                div()
                    .id("viewer-zoom")
                    .test_support()
                    .aria_label(SharedString::from(zoom_text.clone()))
                    .text_xs()
                    .text_color(theme.faint)
                    .w(px(40.))
                    .child(zoom_text),
            )
            .child(ui::icon::button(
                "viewer-zoom-in",
                IconName::Plus,
                t!("uploads-zoom-in"),
                &theme,
                cx.listener(move |this, _, _, cx| this.zoom(zoom * ZOOM_STEP, cx)),
            ))
            .child(actions("viewer", &media, self.runtime.clone(), &theme));
        if count > 1 {
            toolbar = toolbar
                .child(ui::icon::button(
                    "viewer-previous",
                    IconName::ArrowLeft,
                    t!("uploads-previous"),
                    &theme,
                    cx.listener(|this, _, _, cx| this.step(-1, cx)),
                ))
                .child(ui::icon::button(
                    "viewer-next",
                    IconName::ArrowRight,
                    t!("uploads-next"),
                    &theme,
                    cx.listener(|this, _, _, cx| this.step(1, cx)),
                ));
        }
        div()
            .id("viewer")
            .test_support()
            .role(Role::Dialog)
            .aria_label(t!("uploads-viewer-title"))
            .track_focus(&self.focus)
            .flex()
            .flex_col()
            .gap_2()
            .on_key_down(cx.listener(|this, e: &KeyDownEvent, _, cx| {
                let k = &e.keystroke;
                if k.modifiers.secondary(cx) || k.modifiers.alt {
                    return;
                }
                match k.key.as_str() {
                    "left" => this.step(-1, cx),
                    "right" => this.step(1, cx),
                    "+" | "=" => this.zoom(this.zoom * ZOOM_STEP, cx),
                    "-" => this.zoom(this.zoom / ZOOM_STEP, cx),
                    "0" => this.zoom(1.0, cx),
                    _ => return,
                }
                cx.stop_propagation();
            }))
            .child(toolbar)
            .child(
                div()
                    .id("viewer-stage")
                    .test_support()
                    .h(stage_h)
                    .w_full()
                    .rounded_md()
                    .bg(theme.rail)
                    .overflow_scroll()
                    .flex()
                    .when(zoom <= 1.0, |d| d.items_center().justify_center())
                    .child(
                        div()
                            .id(ElementId::Name(
                                format!("viewer-image-{}", self.index).into(),
                            ))
                            .test_support()
                            .role(Role::Image)
                            .aria_label(SharedString::from(media.file_name()))
                            .flex_none()
                            .w(w)
                            .h(h)
                            .child(
                                img(SharedUri::from(media.url.clone()))
                                    .size_full()
                                    .object_fit(ObjectFit::Contain),
                            ),
                    ),
            )
    }
}

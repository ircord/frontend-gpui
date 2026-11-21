//! What a message carries besides its text (package U1): uploaded pictures
//! as one gallery, other uploads as file cards with their type and size,
//! and GIFs and linked pictures inline. Pictures open the viewer; each
//! carries download, copy-link and open buttons.
//!
//! The space for every picture is reserved from its known size before it
//! loads, so loading never moves the timeline. In a channel marked as
//! sensitive, or whose settings are unknown, media stays covered until the
//! person shows it (or allows sensitive content for that network), as
//! `useMediaSensitivity.ts` decides on the web. GIFs play always or only
//! under the pointer, after the appearance setting "Animate media".

use crate::ui::focus::Reachable;
use std::collections::HashSet;
use std::sync::Arc;

use gpui_kit::TestSupportExt;
use gpui_kit::prelude::*;
use gpui_kit::*;
use ircord_client::api::uploads::human_size;
use ircord_client::contract::{Attachment, ConversationKind};
use ircord_client::documents;
use ircord_client::store::link_media;
use tokio::runtime::Runtime;

use crate::features::viewer::{self, Media};
use crate::registry::{self, MessageCtx, PlayableCtx, PlayableKind};
use crate::theme::Theme;
use crate::timeline::rows::{Image, MessageRow, looks_like_image};
use crate::ui;
use crate::ui::icon::IconName;

/// A single picture is at most this large…
const MAX_W: f32 = 400.0;
const MAX_H: f32 = 300.0;
/// …and a gallery fills this stage, whatever the count.
const STAGE_W: f32 = 440.0;
const STAGE_H: f32 = 280.0;
/// A gallery shows this many tiles; a larger one counts the rest on the last.
const MAX_TILES: usize = 9;

/// Messages whose covered media the person chose to show.
#[derive(Default)]
struct Revealed(HashSet<String>);
impl Global for Revealed {}

/// The GIF under the pointer, when GIFs play only on hover.
#[derive(Default)]
struct Hovered(Option<String>);
impl Global for Hovered {}

/// Clients whose preferences and appearance this module watches.
#[derive(Default)]
struct Watched(HashSet<usize>);
impl Global for Watched {}

/// Why media in a conversation is covered.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Sensitivity {
    /// The channel is marked as sensitive.
    Nsfw,
    /// The channel's settings are not known.
    Unknown,
}

/// What the timeline needs to draw media for one message.
struct Context {
    runtime: Arc<Runtime>,
    sensitivity: Option<Sensitivity>,
    animate: bool,
    /// The server's origin, for media named by a path on it.
    origin: Option<String>,
}

fn context(ctx: &MessageCtx, cx: &mut App) -> Option<Context> {
    let session = ctx.session.read(cx);
    let client = session.client()?.clone();
    let runtime = session.runtime.clone();
    let origin = session.server().map(|s| s.origin());
    let key = std::rc::Rc::as_ptr(&client) as usize;
    if cx.default_global::<Watched>().0.insert(key) {
        client.watch_document(&documents::PREFERENCES);
        client.watch_document(&documents::APPEARANCE);
    }
    let w = client.read();
    let animate = w
        .documents
        .appearance
        .value()
        .and_then(|a| a.settings.as_ref())
        .and_then(|s| s.animate_media)
        .unwrap_or(true);
    let sensitivity = match w.conversation(&ctx.conversation) {
        None => Some(Sensitivity::Unknown),
        Some(c) if c.kind != ConversationKind::Channel || c.party.is_some() => None,
        Some(c) => {
            let preferences = w
                .documents
                .preferences
                .value()
                .and_then(|p| p.preferences.as_ref());
            let allowed = preferences.is_some_and(|p| {
                p.show_nsfw == Some(true)
                    || p.nsfw_networks
                        .as_ref()
                        .and_then(|n| n.get(&c.network_id))
                        .copied()
                        == Some(true)
            });
            if allowed {
                None
            } else {
                match c.channel_settings.as_ref().map(|s| s.nsfw) {
                    Some(false) => None,
                    Some(true) => Some(Sensitivity::Nsfw),
                    None => Some(Sensitivity::Unknown),
                }
            }
        }
    };
    Some(Context {
        runtime,
        sensitivity,
        animate,
        origin,
    })
}

fn media_of(a: &Attachment, origin: Option<&str>) -> Media {
    Media {
        url: link_media::resolve_url(&a.url, origin),
        name: a.name.clone(),
        width: a.width.max(0) as u32,
        height: a.height.max(0) as u32,
        content_type: a.content_type.clone().unwrap_or_default(),
        size: a.size,
    }
}

fn media_of_image(i: &Image, origin: Option<&str>) -> Media {
    Media {
        url: link_media::resolve_url(&i.url, origin),
        name: i.alt.clone(),
        width: i.width,
        height: i.height,
        content_type: String::new(),
        size: None,
    }
}

fn is_picture(a: &Attachment) -> bool {
    a.content_type
        .as_deref()
        .map_or_else(|| looks_like_image(&a.url), |t| t.starts_with("image/"))
}

pub fn render(
    m: &MessageRow,
    ctx: &MessageCtx,
    theme: &Theme,
    window: &mut Window,
    cx: &mut App,
) -> Vec<AnyElement> {
    if m.deleted {
        return Vec::new();
    }
    let Some(context) = context(ctx, cx) else {
        return Vec::new();
    };
    let origin = context.origin.as_deref();
    let pictures: Vec<Media> = m
        .attachments
        .iter()
        .filter(|a| is_picture(a))
        .map(|a| media_of(a, origin))
        .collect();
    let files: Vec<Media> = m
        .attachments
        .iter()
        .filter(|a| !is_picture(a))
        .map(|a| media_of(a, origin))
        .collect();
    let inline: Vec<Media> = m.images.iter().map(|i| media_of_image(i, origin)).collect();
    if pictures.is_empty() && files.is_empty() && inline.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    if let Some(why) = covered(&context, &m.id, cx) {
        let (w, h) = if pictures.len() > 1 {
            (STAGE_W, STAGE_H)
        } else {
            (MAX_W, MAX_H * 0.5)
        };
        out.push(cover(ctx, &m.id, why, w, h, theme));
        return out;
    }
    let hovered = cx.try_global::<Hovered>().and_then(|h| h.0.clone());
    let draw = Draw {
        ctx,
        message: &m.id,
        theme,
        context: &context,
        hovered: hovered.as_deref(),
    };
    if pictures.len() == 1 {
        out.push(draw.single("attachment", &pictures, 0));
    } else if !pictures.is_empty() {
        out.push(draw.gallery(&pictures));
    }
    for (i, file) in files.iter().enumerate() {
        out.push(draw.file_card(i, file, window, cx));
    }
    for (i, _) in inline.iter().enumerate() {
        out.push(draw.single("image", &inline, i));
    }
    out
}

/// Whether media under `key` in this message is covered now: the channel
/// is sensitive (or its settings unknown) and the person has not shown it.
/// A playable is only offered media that is not hidden.
pub fn hidden(ctx: &MessageCtx, key: &str, cx: &mut App) -> bool {
    context(ctx, cx).is_some_and(|context| covered(&context, key, cx).is_some())
}

/// What a file plays as, from its content type or else its address.
pub fn playable_kind(content_type: &str, url: &str) -> Option<PlayableKind> {
    if content_type.starts_with("video/") {
        return Some(PlayableKind::Video);
    }
    if content_type.starts_with("audio/") {
        return Some(PlayableKind::Audio);
    }
    let path = url
        .split(['?', '#'])
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    let ext = path.rsplit_once('.').map_or("", |(_, e)| e);
    if ["mp4", "webm", "mov", "m4v"].contains(&ext) {
        Some(PlayableKind::Video)
    } else if ["mp3", "m4a", "ogg", "wav", "flac", "aac"].contains(&ext) {
        Some(PlayableKind::Audio)
    } else {
        None
    }
}

/// Why media under `key` is covered, unless the person already showed it.
fn covered(context: &Context, key: &str, cx: &App) -> Option<Sensitivity> {
    context.sensitivity.filter(|_| {
        !cx.try_global::<Revealed>()
            .is_some_and(|r| r.0.contains(key))
    })
}

/// Pictures found behind a link (package L1), drawn as uploads are: covered
/// in a sensitive channel until shown, one picture sized from what is known
/// of it, several as one gallery that opens the viewer. `key` names the
/// elements (`image-{key}-0`, `gallery-{key}`, `attachment-{key}-1`,
/// `media-cover-{key}`) and what showing covered media remembers.
pub fn pictures(
    ctx: &MessageCtx,
    key: &str,
    items: &[Media],
    theme: &Theme,
    cx: &mut App,
) -> Option<AnyElement> {
    if items.is_empty() {
        return None;
    }
    let context = context(ctx, cx)?;
    if let Some(why) = covered(&context, key, cx) {
        let (w, h) = if items.len() > 1 {
            (STAGE_W, STAGE_H)
        } else {
            (MAX_W, MAX_H * 0.5)
        };
        return Some(cover(ctx, key, why, w, h, theme));
    }
    let hovered = cx.try_global::<Hovered>().and_then(|h| h.0.clone());
    let draw = Draw {
        ctx,
        message: key,
        theme,
        context: &context,
        hovered: hovered.as_deref(),
    };
    Some(if items.len() == 1 {
        draw.single("image", items, 0)
    } else {
        draw.gallery(items)
    })
}

/// The cover over sensitive media, with a button to show it.
fn cover(
    ctx: &MessageCtx,
    message: &str,
    why: Sensitivity,
    w: f32,
    h: f32,
    theme: &Theme,
) -> AnyElement {
    let (title, hint): (SharedString, SharedString) = match why {
        Sensitivity::Nsfw => (
            t!("uploads-sensitive").into(),
            t!("uploads-sensitive-hint").into(),
        ),
        Sensitivity::Unknown => (
            t!("uploads-sensitive-unknown").into(),
            t!("uploads-sensitive-unknown-hint").into(),
        ),
    };
    let id = message.to_string();
    let _ = ctx;
    div()
        .id(ElementId::Name(format!("media-cover-{message}").into()))
        .test_support()
        .role(Role::Group)
        .aria_label(title.clone())
        .mt_1()
        .w(px(w))
        .h(px(h.max(120.)))
        .rounded_md()
        .bg(theme.sidebar)
        .border_1()
        .border_color(theme.rail)
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap_1()
        .p_3()
        .child(
            div()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.text)
                .child(title),
        )
        .child(
            div()
                .text_xs()
                .text_color(theme.muted)
                .text_center()
                .child(hint),
        )
        .child(
            div()
                .id(ElementId::Name(format!("media-reveal-{message}").into()))
                .test_support()
                .role(Role::Button)
                .reachable(theme)
                .aria_label(t!("uploads-show-media"))
                .mt_1()
                .px_3()
                .py_1()
                .rounded_md()
                .bg(theme.accent)
                .text_color(theme.on_accent)
                .text_sm()
                .cursor_pointer()
                .child(t!("uploads-show-media"))
                .on_click(move |_, window, cx| {
                    cx.default_global::<Revealed>().0.insert(id.clone());
                    window.refresh();
                }),
        )
        .into_any_element()
}

struct Draw<'a> {
    ctx: &'a MessageCtx,
    message: &'a str,
    theme: &'a Theme,
    context: &'a Context,
    hovered: Option<&'a str>,
}

impl Draw<'_> {
    /// The picture itself: playing, or held on its first frame.
    fn picture(&self, key: &str, media: &Media, fit: ObjectFit) -> AnyElement {
        let playing = !media.is_gif() || self.context.animate || self.hovered == Some(key);
        let source = if playing {
            ImageSource::from(SharedUri::from(media.url.clone()))
        } else {
            crate::images::still(media.url.clone().into())
        };
        img(source).size_full().object_fit(fit).into_any_element()
    }

    fn open_viewer(
        &self,
        items: &[Media],
        index: usize,
    ) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static {
        let (session, items) = (self.ctx.session.clone(), items.to_vec());
        move |_, window, cx| viewer::open(&session, items.clone(), index, window, cx)
    }

    fn on_hover(&self, key: String, gif: bool) -> impl Fn(&bool, &mut Window, &mut App) + 'static {
        let animate = self.context.animate;
        move |over, window, cx| {
            if !gif || animate {
                return;
            }
            let hovered = &mut cx.default_global::<Hovered>().0;
            if *over {
                *hovered = Some(key.clone());
            } else if hovered.as_deref() == Some(key.as_str()) {
                *hovered = None;
            }
            window.refresh();
        }
    }

    /// The download, copy and open buttons, shown over a picture under the
    /// pointer.
    fn overlay_actions(&self, id: &str, media: &Media, group: &str) -> AnyElement {
        div()
            .absolute()
            .top(px(6.))
            .right(px(6.))
            .rounded_md()
            .bg(self.theme.sidebar)
            .opacity(0.)
            .group_hover(SharedString::from(group.to_string()), |s| s.opacity(1.))
            .child(viewer::actions(
                id,
                media,
                self.context.runtime.clone(),
                self.theme,
            ))
            .into_any_element()
    }

    /// One picture, sized from what is known of it.
    fn single(&self, kind: &str, items: &[Media], i: usize) -> AnyElement {
        let media = &items[i];
        let (w, h) = if media.width > 0 && media.height > 0 {
            let scale = (MAX_W / media.width as f32)
                .min(MAX_H / media.height as f32)
                .min(1.0);
            (media.width as f32 * scale, media.height as f32 * scale)
        } else {
            (MAX_W, MAX_H * 0.66)
        };
        let id = format!("{kind}-{}-{i}", self.message);
        let group = format!("{id}-group");
        div()
            .id(ElementId::Name(id.clone().into()))
            .test_support()
            .group(SharedString::from(group.clone()))
            .role(Role::Image)
            .aria_label(SharedString::from(if media.name.is_empty() {
                media.url.clone()
            } else {
                media.name.clone()
            }))
            .relative()
            .mt_1()
            .w(px(w))
            .h(px(h))
            .rounded_md()
            .overflow_hidden()
            .bg(self.theme.sidebar)
            .cursor_pointer()
            .child(self.picture(&id, media, ObjectFit::Contain))
            .child(self.overlay_actions(&id, media, &group))
            .on_hover(self.on_hover(id.clone(), media.is_gif()))
            .on_click(self.open_viewer(items, i))
            .into_any_element()
    }

    /// Several pictures as one mosaic of a fixed size.
    fn gallery(&self, items: &[Media]) -> AnyElement {
        let shown = items.len().min(MAX_TILES);
        let more = items.len() - shown;
        // Rows of tiles: two side by side, a 2×2 grid, then rows of three.
        let rows: Vec<usize> = match shown {
            2 => vec![2],
            3 => vec![1, 2],
            4 => vec![2, 2],
            n => {
                let mut rows = vec![3; n / 3];
                if n % 3 > 0 {
                    rows.insert(0, n % 3);
                }
                rows
            }
        };
        let row_h = (STAGE_H - 4.0 * (rows.len() as f32 - 1.0)) / rows.len() as f32;
        let mut stage = div()
            .id(ElementId::Name(format!("gallery-{}", self.message).into()))
            .test_support()
            .role(Role::Group)
            .aria_label(t!("uploads-gallery", count = items.len()))
            .mt_1()
            .w(px(STAGE_W))
            .h(px(STAGE_H))
            .flex()
            .flex_col()
            .gap_1();
        let mut at = 0;
        for per_row in rows {
            let mut row = div().w_full().h(px(row_h)).flex().gap_1();
            for _ in 0..per_row {
                let i = at;
                at += 1;
                let media = &items[i];
                let id = format!("attachment-{}-{i}", self.message);
                let group = format!("{id}-group");
                let mut tile = div()
                    .id(ElementId::Name(id.clone().into()))
                    .test_support()
                    .group(SharedString::from(group.clone()))
                    .role(Role::Image)
                    .aria_label(SharedString::from(media.name.clone()))
                    .relative()
                    .flex_1()
                    .h_full()
                    .min_w_0()
                    .rounded_md()
                    .overflow_hidden()
                    .bg(self.theme.sidebar)
                    .cursor_pointer()
                    .child(self.picture(&id, media, ObjectFit::Cover))
                    .on_hover(self.on_hover(id.clone(), media.is_gif()))
                    .on_click(self.open_viewer(items, i));
                if more > 0 && i == shown - 1 {
                    tile = tile.child(
                        div()
                            .absolute()
                            .inset_0()
                            .bg(self.theme.rail.opacity(0.7))
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_xl()
                            .text_color(self.theme.text)
                            .child(t!("uploads-gallery-more", count = more + 1)),
                    );
                } else {
                    tile = tile.child(self.overlay_actions(&id, media, &group));
                }
                row = row.child(tile);
            }
            stage = stage.child(row);
        }
        stage.into_any_element()
    }

    /// A file that is not a picture: a registered playable for a sound or
    /// video, or else its name, type and size, and the buttons.
    fn file_card(&self, i: usize, media: &Media, window: &mut Window, cx: &mut App) -> AnyElement {
        if let Some(kind) = playable_kind(&media.content_type, &media.url) {
            let playable = PlayableCtx {
                session: self.ctx.session.clone(),
                conversation: self.ctx.conversation.clone(),
                message: self.message.to_string(),
                key: format!("file-{}-{i}", self.message),
                kind,
                media: media.clone(),
                duration: None,
                poster: None,
                page: media.url.clone(),
            };
            if let Some(element) = registry::playable(&playable, window, cx) {
                return element;
            }
        }
        let kind = if media.content_type.starts_with("video/") {
            t!("uploads-file-video")
        } else if media.content_type.starts_with("audio/") {
            t!("uploads-file-audio")
        } else if media.content_type == "application/pdf" {
            t!("uploads-file-pdf")
        } else {
            t!("uploads-file-other")
        };
        let detail: String = match media.size {
            Some(bytes) if bytes > 0 => {
                t!(
                    "uploads-file-sized",
                    kind = kind,
                    size = human_size(bytes as u64)
                )
            }
            _ => kind.to_string(),
        };
        let id = format!("file-{}-{i}", self.message);
        let url = media.url.clone();
        let hover = self.theme.hover;
        let icon = if media.content_type.starts_with("video/") {
            IconName::Play
        } else {
            IconName::File
        };
        div()
            .id(ElementId::Name(id.clone().into()))
            .test_support()
            .role(Role::Group)
            .aria_label(SharedString::from(media.file_name()))
            .max_w(px(440.))
            .mt_1()
            .px_3()
            .py_2()
            .rounded_md()
            .border_1()
            .border_color(self.theme.rail)
            .bg(self.theme.sidebar)
            .flex()
            .items_center()
            .gap_3()
            .child(ui::icon::icon(icon, self.theme.muted))
            .child(
                div()
                    .id(ElementId::Name(format!("{id}-name").into()))
                    .flex_1()
                    .min_w_0()
                    .cursor_pointer()
                    .hover(move |d| d.bg(hover))
                    .on_click(move |_, _, cx| cx.open_url(&url))
                    .child(
                        div()
                            .text_sm()
                            .text_color(self.theme.link)
                            .truncate()
                            .child(media.file_name()),
                    )
                    .child(
                        div()
                            .id(ElementId::Name(format!("{id}-detail").into()))
                            .test_support()
                            .aria_label(SharedString::from(detail.clone()))
                            .text_xs()
                            .text_color(self.theme.muted)
                            .child(detail),
                    ),
            )
            .child(viewer::actions(
                &id,
                media,
                self.context.runtime.clone(),
                self.theme,
            ))
            .into_any_element()
    }
}

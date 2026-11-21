//! Link previews (package L1), after `MessageLinks.vue` and the media
//! renderers of the web client. Every link in a message's text is resolved
//! once through the server (`store/link_media.rs`), and drawn by what came
//! back: pictures and GIFs from the server's cache inline (several as one
//! gallery that opens the viewer), social posts as a card with the byline,
//! the caption and their pictures, files as a file card, pages as a card
//! with the site, title, description and picture. Players and videos cannot
//! play here: they are a card with the title and picture that opens the
//! browser. A link to a message in this app shows that message instead.
//!
//! Each card keeps one height while it loads, so a preview never moves the
//! timeline once it is drawn, and every card still opens its link.

use std::rc::Rc;

use gpui_kit::TestSupportExt;
use gpui_kit::prelude::*;
use gpui_kit::*;
use ircord_client::Client;
use ircord_client::api::uploads::human_size;
use ircord_client::contract::{
    ConversationKind, LinkMediaAssetKind, LinkMediaResponse, LinkMediaResponseKind, Message,
};
use ircord_client::format::links::{MessageLink, parse_message_link};
use ircord_client::store::link_media::{self, Linked, Preview};

use crate::features::viewer::Media;
use crate::message::attachments;
use crate::registry::MessageCtx;
use crate::theme::Theme;
use crate::timeline::rows::{LinkCard, MessageRow};
use crate::ui;
use crate::ui::icon::IconName;

/// Cards are at most this wide…
const CARD_W: f32 = 480.0;
/// …and a page card this high, whatever it holds.
const CARD_H: f32 = 120.0;
/// A message-link card's height in every state.
const MESSAGE_CARD_H: f32 = 58.0;
/// The space a picture whose size is unknown reserves while it loads.
const PICTURE_W: f32 = 400.0;
const PICTURE_H: f32 = 200.0;
/// A message link's excerpt is cut after this many characters.
const EXCERPT: usize = 160;

pub fn render(
    m: &MessageRow,
    ctx: &MessageCtx,
    theme: &Theme,
    window: &mut Window,
    cx: &mut App,
) -> Vec<AnyElement> {
    if m.deleted || m.links.is_empty() {
        return Vec::new();
    }
    let session = ctx.session.read(cx);
    let client = session.client().cloned();
    let origin = session.server().map(|s| s.origin());
    let mut out = Vec::new();
    for (i, link) in m.links.iter().enumerate() {
        let key = format!("{}-{i}", m.id);
        let Some(client) = &client else {
            out.push(page_card(&key, link, None, Status::Plain, theme));
            continue;
        };
        if let Some(target) = origin
            .as_deref()
            .and_then(|o| parse_message_link(&link.url, o))
        {
            out.push(message_card(&key, &target, ctx, client, theme, window, cx));
            continue;
        }
        out.push(preview(&key, link, ctx, client, theme, window, cx));
    }
    out
}

/// Ask the server for `link`'s preview when it is not known yet, and draw
/// the window again when the answer is in.
fn resolve(client: &Rc<Client>, ctx: &MessageCtx, url: &str, window: &mut Window, cx: &mut App) {
    let now = ircord_client::events::now_ms();
    let mut ask = false;
    client.change(|w| {
        ask = w.begin_link_preview(&ctx.message, url, now);
        Default::default()
    });
    if !ask {
        return;
    }
    let (conversation, message, url) = (
        ctx.conversation.clone(),
        ctx.message.clone(),
        url.to_string(),
    );
    let (m, u) = (message.clone(), url.clone());
    let done = client.call(
        move |api| link_media::resolve(api, conversation, message, url),
        move |w, result| w.link_preview_answered(&m, &u, result, ircord_client::events::now_ms()),
    );
    window
        .spawn(cx, async move |cx| {
            let _ = done.await;
            let _ = cx.update(|window, _| window.refresh());
        })
        .detach();
}

fn preview(
    key: &str,
    link: &LinkCard,
    ctx: &MessageCtx,
    client: &Rc<Client>,
    theme: &Theme,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    resolve(client, ctx, &link.url, window, cx);
    let state = client.read().link_preview(&ctx.message, &link.url).cloned();
    match state {
        None | Some(Preview::Loading) => {
            if link.picture {
                reserved(key, link, theme)
            } else {
                page_card(key, link, None, Status::Loading, theme)
            }
        }
        Some(Preview::Failed { .. }) => page_card(key, link, None, Status::Failed, theme),
        Some(Preview::Ready { response, .. }) => match response.kind {
            LinkMediaResponseKind::Media => media(key, link, &response, ctx, theme, cx),
            LinkMediaResponseKind::File => file_card(key, link, &response, theme),
            LinkMediaResponseKind::Widget => player_card(key, link, &response, theme),
            LinkMediaResponseKind::Card | LinkMediaResponseKind::Unknown => {
                page_card(key, link, Some(&response), Status::Plain, theme)
            }
        },
    }
}

/// What a card says under its title.
#[derive(Clone, Copy, PartialEq)]
enum Status {
    Plain,
    Loading,
    Failed,
}

/// The card's frame: it opens `url` in the browser. The caller adds the
/// test support, the role and the label.
fn frame(key: &str, url: &str, theme: &Theme) -> Stateful<Div> {
    let hover = theme.hover;
    let open = link_media::public_url(url);
    div()
        .id(ElementId::Name(format!("link-{key}").into()))
        .max_w(px(CARD_W))
        .w_full()
        .mt_1()
        .rounded_md()
        .border_1()
        .border_color(theme.rail)
        .bg(theme.sidebar)
        .overflow_hidden()
        .cursor_pointer()
        .hover(move |d| d.bg(hover))
        .on_click(move |_, _, cx| {
            if let Some(url) = &open {
                cx.open_url(url)
            }
        })
}

/// A small line of muted text, cut to one line.
fn caption(id: String, text: impl Into<SharedString>, theme: &Theme) -> AnyElement {
    let text = text.into();
    div()
        .id(ElementId::Name(id.into()))
        .test_support()
        .aria_label(text.clone())
        .text_xs()
        .text_color(theme.muted)
        .truncate()
        .child(text)
        .into_any_element()
}

/// A page: its picture, site, title and description, or why there is none.
/// Before the server answers, and when it could not, the card shows the
/// link's host.
fn page_card(
    key: &str,
    link: &LinkCard,
    response: Option<&LinkMediaResponse>,
    status: Status,
    theme: &Theme,
) -> AnyElement {
    let card = response.and_then(|r| r.card.clone()).unwrap_or_default();
    let site = if card.site.is_empty() {
        link.title.clone()
    } else {
        card.site.clone()
    };
    let title = if !card.title.is_empty() {
        card.title.clone()
    } else if let Some(t) = response
        .and_then(|r| r.title.clone())
        .filter(|t| !t.is_empty())
    {
        t
    } else if link.video {
        t!("core-message-link-video-title", title = site)
    } else {
        site.clone()
    };
    let description = card
        .description
        .clone()
        .or_else(|| response.and_then(|r| r.description.clone()))
        .unwrap_or_default();
    let note: Option<String> = match status {
        Status::Loading => Some(t!("link-previews-loading").to_string()),
        Status::Failed => Some(t!("link-previews-reason-fetch-failed").to_string()),
        Status::Plain => link_media::reason_key(card.reason, None).map(reason_text),
    };
    let detail = note.unwrap_or_else(|| link.detail.clone());
    let image = card.image.as_deref().and_then(link_media::public_url);
    let icon = card.icon.as_deref().and_then(link_media::public_url);
    let mut text = div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .gap_0p5()
        .child(
            div()
                .flex()
                .items_center()
                .gap_1p5()
                .when_some(icon, |d, icon| {
                    d.child(img(SharedUri::from(icon)).size(px(14.)).flex_none())
                })
                .child(caption(format!("link-{key}-site"), site, theme)),
        )
        .child(
            div()
                .id(ElementId::Name(format!("link-{key}-title").into()))
                .test_support()
                .aria_label(SharedString::from(title.clone()))
                .text_sm()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.link)
                .line_clamp(2)
                .child(title),
        );
    if !description.is_empty() {
        text = text.child(
            div()
                .text_xs()
                .text_color(theme.text)
                .line_clamp(2)
                .child(description),
        );
    }
    text = text.child(caption(format!("link-{key}-status"), detail, theme));
    frame(key, &link.url, theme)
        .test_support()
        .role(Role::Link)
        .aria_label(SharedString::from(link.url.clone()))
        .h(px(CARD_H))
        .p_3()
        .flex()
        .gap_3()
        .border_l_4()
        .border_color(if link.video {
            theme.danger
        } else {
            theme.accent
        })
        .when_some(image, |d, image| {
            d.child(
                div()
                    .size(px(CARD_H - 26.))
                    .flex_none()
                    .rounded_md()
                    .overflow_hidden()
                    .bg(theme.main)
                    .child(
                        img(SharedUri::from(image))
                            .size_full()
                            .object_fit(ObjectFit::Cover),
                    ),
            )
        })
        .child(text)
        .into_any_element()
}

/// The space a linked picture takes while its preview loads.
fn reserved(key: &str, link: &LinkCard, theme: &Theme) -> AnyElement {
    let (w, h) = fit(link.width, link.height);
    div()
        .id(ElementId::Name(format!("link-{key}").into()))
        .test_support()
        .role(Role::Image)
        .aria_label(SharedString::from(link.url.clone()))
        .mt_1()
        .w(px(w))
        .h(px(h))
        .rounded_md()
        .bg(theme.sidebar)
        .into_any_element()
}

/// A picture's box: its own size scaled into the largest a picture is.
fn fit(width: u32, height: u32) -> (f32, f32) {
    if width == 0 || height == 0 {
        return (PICTURE_W, PICTURE_H);
    }
    let scale = (PICTURE_W / width as f32)
        .min(300.0 / height as f32)
        .min(1.0);
    (width as f32 * scale, height as f32 * scale)
}

/// Media from the server's cache: pictures inline, a post with its byline
/// and caption, videos and audio as a card that opens the browser.
fn media(
    key: &str,
    link: &LinkCard,
    r: &LinkMediaResponse,
    ctx: &MessageCtx,
    theme: &Theme,
    cx: &mut App,
) -> AnyElement {
    let name = r.title.clone().unwrap_or_default();
    let pictures: Vec<Media> = r
        .assets
        .iter()
        .filter(|a| a.kind == LinkMediaAssetKind::Image)
        .filter_map(|a| {
            Some(Media {
                url: link_media::public_url(&a.url)?,
                name: name.clone(),
                width: a.width.unwrap_or(0).max(0) as u32,
                height: a.height.unwrap_or(0).max(0) as u32,
                content_type: a.content_type.clone().unwrap_or_default(),
                size: None,
            })
        })
        .collect();
    let playable = r
        .assets
        .iter()
        .filter(|a| {
            matches!(
                a.kind,
                LinkMediaAssetKind::Video | LinkMediaAssetKind::Audio
            )
        })
        .count();
    if pictures.is_empty() && playable == 0 {
        return page_card(key, link, Some(r), Status::Plain, theme);
    }
    let post = is_post(r);
    let shown = attachments::pictures(ctx, &format!("link-{key}"), &pictures, theme, cx);
    if !post {
        let mut column = div()
            .id(ElementId::Name(format!("link-{key}").into()))
            .test_support()
            .role(Role::Group)
            .aria_label(SharedString::from(link.url.clone()))
            .flex()
            .flex_col()
            .children(shown);
        if playable > 0 {
            column = column.child(playable_card(key, link, r, theme));
        }
        return column.into_any_element();
    }
    let text = link_media::post_text(r);
    let provider = provider_name(&r.provider);
    let byline: Vec<String> = [text.name.clone(), text.handle.clone(), provider.clone()]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect();
    let label = if !text.title.is_empty() {
        text.title.clone()
    } else if let Some(first) = text.caption.lines().next().filter(|l| !l.is_empty()) {
        first.chars().take(120).collect()
    } else {
        t!("link-previews-post", provider = provider)
    };
    let hover = theme.hover;
    let open = link_media::public_url(&link.url);
    div()
        .id(ElementId::Name(format!("link-{key}").into()))
        .test_support()
        .role(Role::Article)
        .aria_label(SharedString::from(label))
        .max_w(px(CARD_W))
        .mt_1()
        .p_3()
        .rounded_md()
        .border_1()
        .border_color(theme.rail)
        .bg(theme.sidebar)
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .id(ElementId::Name(format!("link-{key}-byline").into()))
                .test_support()
                .role(Role::Link)
                .aria_label(SharedString::from(byline.join(" · ")))
                .flex()
                .items_center()
                .gap_1()
                .cursor_pointer()
                .hover(move |d| d.bg(hover))
                .on_click(move |_, _, cx| {
                    if let Some(url) = &open {
                        cx.open_url(url)
                    }
                })
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.text)
                        .truncate()
                        .child(byline.join(" · ")),
                )
                .child(ui::icon::icon(IconName::ExternalLink, theme.muted)),
        )
        .when(!text.title.is_empty(), |d| {
            d.child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.text)
                    .line_clamp(2)
                    .child(text.title.clone()),
            )
        })
        .when(!text.caption.is_empty(), |d| {
            d.child(
                div()
                    .id(ElementId::Name(format!("link-{key}-caption").into()))
                    .test_support()
                    .aria_label(SharedString::from(text.caption.clone()))
                    .text_sm()
                    .text_color(theme.text)
                    .line_clamp(4)
                    .child(text.caption.clone()),
            )
        })
        .children(shown)
        .when(playable > 0, |d| {
            d.child(playable_card(key, link, r, theme))
        })
        .into_any_element()
}

/// A social post or an image page with something to say, rather than a
/// bare picture.
fn is_post(r: &LinkMediaResponse) -> bool {
    let said = |s: &Option<String>| s.as_deref().is_some_and(|s| !s.trim().is_empty());
    matches!(r.provider.as_str(), "instagram" | "tiktok" | "reddit")
        || (r.provider != "media" && (said(&r.author) || said(&r.description)))
}

fn provider_name(provider: &str) -> String {
    match provider {
        "reddit" => "Reddit".into(),
        "instagram" => "Instagram".into(),
        "tiktok" => "TikTok".into(),
        "media" | "link" | "page" | "oembed" | "file" => String::new(),
        other => other.to_string(),
    }
}

/// Videos and audio found behind a link: they play in the browser.
fn playable_card(key: &str, link: &LinkCard, r: &LinkMediaResponse, theme: &Theme) -> AnyElement {
    let audio = r.assets.iter().all(|a| a.kind != LinkMediaAssetKind::Video);
    let title = r
        .title
        .clone()
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| link.title.clone());
    let detail = if audio {
        t!("link-previews-audio").to_string()
    } else {
        t!("link-previews-video").to_string()
    };
    open_card(key, "play", &link.url, &title, &detail, None, theme)
}

/// A player (YouTube, Vimeo and some fifty other sites): its picture and
/// title; it plays in the browser.
fn player_card(key: &str, link: &LinkCard, r: &LinkMediaResponse, theme: &Theme) -> AnyElement {
    let site = r
        .card
        .as_ref()
        .map(|c| c.site.clone())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| link.title.clone());
    let title = r
        .title
        .clone()
        .or_else(|| r.card.as_ref().map(|c| c.title.clone()))
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| site.clone());
    let thumbnail = r
        .widget
        .as_ref()
        .and_then(|w| w.thumbnail.as_deref())
        .or_else(|| r.card.as_ref().and_then(|c| c.image.as_deref()))
        .and_then(link_media::public_url);
    let detail = t!("link-previews-player", site = site);
    let wrapper = div()
        .id(ElementId::Name(format!("link-{key}").into()))
        .test_support()
        .role(Role::Group)
        .aria_label(SharedString::from(link.url.clone()));
    wrapper
        .child(open_card(
            key, "player", &link.url, &title, &detail, thumbnail, theme,
        ))
        .into_any_element()
}

/// A card with a play mark that opens `url`: a picture on top when there
/// is one, then the title and what opening it does.
fn open_card(
    key: &str,
    kind: &str,
    url: &str,
    title: &str,
    detail: &str,
    picture: Option<String>,
    theme: &Theme,
) -> AnyElement {
    let hover = theme.hover;
    let open = link_media::public_url(url);
    let label = t!("link-previews-open-in-browser", title = title);
    div()
        .id(ElementId::Name(format!("link-{key}-{kind}").into()))
        .test_support()
        .role(Role::Link)
        .aria_label(SharedString::from(label))
        .max_w(px(CARD_W))
        .w_full()
        .mt_1()
        .rounded_md()
        .border_1()
        .border_color(theme.rail)
        .bg(theme.sidebar)
        .overflow_hidden()
        .cursor_pointer()
        .hover(move |d| d.bg(hover))
        .on_click(move |_, _, cx| {
            if let Some(url) = &open {
                cx.open_url(url)
            }
        })
        .when_some(picture, |d, picture| {
            d.child(
                div()
                    .relative()
                    .w_full()
                    .h(px(240.))
                    .bg(theme.main)
                    .child(
                        img(SharedUri::from(picture))
                            .size_full()
                            .object_fit(ObjectFit::Cover),
                    )
                    .child(
                        div()
                            .absolute()
                            .inset_0()
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(
                                div()
                                    .size(px(48.))
                                    .rounded_full()
                                    .bg(theme.rail.opacity(0.8))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(ui::icon::icon(IconName::Play, theme.text)),
                            ),
                    ),
            )
        })
        .child(
            div()
                .px_3()
                .py_2()
                .flex()
                .items_center()
                .gap_3()
                .child(ui::icon::icon(IconName::Play, theme.danger))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(
                            div()
                                .text_sm()
                                .text_color(theme.link)
                                .truncate()
                                .child(title.to_string()),
                        )
                        .child(caption(
                            format!("link-{key}-{kind}-detail"),
                            detail.to_string(),
                            theme,
                        )),
                )
                .child(ui::icon::icon(IconName::ExternalLink, theme.muted)),
        )
        .into_any_element()
}

/// A file behind a link: its name, detected type and size, or why the
/// server would not keep it. It opens in the browser.
fn file_card(key: &str, link: &LinkCard, r: &LinkMediaResponse, theme: &Theme) -> AnyElement {
    let file = r.file.clone().unwrap_or_default();
    let name = if file.name.is_empty() {
        link.title.clone()
    } else {
        file.name.clone()
    };
    let kind = type_text(link_media::file_type_key(&file.content_type).unwrap_or("file"));
    let detail = match link_media::reason_key(None, file.reason) {
        Some(r) => reason_text(r),
        None if file.size > 0 => t!(
            "link-previews-file-sized",
            kind = kind,
            size = human_size(file.size as u64)
        ),
        None => kind,
    };
    // The server's checked copy when it kept one, else the link itself.
    let target = link_media::public_url(&file.url).unwrap_or_else(|| link.url.clone());
    frame(key, &target, theme)
        .test_support()
        .role(Role::Link)
        .aria_label(SharedString::from(name.clone()))
        .px_3()
        .py_2()
        .flex()
        .items_center()
        .gap_3()
        .child(ui::icon::icon(IconName::File, theme.muted))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .child(
                    div()
                        .text_sm()
                        .text_color(theme.link)
                        .truncate()
                        .child(name),
                )
                .child(caption(format!("link-{key}-detail"), detail, theme)),
        )
        .child(ui::icon::icon(IconName::ExternalLink, theme.muted))
        .into_any_element()
}

/// A link to a message in this app: who wrote it, where, when, and how it
/// starts (`MessageLinkCard.vue`). The message comes from your own history,
/// so one you never received says so. Clicking it jumps to the message.
fn message_card(
    key: &str,
    target: &MessageLink,
    ctx: &MessageCtx,
    client: &Rc<Client>,
    theme: &Theme,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let mut ask = false;
    client.change(|w| {
        ask = w.begin_linked_message(&target.conversation_id, &target.message_id);
        Default::default()
    });
    if ask {
        let (id, m, i) = (
            target.message_id.clone(),
            ctx.message.clone(),
            target.message_id.clone(),
        );
        let done = client.call(
            move |api| async move { api.locate_message(&id).await },
            move |w, result| w.linked_message_answered(&m, &i, result),
        );
        window
            .spawn(cx, async move |cx| {
                let _ = done.await;
                let _ = cx.update(|window, _| window.refresh());
            })
            .detach();
    }
    let w = client.read();
    let state = w.linked_message(&target.conversation_id, &target.message_id);
    let found = match &state {
        Some(Linked::Found(m)) => Some(m.as_ref().clone()),
        _ => None,
    };
    let (top, bottom, label): (AnyElement, AnyElement, String) = match (&state, &found) {
        (_, Some(m)) => {
            let author = author(&w, m);
            let place = place(&w, &m.conversation_id);
            let when = when(&m.created_at);
            let excerpt = excerpt(m);
            let label = t!(
                "link-previews-message-label",
                author = author,
                place = place,
                text = excerpt
            );
            (
                div()
                    .flex()
                    .items_baseline()
                    .gap_1p5()
                    .text_xs()
                    .text_color(theme.muted)
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.text)
                            .truncate()
                            .child(author),
                    )
                    .child(div().truncate().child(place))
                    .child(div().flex_none().child(when))
                    .into_any_element(),
                div()
                    .id(ElementId::Name(format!("link-{key}-excerpt").into()))
                    .test_support()
                    .aria_label(SharedString::from(excerpt.clone()))
                    .text_sm()
                    .text_color(if m.deleted == Some(true) {
                        theme.muted
                    } else {
                        theme.text
                    })
                    .when(m.deleted == Some(true), |d| d.italic())
                    .truncate()
                    .child(excerpt)
                    .into_any_element(),
                label,
            )
        }
        (Some(Linked::Missing | Linked::Failed), _) => {
            let why = if state == Some(Linked::Failed) {
                t!("link-previews-message-failed").to_string()
            } else {
                t!("link-previews-message-unavailable").to_string()
            };
            (
                caption(
                    format!("link-{key}-kind"),
                    t!("link-previews-message").to_string(),
                    theme,
                ),
                caption(format!("link-{key}-excerpt"), why.clone(), theme),
                why,
            )
        }
        _ => (
            div()
                .h(px(10.))
                .w(px(160.))
                .rounded_sm()
                .bg(theme.hover)
                .into_any_element(),
            div()
                .h(px(10.))
                .w(px(256.))
                .rounded_sm()
                .bg(theme.hover)
                .into_any_element(),
            t!("link-previews-message").to_string(),
        ),
    };
    drop(w);
    let session = ctx.session.clone();
    let target = target.clone();
    let hover = theme.hover;
    div()
        .id(ElementId::Name(format!("link-{key}").into()))
        .test_support()
        .role(Role::Link)
        .aria_label(SharedString::from(label))
        .max_w(px(CARD_W))
        .w_full()
        .h(px(MESSAGE_CARD_H))
        .mt_1()
        .pr_3()
        .rounded_md()
        .border_1()
        .border_color(theme.rail)
        .bg(theme.sidebar)
        .overflow_hidden()
        .flex()
        .gap_2p5()
        .cursor_pointer()
        .hover(move |d| d.bg(hover))
        .on_click(move |_, window, cx| match &found {
            Some(m) => crate::timeline::reveal(&session, &m.conversation_id, &m.id, window, cx),
            None => crate::features::text::open_message_link(&session, &target, window, cx),
        })
        .child(div().w(px(3.)).flex_none().bg(theme.accent.opacity(0.6)))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .justify_center()
                .gap_1()
                .child(top)
                .child(bottom),
        )
        .into_any_element()
}

/// Why a preview shows no content (`link_media::reason_key`).
fn reason_text(key: &str) -> String {
    match key {
        "unsupported-type" => t!("link-previews-reason-unsupported-type").to_string(),
        "type-mismatch" => t!("link-previews-reason-type-mismatch").to_string(),
        "too-large" => t!("link-previews-reason-too-large").to_string(),
        "invalid-file" => t!("link-previews-reason-invalid-file").to_string(),
        "fetch-failed" => t!("link-previews-reason-fetch-failed").to_string(),
        "busy" => t!("link-previews-reason-busy").to_string(),
        "storage-unavailable" => t!("link-previews-reason-storage-unavailable").to_string(),
        "no-oembed" => t!("link-previews-reason-no-oembed").to_string(),
        "restricted" => t!("link-previews-reason-restricted").to_string(),
        _ => t!("link-previews-reason-unavailable").to_string(),
    }
}

/// A detected file type (`link_media::file_type_key`).
fn type_text(key: &str) -> String {
    match key {
        "pdf" => t!("link-previews-type-pdf").to_string(),
        "doc" => t!("link-previews-type-doc").to_string(),
        "docx" => t!("link-previews-type-docx").to_string(),
        "zip" => t!("link-previews-type-zip").to_string(),
        "rar" => t!("link-previews-type-rar").to_string(),
        "seven-zip" => t!("link-previews-type-seven-zip").to_string(),
        "tar" => t!("link-previews-type-tar").to_string(),
        "gzip" => t!("link-previews-type-gzip").to_string(),
        "text" => t!("link-previews-type-text").to_string(),
        _ => t!("link-previews-type-file").to_string(),
    }
}

fn author(w: &ircord_client::store::Workspace, m: &Message) -> String {
    let network = w
        .conversation(&m.conversation_id)
        .map(|c| c.network_id.clone())
        .unwrap_or_default();
    w.display_name(&m.sender_id, &network)
}

/// `#channel`, a group's name, or `@nick` for a direct message.
fn place(w: &ircord_client::store::Workspace, conversation: &str) -> String {
    let Some(c) = w.conversation(conversation) else {
        return String::new();
    };
    match c.kind {
        ConversationKind::Channel if c.party.is_none() => format!("#{}", c.name),
        ConversationKind::Channel => c.name.clone(),
        _ => format!("@{}", c.name),
    }
}

fn when(created_at: &str) -> String {
    use chrono::{Local, TimeZone};
    let ms = ircord_client::store::parse_time(created_at);
    Local
        .timestamp_millis_opt(ms)
        .single()
        .map(|t| t.format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_default()
}

fn excerpt(m: &Message) -> String {
    if m.deleted == Some(true) {
        return t!("link-previews-message-deleted").to_string();
    }
    let text = ircord_client::format::strip(&m.text);
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if !text.is_empty() {
        return if text.chars().count() > EXCERPT {
            let cut: String = text.chars().take(EXCERPT - 1).collect();
            format!("{cut}…")
        } else {
            text
        };
    }
    m.gif
        .as_ref()
        .map(|g| g.title.clone())
        .filter(|t| !t.is_empty())
        .or_else(|| {
            m.attachments
                .as_ref()
                .and_then(|a| a.first())
                .or(m.attachment.as_ref())
                .map(|a| a.name.clone())
        })
        .unwrap_or_else(|| t!("link-previews-message-no-text").to_string())
}

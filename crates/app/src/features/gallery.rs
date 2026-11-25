//! The channel gallery (package FD3): every picture and GIF posted in a
//! channel, newest first, in a panel beside it. The web client's gallery
//! dialog read a dedicated `/gallery` route that the server has since
//! folded into its link catalog, so this lists the channel's image and GIF
//! links from `/links` (with the same previews, from the server's media
//! cache, as the timeline and the links view). A picture opens the viewer
//! on the whole gallery; the line under it jumps to its post. Sensitive
//! channels cover the pictures until shown.
//!
//! It opens from the conversation header's gallery button, the channel's
//! menu and a key, and reads the list again each time it opens.

use crate::menu_order;
use crate::ui::focus::Reachable;
use std::rc::Rc;

use gpui_kit::TestSupportExt;
use gpui_kit::prelude::*;
use gpui_kit::*;
use ircord_client::Client;
use ircord_client::api::links::{Cover, LinkFilter};
use ircord_client::contract::LinkCard;
use ircord_client::store::Connection;
use ircord_client::store::link_media::Preview;

use crate::features::links::{
    self, List, ago, author, cover_view, covered, is_channel, jump, picture, place, preview, still,
};
use crate::features::viewer::{self, Media};
use crate::registry::{self, ConversationCtx, KeyDefault, MenuItem, Panel, PanelCtx};
use crate::route::Route;
use crate::session::{Session, SessionEvent};
use crate::sidebar::layout;
use crate::theme;
use crate::ui;
use crate::ui::icon::IconName;

gpui_kit::actions!(
    gallery,
    [
        /// Open (or close) the gallery beside the open channel.
        ToggleGallery,
    ]
);

/// The panel's ID.
pub const PANEL: &str = "gallery";
/// A tile's side.
const TILE: f32 = 112.0;

pub fn init(cx: &mut App) {
    cx.on_action(|_: &ToggleGallery, cx| {
        if let Some(session) = layout::session(cx) {
            let open = session.read(cx).panel.as_deref() == Some(PANEL);
            toggle(&session, !open, cx);
        }
    });
    registry::register(cx, |r| {
        r.panels
            .add(Panel::new(PANEL, |ctx: &PanelCtx, window, cx| {
                panel(ctx, window, cx)
            }));
        r.conversation_menu.add(
            MenuItem::new(
                "gallery",
                t!("links-view-gallery"),
                |ctx: &ConversationCtx, _, cx| {
                    ctx.session
                        .update(cx, |s, cx| s.select_conversation(&ctx.conversation, cx));
                    toggle(&ctx.session, true, cx);
                },
            )
            .icon(IconName::GalleryVerticalEnd)
            .order(menu_order::conversation::GALLERY)
            .when(|ctx: &ConversationCtx, cx: &App| {
                is_channel(&ctx.session, &ctx.conversation, cx)
            }),
        );
        r.keys.add(KeyDefault::new(
            "gallery",
            t!("links-view-gallery-key"),
            "",
            Some("Root"),
            ToggleGallery,
        ));
    });
}

/// Open (or close) the gallery beside the open channel.
pub fn toggle(session: &Entity<Session>, show: bool, cx: &mut App) {
    let channel = match session.read(cx).route() {
        Some(Route::Conversation(id)) => is_channel(session, &id, cx),
        _ => false,
    };
    if show && !channel {
        return;
    }
    session.update(cx, |s, cx| {
        s.set_panel(show.then(|| SharedString::from(PANEL)), cx)
    });
}

/// The gallery of the channel beside it, kept while that channel shows.
struct Shown(Entity<GalleryView>);
impl Global for Shown {}

fn panel(ctx: &PanelCtx, window: &mut Window, cx: &mut App) -> AnyElement {
    let theme = theme::get(cx).clone();
    let Some(conversation) = ctx
        .conversation
        .clone()
        .filter(|c| is_channel(&ctx.session, c, cx))
    else {
        return ui::empty::empty(
            "gallery-not-channel",
            t!("links-view-gallery-not-channel"),
            None,
            &theme,
        )
        .into_any_element();
    };
    let view = match cx.try_global::<Shown>() {
        Some(Shown(view))
            if view.read(cx).session == ctx.session
                && view.read(cx).conversation == conversation =>
        {
            view.clone()
        }
        _ => {
            let session = ctx.session.clone();
            let view = cx.new(|cx| GalleryView::new(session, conversation, window, cx));
            cx.set_global(Shown(view.clone()));
            view
        }
    };
    view.into_any_element()
}

pub struct GalleryView {
    session: Entity<Session>,
    conversation: String,
    list: List,
    showing: bool,
    _subscription: Subscription,
}

impl GalleryView {
    fn new(
        session: Entity<Session>,
        conversation: String,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let subscription =
            cx.subscribe(&session, |this, _, event: &SessionEvent, cx| match event {
                SessionEvent::Route => {
                    let showing = this.is_showing(cx);
                    if showing && !this.showing {
                        this.showing = true;
                        this.reload(cx);
                    }
                    this.showing = showing;
                }
                SessionEvent::Connection => this.ensure_loaded(cx),
                _ => {}
            });
        let mut this = Self {
            session,
            conversation,
            list: List::default(),
            showing: true,
            _subscription: subscription,
        };
        this.reload(cx);
        this
    }

    fn is_showing(&self, cx: &App) -> bool {
        let session = self.session.read(cx);
        session.panel.as_deref() == Some(PANEL)
            && session.route() == Some(Route::Conversation(self.conversation.clone()))
    }

    fn filter(&self) -> LinkFilter {
        LinkFilter::pictures(&self.conversation)
    }

    fn live(&self, cx: &App) -> Option<Rc<Client>> {
        let client = self.session.read(cx).client()?.clone();
        let live = client.read().connection == Some(Connection::Live);
        (live && self.showing).then_some(client)
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        if let Some(client) = self.live(cx) {
            let filter = self.filter();
            List::load(self, |v| &mut v.list, &client, filter, false, cx);
        }
    }

    fn ensure_loaded(&mut self, cx: &mut Context<Self>) {
        if !self.list.loaded && !self.list.loading {
            self.reload(cx);
        }
        cx.notify();
    }

    fn load_more(&mut self, cx: &mut Context<Self>) {
        if let Some(client) = self.session.read(cx).client().cloned() {
            let filter = self.filter();
            List::load(self, |v| &mut v.list, &client, filter, true, cx);
        }
    }
}

/// One tile: the card, its first post, and what its preview shows.
struct Tile {
    card: LinkCard,
    picture: Option<Media>,
    failed: bool,
    cover: Option<Cover>,
}

impl Render for GalleryView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme::get(cx).clone();
        let Some(client) = self.session.read(cx).client().cloned() else {
            return div().into_any_element();
        };
        let tiles: Vec<Tile> = self
            .list
            .items
            .clone()
            .into_iter()
            .filter_map(|card| {
                let source = card.sources.first()?.clone();
                let state = preview(&client, &source, &card.url, window, cx);
                let (picture, failed) = match &state {
                    Some(Preview::Ready { response, .. }) => {
                        let p = picture(response);
                        let failed = p.is_none();
                        (p, failed)
                    }
                    Some(Preview::Failed { .. }) => (None, true),
                    _ => (None, false),
                };
                let cover = picture
                    .as_ref()
                    .and_then(|_| covered(&client, &source, &card.url, cx));
                Some(Tile {
                    card,
                    picture,
                    failed,
                    cover,
                })
            })
            .collect();
        // The viewer pages through the pictures that are shown.
        let gallery: Vec<(usize, Media)> = tiles
            .iter()
            .enumerate()
            .filter(|(_, t)| t.cover.is_none())
            .filter_map(|(i, t)| Some((i, t.picture.clone()?)))
            .collect();
        let media: Vec<Media> = gallery.iter().map(|(_, m)| m.clone()).collect();

        let w = client.read();
        let channel = place(&w, &self.conversation);
        let count = self.list.items.len();
        let l = &self.list;
        let body: AnyElement = if !l.loaded && l.error.is_some() {
            let text = t!(
                "links-view-gallery-failed",
                error = l.error.clone().unwrap_or_default()
            );
            div()
                .id("gallery-failed")
                .test_support()
                .role(Role::Alert)
                .aria_label(SharedString::from(text.clone()))
                .p_4()
                .flex()
                .flex_col()
                .items_center()
                .gap_2()
                .text_sm()
                .text_color(theme.muted)
                .child(text)
                .child(crate::features::inbox::link_button(
                    "gallery-retry",
                    t!("links-view-try-again"),
                    &theme,
                    cx.listener(|this, _, _, cx| this.reload(cx)),
                ))
                .into_any_element()
        } else if !l.loaded {
            crate::features::inbox::loading(
                "gallery-loading",
                t!("links-view-gallery-loading"),
                &theme,
            )
        } else if tiles.is_empty() {
            ui::empty::empty(
                "gallery-empty",
                t!("links-view-gallery-empty", channel = channel.clone()),
                None,
                &theme,
            )
            .into_any_element()
        } else {
            let total = media.len();
            let cells = tiles.iter().enumerate().map(|(i, tile)| {
                let source = tile.card.sources.first().cloned().unwrap_or_default();
                let who = author(&w, &source);
                let time = ago(&source.posted_at);
                let id = format!("gallery-item-{i}");
                let at = gallery.iter().position(|(j, _)| *j == i);
                let picture: AnyElement = match (&tile.cover, &tile.picture) {
                    (Some(why), _) => {
                        cover_view(id.clone(), *why, &source, &tile.card.url, true, &theme)
                    }
                    (None, Some(m)) => still(m),
                    (None, None) => div()
                        .size_full()
                        .p_1()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_xs()
                        .text_color(theme.faint)
                        .when(tile.failed, |d| {
                            d.child(
                                div()
                                    .w_full()
                                    .text_center()
                                    .child(t!("links-view-preview-failed")),
                            )
                        })
                        .into_any_element(),
                };
                let label = match at {
                    Some(at) => t!(
                        "links-view-gallery-item",
                        index = at + 1,
                        total = total,
                        author = who.clone(),
                        time = time.clone()
                    ),
                    None => t!("links-view-gallery-picture", author = who.clone()),
                };
                let (session, items) = (self.session.clone(), media.clone());
                let (jump_session, jump_source) = (self.session.clone(), source.clone());
                let jump_label = t!(
                    "links-view-jump",
                    author = who.clone(),
                    place = channel.clone()
                );
                div()
                    .w(px(TILE))
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .child(
                        div()
                            .id(ElementId::Name(id.clone().into()))
                            .test_support()
                            .role(Role::Button)
                            .reachable(&theme)
                            .aria_label(SharedString::from(label))
                            .size(px(TILE))
                            .rounded_md()
                            .overflow_hidden()
                            .bg(theme.input)
                            .when(at.is_some(), |d| d.cursor_pointer())
                            .child(picture)
                            .on_click(move |_, window, cx| {
                                if let Some(at) = at {
                                    viewer::open(&session, items.clone(), at, window, cx);
                                }
                            }),
                    )
                    .child(
                        div()
                            .id(ElementId::Name(format!("{id}-source").into()))
                            .test_support()
                            .role(Role::Button)
                            .reachable(&theme)
                            .aria_label(SharedString::from(jump_label))
                            .text_xs()
                            .truncate()
                            .cursor_pointer()
                            .text_color(theme.muted)
                            .hover({
                                let text = theme.text;
                                move |d| d.text_color(text)
                            })
                            .child(format!("{who} · {time}"))
                            .on_click(move |_, window, cx| {
                                jump(&jump_session, &jump_source, window, cx)
                            }),
                    )
            });
            let more = crate::features::inbox::more(
                "gallery-load-more",
                t!("links-view-gallery-load-older"),
                l.cursor.is_some(),
                l.loading_more,
                l.more_error.as_deref(),
                &theme,
                cx.listener(|this, _, _, cx| this.load_more(cx)),
            );
            div()
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    div()
                        .id("gallery-grid")
                        .test_support()
                        .role(Role::List)
                        .aria_label(SharedString::from(t!(
                            "links-view-gallery-of",
                            channel = channel.clone()
                        )))
                        .flex()
                        .flex_wrap()
                        .gap_2()
                        .children(cells),
                )
                .children(more)
                .into_any_element()
        };
        let meta: Option<String> = (count > 0).then(|| {
            if l.cursor.is_some() {
                t!("links-view-gallery-count-more", count = count)
            } else if count == 1 {
                t!("links-view-gallery-count-one").to_string()
            } else {
                t!("links-view-gallery-count", count = count)
            }
        });
        let indexing = l.indexing;
        drop(w);

        let (links_session, links_conversation) = (self.session.clone(), self.conversation.clone());
        let close_session = self.session.clone();
        div()
            .id("gallery-panel")
            .test_support()
            .role(Role::Region)
            .aria_label(SharedString::from(t!(
                "links-view-gallery-of",
                channel = channel.clone()
            )))
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.sidebar)
            .child(
                div()
                    .h(px(48.))
                    .flex_none()
                    .px_3()
                    .flex()
                    .items_center()
                    .gap_2()
                    .border_b_1()
                    .border_color(theme.rail)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .truncate()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(theme.text)
                                    .child(t!("links-view-gallery")),
                            )
                            .when_some(meta, |d, meta| {
                                d.child(
                                    div()
                                        .id("gallery-meta")
                                        .test_support()
                                        .role(Role::Status)
                                        .aria_label(SharedString::from(meta.clone()))
                                        .text_xs()
                                        .text_color(theme.muted)
                                        .child(meta),
                                )
                            }),
                    )
                    .child(ui::icon::button(
                        "gallery-all-links",
                        IconName::Globe,
                        t!("links-view-gallery-all-links"),
                        &theme,
                        move |_, _, cx| links::open(&links_session, Some(&links_conversation), cx),
                    ))
                    .child(ui::icon::button(
                        "gallery-close",
                        IconName::Close,
                        t!("links-view-gallery-close"),
                        &theme,
                        move |_, _, cx| toggle(&close_session, false, cx),
                    )),
            )
            .when(indexing, |d| {
                d.child(
                    div()
                        .id("gallery-indexing")
                        .test_support()
                        .role(Role::Status)
                        .aria_label(t!("links-view-indexing"))
                        .px_3()
                        .pt_2()
                        .text_xs()
                        .text_color(theme.muted)
                        .child(t!("links-view-indexing")),
                )
            })
            .child(
                div()
                    .id("gallery-body")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p_3()
                    .child(body),
            )
            .into_any_element()
    }
}

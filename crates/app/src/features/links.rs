//! The links view (package FD3), after `LinksPage.vue` and
//! `LinkCatalogCard.vue` in the web client: the links posted in your
//! channels, grouped by what they point to, newest post first. It lists the
//! channel it was opened from, the channels you choose, or every channel,
//! narrowed by text, type, network, site and the days they were posted.
//! Filters are typed into the box as `in:#general`, `type:images`,
//! `host:example.com`, `since:2026-11-08` (or started from the buttons
//! under it) and show as removable chips.
//!
//! Each card shows a preview from the server's link-media service (the
//! request and cache the timeline uses, so the linked site never sees you),
//! its site and title, what the post said, who posted it where, and how
//! often it was posted. A card jumps to its newest post; "Posted N times"
//! lists the others. Sensitive channels cover the preview until shown.
//!
//! It opens from the row under the conversation list, a channel's menu
//! ("Links in this channel") and a key. The list is read again each time
//! the view opens, so a link posted meanwhile is never missing.
//!
//! The channel gallery (`gallery.rs`) shares the list, the previews and the
//! covers below.

use crate::ui::focus::Reachable;
use std::collections::HashSet;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::TestSupportExt;
use gpui_kit::component::input::{
    Backspace, Escape, Input, InputEvent, InputState, MoveDown, MoveUp,
};
use gpui_kit::prelude::*;
use gpui_kit::*;
use ircord_client::Client;
use ircord_client::api::links::{self as catalog, Cover, DateError, LinkFilter, Token};
use ircord_client::contract::{
    ConversationKind, LinkCard, LinkCardKind, LinkMediaAssetKind, LinkMediaResponse,
    LinkMediaResponseKind, LinkPage, LinkSource,
};
use ircord_client::documents;
use ircord_client::store::link_media::{self, Preview};
use ircord_client::store::{Connection, Workspace};

use crate::features::viewer::Media;
use crate::registry::{
    self, ConversationCtx, KeyDefault, MenuItem, Pane, PaneCtx, SidebarCtx, Slot,
};
use crate::route::{PaneKind, Route};
use crate::session::{Session, SessionEvent};
use crate::sidebar::layout;
use crate::theme::{self, Theme};
use crate::ui;
use crate::ui::icon::IconName;

gpui_kit::actions!(
    links,
    [
        /// Open the links view.
        OpenLinks,
    ]
);

/// How long typing pauses before the list is read for it.
const DEBOUNCE: Duration = Duration::from_millis(220);
/// A card's width, and its preview's 16:9 height.
const CARD_W: f32 = 280.0;
const PREVIEW_H: f32 = 158.0;

pub fn init(cx: &mut App) {
    cx.on_action(|_: &OpenLinks, cx| {
        if let Some(session) = layout::session(cx) {
            open(&session, None, cx);
        }
    });
    registry::register(cx, |r| {
        r.panes
            .add(Pane::new(PaneKind::Links, |ctx: &PaneCtx, window, cx| {
                view(ctx, window, cx).into_any_element()
            }));
        r.sidebar_footer.add(
            Slot::new("links", |ctx: &SidebarCtx, _, cx| {
                Some(sidebar_row(&ctx.session, cx))
            })
            .order(-20),
        );
        r.conversation_menu.add(
            MenuItem::new(
                "links-in-channel",
                t!("links-view-in-channel"),
                |ctx: &ConversationCtx, _, cx| open(&ctx.session, Some(&ctx.conversation), cx),
            )
            .icon(IconName::Globe)
            .order(60)
            .when(|ctx: &ConversationCtx, cx: &App| {
                is_channel(&ctx.session, &ctx.conversation, cx)
            }),
        );
        r.keys.add(KeyDefault::new(
            "links",
            t!("links-view-key"),
            "",
            Some("Root"),
            OpenLinks,
        ));
    });
}

/// Show the links view: of `conversation` when it is a channel, else of
/// every channel.
pub fn open(session: &Entity<Session>, conversation: Option<&str>, cx: &mut App) {
    let scope = conversation
        .filter(|id| is_channel(session, id, cx))
        .map(str::to_string);
    session.update(cx, |s, cx| s.navigate(Route::Links(scope), cx));
}

/// A channel (not a party's), which is what the catalog holds.
pub(crate) fn is_channel(session: &Entity<Session>, conversation: &str, cx: &App) -> bool {
    session.read(cx).client().is_some_and(|c| {
        c.read()
            .conversation(conversation)
            .is_some_and(|c| c.kind == ConversationKind::Channel && c.party.is_none())
    })
}

fn sidebar_row(session: &Entity<Session>, cx: &App) -> AnyElement {
    let theme = theme::get(cx);
    let active = matches!(session.read(cx).route(), Some(Route::Links(_)));
    let session = session.clone();
    let hover = theme.hover;
    div()
        .id("sidebar-links")
        .test_support()
        .role(Role::Button)
        .reachable(theme)
        .aria_label(t!("links-view-title"))
        .aria_selected(active)
        .mx_2()
        .px_2()
        .py_1()
        .rounded_md()
        .flex()
        .items_center()
        .gap_2()
        .text_sm()
        .cursor_pointer()
        .text_color(if active { theme.text } else { theme.muted })
        .when(active, |d| d.bg(theme.selected))
        .hover(move |d| d.bg(hover))
        .child(ui::icon::icon(IconName::Globe, theme.muted))
        .child(div().flex_1().truncate().child(t!("links-view-title")))
        .on_click(move |_, _, cx| open(&session, None, cx))
        .into_any_element()
}

// ---- Shared with the gallery: one paged list of the catalog ----

/// A list read from the catalog page by page, for one filter.
#[derive(Default)]
pub(crate) struct List {
    /// What `items` are for; `None` before the first read.
    pub filter: Option<LinkFilter>,
    pub items: Vec<LinkCard>,
    pub cursor: Option<String>,
    pub indexing: bool,
    pub loaded: bool,
    pub loading: bool,
    pub error: Option<String>,
    pub loading_more: bool,
    pub more_error: Option<String>,
    generation: u64,
}

impl List {
    /// Start reading `filter`'s first page, or (with `more`) the next one.
    /// Returns the request's generation and cursor, or `None` when there is
    /// nothing to read.
    fn start(&mut self, filter: &LinkFilter, more: bool) -> Option<(u64, Option<String>)> {
        if more {
            if self.loading_more || self.loading || self.filter.as_ref() != Some(filter) {
                return None;
            }
            let cursor = self.cursor.clone()?;
            self.loading_more = true;
            self.more_error = None;
            return Some((self.generation, Some(cursor)));
        }
        self.generation += 1;
        if self.filter.as_ref() != Some(filter) {
            self.items.clear();
            self.cursor = None;
            self.loaded = false;
        }
        self.filter = Some(filter.clone());
        self.loading = true;
        self.loading_more = false;
        self.error = None;
        self.more_error = None;
        Some((self.generation, None))
    }

    fn answered(&mut self, generation: u64, more: bool, result: Result<LinkPage, String>) {
        if generation != self.generation {
            return;
        }
        if more {
            self.loading_more = false;
            match result {
                Ok(page) => {
                    self.cursor = page.next_cursor;
                    catalog::merge(&mut self.items, page.items);
                }
                Err(e) => self.more_error = Some(e),
            }
            return;
        }
        self.loading = false;
        match result {
            Ok(page) => {
                self.items = page.items;
                self.cursor = page.next_cursor;
                self.indexing = page.indexing;
                self.loaded = true;
            }
            Err(e) => self.error = Some(e),
        }
    }

    /// Read the first page (or the next one) of `filter` with `client`,
    /// then hand the answer to the list `of` the view.
    pub fn load<V: 'static>(
        view: &mut V,
        of: fn(&mut V) -> &mut List,
        client: &Rc<Client>,
        filter: LinkFilter,
        more: bool,
        cx: &mut Context<V>,
    ) {
        let Some((generation, cursor)) = of(view).start(&filter, more) else {
            return;
        };
        let request = client
            .request(move |api| async move { api.list_links(&filter, cursor.as_deref()).await });
        cx.spawn(async move |this, cx| {
            let result = match request.await {
                Ok(Ok(page)) => Ok(page),
                Ok(Err(e)) => Err(e.to_string()),
                Err(e) => Err(e.to_string()),
            };
            this.update(cx, |view, cx| {
                of(view).answered(generation, more, result);
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }
}

/// Clients whose preferences these views watch, for the covers.
#[derive(Default)]
struct Watched(HashSet<usize>);
impl Global for Watched {}

/// Covered previews shown anyway, by message and link.
#[derive(Default)]
struct Revealed(HashSet<String>);
impl Global for Revealed {}

fn reveal_key(source: &LinkSource, url: &str) -> String {
    format!("{}\n{url}", source.message_id)
}

/// Why `source`'s media is covered right now, if it is.
pub(crate) fn covered(
    client: &Rc<Client>,
    source: &LinkSource,
    url: &str,
    cx: &mut App,
) -> Option<Cover> {
    let key = Rc::as_ptr(client) as usize;
    if cx.default_global::<Watched>().0.insert(key) {
        client.watch_document(&documents::PREFERENCES);
    }
    if cx
        .try_global::<Revealed>()
        .is_some_and(|r| r.0.contains(&reveal_key(source, url)))
    {
        return None;
    }
    let allowed = {
        let w = client.read();
        w.documents
            .preferences
            .value()
            .and_then(|p| p.preferences.as_ref())
            .is_some_and(|p| {
                p.show_nsfw == Some(true)
                    || p.nsfw_networks
                        .as_ref()
                        .and_then(|n| n.get(&source.network_id))
                        .copied()
                        == Some(true)
            })
    };
    catalog::cover(source, allowed)
}

/// The cover over a sensitive preview, with a button that shows it.
pub(crate) fn cover_view(
    id: String,
    why: Cover,
    source: &LinkSource,
    url: &str,
    compact: bool,
    theme: &Theme,
) -> AnyElement {
    let (title, hint): (&'static str, &'static str) = match why {
        Cover::Nsfw => (t!("links-view-nsfw"), t!("links-view-nsfw-hint")),
        Cover::Unknown => (
            t!("links-view-unknown-classification"),
            t!("links-view-unknown-classification-hint"),
        ),
    };
    let key = reveal_key(source, url);
    div()
        .id(ElementId::Name(format!("{id}-cover").into()))
        .test_support()
        .role(Role::Group)
        .aria_label(title)
        .size_full()
        .p_2()
        .bg(theme.sidebar)
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap_1()
        .text_center()
        .child(
            div()
                .text_sm()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.text)
                .child(title),
        )
        .when(!compact, |d| {
            d.child(div().text_xs().text_color(theme.muted).child(hint))
        })
        .child(
            div()
                .id(ElementId::Name(format!("{id}-show").into()))
                .test_support()
                .role(Role::Button)
                .reachable(theme)
                .aria_label(t!("links-view-show"))
                .px_2()
                .py_0p5()
                .rounded_md()
                .cursor_pointer()
                .text_sm()
                .text_color(theme.link)
                .hover({
                    let hover = theme.hover;
                    move |d| d.bg(hover)
                })
                .child(t!("links-view-show"))
                .on_click(move |_, window, cx| {
                    cx.stop_propagation();
                    cx.default_global::<Revealed>().0.insert(key.clone());
                    window.refresh();
                }),
        )
        .into_any_element()
}

/// What the server's link-media service knows of `url` in `source`'s
/// message, asking it when nothing is known yet (or the answer expired).
pub(crate) fn preview(
    client: &Rc<Client>,
    source: &LinkSource,
    url: &str,
    window: &mut Window,
    cx: &mut App,
) -> Option<Preview> {
    let now = ircord_client::events::now_ms();
    let mut ask = false;
    client.change(|w| {
        ask = w.begin_link_preview(&source.message_id, url, now);
        Default::default()
    });
    if ask {
        let (conversation, message, link) = (
            source.conversation_id.clone(),
            source.message_id.clone(),
            url.to_string(),
        );
        let (m, u) = (message.clone(), link.clone());
        let done = client.call(
            move |api| link_media::resolve(api, conversation, message, link),
            move |w, result| {
                w.link_preview_answered(&m, &u, result, ircord_client::events::now_ms())
            },
        );
        window
            .spawn(cx, async move |cx| {
                let _ = done.await;
                let _ = cx.update(|window, _| window.refresh());
            })
            .detach();
    }
    client.read().link_preview(&source.message_id, url).cloned()
}

/// The picture a preview shows, if any: the first picture, else the page's
/// image, else a player's still.
pub(crate) fn picture(r: &LinkMediaResponse) -> Option<Media> {
    let title = r.title.clone().unwrap_or_default();
    if let Some(asset) = r
        .assets
        .iter()
        .find(|a| a.kind == LinkMediaAssetKind::Image)
        && let Some(url) = link_media::public_url(&asset.url)
    {
        return Some(Media {
            url,
            name: title,
            width: asset.width.unwrap_or(0).max(0) as u32,
            height: asset.height.unwrap_or(0).max(0) as u32,
            content_type: asset.content_type.clone().unwrap_or_default(),
            size: None,
        });
    }
    let (url, width, height) = r
        .card
        .as_ref()
        .and_then(|c| Some((c.image.as_deref()?, c.image_width, c.image_height)))
        .or_else(|| {
            r.widget
                .as_ref()
                .and_then(|w| Some((w.thumbnail.as_deref()?, None, None)))
        })?;
    Some(Media {
        url: link_media::public_url(url)?,
        name: title,
        width: width.unwrap_or(0).max(0) as u32,
        height: height.unwrap_or(0).max(0) as u32,
        content_type: String::new(),
        size: None,
    })
}

/// A picture filling its box, a GIF held on its first frame.
pub(crate) fn still(media: &Media) -> AnyElement {
    let source = if media.is_gif() {
        crate::images::still(media.url.clone().into())
    } else {
        ImageSource::from(SharedUri::from(media.url.clone()))
    };
    img(source)
        .size_full()
        .object_fit(ObjectFit::Cover)
        .into_any_element()
}

/// The bottom of a card's preview: a shade with the open (or play) mark,
/// and on a still picture its site and title, after the web's catalog card.
fn open_mark(titled: Option<(String, String)>, play: bool, theme: &Theme) -> AnyElement {
    use crate::theme::{hex, hex_alpha};
    let shade = |top: f32, bottom: f32| {
        linear_gradient(
            180.,
            linear_color_stop(hex_alpha(0x000000, top), 0.),
            linear_color_stop(hex_alpha(0x000000, bottom), 1.),
        )
    };
    let mark = div()
        .flex_none()
        .size(px(32.))
        .rounded_full()
        .bg(theme.accent)
        .flex()
        .items_center()
        .justify_center()
        .child(ui::icon::icon(
            if play {
                IconName::Play
            } else {
                IconName::ExternalLink
            },
            theme.on_accent,
        ));
    let titled_shade = titled.is_some();
    div()
        .absolute()
        .left_0()
        .right_0()
        .bottom_0()
        .when(titled_shade, |d| d.h(relative(0.7)).bg(shade(0., 0.85)))
        .when(!titled_shade, |d| d.h(px(64.)).bg(shade(0., 0.55)))
        .p(px(10.))
        .flex()
        .items_end()
        .gap_3()
        .when(!titled_shade, |d| d.justify_end())
        .when_some(titled, |d, (site, title)| {
            d.child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .child(
                        div()
                            .text_xs()
                            .text_color(hex_alpha(0xffffff, 0.75))
                            .truncate()
                            .child(site),
                    )
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::BOLD)
                            .text_color(hex(0xffffff))
                            .line_clamp(2)
                            .child(title),
                    ),
            )
        })
        .child(mark)
        .into_any_element()
}

/// `#channel`, or that it is closed.
pub(crate) fn place(w: &Workspace, conversation: &str) -> String {
    match w.conversation(conversation) {
        Some(c) => format!("#{}", c.name.trim_start_matches('#')),
        None => t!("links-view-closed-channel").to_string(),
    }
}

/// Who posted `source`.
pub(crate) fn author(w: &Workspace, source: &LinkSource) -> String {
    let (sender, network) = (&source.sender_id, &source.network_id);
    if w.is_self(sender, network) || w.member(sender, network).is_some() {
        let name = w.display_name(sender, network);
        if !name.is_empty() {
            return name;
        }
    }
    t!("links-view-someone").to_string()
}

/// "3 days ago".
pub(crate) fn ago(at: &str) -> String {
    crate::settings::account::ago(at, chrono::Utc::now())
}

/// The day and time of an RFC 3339 time, in local time.
pub(crate) fn when(at: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(at)
        .map(|t| {
            t.with_timezone(&chrono::Local)
                .format("%-d %B %Y %H:%M")
                .to_string()
        })
        .unwrap_or_default()
}

/// Jump to `source`'s message, or say its channel is closed.
pub(crate) fn jump(
    session: &Entity<Session>,
    source: &LinkSource,
    window: &mut Window,
    cx: &mut App,
) {
    let open = session
        .read(cx)
        .client()
        .is_some_and(|c| c.read().conversation(&source.conversation_id).is_some());
    if !open {
        return ui::toast::info(window, cx, t!("links-view-channel-gone"));
    }
    crate::timeline::reveal(
        session,
        &source.conversation_id,
        &source.message_id,
        window,
        cx,
    );
}

fn kind_label(kind: LinkCardKind) -> &'static str {
    match kind {
        LinkCardKind::Link | LinkCardKind::Unknown => t!("links-view-kind-link"),
        LinkCardKind::Image => t!("links-view-kind-image"),
        LinkCardKind::Gif => t!("links-view-kind-gif"),
        LinkCardKind::Video => t!("links-view-kind-video"),
        LinkCardKind::Audio => t!("links-view-kind-audio"),
        LinkCardKind::Youtube => t!("links-view-kind-youtube"),
    }
}

// ---- The view ----

/// A removable chip for one chosen filter.
#[derive(Clone, Debug, PartialEq)]
enum Chip {
    In(String),
    Network,
    Kind(LinkCardKind),
    Host,
    Since,
    Until,
}

/// The view, kept across visits so the filters stay.
struct Page(Entity<LinksView>);
impl Global for Page {}

fn view(ctx: &PaneCtx, window: &mut Window, cx: &mut App) -> Entity<LinksView> {
    let scope = match &ctx.route {
        Route::Links(scope) => scope.clone(),
        _ => None,
    };
    let view = match cx.try_global::<Page>() {
        Some(Page(view)) if view.read(cx).session == ctx.session => view.clone(),
        _ => {
            let view = cx.new(|cx| LinksView::new(ctx.session.clone(), window, cx));
            cx.set_global(Page(view.clone()));
            view
        }
    };
    view.update(cx, |v, cx| v.opened_for(scope, cx));
    view
}

pub struct LinksView {
    session: Entity<Session>,
    /// The route's channel the filters were last set for.
    scope: Option<Option<String>>,
    chosen: Vec<String>,
    network: String,
    kinds: Vec<LinkCardKind>,
    host: String,
    since: String,
    until: String,
    /// The text searched for, once typing pauses.
    text: String,
    /// A typed filter that named nothing.
    notice: Option<String>,
    input: Entity<InputState>,
    list: List,
    selected: usize,
    showing: bool,
    /// Focus the filter box on the next draw (the view just opened).
    focus: bool,
    typing: u64,
    _subscriptions: Vec<Subscription>,
}

impl LinksView {
    fn new(session: Entity<Session>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(t!("links-view-filter")));
        let subscriptions = vec![
            cx.subscribe_in(
                &input,
                window,
                |this: &mut Self, input, event: &InputEvent, window, cx| match event {
                    InputEvent::Change => this.typed(input, window, cx),
                    InputEvent::PressEnter { .. } => this.enter(window, cx),
                    _ => {}
                },
            ),
            cx.subscribe(&session, |this, _, event: &SessionEvent, cx| match event {
                SessionEvent::Route => {
                    let showing = matches!(this.session.read(cx).route(), Some(Route::Links(_)));
                    if showing && !this.showing {
                        // Read again on every visit: links posted meanwhile show.
                        this.showing = true;
                        this.focus = true;
                        this.reload(cx);
                    }
                    this.showing = showing;
                }
                SessionEvent::Connection => {
                    this.ensure_loaded(cx);
                    cx.notify();
                }
                SessionEvent::Changed(changes) if changes.sidebar || changes.reset => {
                    this.ensure_loaded(cx);
                    cx.notify();
                }
                _ => {}
            }),
        ];
        Self {
            session,
            scope: None,
            chosen: Vec::new(),
            network: String::new(),
            kinds: Vec::new(),
            host: String::new(),
            since: String::new(),
            until: String::new(),
            text: String::new(),
            notice: None,
            input,
            list: List::default(),
            selected: 0,
            showing: false,
            focus: true,
            typing: 0,
            _subscriptions: subscriptions,
        }
    }

    /// The route opened the view for `scope`: a new channel starts from it.
    fn opened_for(&mut self, scope: Option<String>, cx: &mut Context<Self>) {
        if self.scope.as_ref() == Some(&scope) {
            return;
        }
        self.chosen = scope.clone().into_iter().collect();
        self.scope = Some(scope);
        self.selected = 0;
        self.showing = true;
        self.focus = true;
        self.reload(cx);
    }

    fn client(&self, cx: &App) -> Option<Rc<Client>> {
        self.session.read(cx).client().cloned()
    }

    fn live(&self, cx: &App) -> Option<Rc<Client>> {
        let client = self.client(cx)?;
        let live = client.read().connection == Some(Connection::Live);
        (live && self.showing).then_some(client)
    }

    /// The chosen channels that are still open channels.
    fn picked(&self, cx: &App) -> Vec<String> {
        let Some(client) = self.client(cx) else {
            return Vec::new();
        };
        let w = client.read();
        self.chosen
            .iter()
            .filter(|id| {
                w.conversation(id)
                    .is_some_and(|c| c.kind == ConversationKind::Channel && c.party.is_none())
            })
            .cloned()
            .collect()
    }

    /// The filter to read, or why a typed filter cannot be used.
    fn filter(&self, cx: &App) -> Result<LinkFilter, String> {
        let host =
            catalog::host_filter(&self.host).map_err(|_| t!("links-view-host-hint").to_string())?;
        let (since, until) = catalog::date_range(&self.since, &self.until).map_err(|e| {
            match e {
                DateError::Format => t!("links-view-date-format"),
                DateError::Order => t!("links-view-date-order"),
            }
            .to_string()
        })?;
        let conversations = self.picked(cx);
        Ok(LinkFilter {
            network: if conversations.is_empty() {
                self.network.clone()
            } else {
                String::new()
            },
            conversations,
            kinds: self.kinds.clone(),
            q: self.text.clone(),
            host,
            since,
            until,
        })
    }

    /// Read the first page again.
    fn reload(&mut self, cx: &mut Context<Self>) {
        let (Some(client), Ok(filter)) = (self.live(cx), self.filter(cx)) else {
            cx.notify();
            return;
        };
        List::load(self, |v| &mut v.list, &client, filter, false, cx);
    }

    /// Read the list if it is missing or for other filters.
    fn ensure_loaded(&mut self, cx: &mut Context<Self>) {
        let (Some(client), Ok(filter)) = (self.live(cx), self.filter(cx)) else {
            return;
        };
        let current = self.list.filter.as_ref() == Some(&filter);
        if !current || (!self.list.loaded && !self.list.loading && self.list.error.is_none()) {
            List::load(self, |v| &mut v.list, &client, filter, false, cx);
        }
    }

    fn load_more(&mut self, cx: &mut Context<Self>) {
        let (Some(client), Ok(filter)) = (self.client(cx), self.filter(cx)) else {
            return;
        };
        List::load(self, |v| &mut v.list, &client, filter, true, cx);
    }

    /// The filters changed.
    fn changed(&mut self, cx: &mut Context<Self>) {
        self.selected = 0;
        self.ensure_loaded(cx);
        cx.notify();
    }

    /// The filter box changed: take the finished tokens out of it as
    /// filters, and search for the text once typing pauses.
    fn typed(&mut self, input: &Entity<InputState>, window: &mut Window, cx: &mut Context<Self>) {
        let value = input.read(cx).value().to_string();
        let typed = catalog::parse_typed(&value);
        if !typed.tokens.is_empty() {
            self.notice = None;
            for token in typed.tokens {
                self.take(token, cx);
            }
            input.update(cx, |i, cx| i.set_value(typed.rest.clone(), window, cx));
            self.changed(cx);
        }
        if typed.text == self.text {
            return;
        }
        self.typing += 1;
        let typing = self.typing;
        let runtime = self.session.read(cx).runtime.clone();
        let settle = runtime.spawn(async { tokio::time::sleep(DEBOUNCE).await });
        cx.spawn(async move |this, cx| {
            let _ = settle.await;
            this.update(cx, |this, cx| {
                if this.typing == typing {
                    this.text = typed.text;
                    this.changed(cx);
                }
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// Apply one typed filter, or say it named nothing.
    fn take(&mut self, token: Token, cx: &App) {
        let Some(client) = self.client(cx) else {
            return;
        };
        let w = client.read();
        let unknown = |filter: &'static str, value: &str| {
            t!("links-view-unknown-filter", value = value, filter = filter).to_string()
        };
        match token {
            Token::In(name) => {
                let wanted = name.trim_start_matches('#');
                let found = w
                    .conversations
                    .iter()
                    .filter(|c| c.kind == ConversationKind::Channel && c.party.is_none())
                    .filter(|c| c.name.trim_start_matches('#').eq_ignore_ascii_case(wanted))
                    .min_by_key(|c| Some(&c.network_id) != self.session.read(cx).network.as_ref())
                    .map(|c| c.id.clone());
                match found {
                    Some(id) if self.chosen.contains(&id) => {}
                    Some(_) if self.chosen.len() >= catalog::MAX_CHANNELS => {
                        self.notice = Some(
                            t!(
                                "links-view-too-many-channels",
                                count = catalog::MAX_CHANNELS
                            )
                            .to_string(),
                        )
                    }
                    Some(id) => self.chosen.push(id),
                    None => self.notice = Some(unknown(t!("links-view-tag-in"), &name)),
                }
            }
            Token::Network(name) => {
                match w
                    .networks
                    .iter()
                    .find(|n| n.name.eq_ignore_ascii_case(&name) || n.id == name)
                {
                    Some(n) => self.network = n.id.clone(),
                    None => self.notice = Some(unknown(t!("links-view-tag-network"), &name)),
                }
            }
            Token::Type(name) => match catalog::parse_kind(&name) {
                Some(kind) if !self.kinds.contains(&kind) => self.kinds.push(kind),
                Some(_) => {}
                None => self.notice = Some(unknown(t!("links-view-tag-type"), &name)),
            },
            Token::Host(host) => self.host = host,
            Token::Since(day) => self.since = day,
            Token::Until(day) => self.until = day,
        }
    }

    fn chips(&self, cx: &App) -> Vec<Chip> {
        let mut chips: Vec<Chip> = self.picked(cx).into_iter().map(Chip::In).collect();
        if !self.network.is_empty() && chips.is_empty() {
            chips.push(Chip::Network);
        }
        chips.extend(self.kinds.iter().copied().map(Chip::Kind));
        for (on, chip) in [
            (!self.host.is_empty(), Chip::Host),
            (!self.since.is_empty(), Chip::Since),
            (!self.until.is_empty(), Chip::Until),
        ] {
            if on {
                chips.push(chip);
            }
        }
        chips
    }

    fn chip_text(&self, chip: &Chip, cx: &App) -> (&'static str, String) {
        let client = self.client(cx);
        let w = client.as_ref().map(|c| c.read());
        match chip {
            Chip::In(id) => (
                t!("links-view-tag-in"),
                w.map(|w| place(&w, id)).unwrap_or_default(),
            ),
            Chip::Network => (
                t!("links-view-tag-network"),
                w.and_then(|w| w.network(&self.network).map(|n| n.name.clone()))
                    .unwrap_or_else(|| self.network.clone()),
            ),
            Chip::Kind(kind) => (t!("links-view-tag-type"), kind_label(*kind).to_string()),
            Chip::Host => (t!("links-view-tag-host"), self.host.clone()),
            Chip::Since => (t!("links-view-tag-since"), self.since.clone()),
            Chip::Until => (t!("links-view-tag-until"), self.until.clone()),
        }
    }

    fn remove(&mut self, chip: &Chip, cx: &mut Context<Self>) {
        match chip {
            Chip::In(id) => self.chosen.retain(|c| c != id),
            Chip::Network => self.network.clear(),
            Chip::Kind(kind) => self.kinds.retain(|k| k != kind),
            Chip::Host => self.host.clear(),
            Chip::Since => self.since.clear(),
            Chip::Until => self.until.clear(),
        }
        self.notice = None;
        self.changed(cx);
    }

    fn toggle_kind(&mut self, kind: LinkCardKind, cx: &mut Context<Self>) {
        if self.kinds.contains(&kind) {
            self.kinds.retain(|k| *k != kind);
        } else {
            self.kinds.push(kind);
        }
        self.changed(cx);
    }

    fn clear(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.network.clear();
        self.kinds.clear();
        self.host.clear();
        self.since.clear();
        self.until.clear();
        self.text.clear();
        self.notice = None;
        self.input.update(cx, |i, cx| i.set_value("", window, cx));
        self.changed(cx);
    }

    /// Start typing a filter: `key:` at the end of the box.
    fn start(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) {
        let value = self.input.read(cx).value().to_string();
        let sep = if value.is_empty() || value.ends_with(' ') {
            ""
        } else {
            " "
        };
        let next = format!("{value}{sep}{key}:");
        self.input.update(cx, |i, cx| i.set_value(next, window, cx));
        self.input.focus_handle(cx).focus(window, cx);
    }

    fn enter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.filter(cx).is_err() || !self.list.loaded {
            return;
        }
        if let Some(source) = self
            .list
            .items
            .get(self.selected)
            .and_then(|c| c.sources.first())
            .cloned()
        {
            jump(&self.session, &source, window, cx);
        }
    }

    fn open_posts(&self, card: &LinkCard, window: &mut Window, cx: &mut App) {
        let title = catalog::link_title(&card.url);
        let sources = card.sources.clone();
        let earlier = card.count - card.sources.len() as i64;
        let session = self.session.clone();
        ui::dialog::open(
            window,
            cx,
            t!("links-view-posts-of", title = title),
            move |dialog, _, cx| {
                let theme = theme::get(cx).clone();
                let client = session.read(cx).client().cloned();
                let Some(client) = client else {
                    return dialog;
                };
                let w = client.read();
                let rows = sources.iter().enumerate().map(|(j, source)| {
                    let (author, place) = (author(&w, source), place(&w, &source.conversation_id));
                    let label = t!(
                        "links-view-jump",
                        author = author.clone(),
                        place = place.clone()
                    );
                    let (session, source) = (session.clone(), source.clone());
                    div()
                        .id(ElementId::Name(format!("links-post-{j}").into()))
                        .test_support()
                        .role(Role::Button)
                        .reachable(&theme)
                        .aria_label(SharedString::from(label))
                        .px_2()
                        .py_1p5()
                        .rounded_md()
                        .cursor_pointer()
                        .hover({
                            let hover = theme.hover;
                            move |d| d.bg(hover)
                        })
                        .flex()
                        .flex_col()
                        .gap_0p5()
                        .child(
                            div()
                                .flex()
                                .gap_2()
                                .text_sm()
                                .child(
                                    div()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(theme.text)
                                        .child(author),
                                )
                                .child(div().text_color(theme.muted).child(place))
                                .child(div().flex_1())
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(theme.faint)
                                        .child(when(&source.posted_at)),
                                ),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.muted)
                                .truncate()
                                .child(source.excerpt.clone()),
                        )
                        .on_click(move |_, window, cx| {
                            ui::dialog::close(window, cx);
                            jump(&session, &source, window, cx);
                        })
                });
                dialog.child(div().flex().flex_col().gap_0p5().children(rows).when(
                    earlier > 0,
                    |d| {
                        d.child(div().px_2().pt_1().text_xs().text_color(theme.muted).child(
                            if earlier == 1 {
                                t!("links-view-earlier-post-one").to_string()
                            } else {
                                t!("links-view-earlier-posts", count = earlier)
                            },
                        ))
                    },
                ))
            },
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn card(
        &self,
        i: usize,
        card: &LinkCard,
        shown: &Shown,
        w: &Workspace,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let source = card.sources.first();
        let response = match &shown.preview {
            Some(Preview::Ready { response, .. }) => Some(response.as_ref()),
            _ => None,
        };
        let title = response
            .and_then(|r| {
                r.card
                    .as_ref()
                    .map(|c| c.title.clone())
                    .filter(|t| !t.is_empty())
                    .or_else(|| r.title.clone())
                    .filter(|t| !t.trim().is_empty())
            })
            .unwrap_or_else(|| catalog::link_title(&card.url));
        let site = response
            .and_then(|r| r.card.as_ref().map(|c| c.site.clone()))
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| card.host.clone());
        let description = response.and_then(|r| {
            r.card
                .as_ref()
                .and_then(|c| c.description.clone())
                .or_else(|| r.description.clone())
                .filter(|d| !d.trim().is_empty())
        });
        let remark = source
            .map(|s| catalog::remark(&s.excerpt))
            .unwrap_or_default();
        let reason: Option<&'static str> = match &shown.preview {
            None | Some(Preview::Loading) => Some(t!("links-view-preview-loading")),
            Some(Preview::Failed { .. }) => Some(t!("links-view-preview-failed")),
            Some(Preview::Ready { expires_ms, .. })
                if expires_ms.is_some_and(|e| e <= ircord_client::events::now_ms()) =>
            {
                Some(t!("links-view-preview-expired"))
            }
            Some(Preview::Ready { response, .. })
                if response.kind == LinkMediaResponseKind::Card =>
            {
                response
                    .card
                    .as_ref()
                    .and_then(|c| c.reason)
                    .and_then(|r| link_media::reason_key(Some(r), None))
                    .map(|_| t!("links-view-preview-failed"))
            }
            _ => None,
        };
        let line: Option<String> = if !remark.is_empty() {
            Some(remark)
        } else if let Some(reason) = reason {
            Some(reason.to_string())
        } else if shown.cover.is_none() && shown.picture.is_none() {
            // The artwork already shows the description.
            None
        } else {
            description.clone()
        };
        let selected = i == self.selected;
        let id = format!("links-card-{i}");

        // A still picture carries the site and title on it; a card without
        // a picture draws them as its artwork. Players keep their still
        // clear, as on the web.
        let widget = response.is_some_and(|r| r.kind == LinkMediaResponseKind::Widget);
        let overlaid = shown.cover.is_none() && shown.picture.is_some() && !widget;
        let preview: AnyElement = match (&shown.cover, source, &shown.picture) {
            (Some(why), Some(source), _) => {
                cover_view(id.clone(), *why, source, &card.url, false, theme)
            }
            (None, _, Some(media)) => div()
                .size_full()
                .relative()
                .child(still(media))
                .child(open_mark(
                    overlaid.then(|| (site.clone(), title.clone())),
                    widget,
                    theme,
                ))
                .into_any_element(),
            _ => {
                let art = link_media::artwork(
                    &card.url,
                    &site,
                    &title,
                    description.as_deref().unwrap_or_default(),
                );
                div()
                    .size_full()
                    .relative()
                    .child(crate::message::links::artwork(
                        format!("{id}-art"),
                        &art,
                        crate::message::links::ArtSize::Poster,
                    ))
                    .child(open_mark(None, false, theme))
                    .into_any_element()
            }
        };

        let posted = (card.count > 1).then(|| {
            let card = card.clone();
            let label = t!("links-view-posted-times", count = card.count);
            div()
                .id(ElementId::Name(format!("{id}-posts").into()))
                .test_support()
                .role(Role::Button)
                .reachable(theme)
                .aria_label(SharedString::from(label.clone()))
                .flex_none()
                .flex()
                .items_center()
                .gap_0p5()
                .cursor_pointer()
                .text_color(theme.link)
                .child(label)
                .child(ui::icon::icon(IconName::ChevronDown, theme.link))
                .on_click(
                    cx.listener(move |this, _, window, cx| this.open_posts(&card, window, cx)),
                )
        });

        // The preview shows the site and title, so the body leaves them out.
        let graphic = shown.cover.is_none() && (shown.picture.is_none() || overlaid);
        let url = card.url.clone();
        let openable = catalog::openable(&url);
        let open_label = SharedString::from(t!("links-view-open", url = url.clone()));
        let open_url = url.clone();
        let title_el = (!graphic).then(|| {
            div()
                .id(ElementId::Name(format!("{id}-title").into()))
                .test_support()
                .role(Role::Link)
                .reachable(theme)
                .aria_label(SharedString::from(t!("links-view-open", url = url.clone())))
                .text_sm()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.text)
                .line_clamp(2)
                .when(openable, |d| {
                    d.cursor_pointer()
                        .on_click(move |_, _, cx| cx.open_url(&url))
                })
                .child(title.clone())
        });

        let footer = source.map(|source| {
            let (author, place) = (author(w, source), place(w, &source.conversation_id));
            let network = w
                .network(&source.network_id)
                .map(|n| n.name.clone())
                .unwrap_or_else(|| t!("links-view-unknown-network").to_string());
            let full = t!(
                "links-view-time",
                time = when(&card.last_posted_at),
                network = network
            );
            let label = t!(
                "links-view-jump",
                author = author.clone(),
                place = place.clone()
            );
            let (session, source) = (self.session.clone(), source.clone());
            div()
                .mt_1()
                .pt_1p5()
                .border_t_1()
                .border_color(theme.border)
                .flex()
                .items_center()
                .gap_2()
                .text_xs()
                .child(
                    div()
                        .id(ElementId::Name(format!("{id}-source").into()))
                        .test_support()
                        .role(Role::Button)
                        .reachable(theme)
                        .aria_label(SharedString::from(label))
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .cursor_pointer()
                        .rounded_md()
                        .text_color(theme.muted)
                        .hover({
                            let text = theme.text;
                            move |d| d.text_color(text)
                        })
                        .child(t!("links-view-source", author = author, place = place))
                        .on_click(move |_, window, cx| jump(&session, &source, window, cx)),
                )
                .child(
                    div()
                        .id(ElementId::Name(format!("{id}-time").into()))
                        .flex_none()
                        .text_color(theme.faint)
                        .tooltip(ui::tooltip::text(full))
                        .child(ago(&card.last_posted_at)),
                )
        });

        div()
            .id(ElementId::Name(id.clone().into()))
            .test_support()
            .role(Role::Article)
            .aria_label(SharedString::from(title.clone()))
            .aria_selected(selected)
            .w(px(CARD_W))
            .flex_none()
            .flex()
            .flex_col()
            .rounded(theme.radius_large)
            .overflow_hidden()
            .border_1()
            .border_color(if selected { theme.accent } else { theme.border })
            .bg(theme.raised)
            .child(
                div()
                    .id(ElementId::Name(format!("{id}-preview").into()))
                    .test_support()
                    .role(Role::Link)
                    .reachable(theme)
                    .aria_label(open_label)
                    .w_full()
                    .h(px(PREVIEW_H))
                    .overflow_hidden()
                    .bg(theme.input)
                    .when(openable && shown.cover.is_none(), |d| {
                        d.cursor_pointer()
                            .on_click(move |_, _, cx| cx.open_url(&open_url))
                    })
                    .child(preview),
            )
            .child(
                div()
                    .px_3()
                    .py_2()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1p5()
                            .text_xs()
                            .text_color(theme.muted)
                            .when(!graphic, |d| {
                                d.child(ui::icon::icon(IconName::Globe, theme.muted))
                                    .child(div().flex_1().min_w_0().truncate().child(site))
                            })
                            .when(graphic, |d| d.justify_end())
                            .children(posted),
                    )
                    .children(title_el)
                    .when_some(line, |d, line| {
                        d.child(
                            div()
                                .text_xs()
                                .text_color(theme.muted)
                                .truncate()
                                .child(line),
                        )
                    })
                    .children(footer),
            )
            .into_any_element()
    }
}

/// What a card shows of its preview this time.
struct Shown {
    preview: Option<Preview>,
    picture: Option<Media>,
    cover: Option<Cover>,
}

impl Render for LinksView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme::get(cx).clone();
        let Some(client) = self.client(cx) else {
            return div().into_any_element();
        };
        if std::mem::take(&mut self.focus) {
            self.input.focus_handle(cx).focus(window, cx);
        }
        let filter = self.filter(cx);
        let problem = filter
            .as_ref()
            .err()
            .cloned()
            .or_else(|| self.notice.clone());
        let pending = filter
            .as_ref()
            .is_ok_and(|f| self.list.filter.as_ref() != Some(f))
            || catalog::parse_typed(&self.input.read(cx).value()).text != self.text;

        // The previews, asked for before the workspace is read for names.
        let shown: Vec<Shown> = self
            .list
            .items
            .clone()
            .iter()
            .map(|card| {
                let Some(source) = card.sources.first() else {
                    return Shown {
                        preview: None,
                        picture: None,
                        cover: None,
                    };
                };
                let preview = preview(&client, source, &card.url, window, cx);
                let picture = match &preview {
                    Some(Preview::Ready { response, .. }) => picture(response),
                    _ => None,
                };
                let cover = picture
                    .as_ref()
                    .and_then(|_| covered(&client, source, &card.url, cx));
                Shown {
                    preview,
                    picture,
                    cover,
                }
            })
            .collect();

        let offline = client.read().connection != Some(Connection::Live);
        let count = self.list.items.len();
        let meta: Option<String> = (count > 0).then(|| {
            if self.list.cursor.is_some() {
                t!("links-view-meta-more", count = count)
            } else if count == 1 {
                t!("links-view-meta-one").to_string()
            } else {
                t!("links-view-meta", count = count)
            }
        });

        // The filter box, the buttons that start a filter, the type pills.
        let (channels, networks) = {
            let w = client.read();
            (
                w.conversations
                    .iter()
                    .filter(|c| c.kind == ConversationKind::Channel && c.party.is_none())
                    .count(),
                w.networks.len(),
            )
        };
        let picked = self.picked(cx);
        let mut starts = div().flex().flex_wrap().gap_1p5();
        for (key, label, icon, show) in [
            ("in", t!("links-view-tag-in"), None, channels > 1),
            (
                "network",
                t!("links-view-tag-network"),
                Some(IconName::Globe),
                networks > 1 && picked.is_empty(),
            ),
            (
                "host",
                t!("links-view-tag-host"),
                Some(IconName::Globe),
                true,
            ),
            (
                "since",
                t!("links-view-tag-since"),
                Some(IconName::Calendar),
                true,
            ),
            (
                "until",
                t!("links-view-tag-until"),
                Some(IconName::Calendar),
                true,
            ),
        ] {
            if !show {
                continue;
            }
            starts = starts.child(
                div()
                    .id(ElementId::Name(format!("links-start-{key}").into()))
                    .test_support()
                    .role(Role::Button)
                    .reachable(&theme)
                    .aria_label(label)
                    .h(px(28.))
                    .px_3()
                    .rounded_full()
                    .cursor_pointer()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .text_xs()
                    .font_weight(FontWeight::MEDIUM)
                    .bg(theme.input)
                    .border_1()
                    .border_color(theme.border)
                    .text_color(theme.muted)
                    .hover({
                        let (hover, text) = (theme.hover, theme.text);
                        move |d| d.bg(hover).text_color(text)
                    })
                    .child(match icon {
                        Some(icon) => ui::icon::icon(icon, theme.muted).into_any_element(),
                        None => div().child("#").into_any_element(),
                    })
                    .child(label)
                    .on_click(cx.listener(move |this, _, window, cx| this.start(key, window, cx))),
            );
        }
        let mut kinds = div()
            .id("links-kinds")
            .test_support()
            .role(Role::Group)
            .aria_label(t!("links-view-filters"))
            .flex()
            .flex_wrap()
            .gap_1p5();
        for kind in catalog::KINDS {
            let on = self.kinds.contains(&kind);
            let label = kind_label(kind);
            let value = catalog::kind_value(kind).unwrap_or_default();
            kinds = kinds.child(
                div()
                    .id(ElementId::Name(format!("links-kind-{value}").into()))
                    .test_support()
                    .role(Role::CheckBox)
                    .reachable(&theme)
                    .aria_label(label)
                    .aria_toggled(if on { Toggled::True } else { Toggled::False })
                    .px_2p5()
                    .py_0p5()
                    .rounded_full()
                    .cursor_pointer()
                    .text_xs()
                    .when(on, |d| d.bg(theme.accent).text_color(theme.on_accent))
                    .when(!on, |d| {
                        let hover = theme.hover;
                        d.bg(theme.input)
                            .text_color(theme.text)
                            .hover(move |d| d.bg(hover))
                    })
                    .child(label)
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle_kind(kind, cx))),
            );
        }
        let chips = self.chips(cx);
        let mut chip_row = div().flex().flex_wrap().gap_1();
        for (i, chip) in chips.iter().enumerate() {
            let (key, value) = self.chip_text(chip, cx);
            let label = t!(
                "links-view-remove-filter",
                filter = key,
                value = value.clone()
            );
            let chip = chip.clone();
            chip_row = chip_row.child(
                div()
                    .id(ElementId::Name(format!("links-chip-{i}").into()))
                    .test_support()
                    .role(Role::Button)
                    .reachable(&theme)
                    .aria_label(SharedString::from(label))
                    .px_2()
                    .py_0p5()
                    .rounded_full()
                    .cursor_pointer()
                    .bg(theme.selected)
                    .text_sm()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(div().text_color(theme.accent).child(format!("{key}:")))
                    .child(div().text_color(theme.text).child(value))
                    .child(ui::icon::icon(IconName::Close, theme.muted))
                    .on_click(cx.listener(move |this, _, _, cx| this.remove(&chip, cx))),
            );
        }
        let toolbar = div()
            .px_4()
            .pt_3()
            .pb_2()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .text_sm()
                    .text_color(theme.muted)
                    .child(t!("links-view-description")),
            )
            .child(
                Input::new(&self.input)
                    .id("links-filter")
                    .aria_label(t!("links-view-filter-label")),
            )
            .when(!chips.is_empty(), |d| d.child(chip_row))
            .child(starts)
            .child(kinds)
            .when_some(problem.clone(), |d, problem| {
                d.child(
                    div()
                        .id("links-problem")
                        .test_support()
                        .role(Role::Alert)
                        .aria_label(SharedString::from(problem.clone()))
                        .text_xs()
                        .text_color(theme.danger)
                        .child(problem),
                )
            });

        let notice = |id: &'static str, text: &'static str| {
            div()
                .id(id)
                .test_support()
                .role(Role::Status)
                .aria_label(text)
                .px_4()
                .pb_2()
                .text_sm()
                .text_color(theme.muted)
                .child(text)
        };

        let l = &self.list;
        let body: AnyElement = if filter.is_err() {
            ui::empty::empty(
                "links-filter-problem",
                problem.clone().unwrap_or_default(),
                None,
                &theme,
            )
            .into_any_element()
        } else if !l.loaded && l.error.is_some() {
            let text = t!(
                "links-view-load-failed",
                error = l.error.clone().unwrap_or_default()
            );
            div()
                .id("links-failed")
                .test_support()
                .role(Role::Alert)
                .aria_label(SharedString::from(text.clone()))
                .p_8()
                .flex()
                .flex_col()
                .items_center()
                .gap_2()
                .text_color(theme.muted)
                .child(text)
                .child(crate::features::inbox::link_button(
                    "links-retry",
                    t!("links-view-try-again"),
                    &theme,
                    cx.listener(|this, _, _, cx| this.reload(cx)),
                ))
                .into_any_element()
        } else if !l.loaded || (pending && l.items.is_empty()) {
            crate::features::inbox::loading("links-loading", t!("links-view-loading"), &theme)
        } else if l.items.is_empty() {
            let narrowed = l.filter.as_ref().is_some_and(LinkFilter::narrowed);
            let w = client.read();
            let title: String = if narrowed {
                t!("links-view-no-matches").to_string()
            } else if picked.len() == 1 {
                t!(
                    "links-view-none-in-channel",
                    channel = place(&w, &picked[0])
                )
            } else if !picked.is_empty() {
                t!("links-view-none-in-channels").to_string()
            } else {
                t!("links-view-none").to_string()
            };
            drop(w);
            div()
                .flex()
                .flex_col()
                .items_center()
                .child(ui::empty::empty("links-empty", title, None, &theme))
                .when(narrowed, |d| {
                    d.child(crate::features::inbox::link_button(
                        "links-clear",
                        t!("links-view-clear-filters"),
                        &theme,
                        cx.listener(|this, _, window, cx| this.clear(window, cx)),
                    ))
                })
                .into_any_element()
        } else {
            let items = l.items.clone();
            let more = crate::features::inbox::more(
                "links-load-more",
                t!("links-view-load-older"),
                l.cursor.is_some(),
                l.loading_more,
                l.more_error.as_deref(),
                &theme,
                cx.listener(|this, _, _, cx| this.load_more(cx)),
            );
            let w_guard = client.clone();
            let w = w_guard.read();
            let cards: Vec<AnyElement> = items
                .iter()
                .zip(&shown)
                .enumerate()
                .map(|(i, (card, shown))| self.card(i, card, shown, &w, &theme, cx))
                .collect();
            drop(w);
            div()
                .flex()
                .flex_col()
                .gap_3()
                .child(
                    div()
                        .id("links-list")
                        .test_support()
                        .role(Role::List)
                        .aria_label(t!("links-view-list"))
                        .flex()
                        .flex_wrap()
                        .gap_4()
                        .children(cards),
                )
                .children(more)
                .into_any_element()
        };
        let indexing = l.indexing;

        let count = self.list.items.len();
        let page = div()
            .id("links-page-root")
            .flex()
            .flex_col()
            .capture_action(cx.listener(move |this, _: &MoveDown, _, cx| {
                if count > 0 {
                    this.selected = (this.selected + 1) % count;
                    cx.stop_propagation();
                    cx.notify();
                }
            }))
            .capture_action(cx.listener(move |this, _: &MoveUp, _, cx| {
                if count > 0 {
                    this.selected = (this.selected + count - 1) % count;
                    cx.stop_propagation();
                    cx.notify();
                }
            }))
            .capture_action(cx.listener(|this, _: &Backspace, _, cx| {
                if this.input.read(cx).value().is_empty()
                    && let Some(chip) = this.chips(cx).pop()
                {
                    this.remove(&chip, cx);
                    cx.stop_propagation();
                }
            }))
            .capture_action(cx.listener(|this, _: &Escape, window, cx| {
                this.session.update(cx, |s, cx| s.go_back(cx));
                window.refresh();
                cx.stop_propagation();
            }))
            .child(toolbar)
            .when(offline, |d| {
                d.child(notice("links-offline", t!("links-view-offline")))
            })
            .when(indexing, |d| {
                d.child(notice("links-indexing", t!("links-view-indexing")))
            })
            .child(div().px_4().pb_4().child(body))
            .child(
                div()
                    .px_4()
                    .py_1()
                    .border_t_1()
                    .border_color(theme.rail)
                    .text_xs()
                    .text_color(theme.faint)
                    .child(t!("links-view-keyboard")),
            );
        let mut frame = ui::page::Page::new("links-page", t!("links-view-title"), page);
        if let Some(meta) = meta {
            frame = frame.action(
                div()
                    .id("links-meta")
                    .test_support()
                    .role(Role::Status)
                    .aria_label(SharedString::from(meta.clone()))
                    .text_sm()
                    .text_color(theme.muted)
                    .child(meta),
            );
        }
        frame.render(&theme).into_any_element()
    }
}

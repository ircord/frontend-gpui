//! The emoji and GIF pickers (package E1).
//!
//! One view, [`PickerView`], with two tabs, as the web client's
//! `ExpressionPicker.vue`:
//!
//! - **Emoji**: search, a group rail down the left (frequently used, the
//!   server's emoji, 7TV, then the Unicode groups) and a skin tone. Picking writes the emoji
//!   into the message box, or reacts with it when the picker was opened from
//!   a message.
//! - **GIFs**: Klipy's categories, trending and search, and your saved GIFs;
//!   a star saves or forgets one. Picking sends it at once.
//!
//! Over the composer the picker shows above the box (`features/klipy.rs`
//! puts it in the composer's `above` slot); for a reaction it shows in a
//! dialog, emoji only, with your quick reactions first.

use crate::ui::focus::Reachable;
use std::time::Duration;

use gpui_kit::TestSupportExt;
use gpui_kit::component::IconName;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::*;
use gpui_kit::*;
use ircord_client::api::emotes::klipy::{Category, GifPage};
use ircord_client::contract as c;
use ircord_client::store::emoji::{self, EmojiItem, GROUPS};

use crate::composer::ComposerView;
use crate::features::klipy;
use crate::message::emoji as pictures;
use crate::registry::MessageCtx;
use crate::session::Session;
use crate::theme::{self, Theme};
use crate::ui;

/// Emoji drawn at most at once in one group or search.
const EMOJI_LIMIT: usize = 400;
/// GIFs listed at most (the web client stops paging there).
const GIF_LIMIT: usize = 240;
/// How long typing in the GIF search waits before it searches.
const DEBOUNCE: Duration = Duration::from_millis(300);
/// The picker's size over the composer.
pub const WIDTH: f32 = 440.;
pub const HEIGHT: f32 = 380.;

/// What picking does.
#[derive(Clone)]
pub enum Purpose {
    /// Write into this composer's box (emoji) or send to its conversation
    /// (GIFs).
    Composer {
        conversation: String,
        composer: WeakEntity<ComposerView>,
    },
    /// React to this message.
    React(MessageCtx),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    Gifs,
    Emoji,
}

#[derive(Clone, Debug, PartialEq)]
pub enum GifView {
    /// Saved, trending and the categories.
    Home,
    /// Klipy's answer for a search (trending for an empty one). `title` is
    /// the category's name when one was opened.
    Results {
        query: String,
        title: Option<String>,
    },
    /// Your saved GIFs.
    Saved,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Load<T> {
    Idle,
    Loading,
    Ready(T),
    Failed(String),
}

pub struct PickerView {
    session: Entity<Session>,
    purpose: Purpose,
    pub tab: Tab,
    pub search: Entity<InputState>,
    pub group: &'static str,
    pub gif_view: GifView,
    categories: Load<Vec<Category>>,
    results: Load<Vec<c::GIF>>,
    next_page: Option<u32>,
    loading_more: bool,
    /// Bumped by every search, so an older search's late answer is dropped.
    generation: u64,
    debounce: Option<Task<()>>,
    tones_open: bool,
    scroll: ScrollHandle,
    _subscriptions: Vec<Subscription>,
}

impl PickerView {
    pub fn new(
        session: Entity<Session>,
        purpose: Purpose,
        tab: Tab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx));
        let subscription = cx.subscribe_in(
            &search,
            window,
            |this: &mut Self, _, event: &InputEvent, _window, cx| {
                if let InputEvent::Change = event {
                    this.typed(cx);
                }
            },
        );
        let mut view = Self {
            session,
            purpose,
            tab,
            search,
            group: "frequent",
            gif_view: GifView::Home,
            categories: Load::Idle,
            results: Load::Idle,
            next_page: None,
            loading_more: false,
            generation: 0,
            debounce: None,
            tones_open: false,
            scroll: ScrollHandle::new(),
            _subscriptions: vec![subscription],
        };
        view.placeholder(window, cx);
        if tab == Tab::Gifs {
            view.load_categories(cx);
        }
        view
    }

    fn reacting(&self) -> bool {
        matches!(self.purpose, Purpose::React(_))
    }

    pub fn conversation(&self) -> &str {
        match &self.purpose {
            Purpose::Composer { conversation, .. } => conversation,
            Purpose::React(ctx) => &ctx.conversation,
        }
    }

    fn query(&self, cx: &App) -> String {
        self.search.read(cx).value().to_string()
    }

    fn placeholder(&self, window: &mut Window, cx: &mut Context<Self>) {
        let text = match self.tab {
            Tab::Emoji => t!("emoji-search-emoji"),
            Tab::Gifs => t!("emoji-search-gifs"),
        };
        self.search
            .update(cx, |s, cx| s.set_placeholder(text, window, cx));
    }

    /// Switch tabs; each starts with an empty search.
    pub fn set_tab(&mut self, tab: Tab, window: &mut Window, cx: &mut Context<Self>) {
        if self.tab == tab || (self.reacting() && tab == Tab::Gifs) {
            return;
        }
        self.tab = tab;
        self.search.update(cx, |s, cx| s.set_value("", window, cx));
        self.placeholder(window, cx);
        if tab == Tab::Gifs && matches!(self.categories, Load::Idle) {
            self.load_categories(cx);
        }
        cx.notify();
    }

    fn typed(&mut self, cx: &mut Context<Self>) {
        self.tones_open = false;
        if self.tab == Tab::Emoji {
            self.scroll.set_offset(Point::default());
            cx.notify();
            return;
        }
        if self.gif_view == GifView::Saved {
            cx.notify();
            return;
        }
        let query = self.query(cx).trim().to_string();
        self.debounce = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(DEBOUNCE).await;
            let _ = this.update(cx, |this, cx| {
                // Switch views only once the search runs, so typing never
                // flashes trending results.
                match &this.gif_view {
                    GifView::Home if query.is_empty() => return,
                    GifView::Results { query: q, .. } if *q == query => return,
                    _ => {}
                }
                this.open_results(query, None, cx);
            });
        }));
    }

    // ---- GIFs ----

    fn load_categories(&mut self, cx: &mut Context<Self>) {
        let Some(api) = klipy::klipy(cx) else {
            self.categories = Load::Failed(t!("emoji-gif-not-configured").into());
            return;
        };
        self.categories = Load::Loading;
        let runtime = self.session.read(cx).runtime.clone();
        let task = runtime.spawn(async move { api.categories().await });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.categories = match result {
                    Ok(Ok(found)) => Load::Ready(found),
                    Ok(Err(e)) => Load::Failed(klipy::error_text(&e)),
                    Err(e) => Load::Failed(e.to_string()),
                };
                cx.notify();
            });
        })
        .detach();
    }

    /// Show Klipy's results for `query` (trending when empty).
    pub fn open_results(&mut self, query: String, title: Option<String>, cx: &mut Context<Self>) {
        self.gif_view = GifView::Results {
            query: query.clone(),
            title,
        };
        self.results = Load::Loading;
        self.next_page = None;
        self.scroll.set_offset(Point::default());
        self.fetch(query, 1, cx);
        cx.notify();
    }

    fn fetch(&mut self, query: String, page: u32, cx: &mut Context<Self>) {
        let Some(api) = klipy::klipy(cx) else {
            self.results = Load::Failed(t!("emoji-gif-not-configured").into());
            return;
        };
        self.generation += 1;
        let generation = self.generation;
        let runtime = self.session.read(cx).runtime.clone();
        let task = runtime.spawn(async move { api.gifs(&query, page).await });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                this.loading_more = false;
                match result {
                    Ok(Ok(GifPage { items, next_page })) => {
                        let mut all = match std::mem::replace(&mut this.results, Load::Idle) {
                            Load::Ready(have) if page > 1 => have,
                            _ => Vec::new(),
                        };
                        for gif in items {
                            if !all.iter().any(|g| g.id == gif.id) {
                                all.push(gif);
                            }
                        }
                        all.truncate(GIF_LIMIT);
                        this.next_page = next_page.filter(|_| all.len() < GIF_LIMIT);
                        this.results = Load::Ready(all);
                    }
                    Ok(Err(e)) if page == 1 => this.results = Load::Failed(klipy::error_text(&e)),
                    Err(e) if page == 1 => this.results = Load::Failed(e.to_string()),
                    _ => this.next_page = None,
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn more(&mut self, cx: &mut Context<Self>) {
        let (GifView::Results { query, .. }, Some(page)) = (&self.gif_view, self.next_page) else {
            return;
        };
        if self.loading_more {
            return;
        }
        self.loading_more = true;
        let query = query.clone();
        self.fetch(query, page, cx);
        cx.notify();
    }

    fn back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.gif_view = GifView::Home;
        self.generation += 1;
        self.search.update(cx, |s, cx| s.set_value("", window, cx));
        cx.notify();
    }

    fn saved(&self, cx: &App) -> Vec<c::GIF> {
        let query = self.query(cx).trim().to_lowercase();
        klipy::saved_gifs(&self.session, cx)
            .into_iter()
            .filter(|g| query.is_empty() || g.title.to_lowercase().contains(&query))
            .collect()
    }

    fn choose_gif(&mut self, gif: c::GIF, window: &mut Window, cx: &mut Context<Self>) {
        let query = match &self.gif_view {
            GifView::Results { query, .. } => query.clone(),
            _ => String::new(),
        };
        let conversation = self.conversation().to_string();
        if klipy::send_gif(&self.session, &conversation, gif, &query, window, cx) {
            klipy::close(window, cx);
        }
    }

    // ---- Emoji ----

    fn emoji_items(&self, cx: &App) -> (Vec<EmojiItem>, SharedString) {
        let query = self.query(cx);
        let session = self.session.read(cx);
        let Some(client) = session.client() else {
            return (Vec::new(), SharedString::default());
        };
        let w = client.read();
        let library = w.documents.library.value();
        let state = &w.emoji;
        if !query.trim().is_empty() {
            let usage = emoji::usage_rank(library);
            let found = emoji::rank(emoji::search(state.all(), &query, None), &usage, &query);
            let count = found.len();
            let items = found.into_iter().take(EMOJI_LIMIT).cloned().collect();
            return (items, t!("emoji-results", count = count).into());
        }
        if self.group == "frequent" {
            let frequent = emoji::frequent(state, library);
            if frequent.is_empty() {
                let popular = emoji::search(emoji::standard().iter(), "", Some("smileys-emotion"))
                    .into_iter()
                    .take(45)
                    .cloned()
                    .collect();
                return (popular, t!("emoji-popular").into());
            }
            return (
                frequent.into_iter().cloned().collect(),
                group_label(self.group),
            );
        }
        let items = emoji::search(state.all(), "", Some(self.group))
            .into_iter()
            .take(EMOJI_LIMIT)
            .cloned()
            .collect();
        (items, group_label(self.group))
    }

    fn select_group(&mut self, group: &'static str, window: &mut Window, cx: &mut Context<Self>) {
        self.group = group;
        self.search.update(cx, |s, cx| s.set_value("", window, cx));
        self.scroll.set_offset(Point::default());
        cx.notify();
    }

    fn choose_emoji(&mut self, item: EmojiItem, window: &mut Window, cx: &mut Context<Self>) {
        let tone = klipy::skin_tone(&self.session, cx);
        let text = item.text(tone);
        klipy::record_pick(&self.session, &item.id, cx);
        match self.purpose.clone() {
            Purpose::Composer { composer, .. } => {
                let Some(composer) = composer.upgrade() else {
                    return;
                };
                let fits = composer.update(cx, |c, cx| {
                    let current = c.text(cx);
                    let range = c.input.read(cx).selected_range();
                    let (start, end) =
                        (range.start.min(current.len()), range.end.min(current.len()));
                    if current.len() - (end - start) + text.len() > emoji::TEXT_LIMIT {
                        return false;
                    }
                    let next = format!("{}{text}{}", &current[..start], &current[end..]);
                    c.set_text_at(&next, start + text.len(), window, cx);
                    true
                });
                if !fits {
                    return ui::toast::info(window, cx, t!("emoji-too-long"));
                }
                klipy::close(window, cx);
                composer.update(cx, |c, cx| {
                    c.input.update(cx, |s, cx| s.focus(window, cx));
                });
            }
            Purpose::React(ctx) => {
                ui::dialog::close(window, cx);
                crate::features::message_actions::react(
                    &ctx.session,
                    &ctx.conversation,
                    &ctx.message,
                    &text,
                    window,
                    cx,
                );
            }
        }
    }

    fn set_tone(&mut self, tone: &'static str, cx: &mut Context<Self>) {
        self.tones_open = false;
        klipy::set_skin_tone(&self.session, tone, cx);
        cx.notify();
    }

    // ---- Drawing ----

    fn tabs(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let tab = |id: &'static str, label: SharedString, which: Tab, on: bool| {
            div()
                .id(id)
                .test_support()
                .role(Role::Tab)
                .reachable(theme)
                .aria_label(label.clone())
                .aria_selected(on)
                .px_3()
                .py_1()
                .rounded_md()
                .text_sm()
                .cursor_pointer()
                .when(on, |d| d.bg(theme.selected).text_color(theme.text))
                .when(!on, |d| {
                    let hover = theme.hover;
                    d.text_color(theme.muted).hover(move |d| d.bg(hover))
                })
                .child(label)
                .on_click(cx.listener(move |this, _, window, cx| this.set_tab(which, window, cx)))
        };
        div()
            .id("expression-tabs")
            .test_support()
            .role(Role::TabList)
            .aria_label(t!("emoji-picker"))
            .flex()
            .gap_1()
            .child(tab(
                "expression-tab-gifs",
                t!("emoji-tab-gifs").into(),
                Tab::Gifs,
                self.tab == Tab::Gifs,
            ))
            .child(tab(
                "expression-tab-emoji",
                t!("emoji-tab-emoji").into(),
                Tab::Emoji,
                self.tab == Tab::Emoji,
            ))
            .into_any_element()
    }

    fn search_row(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let label = match self.tab {
            Tab::Emoji => t!("emoji-search-emoji"),
            Tab::Gifs => t!("emoji-search-gifs"),
        };
        let mut row = div().flex().items_center().gap_2().child(
            div().flex_1().min_w_0().child(
                Input::new(&self.search)
                    .id("expression-search")
                    .aria_label(label),
            ),
        );
        if self.tab == Tab::Emoji {
            let tone = klipy::skin_tone(&self.session, cx);
            let hover = theme.hover;
            row = row.child(
                div()
                    .id("emoji-skin-tone")
                    .test_support()
                    .role(Role::Button)
                    .reachable(theme)
                    .aria_label(t!("emoji-skin-tone"))
                    .aria_expanded(self.tones_open)
                    .tooltip(ui::tooltip::text(t!("emoji-skin-tone")))
                    .px_1()
                    .rounded_md()
                    .text_lg()
                    .cursor_pointer()
                    .hover(move |d| d.bg(hover))
                    .child(tone_glyph(tone))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.tones_open = !this.tones_open;
                        cx.notify();
                    })),
            );
        }
        row.into_any_element()
    }

    fn tones(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let current = klipy::skin_tone(&self.session, cx);
        let mut row = div()
            .id("emoji-skin-tones")
            .test_support()
            .role(Role::Group)
            .aria_label(t!("emoji-skin-tone"))
            .flex()
            .gap_1();
        for tone in emoji::SKIN_TONES {
            let label = tone_label(tone);
            let hover = theme.hover;
            row = row.child(
                div()
                    .id(ElementId::Name(format!("emoji-tone-{tone}").into()))
                    .test_support()
                    .role(Role::Button)
                    .reachable(theme)
                    .aria_label(label.clone())
                    .aria_selected(tone == current)
                    .tooltip(ui::tooltip::text(label))
                    .p_1()
                    .rounded_md()
                    .text_lg()
                    .cursor_pointer()
                    .when(tone == current, |d| d.bg(theme.selected))
                    .hover(move |d| d.bg(hover))
                    .child(tone_glyph(tone))
                    .on_click(cx.listener(move |this, _, _, cx| this.set_tone(tone, cx))),
            );
        }
        row.into_any_element()
    }

    fn groups(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let (has_workspace, has_7tv) = {
            let session = self.session.read(cx);
            match session.client() {
                Some(client) => {
                    let w = client.read();
                    (
                        w.emoji.all().any(|e| e.group == "workspace"),
                        w.emoji.bundled.iter().any(|e| e.group == "7tv"),
                    )
                }
                None => (false, false),
            }
        };
        let searching = !self.query(cx).trim().is_empty();
        let mut bar = div()
            .id("emoji-groups")
            .test_support()
            .role(Role::TabList)
            .aria_label(t!("emoji-groups"))
            .flex_none()
            .flex()
            .flex_col()
            .items_center()
            .gap_0p5()
            .pr_1()
            .border_r_1()
            .border_color(theme.border)
            .overflow_y_scroll();
        for group in GROUPS {
            if (group == "workspace" && !has_workspace) || (group == "7tv" && !has_7tv) {
                continue;
            }
            let on = !searching && self.group == group;
            let label = group_label(group);
            let hover = theme.hover;
            bar =
                bar.child(
                    div()
                        .id(ElementId::Name(format!("emoji-group-{group}").into()))
                        .test_support()
                        .role(Role::Tab)
                        .reachable(theme)
                        .aria_label(label.clone())
                        .aria_selected(on)
                        .tooltip(ui::tooltip::text(label))
                        .size(px(28.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_md()
                        .cursor_pointer()
                        .when(on, |d| d.bg(theme.selected))
                        .hover(move |d| d.bg(hover))
                        .child(group_glyph(group))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.select_group(group, window, cx)
                        })),
                );
        }
        bar.into_any_element()
    }

    fn emoji_body(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let (items, title) = self.emoji_items(cx);
        let session = self.session.read(cx);
        let origin = session.server().cloned();
        let (loading, animate) = match session.client() {
            Some(client) => {
                let w = client.read();
                (!w.emoji.is_loaded(), klipy::animate(&w))
            }
            None => (false, true),
        };
        let tone = klipy::skin_tone(&self.session, cx);
        let mut grid = div()
            .id("emoji-grid")
            .test_support()
            .role(Role::Grid)
            .aria_label(title.clone())
            .flex()
            .flex_wrap()
            .gap_0p5();
        for item in items.iter() {
            let label: SharedString = if item.is_custom() {
                format!(":{}:", item.shortcode).into()
            } else {
                item.label.clone().into()
            };
            let hover = theme.hover;
            let glyph = match pictures::picture_url(item, animate, origin.as_ref()) {
                Some(url) => div()
                    .size(px(26.))
                    .child(pictures::picture(url, theme.faint))
                    .into_any_element(),
                None => div()
                    .text_2xl()
                    .line_height(px(30.))
                    .child(item.text(tone))
                    .into_any_element(),
            };
            let chosen = item.clone();
            grid = grid.child(
                div()
                    .id(ElementId::Name(format!("emoji-{}", item.id).into()))
                    .test_support()
                    .role(Role::GridCell)
                    .aria_label(label)
                    .tooltip(ui::tooltip::text(format!(":{}:", item.shortcode)))
                    .size(px(36.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_md()
                    .cursor_pointer()
                    .hover(move |d| d.bg(hover))
                    .child(glyph)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.choose_emoji(chosen.clone(), window, cx)
                    })),
            );
        }
        let mut body = div().flex().flex_col().gap_1().child(
            div()
                .text_xs()
                .text_color(theme.muted)
                .font_weight(FontWeight::SEMIBOLD)
                .child(title),
        );
        if items.is_empty() {
            body = body.child(div().text_sm().text_color(theme.muted).child(if loading {
                t!("emoji-loading")
            } else {
                t!("emoji-none")
            }));
        } else {
            body = body.child(grid);
        }
        body.into_any_element()
    }

    fn gif_tile(
        &self,
        gif: &c::GIF,
        width: f32,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let height = (width * gif.height as f32 / gif.width.max(1) as f32).clamp(60., 320.);
        let saved = klipy::is_saved(&self.session, &gif.id, cx);
        let label = if gif.title.is_empty() {
            t!("emoji-gif").to_string()
        } else {
            gif.title.clone()
        };
        let save_label = if saved {
            t!("emoji-gif-forget")
        } else {
            t!("emoji-gif-save")
        };
        let chosen = gif.clone();
        let toggled = gif.clone();
        div()
            .id(ElementId::Name(format!("gif-{}", gif.id).into()))
            .test_support()
            .role(Role::Button)
            .reachable(theme)
            .aria_label(SharedString::from(label.clone()))
            .tooltip(ui::tooltip::text(label))
            .relative()
            .w(px(width))
            .h(px(height))
            .rounded_md()
            .overflow_hidden()
            .bg(theme.sidebar)
            .cursor_pointer()
            .child(
                img(SharedUri::from(gif.preview_url.clone()))
                    .size_full()
                    .object_fit(ObjectFit::Cover),
            )
            .child(
                div()
                    .id(ElementId::Name(format!("gif-save-{}", gif.id).into()))
                    .test_support()
                    .role(Role::Button)
                    .reachable(theme)
                    .aria_label(save_label)
                    .aria_selected(saved)
                    .tooltip(ui::tooltip::text(save_label))
                    .absolute()
                    .top_1()
                    .right_1()
                    .p_1()
                    .rounded_md()
                    .bg(theme.main.opacity(0.8))
                    .child(ui::icon::icon(
                        if saved {
                            IconName::StarFill
                        } else {
                            IconName::Star
                        },
                        if saved { theme.warning } else { theme.text },
                    ))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        let session = this.session.clone();
                        klipy::toggle_saved(&session, toggled.clone(), window, cx);
                        cx.notify();
                    })),
            )
            .on_click(
                cx.listener(move |this, _, window, cx| this.choose_gif(chosen.clone(), window, cx)),
            )
            .into_any_element()
    }

    /// Two balanced columns, like a masonry grid.
    fn gif_grid(&self, gifs: &[c::GIF], theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let width = (WIDTH - 40.) / 2.;
        let mut columns: [(Vec<AnyElement>, f32); 2] = [(Vec::new(), 0.), (Vec::new(), 0.)];
        for gif in gifs {
            let i = usize::from(columns[1].1 < columns[0].1);
            columns[i].1 += gif.height as f32 / gif.width.max(1) as f32;
            let tile = self.gif_tile(gif, width, theme, cx);
            columns[i].0.push(tile);
        }
        let [(left, _), (right, _)] = columns;
        div()
            .id("gif-results")
            .test_support()
            .role(Role::Grid)
            .aria_label(t!("emoji-tab-gifs"))
            .flex()
            .gap_2()
            .child(div().flex().flex_col().gap_2().children(left))
            .child(div().flex().flex_col().gap_2().children(right))
            .into_any_element()
    }

    fn gif_home(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let tile = |id: SharedString, label: SharedString, preview: Option<String>| {
            let hover = theme.hover;
            div()
                .id(ElementId::Name(id))
                .test_support()
                .role(Role::Button)
                .reachable(theme)
                .aria_label(label.clone())
                .relative()
                .w(px((WIDTH - 40.) / 2.))
                .h(px(72.))
                .rounded_md()
                .overflow_hidden()
                .bg(theme.sidebar)
                .cursor_pointer()
                .hover(move |d| d.bg(hover))
                .when_some(preview.filter(|p| !p.is_empty()), |d, p| {
                    d.child(
                        img(SharedUri::from(p))
                            .absolute()
                            .size_full()
                            .object_fit(ObjectFit::Cover)
                            .opacity(0.55),
                    )
                })
                .child(
                    div()
                        .absolute()
                        .size_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.text)
                        .child(label),
                )
        };
        let saved = klipy::saved_gifs(&self.session, cx);
        let mut grid = div()
            .id("gif-categories")
            .test_support()
            .role(Role::Group)
            .aria_label(t!("emoji-gif-categories"))
            .flex()
            .flex_wrap()
            .gap_2()
            .child(
                tile(
                    "gif-saved".into(),
                    t!("emoji-gif-saved", count = saved.len()).into(),
                    saved.first().map(|g| g.preview_url.clone()),
                )
                .on_click(cx.listener(|this, _, window, cx| {
                    this.gif_view = GifView::Saved;
                    this.search.update(cx, |s, cx| s.set_value("", window, cx));
                    cx.notify();
                })),
            )
            .child(
                tile("gif-trending".into(), t!("emoji-gif-trending").into(), None).on_click(
                    cx.listener(|this, _, window, cx| {
                        this.search.update(cx, |s, cx| s.set_value("", window, cx));
                        this.open_results(String::new(), Some(t!("emoji-gif-trending").into()), cx);
                    }),
                ),
            );
        let mut notice = None;
        match &self.categories {
            Load::Ready(categories) => {
                for (i, category) in categories.iter().enumerate() {
                    let (name, query) = (category.name.clone(), category.query.clone());
                    grid = grid.child(
                        tile(
                            format!("gif-category-{i}").into(),
                            category.name.clone().into(),
                            Some(category.preview_url.clone()),
                        )
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.search
                                    .update(cx, |s, cx| s.set_value(query.clone(), window, cx));
                                this.open_results(query.clone(), Some(name.clone()), cx);
                            },
                        )),
                    );
                }
            }
            Load::Loading | Load::Idle => notice = Some(t!("emoji-loading").to_string()),
            Load::Failed(why) => notice = Some(why.clone()),
        }
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(grid)
            .children(notice.map(|n| div().text_sm().text_color(theme.muted).child(n)))
            .into_any_element()
    }

    fn gif_body(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let back = |title: SharedString, cx: &mut Context<Self>| {
            let hover = theme.hover;
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .id("gif-back")
                        .test_support()
                        .role(Role::Button)
                        .reachable(theme)
                        .aria_label(t!("emoji-gif-back"))
                        .tooltip(ui::tooltip::text(t!("emoji-gif-back")))
                        .p_1()
                        .rounded_md()
                        .cursor_pointer()
                        .hover(move |d| d.bg(hover))
                        .child(ui::icon::icon(IconName::ArrowLeft, theme.text))
                        .on_click(cx.listener(|this, _, window, cx| this.back(window, cx))),
                )
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(title),
                )
        };
        match self.gif_view.clone() {
            GifView::Home => self.gif_home(theme, cx),
            GifView::Saved => {
                let saved = self.saved(cx);
                let mut body = div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(back(t!("emoji-gif-saved-title").into(), cx));
                body = if saved.is_empty() {
                    body.child(
                        div()
                            .text_sm()
                            .text_color(theme.muted)
                            .child(t!("emoji-gif-saved-none")),
                    )
                } else {
                    body.child(self.gif_grid(&saved, theme, cx))
                };
                body.into_any_element()
            }
            GifView::Results { query, title } => {
                let heading: SharedString = match title {
                    Some(t) => t.into(),
                    None if query.is_empty() => t!("emoji-gif-trending").into(),
                    None => t!("emoji-gif-results", query = query).into(),
                };
                let mut body = div().flex().flex_col().gap_2().child(back(heading, cx));
                match &self.results {
                    Load::Ready(gifs) if gifs.is_empty() => {
                        body = body.child(
                            div()
                                .text_sm()
                                .text_color(theme.muted)
                                .child(t!("emoji-gif-none")),
                        );
                    }
                    Load::Ready(gifs) => {
                        let gifs = gifs.clone();
                        body = body.child(self.gif_grid(&gifs, theme, cx));
                        if self.next_page.is_some() {
                            let hover = theme.hover;
                            body = body.child(
                                div()
                                    .id("gif-more")
                                    .test_support()
                                    .role(Role::Button)
                                    .reachable(theme)
                                    .aria_label(t!("emoji-gif-more"))
                                    .py_1()
                                    .rounded_md()
                                    .text_sm()
                                    .text_center()
                                    .text_color(theme.link)
                                    .cursor_pointer()
                                    .hover(move |d| d.bg(hover))
                                    .child(if self.loading_more {
                                        t!("emoji-loading")
                                    } else {
                                        t!("emoji-gif-more")
                                    })
                                    .on_click(cx.listener(|this, _, _, cx| this.more(cx))),
                            );
                        }
                    }
                    Load::Failed(why) => {
                        body =
                            body.child(div().text_sm().text_color(theme.danger).child(why.clone()));
                    }
                    Load::Loading | Load::Idle => {
                        body = body.child(
                            div()
                                .text_sm()
                                .text_color(theme.muted)
                                .child(t!("emoji-loading")),
                        );
                    }
                }
                body.into_any_element()
            }
        }
    }

    fn quick(&self, theme: &Theme, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.reacting() || !self.query(cx).trim().is_empty() {
            return None;
        }
        let items: Vec<EmojiItem> = {
            let session = self.session.read(cx);
            let client = session.client()?;
            let w = client.read();
            emoji::quick_reactions(&w.emoji, w.documents.library.value(), 8)
                .into_iter()
                .cloned()
                .collect()
        };
        let origin = self.session.read(cx).server().cloned();
        let tone = klipy::skin_tone(&self.session, cx);
        let mut row = div()
            // A1's quick reactions keep their IDs: this row replaces them.
            .id("quick-reactions")
            .test_support()
            .role(Role::Toolbar)
            .aria_label(t!("emoji-quick"))
            .flex()
            .gap_1()
            .pb_1()
            .border_b_1()
            .border_color(theme.sidebar);
        for (i, item) in items.into_iter().enumerate() {
            let text = item.text(tone);
            let label: SharedString = t!("emoji-react-with", emoji = text.clone()).into();
            let hover = theme.hover;
            let glyph = match pictures::picture_url(&item, true, origin.as_ref()) {
                Some(url) => div()
                    .size(px(26.))
                    .child(pictures::picture(url, theme.faint))
                    .into_any_element(),
                None => div().text_2xl().child(text).into_any_element(),
            };
            let chosen = item.clone();
            row = row.child(
                div()
                    .id(ElementId::Name(format!("quick-reaction-{i}").into()))
                    .test_support()
                    .role(Role::Button)
                    .reachable(theme)
                    .aria_label(label.clone())
                    .tooltip(ui::tooltip::text(label))
                    .size(px(40.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_md()
                    .cursor_pointer()
                    .hover(move |d| d.bg(hover))
                    .child(glyph)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.choose_emoji(chosen.clone(), window, cx)
                    })),
            );
        }
        Some(row.into_any_element())
    }
}

impl Render for PickerView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme::get(cx).clone();
        let mut root = div()
            .id("expression-picker")
            .test_support()
            .role(Role::Dialog)
            .aria_label(t!("emoji-picker"))
            .flex()
            .flex_col()
            .gap_2()
            .size_full()
            .min_h_0()
            .capture_key_down(cx.listener(|this, e: &KeyDownEvent, window, cx| {
                if e.keystroke.key == "escape" && !e.keystroke.modifiers.modified() {
                    cx.stop_propagation();
                    if this.reacting() {
                        ui::dialog::close(window, cx);
                    } else {
                        klipy::close(window, cx);
                    }
                }
            }));
        if !self.reacting() {
            root = root.child(self.tabs(&theme, cx));
        }
        if let Some(quick) = self.quick(&theme, cx) {
            root = root.child(quick);
        }
        root = root.child(self.search_row(&theme, cx));
        if self.tab == Tab::Emoji && self.tones_open {
            root = root.child(self.tones(&theme, cx));
        }
        let body = match self.tab {
            Tab::Emoji => self.emoji_body(&theme, cx),
            Tab::Gifs => self.gif_body(&theme, cx),
        };
        let body = div()
            .id("expression-body")
            .flex_1()
            .min_w_0()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .child(body);
        // Emoji groups run down the left, as on the web.
        let rail = (self.tab == Tab::Emoji).then(|| self.groups(&theme, cx));
        root.child(
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .gap_2()
                .children(rail)
                .child(body),
        )
    }
}

pub fn group_label(group: &str) -> SharedString {
    match group {
        "frequent" => t!("emoji-group-frequent"),
        "workspace" => t!("emoji-group-workspace"),
        "7tv" => t!("emoji-group-7tv"),
        "smileys-emotion" => t!("emoji-group-smileys"),
        "people-body" => t!("emoji-group-people"),
        "animals-nature" => t!("emoji-group-animals"),
        "food-drink" => t!("emoji-group-food"),
        "travel-places" => t!("emoji-group-travel"),
        "activities" => t!("emoji-group-activities"),
        "objects" => t!("emoji-group-objects"),
        "symbols" => t!("emoji-group-symbols"),
        _ => t!("emoji-group-flags"),
    }
    .into()
}

fn group_glyph(group: &str) -> &'static str {
    match group {
        "frequent" => "🕘",
        "workspace" => "🏠",
        "7tv" => "🌐",
        "smileys-emotion" => "😀",
        "people-body" => "👋",
        "animals-nature" => "🌿",
        "food-drink" => "🍔",
        "travel-places" => "🚗",
        "activities" => "🏆",
        "objects" => "💡",
        "symbols" => "❤️",
        _ => "🏳️",
    }
}

fn tone_glyph(tone: &str) -> &'static str {
    match tone {
        "1" => "👋🏻",
        "2" => "👋🏼",
        "3" => "👋🏽",
        "4" => "👋🏾",
        "5" => "👋🏿",
        _ => "👋",
    }
}

fn tone_label(tone: &str) -> SharedString {
    match tone {
        "1" => t!("emoji-tone-light"),
        "2" => t!("emoji-tone-medium-light"),
        "3" => t!("emoji-tone-medium"),
        "4" => t!("emoji-tone-medium-dark"),
        "5" => t!("emoji-tone-dark"),
        _ => t!("emoji-tone-default"),
    }
    .into()
}

//! Settings → Emotes: the server's custom emoji (package S5), as the web
//! client's `EmoteSettings.vue`. Two tabs: the server's emoji, searched,
//! each with rename, aliases, enable or disable and remove for whoever may
//! manage it; and Discover, a page of the providers' emoji filtered by
//! source and animation, paged by cursor. Emoji are added by uploading a
//! picture, by pasting a 7TV or FrankerFaceZ link, or from Discover.
//!
//! The catalog read here is the full one (`/api/v1/emotes`, disabled
//! emotes included for managers); the picker's copy (E1) reloads on the
//! `emotes.updated` event each change makes the server send.

use std::collections::HashMap;
use std::sync::Arc;

use gpui_kit::TestSupportExt;
use gpui_kit::component::Disableable;
use gpui_kit::component::Sizable;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::*;
use gpui_kit::*;
use ircord_client::api::ApiError;
use ircord_client::api::emotes::{
    self as api, DISCOVER_PAGE, Discover, NewEmote, emote_matches, image_type, is_installed,
    parse_provider_link, provider_name, split_aliases, suggest_shortcode,
};
use ircord_client::contract as c;
use ircord_client::store::topic;

use crate::registry::{self, SettingsCtx, SettingsGroup, SettingsRow, SettingsSection};
use crate::session::{Session, SessionEvent};
use crate::theme::{self, Theme};
use crate::ui;

/// How long typing in Discover's search settles before it asks.
const DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(250);
/// The largest picture a still frame is made for, as the web client.
const STILL_MAX: u32 = 1024;

pub fn init(cx: &mut App) {
    registry::register(cx, |r| {
        r.settings_sections.add(
            SettingsSection::new("emotes", t!("emotes-title"), |ctx, window, cx| {
                let view = view(ctx, window, cx);
                view.update(cx, |v, _| v.ctx = Some(ctx.clone()));
                view.into_any_element()
            })
            .group(SettingsGroup::Server)
            .order(10)
            .keywords(t!("emotes-keywords").split(", "))
            .row(
                SettingsRow::new("server-emotes", t!("emotes-server-tab"))
                    .description(t!("emotes-server-description"))
                    .keywords(t!("emotes-keywords")),
            )
            .row(
                SettingsRow::new("add-emote", t!("emotes-add"))
                    .description(t!("emotes-add-description"))
                    .keywords(t!("emotes-add-keywords")),
            )
            .row(
                SettingsRow::new("discover-emotes", t!("emotes-discover"))
                    .description(t!("emotes-discover-description"))
                    .keywords(t!("emotes-discover-keywords")),
            ),
        );
    });
}

/// One view per session, so two windows signed in to different servers do
/// not reset each other's.
#[derive(Default)]
struct Views(HashMap<EntityId, Entity<EmotesView>>);
impl Global for Views {}

fn view(ctx: &SettingsCtx, window: &mut Window, cx: &mut App) -> Entity<EmotesView> {
    let key = ctx.session.entity_id();
    if let Some(view) = cx.default_global::<Views>().0.get(&key) {
        return view.clone();
    }
    let view = cx.new(|cx| EmotesView::new(ctx.session.clone(), window, cx));
    cx.default_global::<Views>().0.insert(key, view.clone());
    view
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tab {
    Server,
    Discover,
}

/// What the side form shows.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Panel {
    Closed,
    Upload,
    Import,
    Discover,
}

/// Something read from the server.
enum Load<T> {
    Loading,
    Failed(String),
    Ready(T),
}

/// A picture chosen to upload.
struct Picked {
    name: String,
    content_type: &'static str,
    bytes: Vec<u8>,
    still: Option<Vec<u8>>,
    preview: Arc<Image>,
}

/// Discover's page and how it was asked for.
struct Discovery {
    source: String,
    animation: String,
    /// The cursor of every page so far, the first `""`; the last is shown.
    cursors: Vec<String>,
    page: Option<c::DiscoveryResult>,
    loading: bool,
    error: Option<String>,
    /// Bumped by every request, so an older answer is dropped.
    generation: u64,
    /// The search text the page was asked with.
    query: String,
    started: bool,
}

pub struct EmotesView {
    session: Entity<Session>,
    ctx: Option<SettingsCtx>,
    /// The search landing last acted on.
    landed: Option<SharedString>,
    catalog: Load<c::EmoteCatalog>,
    catalog_generation: u64,
    tab: Tab,
    panel: Panel,
    search: Entity<InputState>,
    discover_search: Entity<InputState>,
    link: Entity<InputState>,
    shortcode: Entity<InputState>,
    aliases: Entity<InputState>,
    picked: Option<Picked>,
    selected: Option<c::DiscoveryItem>,
    discovery: Discovery,
    typing: u64,
    busy: bool,
    error: Option<String>,
    status: Option<&'static str>,
    _subscriptions: Vec<Subscription>,
}

impl EmotesView {
    fn new(session: Entity<Session>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = |placeholder: &'static str, window: &mut Window, cx: &mut Context<Self>| {
            cx.new(|cx| InputState::new(window, cx).placeholder(placeholder))
        };
        let search = input(t!("emotes-search-server"), window, cx);
        let discover_search = input(t!("emotes-search-discover"), window, cx);
        let link = input(api_placeholder(), window, cx);
        let shortcode = input(t!("emotes-shortcode-placeholder"), window, cx);
        let aliases = input("", window, cx);
        let mut subscriptions = Vec::new();
        for field in [&search, &link, &shortcode, &aliases] {
            subscriptions.push(cx.subscribe_in(field, window, |_, _, event, _, cx| {
                if let InputEvent::Change = event {
                    cx.notify();
                }
            }));
        }
        for field in [&link, &shortcode, &aliases] {
            subscriptions.push(cx.subscribe_in(
                field,
                window,
                |this: &mut Self, _, event, window, cx| {
                    if let InputEvent::PressEnter { .. } = event {
                        this.add(window, cx);
                    }
                },
            ));
        }
        subscriptions.push(cx.subscribe_in(
            &discover_search,
            window,
            |this: &mut Self, _, event, _, cx| {
                if let InputEvent::Change = event {
                    this.typed(cx);
                }
            },
        ));
        // A change from anywhere (this view, another device, an owner):
        // read the catalog again.
        subscriptions.push(cx.subscribe(&session, |this, _, event: &SessionEvent, cx| {
            if let SessionEvent::Changed(changes) = event
                && changes.topics.contains_key(topic::EMOTES)
            {
                this.load(cx);
            }
        }));
        let mut view = Self {
            session,
            ctx: None,
            landed: None,
            catalog: Load::Loading,
            catalog_generation: 0,
            tab: Tab::Server,
            panel: Panel::Closed,
            search,
            discover_search,
            link,
            shortcode,
            aliases,
            picked: None,
            selected: None,
            discovery: Discovery {
                source: String::new(),
                animation: "all".into(),
                cursors: vec![String::new()],
                page: None,
                loading: false,
                error: None,
                generation: 0,
                query: String::new(),
                started: false,
            },
            typing: 0,
            busy: false,
            error: None,
            status: None,
            _subscriptions: subscriptions,
        };
        view.load(cx);
        view
    }

    fn client(&self, cx: &App) -> Option<std::rc::Rc<ircord_client::Client>> {
        self.session.read(cx).client().cloned()
    }

    fn items(&self) -> &[c::Emote] {
        match &self.catalog {
            Load::Ready(c) => c.items.as_deref().unwrap_or_default(),
            _ => &[],
        }
    }

    fn can_add(&self) -> bool {
        matches!(&self.catalog, Load::Ready(c) if c.can_add || c.can_manage)
    }

    // ---- Loading ----

    fn load(&mut self, cx: &mut Context<Self>) {
        let Some(client) = self.client(cx) else {
            return;
        };
        self.catalog_generation += 1;
        let generation = self.catalog_generation;
        let task = client.request(|api| async move { api.emotes().await });
        cx.spawn(async move |this, cx| {
            let result = joined(task.await);
            let _ = this.update(cx, |this, cx| {
                if this.catalog_generation != generation {
                    return;
                }
                match result {
                    Ok(catalog) => this.catalog = Load::Ready(catalog),
                    // Keep a list already shown; say why only when there is none.
                    Err(e) if !matches!(this.catalog, Load::Ready(_)) => {
                        this.catalog = Load::Failed(e)
                    }
                    Err(_) => this.error = Some(t!("emotes-refresh-failed").into()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn discover(&mut self, cx: &mut Context<Self>) {
        let Some(client) = self.client(cx) else {
            return;
        };
        self.discovery.started = true;
        self.discovery.generation += 1;
        let generation = self.discovery.generation;
        self.discovery.loading = true;
        self.discovery.error = None;
        let query = Discover {
            source: self.discovery.source.clone(),
            query: self.discovery.query.clone(),
            animation: self.discovery.animation.clone(),
            cursor: self.discovery.cursors.last().cloned().unwrap_or_default(),
            limit: DISCOVER_PAGE,
        };
        cx.notify();
        let task = client.request(move |api| async move { api.discover_emotes(&query).await });
        cx.spawn(async move |this, cx| {
            let result = joined(task.await);
            let _ = this.update(cx, |this, cx| {
                if this.discovery.generation != generation {
                    return;
                }
                this.discovery.loading = false;
                match result {
                    Ok(page) => this.discovery.page = Some(page),
                    Err(e) => this.discovery.error = Some(e),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Discover from its first page, after a filter changed.
    fn restart(&mut self, cx: &mut Context<Self>) {
        self.discovery.cursors = vec![String::new()];
        self.discover(cx);
    }

    fn typed(&mut self, cx: &mut Context<Self>) {
        let text = self.discover_search.read(cx).value().trim().to_string();
        if text == self.discovery.query {
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
                    this.discovery.query = text;
                    this.restart(cx);
                }
            })
            .ok();
        })
        .detach();
    }

    // ---- Navigation ----

    pub fn set_tab(&mut self, tab: Tab, cx: &mut Context<Self>) {
        if tab == self.tab {
            return;
        }
        self.tab = tab;
        self.panel = Panel::Closed;
        if tab == Tab::Discover && !self.discovery.started {
            self.discover(cx);
        }
        cx.notify();
    }

    fn open_panel(&mut self, panel: Panel, window: &mut Window, cx: &mut Context<Self>) {
        if panel != self.panel {
            self.clear_form(window, cx);
        }
        self.panel = panel;
        self.error = None;
        self.status = None;
        cx.notify();
    }

    fn clear_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.picked = None;
        for field in [&self.link, &self.shortcode, &self.aliases] {
            field.update(cx, |f, cx| f.set_value("", window, cx));
        }
    }

    fn select(&mut self, item: c::DiscoveryItem, window: &mut Window, cx: &mut Context<Self>) {
        self.open_panel(Panel::Discover, window, cx);
        let code = suggest_shortcode(&item.name);
        self.shortcode
            .update(cx, |f, cx| f.set_value(code, window, cx));
        self.selected = Some(item);
        cx.notify();
    }

    // ---- Choosing a picture ----

    fn choose_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(t!("emotes-choose-file").into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = picked.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let read = cx
                .background_spawn(async move { std::fs::read(&path).ok() })
                .await;
            let _ = this.update_in(cx, |this, window, cx| match read {
                Some(bytes) => this.use_file(name, bytes, window, cx),
                None => {
                    this.error = Some(t!("emotes-open-failed").into());
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Take `bytes` as the picture to upload, if the server takes its type
    /// and size. Public for the tests, which cannot use the file dialog.
    pub fn use_file(
        &mut self,
        name: String,
        bytes: Vec<u8>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Load::Ready(catalog) = &self.catalog else {
            return;
        };
        let content_type =
            image_type(&bytes).filter(|t| catalog.types.iter().flatten().any(|ok| ok == t));
        let Some(content_type) = content_type else {
            self.error = Some(t!("emotes-invalid-type").into());
            cx.notify();
            return;
        };
        let max = catalog.max_bytes.max(0) as usize;
        if bytes.is_empty() || bytes.len() > max {
            self.error = Some(t!("emotes-invalid-size", size = kib(catalog.max_bytes)));
            cx.notify();
            return;
        }
        self.error = None;
        let stem = name.rsplit_once('.').map_or(name.as_str(), |(s, _)| s);
        if self.shortcode.read(cx).value().trim().is_empty() {
            let code = suggest_shortcode(stem);
            self.shortcode
                .update(cx, |f, cx| f.set_value(code, window, cx));
        }
        let format = match content_type {
            "image/png" => ImageFormat::Png,
            "image/gif" => ImageFormat::Gif,
            "image/webp" => ImageFormat::Webp,
            _ => ImageFormat::Jpeg,
        };
        let preview = Arc::new(Image::from_bytes(format, bytes.clone()));
        let still = still_frame(content_type, &bytes);
        self.picked = Some(Picked {
            name,
            content_type,
            bytes,
            still,
            preview,
        });
        cx.notify();
    }

    // ---- Changes ----

    /// Run `action` against the server; on success say `done` and read the
    /// catalog again.
    fn run<T: Send + 'static, Fut>(
        &mut self,
        cx: &mut Context<Self>,
        done: &'static str,
        request: impl FnOnce(ircord_client::api::Api) -> Fut + Send + 'static,
        after: impl FnOnce(&mut Self, &mut Window, &mut Context<Self>) + 'static,
        window: &mut Window,
    ) where
        Fut: Future<Output = Result<T, ApiError>> + Send + 'static,
    {
        let Some(client) = self.client(cx) else {
            return;
        };
        if self.busy {
            return;
        }
        self.busy = true;
        self.error = None;
        self.status = None;
        cx.notify();
        let task = client.request(request);
        cx.spawn_in(window, async move |this, cx| {
            let result = joined(task.await);
            let _ = this.update_in(cx, |this, window, cx| {
                this.busy = false;
                match result {
                    Ok(_) => {
                        this.status = Some(done);
                        after(this, window, cx);
                        this.load(cx);
                    }
                    Err(e) => this.error = Some(e),
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn add(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy || !self.can_add() {
            return;
        }
        let shortcode = self.shortcode.read(cx).value().trim().to_string();
        let aliases = split_aliases(self.aliases.read(cx).value().as_ref());
        match self.panel {
            Panel::Closed => {}
            Panel::Upload => {
                let Some(picked) = &self.picked else {
                    return;
                };
                if shortcode.is_empty() {
                    return;
                }
                let emote = NewEmote {
                    file_name: picked.name.clone(),
                    content_type: picked.content_type.into(),
                    bytes: picked.bytes.clone(),
                    still: picked.still.clone(),
                    shortcode,
                    aliases,
                };
                self.run(
                    cx,
                    t!("emotes-added"),
                    move |api| async move { api.upload_emote(&emote).await },
                    |this, window, cx| this.clear_form(window, cx),
                    window,
                );
            }
            Panel::Import => {
                let link = self.link.read(cx).value().to_string();
                let Some((provider, provider_id)) = parse_provider_link(&link) else {
                    self.error = Some(t!("emotes-paste-link").into());
                    cx.notify();
                    return;
                };
                let import = c::EmoteImport {
                    provider,
                    provider_id,
                    shortcode: Some(shortcode).filter(|s| !s.is_empty()),
                    aliases: Some(aliases),
                };
                self.run(
                    cx,
                    t!("emotes-imported"),
                    move |api| async move { api.import_emote(&import).await },
                    |this, window, cx| this.clear_form(window, cx),
                    window,
                );
            }
            Panel::Discover => {
                let Some(item) = self.selected.clone() else {
                    return;
                };
                if shortcode.is_empty() || is_installed(&item, self.items()) {
                    return;
                }
                let import = c::EmoteImport {
                    provider: api::import_provider(&item.provider),
                    provider_id: item.provider_id.clone(),
                    shortcode: Some(shortcode),
                    aliases: Some(aliases),
                };
                self.run(
                    cx,
                    t!("emotes-imported"),
                    move |api| async move { api.import_emote(&import).await },
                    move |this, _, cx| {
                        if let Some(selected) = &mut this.selected
                            && selected.provider_id == item.provider_id
                        {
                            selected.installed = true;
                        }
                        // The page's Installed marks.
                        this.discover(cx);
                    },
                    window,
                );
            }
        }
    }

    fn patch(
        &mut self,
        id: String,
        patch: c::EmotePatch,
        done: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.run(
            cx,
            done,
            move |api| async move { api.update_emote(&id, &patch).await },
            |_, _, _| {},
            window,
        );
    }

    fn ask_rename(&mut self, emote: c::Emote, window: &mut Window, cx: &mut Context<Self>) {
        let this = cx.entity();
        let old = emote.shortcode.clone();
        ui::prompt::prompt(
            window,
            cx,
            ui::prompt::Prompt {
                title: t!("emotes-rename-title", shortcode = emote.shortcode).into(),
                label: t!("emotes-rename-label").into(),
                initial: emote.shortcode.clone(),
                ok: t!("emotes-rename-confirm").into(),
            },
            move |text, window, cx| {
                let next = text.trim().to_string();
                if next.is_empty() || next == old {
                    return;
                }
                let id = emote.id.clone();
                this.update(cx, |v, cx| {
                    v.patch(
                        id,
                        c::EmotePatch {
                            shortcode: Some(next),
                            ..Default::default()
                        },
                        t!("emotes-renamed"),
                        window,
                        cx,
                    )
                });
            },
        );
    }

    fn ask_aliases(&mut self, emote: c::Emote, window: &mut Window, cx: &mut Context<Self>) {
        let this = cx.entity();
        // The canonical name is kept as an alias by the server; it is not
        // the person's to edit.
        let initial = emote
            .aliases
            .iter()
            .flatten()
            .filter(|a| **a != emote.name)
            .cloned()
            .collect::<Vec<_>>()
            .join(", ");
        ui::prompt::prompt(
            window,
            cx,
            ui::prompt::Prompt {
                title: t!("emotes-aliases-title", shortcode = emote.shortcode).into(),
                label: t!("emotes-aliases-prompt").into(),
                initial,
                ok: t!("emotes-save").into(),
            },
            move |text, window, cx| {
                let id = emote.id.clone();
                this.update(cx, |v, cx| {
                    v.patch(
                        id,
                        c::EmotePatch {
                            aliases: Some(split_aliases(&text)),
                            ..Default::default()
                        },
                        t!("emotes-aliases-saved"),
                        window,
                        cx,
                    )
                });
            },
        );
    }

    fn toggle(&mut self, emote: &c::Emote, window: &mut Window, cx: &mut Context<Self>) {
        let done = if emote.enabled {
            t!("emotes-disabled")
        } else {
            t!("emotes-enabled")
        };
        self.patch(
            emote.id.clone(),
            c::EmotePatch {
                enabled: Some(!emote.enabled),
                ..Default::default()
            },
            done,
            window,
            cx,
        );
    }

    fn ask_remove(&mut self, emote: c::Emote, window: &mut Window, cx: &mut Context<Self>) {
        let this = cx.entity();
        ui::confirm::confirm(
            window,
            cx,
            ui::confirm::Confirm {
                title: t!("emotes-remove-title", shortcode = emote.shortcode).into(),
                body: t!("emotes-remove-body", shortcode = emote.shortcode).into(),
                ok: t!("emotes-remove-confirm").into(),
                danger: true,
            },
            move |window, cx| {
                let id = emote.id.clone();
                this.update(cx, |v, cx| {
                    v.run(
                        cx,
                        t!("emotes-removed"),
                        move |api| async move { api.delete_emote(&id).await },
                        |_, _, _| {},
                        window,
                    )
                });
            },
        );
    }
}

fn api_placeholder() -> &'static str {
    "https://7tv.app/emotes/01GA29CZ2R000C36HNE7Z0DQXD"
}

/// A request's answer, with a lost task as its error text.
fn joined<T>(result: Result<Result<T, ApiError>, tokio::task::JoinError>) -> Result<T, String> {
    result
        .map_err(|e| e.to_string())
        .and_then(|r| r.map_err(|e| e.to_string()))
}

/// `max_bytes` in whole KiB, as the web client says it.
fn kib(bytes: i64) -> i64 {
    (bytes as f64 / 1024.0).round() as i64
}

/// A still PNG of the first frame of a PNG or WebP no larger than 1024 on
/// a side, for viewers with animation off. JPEGs never animate and the
/// server takes a GIF's first frame itself.
pub fn still_frame(content_type: &str, bytes: &[u8]) -> Option<Vec<u8>> {
    let format = match content_type {
        "image/png" => image::ImageFormat::Png,
        "image/webp" => image::ImageFormat::WebP,
        _ => return None,
    };
    let decoded = image::load_from_memory_with_format(bytes, format).ok()?;
    if decoded.width() > STILL_MAX || decoded.height() > STILL_MAX {
        return None;
    }
    let mut png = Vec::new();
    decoded
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .ok()?;
    Some(png)
}

// ---- Drawing ----

impl Render for EmotesView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme::get(cx).clone();
        let ctx = self.ctx.clone().unwrap_or(SettingsCtx {
            session: self.session.clone(),
            target: None,
        });
        // A search landing on a row opens what it names, once per landing.
        let target = ctx.target.as_ref().map(|t| t.id.clone());
        if target != self.landed {
            self.landed = target.clone();
            match target.as_deref() {
                Some("discover-emotes") => self.set_tab(Tab::Discover, cx),
                Some("server-emotes") => self.set_tab(Tab::Server, cx),
                // Like the web's `setting-reveal`: the search opens the form.
                Some("add-emote") if self.panel == Panel::Closed && self.can_add() => {
                    self.panel = Panel::Upload
                }
                _ => {}
            }
        }
        let origin = self.session.read(cx).server().cloned();
        let animate = self
            .client(cx)
            .map(|c| crate::features::klipy::animate(&c.read()))
            .unwrap_or(true);
        let here = Here {
            theme: &theme,
            origin: origin.as_ref(),
            animate,
        };
        let mut page = div().id("emote-settings").flex().flex_col().gap_3().child(
            div()
                .text_sm()
                .text_color(theme.muted)
                .child(t!("emotes-intro")),
        );
        if self.panel == Panel::Closed {
            page = page.children(self.notes(&theme));
        }
        page.child(self.tabs(&theme, cx))
            .child(self.toolbar(&ctx, &theme, cx))
            .when(self.panel != Panel::Closed, |d| {
                d.child(self.detail(&ctx, &here, cx))
            })
            .child(match self.tab {
                Tab::Server => self.server_list(&ctx, &here, cx).into_any_element(),
                Tab::Discover => self.discover_grid(&ctx, &here, cx).into_any_element(),
            })
    }
}

/// What every part of the page draws with.
struct Here<'a> {
    theme: &'a Theme,
    origin: Option<&'a ircord_client::ServerAddress>,
    animate: bool,
}

impl EmotesView {
    fn notes(&self, theme: &Theme) -> Vec<AnyElement> {
        let mut out = Vec::new();
        if let Some(e) = &self.error {
            out.push(note("emotes-error", e.clone(), theme.danger, true).into_any_element());
        }
        if let Some(s) = self.status {
            out.push(note("emotes-status", s, theme.online, false).into_any_element());
        }
        out
    }

    fn tabs(&self, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let count = self.items().len();
        div()
            .id("emote-tabs")
            .test_support()
            .role(Role::TabList)
            .aria_label(t!("emotes-browse"))
            .flex()
            .gap_1()
            .child(
                option(
                    "emotes-tab-server",
                    t!("emotes-server-tab-count", count = count),
                    self.tab == Tab::Server,
                    theme,
                )
                .role(Role::Tab)
                .test_support()
                .on_click(cx.listener(|this, _, _, cx| this.set_tab(Tab::Server, cx))),
            )
            .child(
                option(
                    "emotes-tab-discover",
                    t!("emotes-discover"),
                    self.tab == Tab::Discover,
                    theme,
                )
                .role(Role::Tab)
                .test_support()
                .on_click(cx.listener(|this, _, _, cx| this.set_tab(Tab::Discover, cx))),
            )
    }

    fn toolbar(&self, ctx: &SettingsCtx, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let search = match self.tab {
            Tab::Server => Input::new(&self.search)
                .id("emote-search")
                .aria_label(t!("emotes-search-server")),
            Tab::Discover => Input::new(&self.discover_search)
                .id("discover-search")
                .aria_label(t!("emotes-search-discover")),
        };
        let mut bar = div().flex().flex_col().gap_2().child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(div().flex_1().min_w_0().child(search))
                .when(self.can_add(), |d| {
                    d.child(
                        Button::new("emotes-upload")
                            .small()
                            .label(t!("emotes-upload-image"))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_panel(Panel::Upload, window, cx)
                            })),
                    )
                }),
        );
        if self.tab == Tab::Discover
            && let Some(page) = &self.discovery.page
        {
            bar = bar.child(self.filters(&page.filters, theme, cx));
        }
        let _ = ctx;
        bar.into_any_element()
    }

    fn filters(
        &self,
        filters: &c::DiscoveryFilters,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let sources = filters.sources.clone().unwrap_or_default();
        let animations = filters.animations.clone().unwrap_or_default();
        let mut row = div().flex().flex_wrap().items_center().gap_3();
        if sources.len() > 1 {
            let mut group = div()
                .id("emote-sources")
                .test_support()
                .role(Role::RadioGroup)
                .aria_label(t!("emotes-source-filter"))
                .flex()
                .flex_wrap()
                .gap_1();
            let all = std::iter::once((String::new(), t!("emotes-all-sources").to_string()));
            for (value, label) in all.chain(sources.iter().map(|s| (s.clone(), s.clone()))) {
                let selected = self.discovery.source == value;
                let id = format!(
                    "emote-source-{}",
                    if value.is_empty() { "all" } else { &value }
                );
                group = group.child(option(id, label, selected, theme).test_support().on_click(
                    cx.listener(move |this, _, _, cx| {
                        if this.discovery.source != value {
                            this.discovery.source = value.clone();
                            this.restart(cx);
                        }
                    }),
                ));
            }
            row = row.child(group);
        } else if let Some(only) = sources.first() {
            row = row.child(
                div()
                    .px_2()
                    .rounded(theme.radius)
                    .bg(theme.raised)
                    .text_sm()
                    .text_color(theme.muted)
                    .child(only.clone()),
            );
        }
        if !animations.is_empty() {
            let mut group = div()
                .id("emote-animations")
                .test_support()
                .role(Role::RadioGroup)
                .aria_label(t!("emotes-animation"))
                .flex()
                .gap_1();
            for value in animations {
                let label = match value.as_str() {
                    "all" => t!("emotes-animation-all"),
                    "static" => t!("emotes-animation-static"),
                    "animated" => t!("emotes-animation-animated"),
                    _ => continue,
                };
                let selected = self.discovery.animation == value;
                group = group.child(
                    option(format!("emote-animation-{value}"), label, selected, theme)
                        .test_support()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if this.discovery.animation != value {
                                this.discovery.animation = value.clone();
                                this.restart(cx);
                            }
                        })),
                );
            }
            row = row.child(group);
        }
        row
    }

    fn server_list(&self, ctx: &SettingsCtx, here: &Here, cx: &mut Context<Self>) -> AnyElement {
        let theme = here.theme;
        let body: AnyElement = match &self.catalog {
            Load::Loading => state("emotes-loading", t!("emotes-loading-server"), None, theme),
            Load::Failed(e) => state(
                "emotes-unavailable",
                t!("emotes-unavailable"),
                Some(t!("emotes-try-later-because", error = e).into()),
                theme,
            ),
            Load::Ready(catalog) => {
                let query = self.search.read(cx).value().to_string();
                let items = catalog.items.as_deref().unwrap_or_default();
                let visible: Vec<&c::Emote> =
                    items.iter().filter(|e| emote_matches(e, &query)).collect();
                if items.is_empty() {
                    let hint = if self.can_add() {
                        t!("emotes-empty-add")
                    } else {
                        t!("emotes-empty-browse")
                    };
                    state("emotes-none", t!("emotes-none"), Some(hint.into()), theme)
                } else if visible.is_empty() {
                    state(
                        "emotes-no-matches",
                        t!("emotes-no-matches"),
                        Some(t!("emotes-adjust-search").into()),
                        theme,
                    )
                } else {
                    let mut list = div()
                        .id("emote-list")
                        .test_support()
                        .role(Role::List)
                        .aria_label(t!("emotes-server-tab"))
                        .flex()
                        .flex_col()
                        .gap_1();
                    for emote in visible {
                        let manage = emote.can_manage || catalog.can_manage;
                        list = list.child(self.emote_row(emote, manage, here, cx));
                    }
                    list.into_any_element()
                }
            }
        };
        super::block(ctx, "server-emotes", t!("emotes-server-tab"), body, theme).into_any_element()
    }

    fn emote_row(
        &self,
        emote: &c::Emote,
        manage: bool,
        here: &Here,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = here.theme;
        let code = format!(":{}:", emote.shortcode);
        let picture = if here.animate {
            &emote.url
        } else {
            &emote.static_url
        };
        let picture = crate::message::emoji::resolve(picture, here.origin);
        let source: SharedString = if emote.source == c::EmoteSource::Upload {
            t!("emotes-uploaded").into()
        } else {
            provider_name(emote.source).unwrap_or_default().into()
        };
        let aliases = emote.aliases.clone().unwrap_or_default();
        let mut name = div().flex_1().min_w(px(160.)).flex().flex_col().child(
            div()
                .flex()
                .gap_1()
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.text)
                        .child(code.clone()),
                )
                .when(!emote.enabled, |d| {
                    d.child(
                        div()
                            .text_color(theme.muted)
                            .child(t!("emotes-disabled-tag")),
                    )
                }),
        );
        if !aliases.is_empty() {
            name = name.child(
                div()
                    .text_xs()
                    .text_color(theme.muted)
                    .child(t!("emotes-also", aliases = aliases.join(", "))),
            );
        }
        // Where it came from, and who made it, under the name.
        let mut credit = source.to_string();
        if let Some(creator) = emote.creator.as_deref().filter(|c| !c.is_empty()) {
            credit.push_str(" · ");
            credit.push_str(&t!("emotes-by", creator = creator));
        }
        name = name.child(div().text_xs().text_color(theme.muted).child(credit));
        let id = &emote.id;
        let mut row = div()
            .id(ElementId::Name(format!("emote-{id}").into()))
            .test_support()
            .role(Role::ListItem)
            .aria_label(SharedString::from(code.clone()))
            .flex()
            .items_center()
            .gap_3()
            .px_2()
            .py_1()
            .rounded_md()
            .hover(|d| d.bg(theme.row_hover))
            .when(!emote.enabled, |d| d.opacity(0.6))
            .child(
                div()
                    .size(px(32.))
                    .flex_none()
                    .when_some(picture, |d, url| {
                        d.child(crate::message::emoji::picture(url, theme.faint))
                    }),
            )
            .child(name);
        if manage {
            let busy = self.busy;
            let button = |kind: &str, label: &'static str| {
                Button::new(ElementId::Name(format!("emote-{kind}-{id}").into()))
                    .small()
                    .ghost()
                    .disabled(busy)
                    .label(label)
            };
            let (e1, e2, e3, e4) = (emote.clone(), emote.clone(), emote.clone(), emote.clone());
            row = row.child(
                div()
                    .flex()
                    .flex_none()
                    .gap_1()
                    .child(button("rename", t!("emotes-rename")).on_click(cx.listener(
                        move |this, _, window, cx| this.ask_rename(e1.clone(), window, cx),
                    )))
                    .child(
                        button("aliases", t!("emotes-edit-aliases")).on_click(cx.listener(
                            move |this, _, window, cx| this.ask_aliases(e2.clone(), window, cx),
                        )),
                    )
                    .child(
                        button(
                            "toggle",
                            if emote.enabled {
                                t!("emotes-disable")
                            } else {
                                t!("emotes-enable")
                            },
                        )
                        .on_click(
                            cx.listener(move |this, _, window, cx| this.toggle(&e3, window, cx)),
                        ),
                    )
                    .child(button("remove", t!("emotes-remove")).on_click(cx.listener(
                        move |this, _, window, cx| this.ask_remove(e4.clone(), window, cx),
                    ))),
            );
        }
        row
    }

    fn discover_grid(&self, ctx: &SettingsCtx, here: &Here, cx: &mut Context<Self>) -> AnyElement {
        let theme = here.theme;
        let d = &self.discovery;
        let mut body = div().flex().flex_col().gap_3();
        if d.page.as_ref().is_some_and(|p| p.stale) {
            body = body.child(
                div()
                    .text_sm()
                    .text_color(theme.warning)
                    .child(t!("emotes-stale")),
            );
        }
        if let Some(error) = &d.error {
            body = body.child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(note(
                        "discover-error",
                        t!("emotes-discovery-failed", error = error),
                        theme.danger,
                        true,
                    ))
                    .child(
                        Button::new("discover-retry")
                            .small()
                            .label(t!("emotes-retry"))
                            .on_click(cx.listener(|this, _, _, cx| this.restart(cx))),
                    ),
            );
        }
        let items = d
            .page
            .as_ref()
            .and_then(|p| p.items.clone())
            .unwrap_or_default();
        if d.loading {
            body = body.child(state("discover-loading", t!("emotes-loading"), None, theme));
        } else if d.error.is_none()
            && let Some(page) = &d.page
            && items.is_empty()
        {
            let paged = !page.next_cursor.is_empty() || d.cursors.len() > 1;
            body = body.child(state(
                "discover-empty",
                if paged {
                    t!("emotes-no-page-matches")
                } else {
                    t!("emotes-no-discover-matches")
                },
                Some(
                    if page.next_cursor.is_empty() {
                        t!("emotes-adjust-filters")
                    } else {
                        t!("emotes-more-page-matches")
                    }
                    .into(),
                ),
                theme,
            ));
        }
        if !d.loading && d.error.is_none() && !items.is_empty() {
            let mut grid = div()
                .id("discover-grid")
                .test_support()
                .role(Role::List)
                .aria_label(t!("emotes-discover"))
                .flex()
                .flex_wrap()
                .gap_2();
            for item in items {
                grid = grid.child(self.tile(item, here, cx));
            }
            body = body.child(grid);
        }
        if d.error.is_none()
            && let Some(page) = &d.page
            && (d.cursors.len() > 1 || !page.next_cursor.is_empty())
        {
            let (has_prev, has_next) = (d.cursors.len() > 1, !page.next_cursor.is_empty());
            body = body.child(
                div()
                    .flex()
                    .gap_2()
                    .child(
                        Button::new("discover-previous")
                            .small()
                            .label(t!("emotes-previous"))
                            .disabled(!has_prev || d.loading)
                            .on_click(cx.listener(|this, _, _, cx| {
                                if this.discovery.cursors.len() > 1 && !this.discovery.loading {
                                    this.discovery.cursors.pop();
                                    this.discover(cx);
                                }
                            })),
                    )
                    .child(
                        Button::new("discover-next")
                            .small()
                            .label(t!("emotes-next"))
                            .disabled(!has_next || d.loading)
                            .on_click(cx.listener(|this, _, _, cx| {
                                let next = this
                                    .discovery
                                    .page
                                    .as_ref()
                                    .map(|p| p.next_cursor.clone())
                                    .unwrap_or_default();
                                if !next.is_empty() && !this.discovery.loading {
                                    this.discovery.cursors.push(next);
                                    this.discover(cx);
                                }
                            })),
                    ),
            );
        }
        super::block(ctx, "discover-emotes", t!("emotes-discover"), body, theme).into_any_element()
    }

    fn tile(
        &self,
        item: c::DiscoveryItem,
        here: &Here,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = here.theme;
        let installed = is_installed(&item, self.items());
        let selected = self.panel == Panel::Discover
            && self
                .selected
                .as_ref()
                .is_some_and(|s| s.provider_id == item.provider_id && s.provider == item.provider);
        let label = format!(":{}:", item.name);
        let picture = crate::message::emoji::resolve(&item.preview_url, here.origin);
        let id = format!("discover-{}-{}", item.provider, item.provider_id);
        div()
            .id(ElementId::Name(id.into()))
            .test_support()
            .role(Role::Button)
            .aria_label(SharedString::from(label.clone()))
            .aria_selected(selected)
            .w(px(104.))
            .p_2()
            .flex()
            .flex_col()
            .items_center()
            .gap_1()
            .rounded(theme.radius)
            .border_1()
            .border_color(if selected { theme.accent } else { theme.border })
            .bg(theme.raised)
            .cursor_pointer()
            .hover(|d| d.bg(theme.hover))
            .child(div().size(px(48.)).when_some(picture, |d, url| {
                d.child(crate::message::emoji::picture(url, theme.faint))
            }))
            .child(
                div()
                    .w_full()
                    .text_xs()
                    .text_center()
                    .text_ellipsis()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_color(theme.text)
                    .child(label),
            )
            .when(item.animated, |d| {
                d.child(
                    div()
                        .text_xs()
                        .text_color(theme.muted)
                        .child(t!("emotes-animated")),
                )
            })
            .when(installed, |d| {
                d.child(
                    div()
                        .text_xs()
                        .text_color(theme.online)
                        .child(t!("emotes-installed")),
                )
            })
            .on_click(cx.listener(move |this, _, window, cx| this.select(item.clone(), window, cx)))
    }

    /// The form: upload, import a link, or the Discover pick.
    fn detail(&self, ctx: &SettingsCtx, here: &Here, cx: &mut Context<Self>) -> AnyElement {
        let theme = here.theme;
        let heading: SharedString = match self.panel {
            Panel::Discover => self
                .selected
                .as_ref()
                .map(|s| s.name.clone())
                .unwrap_or_default()
                .into(),
            Panel::Upload => t!("emotes-upload-image").into(),
            _ => t!("emotes-import-provider").into(),
        };
        let mut form = div()
            .id("emote-form")
            .test_support()
            .role(Role::Form)
            .aria_label(heading.clone())
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.text)
                            .child(heading),
                    )
                    .child(
                        Button::new("emote-form-close")
                            .small()
                            .ghost()
                            .label(t!("emotes-close"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.panel = Panel::Closed;
                                cx.notify();
                            })),
                    ),
            );
        if self.panel != Panel::Discover {
            form = form.child(
                div()
                    .id("emote-how")
                    .test_support()
                    .role(Role::RadioGroup)
                    .aria_label(t!("emotes-how-to-add"))
                    .flex()
                    .gap_1()
                    .child(
                        option(
                            "emote-how-upload",
                            t!("emotes-upload-image"),
                            self.panel == Panel::Upload,
                            theme,
                        )
                        .test_support()
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.open_panel(Panel::Upload, window, cx)
                        })),
                    )
                    .child(
                        option(
                            "emote-how-import",
                            t!("emotes-import-provider"),
                            self.panel == Panel::Import,
                            theme,
                        )
                        .test_support()
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.open_panel(Panel::Import, window, cx)
                        })),
                    ),
            );
        }
        let max = match &self.catalog {
            Load::Ready(c) => kib(c.max_bytes),
            _ => 0,
        };
        form = match self.panel {
            Panel::Upload => form.child(
                ui::field::field(
                    t!("emotes-image"),
                    div()
                        .flex()
                        .items_center()
                        .gap_3()
                        .when_some(self.picked.as_ref(), |d, p| {
                            d.child(
                                img(p.preview.clone())
                                    .size(px(48.))
                                    .object_fit(ObjectFit::Contain),
                            )
                            .child(
                                div()
                                    .id("emote-file-name")
                                    .test_support()
                                    .role(Role::Status)
                                    .aria_label(SharedString::from(p.name.clone()))
                                    .text_sm()
                                    .text_color(theme.text)
                                    .child(p.name.clone()),
                            )
                        })
                        .child(
                            Button::new("emote-choose-file")
                                .small()
                                .label(if self.picked.is_some() {
                                    t!("emotes-choose-another")
                                } else {
                                    t!("emotes-choose-file")
                                })
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.choose_file(window, cx)),
                                ),
                        ),
                    theme,
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted)
                        .child(t!("emotes-image-help", size = max)),
                ),
            ),
            Panel::Import => form.child(
                ui::field::field(
                    t!("emotes-source"),
                    Input::new(&self.link)
                        .id("emote-link")
                        .aria_label(t!("emotes-source")),
                    theme,
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted)
                        .child(t!("emotes-source-help")),
                ),
            ),
            _ => match &self.selected {
                Some(item) => form.child(self.provider_preview(item, here)),
                None => form,
            },
        };
        let shortcode = self.shortcode.read(cx).value().trim().to_string();
        form = form
            .child(ui::field::field(
                t!("emotes-shortcode"),
                Input::new(&self.shortcode)
                    .id("emote-shortcode")
                    .aria_label(t!("emotes-shortcode")),
                theme,
            ))
            .child(ui::field::field(
                t!("emotes-aliases-label"),
                Input::new(&self.aliases)
                    .id("emote-aliases")
                    .aria_label(t!("emotes-aliases-label")),
                theme,
            ));
        if let Some(preview) = self.chat_preview(&shortcode, here, cx) {
            form = form.child(preview);
        }
        form = form.children(self.notes(theme));
        let installed = self.panel == Panel::Discover
            && self
                .selected
                .as_ref()
                .is_some_and(|s| is_installed(s, self.items()));
        let ready = match self.panel {
            Panel::Upload => self.picked.is_some() && !shortcode.is_empty(),
            Panel::Import => !self.link.read(cx).value().trim().is_empty(),
            Panel::Discover => self.selected.is_some() && !installed && !shortcode.is_empty(),
            Panel::Closed => false,
        };
        form = form.child(
            div().flex().justify_end().child(
                Button::new("emote-add")
                    .small()
                    .primary()
                    .disabled(self.busy || !self.can_add() || !ready)
                    .label(if installed {
                        t!("emotes-installed")
                    } else {
                        t!("emotes-add-to-server")
                    })
                    .on_click(cx.listener(|this, _, window, cx| this.add(window, cx))),
            ),
        );
        super::block(ctx, "add-emote", t!("emotes-add"), form, theme).into_any_element()
    }

    fn provider_preview(&self, item: &c::DiscoveryItem, here: &Here) -> impl IntoElement {
        let theme = here.theme;
        let picture = crate::message::emoji::resolve(&item.preview_url, here.origin);
        let mut from = t!("emotes-from", provider = item.provider);
        if !item.creator.is_empty() {
            from.push(' ');
            from.push_str(&t!("emotes-by", creator = item.creator));
        }
        div()
            .flex()
            .items_center()
            .gap_3()
            .child(
                div()
                    .size(px(64.))
                    .flex_none()
                    .when_some(picture, |d, url| {
                        d.child(crate::message::emoji::picture(url, theme.faint))
                    }),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .text_sm()
                    .text_color(theme.muted)
                    .child(from)
                    .when(item.animated, |d| d.child(t!("emotes-animated")))
                    .when(!item.license.is_empty(), |d| {
                        d.child(t!("emotes-license", license = item.license))
                    }),
            )
    }

    /// How the emote will look in a message, once there is a picture.
    fn chat_preview(&self, shortcode: &str, here: &Here, cx: &App) -> Option<AnyElement> {
        let theme = here.theme;
        let picture: AnyElement = match self.panel {
            Panel::Upload => {
                let p = self.picked.as_ref()?;
                img(p.preview.clone())
                    .size_full()
                    .object_fit(ObjectFit::Contain)
                    .into_any_element()
            }
            Panel::Discover => {
                let item = self.selected.as_ref()?;
                let url = crate::message::emoji::resolve(&item.preview_url, here.origin)?;
                crate::message::emoji::picture(url, theme.faint)
            }
            _ => return None,
        };
        let me = self
            .client(cx)
            .map(|c| {
                let w = c.read();
                let u = &w.user;
                if u.display_name.is_empty() {
                    u.nick.clone()
                } else {
                    u.display_name.clone()
                }
            })
            .unwrap_or_default();
        Some(
            div()
                .id("emote-chat-preview")
                .test_support()
                .role(Role::Group)
                .aria_label(t!("emotes-chat-preview"))
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::BOLD)
                        .text_color(theme.muted)
                        .child(t!("emotes-chat-preview").to_uppercase()),
                )
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .child(ui::avatar::avatar(&me, None, px(32.), theme))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .child(
                                    div()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(theme.text)
                                        .child(me.clone()),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap_1()
                                        .text_color(theme.text)
                                        .child(t!("emotes-preview-message"))
                                        .child(div().size(px(22.)).child(picture))
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(theme.muted)
                                                .child(format!(":{shortcode}:")),
                                        ),
                                ),
                        ),
                )
                .into_any_element(),
        )
    }
}

/// A choice button: tabs, filters, how to add.
fn option(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    selected: bool,
    theme: &Theme,
) -> Stateful<Div> {
    let id: SharedString = id.into();
    let label: SharedString = label.into();
    div()
        .id(ElementId::Name(id))
        .role(Role::RadioButton)
        .aria_label(label.clone())
        .aria_selected(selected)
        .px_3()
        .py_1()
        .rounded(theme.radius)
        .border_1()
        .text_sm()
        .cursor_pointer()
        .when(selected, |d| {
            d.bg(theme.accent)
                .border_color(theme.accent)
                .text_color(theme.on_accent)
        })
        .when(!selected, |d| {
            d.bg(theme.raised)
                .border_color(theme.control_border)
                .text_color(theme.text)
                .hover(|s| s.bg(theme.hover))
        })
        .child(label)
}

/// A line of feedback: an error (an alert) or what just happened (a status).
fn note(
    id: &'static str,
    text: impl Into<SharedString>,
    color: Hsla,
    alert: bool,
) -> impl IntoElement {
    let text: SharedString = text.into();
    div()
        .id(id)
        .test_support()
        .role(if alert { Role::Alert } else { Role::Status })
        .aria_label(text.clone())
        .text_sm()
        .text_color(color)
        .child(text)
}

/// A list's loading, empty or failed state.
fn state(
    id: &'static str,
    title: &'static str,
    body: Option<SharedString>,
    theme: &Theme,
) -> AnyElement {
    div()
        .id(id)
        .test_support()
        .role(Role::Status)
        .aria_label(title)
        .flex()
        .flex_col()
        .items_center()
        .gap_1()
        .p_4()
        .child(
            div()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.text)
                .child(title),
        )
        .when_some(body, |d, body| {
            d.child(div().text_sm().text_color(theme.muted).child(body))
        })
        .into_any_element()
}

/// Make the view for `session` show `tab`, for the tests.
pub fn show_tab(session: &Entity<Session>, tab: Tab, cx: &mut App) {
    let view = cx
        .try_global::<Views>()
        .and_then(|v| v.0.get(&session.entity_id()).cloned());
    if let Some(view) = view {
        view.update(cx, |v, cx| v.set_tab(tab, cx));
    }
}

/// Hand the view for `session` a picture, as the file dialog would.
pub fn choose(
    session: &Entity<Session>,
    name: &str,
    bytes: Vec<u8>,
    window: &mut Window,
    cx: &mut App,
) {
    let view = cx
        .try_global::<Views>()
        .and_then(|v| v.0.get(&session.entity_id()).cloned());
    if let Some(view) = view {
        let name = name.to_string();
        view.update(cx, |v, cx| v.use_file(name, bytes, window, cx));
    }
}

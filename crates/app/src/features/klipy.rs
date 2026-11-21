//! GIF search through Klipy, and /gif (package E1), with the rest of emoji
//! and GIFs wired in from here:
//!
//! - the custom emoji catalogs load at sign-in and again when
//!   `emotes.updated` names a new version; the picker library (saved GIFs,
//!   use counts, skin tone) is the watched `library` document;
//! - custom `:name:` in messages draw as pictures (`message/emoji.rs`);
//! - the composer gets GIF and emoji buttons and the picker above the box
//!   (`composer/expressions.rs`); Ctrl+G (N1's `Gifs` key) opens the GIFs;
//! - `:na` in the box suggests emoji;
//! - "React" in the message menu opens the full emoji picker;
//! - `/gif cats` sends a random matching GIF.
//!
//! Klipy is asked directly with the instance's public app key, as the web
//! client does. The backend does not serve the key; the app is built with
//! it (`IRCORD_KLIPY_KEY` at build time, or at run time for a test build).
//! Without a key the GIF tab says so and saved GIFs still work.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use gpui_kit::prelude::*;
use gpui_kit::*;
use ircord_client::Client;
use ircord_client::api::emotes::klipy::{self as api, Klipy, KlipyError};
use ircord_client::client::random_id;
use ircord_client::commands::View;
use ircord_client::contract as c;
use ircord_client::documents::LIBRARY;
use ircord_client::features::Context as Allowed;
use ircord_client::store::documents::document_id;
use ircord_client::store::emoji;
use ircord_client::store::{Changes, Connection, PendingSend, SendState, Workspace, topic};

use crate::composer::expressions::{self, PickerView, Purpose, Tab};
use crate::composer::{self, ComposerView};
use crate::features::commands::{self, ViewRequest};
use crate::features::message_actions::{self, Mode};
use crate::features::shortcuts::Gifs;
use crate::message::emoji as pictures;
use crate::registry::{self, ComposerCtx, MessageCtx, Slot, Suggestion, SuggestionProvider};
use crate::session::{Session, SessionEvent};
use crate::sidebar::layout;
use crate::theme;
use crate::ui;

/// Suggestions `:na` offers at most.
const SUGGESTIONS: usize = 8;

#[derive(Default)]
struct Expressions {
    /// The open picker over a composer, and its conversation.
    open: Option<(String, Entity<PickerView>)>,
    /// The composer last drawn, for the GIF key.
    composer: Option<ComposerCtx>,
    /// Clients whose library this module watches.
    watched: HashSet<usize>,
    /// Conversations with a `/gif` lookup running.
    lookups: HashSet<String>,
    /// Klipy's address and key, when set by [`configure`].
    klipy: Option<(String, String)>,
}

impl Global for Expressions {}

pub fn init(cx: &mut App) {
    cx.default_global::<Expressions>();
    cx.observe_new::<Session>(|_, _, cx| {
        cx.subscribe_self(|session: &mut Session, event: &SessionEvent, cx| {
            let SessionEvent::Changed(changes) = event else {
                return;
            };
            let Some(client) = session.client().cloned() else {
                return;
            };
            if changes.reset || changes.touches(emoji::TOPIC) {
                load(&client);
                watch_library(&client, cx);
            }
            if changes.touches(emoji::TOPIC) {
                let names = picture_map(&client.read(), session.server());
                pictures::set(names, cx);
            }
            if changes.touches_id(topic::DOCUMENTS, document_id(LIBRARY.document)) {
                refresh_picker(cx);
            }
        })
        .detach();
    })
    .detach();
    registry::register(cx, |r| {
        r.inline_renderers.add(pictures::renderer());
        let c = &mut r.composer;
        c.above.add(Slot::new("expressions", picker_slot).order(20));
        c.buttons.add(Slot::new("gif", gif_button).order(20));
        c.buttons.add(Slot::new("emoji", emoji_button).order(30));
        c.suggestions.add(SuggestionProvider {
            id: "emoji".into(),
            order: 30,
            suggest: Rc::new(suggest),
        });
        // A1's "React" opens the full picker; its checks stay as they are.
        if let Some(react) = r.message_menu.get("react").cloned() {
            let unavailable = react.unavailable.clone();
            let mut item = react;
            item.run = Rc::new(move |ctx: &MessageCtx, window, cx| {
                if let Some(why) = unavailable.as_ref().and_then(|u| u(ctx, cx)) {
                    return ui::toast::info(window, cx, why);
                }
                open_react(ctx.clone(), window, cx);
            });
            r.message_menu.add(item);
        }
    });
    cx.on_action(|_: &Gifs, cx| layout::with_window(cx, toggle_gifs_from_key));
    cx.intercept_keystrokes(on_key).detach();
    commands::on_view(cx, View::Gif, gif_command);
}

/// Point GIF search at `base` with `key` (tests use the canned server).
pub fn configure(cx: &mut App, base: impl Into<String>, key: impl Into<String>) {
    cx.default_global::<Expressions>().klipy = Some((base.into(), key.into()));
}

/// The configured Klipy, if there is a key.
pub fn klipy(cx: &App) -> Option<Klipy> {
    let (base, key) = match cx.try_global::<Expressions>().and_then(|e| e.klipy.clone()) {
        Some(set) => set,
        None => {
            let key = std::env::var("IRCORD_KLIPY_KEY")
                .ok()
                .or_else(|| option_env!("IRCORD_KLIPY_KEY").map(str::to_string))
                .unwrap_or_default();
            (api::BASE.to_string(), key)
        }
    };
    let klipy = Klipy::new(base, key).locale(locale());
    klipy.configured().then_some(klipy)
}

fn locale() -> String {
    std::env::var("LANG")
        .ok()
        .and_then(|l| l.split('.').next().map(str::to_string))
        .filter(|l| !l.is_empty() && l != "C" && l != "POSIX")
        .unwrap_or_else(|| "en".into())
}

pub fn error_text(e: &KlipyError) -> String {
    match e {
        KlipyError::NotConfigured => t!("emoji-gif-not-configured"),
        KlipyError::Unreachable => t!("emoji-gif-unreachable"),
        KlipyError::Busy => t!("emoji-gif-busy"),
        KlipyError::KeyRejected => t!("emoji-gif-key-rejected"),
        KlipyError::LoadFailed => t!("emoji-gif-load-failed"),
        KlipyError::Unreadable => t!("emoji-gif-unreadable"),
        KlipyError::UnsafeGif => t!("emoji-gif-unsafe"),
    }
    .to_string()
}

// ---- Loading ----

/// Load the custom catalogs if they are missing or out of date.
fn load(client: &Client) {
    let mut start = false;
    client.change(|w| {
        start = w.emoji.begin_load();
        Changes::default()
    });
    if !start {
        return;
    }
    client.call(
        |api| async move {
            let (instance, bundled) = futures::join!(api.emotes(), api.app_emoji_catalog());
            Ok::<_, ircord_client::api::ApiError>((
                instance.map_err(|e| e.to_string()),
                bundled.map_err(|e| e.to_string()),
            ))
        },
        |w, result| match result {
            Ok((instance, bundled)) => w.emoji.load_done(instance, bundled),
            Err(e) => w.emoji.load_done(Err(e.to_string()), Err(e.to_string())),
        },
    );
}

fn watch_library(client: &Rc<Client>, cx: &mut App) {
    let key = Rc::as_ptr(client) as usize;
    if cx.default_global::<Expressions>().watched.insert(key) {
        client.watch_document(&LIBRARY);
    }
}

/// `name` → the picture `:name:` draws.
fn picture_map(
    w: &Workspace,
    origin: Option<&ircord_client::ServerAddress>,
) -> HashMap<String, SharedString> {
    let animate = animate(w);
    w.emoji
        .names()
        .filter_map(|(name, item)| {
            Some((
                name.to_string(),
                pictures::picture_url(item, animate, origin)?,
            ))
        })
        .collect()
}

/// Whether animated emoji and GIFs play (Appearance, S1; on by default).
pub fn animate(w: &Workspace) -> bool {
    w.documents
        .appearance
        .value()
        .and_then(|a| a.settings.as_ref())
        .and_then(|s| s.animate_media)
        .unwrap_or(true)
}

fn refresh_picker(cx: &mut App) {
    let open = cx
        .try_global::<Expressions>()
        .and_then(|e| e.open.as_ref().map(|(_, v)| v.clone()));
    if let Some(view) = open {
        view.update(cx, |_, cx| cx.notify());
    }
    cx.refresh_windows();
}

// ---- The library ----

fn library(session: &Entity<Session>, cx: &App) -> Option<c::Library> {
    let client = session.read(cx).client()?.clone();
    client.read().documents.library.value().cloned()
}

pub fn skin_tone(session: &Entity<Session>, cx: &App) -> &'static str {
    emoji::skin_tone(library(session, cx).as_ref())
}

pub fn set_skin_tone(session: &Entity<Session>, tone: &str, cx: &mut App) {
    let Some(client) = session.read(cx).client().cloned() else {
        return;
    };
    let mut next = library(session, cx).unwrap_or_default();
    next.skin_tone = emoji::tone_value(tone);
    client.edit_document(&LIBRARY, next);
}

/// Count a pick of `id` toward "Frequently used": shown at once, then sent.
pub fn record_pick(session: &Entity<Session>, id: &str, cx: &mut App) {
    let Some(client) = session.read(cx).client().cloned() else {
        return;
    };
    let id = id.to_string();
    let now = chrono_now();
    let picked = id.clone();
    client.change(move |w| {
        let Some(mut lib) = w.documents.library.value().cloned() else {
            return Changes::default();
        };
        lib.emoji_usage = emoji::record_usage(&lib.emoji_usage, &[picked], now);
        w.documents.library.read_done(Ok(lib));
        Changes::topic_id(topic::DOCUMENTS, document_id(LIBRARY.document))
    });
    let patch = c::LibraryPatch {
        recent: Some(vec![id.clone()]),
        uses: Some([(id, 1)].into()),
        ..Default::default()
    };
    client.call(
        move |api| async move { api.update_library(&patch).await },
        library_answered,
    );
}

fn chrono_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or_default()
}

fn library_answered(
    w: &mut Workspace,
    result: Result<c::Library, ircord_client::api::ApiError>,
) -> Changes {
    match result {
        Ok(lib) => {
            w.documents.library.read_done(Ok(lib));
            Changes::topic_id(topic::DOCUMENTS, document_id(LIBRARY.document))
        }
        Err(e) => Changes {
            notices: vec![e.to_string()],
            ..Default::default()
        },
    }
}

/// Your saved GIFs, most recently saved first.
pub fn saved_gifs(session: &Entity<Session>, cx: &App) -> Vec<c::GIF> {
    library(session, cx)
        .map(|l| {
            l.gifs
                .into_iter()
                .map(|g| c::GIF {
                    id: g.id,
                    slug: g.slug,
                    title: g.title,
                    url: g.url,
                    preview_url: g.preview_url,
                    still_url: g.still_url,
                    width: g.width,
                    height: g.height,
                    provider: g.provider,
                })
                .collect()
        })
        .unwrap_or_default()
}

pub fn is_saved(session: &Entity<Session>, id: &str, cx: &App) -> bool {
    library(session, cx).is_some_and(|l| l.gifs.iter().any(|g| g.id == id))
}

/// Save `gif`, or forget it when it is saved.
pub fn toggle_saved(session: &Entity<Session>, gif: c::GIF, window: &mut Window, cx: &mut App) {
    let Some(client) = session.read(cx).client().cloned() else {
        return;
    };
    if library(session, cx).is_none() {
        return ui::toast::info(window, cx, t!("emoji-loading"));
    }
    if is_saved(session, &gif.id, cx) {
        let id = gif.id.clone();
        client.call(
            move |api| async move { api.delete_gif(&id).await },
            library_answered,
        );
    } else {
        let body = c::SavePickerGIF {
            id: gif.id,
            slug: gif.slug,
            title: gif.title,
            url: gif.url,
            preview_url: gif.preview_url,
            still_url: gif.still_url,
            width: gif.width,
            height: gif.height,
            provider: gif.provider,
        };
        client.call(
            move |api| async move { api.save_gif(&body).await },
            library_answered,
        );
    }
}

// ---- Sending a GIF ----

/// Send `gif` to `conversation`, replying when the box is replying. False
/// (with the reason shown) when it cannot be sent now.
pub fn send_gif(
    session: &Entity<Session>,
    conversation: &str,
    gif: c::GIF,
    query: &str,
    window: &mut Window,
    cx: &mut App,
) -> bool {
    let Some(client) = session.read(cx).client().cloned() else {
        return false;
    };
    let reason = {
        let w = client.read();
        Allowed::of(&w, conversation, w.connection == Some(Connection::Live)).send_reason()
    };
    if let Some(reason) = reason {
        ui::toast::info(window, cx, message_actions::reason_text(&reason));
        return false;
    }
    if !api::valid(&gif) {
        ui::toast::error(window, cx, t!("emoji-gif-unsafe"));
        return false;
    }
    let reply = match message_actions::mode(conversation, cx) {
        Some(Mode::Reply { message, .. }) => Some(message),
        _ => None,
    };
    let pending = PendingSend {
        client_id: random_id(),
        conversation_id: conversation.to_string(),
        text: gif.url.clone(),
        action: false,
        state: SendState::Sending,
    };
    client.typing(conversation, false);
    client.change(|w| w.queue_send(pending.clone()));
    let body = c::MessageInputBody {
        client_id: pending.client_id.clone(),
        text: gif.url.clone(),
        kind: Some(c::MessageInputBodyKind::Message),
        gif: Some(gif.clone()),
        reply_to_id: reply.clone(),
        ..Default::default()
    };
    let (id, client_id) = (conversation.to_string(), pending.client_id);
    client.call(
        move |api| async move { api.send_message(&id, &body).await },
        move |w, result| match result {
            Ok(response) => w.send_answered(&response),
            Err(e) => w.fail_send(&client_id, &e.to_string()).unwrap_or_default(),
        },
    );
    if reply.is_some() {
        message_actions::cancel(conversation, window, cx);
    }
    if let Some(klipy) = klipy(cx) {
        let query = query.to_string();
        session
            .read(cx)
            .runtime
            .spawn(async move { klipy.share(&gif, &query).await });
    }
    true
}

/// `/gif cats`: search, pick one result at random and send it where the
/// command was typed, even if you moved on meanwhile. One lookup per
/// conversation at a time.
fn gif_command(request: &ViewRequest, window: &mut Window, cx: &mut App) {
    let query = request.args.trim().to_string();
    if query.is_empty() {
        return ui::toast::info(window, cx, t!("emoji-gif-usage"));
    }
    let Some(klipy) = klipy(cx) else {
        return ui::toast::info(window, cx, t!("emoji-gif-not-configured"));
    };
    let conversation = request.conversation.clone();
    if !cx
        .default_global::<Expressions>()
        .lookups
        .insert(conversation.clone())
    {
        return ui::toast::info(window, cx, t!("emoji-gif-pending"));
    }
    let session = request.session.clone();
    let search = query.clone();
    let task = session
        .read(cx)
        .runtime
        .spawn(async move { klipy.gifs(&search, 1).await });
    window
        .spawn(cx, async move |cx| {
            let result = task.await;
            cx.update(|window, cx| {
                cx.default_global::<Expressions>()
                    .lookups
                    .remove(&conversation);
                match result {
                    Ok(Ok(page)) if page.items.is_empty() => {
                        ui::toast::info(window, cx, t!("emoji-gif-no-match", query = query))
                    }
                    Ok(Ok(page)) => {
                        let gif = page.items[pick_random(page.items.len())].clone();
                        send_gif(&session, &conversation, gif, &query, window, cx);
                    }
                    Ok(Err(e)) => ui::toast::error(window, cx, error_text(&e)),
                    Err(e) => ui::toast::error(window, cx, e.to_string()),
                }
            })
            .ok();
        })
        .detach();
}

/// A position below `len`, uniformly enough for picking a GIF.
fn pick_random(len: usize) -> usize {
    let seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as usize ^ d.as_secs() as usize)
        .unwrap_or_default();
    seed % len.max(1)
}

// ---- The picker over the composer ----

fn remember(ctx: &ComposerCtx, cx: &mut App) {
    cx.default_global::<Expressions>().composer = Some(ctx.clone());
}

fn open_view(cx: &App) -> Option<(String, Entity<PickerView>)> {
    cx.try_global::<Expressions>()?.open.clone()
}

/// Whether the picker is open over the composer, and on which tab.
pub fn open_tab(cx: &App) -> Option<Tab> {
    open_view(cx).map(|(_, v)| v.read(cx).tab)
}

/// Open the picker on `tab` over the composer, switch it to `tab`, or close
/// it when it already shows `tab`.
pub fn toggle(ctx: &ComposerCtx, tab: Tab, window: &mut Window, cx: &mut App) {
    if let Some((conversation, view)) = open_view(cx)
        && conversation == ctx.conversation
    {
        if view.read(cx).tab == tab {
            return close(window, cx);
        }
        view.update(cx, |v, cx| v.set_tab(tab, window, cx));
        let input = view.read(cx).search.clone();
        input.update(cx, |s, cx| s.focus(window, cx));
        return;
    }
    let purpose = Purpose::Composer {
        conversation: ctx.conversation.clone(),
        composer: ctx.composer.downgrade(),
    };
    let session = ctx.session.clone();
    let view = cx.new(|cx| PickerView::new(session, purpose, tab, window, cx));
    let input = view.read(cx).search.clone();
    cx.default_global::<Expressions>().open = Some((ctx.conversation.clone(), view));
    input.update(cx, |s, cx| s.focus(window, cx));
    cx.refresh_windows();
}

/// Close the picker over the composer and give the box the focus back.
pub fn close(window: &mut Window, cx: &mut App) {
    let e = cx.default_global::<Expressions>();
    if e.open.take().is_none() {
        return;
    }
    let composer = e.composer.as_ref().map(|c| c.composer.clone());
    if let Some(composer) = composer {
        focus_box(&composer, window, cx);
    }
    cx.refresh_windows();
}

fn focus_box(composer: &Entity<ComposerView>, window: &mut Window, cx: &mut App) {
    let input = composer.read(cx).input.clone();
    input.update(cx, |s, cx| s.focus(window, cx));
}

/// Escape in the picker's search closes the picker (the field takes the key
/// before the picker sees it).
fn on_key(event: &KeystrokeEvent, window: &mut Window, cx: &mut App) {
    let key = &event.keystroke;
    if key.key != "escape" || key.modifiers.modified() {
        return;
    }
    let Some((_, view)) = open_view(cx) else {
        return;
    };
    let search = view.read(cx).search.clone();
    if search.read(cx).focus_handle(cx).is_focused(window) {
        close(window, cx);
    }
}

fn toggle_gifs_from_key(window: &mut Window, cx: &mut App) {
    let Some(ctx) = cx
        .try_global::<Expressions>()
        .and_then(|e| e.composer.clone())
    else {
        return;
    };
    toggle(&ctx, Tab::Gifs, window, cx);
}

fn picker_slot(ctx: &ComposerCtx, _window: &mut Window, cx: &mut App) -> Option<AnyElement> {
    remember(ctx, cx);
    let (conversation, view) = open_view(cx)?;
    if conversation != ctx.conversation {
        return None;
    }
    let theme = theme::get(cx).clone();
    Some(
        div()
            .flex()
            .justify_end()
            .pb_1()
            .child(
                div()
                    .w(px(expressions::WIDTH))
                    .max_w_full()
                    .h(px(expressions::HEIGHT))
                    .p_2()
                    .rounded_lg()
                    .bg(theme.sidebar)
                    .border_1()
                    .border_color(theme.hover)
                    .shadow_md()
                    .child(view),
            )
            .into_any_element(),
    )
}

fn text_button(
    id: &'static str,
    glyph: &'static str,
    label: SharedString,
    expanded: bool,
    ctx: &ComposerCtx,
    tab: Tab,
    cx: &App,
) -> AnyElement {
    use gpui_kit::TestSupportExt;
    let theme = theme::get(cx).clone();
    let ctx = ctx.clone();
    let hover = theme.hover;
    div()
        .id(id)
        .test_support()
        .role(Role::Button)
        .aria_label(label.clone())
        .aria_expanded(expanded)
        .tooltip(ui::tooltip::text(label))
        .px_1()
        .h(px(24.))
        .flex()
        .items_center()
        .rounded_md()
        .cursor_pointer()
        .text_sm()
        .font_weight(FontWeight::BOLD)
        .text_color(if expanded { theme.text } else { theme.muted })
        .when(expanded, |d| d.bg(theme.selected))
        .hover(move |d| d.bg(hover))
        .child(glyph)
        .on_click(move |_, window, cx| toggle(&ctx, tab, window, cx))
        .into_any_element()
}

fn gif_button(ctx: &ComposerCtx, _window: &mut Window, cx: &mut App) -> Option<AnyElement> {
    remember(ctx, cx);
    let expanded = is_open_on(ctx, Tab::Gifs, cx);
    Some(text_button(
        "composer-gif",
        "GIF",
        t!("emoji-open-gifs").into(),
        expanded,
        ctx,
        Tab::Gifs,
        cx,
    ))
}

fn emoji_button(ctx: &ComposerCtx, _window: &mut Window, cx: &mut App) -> Option<AnyElement> {
    let expanded = is_open_on(ctx, Tab::Emoji, cx);
    Some(text_button(
        "composer-emoji",
        "☺",
        t!("emoji-open-emoji").into(),
        expanded,
        ctx,
        Tab::Emoji,
        cx,
    ))
}

fn is_open_on(ctx: &ComposerCtx, tab: Tab, cx: &App) -> bool {
    open_view(cx)
        .is_some_and(|(conversation, v)| conversation == ctx.conversation && v.read(cx).tab == tab)
}

// ---- Reacting with any emoji ----

/// The full emoji picker in a dialog, reacting to the message with the pick.
pub fn open_react(ctx: MessageCtx, window: &mut Window, cx: &mut App) {
    let session = ctx.session.clone();
    let view = cx.new(|cx| PickerView::new(session, Purpose::React(ctx), Tab::Emoji, window, cx));
    let input = view.read(cx).search.clone();
    let focus = std::cell::Cell::new(true);
    ui::dialog::open(
        window,
        cx,
        t!("emoji-react-title"),
        move |dialog, window, cx| {
            if focus.replace(false) {
                input.update(cx, |s, cx| s.focus(window, cx));
            }
            dialog
                .w(px(expressions::WIDTH + 32.))
                .child(div().h(px(expressions::HEIGHT + 40.)).child(view.clone()))
        },
    );
}

// ---- Autocomplete ----

/// `:gri` → 😀 and friends; custom emoji complete to `:name: `.
fn suggest(ctx: &ComposerCtx, text: &str, cx: &App) -> Vec<Suggestion> {
    let (_, caret) = composer::typed(&ctx.composer, cx);
    let Some((range, query)) = emoji::typed_shortcode(text, caret) else {
        return Vec::new();
    };
    let session = ctx.session.read(cx);
    let Some(client) = session.client() else {
        return Vec::new();
    };
    let w = client.read();
    let library = w.documents.library.value();
    let tone = emoji::skin_tone(library);
    let usage = emoji::usage_rank(library);
    emoji::rank(emoji::search(w.emoji.all(), query, None), &usage, query)
        .into_iter()
        .take(SUGGESTIONS)
        .map(|e| {
            let shown = e.text(tone);
            let (label, insert) = if e.is_custom() {
                (shown.clone(), format!("{shown} "))
            } else {
                (format!("{shown}  :{}:", e.shortcode), shown)
            };
            Suggestion {
                id: format!("emoji-{}", e.id).into(),
                label: label.into(),
                detail: Some(e.label.clone().into()),
                replace: range.clone(),
                insert,
            }
        })
        .collect()
}

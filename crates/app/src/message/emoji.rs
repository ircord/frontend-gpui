//! Inline emoji pictures in message text (package E1).
//!
//! A `:name:` that names a custom emoji (the server's emotes, its bundled
//! workspace emoji, 7TV globals) draws as the picture; any other `:name:`
//! stays text, as in the web client's `MessageRuns.vue`. Standard emoji are
//! characters and need nothing here. The pictures to draw are kept in a
//! global, refreshed by `features/klipy.rs` whenever the catalog loads.

use std::collections::HashMap;
use std::rc::Rc;

use gpui_kit::prelude::*;
use gpui_kit::*;
use ircord_client::store::emoji::{self, EmojiItem};

use crate::registry::{ComposerToken, Inline, InlineRenderer, InlineSpan, TokenProvider};

/// `name` → the picture `:name:` draws, its URL resolved against the server.
#[derive(Default)]
pub struct Pictures(pub Rc<HashMap<String, SharedString>>);

impl Global for Pictures {}

/// Replace the pictures messages and the message box draw, and redraw.
pub fn set(pictures: HashMap<String, SharedString>, cx: &mut App) {
    cx.set_global(Pictures(Rc::new(pictures)));
    crate::composer::retokenize(cx);
    cx.refresh_windows();
}

/// The URL an emoji's picture is fetched from: as given, or a path on the
/// server.
pub fn resolve(url: &str, origin: Option<&ircord_client::ServerAddress>) -> Option<SharedString> {
    if url.is_empty() {
        return None;
    }
    if url.starts_with("https://") || url.starts_with("http://") {
        return Some(url.to_string().into());
    }
    let joined = origin?.join(url);
    Some(joined.to_string().into())
}

/// The picture for `item`, or `None` for a standard emoji.
pub fn picture_url(
    item: &EmojiItem,
    animate: bool,
    origin: Option<&ircord_client::ServerAddress>,
) -> Option<SharedString> {
    resolve(item.picture(animate)?, origin)
}

/// A picture that fills its box. One that does not load is a small outlined
/// square in `faint`: the box is too small for the `:name:` text, which its
/// owner keeps as the label and tooltip.
pub fn picture(url: SharedString, faint: Hsla) -> AnyElement {
    img(SharedUri::from(url.to_string()))
        .size_full()
        .object_fit(ObjectFit::Contain)
        .with_fallback(move || {
            div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .size(rems(0.8))
                        .rounded_sm()
                        .border_1()
                        .border_color(faint),
                )
                .into_any_element()
        })
        .into_any_element()
}

/// The registered renderer: `:name:` spans of message text become pictures.
pub fn renderer() -> InlineRenderer {
    InlineRenderer {
        id: "emoji".into(),
        order: 0,
        spans: Rc::new(|m, theme, cx| {
            let Some(pictures) = cx.try_global::<Pictures>().map(|p| p.0.clone()) else {
                return Vec::new();
            };
            if pictures.is_empty() || !m.body.text.contains(':') {
                return Vec::new();
            }
            let faint = theme.faint;
            emoji::shortcodes_in(&m.body.text)
                .into_iter()
                .filter_map(|(range, name)| {
                    let url = pictures.get(name)?.clone();
                    Some(InlineSpan {
                        range,
                        inline: Inline::Element(Rc::new(move |_, _| picture(url.clone(), faint))),
                    })
                })
                .collect()
        }),
    }
}

/// The registered composer tokens: a `:name:` typed, pasted or picked in the
/// message box draws as its picture, one line high, and edits as one unit.
/// What is sent is still the name.
pub fn token_provider() -> TokenProvider {
    TokenProvider {
        id: "emoji".into(),
        order: 0,
        find: Rc::new(|text, cx| {
            let Some(pictures) = cx.try_global::<Pictures>().map(|p| p.0.clone()) else {
                return Vec::new();
            };
            if pictures.is_empty() || !text.contains(':') {
                return Vec::new();
            }
            emoji::shortcodes_in(text)
                .into_iter()
                .filter(|(_, name)| pictures.contains_key(*name))
                .map(|(range, name)| ComposerToken {
                    range,
                    key: name.to_string().into(),
                })
                .collect()
        }),
        render: Rc::new(|view, _, cx| {
            let faint = crate::theme::get(cx).faint;
            let url = cx
                .try_global::<Pictures>()
                .and_then(|p| p.0.get(view.key.as_ref()).cloned());
            div()
                .size(view.line_height)
                .flex_none()
                .children(url.map(|url| picture(url, faint)))
                .into_any_element()
        }),
    }
}

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

use crate::registry::{Inline, InlineRenderer, InlineSpan};

/// `name` → the picture `:name:` draws, its URL resolved against the server.
#[derive(Default)]
pub struct Pictures(pub Rc<HashMap<String, SharedString>>);

impl Global for Pictures {}

/// Replace the pictures messages draw, and redraw.
pub fn set(pictures: HashMap<String, SharedString>, cx: &mut App) {
    cx.set_global(Pictures(Rc::new(pictures)));
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

/// A picture that fills its box, with `label` (`:name:`) as its fallback.
pub fn picture(url: SharedString, label: SharedString) -> AnyElement {
    img(SharedUri::from(url.to_string()))
        .size_full()
        .object_fit(ObjectFit::Contain)
        .with_fallback(move || div().text_xs().child(label.clone()).into_any_element())
        .into_any_element()
}

/// The registered renderer: `:name:` spans of message text become pictures.
pub fn renderer() -> InlineRenderer {
    InlineRenderer {
        id: "emoji".into(),
        order: 0,
        spans: Rc::new(|m, _theme, cx| {
            let Some(pictures) = cx.try_global::<Pictures>().map(|p| p.0.clone()) else {
                return Vec::new();
            };
            if pictures.is_empty() || !m.body.text.contains(':') {
                return Vec::new();
            }
            emoji::shortcodes_in(&m.body.text)
                .into_iter()
                .filter_map(|(range, name)| {
                    let url = pictures.get(name)?.clone();
                    let label: SharedString = format!(":{name}:").into();
                    Some(InlineSpan {
                        range,
                        inline: Inline::Element(Rc::new(move |_, _| {
                            picture(url.clone(), label.clone())
                        })),
                    })
                })
                .collect()
        }),
    }
}

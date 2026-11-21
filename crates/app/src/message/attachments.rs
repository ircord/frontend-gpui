//! Pictures in a message: uploaded images, GIFs and image links, drawn
//! inline and opened in the browser on click.

use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::registry::MessageCtx;
use crate::theme::Theme;
use crate::timeline::rows::{Image, MessageRow};

pub fn render(
    m: &MessageRow,
    _ctx: &MessageCtx,
    theme: &Theme,
    _window: &mut Window,
    _cx: &mut App,
) -> Vec<AnyElement> {
    m.images
        .iter()
        .enumerate()
        .map(|(i, image)| image_element(&m.id, i, image, theme).into_any_element())
        .collect()
}

fn image_element(message: &str, i: usize, image: &Image, theme: &Theme) -> impl IntoElement {
    let (max_w, max_h) = (400.0_f32, 300.0_f32);
    let (w, h) = if image.width > 0 && image.height > 0 {
        let scale = (max_w / image.width as f32)
            .min(max_h / image.height as f32)
            .min(1.0);
        (image.width as f32 * scale, image.height as f32 * scale)
    } else {
        (max_w, max_h * 0.66)
    };
    let url = image.url.clone();
    div()
        .id(ElementId::Name(format!("image-{message}-{i}").into()))
        .role(Role::Image)
        .aria_label(SharedString::from(if image.alt.is_empty() {
            image.url.clone()
        } else {
            image.alt.clone()
        }))
        .mt_1()
        .w(px(w))
        .h(px(h))
        .rounded_md()
        .overflow_hidden()
        .bg(theme.sidebar)
        .cursor_pointer()
        .child(
            img(SharedUri::from(image.url.clone()))
                .size_full()
                .object_fit(ObjectFit::Contain),
        )
        .on_click(move |_, _, cx| cx.open_url(&url))
}

//! A network's icon on the rail: its own picture from the preferences, or
//! the pattern and hue the web client draws from its name.

use std::sync::Arc;

use gpui_kit::TestSupportExt;
use gpui_kit::prelude::*;
use gpui_kit::*;
use ircord_client::contract::Network;
use ircord_client::store::Workspace;
use ircord_client::store::sidebar_order::{self as order, network_glyph};

/// The tile for `network`, `size` square.
pub fn network_icon(w: &Workspace, network: &Network, size: Pixels) -> AnyElement {
    if let Some((mime, bytes)) = order::network_icon(w, &network.id) {
        let format = match mime.as_str() {
            "image/png" => Some(ImageFormat::Png),
            "image/jpeg" | "image/jpg" => Some(ImageFormat::Jpeg),
            "image/webp" => Some(ImageFormat::Webp),
            _ => None,
        };
        if let Some(format) = format {
            let image = Arc::new(Image::from_bytes(format, bytes));
            return img(image)
                .id(ElementId::Name(
                    format!("network-{}-icon", network.id).into(),
                ))
                .size(size)
                .rounded(px(8.))
                .into_any_element();
        }
    }
    pattern(&network.id, &network.name, size)
}

/// A mirrored 5×5 pattern on a light tile of the name's hue; colours are
/// data here (they identify the network), not theme tokens.
fn pattern(id: &str, name: &str, size: Pixels) -> AnyElement {
    let glyph = network_glyph(name);
    let hue = glyph.hue as f32 / 360.;
    // One cell of padding on each side: a 7×7 grid, as the web client's viewBox.
    let cell = size / 7.;
    let mut tile = div()
        .id(ElementId::Name(format!("network-{id}-glyph").into()))
        .test_support()
        .size(size)
        .flex_none()
        .rounded(px(8.))
        .relative()
        .bg(hsla(hue, 0.42, 0.72, 1.));
    for (column, row) in glyph.cells {
        tile = tile.child(
            div()
                .absolute()
                .left(cell * (f32::from(column) + 1.))
                .top(cell * (f32::from(row) + 1.))
                .size(cell)
                .bg(hsla(hue, 0.38, 0.30, 1.)),
        );
    }
    tile.into_any_element()
}

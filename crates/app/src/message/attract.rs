//! `/attract` on a message row, as the web's `presence.css` draws it: a tint
//! with a bar on the left and a shine sweeping across the row, and a ring
//! with a pulse and a few twinkles around the avatar. The numbers come from
//! `timeline::attract`; the elements carry IDs so the tests can see them.

use gpui_kit::TestSupportExt;
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::theme::Theme;
use crate::timeline::attract::{self, Fx};

/// The tint, bar and shine over a row. It is drawn over the row's text, but
/// the tint is faint and the layer takes no clicks.
pub fn row(fx: Fx, id: &str, theme: &Theme) -> AnyElement {
    let accent = theme.accent;
    let mut layer = div()
        .id(ElementId::Name(format!("attract-row-{id}").into()))
        .test_support()
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .overflow_hidden()
        .bg(accent.opacity(0.12))
        .child(
            div()
                .absolute()
                .top_0()
                .bottom_0()
                .left_0()
                .w(px(2.))
                .bg(accent),
        );
    if let Some(pulse) = fx.pulse {
        let (centre, opacity) = attract::shine(pulse);
        let width = 0.62;
        let clear = accent.opacity(0.);
        let glow = accent.opacity(0.3);
        let half = |from: Hsla, to: Hsla| {
            div().h_full().w(relative(0.5)).bg(linear_gradient(
                90.,
                linear_color_stop(from, 0.),
                linear_color_stop(to, 1.),
            ))
        };
        layer = layer.child(
            div()
                .id(ElementId::Name(format!("attract-shine-{id}").into()))
                .test_support()
                .absolute()
                .top_0()
                .bottom_0()
                .left(relative(centre - width / 2.))
                .w(relative(width))
                .opacity(opacity)
                .flex()
                .child(half(clear, glow))
                .child(half(glow, clear)),
        );
    }
    layer.into_any_element()
}

/// The five twinkles' places in the box around the avatar (a share of its
/// width and height) and their colours.
const TWINKLES: [(f32, f32, u32); 5] = [
    (0.12, 0.18, 0xffffff),
    (0.88, 0.12, 0xfff6c8),
    (0.92, 0.82, 0xffffff),
    (0.08, 0.78, 0xfff6c8),
    (0.5, 0.0, 0xffffff),
];

/// `face` (the avatar, `size` across) with the ring, pulse and twinkles
/// around it. Nothing moves: they are drawn outside its box.
pub fn avatar(face: AnyElement, fx: Fx, size: f32, id: &str, theme: &Theme) -> AnyElement {
    let accent = theme.accent;
    let ring = |spread: f32, color: Hsla| {
        let edge = 4. + spread;
        div()
            .absolute()
            .top(px(-edge))
            .left(px(-edge))
            .size(px(size + 2. * edge))
            .rounded_full()
            .border_2()
            .border_color(color)
    };
    let mut wrap = div()
        .id(ElementId::Name(format!("attract-avatar-{id}").into()))
        .test_support()
        .relative()
        .flex_none()
        .child(face)
        .child(ring(0., accent));
    if let Some(pulse) = fx.pulse {
        let (spread, opacity) = attract::ring(pulse);
        wrap = wrap.child(ring(spread, accent.opacity(opacity)));
        let (opacity, scale) = attract::twinkle(pulse);
        let area = size + 20.;
        let dot = 3. * scale;
        for (x, y, color) in TWINKLES {
            wrap = wrap.child(
                div()
                    .absolute()
                    .left(px(-10. + x * area - dot / 2.))
                    .top(px(-10. + y * area - dot / 2.))
                    .size(px(dot))
                    .rounded_full()
                    .bg(rgb(color))
                    .opacity(opacity),
            );
        }
    }
    wrap.into_any_element()
}

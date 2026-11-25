//! `/attract` on a message row and its avatar. While the comet moves, the
//! row and the avatar only report where they were painted, and the layer
//! over the timeline (`timeline::comet`) draws the comet from there. With
//! reduced motion they show the web's static marks instead: a tint with a
//! bar on the left of the row and a ring around the avatar. The elements
//! carry IDs so the tests can see them.

use gpui_kit::TestSupportExt;
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::theme::Theme;
use crate::timeline::attract::{Fx, Stage};

/// A layer the size of its parent that draws nothing and tells `stage`
/// where it was painted.
fn probe(stage: Stage, id: String, report: fn(&Stage, &str, Bounds<Pixels>)) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |bounds, _, _, _| report(&stage, &id, bounds),
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

/// The row's part: its place for the comet, or the tint and bar. Neither
/// takes clicks. A replaced attract that is letting its sparks land keeps
/// the place but not the ID.
pub fn row(fx: Fx, id: &str, theme: &Theme) -> AnyElement {
    if let Some(stage) = fx.stage {
        let layer = div().absolute().top_0().left_0().size_full();
        let layer = if fx.settling {
            layer.into_any_element()
        } else {
            layer
                .id(ElementId::Name(format!("attract-row-{id}").into()))
                .test_support()
                .into_any_element()
        };
        return div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .child(layer)
            .child(probe(stage, id.to_owned(), Stage::row))
            .into_any_element();
    }
    let accent = theme.accent;
    div()
        .id(ElementId::Name(format!("attract-row-{id}").into()))
        .test_support()
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .child(
            div()
                .id(ElementId::Name(format!("attract-tint-{id}").into()))
                .test_support()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .bg(accent.opacity(0.12)),
        )
        .child(
            div()
                .absolute()
                .top_0()
                .bottom_0()
                .left_0()
                .w(px(2.))
                .bg(accent),
        )
        .into_any_element()
}

/// `face` (the avatar, `size` across) with its place reported for the
/// comet, or with the ring around it. Nothing around it moves: the ring is
/// drawn outside its box. The wrapper carries no ID, so the picture keeps
/// its element state (a loaded picture stays loaded) while the effect plays.
pub fn avatar(face: AnyElement, fx: Fx, size: f32, id: &str, theme: &Theme) -> AnyElement {
    let marker = div()
        .id(ElementId::Name(format!("attract-avatar-{id}").into()))
        .test_support()
        .absolute()
        .top_0()
        .left_0()
        .size(px(size));
    let wrap = div()
        .relative()
        .flex_none()
        .self_start()
        .size(px(size))
        .child(face)
        .child(marker);
    let wrap = match fx.stage {
        Some(stage) => wrap.child(probe(stage, id.to_owned(), Stage::avatar)),
        None => {
            let edge = 4.;
            wrap.child(
                div()
                    .id(ElementId::Name(format!("attract-ring-{id}").into()))
                    .test_support()
                    .absolute()
                    .top(px(-edge))
                    .left(px(-edge))
                    .size(px(size + 2. * edge))
                    .rounded_full()
                    .border_2()
                    .border_color(theme.accent),
            )
        }
    };
    wrap.into_any_element()
}

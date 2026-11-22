//! Floating expression picker placement and resizing.

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::TestSupportExt;
use gpui_kit::prelude::*;
use gpui_kit::*;
use ircord_client::store::sidebar_order::{PICKER_HEIGHT, PICKER_WIDTH};

use crate::registry::ComposerCtx;
use crate::sidebar::layout;
use crate::theme::{self, Theme};
use crate::ui::focus::Reachable;
use crate::ui::tooltip;

use super::{dismiss, opened};

const MARGIN: f32 = 8.;

pub(super) fn slot(ctx: &ComposerCtx, _: &mut Window, cx: &mut App) -> Option<AnyElement> {
    let open = opened(cx).filter(|open| open.belongs_to(ctx))?;
    let theme = theme::get(cx).clone();
    Some(
        canvas(
            move |anchor, window, cx| {
                let layout = layout::get(cx);
                let viewport = window.viewport_size();
                let placement = place(
                    anchor,
                    viewport,
                    size(px(layout.picker_width), px(layout.picker_height)),
                );
                let id = open.view.entity_id();
                let mut panel = div()
                    .id("expression-popover")
                    .relative()
                    .w(placement.bounds.size.width)
                    .h(placement.bounds.size.height)
                    .p_2()
                    .bg(theme.raised)
                    .rounded(theme.radius_large)
                    .shadow_md()
                    .occlude()
                    .on_mouse_down_out(move |event, _, cx| {
                        if !anchor.contains(&event.position)
                            && opened(cx).is_some_and(|open| open.view.entity_id() == id)
                        {
                            dismiss(cx);
                        }
                    })
                    .child(open.view.clone());
                if !placement.sheet {
                    panel = panel.child(resizer(
                        layout.picker_width,
                        layout.picker_height,
                        placement.below,
                        &theme,
                        window,
                        cx,
                    ));
                }
                let mut panel = panel.into_any_element();
                panel.layout_as_root(placement.bounds.size.into(), window, cx);
                window.defer_draw(panel, placement.bounds.origin, 0, None);
            },
            |_, (), _, _| {},
        )
        .absolute()
        .inset_0()
        .into_any_element(),
    )
}

#[derive(Debug, PartialEq)]
struct Placement {
    bounds: Bounds<Pixels>,
    below: bool,
    sheet: bool,
}

fn place(anchor: Bounds<Pixels>, viewport: Size<Pixels>, preferred: Size<Pixels>) -> Placement {
    if viewport.width <= px(700.) {
        let height = (viewport.height * 0.62).min(px(520.));
        return Placement {
            bounds: Bounds {
                origin: point(px(0.), viewport.height - height),
                size: size(viewport.width, height),
            },
            below: false,
            sheet: true,
        };
    }
    let margin = px(MARGIN);
    let available_height = (viewport.height - margin * 2.).max(px(0.));
    let width = preferred.width.min(viewport.width - margin * 2.);
    let above = anchor.top() - margin * 2.;
    let below = viewport.height - anchor.bottom() - margin * 2.;
    let place_below = below > above && above < px(360.);
    let height = preferred
        .height
        .min(if place_below { below } else { above })
        .max(px(160.))
        .min(available_height);
    let top = if place_below {
        anchor.bottom() + margin
    } else {
        anchor.top() - height - margin
    };
    let x = (anchor.right() - width)
        .min(viewport.width - width - margin)
        .max(margin);
    let y = top.min(viewport.height - height - margin).max(margin);
    Placement {
        bounds: Bounds {
            origin: point(x, y),
            size: size(width, height),
        },
        below: place_below,
        sheet: false,
    }
}

#[derive(Clone)]
struct Resize(Rc<Cell<(Point<Pixels>, f32, f32)>>);

struct DragPreview;

impl Render for DragPreview {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size(px(0.))
    }
}

fn resize(width: f32, height: f32, cx: &mut App) {
    layout::update(cx, |layout| {
        layout.picker_width = width;
        layout.picker_height = height;
    });
}

fn resizer(
    width: f32,
    height: f32,
    below: bool,
    theme: &Theme,
    window: &mut Window,
    cx: &mut App,
) -> impl IntoElement {
    let drag = window
        .use_keyed_state("expression-picker-resize-drag", cx, |_, _| {
            Resize(Rc::new(Cell::new((Point::default(), width, height))))
        })
        .read(cx)
        .clone();
    let start = drag.0.clone();
    let vertical = if below { 1. } else { -1. };
    let color = theme.muted.opacity(0.6);
    div()
        .id("expression-resize")
        .test_support()
        .role(Role::Splitter)
        .aria_label(t!("emoji-resize"))
        .aria_numeric_value(width.into())
        .aria_min_numeric_value(PICKER_WIDTH.0.into())
        .aria_max_numeric_value(PICKER_WIDTH.1.into())
        .reachable(theme)
        .absolute()
        .left_0()
        .when(below, |d| {
            d.bottom_0().cursor(CursorStyle::ResizeUpRightDownLeft)
        })
        .when(!below, |d| {
            d.top_0().cursor(CursorStyle::ResizeUpLeftDownRight)
        })
        .size(px(18.))
        .tooltip(tooltip::text(t!("emoji-resize-help")))
        .child(
            div()
                .absolute()
                .left_1()
                .top_1()
                .size(px(7.))
                .border_l_1()
                .border_t_1()
                .border_color(color),
        )
        .on_mouse_down(MouseButton::Left, move |event, _, _| {
            start.set((event.position, width, height))
        })
        .on_drag(drag, |_, _, _, cx| cx.new(|_| DragPreview))
        .on_drag_move::<Resize>(move |event, _, cx| {
            let (origin, width, height) = event.drag(cx).0.get();
            let delta = event.event.position - origin;
            resize(
                width - f32::from(delta.x),
                height + vertical * f32::from(delta.y),
                cx,
            );
        })
        .on_click(|event, _, cx| {
            if event.click_count() == 2 {
                resize(PICKER_WIDTH.2, PICKER_HEIGHT.2, cx);
            }
        })
        .on_key_down(move |event, _, cx| {
            let (x, y) = match event.keystroke.key.as_str() {
                "left" => (16., 0.),
                "right" => (-16., 0.),
                "up" => (0., -16.),
                "down" => (0., 16.),
                _ => return,
            };
            cx.stop_propagation();
            resize(width + x, height + vertical * y, cx);
        })
}

#[cfg(test)]
mod tests {
    use super::place;
    use gpui_kit::{Bounds, point, px, size};

    #[test]
    fn places_the_picker_over_the_anchor_and_fits_it_on_small_screens() {
        let desired = size(px(480.), px(460.));
        let viewport = size(px(1000.), px(800.));
        let mut anchor = Bounds {
            origin: point(px(200.), px(730.)),
            size: size(px(600.), px(48.)),
        };
        let popup = place(anchor, viewport, desired);
        assert_eq!(popup.bounds.right(), anchor.right());
        assert_eq!(popup.bounds.bottom(), anchor.top() - px(8.));
        assert_eq!(popup.bounds.size, desired);
        anchor.origin.y = px(130.);
        let popup = place(anchor, viewport, desired);
        assert!(popup.below);
        assert_eq!(popup.bounds.top(), anchor.bottom() + px(8.));
        let sheet = place(anchor, size(px(650.), px(600.)), desired);
        assert!(sheet.sheet);
        assert_eq!(sheet.bounds.bottom(), px(600.));
        assert_eq!(sheet.bounds.size.width, px(650.));
    }
}

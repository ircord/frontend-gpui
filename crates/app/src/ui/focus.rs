//! Keyboard reach for controls drawn from plain elements.
//!
//! A `gpui-kit` button or field is a Tab stop already. A control built from
//! a `div` (a sidebar row, a tab, an icon button) is not: `.reachable(theme)`
//! makes it one, with a focus handle kept for as long as the element is
//! drawn, and a ring while the keyboard is on it. GPUI fires the element's
//! `on_click` for Enter and Space while it holds focus, so nothing else is
//! needed for it to work from the keyboard.
//!
//! The wrapper passes styles, children and handlers through, so it goes
//! anywhere in the chain after `.test_support()` (usually beside the role),
//! and headless tests see the focus binding.
//!
//! [`init`] keeps Tab working when nothing holds focus, as after the
//! focused control went away with the page that drew it: `gpui-kit` binds
//! Tab only inside its root, which a key without a focused element never
//! reaches.

use gpui_kit::prelude::*;
use gpui_kit::*;

pub fn init(cx: &mut App) {
    cx.intercept_keystrokes(|event, window, cx| {
        let k = &event.keystroke;
        let m = &k.modifiers;
        if k.key != "tab" || m.control || m.alt || m.platform || m.function {
            return;
        }
        // Inside the root, gpui-kit's own Tab binding moves focus.
        if event.context_stack.iter().any(|c| c.contains("Root")) {
            return;
        }
        if m.shift {
            window.focus_prev(cx);
        } else {
            window.focus_next(cx);
        }
        cx.stop_propagation();
    })
    .detach();
}

/// A control made a Tab stop; see the module docs.
pub struct Reach<E> {
    element: E,
    ring: Hsla,
    focus: Option<FocusHandle>,
}

/// `.reachable(theme)` on any stateful element.
pub trait Reachable: Sized {
    fn reachable(self, theme: &crate::theme::Theme) -> Reach<Self> {
        Reach {
            element: self,
            ring: theme.accent,
            focus: None,
        }
    }
}

impl<E: StatefulInteractiveElement + Styled> Reachable for E {}

impl<E> Reach<E> {
    /// Track a focus target that its owner will also use to restore focus.
    pub fn with_focus(mut self, handle: &FocusHandle) -> Self {
        self.focus = Some(handle.clone());
        self
    }
}

impl<E> IntoElement for Reach<E>
where
    E: Element + StatefulInteractiveElement + Styled + 'static,
{
    type Element = ViewElement<Self>;

    #[track_caller]
    fn into_element(self) -> Self::Element {
        ViewElement::new(self)
    }
}

impl<E> RenderOnce for Reach<E>
where
    E: Element + StatefulInteractiveElement + Styled + 'static,
{
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let Some(id) = Element::id(&self.element) else {
            return self.element;
        };
        let handle = self.focus.unwrap_or_else(|| {
            window
                .use_keyed_state(id, cx, |_, cx| cx.focus_handle().tab_stop(true))
                .read(cx)
                .clone()
        });
        let ring = self.ring;
        self.element.track_focus(&handle).focus_visible(move |s| {
            s.shadow(vec![BoxShadow {
                color: ring,
                offset: point(px(0.), px(0.)),
                blur_radius: px(0.),
                spread_radius: px(2.),
                inset: true,
            }])
        })
    }
}

impl<E: Styled> Styled for Reach<E> {
    fn style(&mut self) -> &mut StyleRefinement {
        self.element.style()
    }
}

impl<E: ParentElement> ParentElement for Reach<E> {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.element.extend(elements)
    }
}

impl<E: InteractiveElement> InteractiveElement for Reach<E> {
    fn interactivity(&mut self) -> &mut Interactivity {
        self.element.interactivity()
    }
}

impl<E: StatefulInteractiveElement> StatefulInteractiveElement for Reach<E> {}

//! Registration points. A feature adds what it shows to these lists from its
//! `init` (see `features/mod.rs`), and the views that own each place draw
//! whatever is registered, so a package never edits another package's view.
//!
//! ```ignore
//! registry::register(cx, |r| {
//!     r.message_menu.add(
//!         MenuItem::new("copy-text", t!("message-actions-copy-text"), |m: &MessageCtx, _, cx| {
//!             // …
//!         })
//!         .icon(IconName::Copy)
//!         .toolbar(),
//!     );
//! });
//! ```
//!
//! Every list keeps its entries sorted by `order` (lower first; equal orders
//! keep registration order). Registering an ID that is already present
//! replaces the earlier entry.

use std::ops::Range;
use std::rc::Rc;

use gpui_kit::component::IconName;
use gpui_kit::*;
use ircord_client::store::Workspace;

use crate::composer::ComposerView;
use crate::route::{PaneKind, Route};
use crate::session::Session;
use crate::theme::Theme;
use crate::timeline::rows::{MessageRow, Row};

pub type Handler<C> = Rc<dyn Fn(&C, &mut Window, &mut App)>;
pub type Predicate<C> = Rc<dyn Fn(&C, &App) -> bool>;
/// Why an entry cannot be used right now, or `None` when it can.
pub type Unavailable<C> = Rc<dyn Fn(&C, &App) -> Option<SharedString>>;
pub type ElementFn = Rc<dyn Fn(&mut Window, &mut App) -> AnyElement>;
pub type Render<C> = Rc<dyn Fn(&C, &mut Window, &mut App) -> Option<AnyElement>>;

/// Something with an ID and a place in a sorted list.
pub trait Entry {
    fn id(&self) -> &str;
    fn order(&self) -> i32;
}

/// A sorted list of registered entries.
pub struct List<T>(Vec<T>);

impl<T> Default for List<T> {
    fn default() -> Self {
        Self(Vec::new())
    }
}

impl<T: Entry> List<T> {
    pub fn add(&mut self, entry: T) {
        self.0.retain(|e| e.id() != entry.id());
        let at = self.0.partition_point(|e| e.order() <= entry.order());
        self.0.insert(at, entry);
    }

    pub fn remove(&mut self, id: &str) {
        self.0.retain(|e| e.id() != id);
    }

    pub fn get(&self, id: &str) -> Option<&T> {
        self.0.iter().find(|e| e.id() == id)
    }

    pub fn iter(&self) -> std::slice::Iter<'_, T> {
        self.0.iter()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl<T: Clone> List<T> {
    /// A copy to draw from, so handlers can take `cx` mutably.
    pub fn to_vec(&self) -> Vec<T> {
        self.0.clone()
    }
}

macro_rules! entry {
    ($($ty:ident $(<$g:ident>)?),* $(,)?) => {$(
        impl$(<$g>)? Entry for $ty$(<$g>)? {
            fn id(&self) -> &str {
                &self.id
            }
            fn order(&self) -> i32 {
                self.order
            }
        }
    )*};
}

// ---- Contexts handed to registered entries ----

/// A message in the timeline.
#[derive(Clone)]
pub struct MessageCtx {
    pub session: Entity<Session>,
    pub conversation: String,
    pub message: String,
    pub own: bool,
}

#[derive(Clone)]
pub struct ConversationCtx {
    pub session: Entity<Session>,
    pub conversation: String,
}

#[derive(Clone)]
pub struct MemberCtx {
    pub session: Entity<Session>,
    pub member: String,
    /// The conversation the member was picked from, if any.
    pub conversation: Option<String>,
}

#[derive(Clone)]
pub struct NetworkCtx {
    pub session: Entity<Session>,
    pub network: String,
}

#[derive(Clone)]
pub struct ComposerCtx {
    pub session: Entity<Session>,
    pub conversation: String,
    pub composer: Entity<ComposerView>,
}

#[derive(Clone)]
pub struct HeaderCtx {
    pub session: Entity<Session>,
    pub route: Route,
}

#[derive(Clone)]
pub struct PaneCtx {
    pub session: Entity<Session>,
    pub route: Route,
}

#[derive(Clone)]
pub struct PanelCtx {
    pub session: Entity<Session>,
    /// The conversation showing beside the panel, if any.
    pub conversation: Option<String>,
}

#[derive(Clone)]
pub struct SettingsCtx {
    pub session: Entity<Session>,
}

// ---- Entries ----

/// An entry in a menu. Message menu entries marked [`MenuItem::toolbar`] also
/// show as an icon in the toolbar over the message under the pointer.
pub struct MenuItem<C> {
    pub id: SharedString,
    pub label: SharedString,
    pub icon: Option<IconName>,
    pub order: i32,
    /// Drawn in the danger colour (delete, kick, ban).
    pub danger: bool,
    pub toolbar: bool,
    pub visible: Predicate<C>,
    /// Shown but disabled, with the reason under the label, when this says why.
    pub unavailable: Option<Unavailable<C>>,
    /// The reason for one context, filled in by `ui::context_menu::visible`.
    pub reason: Option<SharedString>,
    pub run: Handler<C>,
}

impl<C> Clone for MenuItem<C> {
    fn clone(&self) -> Self {
        Self {
            id: self.id.clone(),
            label: self.label.clone(),
            icon: self.icon.clone(),
            order: self.order,
            danger: self.danger,
            toolbar: self.toolbar,
            visible: self.visible.clone(),
            unavailable: self.unavailable.clone(),
            reason: self.reason.clone(),
            run: self.run.clone(),
        }
    }
}

impl<C: 'static> MenuItem<C> {
    pub fn new(
        id: impl Into<SharedString>,
        label: impl Into<SharedString>,
        run: impl Fn(&C, &mut Window, &mut App) + 'static,
    ) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            icon: None,
            order: 0,
            danger: false,
            toolbar: false,
            visible: Rc::new(|_, _| true),
            unavailable: None,
            reason: None,
            run: Rc::new(run),
        }
    }

    pub fn order(mut self, order: i32) -> Self {
        self.order = order;
        self
    }

    pub fn icon(mut self, icon: IconName) -> Self {
        self.icon = Some(icon);
        self
    }

    pub fn danger(mut self) -> Self {
        self.danger = true;
        self
    }

    pub fn toolbar(mut self) -> Self {
        self.toolbar = true;
        self
    }

    /// Show the entry only when `visible` says so.
    pub fn when(mut self, visible: impl Fn(&C, &App) -> bool + 'static) -> Self {
        self.visible = Rc::new(visible);
        self
    }

    /// Show the entry disabled, with the reason, whenever `why` gives one
    /// (an action the network does not allow here). The toolbar leaves such
    /// entries out.
    pub fn unavailable_when(
        mut self,
        why: impl Fn(&C, &App) -> Option<SharedString> + 'static,
    ) -> Self {
        self.unavailable = Some(Rc::new(why));
        self
    }
}

/// A piece of view placed in a slot (header buttons, composer slots).
pub struct Slot<C> {
    pub id: SharedString,
    pub order: i32,
    pub render: Render<C>,
}

impl<C> Clone for Slot<C> {
    fn clone(&self) -> Self {
        Self {
            id: self.id.clone(),
            order: self.order,
            render: self.render.clone(),
        }
    }
}

impl<C: 'static> Slot<C> {
    /// `render` returns `None` to show nothing this time.
    pub fn new(
        id: impl Into<SharedString>,
        render: impl Fn(&C, &mut Window, &mut App) -> Option<AnyElement> + 'static,
    ) -> Self {
        Self {
            id: id.into(),
            order: 0,
            render: Rc::new(render),
        }
    }

    pub fn order(mut self, order: i32) -> Self {
        self.order = order;
        self
    }
}

/// One suggestion offered while typing.
#[derive(Clone, Debug, PartialEq)]
pub struct Suggestion {
    pub id: SharedString,
    pub label: SharedString,
    pub detail: Option<SharedString>,
    /// The byte range of the composer text to replace…
    pub replace: Range<usize>,
    /// …and what to put there.
    pub insert: String,
}

/// Offers suggestions for the text being typed (commands, nicks, emoji).
/// The first provider with suggestions wins.
#[derive(Clone)]
pub struct SuggestionProvider {
    pub id: SharedString,
    pub order: i32,
    #[allow(clippy::type_complexity)]
    pub suggest: Rc<dyn Fn(&ComposerCtx, &str, &App) -> Vec<Suggestion>>,
}

/// What a send filter decided.
#[derive(Clone, Debug, PartialEq)]
pub enum SendOutcome {
    /// Pass the (possibly changed) text on to the next filter, then send.
    Continue(String),
    /// The filter took care of it (a command ran): clear the box.
    Handled,
    /// Do not send and keep the text, for the person to fix.
    Keep,
}

/// Runs on the text before it is sent, in order: commands, Markdown to IRC,
/// length limits.
#[derive(Clone)]
pub struct SendFilter {
    pub id: SharedString,
    pub order: i32,
    #[allow(clippy::type_complexity)]
    pub filter: Rc<dyn Fn(&ComposerCtx, String, &mut Window, &mut App) -> SendOutcome>,
}

/// Runs after every submit (Enter, or a send button), once the text went
/// through the send filters. `sent` is the text that was sent, empty when
/// there was none; the upload tray sends its files here. Not run when a
/// filter handled the text (a command) or kept it.
#[derive(Clone)]
pub struct SubmitHook {
    pub id: SharedString,
    pub order: i32,
    #[allow(clippy::type_complexity)]
    pub run: Rc<dyn Fn(&ComposerCtx, &str, &mut Window, &mut App)>,
}

impl SubmitHook {
    pub fn new(
        id: impl Into<SharedString>,
        run: impl Fn(&ComposerCtx, &str, &mut Window, &mut App) + 'static,
    ) -> Self {
        Self {
            id: id.into(),
            order: 0,
            run: Rc::new(run),
        }
    }
}

/// The composer's slots.
#[derive(Default)]
pub struct ComposerSlots {
    /// Above the box: the reply bar, the upload tray.
    pub above: List<Slot<ComposerCtx>>,
    /// Buttons beside the box: upload, emoji, GIF.
    pub buttons: List<Slot<ComposerCtx>>,
    pub suggestions: List<SuggestionProvider>,
    pub send_filters: List<SendFilter>,
    pub on_submit: List<SubmitHook>,
}

/// A section of the settings page.
#[derive(Clone)]
pub struct SettingsSection {
    pub id: SharedString,
    pub title: SharedString,
    pub order: i32,
    /// Extra words settings search matches, beyond the title.
    pub keywords: Vec<SharedString>,
    #[allow(clippy::type_complexity)]
    pub render: Rc<dyn Fn(&SettingsCtx, &mut Window, &mut App) -> AnyElement>,
}

impl SettingsSection {
    pub fn new(
        id: impl Into<SharedString>,
        title: impl Into<SharedString>,
        render: impl Fn(&SettingsCtx, &mut Window, &mut App) -> AnyElement + 'static,
    ) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            order: 0,
            keywords: Vec::new(),
            render: Rc::new(render),
        }
    }

    pub fn order(mut self, order: i32) -> Self {
        self.order = order;
        self
    }

    pub fn keywords(mut self, words: impl IntoIterator<Item = impl Into<SharedString>>) -> Self {
        self.keywords = words.into_iter().map(Into::into).collect();
        self
    }

    /// Whether settings search for `query` finds this section.
    pub fn matches(&self, query: &str) -> bool {
        let query = query.trim().to_lowercase();
        query.is_empty()
            || self.title.to_lowercase().contains(&query)
            || self
                .keywords
                .iter()
                .any(|k| k.to_lowercase().contains(&query))
    }
}

/// How an inline renderer changes part of a message's text.
#[derive(Clone)]
pub enum Inline {
    Style(HighlightStyle),
    /// Draw an element in place of the text (an emoji picture, a pill). The
    /// text stays as the fallback and for copying.
    Element(ElementFn),
}

#[derive(Clone)]
pub struct InlineSpan {
    /// A byte range of the drawn message text (`MessageRow::body.text`).
    pub range: Range<usize>,
    pub inline: Inline,
}

/// Finds spans of message text to draw differently.
#[derive(Clone)]
pub struct InlineRenderer {
    pub id: SharedString,
    pub order: i32,
    #[allow(clippy::type_complexity)]
    pub spans: Rc<dyn Fn(&MessageRow, &Theme) -> Vec<InlineSpan>>,
}

/// Hides timeline rows (ignored people, folded events). A row shows when
/// every filter keeps it.
#[derive(Clone)]
pub struct RowFilter {
    pub id: SharedString,
    pub order: i32,
    #[allow(clippy::type_complexity)]
    pub keep: Rc<dyn Fn(&Row, &Workspace, &App) -> bool>,
}

/// A default key binding, listed so the shortcuts settings can show and
/// change it.
#[derive(Clone)]
pub struct KeyDefault {
    pub id: SharedString,
    pub label: SharedString,
    pub order: i32,
    /// GPUI keystrokes, such as `ctrl-k` or `alt-up`.
    pub keys: &'static str,
    /// The key context it applies in, such as `Workspace`.
    pub context: Option<SharedString>,
    #[allow(clippy::type_complexity)]
    bind: Rc<dyn Fn(&'static str, Option<&str>) -> KeyBinding>,
}

impl KeyDefault {
    pub fn new<A: Action + Clone>(
        id: impl Into<SharedString>,
        label: impl Into<SharedString>,
        keys: &'static str,
        context: Option<&str>,
        action: A,
    ) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            order: 0,
            keys,
            context: context.map(|c| SharedString::from(c.to_string())),
            bind: Rc::new(move |keys, context| KeyBinding::new(keys, action.clone(), context)),
        }
    }

    pub fn order(mut self, order: i32) -> Self {
        self.order = order;
        self
    }

    /// The binding with `keys` in place of the default. GPUI keeps
    /// keystrokes as `&'static str`, so a changed key leaks its few bytes.
    pub fn binding(&self, keys: &str) -> KeyBinding {
        let keys: &'static str = if keys == self.keys {
            self.keys
        } else {
            Box::leak(keys.to_string().into_boxed_str())
        };
        (self.bind)(keys, self.context.as_deref())
    }
}

/// Draws the main pane for a route other than a conversation.
#[derive(Clone)]
pub struct Pane {
    pub id: SharedString,
    pub order: i32,
    pub kind: PaneKind,
    #[allow(clippy::type_complexity)]
    pub render: Rc<dyn Fn(&PaneCtx, &mut Window, &mut App) -> AnyElement>,
}

impl Pane {
    pub fn new(
        kind: PaneKind,
        render: impl Fn(&PaneCtx, &mut Window, &mut App) -> AnyElement + 'static,
    ) -> Self {
        Self {
            id: format!("{kind:?}").into(),
            order: 0,
            kind,
            render: Rc::new(render),
        }
    }
}

/// Draws the right-hand panel when the session's panel is this ID.
#[derive(Clone)]
pub struct Panel {
    pub id: SharedString,
    pub order: i32,
    #[allow(clippy::type_complexity)]
    pub render: Rc<dyn Fn(&PanelCtx, &mut Window, &mut App) -> AnyElement>,
}

impl Panel {
    pub fn new(
        id: impl Into<SharedString>,
        render: impl Fn(&PanelCtx, &mut Window, &mut App) -> AnyElement + 'static,
    ) -> Self {
        Self {
            id: id.into(),
            order: 0,
            render: Rc::new(render),
        }
    }
}

entry!(
    MenuItem<C>,
    Slot<C>,
    SuggestionProvider,
    SendFilter,
    SubmitHook,
    SettingsSection,
    InlineRenderer,
    RowFilter,
    KeyDefault,
    Pane,
    Panel,
);

/// Everything registered.
#[derive(Default)]
pub struct Registry {
    /// The message menu and the hover toolbar.
    pub message_menu: List<MenuItem<MessageCtx>>,
    pub conversation_menu: List<MenuItem<ConversationCtx>>,
    pub member_menu: List<MenuItem<MemberCtx>>,
    pub network_menu: List<MenuItem<NetworkCtx>>,
    pub composer: ComposerSlots,
    /// Buttons at the right of the conversation header.
    pub header_buttons: List<Slot<HeaderCtx>>,
    pub settings_sections: List<SettingsSection>,
    pub inline_renderers: List<InlineRenderer>,
    pub row_filters: List<RowFilter>,
    pub keys: List<KeyDefault>,
    pub panes: List<Pane>,
    pub panels: List<Panel>,
}

impl Global for Registry {}

impl Registry {
    pub fn pane(&self, kind: &PaneKind) -> Option<&Pane> {
        self.panes.iter().find(|p| &p.kind == kind)
    }
}

/// Add to the registry.
pub fn register(cx: &mut App, f: impl FnOnce(&mut Registry)) {
    f(cx.default_global::<Registry>());
}

/// What is registered. Empty before [`crate::init`].
pub fn get(cx: &App) -> &Registry {
    cx.try_global::<Registry>().unwrap_or_else(|| empty())
}

fn empty() -> &'static Registry {
    thread_local! {
        static EMPTY: &'static Registry = Box::leak(Box::default());
    }
    EMPTY.with(|e| *e)
}

/// Bind every registered default key. `overrides` maps a key ID to the keys
/// the person chose instead (from the shortcuts settings).
pub fn bind_keys(cx: &mut App, overrides: &dyn Fn(&str) -> Option<String>) {
    let bindings: Vec<KeyBinding> = get(cx)
        .keys
        .iter()
        .map(|k| match overrides(&k.id) {
            Some(keys) => k.binding(&keys),
            None => k.binding(k.keys),
        })
        .collect();
    cx.bind_keys(bindings);
}

#[cfg(test)]
mod tests {
    use super::{Entry, List, SettingsSection};
    use gpui_kit::IntoElement;

    #[derive(Clone)]
    struct E(&'static str, i32);
    impl Entry for E {
        fn id(&self) -> &str {
            self.0
        }
        fn order(&self) -> i32 {
            self.1
        }
    }

    #[test]
    fn lists_sort_by_order_and_replace_by_id() {
        let mut list = List::default();
        list.add(E("b", 10));
        list.add(E("a", 0));
        list.add(E("c", 10));
        list.add(E("d", 5));
        let ids: Vec<_> = list.iter().map(|e| e.0).collect();
        assert_eq!(ids, ["a", "d", "b", "c"]);
        list.add(E("a", 20));
        let ids: Vec<_> = list.iter().map(|e| e.0).collect();
        assert_eq!(ids, ["d", "b", "c", "a"]);
        list.remove("b");
        assert_eq!(list.len(), 3);
    }

    #[test]
    fn settings_search_matches_title_and_keywords() {
        let s = SettingsSection::new("appearance", "Appearance", |_, _, _| {
            gpui_kit::div().into_any_element()
        })
        .keywords(["theme", "colours"]);
        assert!(s.matches(""));
        assert!(s.matches("appear"));
        assert!(s.matches("COLOUR"));
        assert!(!s.matches("password"));
    }
}

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
/// The text of an entry for one context.
pub type Label<C> = Rc<dyn Fn(&C, &App) -> SharedString>;
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

/// What a playable plays.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlayableKind {
    Audio,
    Video,
}

/// A sound or video in a message (an attachment or a link's asset), offered
/// to [`Registry::playables`] before the file card or link card is drawn.
#[derive(Clone)]
pub struct PlayableCtx {
    pub session: Entity<Session>,
    pub conversation: String,
    pub message: String,
    /// Unique within the window: the element ID prefix to use.
    pub key: String,
    pub kind: PlayableKind,
    pub media: crate::features::viewer::Media,
    /// Seconds, when the server said.
    pub duration: Option<f64>,
    /// A picture to show before playing, when there is one.
    pub poster: Option<String>,
    /// What "open in the browser" opens.
    pub page: String,
}

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

/// The sidebar, for slots under the conversation list.
#[derive(Clone)]
pub struct SidebarCtx {
    pub session: Entity<Session>,
    /// The network whose conversations are listed, if any.
    pub network: Option<String>,
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

/// The main pane, for banners across its top.
#[derive(Clone)]
pub struct BannerCtx {
    pub session: Entity<Session>,
    /// What the main pane shows, if anything.
    pub route: Option<Route>,
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
    /// The row a settings search landed on, if it is in this section. Draw
    /// rows with `settings::row`, which highlights this one and scrolls to it.
    pub target: Option<RowTarget>,
}

/// A row to land on: its ID ([`SettingsRow::id`]), and what the row calls
/// with its bounds once it is laid out, so the page can scroll it into view.
#[derive(Clone)]
pub struct RowTarget {
    pub id: SharedString,
    #[allow(clippy::type_complexity)]
    pub reached: Rc<dyn Fn(Bounds<Pixels>, &mut Window, &mut App)>,
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
    /// The ID of the entry whose submenu this entry is in (the notification
    /// levels under "Notifications").
    pub parent: Option<SharedString>,
    /// A label row that is not an action (a submenu's heading).
    pub heading: bool,
    /// The label for one context, when it depends on it.
    pub label_for: Option<Label<C>>,
    /// Drawn with a check mark when this says so (the chosen level).
    pub checked_when: Option<Predicate<C>>,
    /// Shown but disabled, without a reason of its own, when this says so.
    pub disabled_when: Option<Predicate<C>>,
    /// For one context, filled in by `ui::context_menu::visible`.
    pub checked: bool,
    pub disabled: bool,
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
            parent: self.parent.clone(),
            heading: self.heading,
            label_for: self.label_for.clone(),
            checked_when: self.checked_when.clone(),
            disabled_when: self.disabled_when.clone(),
            checked: self.checked,
            disabled: self.disabled,
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
            parent: None,
            heading: false,
            label_for: None,
            checked_when: None,
            disabled_when: None,
            checked: false,
            disabled: false,
            run: Rc::new(run),
        }
    }

    /// A label row that is not an action, such as a submenu's heading.
    pub fn heading(id: impl Into<SharedString>, label: impl Into<SharedString>) -> Self {
        let mut item = Self::new(id, label, |_, _, _| {});
        item.heading = true;
        item
    }

    /// Put the entry in the submenu of the entry `parent`, which opens the
    /// submenu instead of running while anything is under it.
    pub fn under(mut self, parent: impl Into<SharedString>) -> Self {
        self.parent = Some(parent.into());
        self
    }

    /// Label the entry for each context (a level's "Default (mentions)").
    pub fn label_with(mut self, label: impl Fn(&C, &App) -> SharedString + 'static) -> Self {
        self.label_for = Some(Rc::new(label));
        self
    }

    /// Draw a check mark beside the entry when `checked` says so.
    pub fn checked_when(mut self, checked: impl Fn(&C, &App) -> bool + 'static) -> Self {
        self.checked_when = Some(Rc::new(checked));
        self
    }

    /// Show the entry disabled when `disabled` says so; a heading in the
    /// same menu says why.
    pub fn disabled_when(mut self, disabled: impl Fn(&C, &App) -> bool + 'static) -> Self {
        self.disabled_when = Some(Rc::new(disabled));
        self
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
    /// In the sidebar: drawn at its foot rather than in the block of pages
    /// at its top (the connection indicator).
    pub bottom: bool,
    pub render: Render<C>,
}

impl<C> Clone for Slot<C> {
    fn clone(&self) -> Self {
        Self {
            id: self.id.clone(),
            order: self.order,
            bottom: self.bottom,
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
            bottom: false,
            render: Rc::new(render),
        }
    }

    pub fn order(mut self, order: i32) -> Self {
        self.order = order;
        self
    }

    /// Draw a sidebar slot at the sidebar's foot.
    pub fn at_bottom(mut self) -> Self {
        self.bottom = true;
        self
    }
}

/// One suggestion offered while typing.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Suggestion {
    pub id: SharedString,
    pub label: SharedString,
    pub detail: Option<SharedString>,
    /// The byte range of the composer text to replace…
    pub replace: Range<usize>,
    /// …and what to put there.
    pub insert: String,
    /// The list's heading ("Commands"), read from its first suggestion.
    pub heading: Option<SharedString>,
    /// How the row is drawn.
    pub look: SuggestionLook,
}

/// How the composer draws one suggestion.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum SuggestionLook {
    /// The label, then the detail on the same line.
    #[default]
    Inline,
    /// The command and its syntax in monospace, with its description below.
    Command { name: &'static str },
    /// A person: their picture, the label (their name), the detail (their
    /// `@nick`) at the right. `member` is the member's ID.
    Person { member: String },
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

/// A stretch of the composer's text drawn as one picture and edited as one
/// unit (a server emoji's `:name:`). The text stays as it is: sending,
/// drafts, suggestions and send filters all see `text[range]`.
#[derive(Clone, Debug, PartialEq)]
pub struct ComposerToken {
    /// A byte range of the composer text.
    pub range: Range<usize>,
    /// What the provider's `render` is given to draw it (an emoji's name).
    pub key: SharedString,
}

/// What a token provider is given to draw one of its tokens.
#[derive(Clone, Debug)]
pub struct TokenView {
    pub key: SharedString,
    /// The text the token stands for.
    pub text: SharedString,
    /// The height of a line of the box.
    pub line_height: Pixels,
}

/// Finds stretches of the composer's text to draw as tokens, after every
/// change to the text. A stretch the caret is inside stays text until the
/// next change. Call `composer::retokenize` when what it finds changes
/// without the text changing (a catalog loaded).
#[derive(Clone)]
pub struct TokenProvider {
    /// Kept out of `:`, which separates it from a token's key.
    pub id: SharedString,
    pub order: i32,
    #[allow(clippy::type_complexity)]
    pub find: Rc<dyn Fn(&str, &App) -> Vec<ComposerToken>>,
    /// The token's look, filling the box the composer gives it; the
    /// composer draws the selected look around it.
    #[allow(clippy::type_complexity)]
    pub render: Rc<dyn Fn(&TokenView, &mut Window, &mut App) -> AnyElement>,
}

/// What a send filter decided.
#[derive(Clone, Debug, PartialEq)]
pub enum SendOutcome {
    /// Pass the (possibly changed) text on to the next filter, then send.
    Continue(String),
    /// The filter sent a message with its own metadata. Clear the box and run the submit hooks.
    Sent(String),
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
/// there was none; the upload tray sends its files here. Runs after a plain
/// message or a reply, and skips commands, edits, and text kept in the box.
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
    /// The first visible replacement for the normal message field and buttons.
    pub input: List<Slot<ComposerCtx>>,
    /// Floating elements anchored to the message field's rectangle.
    pub overlays: List<Slot<ComposerCtx>>,
    /// Buttons beside the box: upload, emoji, GIF.
    pub buttons: List<Slot<ComposerCtx>>,
    pub suggestions: List<SuggestionProvider>,
    /// Text drawn as pictures in the box (server emoji).
    pub tokens: List<TokenProvider>,
    pub send_filters: List<SendFilter>,
    pub on_submit: List<SubmitHook>,
}

/// The headings the settings sections are listed under.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SettingsGroup {
    /// Profile, account, networks.
    User,
    /// Appearance, messages, notifications, this device, privacy, shortcuts.
    #[default]
    App,
    /// Emotes, about: the IRCord server.
    Server,
}

/// One control in a settings section, as settings search lists it. Its `id`
/// is also the element the section draws for it (`settings::row`), so a
/// search result can open the section and land on the row.
#[derive(Clone, Debug, PartialEq)]
pub struct SettingsRow {
    pub id: SharedString,
    /// Makes sense without the section around it ("Notification sound").
    pub label: SharedString,
    /// A short summary for the search result.
    pub description: Option<SharedString>,
    /// Other words people use for it.
    pub keywords: Vec<SharedString>,
}

impl SettingsRow {
    pub fn new(id: impl Into<SharedString>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            description: None,
            keywords: Vec::new(),
        }
    }

    pub fn description(mut self, description: impl Into<SharedString>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Words separated by commas: `"sign out, logout, session"`, so one
    /// catalog string holds them all.
    pub fn keywords(mut self, words: impl AsRef<str>) -> Self {
        self.keywords = words
            .as_ref()
            .split(',')
            .map(str::trim)
            .filter(|w| !w.is_empty())
            .map(|w| SharedString::from(w.to_string()))
            .collect();
        self
    }

    /// Whether settings search for `query` finds this row: every word of the
    /// query is in its label, description or keywords.
    pub fn matches(&self, query: &str) -> bool {
        let haystack = std::iter::once(&self.label)
            .chain(self.description.as_ref())
            .chain(&self.keywords)
            .map(|t| t.to_lowercase())
            .collect::<Vec<_>>()
            .join(" ");
        let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
        !words.is_empty() && words.iter().all(|w| haystack.contains(w.as_str()))
    }
}

/// A section of the settings page.
#[derive(Clone)]
pub struct SettingsSection {
    pub id: SharedString,
    pub title: SharedString,
    pub group: SettingsGroup,
    pub order: i32,
    /// Extra words settings search matches, beyond the title.
    pub keywords: Vec<SharedString>,
    /// Its controls, for settings search to list and land on.
    pub rows: Vec<SettingsRow>,
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
            group: SettingsGroup::default(),
            order: 0,
            keywords: Vec::new(),
            rows: Vec::new(),
            render: Rc::new(render),
        }
    }

    /// The order within its group.
    pub fn order(mut self, order: i32) -> Self {
        self.order = order;
        self
    }

    pub fn group(mut self, group: SettingsGroup) -> Self {
        self.group = group;
        self
    }

    pub fn keywords(mut self, words: impl IntoIterator<Item = impl Into<SharedString>>) -> Self {
        self.keywords = words.into_iter().map(Into::into).collect();
        self
    }

    pub fn row(mut self, row: SettingsRow) -> Self {
        self.rows.push(row);
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
    /// Draw an element in place of the text (an emoji picture). The body
    /// keeps a small fixed space in the line for it, about a line high and
    /// one and a half wide, and the element fills that space; the text is
    /// its label for screen readers and tests. Spans inside a link, a pill
    /// or a code block stay text.
    Element(ElementFn),
}

#[derive(Clone)]
pub struct InlineSpan {
    /// A byte range of the drawn message text (`MessageRow::body.text`).
    pub range: Range<usize>,
    pub inline: Inline,
}

/// Finds spans of message text to draw differently. It reads what it
/// needs (an emoji catalog) from the app; it runs on every draw of a
/// message, so it should be quick.
#[derive(Clone)]
pub struct InlineRenderer {
    pub id: SharedString,
    pub order: i32,
    #[allow(clippy::type_complexity)]
    pub spans: Rc<dyn Fn(&MessageRow, &Theme, &App) -> Vec<InlineSpan>>,
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
    /// GPUI keystrokes, such as `ctrl-k` or `alt-up`. Empty for an action
    /// that starts without a key: it is listed (and can be given one in the
    /// shortcuts settings) but not bound.
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
    TokenProvider,
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
    /// Your own menu, opened from your button at the right of the top bar.
    pub self_menu: List<MenuItem<SidebarCtx>>,
    /// Under your name in that button (your status): the first slot that
    /// draws replaces the account name.
    pub self_line: List<Slot<SidebarCtx>>,
    /// Extra actions before Settings in the network bar.
    pub topbar_actions: List<Slot<SidebarCtx>>,
    /// Drawn at the start of a conversation's history, under "the start of
    /// the conversation" (an empty direct conversation shows the person).
    pub timeline_start: List<Slot<ConversationCtx>>,
    /// Opens a person when their name or picture in the timeline is clicked.
    pub open_person: Option<Handler<MemberCtx>>,
    pub composer: ComposerSlots,
    /// Buttons at the right of the conversation header, before the member
    /// list toggle and the conversation menu.
    pub header_buttons: List<Slot<HeaderCtx>>,
    /// Across the top of the main pane, under the conversation header and
    /// the connection banner, on every route (invitations).
    pub banners: List<Slot<BannerCtx>>,
    /// The sidebar's pages and notices, in the block above the conversation
    /// list (Inbox, Saved, Drafts, Links); a slot marked
    /// [`Slot::at_bottom`] goes under the list instead (the connection
    /// indicator).
    pub sidebar_footer: List<Slot<SidebarCtx>>,
    pub settings_sections: List<SettingsSection>,
    pub inline_renderers: List<InlineRenderer>,
    pub row_filters: List<RowFilter>,
    pub keys: List<KeyDefault>,
    pub panes: List<Pane>,
    pub panels: List<Panel>,
    /// Draws a sound or video in a message (the audio card, the video
    /// player). The first slot that gives an element replaces the file or
    /// link card; with none, the card draws as before.
    pub playables: List<Slot<PlayableCtx>>,
}

impl Global for Registry {}

impl Registry {
    pub fn pane(&self, kind: &PaneKind) -> Option<&Pane> {
        self.panes.iter().find(|p| &p.kind == kind)
    }
}

/// The first registered playable's element for `ctx`, or `None` when no
/// playable draws it.
pub fn playable(ctx: &PlayableCtx, window: &mut Window, cx: &mut App) -> Option<AnyElement> {
    let slots = get(cx).playables.to_vec();
    slots.iter().find_map(|slot| (slot.render)(ctx, window, cx))
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

/// The keys the person gave registered key IDs in the shortcuts settings
/// (S2), which sets it whenever it binds them. An ID not here has its
/// default; an empty list means no key.
#[derive(Default)]
pub struct BoundKeys(pub std::collections::HashMap<SharedString, Vec<String>>);
impl Global for BoundKeys {}

/// The keys `key` is bound to now, as GPUI keystrokes: the person's own, or
/// its default.
pub fn keys_of(cx: &App, key: &KeyDefault) -> Vec<String> {
    match cx.try_global::<BoundKeys>().and_then(|b| b.0.get(&key.id)) {
        Some(keys) => keys.clone(),
        None if key.keys.trim().is_empty() => Vec::new(),
        None => vec![key.keys.to_string()],
    }
}

/// Bind every registered default key. `overrides` maps a key ID to the keys
/// the person chose instead (from the shortcuts settings).
pub fn bind_keys(cx: &mut App, overrides: &dyn Fn(&str) -> Option<String>) {
    let bindings: Vec<KeyBinding> = get(cx)
        .keys
        .iter()
        .filter_map(|k| {
            let keys = overrides(&k.id).unwrap_or_else(|| k.keys.to_string());
            (!keys.trim().is_empty()).then(|| k.binding(&keys))
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

    #[test]
    fn settings_rows_match_every_word() {
        let row = super::SettingsRow::new("log-out", "Log out")
            .description("On this device")
            .keywords("sign out, logout , session,");
        assert_eq!(row.keywords.len(), 3);
        assert!(row.matches("sign"));
        assert!(row.matches("out device"));
        assert!(row.matches("SESSION"));
        assert!(!row.matches("out password"));
        assert!(!row.matches("  "), "an empty query lists no rows");
    }
}

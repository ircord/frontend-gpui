//! The message text: IRC formatting over selectable text, split into the
//! blocks the web client draws (fenced code with highlighting, `>` quotes,
//! Markdown lists), with mention, channel and message-link pills, and links
//! that open — `irc://` and message links inside the app.
//!
//! Each paragraph is a [`RichText`]: selectable text that also paints the
//! pills' rounded backgrounds and records where each pill and link landed.
//! Over those places the body puts one small element per pill or link, with
//! an ID, a role and a label, so a click, a screen reader and a headless test
//! all find them. They are placed from the previous frame's layout; a frame
//! whose layout moved them draws again.
//!
//! Inline renderers that return `Inline::Element` (an emoji picture) get the
//! same treatment as a pill: the text keeps a fixed space for the element
//! ([`ELEMENT_SPACE`]) and the element is placed over where that space landed.

use crate::ui::focus::Reachable;
use std::cell::RefCell;
use std::ops::Range;
use std::rc::Rc;

use gpui_kit::base::{TextSelectionHandle, TextSelectionRegistration, TextSelectionRun};
use gpui_kit::component::ActiveTheme;
use gpui_kit::prelude::*;
use gpui_kit::*;
use ircord_client::contract::ConversationKind;
use ircord_client::format::blocks::{Block, List, Mapped, message_blocks};
use ircord_client::format::highlight::{Kind, highlight};
use ircord_client::format::inline::{Token, tokens};
use ircord_client::format::links::{MessageLink, parse_message_link};
use ircord_client::format::mentions::{fold, group_mention};
use ircord_client::format::{Color as IrcColor, Formatted};

use crate::registry::{ElementFn, Inline, InlineRenderer, MessageCtx};
use crate::session::Session;
use crate::theme::{self, Theme};
use crate::timeline::rows::MessageRow;
use crate::ui;

pub fn render(
    m: &MessageRow,
    ctx: &MessageCtx,
    theme: &Theme,
    inline: &[InlineRenderer],
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    if m.deleted {
        return div()
            .italic()
            .text_color(theme.faint)
            .child(t!("core-message-deleted"))
            .into_any_element();
    }
    let mut extra = Vec::new();
    let mut elements = Vec::new();
    for renderer in inline {
        for span in (renderer.spans)(m, theme, cx) {
            if span.range.end > m.body.text.len() {
                continue;
            }
            match span.inline {
                Inline::Style(style) => extra.push((span.range, style)),
                Inline::Element(draw) => elements.push((span.range, draw)),
            }
        }
    }
    let env = Env::new(ctx, cx);
    let mut body = Body {
        m,
        ctx,
        theme,
        env: &env,
        extra: &extra,
        elements: &elements,
        paragraphs: 0,
        codes: 0,
        lists: 0,
        mono: cx.theme().mono_font_family.clone(),
    };
    let blocks = message_blocks(&Mapped::new(&m.body.text), m.action);
    let last_is_text = matches!(blocks.last(), Some(Block::Text(_)) | None);
    let count = blocks.len();
    let mut column = div().flex().flex_col().gap_0p5().min_w_0();
    if blocks.is_empty() {
        column = column.child(body.paragraph(&Mapped::default(), true, true, window, cx));
    }
    for (i, block) in blocks.iter().enumerate() {
        column = column.child(body.block(block, i == 0, i + 1 == count, window, cx));
    }
    if m.edited && !last_is_text {
        column = column.child(
            div()
                .text_xs()
                .text_color(theme.faint)
                .child(t!("core-message-edited")),
        );
    }
    let color = if m.notice { theme.muted } else { theme.text };
    div()
        .text_color(color)
        .when(m.action, |d| d.italic())
        .child(column)
        .into_any_element()
}

/// What the text needs from the workspace: who a `@nick` is, which channels
/// you are in, and the server's origin for message links.
struct Env {
    session: Entity<Session>,
    network: String,
    case_mapping: String,
    origin: Option<String>,
    /// Folded nicks of the conversation's members, and whether each is you.
    members: Vec<(String, bool)>,
}

impl Env {
    fn new(ctx: &MessageCtx, cx: &App) -> Env {
        let session = ctx.session.read(cx);
        let mut env = Env {
            session: ctx.session.clone(),
            network: String::new(),
            case_mapping: String::new(),
            origin: session.server().map(|s| s.origin()),
            members: Vec::new(),
        };
        let Some(client) = session.client() else {
            return env;
        };
        let w = client.read();
        let Some(conversation) = w.conversation(&ctx.conversation) else {
            return env;
        };
        env.network = conversation.network_id.clone();
        if let Some(network) = w.network(&conversation.network_id) {
            env.case_mapping = network
                .isupport
                .get("CASEMAPPING")
                .cloned()
                .unwrap_or_default();
            if !network.nick.is_empty() {
                env.members
                    .push((fold(&network.nick, &env.case_mapping), true));
            }
        }
        for id in &conversation.member_ids {
            if let Some(member) = w.member(id, &conversation.network_id) {
                let own = w.is_self(id, &conversation.network_id);
                env.members
                    .push((fold(&member.nick, &env.case_mapping), own));
            }
        }
        env
    }

    /// Whether `nick` is someone here, and whether that is you.
    fn member(&self, nick: &str) -> Option<bool> {
        let folded = fold(nick, &self.case_mapping);
        self.members
            .iter()
            .filter(|(n, _)| *n == folded)
            .map(|(_, own)| *own)
            .max()
    }

    fn message_link(&self, url: &str) -> Option<MessageLink> {
        parse_message_link(url, self.origin.as_deref()?)
    }

    /// Where a message link points, for its pill.
    fn place(&self, link: &MessageLink, cx: &App) -> Option<String> {
        let client = self.session.read(cx).client()?.clone();
        let w = client.read();
        let c = w.conversation(&link.conversation_id)?;
        Some(match c.kind {
            ConversationKind::Channel if c.party.is_none() => format!("#{}", c.name),
            _ => c.name.clone(),
        })
    }
}

/// What a pill or a link does when clicked.
#[derive(Clone)]
enum Action {
    Url(String),
    Irc(String),
    Channel(String),
    Message(MessageLink),
}

/// What the text shows in place of an [`Inline::Element`]: figure spaces,
/// which take a fixed width and never break between themselves, so the
/// element's place is one box on one line.
pub const ELEMENT_SPACE: &str = "\u{2007}\u{2007}\u{2007}";

/// A pill, a link or an inline element in a paragraph.
#[derive(Clone)]
struct Piece {
    range: Range<usize>,
    /// The pill's background; links have none.
    background: Option<Hsla>,
    label: SharedString,
    action: Option<Action>,
    /// Drawn over the piece's place (an emoji picture).
    element: Option<ElementFn>,
}

struct Body<'a> {
    m: &'a MessageRow,
    ctx: &'a MessageCtx,
    theme: &'a Theme,
    env: &'a Env,
    /// Styles from the registered inline renderers, over the whole text.
    extra: &'a [(Range<usize>, HighlightStyle)],
    /// Elements from the registered inline renderers, over the whole text.
    elements: &'a [(Range<usize>, ElementFn)],
    paragraphs: usize,
    codes: usize,
    lists: usize,
    mono: SharedString,
}

impl Body<'_> {
    fn block(
        &mut self,
        block: &Block,
        first: bool,
        last: bool,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        match block {
            Block::Text(text) => self.paragraph(text, first, last, window, cx),
            Block::Code { code, lang } => self.code(&code.text, lang.as_deref()),
            Block::Quote(inner) => {
                let mut quote = div()
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .pl_3()
                    .border_l_4()
                    .border_color(self.theme.faint)
                    .rounded_sm();
                for b in inner {
                    quote = quote.child(self.block(b, false, false, window, cx));
                }
                quote.into_any_element()
            }
            Block::List(list) => self.list(list, 0, window, cx),
        }
    }

    fn list(&mut self, list: &List, depth: usize, window: &mut Window, cx: &mut App) -> AnyElement {
        let list_id = format!("list-{}-{}", self.m.id, self.lists);
        self.lists += 1;
        let mut column = div()
            .id(ElementId::Name(list_id.clone().into()))
            .test_support()
            .role(Role::List)
            .flex()
            .flex_col()
            .gap_0p5();
        for (i, item) in list.items.iter().enumerate() {
            let marker = if list.ordered {
                format!("{}.", list.start.saturating_add(i as u64))
            } else {
                ["•", "◦", "▪"][depth % 3].to_string()
            };
            let mut content = div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap_0p5()
                .child(self.paragraph(&item.text, false, false, window, cx));
            for child in &item.children {
                content = content.child(self.list(child, depth + 1, window, cx));
            }
            column = column.child(
                div()
                    .id(ElementId::Name(format!("{list_id}-{i}").into()))
                    .test_support()
                    .role(Role::ListItem)
                    .flex()
                    .gap_1()
                    .child(
                        div()
                            .flex_none()
                            .min_w(px(18.))
                            .text_right()
                            .text_color(self.theme.muted)
                            .child(marker),
                    )
                    .child(content),
            );
        }
        column.into_any_element()
    }

    fn code(&mut self, code: &str, lang: Option<&str>) -> AnyElement {
        let n = self.codes;
        self.codes += 1;
        let id = format!("code-{}-{n}", self.m.id);
        let theme = self.theme;
        let highlights: Vec<(Range<usize>, HighlightStyle)> = highlight(code, lang)
            .into_iter()
            .map(|(r, kind)| (r, code_style(kind, theme)))
            .collect();
        let label: SharedString = match lang {
            Some(lang) => t!("text-code-language", language = lang).into(),
            None => t!("text-code").into(),
        };
        let source = code.to_string();
        let copy = ui::icon::button(
            ElementId::Name(format!("{id}-copy").into()),
            ui::icon::IconName::Copy,
            t!("text-code-copy"),
            theme,
            move |_, window, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(source.clone()));
                ui::toast::success(window, cx, t!("text-code-copied"));
            },
        );
        div()
            .id(ElementId::Name(id.clone().into()))
            .test_support()
            .role(Role::Group)
            .aria_label(label)
            .max_w_full()
            .my_0p5()
            .rounded_md()
            .border_1()
            .border_color(theme.rail)
            .bg(theme.code)
            .not_italic()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .pl_2p5()
                    .pr_1()
                    .pt_0p5()
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.faint)
                            .child(lang.unwrap_or_default().to_lowercase()),
                    )
                    .child(copy),
            )
            .child(
                div()
                    .id(ElementId::Name(format!("{id}-scroll").into()))
                    .overflow_x_scroll()
                    .max_h(rems(24.))
                    .px_2p5()
                    .pb_2()
                    .font_family(self.mono.clone())
                    .text_sm()
                    .whitespace_nowrap()
                    .child(
                        RichText::new(
                            ElementId::Name(format!("{id}-text").into()),
                            code.to_string(),
                        )
                        .highlights(highlights),
                    ),
            )
            .into_any_element()
    }

    fn paragraph(
        &mut self,
        text: &Mapped,
        first: bool,
        last: bool,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        let n = self.paragraphs;
        self.paragraphs += 1;
        let id = format!("text-{}-{n}", self.m.id);
        let theme = self.theme;
        let f = text.formatted(&self.m.body);
        let mut spans = highlights_for(&f, theme);
        spans.extend(
            self.extra
                .iter()
                .filter_map(|(r, s)| Some((text.project(r)?, *s))),
        );
        // Pills and links, and the text each pill shows in place of its source.
        let mut pieces = Vec::new();
        let mut replace: Vec<(Range<usize>, String)> = Vec::new();
        for (range, token) in tokens(&f) {
            match token {
                Token::Link(url) => {
                    if let Some(link) = self.env.message_link(&url) {
                        let shown = match self.env.place(&link, cx) {
                            Some(place) => t!("text-message-link", place = place),
                            None => t!("text-message-link-here").to_string(),
                        };
                        let label = shown.clone();
                        replace.push((range.clone(), shown));
                        pieces.push(Piece {
                            range,
                            background: Some(theme.selected),
                            label: label.into(),
                            action: Some(Action::Message(link)),
                            element: None,
                        });
                    } else {
                        let irc = url.to_ascii_lowercase().starts_with("irc");
                        pieces.push(Piece {
                            range,
                            background: None,
                            label: t!("text-open-link", url = url.clone()).into(),
                            action: Some(if irc {
                                Action::Irc(url)
                            } else {
                                Action::Url(url)
                            }),
                            element: None,
                        });
                    }
                }
                Token::Mention(nick) => {
                    let own = match self.env.member(&nick) {
                        Some(own) => own,
                        None if group_mention(&f.text[range.clone()]).is_some() => false,
                        None => continue,
                    };
                    let color = if own { theme.warning } else { theme.link };
                    spans.push((range.clone(), pill_style(color)));
                    pieces.push(Piece {
                        range,
                        background: Some(if own {
                            theme::hex_alpha(0xf0b232, 0.16)
                        } else {
                            theme.selected
                        }),
                        label: t!("text-mention", name = nick).into(),
                        action: None,
                        element: None,
                    });
                }
                Token::Channel(name) => {
                    if self.env.network.is_empty() {
                        continue;
                    }
                    let joined = crate::features::text::joined_channel(
                        self.env.session.read(cx),
                        &self.env.network,
                        &name,
                    )
                    .is_some();
                    spans.push((range.clone(), pill_style(theme.link)));
                    pieces.push(Piece {
                        range,
                        background: Some(theme.selected),
                        label: if joined {
                            t!("text-open-channel", channel = name.clone()).into()
                        } else {
                            t!("text-join-channel", channel = name.clone()).into()
                        },
                        action: Some(Action::Channel(name)),
                        element: None,
                    });
                }
            }
        }
        // Inline elements, where no pill or link already is.
        for (range, draw) in self.elements {
            let Some(range) = text.project(range) else {
                continue;
            };
            if range.is_empty()
                || pieces
                    .iter()
                    .any(|p: &Piece| p.range.start < range.end && range.start < p.range.end)
            {
                continue;
            }
            replace.push((range.clone(), ELEMENT_SPACE.to_string()));
            pieces.push(Piece {
                label: f.text[range.clone()].to_string().into(),
                range,
                background: None,
                action: None,
                element: Some(draw.clone()),
            });
        }
        replace.sort_by_key(|(r, _)| r.start);
        pieces.sort_by_key(|p| p.range.start);
        let mut highlights = merge(spans);
        let mut shown = f.text.clone();
        if !replace.is_empty() {
            (shown, highlights, pieces) = substitute(&f.text, highlights, pieces, &replace, theme);
        }
        // An action's sender, and the edited mark, sit on the first and last lines.
        if first && self.m.action {
            let prefix = format!("* {} ", self.m.sender);
            let len = prefix.len();
            shown.insert_str(0, &prefix);
            highlights = shift(highlights, len);
            highlights.insert(
                0,
                (
                    0..len - 1,
                    HighlightStyle {
                        font_weight: Some(FontWeight::SEMIBOLD),
                        color: self.m.sender_color.map(theme::hex),
                        ..Default::default()
                    },
                ),
            );
            for p in &mut pieces {
                p.range = p.range.start + len..p.range.end + len;
            }
        }
        if last && self.m.edited {
            let start = shown.len();
            shown.push(' ');
            shown.push_str(t!("core-message-edited"));
            highlights.push((
                start..shown.len(),
                HighlightStyle {
                    color: Some(theme.faint),
                    ..Default::default()
                },
            ));
        }
        let element_id = ElementId::Name(id.clone().into());
        let places = places_for(&id, window, cx);
        let text = RichText::new(element_id, shown)
            .highlights(highlights)
            .pills(
                pieces
                    .iter()
                    .map(|p| (p.range.clone(), p.background))
                    .collect(),
            )
            .places(places.clone());
        let mut wrapper = div().relative().min_w_0().child(text);
        let known = places.borrow().clone();
        for (i, piece) in pieces.into_iter().enumerate() {
            let Some(bounds) = known.get(i) else {
                continue;
            };
            for (j, b) in bounds.iter().enumerate() {
                wrapper = wrapper.child(self.overlay(&id, (i, j), *b, &piece, window, cx));
            }
        }
        wrapper.into_any_element()
    }

    /// The element over one line of a pill or a link.
    fn overlay(
        &self,
        id: &str,
        (i, j): (usize, usize),
        bounds: Bounds<Pixels>,
        piece: &Piece,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        let overlay_id = if j == 0 {
            format!("{id}-piece-{i}")
        } else {
            format!("{id}-piece-{i}-{j}")
        };
        let mut d = div()
            .id(ElementId::Name(overlay_id.into()))
            .test_support()
            .absolute()
            .left(bounds.origin.x)
            .top(bounds.origin.y)
            .w(bounds.size.width)
            .h(bounds.size.height);
        // Only the first line of a wrapped pill speaks; the rest repeat it.
        if j == 0 {
            d = d.aria_label(piece.label.clone());
        }
        if let Some(draw) = &piece.element {
            if j > 0 {
                return d.into_any_element();
            }
            return d
                .role(Role::Image)
                .flex()
                .items_center()
                .justify_center()
                .tooltip(ui::tooltip::text(piece.label.clone()))
                .child(draw(window, cx))
                .into_any_element();
        }
        match piece.action.clone() {
            None => d.role(Role::Mark).into_any_element(),
            Some(action) => {
                let session = self.ctx.session.clone();
                let network = self.env.network.clone();
                let d = d
                    .role(if matches!(action, Action::Url(_)) {
                        Role::Link
                    } else {
                        Role::Button
                    })
                    .cursor_pointer()
                    .tooltip(ui::tooltip::text(piece.label.clone()))
                    .on_click(move |_, window, cx| {
                        cx.stop_propagation();
                        run(&action, &session, &network, window, cx);
                    });
                // One Tab stop per link: the first line of a wrapped one.
                if j == 0 {
                    d.reachable(crate::theme::get(cx)).into_any_element()
                } else {
                    d.into_any_element()
                }
            }
        }
    }
}

fn run(
    action: &Action,
    session: &Entity<Session>,
    network: &str,
    window: &mut Window,
    cx: &mut App,
) {
    use crate::features::text;
    match action {
        Action::Url(url) => cx.open_url(url),
        Action::Irc(url) => text::open_irc_link(session, url, window, cx),
        Action::Channel(name) => text::open_channel(session, network, name, window, cx),
        Action::Message(link) => text::open_message_link(session, link, window, cx),
    }
}

/// The places a paragraph's pills landed in the last frame, kept between
/// frames under the paragraph's ID.
fn places_for(id: &str, window: &mut Window, cx: &mut App) -> Places {
    let state = window.use_keyed_state(
        ElementId::Name(format!("{id}-places").into()),
        cx,
        |_, _| Places::default(),
    );
    state.read(cx).clone()
}

fn pill_style(color: Hsla) -> HighlightStyle {
    HighlightStyle {
        color: Some(color),
        font_weight: Some(FontWeight::MEDIUM),
        underline: None,
        ..Default::default()
    }
}

fn code_style(kind: Kind, theme: &Theme) -> HighlightStyle {
    let color = match kind {
        Kind::Keyword => theme.accent,
        Kind::String | Kind::Added => theme.online,
        Kind::Comment => theme.faint,
        Kind::Number | Kind::Literal => theme.warning,
        Kind::Removed => theme.danger,
        Kind::Meta => theme.link,
    };
    HighlightStyle {
        color: Some(color),
        font_style: (kind == Kind::Comment).then_some(FontStyle::Italic),
        ..Default::default()
    }
}

/// Styled ranges of a paragraph's text.
type Highlights = Vec<(Range<usize>, HighlightStyle)>;

fn shift(
    highlights: Vec<(Range<usize>, HighlightStyle)>,
    by: usize,
) -> Vec<(Range<usize>, HighlightStyle)> {
    highlights
        .into_iter()
        .map(|(r, s)| (r.start + by..r.end + by, s))
        .collect()
}

/// The text with each `replace` range showing other text (a message link as
/// its pill's words), and highlights and pieces moved to match. Highlights
/// inside a replaced range give way to the pill's own style.
fn substitute(
    text: &str,
    highlights: Vec<(Range<usize>, HighlightStyle)>,
    pieces: Vec<Piece>,
    replace: &[(Range<usize>, String)],
    theme: &Theme,
) -> (String, Highlights, Vec<Piece>) {
    let mut out = String::new();
    let mut at = 0;
    // (old start, old end, new start, new end) for each replacement.
    let mut moves = Vec::new();
    for (range, with) in replace {
        out.push_str(&text[at..range.start]);
        let start = out.len();
        out.push_str(with);
        moves.push((range.start, range.end, start, out.len()));
        at = range.end;
    }
    out.push_str(&text[at..]);
    let map = |i: usize| -> usize {
        let mut delta: isize = 0;
        for (old_start, old_end, new_start, new_end) in &moves {
            if i >= *old_end {
                delta += (*new_end - *new_start) as isize - (*old_end - *old_start) as isize;
            } else if i > *old_start {
                return *new_end;
            }
        }
        (i as isize + delta) as usize
    };
    let inside = |r: &Range<usize>| moves.iter().any(|(s, e, _, _)| r.start < *e && *s < r.end);
    let mut new_highlights: Vec<(Range<usize>, HighlightStyle)> = highlights
        .into_iter()
        .filter(|(r, _)| !inside(r))
        .map(|(r, s)| (map(r.start)..map(r.end), s))
        .collect();
    for (_, _, s, e) in &moves {
        new_highlights.push((*s..*e, pill_style(theme.link)));
    }
    new_highlights.sort_by_key(|(r, _)| r.start);
    let pieces = pieces
        .into_iter()
        .map(|mut p| {
            p.range = match moves.iter().find(|(s, e, _, _)| p.range == (*s..*e)) {
                Some((_, _, s, e)) => *s..*e,
                None => map(p.range.start)..map(p.range.end),
            };
            p
        })
        .collect();
    (out, new_highlights, pieces)
}

fn irc_color(color: IrcColor, theme: &Theme) -> Hsla {
    match color {
        IrcColor::Rgb(rgb) => theme::hex(rgb),
        IrcColor::ThemeText => theme.text,
        IrcColor::ThemeBackground => theme.main,
    }
}

/// Formatting runs and links as highlights.
pub fn highlights_for(f: &Formatted, theme: &Theme) -> Vec<(Range<usize>, HighlightStyle)> {
    let mut out = Vec::new();
    for (r, s) in &f.runs {
        let mut style = HighlightStyle::default();
        if s.bold {
            style.font_weight = Some(FontWeight::BOLD);
        }
        if s.italic {
            style.font_style = Some(FontStyle::Italic);
        }
        if s.underline {
            style.underline = Some(UnderlineStyle {
                thickness: px(1.),
                ..Default::default()
            });
        }
        if s.strike {
            style.strikethrough = Some(StrikethroughStyle {
                thickness: px(1.),
                ..Default::default()
            });
        }
        style.color = s.fg.map(|c| irc_color(c, theme));
        style.background_color =
            s.bg.map(|c| irc_color(c, theme))
                .or(s.mono.then_some(theme.code));
        out.push((r.clone(), style));
    }
    for (r, _) in &f.links {
        out.push((
            r.clone(),
            HighlightStyle {
                color: Some(theme.link),
                underline: Some(UnderlineStyle {
                    thickness: px(1.),
                    ..Default::default()
                }),
                ..Default::default()
            },
        ));
    }
    out
}

/// Overlapping highlights as non-overlapping ones in order; where several
/// cover a piece of text, later ones win field by field.
pub fn merge(spans: Vec<(Range<usize>, HighlightStyle)>) -> Vec<(Range<usize>, HighlightStyle)> {
    let mut cuts: Vec<usize> = spans.iter().flat_map(|(r, _)| [r.start, r.end]).collect();
    cuts.sort_unstable();
    cuts.dedup();
    let mut out = Vec::new();
    for pair in cuts.windows(2) {
        let (start, end) = (pair[0], pair[1]);
        let mut style: Option<HighlightStyle> = None;
        for (r, s) in &spans {
            if r.start <= start && end <= r.end {
                style = Some(match style {
                    Some(base) => base.highlight(*s),
                    None => *s,
                });
            }
        }
        if let Some(style) = style {
            out.push((start..end, style));
        }
    }
    out
}

// ---- The paragraph element ------------------------------------------------

/// Where each pill's lines landed, relative to the paragraph's top left.
type Places = Rc<RefCell<Vec<Vec<Bounds<Pixels>>>>>;

/// Selectable text, as gpui-base's `SelectableText`, that also paints
/// rounded pill backgrounds under ranges and records where those ranges
/// landed (`places`), for the elements the body lays over them.
pub struct RichText {
    id: ElementId,
    text: SharedString,
    styled: StyledText,
    pills: Vec<(Range<usize>, Option<Hsla>)>,
    places: Option<Places>,
}

impl RichText {
    pub fn new(id: impl Into<ElementId>, text: impl Into<SharedString>) -> Self {
        let text = text.into();
        RichText {
            id: id.into(),
            styled: StyledText::new(text.clone()),
            text,
            pills: Vec::new(),
            places: None,
        }
    }

    /// Styles ranges, in order and without overlap.
    pub fn highlights(mut self, highlights: Vec<(Range<usize>, HighlightStyle)>) -> Self {
        self.styled = self.styled.with_highlights(highlights);
        self
    }

    fn pills(mut self, pills: Vec<(Range<usize>, Option<Hsla>)>) -> Self {
        self.pills = pills;
        self
    }

    fn places(mut self, places: Places) -> Self {
        self.places = Some(places);
        self
    }
}

/// A run's own selection, kept between frames.
struct OwnedSelection {
    handle: TextSelectionHandle,
    _refresh: Subscription,
}

impl IntoElement for RichText {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for RichText {
    type RequestLayoutState = TextSelectionHandle;
    type PrepaintState = Hitbox;

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        global_id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let text = self.text.clone();
        let handle = window.with_element_state(
            global_id.expect("RichText has an ID"),
            |retained: Option<OwnedSelection>, window| {
                let retained = retained.unwrap_or_else(|| {
                    let handle = TextSelectionHandle::new(text, cx);
                    let refresh = handle.refresh_window_on_change(window, cx);
                    OwnedSelection {
                        handle,
                        _refresh: refresh,
                    }
                });
                (retained.handle.clone(), retained)
            },
        );
        let (layout_id, ()) = self
            .styled
            .request_layout(global_id, inspector_id, window, cx);
        (layout_id, handle)
    }

    fn prepaint(
        &mut self,
        global_id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        handle: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        self.styled
            .prepaint(global_id, inspector_id, bounds, &mut (), window, cx);
        let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);
        let registration = TextSelectionRegistration::new(hitbox.clone(), bounds)
            .with_text_bounds(vec![bounds])
            .with_rendered_element(handle, window, cx);
        handle.register(registration, window, cx);
        hitbox
    }

    fn paint(
        &mut self,
        global_id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        handle: &mut Self::RequestLayoutState,
        _: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let layout = self.styled.layout().clone();
        let mut places = Vec::with_capacity(self.pills.len());
        for (range, background) in &self.pills {
            let quads = range_bounds(&layout, range.clone(), bounds, true);
            if let Some(color) = background {
                for q in &quads {
                    window.paint_quad(
                        fill(
                            Bounds::new(
                                point(q.origin.x - px(2.), q.origin.y),
                                size(q.size.width + px(4.), q.size.height),
                            ),
                            *color,
                        )
                        .corner_radii(px(3.)),
                    );
                }
            }
            places.push(
                quads
                    .into_iter()
                    .map(|q| Bounds::new(q.origin - bounds.origin, q.size))
                    .collect::<Vec<_>>(),
            );
        }
        if let Some(store) = &self.places
            && *store.borrow() != places
        {
            *store.borrow_mut() = places;
            window.refresh();
        }
        let projection = handle.update_runs(
            &[TextSelectionRun::new(
                self.text.clone(),
                layout.clone(),
                bounds,
            )],
            cx,
        );
        let color = gpui_kit::base::Theme::global(cx).tokens.colors.selection;
        for range in projection.ranges().iter().flatten().cloned() {
            for q in range_bounds(&layout, range, bounds, false) {
                window.paint_quad(fill(q, color));
            }
        }
        self.styled.paint(
            global_id,
            inspector_id,
            bounds,
            &mut (),
            &mut (),
            window,
            cx,
        );
    }
}

/// The boxes a range of laid-out text covers, one per line.
fn range_bounds(
    layout: &TextLayout,
    range: Range<usize>,
    bounds: Bounds<Pixels>,
    ascii_start: bool,
) -> Vec<Bounds<Pixels>> {
    let (Some(mut start), Some(end)) = (
        layout.position_for_index(range.start),
        layout.position_for_index(range.end),
    ) else {
        return Vec::new();
    };
    let line = layout.line_height();
    // At a wrap, the start reads as the end of the line before; the range
    // begins on the next line. Pills and links start with an ASCII character;
    // a selection may not, so it is drawn as gpui-base draws it.
    if ascii_start
        && range.start < range.end
        && let Some(next) = layout.position_for_index(range.start + 1)
        && next.y > start.y
    {
        start = point(bounds.left(), next.y);
    }
    if start.y == end.y {
        return vec![Bounds::from_corners(start, point(end.x, end.y + line))];
    }
    let mut quads = vec![Bounds::from_corners(
        start,
        point(bounds.right(), start.y + line),
    )];
    let mut y = start.y + line;
    while y < end.y {
        quads.push(Bounds::from_corners(
            point(bounds.left(), y),
            point(bounds.right(), y + line),
        ));
        y += line;
    }
    if end.x > bounds.left() {
        quads.push(Bounds::from_corners(
            point(bounds.left(), end.y),
            point(end.x, end.y + line),
        ));
    }
    quads
}

#[cfg(test)]
mod tests {
    use super::{Theme, highlights_for, merge, theme};
    use gpui_kit::{FontWeight, HighlightStyle};

    #[test]
    fn merge_splits_overlaps_and_later_wins() {
        let bold = HighlightStyle {
            font_weight: Some(FontWeight::BOLD),
            ..Default::default()
        };
        let red = HighlightStyle {
            color: Some(theme::hex(0xff0000)),
            ..Default::default()
        };
        let out = merge(vec![(0..10, bold), (5..15, red)]);
        let ranges: Vec<_> = out.iter().map(|(r, _)| r.clone()).collect();
        assert_eq!(ranges, [0..5, 5..10, 10..15]);
        assert_eq!(out[1].1.font_weight, Some(FontWeight::BOLD));
        assert_eq!(out[1].1.color, red.color);
        assert_eq!(out[2].1.font_weight, None);
    }

    #[test]
    fn links_are_highlighted() {
        let f = ircord_client::format::parse("see https://ircord.com now");
        let theme = Theme::default();
        let spans = merge(highlights_for(&f, &theme));
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].1.color, Some(theme.link));
    }
}

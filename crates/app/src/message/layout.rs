//! How a message row is laid out (package S2), from the message settings in
//! the appearance document, as the web client's `MessageItem.vue`:
//!
//! - Cozy: a 40 px picture and the name and time over each group.
//! - Compact: a 24 px picture, a smaller name, tighter rows; grouped rows
//!   keep the text column without a time.
//! - IRC: one line per message, `time <nick> text`, the nick in a fixed
//!   column; reactions, files and links follow the line.
//!
//! The message font, text size, space between groups, the clock and coloured
//! names apply to every layout. [`settings`] reads them through the
//! appearance package (S1), so they follow this device when appearance sync
//! is off.

use crate::ui::focus::Reachable;
use chrono::{DateTime, Local};
use gpui_kit::TestSupportExt;
use gpui_kit::component::menu::ContextMenuExt;
use gpui_kit::prelude::*;
use gpui_kit::*;
use ircord_client::contract::{Appearance, AppearanceMessageFont, AppearanceMessageLayout};
use ircord_client::store::Workspace;

use super::{MessageView, attachments, body, links, reactions, toolbar};
use crate::registry::{self, MemberCtx};
use crate::theme::{self, Theme};
use crate::ui;

/// Message text size in pixels: the web client's range and default.
pub const SIZE: (i64, i64, i64) = (12, 20, 14);
/// Space before each new speaker in pixels, in steps of [`SPACING_STEP`].
pub const SPACING: (i64, i64, i64) = (0, 24, 12);
pub const SPACING_STEP: i64 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Layout {
    Cozy,
    Compact,
    Irc,
}

impl Layout {
    pub const ALL: [Layout; 3] = [Layout::Cozy, Layout::Compact, Layout::Irc];

    pub fn wire(self) -> AppearanceMessageLayout {
        match self {
            Layout::Cozy => AppearanceMessageLayout::Cozy,
            Layout::Compact => AppearanceMessageLayout::Compact,
            Layout::Irc => AppearanceMessageLayout::Irc,
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Layout::Cozy => "cozy",
            Layout::Compact => "compact",
            Layout::Irc => "irc",
        }
    }
}

/// The message settings, with the web client's defaults for what is unset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Look {
    pub layout: Layout,
    pub font: AppearanceMessageFont,
    pub size: i64,
    pub spacing: i64,
    pub clock_12h: bool,
    pub colored_names: bool,
    pub animate_media: bool,
}

impl Default for Look {
    fn default() -> Self {
        Self {
            layout: Layout::Cozy,
            font: AppearanceMessageFont::Interface,
            size: SIZE.2,
            spacing: SPACING.2,
            clock_12h: false,
            colored_names: true,
            animate_media: true,
        }
    }
}

impl Look {
    pub fn of(a: &Appearance) -> Self {
        use ircord_client::contract::AppearanceTimeFormat;
        let d = Self::default();
        Self {
            layout: match a.message_layout {
                Some(AppearanceMessageLayout::Compact) => Layout::Compact,
                Some(AppearanceMessageLayout::Irc) => Layout::Irc,
                _ => Layout::Cozy,
            },
            font: match a.message_font {
                Some(AppearanceMessageFont::Unknown) | None => d.font,
                Some(font) => font,
            },
            size: a.message_size.unwrap_or(d.size).clamp(SIZE.0, SIZE.1),
            spacing: a
                .group_spacing
                .unwrap_or(d.spacing)
                .clamp(SPACING.0, SPACING.1),
            clock_12h: a.time_format == Some(AppearanceTimeFormat::V12h),
            colored_names: a.colored_names.unwrap_or(d.colored_names),
            animate_media: a.animate_media.unwrap_or(d.animate_media),
        }
    }

    /// The font family the message text is drawn in; `None` is the
    /// interface font. The files themselves are bundled by S1; a family the
    /// system lacks falls back to the interface font.
    pub fn family(&self) -> Option<&'static str> {
        family(self.font)
    }
}

/// The family name for a message font choice.
pub fn family(font: AppearanceMessageFont) -> Option<&'static str> {
    match font {
        AppearanceMessageFont::Atkinson => Some("Atkinson Hyperlegible Next"),
        AppearanceMessageFont::Lexend => Some("Lexend"),
        AppearanceMessageFont::Serif => Some("Source Serif 4"),
        AppearanceMessageFont::Literata => Some("Literata"),
        AppearanceMessageFont::JetbrainsMinusMono => Some("JetBrains Mono"),
        AppearanceMessageFont::GeistMinusMono => Some("Geist Mono"),
        AppearanceMessageFont::AtkinsonMinusMono => Some("Atkinson Hyperlegible Mono"),
        AppearanceMessageFont::Interface | AppearanceMessageFont::Unknown => None,
    }
}

/// The message settings shown: the account's with appearance sync on, this
/// device's with it off (`theme::appearance::current`).
pub fn settings(cx: &App) -> Look {
    let look = Look::of(&crate::theme::appearance::current(cx));
    CLOCK_12H.with(|c| c.set(look.clock_12h));
    look
}

thread_local! {
    /// The clock [`look`] saw last, for the timeline's rows, which are built
    /// from the workspace alone right after it.
    static CLOCK_12H: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// A message time as the clock setting shows it: "14:05" or "2:05 PM".
pub fn time_label(_w: &Workspace, at: &DateTime<Local>) -> String {
    clock(CLOCK_12H.with(|c| c.get()), at)
}

pub fn clock(twelve: bool, at: &DateTime<Local>) -> String {
    if twelve {
        at.format("%-I:%M %p").to_string()
    } else {
        at.format("%H:%M").to_string()
    }
}

/// A fingerprint of what the timeline's rows depend on from the message
/// settings: when it changes, the timeline measures every row again.
pub fn look(cx: &App) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    settings(cx).hash(&mut h);
    h.finish()
}

pub fn render(view: MessageView, theme: &Theme, window: &mut Window, cx: &mut App) -> AnyElement {
    let look = settings(cx);
    let m = &view.row;
    let sender_color = m.sender_color.filter(|_| look.colored_names);
    let name_color = sender_color.map(theme::hex).unwrap_or(theme.text);
    // A click on the name or picture opens the person (`Registry::open_person`).
    let open_person = registry::get(cx).open_person.clone().map(|open| {
        let ctx = MemberCtx {
            session: view.ctx.session.clone(),
            member: m.sender_id.clone(),
            conversation: Some(view.ctx.conversation.clone()),
        };
        move |_: &ClickEvent, window: &mut Window, cx: &mut App| open(&ctx, window, cx)
    });
    let person = |el: Div, id: String, label: String| -> AnyElement {
        match open_person.clone() {
            Some(open) => el
                .id(ElementId::Name(id.into()))
                .test_support()
                .role(Role::Link)
                .reachable(&theme)
                .aria_label(SharedString::from(label))
                .cursor_pointer()
                .on_click(open)
                .into_any_element(),
            None => el.into_any_element(),
        }
    };
    let text = |d: Div| -> Div {
        let d = d.text_size(px(look.size as f32));
        match look.family() {
            Some(family) => d.font_family(family),
            None => d,
        }
    };

    let reply = m.reply.as_ref().map(|(nick, quoted)| {
        let quoted = ircord_client::format::parse(quoted).text;
        let line = t!("core-message-reply-line", nick = nick, text = quoted);
        let hover_text = theme.text;
        let reply = div()
            .id(ElementId::Name(format!("reply-{}", m.id).into()))
            .test_support()
            .text_xs()
            .text_color(theme.muted)
            .truncate()
            .child(line.clone());
        // Clicking the quote goes to the original (`timeline::reveal`).
        match m.reply_id.clone() {
            Some(original) => {
                let ctx = view.ctx.clone();
                reply
                    .role(Role::Link)
                    .reachable(&theme)
                    .aria_label(line)
                    .cursor_pointer()
                    .hover(move |d| d.text_color(hover_text))
                    .on_click(move |_, window, cx| {
                        crate::timeline::reveal(
                            &ctx.session,
                            &ctx.conversation,
                            &original,
                            window,
                            cx,
                        )
                    })
                    .into_any_element()
            }
            None => reply.into_any_element(),
        }
    });
    let inline = registry::get(cx).inline_renderers.to_vec();
    // A message of uploads only has no text left to draw.
    let body = (!(m.body.text.trim().is_empty() && !m.attachments.is_empty()))
        .then(|| body::render(m, &view.ctx, theme, &inline, window, cx));
    let extras: Vec<AnyElement> = reactions::render(m, &view.ctx, theme, window, cx)
        .into_iter()
        .chain(attachments::render(m, &view.ctx, theme, window, cx))
        .chain(links::render(m, &view.ctx, theme, window, cx))
        .map(IntoElement::into_any_element)
        .collect();

    let on_hover = view.on_hover.clone();
    let row_hover = theme.row_hover;
    // A screen reader names the row by who and when, as the web client does;
    // its text and controls are read inside it.
    let row_label = if m.mention {
        t!(
            "audit-message-row-mention",
            sender = m.sender,
            time = m.time
        )
    } else {
        t!("audit-message-row", sender = m.sender, time = m.time)
    };
    let mut frame = div()
        .id(ElementId::Name(format!("message-{}", m.id).into()))
        .test_support()
        .role(Role::Article)
        .aria_label(SharedString::from(row_label))
        .relative()
        .w_full()
        .px_4()
        .when(m.mention || m.focused, |d| {
            let mark = if m.focused {
                theme.accent
            } else {
                theme.mention
            };
            d.bg(mark.opacity(0.12)).border_l_2().border_color(mark)
        })
        .hover(move |d| d.bg(row_hover))
        .on_hover(move |over: &bool, window, cx| on_hover(*over, window, cx));

    match look.layout {
        Layout::Irc => {
            // One line: the time, the nick in its column, the text.
            let (before, after) = if m.action {
                ("* ", "")
            } else if m.notice {
                ("-", "-")
            } else {
                ("<", ">")
            };
            let faint = theme.faint;
            let nick = person(
                div()
                    .flex()
                    .justify_end()
                    .overflow_hidden()
                    .child(div().text_color(faint).child(before))
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(name_color)
                            .truncate()
                            .hover(|d| d.underline())
                            .child(m.sender.clone()),
                    )
                    .child(div().text_color(faint).child(after)),
                format!("sender-{}", m.id),
                m.sender.clone(),
            );
            let line = text(div())
                .w_full()
                .flex()
                .items_baseline()
                .gap_2p5()
                .child(
                    div()
                        .flex_none()
                        .text_size(px(look.size as f32 * 0.85))
                        .text_color(theme.faint)
                        .child(m.time.clone()),
                )
                .child(div().w(px(128.)).flex_none().child(nick))
                .child(div().flex_1().min_w_0().children(body));
            let mut column = div()
                .w_full()
                .flex()
                .flex_col()
                .gap_0p5()
                .py(px(1.))
                .children(reply)
                .child(line);
            if !extras.is_empty() {
                column = column.child(div().pl(px(138.)).flex().flex_col().children(extras));
            }
            frame = frame.child(column);
        }
        Layout::Cozy | Layout::Compact => {
            let compact = look.layout == Layout::Compact;
            let avatar = if compact { 24. } else { 40. };
            let mut column = text(div()).flex_1().min_w_0().flex().flex_col().gap_0p5();
            if m.header && !m.action {
                column = column.child(
                    div()
                        .flex()
                        .items_baseline()
                        .gap_2()
                        .child(person(
                            div()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_size(px(if compact { 14. } else { 15. }))
                                .text_color(name_color)
                                .hover(|d| d.underline())
                                .child(m.sender.clone()),
                            format!("sender-{}", m.id),
                            m.sender.clone(),
                        ))
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.faint)
                                .child(m.time.clone()),
                        ),
                );
            }
            column = column.children(reply).children(body).children(extras);
            let gutter = if m.header && !m.action {
                person(
                    ui::avatar::avatar(&m.sender, sender_color.map(theme::hex), px(avatar), theme),
                    format!("sender-avatar-{}", m.id),
                    m.sender.clone(),
                )
            } else {
                // Compact rows keep the text column without the time.
                div()
                    .w(px(avatar))
                    .flex_none()
                    .text_xs()
                    .text_color(theme.faint)
                    .pt_0p5()
                    .child(if m.action && !compact {
                        m.time.clone()
                    } else {
                        String::new()
                    })
                    .into_any_element()
            };
            let space = if compact {
                look.spacing as f32 / 2.
            } else {
                look.spacing as f32
            };
            frame = frame
                .py(px(if compact { 1. } else { 2. }))
                .when(m.header, |d| d.pt(px(space.max(2.))))
                .flex()
                .when(compact, |d| d.gap_2p5())
                .when(!compact, |d| d.gap_4())
                .child(gutter)
                .child(column);
        }
    }
    if view.hovered {
        frame = frame.child(toolbar(&view.ctx, theme, window, cx));
    }
    let ctx = view.ctx.clone();
    frame
        .context_menu(move |menu, _, cx| {
            let items = registry::get(cx).message_menu.to_vec();
            let items = ui::context_menu::visible(&items, &ctx, cx);
            ui::context_menu::fill(menu, items, ctx.clone())
        })
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::{Layout, Look, clock};
    use chrono::{Local, TimeZone};
    use ircord_client::contract::Appearance;

    #[test]
    fn unset_settings_are_the_web_clients_defaults() {
        let look = Look::of(&Appearance::default());
        assert_eq!(look, Look::default());
        assert_eq!(look.layout, Layout::Cozy);
        let a: Appearance = serde_json::from_value(serde_json::json!({
            "messageLayout": "irc", "messageSize": 40, "groupSpacing": -3,
            "timeFormat": "12h", "coloredNames": false, "messageFont": "geist-mono"
        }))
        .unwrap();
        let look = Look::of(&a);
        assert_eq!(look.layout, Layout::Irc);
        assert_eq!((look.size, look.spacing), (20, 0), "kept in range");
        assert!(look.clock_12h && !look.colored_names);
        assert_eq!(look.family(), Some("Geist Mono"));
    }

    #[test]
    fn the_clock_has_two_forms() {
        let at = Local.with_ymd_and_hms(2026, 11, 21, 14, 5, 0).unwrap();
        assert_eq!(clock(false, &at), "14:05");
        assert_eq!(clock(true, &at), "2:05 PM");
    }
}

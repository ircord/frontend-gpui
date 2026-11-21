//! The message text: IRC formatting and links as highlights over selectable
//! text, plus whatever registered inline renderers add.
//!
//! Inline renderers that return `Inline::Element` are drawn as their text
//! for now: drawing elements inside a line needs gpui-base's `TextView`,
//! which is package T1's change to this file.

use std::ops::Range;

use gpui_kit::base::SelectableText;
use gpui_kit::prelude::*;
use gpui_kit::*;
use ircord_client::format::{Color as IrcColor, Formatted};

use crate::registry::{Inline, InlineRenderer};
use crate::theme::{self, Theme};
use crate::timeline::rows::MessageRow;

pub fn render(m: &MessageRow, theme: &Theme, inline: &[InlineRenderer]) -> AnyElement {
    if m.deleted {
        return div()
            .italic()
            .text_color(theme.faint)
            .child(t!("core-message-deleted"))
            .into_any_element();
    }
    let mut text = String::new();
    let mut highlights = Vec::new();
    if m.action {
        text.push_str("* ");
        text.push_str(&m.sender);
        text.push(' ');
        highlights.push((
            0..text.len() - 1,
            HighlightStyle {
                font_weight: Some(FontWeight::SEMIBOLD),
                color: m.sender_color.map(theme::hex),
                ..Default::default()
            },
        ));
    }
    let offset = text.len();
    text.push_str(&m.body.text);
    let mut spans = highlights_for(&m.body, theme);
    for renderer in inline {
        for span in (renderer.spans)(m, theme) {
            if let Inline::Style(style) = span.inline
                && span.range.end <= m.body.text.len()
            {
                spans.push((span.range, style));
            }
        }
    }
    highlights.extend(
        merge(spans)
            .into_iter()
            .map(|(r, s)| (r.start + offset..r.end + offset, s)),
    );
    if m.edited {
        let start = text.len();
        text.push(' ');
        text.push_str(t!("core-message-edited"));
        highlights.push((
            start..text.len(),
            HighlightStyle {
                color: Some(theme.faint),
                ..Default::default()
            },
        ));
    }
    let color = if m.notice { theme.muted } else { theme.text };
    div()
        .text_color(color)
        .when(m.action, |d| d.italic())
        .child(
            SelectableText::new(ElementId::Name(format!("text-{}", m.id).into()), text)
                .highlights(highlights),
        )
        .into_any_element()
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

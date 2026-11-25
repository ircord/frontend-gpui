//! Plain Markdown for the lists in an HTML clipboard selection. Parsing
//! reads a document tree locally without displaying or fetching anything.

use std::borrow::Cow;
use std::ops::Range;

use html5ever::{parse_document, tendril::TendrilSink};
use markup5ever_rcdom::{Handle, NodeData, RcDom};

const MAX_HTML_BYTES: usize = 2 * 1024 * 1024;
const MAX_NODE_DEPTH: usize = 128;
const MAX_NUMBER: u32 = 999_999_999;
const START: &str = "ircord-selection-start";
const END: &str = "ircord-selection-end";

/// The pasted text if there is a list with contents. A byte range selects
/// part of a document while keeping the numbering of its enclosing list.
pub fn list(html: &str, selection: Option<Range<usize>>) -> Option<String> {
    if html.len() > MAX_HTML_BYTES {
        return None;
    }
    let has_selection = selection.is_some();
    let html = if let Some(range) = selection {
        let fragment = html.get(range.clone())?;
        Cow::Owned(format!(
            "{}<!--{START}-->{fragment}<!--{END}-->{}",
            &html[..range.start],
            &html[range.end..],
        ))
    } else {
        Cow::Borrowed(html)
    };
    let document = parse_document(RcDom::default(), Default::default()).one(html.as_ref());
    let mut text = Paste {
        uses_selection: has_selection,
        active: !has_selection,
        ..Default::default()
    };
    text.walk(&document.document, 0, 0)?;
    text.break_line();
    if !text.has_item || text.lines.is_empty() {
        return None;
    }
    let plain = text.lines.join("\n");
    Some(super::blocks::pasted_bullets(&plain).unwrap_or(plain))
}

#[derive(Default)]
struct Paste {
    lines: Vec<String>,
    content: String,
    prefix: String,
    in_item: usize,
    preformatted: usize,
    uses_selection: bool,
    active: bool,
    has_item: bool,
}

impl Paste {
    fn break_line(&mut self) {
        let line = self.content.trim_end_matches([' ', '\t']);
        if !line.trim().is_empty() {
            if !self.prefix.is_empty() {
                self.lines.push(format!("{}{line}", self.prefix));
                self.prefix.clear();
                self.has_item = true;
            } else if let Some(last) = self.lines.last_mut().filter(|_| self.in_item > 0) {
                last.push(' ');
                last.push_str(line.trim_start());
            } else {
                self.lines.push(line.to_string());
            }
        }
        self.content.clear();
    }

    fn text(&mut self, text: &str) {
        if !self.active {
            return;
        }
        if self.preformatted == 0 {
            let mut space = self.content.ends_with(' ');
            for c in text.chars() {
                if c.is_whitespace() {
                    if !space && !self.content.is_empty() {
                        self.content.push(' ');
                    }
                    space = true;
                } else {
                    self.content.push(c);
                    space = false;
                }
            }
        } else {
            for (i, line) in text.split('\n').enumerate() {
                if i > 0 {
                    self.break_line();
                }
                self.content.push_str(line);
            }
        }
    }

    fn walk(&mut self, node: &Handle, list_depth: usize, depth: usize) -> Option<()> {
        if depth >= MAX_NODE_DEPTH {
            return None;
        }
        match &node.data {
            NodeData::Text { contents } => self.text(&contents.borrow()),
            NodeData::Comment { contents } if self.uses_selection => match contents.as_ref() {
                START => self.active = true,
                END => {
                    self.break_line();
                    self.active = false;
                }
                _ => {}
            },
            NodeData::Element { name, .. } => {
                let tag = name.local.as_ref();
                match tag {
                    "script" | "style" | "template" | "head" | "title" | "meta" | "noscript"
                    | "select" | "textarea" => {}
                    "br" => self.break_line(),
                    "ol" | "ul" => self.walk_list(node, tag == "ol", list_depth, depth)?,
                    _ => {
                        let block = !inline(tag);
                        let pre = usize::from(tag == "pre");
                        if block {
                            self.break_line();
                        }
                        self.preformatted += pre;
                        self.walk_children(node, list_depth, depth)?;
                        self.preformatted -= pre;
                        if block {
                            self.break_line();
                        }
                    }
                }
            }
            NodeData::Document => self.walk_children(node, list_depth, depth)?,
            _ => {}
        }
        Some(())
    }

    fn walk_children(&mut self, node: &Handle, list_depth: usize, depth: usize) -> Option<()> {
        for child in node.children.borrow().iter() {
            self.walk(child, list_depth, depth + 1)?;
        }
        Some(())
    }

    fn walk_list(
        &mut self,
        node: &Handle,
        ordered: bool,
        list_depth: usize,
        depth: usize,
    ) -> Option<()> {
        self.break_line();
        let mut number = list_number(node, "start").unwrap_or(1);
        for child in node.children.borrow().iter() {
            match &child.data {
                NodeData::Element { name, .. } if name.local.as_ref() == "li" => {
                    self.break_line();
                    if ordered {
                        number = list_number(child, "value").unwrap_or(number);
                    }
                    self.prefix = "  ".repeat(list_depth);
                    if ordered {
                        use std::fmt::Write as _;
                        let _ = write!(self.prefix, "{number}. ");
                        number = (number + 1).min(MAX_NUMBER);
                    } else {
                        self.prefix.push_str("- ");
                    }
                    self.in_item += 1;
                    self.walk_children(child, list_depth + 1, depth + 1)?;
                    self.break_line();
                    self.in_item -= 1;
                    self.prefix.clear();
                }
                // Some editors put a nested list directly inside another list.
                NodeData::Element { name, .. } if matches!(name.local.as_ref(), "ol" | "ul") => {
                    self.walk(child, list_depth + 1, depth + 1)?;
                }
                _ => self.walk(child, list_depth, depth + 1)?,
            }
        }
        self.break_line();
        Some(())
    }
}

fn inline(tag: &str) -> bool {
    matches!(
        tag,
        "a" | "abbr"
            | "b"
            | "bdi"
            | "bdo"
            | "button"
            | "cite"
            | "code"
            | "data"
            | "del"
            | "dfn"
            | "em"
            | "font"
            | "i"
            | "img"
            | "ins"
            | "kbd"
            | "label"
            | "mark"
            | "q"
            | "s"
            | "samp"
            | "small"
            | "span"
            | "strike"
            | "strong"
            | "sub"
            | "sup"
            | "time"
            | "tt"
            | "u"
            | "var"
            | "wbr"
    )
}

fn list_number(node: &Handle, attribute: &str) -> Option<u32> {
    let NodeData::Element { attrs, .. } = &node.data else {
        return None;
    };
    let attrs = attrs.borrow();
    let value = attrs
        .iter()
        .find(|attr| attr.name.local.as_ref() == attribute)?
        .value
        .trim_start();
    let value = value.strip_prefix('+').unwrap_or(value);
    let end = value
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(value.len());
    value[..end]
        .parse()
        .ok()
        .filter(|number| *number <= MAX_NUMBER)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_html_list_structure_and_surrounding_text() {
        let html = "<div>Intro</div><ol start='7'><li><b>Cats</b> &amp; dogs<ul><li>one<br>two</li></ul></li><li value='12'><p>More</p><p>info</p></li><li>Last</li></ol><p>After</p>";
        assert_eq!(
            list(html, None).as_deref(),
            Some("Intro\n7. Cats & dogs\n  - one two\n12. More info\n13. Last\nAfter")
        );
    }

    #[test]
    fn a_partial_selection_keeps_list_context_and_omits_text_outside_it() {
        let html =
            "<p>Outside</p><ol start='3'><li>first</li><li>sélectionné</li><li>other</li></ol>";
        let item = "<li>sélectionné</li>";
        let start = html.find(item).unwrap();
        assert_eq!(
            list(html, Some(start..start + item.len())).as_deref(),
            Some("4. sélectionné")
        );
    }

    #[test]
    fn non_list_html_and_hidden_lists_fall_back_to_plain_text() {
        for html in [
            "<p>Only <b>style</b></p>",
            "<template><ul><li>hidden</li></ul></template>",
            "<ul><li><img src='https://example.test/pixel.png'></li></ul>",
        ] {
            assert_eq!(list(html, None), None);
        }
    }
}

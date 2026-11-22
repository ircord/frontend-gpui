//! The blocks a message body draws as: fenced code, `>` quotes, Markdown
//! lists and the text between. A port of the web client's
//! `shared/utils/blocks.ts`, `quotes.ts`, `lists.ts` and `splitCodeBlocks`.
//!
//! Quotes, lists and fences travel on IRC exactly as typed, so other clients
//! read the plain markers and every IRCord view draws them from the same
//! text. The app splits a message's text with formatting codes already
//! removed; each piece of text keeps, byte by byte, where it came from
//! ([`Mapped`]), so formatting runs, links and inline spans found on the whole
//! text still land on the right characters of a quote or a list item.

use std::ops::Range;

use super::Formatted;

/// Turn leading bullet glyphs from the clipboard into Markdown list markers.
/// Return `None` when the input should go through the normal paste path.
pub fn pasted_bullets(text: &str) -> Option<String> {
    let normalized = crate::features::plain_line_breaks(text);
    let mut converted = None::<String>;
    let mut offset = 0;
    for line in normalized.split_inclusive('\n') {
        let trimmed = line.trim_start_matches([' ', '\t']);
        let mut chars = trimmed.chars();
        let bullet = chars.next().is_some_and(|c| "•◦▪▫‣⁃●○■□".contains(c));
        let after = chars.as_str();
        let content = after.trim_start_matches([' ', '\t']);
        if bullet
            && after.len() != content.len()
            && content.starts_with(|c: char| !c.is_whitespace() && c != '\u{feff}')
        {
            let result = converted.get_or_insert_with(|| normalized[..offset].to_owned());
            result.push_str(&line[..line.len() - trimmed.len()]);
            result.push_str("- ");
            result.push_str(content);
        } else if let Some(result) = &mut converted {
            result.push_str(line);
        }
        offset += line.len();
    }
    converted
}

/// Text cut out of a longer source text, with each byte's offset in that
/// source. Quote markers and list indentation are left out, so the offsets
/// are increasing but not always consecutive.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Mapped {
    pub text: String,
    /// `at[i]` is the source offset of `text`'s byte `i`.
    pub at: Vec<usize>,
}

impl Mapped {
    pub fn new(text: &str) -> Self {
        Mapped {
            text: text.to_string(),
            at: (0..text.len()).collect(),
        }
    }

    pub fn as_str(&self) -> &str {
        &self.text
    }

    fn slice(&self, range: Range<usize>) -> Mapped {
        Mapped {
            text: self.text[range.clone()].to_string(),
            at: self.at[range].to_vec(),
        }
    }

    fn from(&self, start: usize) -> Mapped {
        self.slice(start..self.text.len())
    }

    fn push(&mut self, other: &Mapped) {
        self.text.push_str(&other.text);
        self.at.extend_from_slice(&other.at);
    }

    /// The lines, each with its line break except the last.
    fn lines(&self) -> Vec<Mapped> {
        let mut out = Vec::new();
        let mut start = 0;
        for (i, b) in self.text.bytes().enumerate() {
            if b == b'\n' {
                out.push(self.slice(start..i + 1));
                start = i + 1;
            }
        }
        out.push(self.from(start));
        out
    }

    fn strip_prefix_newline(&self) -> Mapped {
        if self.text.starts_with('\n') {
            self.from(1)
        } else {
            self.clone()
        }
    }

    fn strip_suffix_newline(&self) -> Mapped {
        if self.text.ends_with('\n') {
            self.slice(0..self.text.len() - 1)
        } else {
            self.clone()
        }
    }

    fn trim_start(&self) -> Mapped {
        let trimmed = self.text.trim_start();
        self.from(self.text.len() - trimmed.len())
    }

    /// Where the byte range `source` of the source text falls in this text:
    /// the bytes that came from inside it.
    pub fn project(&self, source: &Range<usize>) -> Option<Range<usize>> {
        let start = self.at.partition_point(|&a| a < source.start);
        let end = self.at.partition_point(|&a| a < source.end);
        (start < end).then_some(start..end)
    }

    /// The formatting runs and links of `source` that fall in this text, as
    /// a `Formatted` of its own.
    pub fn formatted(&self, source: &Formatted) -> Formatted {
        Formatted {
            text: self.text.clone(),
            runs: source
                .runs
                .iter()
                .filter_map(|(r, s)| Some((self.project(r)?, *s)))
                .collect(),
            links: source
                .links
                .iter()
                .filter_map(|(r, url)| {
                    let projected = self.project(r)?;
                    // A link cut by a quote marker is no longer that link.
                    (projected.len() == r.len()).then(|| (projected, url.clone()))
                })
                .collect(),
        }
    }
}

/// Lines joined back together: the last line's break belongs to whatever
/// comes after, so it is left out.
fn join(lines: &[Mapped]) -> Mapped {
    let mut out = Mapped::default();
    for line in lines {
        out.push(line);
    }
    match lines.last() {
        Some(last) if last.text.ends_with('\n') => out.strip_suffix_newline(),
        _ => out,
    }
}

fn content(line: &Mapped) -> &str {
    line.text.strip_suffix('\n').unwrap_or(&line.text)
}

fn blank<'a>(lines: impl IntoIterator<Item = &'a str>) -> bool {
    lines.into_iter().all(|l| l.trim().is_empty())
}

// ---- Fenced code ----------------------------------------------------------

/// A fenced code block, or the ordinary text between blocks. `raw` is the
/// exact source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Segment {
    Text {
        raw: Mapped,
    },
    Code {
        raw: Mapped,
        code: Mapped,
        lang: Option<String>,
    },
}

impl Segment {
    pub fn raw(&self) -> &Mapped {
        match self {
            Segment::Text { raw } | Segment::Code { raw, .. } => raw,
        }
    }
}

/// Splits out ``` fenced code blocks, as Discord and Markdown write them. A
/// fence opens at the start of a line, may name a language on its first
/// line, and closes at the next ```. An unterminated fence, or one with
/// nothing but blanks inside, stays ordinary text. Linear in the input.
pub fn split_code_blocks(text: &Mapped) -> Vec<Segment> {
    let s = text.as_str();
    if !s.contains("```") {
        return vec![Segment::Text { raw: text.clone() }];
    }
    let mut segments = Vec::new();
    let mut pending = 0;
    let mut at = 0;
    while let Some(found) = s[at..].find("```").map(|i| i + at) {
        at = found;
        if at > 0 && s.as_bytes()[at - 1] != b'\n' {
            at += 3;
            continue;
        }
        let Some(close) = s[at + 3..].find("```").map(|i| i + at + 3) else {
            break;
        };
        let mut code_start = at + 3;
        let mut code_end = close;
        let mut lang = None;
        if let Some((hint_len, name)) = language_hint(&s[code_start..code_end]) {
            lang = name;
            code_start += hint_len;
        }
        if s[code_start..code_end].ends_with('\n') {
            code_end -= 1;
        }
        if s[code_start..code_end].trim().is_empty() {
            at += 3;
            continue;
        }
        if at > pending {
            segments.push(Segment::Text {
                raw: text.slice(pending..at),
            });
        }
        segments.push(Segment::Code {
            raw: text.slice(at..close + 3),
            code: text.slice(code_start..code_end),
            lang,
        });
        at = close + 3;
        pending = at;
    }
    if pending < s.len() {
        segments.push(Segment::Text {
            raw: text.from(pending),
        });
    }
    segments
}

/// `^([\w+#.-]{1,32})?[ \t]*\n`: the length of a fence's first line when it
/// holds only a language name, and the name.
fn language_hint(code: &str) -> Option<(usize, Option<String>)> {
    let name_len = code
        .bytes()
        .take_while(|b| b.is_ascii_alphanumeric() || b"_+#.-".contains(b))
        .count();
    let name_len = if name_len > 32 { return None } else { name_len };
    let rest = &code[name_len..];
    let spaces = rest
        .bytes()
        .take_while(|b| *b == b' ' || *b == b'\t')
        .count();
    if rest.as_bytes().get(spaces) != Some(&b'\n') {
        return None;
    }
    let name = (name_len > 0).then(|| code[..name_len].to_string());
    Some((name_len + spaces + 1, name))
}

/// Text outside code (fenced blocks and IRC monospace), with formatting codes
/// removed, e.g. for finding links to preview. Takes the raw text.
pub fn text_outside_code(raw: &str) -> String {
    split_code_blocks(&Mapped::new(raw))
        .iter()
        .map(|segment| match segment {
            Segment::Code { .. } => " ".to_string(),
            Segment::Text { raw } => {
                let f = super::parse(raw.as_str());
                let mut out = String::new();
                let mut at = 0;
                for (r, style) in &f.runs {
                    if style.mono {
                        out.push_str(&f.text[at..r.start]);
                        out.push(' ');
                        at = r.end;
                    }
                }
                out.push_str(&f.text[at..]);
                out
            }
        })
        .collect()
}

// ---- Quotes ---------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuoteBlock {
    Text(Mapped),
    /// `rest` marks a `>>> ` quote, which runs to the end of the message.
    Quote {
        text: Mapped,
        rest: bool,
    },
}

/// Splits text into ordinary text and quotes. A line that starts with `> `
/// is quoted, and consecutive quoted lines form one quote. A line that
/// starts with `>>> ` quotes itself and every line after it. A quote of
/// blank lines only stays text, as typed, and a `>>> ` with nothing after it
/// on its line starts the quote on the next line.
pub fn split_quotes(text: &Mapped) -> Vec<QuoteBlock> {
    let s = text.as_str();
    let any =
        s.starts_with("> ") || s.starts_with(">>> ") || s.contains("\n> ") || s.contains("\n>>> ");
    if !any {
        return vec![QuoteBlock::Text(text.clone())];
    }
    let mut blocks = Vec::new();
    let mut lines: Vec<Mapped> = Vec::new();
    // The `> ` lines being collected: the whole line and the part after the marker.
    let mut quoted: Vec<(Mapped, Mapped)> = Vec::new();
    let end_quote = |blocks: &mut Vec<QuoteBlock>,
                     lines: &mut Vec<Mapped>,
                     quoted: &mut Vec<(Mapped, Mapped)>| {
        if quoted.is_empty() {
            return;
        }
        if blank(quoted.iter().map(|(_, q)| content(q))) {
            lines.extend(quoted.drain(..).map(|(line, _)| line));
            return;
        }
        if !lines.is_empty() {
            blocks.push(QuoteBlock::Text(join(lines)));
            lines.clear();
        }
        let inner: Vec<Mapped> = quoted.drain(..).map(|(_, q)| q).collect();
        blocks.push(QuoteBlock::Quote {
            text: join(&inner),
            rest: false,
        });
    };
    let all = text.lines();
    for (i, line) in all.iter().enumerate() {
        let c = content(line);
        if let Some(first) = c.strip_prefix(">>> ") {
            let after = &all[i + 1..];
            if !blank(std::iter::once(first).chain(after.iter().map(content))) {
                end_quote(&mut blocks, &mut lines, &mut quoted);
                if !lines.is_empty() {
                    blocks.push(QuoteBlock::Text(join(&lines)));
                }
                let mut inner = Vec::new();
                if !first.trim().is_empty() {
                    inner.push(line.from(4));
                }
                inner.extend(after.iter().cloned());
                blocks.push(QuoteBlock::Quote {
                    text: join(&inner),
                    rest: true,
                });
                return blocks;
            }
        }
        if c.starts_with("> ") {
            quoted.push((line.clone(), line.from(2)));
        } else {
            end_quote(&mut blocks, &mut lines, &mut quoted);
            lines.push(line.clone());
        }
    }
    end_quote(&mut blocks, &mut lines, &mut quoted);
    if !lines.is_empty() {
        blocks.push(QuoteBlock::Text(join(&lines)));
    }
    blocks
}

// ---- Lists ----------------------------------------------------------------

/// Deeper indentation than this nests no further, so hostile input stays shallow.
pub const MAX_LIST_DEPTH: usize = 5;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListItem {
    /// The item's text after its marker; continuation lines join with a break.
    pub text: Mapped,
    /// Lists indented under this item.
    pub children: Vec<List>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct List {
    pub ordered: bool,
    /// The first number of an ordered list.
    pub start: u64,
    pub items: Vec<ListItem>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ListBlock {
    Text(Mapped),
    List(List),
}

struct ItemLine {
    indent: usize,
    ordered: bool,
    number: u64,
    /// Where the item's text starts in the line.
    text_at: usize,
}

/// `^([ \t]*)([-*+]|(\d{1,9})[.)])[ \t]+(?=\S)(.*)$`
fn item_line(line: &str) -> Option<ItemLine> {
    let b = line.as_bytes();
    let lead = b.iter().take_while(|c| **c == b' ' || **c == b'\t').count();
    let mut i = lead;
    let (ordered, number) = match b.get(i)? {
        b'-' | b'*' | b'+' => {
            i += 1;
            (false, 1)
        }
        _ => {
            let digits = b[i..].iter().take_while(|c| c.is_ascii_digit()).count();
            if digits == 0 || digits > 9 || !matches!(b.get(i + digits), Some(b'.' | b')')) {
                return None;
            }
            let n = line[i..i + digits].parse().ok()?;
            i += digits + 1;
            (true, n)
        }
    };
    let spaces = b[i..]
        .iter()
        .take_while(|c| **c == b' ' || **c == b'\t')
        .count();
    if spaces == 0 {
        return None;
    }
    i += spaces;
    // The text must start with a non-space character.
    if line[i..].chars().next().is_none_or(char::is_whitespace) {
        return None;
    }
    Some(ItemLine {
        indent: indent_of(&line[..lead]),
        ordered,
        number,
        text_at: i,
    })
}

fn indent_of(space: &str) -> usize {
    space.chars().map(|c| if c == '\t' { 4 } else { 1 }).sum()
}

fn any_item(text: &str) -> bool {
    text.split('\n').any(|l| item_line(l).is_some())
}

/// Where a list item sits: the path of item indices from the outermost list.
type Path = Vec<(usize, usize)>;

/// Splits text into ordinary text and lists. A list item is a line that
/// starts (after optional indentation) with `-`, `*`, `+`, `1.` or `1)` and a
/// space, followed by text; deeper indentation nests under the item above. A
/// line indented past the list's own indentation continues the item above
/// it; any other line ends the list, and a change between bulleted and
/// numbered items at the same level starts a new list.
pub fn split_lists(text: &Mapped) -> Vec<ListBlock> {
    if !any_item(text.as_str()) {
        return vec![ListBlock::Text(text.clone())];
    }
    let mut blocks: Vec<ListBlock> = Vec::new();
    let mut lines: Vec<Mapped> = Vec::new();
    // The open lists, outermost first: their indentation and the list itself,
    // found again through the block index and the path of items above it.
    let mut stack: Vec<(usize, ListRef)> = Vec::new();
    // The break that ended the previous line, for joining continuation lines.
    let mut previous_break: Option<Mapped> = None;
    for line in text.lines() {
        let c = content(&line).to_string();
        let this_break = line
            .text
            .ends_with('\n')
            .then(|| line.from(line.text.len() - 1));
        match item_line(&c) {
            None => {
                let lead = c.len() - c.trim_start_matches([' ', '\t']).len();
                let indent = indent_of(&c[..lead]);
                if let Some((outer, _)) = stack.first()
                    && !c.trim().is_empty()
                    && indent > *outer
                {
                    while stack.len() > 1 && indent <= stack.last().unwrap().0 {
                        stack.pop();
                    }
                    let list = stack.last().unwrap().1.get(&mut blocks);
                    let item = list.items.last_mut().unwrap();
                    if let Some(br) = &previous_break {
                        item.text.push(br);
                    }
                    item.text.push(&line.slice(0..c.len()).trim_start());
                } else {
                    stack.clear();
                    lines.push(line.clone());
                }
            }
            Some(item) => {
                if !lines.is_empty() {
                    blocks.push(ListBlock::Text(join(&lines)));
                    lines.clear();
                }
                while stack.len() > 1 && item.indent < stack.last().unwrap().0 {
                    stack.pop();
                }
                let mut level = stack.last().map(|(indent, r)| (*indent, r.clone()));
                if let Some((indent, r)) = &level {
                    let ordered = r.get(&mut blocks).ordered;
                    if item.indent > *indent && stack.len() < MAX_LIST_DEPTH {
                        level = None;
                    } else if ordered != item.ordered {
                        stack.pop();
                        level = None;
                    }
                }
                let r = match level {
                    Some((_, r)) => r,
                    None => {
                        let list = List {
                            ordered: item.ordered,
                            start: item.number,
                            items: Vec::new(),
                        };
                        let r = match stack.last() {
                            Some((_, parent)) => {
                                let parent_list = parent.get(&mut blocks);
                                let index = parent_list.items.len() - 1;
                                let item = parent_list.items.last_mut().unwrap();
                                item.children.push(list);
                                let mut path = parent.path.clone();
                                path.push((index, item.children.len() - 1));
                                ListRef {
                                    block: parent.block,
                                    path,
                                }
                            }
                            None => {
                                blocks.push(ListBlock::List(list));
                                ListRef {
                                    block: blocks.len() - 1,
                                    path: Vec::new(),
                                }
                            }
                        };
                        stack.push((item.indent, r.clone()));
                        r
                    }
                };
                r.get(&mut blocks).items.push(ListItem {
                    text: line.slice(item.text_at..c.len()),
                    children: Vec::new(),
                });
            }
        }
        previous_break = this_break;
    }
    if !lines.is_empty() {
        blocks.push(ListBlock::Text(join(&lines)));
    }
    blocks
}

#[derive(Clone)]
struct ListRef {
    block: usize,
    path: Path,
}

impl ListRef {
    fn get<'a>(&self, blocks: &'a mut [ListBlock]) -> &'a mut List {
        let ListBlock::List(list) = &mut blocks[self.block] else {
            unreachable!("a list reference points at a list");
        };
        let mut list = list;
        for (item, child) in &self.path {
            list = &mut list.items[*item].children[*child];
        }
        list
    }
}

// ---- Blocks ---------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    Text(Mapped),
    Code { code: Mapped, lang: Option<String> },
    Quote(Vec<Block>),
    List(List),
}

/// Splits a message body into blocks. Fenced code comes first, so nothing
/// inside it is a quote or a list; then `> ` quotes, whose contents split
/// into code and lists again; then lists. A `>>> ` quote takes everything
/// after it, code blocks included. The line breaks around a block belong to
/// the block. With `continues_line` the first line follows other content (an
/// action's nick), so it starts no quote or list.
pub fn message_blocks(text: &Mapped, continues_line: bool) -> Vec<Block> {
    blocks_with(text, true, continues_line)
}

fn blocks_with(text: &Mapped, quotes: bool, continues_line: bool) -> Vec<Block> {
    if continues_line {
        let Some(end) = text.as_str().find('\n') else {
            return vec![Block::Text(text.clone())];
        };
        let head = text.slice(0..end + 1);
        let mut rest = blocks_with(&text.from(end + 1), quotes, false);
        if let Some(Block::Text(first)) = rest.first_mut() {
            let mut joined = head;
            joined.push(first);
            *first = joined;
            return rest;
        }
        rest.insert(0, Block::Text(head.strip_suffix_newline()));
        return rest;
    }
    let segments = split_code_blocks(text);
    let mut blocks = Vec::new();
    let mut index = 0;
    while index < segments.len() {
        let segment = &segments[index];
        let raw = match segment {
            Segment::Code { code, lang, .. } => {
                blocks.push(Block::Code {
                    code: code.clone(),
                    lang: lang.clone(),
                });
                index += 1;
                continue;
            }
            Segment::Text { raw } => raw,
        };
        let mut part = raw.clone();
        let next_is_code = matches!(segments.get(index + 1), Some(Segment::Code { .. }));
        if index > 0 && matches!(segments[index - 1], Segment::Code { .. }) {
            part = part.strip_prefix_newline();
        }
        if next_is_code {
            part = part.strip_suffix_newline();
        }
        if !quotes {
            blocks.extend(list_blocks(&part));
            index += 1;
            continue;
        }
        let mut consumed_rest = false;
        for piece in split_quotes(&part) {
            match piece {
                QuoteBlock::Text(t) => blocks.extend(list_blocks(&t)),
                QuoteBlock::Quote { text, rest } => {
                    let mut inner = text;
                    if rest && next_is_code {
                        // The rest of the message: the break trimmed above and every later segment.
                        if raw.text.ends_with('\n') {
                            inner.push(&raw.from(raw.text.len() - 1));
                        }
                        for later in &segments[index + 1..] {
                            inner.push(later.raw());
                        }
                        consumed_rest = true;
                    }
                    blocks.push(Block::Quote(blocks_with(&inner, false, false)));
                }
            }
        }
        index = if consumed_rest {
            segments.len()
        } else {
            index + 1
        };
    }
    blocks
}

fn list_blocks(text: &Mapped) -> Vec<Block> {
    split_lists(text)
        .into_iter()
        .map(|b| match b {
            ListBlock::Text(t) => Block::Text(t),
            ListBlock::List(l) => Block::List(l),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clipboard_bullets_keep_indentation_crlf_and_surrounding_text() {
        assert_eq!(
            pasted_bullets("Intro\r\n• first\r\n\t◦\t second\r\n■ third\r\nOutro\r\n").as_deref(),
            Some("Intro\n- first\n\t- second\n- third\nOutro\n")
        );
        assert_eq!(
            pasted_bullets("inline • dot\r\n•\t\r\n•word\r\n1. item"),
            None
        );
    }

    fn texts(blocks: &[Block]) -> Vec<String> {
        blocks
            .iter()
            .map(|b| match b {
                Block::Text(t) => format!("text:{}", t.text),
                Block::Code { code, lang } => {
                    format!(
                        "code{}:{}",
                        lang.as_deref()
                            .map(|l| format!("[{l}]"))
                            .unwrap_or_default(),
                        code.text
                    )
                }
                Block::Quote(inner) => format!("quote({})", texts(inner).join("|")),
                Block::List(l) => format!(
                    "list{}{}:{}",
                    if l.ordered { "#" } else { "-" },
                    l.start,
                    l.items
                        .iter()
                        .map(|i| {
                            let kids: Vec<String> = i
                                .children
                                .iter()
                                .map(|c| texts(&[Block::List(c.clone())]).join(""))
                                .collect();
                            if kids.is_empty() {
                                i.text.text.clone()
                            } else {
                                format!("{}[{}]", i.text.text, kids.join(","))
                            }
                        })
                        .collect::<Vec<_>>()
                        .join(";")
                ),
            })
            .collect()
    }

    fn blocks(text: &str) -> Vec<String> {
        texts(&message_blocks(&Mapped::new(text), false))
    }

    #[test]
    fn plain_text_is_one_block() {
        assert_eq!(blocks("hello\nworld"), ["text:hello\nworld"]);
    }

    #[test]
    fn fenced_code_with_and_without_language() {
        assert_eq!(
            blocks("look:\n```rust\nfn main() {}\n```\nnice"),
            ["text:look:", "code[rust]:fn main() {}", "text:nice"]
        );
        assert_eq!(blocks("```a b```"), ["code:a b"]);
        // Not at a line start, unterminated or blank: text.
        assert_eq!(blocks("x ```y```"), ["text:x ```y```"]);
        assert_eq!(blocks("```\nopen"), ["text:```\nopen"]);
        assert_eq!(blocks("```\n  \n```"), ["text:```\n  \n```"]);
    }

    #[test]
    fn quotes_single_and_rest() {
        assert_eq!(
            blocks("> one\n> two\nafter"),
            ["quote(text:one\ntwo)", "text:after"]
        );
        assert_eq!(
            blocks("before\n>>> all\nof this"),
            ["text:before", "quote(text:all\nof this)"]
        );
        // `>` without a space, or only blank lines quoted, stay text.
        assert_eq!(blocks(">no"), ["text:>no"]);
        assert_eq!(blocks("> \nx"), ["text:> \nx"]);
        // A `>>> ` quote takes the code after it.
        assert_eq!(
            blocks(">>> see\n```\ncode\n```\nend"),
            ["quote(text:see|code:code|text:end)"]
        );
        // Quotes hold lists, but not quotes.
        assert_eq!(blocks("> - a\n> - b"), ["quote(list-1:a;b)"]);
        assert_eq!(blocks("> > x"), ["quote(text:> x)"]);
    }

    #[test]
    fn lists_nest_and_continue() {
        assert_eq!(
            blocks("todo:\n- a\n  - a1\n- b\n  more\nend"),
            ["text:todo:", "list-1:a[list-1:a1];b\nmore", "text:end"]
        );
        assert_eq!(blocks("3. c\n4) d"), ["list#3:c;d"]);
        // Switching kind at the same level starts a new list.
        assert_eq!(blocks("- a\n1. b"), ["list-1:a", "list#1:b"]);
        // A marker needs a space and text after it.
        assert_eq!(blocks("-a\n- \n1.x"), ["text:-a\n- \n1.x"]);
        assert_eq!(blocks("1234567890. x"), ["text:1234567890. x"]);
    }

    #[test]
    fn list_depth_is_capped() {
        let text = (0..8)
            .map(|i| format!("{}- {i}", "  ".repeat(i)))
            .collect::<Vec<_>>()
            .join("\n");
        fn depth(list: &List) -> usize {
            1 + list
                .items
                .iter()
                .flat_map(|i| i.children.iter().map(depth))
                .max()
                .unwrap_or(0)
        }
        let b = message_blocks(&Mapped::new(&text), false);
        let Block::List(list) = &b[0] else { panic!() };
        assert_eq!(depth(list), MAX_LIST_DEPTH);
    }

    #[test]
    fn continues_line_starts_no_block() {
        let b = texts(&message_blocks(&Mapped::new("- waves\n- at you"), true));
        assert_eq!(b, ["text:- waves", "list-1:at you"]);
    }

    #[test]
    fn mapped_text_keeps_source_offsets() {
        let source = "> a **b**\n> c";
        let b = message_blocks(&Mapped::new(source), false);
        let Block::Quote(inner) = &b[0] else { panic!() };
        let Block::Text(t) = &inner[0] else { panic!() };
        assert_eq!(t.text, "a **b**\nc");
        // "c" came from the source's last byte.
        assert_eq!(t.at.last(), Some(&(source.len() - 1)));
        assert_eq!(t.project(&(4..9)), Some(2..7));
        assert_eq!(t.project(&(0..2)), None);
    }

    #[test]
    fn formatting_follows_into_list_items() {
        let f = super::super::parse("- \x02bold\x02 https://x.test/a");
        let b = message_blocks(&Mapped::new(&f.text), false);
        let Block::List(list) = &b[0] else { panic!() };
        let item = list.items[0].text.formatted(&f);
        assert_eq!(item.text, "bold https://x.test/a");
        assert_eq!(item.runs[0].0, 0..4);
        assert_eq!(item.links[0].0, 5..21);
    }

    /// The backend's `code-parity.json`: links outside code.
    #[test]
    fn backend_code_parity() {
        #[derive(serde::Deserialize)]
        struct Case {
            name: String,
            text: String,
            urls: Vec<String>,
        }
        let cases: Vec<Case> =
            serde_json::from_str(include_str!("testdata/code-parity.json")).unwrap();
        assert!(!cases.is_empty());
        for case in cases {
            let urls: Vec<String> = super::super::find_links(&text_outside_code(&case.text))
                .into_iter()
                .map(|(_, u)| u)
                .collect();
            assert_eq!(urls, case.urls, "{}", case.name);
        }
    }
}

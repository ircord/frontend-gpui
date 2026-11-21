//! Syntax highlighting for fenced code blocks: keywords, strings, comments,
//! numbers and literals for the languages the web client ships grammars
//! for. Only a fence's own language tag selects one; untagged or unknown
//! code stays plain, as in the web client (no guessing).
//!
//! This is a small tokenizer, not a grammar: it colours the common pieces
//! and never fails, which is what a chat message needs.

use std::ops::Range;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Keyword,
    String,
    Comment,
    Number,
    /// `true`, `false`, `null` and the like.
    Literal,
    /// A diff's added line.
    Added,
    /// A diff's removed line.
    Removed,
    /// A diff's hunk header, a YAML or INI key, a CSS selector.
    Meta,
}

struct Language {
    line_comments: &'static [&'static str],
    block_comment: Option<(&'static str, &'static str)>,
    quotes: &'static str,
    keywords: &'static [&'static str],
    literals: &'static [&'static str],
    /// SQL keywords are written in any case.
    any_case: bool,
}

const C_LIKE: &[&str] = &[
    "break", "case", "const", "continue", "default", "do", "else", "enum", "for", "goto", "if",
    "return", "sizeof", "static", "struct", "switch", "typedef", "union", "while", "void", "int",
    "char", "float", "double", "long", "short", "unsigned", "signed", "extern", "volatile",
    "inline", "auto", "register", "bool",
];

const CPP: &[&str] = &[
    "break",
    "case",
    "catch",
    "class",
    "const",
    "constexpr",
    "continue",
    "default",
    "delete",
    "do",
    "else",
    "enum",
    "explicit",
    "for",
    "friend",
    "if",
    "inline",
    "namespace",
    "new",
    "operator",
    "private",
    "protected",
    "public",
    "return",
    "sizeof",
    "static",
    "struct",
    "switch",
    "template",
    "this",
    "throw",
    "try",
    "typedef",
    "typename",
    "union",
    "using",
    "virtual",
    "void",
    "while",
    "int",
    "char",
    "float",
    "double",
    "long",
    "bool",
    "auto",
    "override",
    "final",
    "noexcept",
];

const RUST: &[&str] = &[
    "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern",
    "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub", "ref",
    "return", "self", "Self", "static", "struct", "super", "trait", "type", "unsafe", "use",
    "where", "while", "Some", "None", "Ok", "Err",
];

const JS: &[&str] = &[
    "async",
    "await",
    "break",
    "case",
    "catch",
    "class",
    "const",
    "continue",
    "debugger",
    "default",
    "delete",
    "do",
    "else",
    "export",
    "extends",
    "finally",
    "for",
    "from",
    "function",
    "if",
    "import",
    "in",
    "instanceof",
    "let",
    "new",
    "of",
    "return",
    "static",
    "super",
    "switch",
    "this",
    "throw",
    "try",
    "typeof",
    "var",
    "void",
    "while",
    "yield",
    "interface",
    "type",
    "enum",
    "implements",
    "private",
    "protected",
    "public",
    "readonly",
    "as",
];

const PYTHON: &[&str] = &[
    "and", "as", "assert", "async", "await", "break", "class", "continue", "def", "del", "elif",
    "else", "except", "finally", "for", "from", "global", "if", "import", "in", "is", "lambda",
    "nonlocal", "not", "or", "pass", "raise", "return", "try", "while", "with", "yield", "self",
];

const GO: &[&str] = &[
    "break",
    "case",
    "chan",
    "const",
    "continue",
    "default",
    "defer",
    "else",
    "fallthrough",
    "for",
    "func",
    "go",
    "goto",
    "if",
    "import",
    "interface",
    "map",
    "package",
    "range",
    "return",
    "select",
    "struct",
    "switch",
    "type",
    "var",
];

const JAVA: &[&str] = &[
    "abstract",
    "break",
    "case",
    "catch",
    "class",
    "const",
    "continue",
    "default",
    "do",
    "else",
    "enum",
    "extends",
    "final",
    "finally",
    "for",
    "if",
    "implements",
    "import",
    "instanceof",
    "interface",
    "new",
    "package",
    "private",
    "protected",
    "public",
    "return",
    "static",
    "super",
    "switch",
    "synchronized",
    "this",
    "throw",
    "throws",
    "try",
    "void",
    "while",
    "int",
    "long",
    "boolean",
    "char",
    "double",
    "float",
    "var",
    "val",
    "fun",
    "object",
    "when",
    "is",
    "in",
    "override",
    "data",
    "sealed",
    "namespace",
    "using",
    "struct",
    "string",
    "async",
    "await",
    "func",
    "let",
    "guard",
    "protocol",
    "extension",
    "import",
    "self",
    "init",
];

const RUBY: &[&str] = &[
    "alias", "and", "begin", "break", "case", "class", "def", "do", "else", "elsif", "end",
    "ensure", "for", "if", "in", "module", "next", "not", "or", "redo", "rescue", "retry",
    "return", "self", "super", "then", "unless", "until", "when", "while", "yield", "require",
];

const LUA: &[&str] = &[
    "and", "break", "do", "else", "elseif", "end", "for", "function", "goto", "if", "in", "local",
    "not", "or", "repeat", "return", "then", "until", "while",
];

const PHP: &[&str] = &[
    "abstract",
    "as",
    "break",
    "case",
    "catch",
    "class",
    "const",
    "continue",
    "default",
    "do",
    "echo",
    "else",
    "elseif",
    "extends",
    "final",
    "finally",
    "fn",
    "for",
    "foreach",
    "function",
    "global",
    "if",
    "implements",
    "interface",
    "namespace",
    "new",
    "private",
    "protected",
    "public",
    "return",
    "static",
    "switch",
    "throw",
    "trait",
    "try",
    "use",
    "while",
];

const SHELL: &[&str] = &[
    "if", "then", "else", "elif", "fi", "case", "esac", "for", "while", "until", "do", "done",
    "in", "function", "return", "local", "export", "echo", "exit", "set", "unset", "source",
];

const SQL: &[&str] = &[
    "select",
    "from",
    "where",
    "and",
    "or",
    "not",
    "insert",
    "into",
    "values",
    "update",
    "set",
    "delete",
    "create",
    "table",
    "drop",
    "alter",
    "index",
    "join",
    "left",
    "right",
    "inner",
    "outer",
    "on",
    "group",
    "by",
    "order",
    "having",
    "limit",
    "offset",
    "as",
    "distinct",
    "union",
    "all",
    "primary",
    "key",
    "foreign",
    "references",
    "default",
    "is",
    "in",
    "like",
    "between",
    "case",
    "when",
    "then",
    "else",
    "end",
    "with",
    "returning",
];

const DOCKER: &[&str] = &[
    "FROM",
    "RUN",
    "CMD",
    "LABEL",
    "EXPOSE",
    "ENV",
    "ADD",
    "COPY",
    "ENTRYPOINT",
    "VOLUME",
    "USER",
    "WORKDIR",
    "ARG",
    "ONBUILD",
    "STOPSIGNAL",
    "HEALTHCHECK",
    "SHELL",
    "AS",
];

const COMMON_LITERALS: &[&str] = &[
    "true",
    "false",
    "null",
    "nil",
    "None",
    "True",
    "False",
    "undefined",
    "NULL",
    "nullptr",
];

fn language(name: &str) -> Option<Language> {
    let c_comments = (&["//"][..], Some(("/*", "*/")));
    let lang = |(line_comments, block_comment): (
        &'static [&'static str],
        Option<(&'static str, &'static str)>,
    ),
                quotes: &'static str,
                keywords: &'static [&'static str]| Language {
        line_comments,
        block_comment,
        quotes,
        keywords,
        literals: COMMON_LITERALS,
        any_case: false,
    };
    Some(match name.to_ascii_lowercase().as_str() {
        "rust" | "rs" => lang(c_comments, "\"", RUST),
        "js" | "javascript" | "jsx" | "mjs" | "ts" | "typescript" | "tsx" => {
            lang(c_comments, "\"'`", JS)
        }
        "c" | "h" => lang(c_comments, "\"'", C_LIKE),
        "cpp" | "c++" | "cc" | "hpp" | "cxx" => lang(c_comments, "\"'", CPP),
        "go" | "golang" => lang(c_comments, "\"'`", GO),
        "java" | "kotlin" | "kt" | "cs" | "csharp" | "c#" | "swift" => {
            lang(c_comments, "\"'", JAVA)
        }
        "php" => lang((&["//", "#"], Some(("/*", "*/"))), "\"'", PHP),
        "python" | "py" => lang((&["#"], None), "\"'", PYTHON),
        "ruby" | "rb" => lang((&["#"], None), "\"'", RUBY),
        "lua" => lang((&["--"], Some(("--[[", "]]"))), "\"'", LUA),
        "bash" | "sh" | "shell" | "zsh" | "console" => lang((&["#"], None), "\"'", SHELL),
        "sql" => Language {
            any_case: true,
            ..lang((&["--"], Some(("/*", "*/"))), "'\"", SQL)
        },
        "json" => lang((&[], None), "\"", &[]),
        "yaml" | "yml" | "toml" | "ini" => lang((&["#", ";"], None), "\"'", &[]),
        "css" | "scss" => lang(c_comments, "\"'", &[]),
        "html" | "xml" | "svg" | "markdown" | "md" => lang((&[], Some(("<!--", "-->"))), "\"", &[]),
        "dockerfile" | "docker" => lang((&["#"], None), "\"'", DOCKER),
        "makefile" | "make" => lang((&["#"], None), "\"'", &[]),
        "diff" | "patch" => return Some(DIFF),
        _ => return None,
    })
}

const DIFF: Language = Language {
    line_comments: &[],
    block_comment: None,
    quotes: "",
    keywords: &[],
    literals: &[],
    any_case: false,
};

/// Coloured ranges of `code` for a fence named `lang`; empty when the
/// language is unknown.
pub fn highlight(code: &str, lang: Option<&str>) -> Vec<(Range<usize>, Kind)> {
    let Some(name) = lang else {
        return Vec::new();
    };
    let lower = name.to_ascii_lowercase();
    if matches!(lower.as_str(), "diff" | "patch") {
        return diff(code);
    }
    let Some(language) = language(name) else {
        return Vec::new();
    };
    let keyed = matches!(lower.as_str(), "yaml" | "yml" | "toml" | "ini");
    let mut out = Vec::new();
    let b = code.as_bytes();
    let mut i = 0;
    let mut line_start = true;
    while i < b.len() {
        let rest = &code[i..];
        if keyed && line_start {
            let line = &rest[..rest.find('\n').unwrap_or(rest.len())];
            let lead = line.len() - line.trim_start().len();
            if let Some(sep) = line.find([':', '=']).filter(|s| *s > lead)
                && !line[lead..sep].contains(['"', '\'', '#'])
            {
                out.push((
                    i + lead..i + lead + line[lead..sep].trim_end().len(),
                    Kind::Meta,
                ));
            }
        }
        line_start = false;
        if let Some((open, close)) = language.block_comment
            && rest.starts_with(open)
        {
            let end = rest[open.len()..]
                .find(close)
                .map_or(rest.len(), |e| e + open.len() + close.len());
            out.push((i..i + end, Kind::Comment));
            line_start = rest[..end].ends_with('\n');
            i += end;
            continue;
        }
        if language.line_comments.iter().any(|p| rest.starts_with(p)) {
            let end = rest.find('\n').unwrap_or(rest.len());
            out.push((i..i + end, Kind::Comment));
            i += end;
            continue;
        }
        let c = rest.chars().next().unwrap();
        if language.quotes.contains(c) {
            let mut j = c.len_utf8();
            let mut escaped = false;
            for ch in rest[j..].chars() {
                j += ch.len_utf8();
                if escaped {
                    escaped = false;
                } else if ch == '\\' {
                    escaped = true;
                } else if ch == c || (ch == '\n' && c != '`') {
                    break;
                }
            }
            out.push((i..i + j, Kind::String));
            i += j;
            continue;
        }
        if c.is_ascii_digit() {
            let prev_word = code[..i].chars().next_back().is_some_and(is_ident);
            let len = rest
                .find(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '.' || ch == '_'))
                .unwrap_or(rest.len());
            if !prev_word {
                out.push((i..i + len, Kind::Number));
            }
            i += len;
            continue;
        }
        if is_ident(c) {
            let len = rest.find(|ch: char| !is_ident(ch)).unwrap_or(rest.len());
            let word = &rest[..len];
            let keyword = if language.any_case {
                language
                    .keywords
                    .iter()
                    .any(|k| k.eq_ignore_ascii_case(word))
            } else {
                language.keywords.contains(&word)
            };
            if keyword {
                out.push((i..i + len, Kind::Keyword));
            } else if language.literals.contains(&word) {
                out.push((i..i + len, Kind::Literal));
            }
            i += len;
            continue;
        }
        line_start = c == '\n';
        i += c.len_utf8();
    }
    out
}

fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '$'
}

fn diff(code: &str) -> Vec<(Range<usize>, Kind)> {
    let mut out = Vec::new();
    let mut at = 0;
    for line in code.split_inclusive('\n') {
        let text = line.trim_end_matches('\n');
        let kind = if text.starts_with("+++") || text.starts_with("---") || text.starts_with("@@") {
            Some(Kind::Meta)
        } else if text.starts_with('+') {
            Some(Kind::Added)
        } else if text.starts_with('-') {
            Some(Kind::Removed)
        } else {
            None
        };
        if let Some(kind) = kind
            && !text.is_empty()
        {
            out.push((at..at + text.len(), kind));
        }
        at += line.len();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds<'a>(code: &'a str, lang: &str) -> Vec<(&'a str, Kind)> {
        highlight(code, Some(lang))
            .into_iter()
            .map(|(r, k)| (&code[r], k))
            .collect()
    }

    #[test]
    fn rust_pieces() {
        assert_eq!(
            kinds("fn main() { let x = \"hi\"; // done\n 42 }", "rust"),
            [
                ("fn", Kind::Keyword),
                ("let", Kind::Keyword),
                ("\"hi\"", Kind::String),
                ("// done", Kind::Comment),
                ("42", Kind::Number),
            ]
        );
    }

    #[test]
    fn python_sql_and_diff() {
        assert_eq!(
            kinds("def f(): return None # x", "py"),
            [
                ("def", Kind::Keyword),
                ("return", Kind::Keyword),
                ("None", Kind::Literal),
                ("# x", Kind::Comment)
            ]
        );
        assert_eq!(
            kinds("SELECT a FROM t", "sql"),
            [("SELECT", Kind::Keyword), ("FROM", Kind::Keyword)]
        );
        assert_eq!(
            kinds("@@ -1 +1 @@\n-old\n+new\n same", "diff"),
            [
                ("@@ -1 +1 @@", Kind::Meta),
                ("-old", Kind::Removed),
                ("+new", Kind::Added)
            ]
        );
    }

    #[test]
    fn unknown_or_missing_language_is_plain() {
        assert!(highlight("fn x", None).is_empty());
        assert!(highlight("fn x", Some("klingon")).is_empty());
        // Unterminated strings and comments never panic.
        assert_eq!(kinds("\"open", "js"), [("\"open", Kind::String)]);
        assert_eq!(kinds("/* open", "c"), [("/* open", Kind::Comment)]);
        assert_eq!(
            kinds("name: x\nkey = 1", "yaml"),
            [
                ("name", Kind::Meta),
                ("key", Kind::Meta),
                ("1", Kind::Number)
            ]
        );
    }
}

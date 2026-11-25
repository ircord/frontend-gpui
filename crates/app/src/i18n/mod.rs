//! Interface text. Every string a person can read goes through [`t!`]:
//!
//! ```ignore
//! div().child(t!("sidebar-sign-out"))                       // &'static str
//! div().child(t!("core-connecting-to", network = name)) // String
//! ```
//!
//! The text lives in catalog files, `crates/app/i18n/<language>/<area>.ftl`,
//! one per work package so packages never edit the same catalog. The syntax
//! is the single-line subset of [Fluent](https://projectfluent.org): `id = text`,
//! `#` comments, and `{ $name }` placeholders. IDs are lowercase words
//! joined by `-` and start with their file's name (`message_actions.ftl`
//! holds `message-actions-…`), so two files never define the same ID.
//!
//! English is the only language today; [`CATALOGS`] lists its files. A
//! missing ID shows the ID itself and logs once; the `i18n` unit tests fail
//! the build first, by scanning the sources for every `t!` ID.

use std::collections::HashMap;
use std::fmt::Display;
use std::sync::OnceLock;

/// The English catalog, one file per area. Written once by the
/// foundations; a package adds text only to its own file.
pub const CATALOGS: &[(&str, &str)] = &[
    ("core", include_str!("../../i18n/en/core.ftl")),
    ("text", include_str!("../../i18n/en/text.ftl")),
    (
        "message_actions",
        include_str!("../../i18n/en/message_actions.ftl"),
    ),
    ("timeline", include_str!("../../i18n/en/timeline.ftl")),
    ("sidebar", include_str!("../../i18n/en/sidebar.ftl")),
    ("composer", include_str!("../../i18n/en/composer.ftl")),
    ("commands", include_str!("../../i18n/en/commands.ftl")),
    ("uploads", include_str!("../../i18n/en/uploads.ftl")),
    ("settings", include_str!("../../i18n/en/settings.ftl")),
    ("networks", include_str!("../../i18n/en/networks.ftl")),
    (
        "server_window",
        include_str!("../../i18n/en/server_window.ftl"),
    ),
    ("people", include_str!("../../i18n/en/people.ftl")),
    ("search", include_str!("../../i18n/en/search.ftl")),
    ("inbox_saved", include_str!("../../i18n/en/inbox_saved.ftl")),
    ("channels", include_str!("../../i18n/en/channels.ftl")),
    (
        "link_previews",
        include_str!("../../i18n/en/link_previews.ftl"),
    ),
    ("emoji", include_str!("../../i18n/en/emoji.ftl")),
    ("drafts", include_str!("../../i18n/en/drafts.ftl")),
    (
        "channel_settings",
        include_str!("../../i18n/en/channel_settings.ftl"),
    ),
    ("parties", include_str!("../../i18n/en/parties.ftl")),
    ("appearance", include_str!("../../i18n/en/appearance.ftl")),
    (
        "settings_messages",
        include_str!("../../i18n/en/settings_messages.ftl"),
    ),
    (
        "notifications",
        include_str!("../../i18n/en/notifications.ftl"),
    ),
    ("profile", include_str!("../../i18n/en/profile.ftl")),
    ("admin", include_str!("../../i18n/en/admin.ftl")),
    ("desktop", include_str!("../../i18n/en/desktop.ftl")),
    ("links_view", include_str!("../../i18n/en/links_view.ftl")),
    ("emotes", include_str!("../../i18n/en/emotes.ftl")),
    ("audit", include_str!("../../i18n/en/audit.ftl")),
    ("media", include_str!("../../i18n/en/media.ftl")),
    ("audio_card", include_str!("../../i18n/en/audio_card.ftl")),
    ("video", include_str!("../../i18n/en/video.ftl")),
    ("updates", include_str!("../../i18n/en/updates.ftl")),
    ("speech", include_str!("../../i18n/en/speech.ftl")),
];

/// Look up text by ID, with `{ $name }` placeholders filled in:
/// `t!("id")` is a `&'static str`, `t!("id", name = value, …)` a `String`.
#[macro_export]
macro_rules! t {
    ($id:literal) => {
        $crate::i18n::text($id)
    };
    ($id:literal, $($name:ident = $value:expr),+ $(,)?) => {
        $crate::i18n::format(
            $id,
            &[$((stringify!($name), &$value as &dyn ::std::fmt::Display)),+],
        )
    };
}

/// A catalog file that does not parse.
#[derive(Debug, PartialEq)]
pub struct ParseError {
    pub line: usize,
    pub reason: &'static str,
}

/// Parse one catalog file into `(id, text)` pairs, in order.
pub fn parse(source: &'static str) -> Result<Vec<(&'static str, &'static str)>, ParseError> {
    let mut out = Vec::new();
    for (i, raw) in source.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let error = |reason| ParseError {
            line: i + 1,
            reason,
        };
        let (id, text) = line.split_once('=').ok_or(error("expected `id = text`"))?;
        let id = id.trim();
        if id.is_empty()
            || !id
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        {
            return Err(error("IDs are lowercase words joined by `-`"));
        }
        out.push((id, text.trim()));
    }
    Ok(out)
}

fn catalog() -> &'static HashMap<&'static str, &'static str> {
    static CATALOG: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
    CATALOG.get_or_init(|| {
        let mut map = HashMap::new();
        for (area, source) in CATALOGS {
            match parse(source) {
                Ok(entries) => map.extend(entries),
                Err(e) => log::error!("catalog {area}, line {}: {}", e.line, e.reason),
            }
        }
        map
    })
}

/// The text for `id`, or the ID itself when the catalog lacks it.
pub fn text(id: &'static str) -> &'static str {
    match catalog().get(id) {
        Some(text) => text,
        None => {
            log::warn!("no text for {id}");
            id
        }
    }
}

/// The text for `id` with each `{ $name }` replaced by its value.
pub fn format(id: &'static str, args: &[(&str, &dyn Display)]) -> String {
    fill(text(id), args)
}

fn fill(template: &str, args: &[(&str, &dyn Display)]) -> String {
    let mut out = String::with_capacity(template.len() + 16);
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let Some(end) = rest[start..].find('}') else {
            break;
        };
        let name = rest[start + 1..start + end].trim().trim_start_matches('$');
        match args.iter().find(|(n, _)| *n == name) {
            Some((_, value)) => out.push_str(&value.to_string()),
            None => out.push_str(&rest[start..start + end + 1]),
        }
        rest = &rest[start + end + 1..];
    }
    out.push_str(rest);
    out
}

/// The `{ $name }` placeholders a text uses.
pub fn placeholders(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find('{') {
        let Some(end) = rest[start..].find('}') else {
            break;
        };
        out.push(rest[start + 1..start + end].trim().trim_start_matches('$'));
        rest = &rest[start + end + 1..];
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, BTreeSet};
    use std::path::Path;

    #[test]
    fn fills_placeholders() {
        assert_eq!(
            fill("{ $a } and {$b}, { $c }", &[("a", &1), ("b", &"two")]),
            "1 and two, { $c }"
        );
        assert_eq!(placeholders("x { $a } y {$b}"), ["a", "b"]);
    }

    #[test]
    fn rejects_bad_lines() {
        assert_eq!(
            parse("# note\n\nok-id = Fine").unwrap(),
            [("ok-id", "Fine")]
        );
        assert_eq!(parse("Bad.Id = x").unwrap_err().line, 1);
        assert_eq!(parse("a = b\nno equals").unwrap_err().line, 2);
    }

    /// Every `t!` use in the sources, as (id, placeholders, file).
    fn uses() -> Vec<(String, BTreeSet<String>, String)> {
        fn walk(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
            for entry in std::fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    walk(&path, out);
                } else if path.extension().is_some_and(|e| e == "rs")
                    && !path.ends_with("i18n/mod.rs")
                {
                    out.push(path);
                }
            }
        }
        let mut files = Vec::new();
        walk(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
            &mut files,
        );
        let mut out = Vec::new();
        for file in files {
            // Comments may show `t!` without a real ID.
            let source: String = std::fs::read_to_string(&file)
                .unwrap()
                .lines()
                .filter(|l| !l.trim_start().starts_with("//"))
                .collect::<Vec<_>>()
                .join("\n");
            let mut rest = source.as_str();
            while let Some(at) = rest.find("t!(") {
                // Not the end of another macro's name, such as `format!(`.
                let word = rest[..at]
                    .chars()
                    .next_back()
                    .is_some_and(|c| c.is_alphanumeric() || c == '_');
                let after = rest[at + 3..].trim_start();
                if word {
                    rest = &rest[at + 3..];
                    continue;
                }
                let Some(after) = after.strip_prefix('"') else {
                    rest = &rest[at + 3..];
                    continue;
                };
                let Some(end) = after.find('"') else { break };
                let id = after[..end].to_string();
                out.push((id, arg_names(&after[end + 1..]), file.display().to_string()));
                rest = &after[end..];
            }
        }
        out
    }

    /// The `name = value` argument names after a `t!` ID, up to its closing
    /// parenthesis.
    fn arg_names(rest: &str) -> BTreeSet<String> {
        let mut names = BTreeSet::new();
        let (mut depth, mut quoted, mut arg) = (1, false, String::new());
        for ch in rest.chars() {
            match ch {
                '"' => quoted = !quoted,
                _ if quoted => {}
                '(' | '[' | '{' => depth += 1,
                ')' | ']' | '}' => depth -= 1,
                ',' if depth == 1 => {
                    names.extend(name_of(&arg));
                    arg.clear();
                    continue;
                }
                _ => {}
            }
            if depth == 0 {
                break;
            }
            arg.push(ch);
        }
        names.extend(name_of(&arg));
        names
    }

    fn name_of(arg: &str) -> Option<String> {
        let (name, _) = arg.split_once('=')?;
        let name = name.trim();
        (!name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '_'))
            .then(|| name.to_string())
    }

    #[test]
    fn reads_argument_names() {
        let names = arg_names(", seconds = ms.div_ceil(1000), reason = f(a, b))");
        assert_eq!(names, BTreeSet::from(["seconds".into(), "reason".into()]));
        assert!(arg_names(")").is_empty());
    }

    #[test]
    fn every_catalog_parses_without_duplicates() {
        let mut seen = BTreeMap::new();
        for (area, source) in CATALOGS {
            let prefix = format!("{}-", area.replace('_', "-"));
            for (id, _) in parse(source).unwrap_or_else(|e| panic!("{area}: {e:?}")) {
                assert!(
                    id.starts_with(&prefix),
                    "{id} in {area}.ftl must start with {prefix}"
                );
                if let Some(other) = seen.insert(id, area) {
                    panic!("{id} is in both {other} and {area}");
                }
            }
        }
    }

    #[test]
    fn every_id_in_the_sources_has_text_with_matching_placeholders() {
        let catalog = catalog();
        let mut used = BTreeSet::new();
        for (id, names, file) in uses() {
            let text = catalog
                .get(id.as_str())
                .unwrap_or_else(|| panic!("{file}: no text for {id}"));
            let wanted: BTreeSet<String> =
                placeholders(text).into_iter().map(str::to_string).collect();
            assert_eq!(wanted, names, "{file}: {id} takes {wanted:?}");
            used.insert(id);
        }
        let unused: Vec<_> = catalog.keys().filter(|id| !used.contains(**id)).collect();
        assert!(unused.is_empty(), "text nothing uses: {unused:?}");
    }
}

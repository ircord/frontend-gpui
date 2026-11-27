//! The people Ircoid's credits screen names: `contributors.txt`, written from
//! git history by `cargo xtask credits` while preparing a release and
//! compiled in. Nothing is fetched or worked out at runtime.

/// The list as `cargo xtask credits` wrote it.
const FILE: &str = include_str!("contributors.txt");

/// Up to this many names fit one column...
pub const ONE_COLUMN: usize = 5;
/// ...and with more, two columns hold this many names a page.
pub const PAGE: usize = 10;

/// The contributors, in the order of their first commit.
pub fn contributors() -> Vec<&'static str> {
    FILE.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .collect()
}

/// How many pages `count` names take.
pub fn pages(count: usize) -> usize {
    if count <= ONE_COLUMN {
        1
    } else {
        count.div_ceil(PAGE)
    }
}

/// Page `n` (from 0) of `names`.
pub fn page<'a>(names: &'a [&'static str], n: usize) -> &'a [&'static str] {
    if names.len() <= ONE_COLUMN {
        return names;
    }
    let start = (n * PAGE).min(names.len());
    &names[start..(start + PAGE).min(names.len())]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_list_has_one_clean_entry_per_person() {
        let names = contributors();
        assert!(!names.is_empty());
        for (i, name) in names.iter().enumerate() {
            assert!(!name.is_empty() && !name.starts_with('#'));
            assert!(
                names[..i]
                    .iter()
                    .all(|other| other.to_lowercase() != name.to_lowercase()),
                "{name} twice"
            );
        }
    }

    #[test]
    fn names_fill_pages() {
        assert_eq!(pages(0), 1);
        assert_eq!(pages(1), 1);
        assert_eq!(pages(5), 1);
        assert_eq!(pages(6), 1);
        assert_eq!(pages(10), 1);
        assert_eq!(pages(11), 2);
        assert_eq!(pages(21), 3);
        let names: Vec<&'static str> = (0..13)
            .map(|n| &*Box::leak(format!("n{n}").into_boxed_str()))
            .collect();
        assert_eq!(page(&names, 0).len(), 10);
        assert_eq!(page(&names, 1).len(), 3);
        assert_eq!(page(&names[..3], 0).len(), 3);
    }
}

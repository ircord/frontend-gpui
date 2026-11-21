//! `/ignore` and `/unignore`, from the web client's
//! `shared/utils/ignore-command.ts`: they edit the ignore list in the
//! notification settings, the same list the user menu and settings edit.

/// Most entries the server keeps.
pub const MAX_ENTRIES: usize = 200;
/// The longest entry (nick or mask) the server keeps.
pub const MAX_ENTRY: usize = 120;

/// Who a nick or mask is checked against: their IRC identity when known.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Identity {
    pub nick: String,
    pub user: Option<String>,
    pub host: Option<String>,
}

/// What the command says, for the app to word.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Notice {
    Usage { command: &'static str },
    List(Vec<String>),
    Empty,
    TooLong,
    AlreadyIgnoring(String),
    Yourself,
    HiddenBy { target: String, masks: Vec<String> },
    Maximum,
    Added(String),
    Removed(String),
    Covered { target: String, masks: Vec<String> },
    NotIgnoring(String),
}

/// The outcome: a new list when it changes, and what to tell the person.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub ignore: Option<Vec<String>>,
    pub notice: Notice,
}

fn said(notice: Notice) -> Outcome {
    Outcome {
        ignore: None,
        notice,
    }
}

/// `nick`, `user@host`, `nick!user` or `nick!user@host` as a full mask;
/// missing parts match anything.
pub fn expand_mask(value: &str) -> String {
    let v = value.trim();
    let (left, host) = match v.rfind('@') {
        Some(at) => (&v[..at], &v[at + 1..]),
        None => (v, ""),
    };
    let at = v.contains('@');
    let (nick, user) = match left.find('!') {
        Some(bang) => (&left[..bang], &left[bang + 1..]),
        // `bob@host` names a user, not a nick.
        None if at => ("", left),
        None => (left, ""),
    };
    let or_any = |s: &str| {
        if s.is_empty() {
            "*".to_string()
        } else {
            s.to_string()
        }
    };
    format!("{}!{}@{}", or_any(nick), or_any(user), or_any(host))
}

/// `*` and `?` wildcards, case-insensitive.
fn wildcard(pattern: &[char], text: &[char]) -> bool {
    match pattern.split_first() {
        None => text.is_empty(),
        Some(('*', rest)) => (0..=text.len()).any(|i| wildcard(rest, &text[i..])),
        Some((p, rest)) => text
            .split_first()
            .is_some_and(|(t, more)| (*p == '?' || p == t) && wildcard(rest, more)),
    }
}

/// Whether an ignore entry hides this person.
pub fn mask_matches(mask: &str, who: &Identity) -> bool {
    let pattern: Vec<char> = expand_mask(mask).to_lowercase().chars().collect();
    let identity = format!(
        "{}!{}@{}",
        who.nick,
        who.user.as_deref().unwrap_or("*"),
        who.host.as_deref().unwrap_or("*")
    );
    let identity: Vec<char> = identity.to_lowercase().chars().collect();
    wildcard(&pattern, &identity)
}

fn has(list: &[String], entry: &str) -> bool {
    list.iter().any(|m| m.eq_ignore_ascii_case(entry))
}

fn covering(list: &[String], who: &Identity) -> Vec<String> {
    list.iter()
        .filter(|m| mask_matches(m, who))
        .cloned()
        .collect()
}

/// Run `/ignore [arg]` or `/unignore arg` against `list`. `person` is the
/// target's identity when they are known, so masks that already hide them
/// are found; `me` keeps you from hiding your own messages.
pub fn run(
    unignore: bool,
    arg: &str,
    list: &[String],
    person: Option<Identity>,
    me: Option<Identity>,
) -> Outcome {
    let command = if unignore { "unignore" } else { "ignore" };
    let target = arg.trim();
    let target = target.strip_prefix('@').unwrap_or(target);
    if target.is_empty() {
        return said(match (unignore, list.is_empty()) {
            (true, _) => Notice::Usage { command },
            (false, true) => Notice::Empty,
            (false, false) => Notice::List(list.to_vec()),
        });
    }
    if target.contains(char::is_whitespace) || target.contains('\0') {
        return said(Notice::Usage { command });
    }
    if target.chars().count() > MAX_ENTRY {
        return said(Notice::TooLong);
    }
    let person = person.unwrap_or_else(|| Identity {
        nick: target.to_string(),
        ..Default::default()
    });
    let target = target.to_string();
    if !unignore {
        if has(list, &target) {
            return said(Notice::AlreadyIgnoring(target));
        }
        if me.is_some_and(|me| mask_matches(&target, &me)) {
            return said(Notice::Yourself);
        }
        let by = covering(list, &person);
        if !by.is_empty() {
            return said(Notice::HiddenBy { target, masks: by });
        }
        if list.len() >= MAX_ENTRIES {
            return said(Notice::Maximum);
        }
        let mut next = list.to_vec();
        next.push(target.clone());
        return Outcome {
            ignore: Some(next),
            notice: Notice::Added(target),
        };
    }
    if has(list, &target) {
        let next = list
            .iter()
            .filter(|m| !m.eq_ignore_ascii_case(&target))
            .cloned()
            .collect();
        return Outcome {
            ignore: Some(next),
            notice: Notice::Removed(target),
        };
    }
    let by = covering(list, &person);
    if !by.is_empty() {
        return said(Notice::Covered { target, masks: by });
    }
    said(Notice::NotIgnoring(target))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn who(nick: &str, user: &str, host: &str) -> Identity {
        Identity {
            nick: nick.into(),
            user: Some(user.into()),
            host: Some(host.into()),
        }
    }

    #[test]
    fn masks_expand_and_match() {
        assert_eq!(expand_mask("bob"), "bob!*@*");
        assert_eq!(expand_mask("bob@host"), "*!bob@host");
        assert_eq!(expand_mask("carol!id"), "carol!id@*");
        assert!(mask_matches("*!*@*.example", &who("Bob", "b", "x.example")));
        assert!(mask_matches("BOB", &who("bob", "b", "h")));
        assert!(!mask_matches("bo?", &who("bobby", "b", "h")));
    }

    #[test]
    fn ignore_and_unignore_edit_the_list() {
        let list = vec!["eve".to_string()];
        let out = run(false, "@bob", &list, None, None);
        assert_eq!(out.ignore.unwrap(), ["eve", "bob"]);
        assert_eq!(out.notice, Notice::Added("bob".into()));
        assert_eq!(
            run(false, "EVE", &list, None, None).notice,
            Notice::AlreadyIgnoring("EVE".into())
        );
        assert_eq!(run(false, "", &[], None, None).notice, Notice::Empty);
        assert_eq!(
            run(false, "", &list, None, None).notice,
            Notice::List(list.clone())
        );
        assert_eq!(
            run(false, "me", &[], None, Some(who("me", "u", "h"))).notice,
            Notice::Yourself
        );
        let out = run(true, "eve", &list, None, None);
        assert_eq!(out.ignore.unwrap(), Vec::<String>::new());
        assert_eq!(
            run(true, "", &list, None, None).notice,
            Notice::Usage {
                command: "unignore"
            }
        );
        let masks = vec!["*!*@bad.host".to_string()];
        let carol = Some(who("carol", "c", "bad.host"));
        assert!(matches!(
            run(true, "carol", &masks, carol.clone(), None).notice,
            Notice::Covered { .. }
        ));
        assert!(matches!(
            run(false, "carol", &masks, carol, None).notice,
            Notice::HiddenBy { .. }
        ));
        assert_eq!(
            run(true, "zed", &masks, None, None).notice,
            Notice::NotIgnoring("zed".into())
        );
    }
}

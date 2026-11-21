//! Unsent drafts per conversation, kept across restarts.
//!
//! Owned by package D1. The text each conversation's message box held when
//! you left it, and when it was last edited. The store holds them in memory
//! ([`State`], the workspace's `drafts` field); the app reads and writes them
//! to this device's state file with [`read`] and [`write`], one set per
//! account, in the same shape as the web client's stored drafts.
//!
//! Drafts are this device's own: nothing here talks to the server, and a
//! bootstrap keeps them.

use std::collections::{BTreeMap, HashMap};

use ircord_contract::Snapshot;
use ircord_contract::event::ServerEvent;
use serde::{Deserialize, Serialize};

use super::{Changes, Workspace};
use crate::config::ConfigDir;

/// The topic this sub-state reports in [`Changes`].
pub const TOPIC: &str = "drafts";

/// Conversations whose text is kept, and the longest text kept for each.
pub const LIMIT: usize = 100;
pub const TEXT_LIMIT: usize = 4000;
/// Characters of a draft shown in the drafts page.
pub const PREVIEW_LIMIT: usize = 160;

/// The section of the device state that holds drafts, by account.
const SECTION: &str = "drafts";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Draft {
    pub text: String,
    /// When it was last edited (ms since the epoch); unknown for drafts
    /// carried over without a time.
    pub edited_ms: Option<i64>,
}

#[derive(Debug, Clone, Default)]
pub struct State {
    drafts: HashMap<String, Draft>,
    /// The saved drafts were read in; until then nothing is written back,
    /// so an early empty box cannot wipe them.
    loaded: bool,
}

/// The drafts as stored: text by conversation ID, and when each was edited.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Saved {
    pub drafts: BTreeMap<String, String>,
    pub times: BTreeMap<String, i64>,
}

/// What putting a discarded draft back did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Restore {
    Restored,
    /// Newer text was typed there since; it is kept.
    Conflict,
    /// [`LIMIT`] drafts are kept already.
    Full,
}

impl State {
    pub fn loaded(&self) -> bool {
        self.loaded
    }

    /// Take the saved drafts: at most [`LIMIT`], the newest first, each cut
    /// to [`TEXT_LIMIT`]; empty ones and nonsense times are dropped.
    pub fn load(&mut self, saved: Saved) {
        let mut drafts: Vec<(String, Draft)> = saved
            .drafts
            .into_iter()
            .filter(|(_, text)| !text.is_empty())
            .map(|(id, text)| {
                let edited_ms = saved.times.get(&id).copied().filter(|t| *t > 0);
                (
                    id,
                    Draft {
                        text: cut(&text, TEXT_LIMIT),
                        edited_ms,
                    },
                )
            })
            .collect();
        drafts.sort_by_key(|(_, d)| std::cmp::Reverse(d.edited_ms.unwrap_or(0)));
        drafts.truncate(LIMIT);
        // Typed before the saved ones were read: that text is newer.
        for (id, draft) in drafts {
            self.drafts.entry(id).or_insert(draft);
        }
        self.loaded = true;
    }

    /// What to store.
    pub fn saved(&self) -> Saved {
        let mut saved = Saved::default();
        for (id, draft) in &self.drafts {
            saved.drafts.insert(id.clone(), draft.text.clone());
            if let Some(t) = draft.edited_ms {
                saved.times.insert(id.clone(), t);
            }
        }
        saved
    }

    pub fn get(&self, conversation: &str) -> Option<&Draft> {
        self.drafts.get(conversation)
    }

    pub fn text(&self, conversation: &str) -> &str {
        self.get(conversation).map_or("", |d| d.text.as_str())
    }

    /// Drafts with visible text.
    pub fn count(&self) -> usize {
        self.drafts
            .values()
            .filter(|d| !d.text.trim().is_empty())
            .count()
    }

    /// Keep `text` as the draft of `conversation`; empty text removes it.
    /// A new draft past [`LIMIT`] pushes out the one edited longest ago.
    /// Returns whether anything changed.
    pub fn set(&mut self, conversation: &str, text: &str, now_ms: i64) -> bool {
        let text = cut(text, TEXT_LIMIT);
        if text.is_empty() {
            return self.drafts.remove(conversation).is_some();
        }
        if self.text(conversation) == text {
            return false;
        }
        if !self.drafts.contains_key(conversation) && self.drafts.len() >= LIMIT {
            let oldest = self
                .drafts
                .iter()
                .min_by_key(|(_, d)| d.edited_ms.unwrap_or(0))
                .map(|(id, _)| id.clone());
            if let Some(oldest) = oldest {
                self.drafts.remove(&oldest);
            }
        }
        self.drafts.insert(
            conversation.to_string(),
            Draft {
                text,
                edited_ms: Some(now_ms),
            },
        );
        true
    }

    /// Remove a draft only if it still holds the text the caller saw, so
    /// newer text survives.
    pub fn discard(&mut self, conversation: &str, seen: &str) -> bool {
        if self.text(conversation) != seen || seen.is_empty() {
            return false;
        }
        self.drafts.remove(conversation);
        true
    }

    /// Put discarded text back, but never over a draft written since.
    pub fn restore(&mut self, conversation: &str, draft: Draft) -> Restore {
        if self.drafts.contains_key(conversation) {
            return Restore::Conflict;
        }
        if self.drafts.len() >= LIMIT {
            return Restore::Full;
        }
        self.drafts.insert(
            conversation.to_string(),
            Draft {
                text: cut(&draft.text, TEXT_LIMIT),
                ..draft
            },
        );
        Restore::Restored
    }
}

/// [`State::set`] on the workspace, with what changed.
pub fn set(w: &mut Workspace, conversation: &str, text: &str, now_ms: i64) -> Changes {
    if w.drafts.set(conversation, text, now_ms) {
        Changes::topic_id(TOPIC, conversation)
    } else {
        Changes::default()
    }
}

/// [`State::load`] on the workspace.
pub fn load(w: &mut Workspace, saved: Saved) -> Changes {
    w.drafts.load(saved);
    Changes::topic(TOPIC)
}

/// The key the drafts of the account signed in to `origin` are stored under.
pub fn account_key(origin: &str, w: &Workspace) -> String {
    format!("{origin} {}", w.user.id)
}

/// The drafts stored on this device for `account` (see [`account_key`]).
pub fn read(config: &ConfigDir, account: &str) -> Saved {
    config
        .section::<BTreeMap<String, Saved>>(SECTION)
        .remove(account)
        .unwrap_or_default()
}

/// Store `saved` for `account`, keeping other accounts' drafts.
pub fn write(config: &ConfigDir, account: &str, saved: Saved) -> std::io::Result<()> {
    let mut all = config.section::<BTreeMap<String, Saved>>(SECTION);
    if saved.drafts.is_empty() {
        all.remove(account);
    } else {
        all.insert(account.to_string(), saved);
    }
    config.save_section(SECTION, &all)
}

/// Where a draft belongs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Place {
    Channel,
    Direct,
}

/// A draft as the drafts page lists it.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub conversation: String,
    pub text: String,
    pub preview: String,
    pub edited_ms: Option<i64>,
    /// The conversation's name; `None` when it is gone (the text is kept).
    pub name: Option<String>,
    pub place: Place,
    pub network: Option<String>,
}

impl Row {
    /// The conversation still exists, so the draft can be opened.
    pub fn available(&self) -> bool {
        self.name.is_some()
    }
}

/// Every draft with visible text, newest edit first, then drafts without a
/// time by name; drafts of conversations that are gone stay in the list.
pub fn rows(w: &Workspace) -> Vec<Row> {
    let mut rows: Vec<Row> = w
        .drafts
        .drafts
        .iter()
        .filter(|(_, d)| !d.text.trim().is_empty())
        .map(|(id, d)| {
            let c = w.conversation(id);
            let network = c.and_then(|c| w.network(&c.network_id));
            Row {
                conversation: id.clone(),
                text: d.text.clone(),
                preview: preview(&d.text, PREVIEW_LIMIT),
                edited_ms: d.edited_ms,
                name: c.filter(|_| network.is_some()).map(|c| c.name.clone()),
                place: match c {
                    Some(c)
                        if c.party.is_some()
                            || c.kind == ircord_contract::ConversationKind::Direct =>
                    {
                        Place::Direct
                    }
                    _ => Place::Channel,
                },
                network: network.map(|n| n.name.clone()),
            }
        })
        .collect();
    rows.sort_by(|a, b| {
        b.edited_ms
            .unwrap_or(0)
            .cmp(&a.edited_ms.unwrap_or(0))
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.network.cmp(&b.network))
            .then_with(|| a.conversation.cmp(&b.conversation))
    });
    rows
}

/// A single-line preview: whitespace collapsed, cut on a character with `…`.
pub fn preview(text: &str, limit: usize) -> String {
    let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.chars().count() <= limit {
        return line;
    }
    let kept: String = line.chars().take(limit.saturating_sub(1)).collect();
    format!("{}…", kept.trim_end())
}

/// `text` cut to `limit` characters.
fn cut(text: &str, limit: usize) -> String {
    match text.char_indices().nth(limit) {
        Some((end, _)) => text[..end].to_string(),
        None => text.to_string(),
    }
}

/// Called for every fresh event after the workspace's own handling.
pub(super) fn apply(_w: &mut Workspace, _event: &ServerEvent, _now_ms: i64) -> Changes {
    Changes::default()
}

/// Called when a bootstrap replaces the workspace, before it is replaced.
/// Drafts are this device's and stay.
pub(super) fn reset(_w: &mut Workspace, _snapshot: &Snapshot) -> Changes {
    Changes::default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ircord_contract::{Conversation, ConversationKind, Network};

    fn workspace() -> Workspace {
        let mut w = Workspace::default();
        w.user.id = "u-me".into();
        w.networks.push(Network {
            id: "n1".into(),
            name: "Home".into(),
            ..Default::default()
        });
        for (id, name, kind) in [
            ("c-general", "general", ConversationKind::Channel),
            ("c-bob", "bob", ConversationKind::Direct),
        ] {
            w.conversations.push(Conversation {
                id: id.into(),
                network_id: "n1".into(),
                name: name.into(),
                kind,
                ..Default::default()
            });
        }
        w
    }

    #[test]
    fn set_keeps_replaces_and_removes() {
        let mut s = State::default();
        assert!(s.set("a", "hello", 10));
        assert!(!s.set("a", "hello", 20), "the same text is no change");
        assert_eq!(s.get("a").unwrap().edited_ms, Some(10));
        assert!(s.set("a", "hello there", 30));
        assert_eq!(s.text("a"), "hello there");
        assert!(s.set("a", "", 40));
        assert_eq!(s.get("a"), None);
        assert!(!s.set("a", "", 50));
    }

    #[test]
    fn text_is_cut_and_the_oldest_draft_makes_room() {
        let mut s = State::default();
        s.set("long", &"é".repeat(TEXT_LIMIT + 10), 1);
        assert_eq!(s.text("long").chars().count(), TEXT_LIMIT);
        for i in 0..LIMIT {
            s.set(&format!("c{i}"), "x", 100 + i as i64);
        }
        assert_eq!(s.drafts.len(), LIMIT);
        assert_eq!(s.get("long"), None, "the oldest went");
        assert!(s.get("c0").is_some());
    }

    #[test]
    fn load_keeps_text_typed_before_it_and_round_trips() {
        let mut s = State::default();
        s.set("a", "typed first", 500);
        let mut saved = Saved::default();
        saved.drafts.insert("a".into(), "older".into());
        saved.drafts.insert("b".into(), "kept".into());
        saved.drafts.insert("empty".into(), String::new());
        saved.times.insert("b".into(), 200);
        saved.times.insert("a".into(), -4);
        s.load(saved);
        assert!(s.loaded());
        assert_eq!(s.text("a"), "typed first");
        assert_eq!(s.get("b").unwrap().edited_ms, Some(200));
        assert_eq!(s.get("empty"), None);
        let mut again = State::default();
        again.load(s.saved());
        assert_eq!(again.saved(), s.saved());
    }

    #[test]
    fn discard_only_what_was_seen_and_restore_without_overwriting() {
        let mut s = State::default();
        s.set("a", "one", 1);
        assert!(!s.discard("a", "other"), "newer text survives");
        assert!(s.discard("a", "one"));
        let gone = Draft {
            text: "one".into(),
            edited_ms: Some(1),
        };
        assert_eq!(s.restore("a", gone.clone()), Restore::Restored);
        assert_eq!(s.get("a"), Some(&gone));
        s.discard("a", "one");
        s.set("a", "new", 2);
        assert_eq!(s.restore("a", gone), Restore::Conflict);
        assert_eq!(s.text("a"), "new");
    }

    #[test]
    fn rows_newest_first_with_gone_conversations_kept() {
        let mut w = workspace();
        w.drafts.set("c-general", "  hi\n\nthere  ", 100);
        w.drafts.set("c-bob", "for bob", 300);
        w.drafts.set("c-gone", "stranded", 200);
        w.drafts.set("c-blank", "   ", 400);
        let rows = rows(&w);
        let ids: Vec<&str> = rows.iter().map(|r| r.conversation.as_str()).collect();
        assert_eq!(ids, ["c-bob", "c-gone", "c-general"]);
        assert_eq!(rows[0].place, Place::Direct);
        assert_eq!(rows[0].network.as_deref(), Some("Home"));
        assert!(!rows[1].available());
        assert_eq!(rows[2].preview, "hi there");
        assert_eq!(rows[2].name.as_deref(), Some("general"));
        assert_eq!(w.drafts.count(), 3);
    }

    #[test]
    fn preview_is_cut_on_a_character() {
        assert_eq!(preview("abcdef", 4), "abc…");
        assert_eq!(preview("ééééé", 5), "ééééé");
        assert_eq!(preview("a  b\tc", 10), "a b c");
    }

    #[test]
    fn stored_per_account() {
        let dir = tempfile::tempdir().unwrap();
        let config = ConfigDir::at(dir.path());
        let mut w = workspace();
        w.drafts.set("c-general", "hi", 5);
        let key = account_key("https://a.example", &w);
        write(&config, &key, w.drafts.saved()).unwrap();
        write(&config, "other", Saved::default()).unwrap();
        assert_eq!(read(&config, &key), w.drafts.saved());
        assert_eq!(read(&config, "other"), Saved::default());
        write(&config, &key, Saved::default()).unwrap();
        assert_eq!(read(&config, &key), Saved::default());
    }
}

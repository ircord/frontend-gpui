//! The emoji catalog (`emotes.updated` names a new version), recent emoji and saved GIFs.
//!
//! Owned by package E1. Keep this feature's state in [`State`] (the
//! workspace's `emoji` field) and react to events in [`apply`]; report what
//! changed with `Changes::topic(TOPIC)` or `Changes::topic_id(TOPIC, id)`.
//!
//! Three catalogs make the emoji a person can pick, as in the web client
//! (`app/stores/media.ts`, `shared/utils/media.ts`):
//!
//! - the standard Unicode set, bundled with the app ([`standard`], the web
//!   client's `public/data/emoji-v18.json` from Emojibase);
//! - the backend's shared emotes (`/api/v1/emotes`), with IDs `instance:<id>`;
//! - the bundled workspace emoji and the cached 7TV globals
//!   (`/api/v1/app/emoji/catalog`).
//!
//! Custom emoji are written into text as `:shortcode:` and drawn as pictures.
//! Saved GIFs, use counts and the skin tone are the library document
//! (`crate::documents::LIBRARY`); the ranking helpers here read it.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::ops::Range;
use std::sync::OnceLock;

use ircord_contract::event::ServerEvent;
use ircord_contract::{
    AppEmojiCatalog, AppEmojiSource, EmojiUsage, EmoteCatalog, Library, LibrarySkinTone, Snapshot,
};
use serde::Deserialize;

use super::{Changes, Workspace};

/// The topic this sub-state reports in [`Changes`].
pub const TOPIC: &str = "emoji";

/// The picker's groups, in the order its group bar shows them.
pub const GROUPS: [&str; 12] = [
    "frequent",
    "workspace",
    "7tv",
    "smileys-emotion",
    "people-body",
    "animals-nature",
    "food-drink",
    "travel-places",
    "activities",
    "objects",
    "symbols",
    "flags",
];

/// The skin tones, `0` for the default yellow.
pub const SKIN_TONES: [&str; 6] = ["0", "1", "2", "3", "4", "5"];

/// How many emoji the library keeps counts for.
pub const USAGE_LIMIT: usize = 120;

/// Quick reactions before anyone has history (`defaultReactions`).
pub const DEFAULT_REACTIONS: [&str; 5] = ["1F44D", "2764", "1F602", "1F62E", "1F622"];

/// The longest message the box sends, for inserting an emoji.
pub const TEXT_LIMIT: usize = 4000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Source {
    /// The standard Unicode set.
    Unicode,
    /// The backend's shared emotes.
    Instance,
    /// Bundled with the server.
    Workspace,
    /// 7TV's global set.
    SevenTv,
}

/// One emoji the picker offers.
#[derive(Debug, Clone, PartialEq)]
pub struct EmojiItem {
    pub id: String,
    pub label: String,
    pub shortcode: String,
    pub aliases: Vec<String>,
    pub group: String,
    /// The character, for standard emoji.
    pub unicode: Option<String>,
    /// Skin tone → the character in that tone.
    pub variants: BTreeMap<String, String>,
    /// The picture, for custom emoji: a URL, or a path on the server.
    pub url: Option<String>,
    pub static_url: Option<String>,
    pub source: Source,
    pub animated: bool,
}

impl EmojiItem {
    pub fn is_custom(&self) -> bool {
        self.unicode.is_none()
    }

    /// What goes into the text: the character in `tone`, or `:shortcode:`.
    pub fn text(&self, tone: &str) -> String {
        match &self.unicode {
            Some(u) => self.variants.get(tone).unwrap_or(u).clone(),
            None => format!(":{}:", self.shortcode),
        }
    }

    /// The picture to draw: the still one unless `animate`.
    pub fn picture(&self, animate: bool) -> Option<&str> {
        let still = self.static_url.as_deref().filter(|s| !s.is_empty());
        if animate {
            self.url.as_deref().or(still)
        } else {
            still.or(self.url.as_deref())
        }
    }

    fn haystack(&self) -> String {
        format!(
            "{} {} {}",
            self.label,
            self.shortcode,
            self.aliases.join(" ")
        )
        .to_lowercase()
    }
}

#[derive(Deserialize)]
struct RawStandard {
    id: String,
    label: String,
    shortcode: String,
    #[serde(default)]
    aliases: Vec<String>,
    group: String,
    unicode: String,
    #[serde(default)]
    variants: BTreeMap<String, String>,
}

/// The standard Unicode emoji, in the picker's order.
pub fn standard() -> &'static [EmojiItem] {
    static STANDARD: OnceLock<Vec<EmojiItem>> = OnceLock::new();
    STANDARD.get_or_init(|| {
        let raw: Vec<RawStandard> = serde_json::from_str(include_str!("../../data/emoji-v18.json"))
            .expect("the bundled emoji catalog is valid");
        raw.into_iter()
            .map(|r| EmojiItem {
                id: r.id,
                label: r.label,
                shortcode: r.shortcode,
                aliases: r.aliases,
                group: r.group,
                unicode: Some(r.unicode),
                variants: r.variants,
                url: None,
                static_url: None,
                source: Source::Unicode,
                animated: false,
            })
            .collect()
    })
}

fn standard_by_id() -> &'static HashMap<String, usize> {
    static BY_ID: OnceLock<HashMap<String, usize>> = OnceLock::new();
    BY_ID.get_or_init(|| {
        standard()
            .iter()
            .enumerate()
            .map(|(i, e)| (e.id.clone(), i))
            .collect()
    })
}

/// Every standard shortcode and alias: a custom emote's former name never
/// takes one of these.
fn reserved() -> &'static HashSet<String> {
    static RESERVED: OnceLock<HashSet<String>> = OnceLock::new();
    RESERVED.get_or_init(|| {
        standard()
            .iter()
            .flat_map(|e| std::iter::once(&e.shortcode).chain(&e.aliases))
            .cloned()
            .collect()
    })
}

/// The custom emoji and where they stand.
#[derive(Debug, Clone, Default)]
pub struct State {
    /// The backend's emotes, enabled ones only.
    pub instance: Vec<EmojiItem>,
    /// Workspace emoji and 7TV globals.
    pub bundled: Vec<EmojiItem>,
    /// `:name:` → the custom emoji it draws.
    by_shortcode: HashMap<String, EmojiItem>,
    by_id: HashMap<String, EmojiItem>,
    /// The catalog version loaded, from `/emotes`.
    pub version: Option<i64>,
    loaded: bool,
    loading: bool,
    stale: bool,
    /// Why the last load failed, if it did.
    pub error: Option<String>,
}

impl State {
    /// Whether the custom catalogs were loaded at least once.
    pub fn is_loaded(&self) -> bool {
        self.loaded
    }

    pub fn is_loading(&self) -> bool {
        self.loading
    }

    /// Start loading the custom catalogs if they are missing or out of date
    /// and no load runs. True when the caller should load.
    pub fn begin_load(&mut self) -> bool {
        if self.loading || (self.loaded && !self.stale) {
            return false;
        }
        self.loading = true;
        self.stale = false;
        true
    }

    /// The loads answered. A failed half keeps what was there before.
    pub fn load_done(
        &mut self,
        instance: Result<EmoteCatalog, String>,
        bundled: Result<AppEmojiCatalog, String>,
    ) -> Changes {
        self.loading = false;
        self.loaded = true;
        self.error = None;
        let catalog = match instance {
            Ok(catalog) => {
                self.version = Some(catalog.version);
                self.instance = instance_items(&catalog);
                Some(catalog)
            }
            // Tried again at the next bootstrap or catalog change, not at
            // once: a server without the catalog would be asked forever.
            Err(e) => {
                self.error = Some(e);
                None
            }
        };
        match bundled {
            Ok(b) => self.bundled = bundled_items(&b),
            Err(e) => {
                self.error.get_or_insert(e);
            }
        }
        let retired: Vec<(String, String)> = catalog
            .iter()
            .flat_map(|c| c.items.iter().flatten())
            .flat_map(|e| {
                let id = format!("instance:{}", e.id);
                e.retired
                    .iter()
                    .flatten()
                    .map(move |name| (name.clone(), id.clone()))
            })
            .collect();
        self.by_shortcode = shortcodes(&self.bundled, &self.instance, &retired, reserved());
        self.by_id = self
            .instance
            .iter()
            .chain(&self.bundled)
            .map(|e| (e.id.clone(), e.clone()))
            .collect();
        Changes::topic(TOPIC)
    }

    /// The custom emoji `:name:` draws, if any.
    pub fn by_shortcode(&self, name: &str) -> Option<&EmojiItem> {
        self.by_shortcode.get(name)
    }

    /// Any emoji by its ID: custom or standard.
    pub fn by_id(&self, id: &str) -> Option<&EmojiItem> {
        self.by_id
            .get(id)
            .or_else(|| standard_by_id().get(id).map(|&i| &standard()[i]))
    }

    /// Custom emoji first (the backend's, then the bundled), then standard.
    pub fn all(&self) -> impl Iterator<Item = &EmojiItem> {
        self.instance
            .iter()
            .chain(&self.bundled)
            .chain(standard().iter())
    }

    /// Every `:name:` that draws a custom emoji, with the emoji.
    pub fn names(&self) -> impl Iterator<Item = (&str, &EmojiItem)> {
        self.by_shortcode.iter().map(|(k, v)| (k.as_str(), v))
    }
}

fn instance_items(catalog: &EmoteCatalog) -> Vec<EmojiItem> {
    catalog
        .items
        .iter()
        .flatten()
        .filter(|e| e.enabled)
        .map(|e| EmojiItem {
            id: format!("instance:{}", e.id),
            label: e.name.clone(),
            shortcode: e.shortcode.clone(),
            aliases: e.aliases.clone().unwrap_or_default(),
            group: "workspace".into(),
            unicode: None,
            variants: BTreeMap::new(),
            url: Some(e.url.clone()),
            static_url: Some(e.static_url.clone()),
            source: Source::Instance,
            animated: e.animated,
        })
        .collect()
}

fn bundled_items(catalog: &AppEmojiCatalog) -> Vec<EmojiItem> {
    catalog
        .items
        .iter()
        .filter_map(|e| {
            let (source, group) = match e.source {
                AppEmojiSource::Workspace => (Source::Workspace, "workspace"),
                AppEmojiSource::V7tv => (Source::SevenTv, "7tv"),
                AppEmojiSource::Unknown => return None,
            };
            Some(EmojiItem {
                id: e.id.clone(),
                label: e.label.clone(),
                shortcode: e.shortcode.clone(),
                aliases: e.aliases.clone(),
                group: group.into(),
                unicode: None,
                variants: BTreeMap::new(),
                url: Some(e.url.clone()),
                static_url: Some(e.static_url.clone()),
                source,
                animated: e.url != e.static_url,
            })
        })
        .collect()
}

/// `:name:` → custom emoji. Later sources win: former names of the
/// backend's emotes (never over a standard name), 7TV globals, the bundled
/// workspace set, then the backend's catalog (`customShortcodes`).
fn shortcodes(
    bundled: &[EmojiItem],
    instance: &[EmojiItem],
    retired: &[(String, String)],
    reserved: &HashSet<String>,
) -> HashMap<String, EmojiItem> {
    let mut map = HashMap::new();
    for (name, id) in retired {
        if !reserved.contains(name)
            && let Some(item) = instance.iter().find(|e| &e.id == id)
        {
            map.insert(name.clone(), item.clone());
        }
    }
    for source in [Source::SevenTv, Source::Workspace] {
        for item in bundled.iter().filter(|e| e.source == source) {
            map.insert(item.shortcode.clone(), item.clone());
        }
    }
    for item in instance {
        for alias in &item.aliases {
            map.insert(alias.clone(), item.clone());
        }
        map.insert(item.shortcode.clone(), item.clone());
    }
    map
}

/// Emoji whose label, shortcode or aliases hold every word of `query`, in
/// `group` (a group or a source; `None` for all).
pub fn search<'a>(
    items: impl Iterator<Item = &'a EmojiItem>,
    query: &str,
    group: Option<&str>,
) -> Vec<&'a EmojiItem> {
    let query = query.to_lowercase().replace(':', "");
    let words: Vec<&str> = query.split_whitespace().collect();
    items
        .filter(|e| {
            group.is_none_or(|g| {
                e.group == g
                    || matches!(
                        (g, e.source),
                        ("instance", Source::Instance) | ("unicode", Source::Unicode)
                    )
            })
        })
        .filter(|e| {
            words.is_empty() || {
                let hay = e.haystack();
                words.iter().all(|w| hay.contains(w))
            }
        })
        .collect()
}

/// Orders matches: exact shortcode, prefix, contained in the shortcode, then
/// matched only by label or alias; within each, your most used first, then
/// shorter (`rankEmoji`). `usage` is an emoji ID's place in your counts.
pub fn rank<'a>(
    mut items: Vec<&'a EmojiItem>,
    usage: &HashMap<String, usize>,
    query: &str,
) -> Vec<&'a EmojiItem> {
    let q = query.to_lowercase().replace(':', "");
    let tier = |e: &EmojiItem| {
        let code = e.shortcode.to_lowercase();
        if code == q {
            0
        } else if code.starts_with(&q) {
            1
        } else if code.contains(&q) {
            2
        } else {
            3
        }
    };
    items.sort_by(|a, b| {
        tier(a)
            .cmp(&tier(b))
            .then_with(|| {
                let ua = usage.get(&a.id).copied().unwrap_or(usize::MAX);
                let ub = usage.get(&b.id).copied().unwrap_or(usize::MAX);
                ua.cmp(&ub)
            })
            .then_with(|| a.shortcode.len().cmp(&b.shortcode.len()))
    });
    items
}

/// Each emoji ID's place in the library's counts (most used first).
pub fn usage_rank(library: Option<&Library>) -> HashMap<String, usize> {
    library
        .map(|l| {
            l.emoji_usage
                .iter()
                .enumerate()
                .map(|(i, u)| (u.id.clone(), i))
                .collect()
        })
        .unwrap_or_default()
}

/// The library's skin tone, `"0"` to `"5"`.
pub fn skin_tone(library: Option<&Library>) -> &'static str {
    match library.map(|l| l.skin_tone) {
        Some(LibrarySkinTone::V1) => "1",
        Some(LibrarySkinTone::V2) => "2",
        Some(LibrarySkinTone::V3) => "3",
        Some(LibrarySkinTone::V4) => "4",
        Some(LibrarySkinTone::V5) => "5",
        _ => "0",
    }
}

pub fn tone_value(tone: &str) -> LibrarySkinTone {
    match tone {
        "1" => LibrarySkinTone::V1,
        "2" => LibrarySkinTone::V2,
        "3" => LibrarySkinTone::V3,
        "4" => LibrarySkinTone::V4,
        "5" => LibrarySkinTone::V5,
        _ => LibrarySkinTone::V0,
    }
}

/// Recent heavy use outranks old heavy use: counts fade with a two-week
/// half-life (`usageScore`).
pub fn usage_score(usage: &EmojiUsage, now_ms: i64) -> f64 {
    let days = (now_ms - super::parse_time(&usage.last_used)).max(0) as f64 / 86_400_000.0;
    usage.count as f64 * 0.5f64.powf(days / 14.0)
}

/// Add one pick of each of `picked` (oldest first) to `list` and keep the
/// highest-scoring [`USAGE_LIMIT`], as the server does (`recordUsage`), so a
/// pick shows before the server answers.
pub fn record_usage(list: &[EmojiUsage], picked: &[String], now_ms: i64) -> Vec<EmojiUsage> {
    let mut by_id: Vec<EmojiUsage> = list.to_vec();
    let n = picked.len() as i64;
    for (i, id) in picked.iter().enumerate() {
        // A batch of picks keeps its order, a millisecond apart.
        let at = now_ms - (n - 1 - i as i64);
        let last_used = chrono::DateTime::from_timestamp_millis(at)
            .unwrap_or_default()
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        match by_id.iter_mut().find(|u| &u.id == id) {
            Some(u) => {
                u.count = (u.count + 1).min(1_000_000);
                u.last_used = last_used;
            }
            None => by_id.push(EmojiUsage {
                id: id.clone(),
                count: 1,
                last_used,
            }),
        }
    }
    by_id.sort_by(|a, b| {
        usage_score(b, now_ms)
            .partial_cmp(&usage_score(a, now_ms))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    by_id.truncate(USAGE_LIMIT);
    by_id
}

/// Emoji IDs, most recently picked first; ties keep the stored order
/// (`recentEmojiIds`).
pub fn recent_ids(list: &[EmojiUsage]) -> Vec<String> {
    let mut rows: Vec<(i64, usize, &str)> = list
        .iter()
        .enumerate()
        .map(|(i, u)| (super::parse_time(&u.last_used), i, u.id.as_str()))
        .collect();
    rows.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    rows.into_iter().map(|r| r.2.to_string()).collect()
}

/// The most recently used distinct emoji that still exist, topped up with
/// the defaults, `count` in all (`pickQuickReactions`).
pub fn quick_reactions<'a>(
    state: &'a State,
    library: Option<&Library>,
    count: usize,
) -> Vec<&'a EmojiItem> {
    let mut picked: Vec<&EmojiItem> = Vec::new();
    let recent = library
        .map(|l| recent_ids(&l.emoji_usage))
        .unwrap_or_default();
    for id in recent.iter().map(String::as_str).chain(DEFAULT_REACTIONS) {
        if picked.len() >= count {
            break;
        }
        if let Some(item) = state.by_id(id)
            && !picked.iter().any(|p| p.id == item.id)
        {
            picked.push(item);
        }
    }
    picked
}

/// "Frequently used": the library's counts that still resolve, at most 36.
pub fn frequent<'a>(state: &'a State, library: Option<&Library>) -> Vec<&'a EmojiItem> {
    library
        .map(|l| {
            l.emoji_usage
                .iter()
                .filter_map(|u| state.by_id(&u.id))
                .take(36)
                .collect()
        })
        .unwrap_or_default()
}

fn name_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '+' || c == '-'
}

/// Every `:name:` in `text` (1 to 64 name characters), with its byte range,
/// outside `` `inline code` ``.
pub fn shortcodes_in(text: &str) -> Vec<(Range<usize>, &str)> {
    let mut out = Vec::new();
    let mut code = false;
    let mut open: Option<usize> = None;
    for (i, c) in text.char_indices() {
        if c == '`' {
            code = !code;
            open = None;
            continue;
        }
        if code {
            continue;
        }
        if c == ':' {
            if let Some(start) = open {
                let name = &text[start + 1..i];
                if !name.is_empty() && name.chars().count() <= 64 {
                    out.push((start..i + 1, name));
                    open = None;
                    continue;
                }
            }
            open = Some(i);
        } else if !name_char(c) {
            open = None;
        }
    }
    out
}

/// The `:query` being typed before the caret, for autocomplete: two to 32
/// name characters after a colon at the start or after a space. Returns the
/// range from the colon to the caret, and the query.
pub fn typed_shortcode(text: &str, caret: usize) -> Option<(Range<usize>, &str)> {
    let before = text.get(..caret)?;
    let start = before.rfind(':')?;
    let query = &before[start + 1..];
    let count = query.chars().count();
    let starts_word = before[..start]
        .chars()
        .next_back()
        .is_none_or(char::is_whitespace);
    (starts_word && (2..=32).contains(&count) && query.chars().all(name_char))
        .then_some((start..caret, query))
}

/// Called for every fresh event after the workspace's own handling.
pub(super) fn apply(w: &mut Workspace, event: &ServerEvent, _now_ms: i64) -> Changes {
    match event {
        ServerEvent::EmotesUpdated(v) if w.emoji.version != Some(v.version) => {
            w.emoji.stale = true;
            Changes::topic(TOPIC)
        }
        _ => Changes::default(),
    }
}

/// Called when a bootstrap replaces the workspace, before it is replaced.
/// The catalog may have changed while the app was away: read it again.
pub(super) fn reset(w: &mut Workspace, _snapshot: &Snapshot) -> Changes {
    if w.emoji.loaded {
        w.emoji.stale = true;
    }
    Changes::default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ircord_contract::{AppEmoji, Emote, EmoteSource, EmoteVersion};

    fn emote(id: &str, code: &str) -> Emote {
        Emote {
            id: id.into(),
            name: code.to_uppercase(),
            shortcode: code.into(),
            enabled: true,
            url: format!("/api/v1/emotes/{id}/image"),
            static_url: format!("/api/v1/emotes/{id}/still"),
            source: EmoteSource::Upload,
            ..Default::default()
        }
    }

    fn loaded(emotes: Vec<Emote>, bundled: Vec<AppEmoji>) -> State {
        let mut s = State::default();
        assert!(s.begin_load());
        assert!(!s.begin_load(), "one load at a time");
        let changes = s.load_done(
            Ok(EmoteCatalog {
                version: 7,
                items: Some(emotes),
                ..Default::default()
            }),
            Ok(AppEmojiCatalog {
                items: bundled,
                ..Default::default()
            }),
        );
        assert!(changes.touches(TOPIC));
        s
    }

    fn seven_tv(code: &str) -> AppEmoji {
        AppEmoji {
            id: format!("7tv:{code}"),
            label: code.into(),
            shortcode: code.into(),
            source: AppEmojiSource::V7tv,
            url: format!("https://cdn.7tv.app/{code}.webp"),
            static_url: format!("https://cdn.7tv.app/{code}.png"),
            ..Default::default()
        }
    }

    #[test]
    fn the_standard_set_loads_with_skin_tones() {
        let all = standard();
        assert!(all.len() > 1800);
        let wave = all.iter().find(|e| e.shortcode == "wave").unwrap();
        assert_eq!(wave.text("0"), "👋");
        assert_eq!(wave.text("3"), "👋🏽");
        let heart = all.iter().find(|e| e.id == "2764").unwrap();
        assert_eq!(
            heart.text("3"),
            heart.unicode.clone().unwrap(),
            "no tones: the default"
        );
    }

    #[test]
    fn the_backend_wins_over_bundled_names_and_former_names_never_shadow() {
        let mut party = emote("e1", "party");
        party.aliases = Some(vec!["celebrate".into()]);
        party.retired = Some(vec!["oldparty".into(), "smile".into()]);
        let mut off = emote("e2", "hidden");
        off.enabled = false;
        let s = loaded(vec![party, off], vec![seven_tv("party"), seven_tv("pog")]);
        assert_eq!(s.by_shortcode("party").unwrap().id, "instance:e1");
        assert_eq!(s.by_shortcode("celebrate").unwrap().id, "instance:e1");
        assert_eq!(s.by_shortcode("oldparty").unwrap().id, "instance:e1");
        assert!(
            s.by_shortcode("smile").is_none(),
            "a standard name stays standard"
        );
        assert_eq!(s.by_shortcode("pog").unwrap().source, Source::SevenTv);
        assert!(
            s.by_shortcode("hidden").is_none(),
            "disabled emotes are left out"
        );
        assert_eq!(s.version, Some(7));
        assert_eq!(s.by_id("1F600").unwrap().unicode.as_deref(), Some("😀"));
        assert_eq!(s.by_id("instance:e1").unwrap().text("0"), ":party:");
    }

    #[test]
    fn search_ranks_exact_then_prefix_then_your_most_used() {
        let s = loaded(vec![], vec![]);
        let found = search(s.all(), "smile", None);
        assert!(found.len() > 5);
        let ranked = rank(found, &HashMap::new(), "smile");
        assert_eq!(ranked[0].shortcode, "smile");
        // A prefix match you use beats an unused one of the same tier.
        let found = search(s.all(), "grin", None);
        let grinning = found
            .iter()
            .find(|e| e.shortcode == "grinning")
            .unwrap()
            .id
            .clone();
        let usage = HashMap::from([(grinning.clone(), 0)]);
        let ranked = rank(found, &usage, "grin");
        assert_eq!(ranked[0].shortcode, "grin", "the exact name first");
        assert_eq!(ranked[1].id, grinning, "then the used prefix match");
        // Every word must match; colons are ignored.
        assert!(
            search(s.all(), ":thumbs up:", None)
                .iter()
                .any(|e| e.id == "1F44D")
        );
        assert!(
            search(s.all(), "", Some("flags"))
                .iter()
                .all(|e| e.group == "flags")
        );
    }

    #[test]
    fn usage_counts_fade_and_recent_picks_lead() {
        let now = parse("2026-11-21T12:00:00Z");
        let old = EmojiUsage {
            id: "1F602".into(),
            count: 10,
            last_used: "2026-09-01T12:00:00Z".into(),
        };
        let list = record_usage(&[old], &["1F44D".into(), "2764".into()], now);
        assert_eq!(list.len(), 3);
        // 10 picks eleven weeks ago fade below one pick now.
        assert_eq!(list[2].id, "1F602");
        assert_eq!(
            recent_ids(&list)[0],
            "2764",
            "the last pick of a batch is newest"
        );
        let again = record_usage(&list, &["1F44D".into()], now + 1000);
        assert_eq!(again.iter().find(|u| u.id == "1F44D").unwrap().count, 2);
    }

    #[test]
    fn quick_reactions_take_recent_then_defaults() {
        let s = loaded(vec![emote("e1", "party")], vec![]);
        let library = Library {
            emoji_usage: vec![
                EmojiUsage {
                    id: "instance:e1".into(),
                    count: 1,
                    last_used: "2026-11-21T12:00:00Z".into(),
                },
                EmojiUsage {
                    id: "instance:gone".into(),
                    count: 9,
                    last_used: "2026-11-21T12:00:01Z".into(),
                },
            ],
            ..Default::default()
        };
        let quick: Vec<_> = quick_reactions(&s, Some(&library), 5)
            .iter()
            .map(|e| e.id.as_str())
            .collect();
        assert_eq!(quick, ["instance:e1", "1F44D", "2764", "1F602", "1F62E"]);
        assert_eq!(quick_reactions(&s, None, 5).len(), 5);
    }

    #[test]
    fn shortcodes_are_found_outside_code() {
        let text = "hi :party: and :not a code: `:code:` :a:b: :pog:";
        let names: Vec<_> = shortcodes_in(text).into_iter().map(|(_, n)| n).collect();
        assert_eq!(names, ["party", "a", "pog"]);
        let (range, name) = shortcodes_in("x :party:").remove(0);
        assert_eq!((range, name), (2..9, "party"));
        assert!(shortcodes_in("12:30:45").iter().all(|(_, n)| *n == "30"));
    }

    #[test]
    fn a_typed_colon_word_is_offered_for_completion() {
        assert_eq!(typed_shortcode("hi :gri", 7), Some((3..7, "gri")));
        assert_eq!(typed_shortcode(":th", 3), Some((0..3, "th")));
        assert_eq!(typed_shortcode("hi :g", 5), None, "one letter is too few");
        assert_eq!(typed_shortcode("time 12:30", 10), None, "not after a word");
        assert_eq!(typed_shortcode("hi :gri ", 8), None, "a space ends it");
        assert_eq!(typed_shortcode("hi :gri", 9), None, "past the end");
    }

    #[test]
    fn a_new_catalog_version_makes_it_stale() {
        let mut w = Workspace {
            emoji: loaded(vec![], vec![]),
            ..Default::default()
        };
        assert!(!w.emoji.begin_load());
        let same = apply(
            &mut w,
            &ServerEvent::EmotesUpdated(EmoteVersion { version: 7 }),
            0,
        );
        assert!(same.is_empty());
        let newer = apply(
            &mut w,
            &ServerEvent::EmotesUpdated(EmoteVersion { version: 8 }),
            0,
        );
        assert!(newer.touches(TOPIC));
        assert!(w.emoji.begin_load());
    }

    #[test]
    fn a_failed_load_keeps_the_old_catalog_and_waits_for_the_next_bootstrap() {
        let mut w = Workspace {
            emoji: loaded(vec![emote("e1", "party")], vec![]),
            ..Default::default()
        };
        w.emoji.stale = true;
        assert!(w.emoji.begin_load());
        w.emoji.load_done(Err("down".into()), Err("down".into()));
        assert!(w.emoji.by_shortcode("party").is_some());
        assert_eq!(w.emoji.error.as_deref(), Some("down"));
        assert!(!w.emoji.begin_load(), "not asked again at once");
        reset(&mut w, &Snapshot::default());
        assert!(w.emoji.begin_load());
    }

    fn parse(t: &str) -> i64 {
        super::super::parse_time(t)
    }
}

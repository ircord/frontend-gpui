//! Profiles, statuses, whois results and the ignore list.
//!
//! Owned by package P1. The workspace's `people` field keeps what the
//! server said when a profile was looked up (WHOIS, "ask their client");
//! the rest of this file is what the people views decide from the
//! workspace, kept here so it is tested without a window: whose status
//! shows, how a status edit is sent, who is ignored, how the member list
//! groups, which channels you share and who an empty direct conversation is
//! with.

use std::collections::{BTreeMap, HashMap};
use std::sync::OnceLock;

use chrono::{DateTime, Duration, TimeZone};
use icu_locale::{Locale, LocaleCanonicalizer, locale, names::LanguageIdentifierDisplayName};
use ircord_contract::event::ServerEvent;
use ircord_contract::{
    CTCPReply, Conversation, ConversationKind, Member, MemberStatus, Network, ProfileUpdate,
    Snapshot, WhoisDetails,
};

use super::{Changes, SELF_ID, Workspace, parse_time};
use crate::api::ApiError;
use crate::commands::ignore::{self, Identity};
use crate::commands::run::member_by_nick;

/// The topic this sub-state reports in [`Changes`]; the ID is [`key`].
pub const TOPIC: &str = "people";

/// How long a WHOIS answer is shown before a card asks again.
pub const WHOIS_FRESH_MS: i64 = 60_000;
/// Channels a profile lists at most.
pub const SHARED_CHANNELS: usize = 12;
/// The longest status text and emoji the server keeps, in UTF-8 bytes.
pub const STATUS_TEXT_BYTES: usize = 200;
pub const STATUS_EMOJI_BYTES: usize = 64;

/// What P1 keeps in the device state file (`ConfigDir::section`).
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Device {
    /// The member list beside channels is hidden.
    pub members_hidden: bool,
}

/// A lookup the server answers.
#[derive(Debug, Clone, PartialEq)]
pub enum Lookup<T> {
    Loading,
    Done(T),
    Failed(String),
}

impl<T> Lookup<T> {
    pub fn done(&self) -> Option<&T> {
        match self {
            Lookup::Done(value) => Some(value),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct State {
    /// WHOIS answers by [`key`], with when they arrived.
    pub whois: HashMap<String, (Lookup<WhoisDetails>, i64)>,
    /// "Ask their client" answers by [`key`]: VERSION, TIME and PING.
    pub ctcp: HashMap<String, Lookup<Vec<CTCPReply>>>,
}

impl State {
    pub fn whois(&self, network: &str, nick: &str) -> Option<&Lookup<WhoisDetails>> {
        self.whois.get(&key(network, nick)).map(|(l, _)| l)
    }

    pub fn ctcp(&self, network: &str, nick: &str) -> Option<&Lookup<Vec<CTCPReply>>> {
        self.ctcp.get(&key(network, nick))
    }
}

/// A person on a network, as lookups are kept: `n1\nbob`.
pub fn key(network: &str, nick: &str) -> String {
    format!("{network}\n{}", nick.to_lowercase())
}

/// Called for every fresh event after the workspace's own handling. The
/// people views read members and profiles the workspace keeps itself.
pub(super) fn apply(_w: &mut Workspace, _event: &ServerEvent, _now_ms: i64) -> Changes {
    Changes::default()
}

/// Called when a bootstrap replaces the workspace: lookups start again.
pub(super) fn reset(w: &mut Workspace, _snapshot: &Snapshot) -> Changes {
    w.people = State::default();
    Changes::topic(TOPIC)
}

// ---- Lookups ----

/// Mark a WHOIS for `nick` as asked, unless a fresh answer or a lookup is
/// already there. Returns whether to send it.
pub fn begin_whois(w: &mut Workspace, network: &str, nick: &str, now_ms: i64) -> bool {
    let k = key(network, nick);
    match w.people.whois.get(&k) {
        Some((Lookup::Loading, _)) => false,
        Some((Lookup::Done(_), at)) if now_ms - at < WHOIS_FRESH_MS => false,
        _ => {
            w.people.whois.insert(k, (Lookup::Loading, now_ms));
            true
        }
    }
}

pub fn whois_done(
    w: &mut Workspace,
    network: &str,
    nick: &str,
    result: Result<WhoisDetails, ApiError>,
    now_ms: i64,
) -> Changes {
    let k = key(network, nick);
    let lookup = match result {
        Ok(details) => Lookup::Done(details),
        Err(e) => Lookup::Failed(e.to_string()),
    };
    w.people.whois.insert(k.clone(), (lookup, now_ms));
    Changes::topic_id(TOPIC, k)
}

/// Mark "ask their client" as asked; `false` while an ask is out.
pub fn begin_ctcp(w: &mut Workspace, network: &str, nick: &str) -> bool {
    let k = key(network, nick);
    if matches!(w.people.ctcp.get(&k), Some(Lookup::Loading)) {
        return false;
    }
    w.people.ctcp.insert(k, Lookup::Loading);
    true
}

pub fn ctcp_done(
    w: &mut Workspace,
    network: &str,
    nick: &str,
    result: Result<Vec<CTCPReply>, ApiError>,
) -> Changes {
    let k = key(network, nick);
    let lookup = match result {
        Ok(replies) => Lookup::Done(replies),
        Err(e) => Lookup::Failed(e.to_string()),
    };
    w.people.ctcp.insert(k.clone(), lookup);
    Changes::topic_id(TOPIC, k)
}

/// Put a member the server answered with (a bot mark, your status) into
/// the workspace, as `member.updated` would.
pub fn put_member(w: &mut Workspace, member: Member) -> Changes {
    if member.id == w.user.id || member.id == SELF_ID {
        w.user = member.clone();
    }
    if let Some(profile) = w
        .networks
        .iter_mut()
        .filter_map(|n| n.profile.as_mut())
        .find(|p| p.id == member.id)
    {
        *profile = member.clone();
    }
    if w.members.contains_key(&member.id) {
        w.members.insert(member.id.clone(), member);
    }
    Changes {
        members: true,
        sidebar: true,
        ..Changes::topic(TOPIC)
    }
}

// ---- Who is who ----

/// You on a network: your profile there, else the account's member.
pub fn me<'a>(w: &'a Workspace, network: Option<&str>) -> &'a Member {
    network
        .and_then(|id| w.network(id))
        .and_then(|n| n.profile.as_ref())
        .unwrap_or(&w.user)
}

/// The member behind an ID on a network; `you` and your own IDs are you.
pub fn person<'a>(w: &'a Workspace, id: &str, network: &str) -> Option<&'a Member> {
    if w.is_self(id, network) {
        return Some(me(w, Some(network)));
    }
    w.members.get(id)
}

/// `nick!user@host`, when the user and host are known.
pub fn address(m: &Member) -> Option<String> {
    match (m.username.as_deref(), m.host.as_deref()) {
        (Some(user), Some(host)) if !user.is_empty() && !host.is_empty() => {
            Some(format!("{}!{user}@{host}", m.nick))
        }
        _ => None,
    }
}

/// The name to show: the display name, else the nick.
pub fn name(m: &Member) -> &str {
    if m.display_name.is_empty() {
        &m.nick
    } else {
        &m.display_name
    }
}

/// The other person in a direct conversation. The server lists only you as
/// its member, so they are matched by nick on the network, as the web
/// client's header does. `None` for someone never seen.
pub fn direct_partner<'a>(w: &'a Workspace, conversation: &str) -> Option<&'a Member> {
    let c = w.conversation(conversation)?;
    if c.kind != ConversationKind::Direct || c.party.is_some() {
        return None;
    }
    if let Some(m) = c
        .member_ids
        .iter()
        .filter(|id| !w.is_self(id, &c.network_id))
        .find_map(|id| w.members.get(id))
    {
        return Some(m);
    }
    member_by_nick(w, &c.network_id, &c.name)
}

/// Channels on `network` you share with them (for yourself: the channels
/// you are in), at most [`SHARED_CHANNELS`], by name.
pub fn shared_channels<'a>(w: &'a Workspace, member: &str, network: &str) -> Vec<&'a Conversation> {
    let mine = w.is_self(member, network);
    let mut list: Vec<&Conversation> = w
        .conversations
        .iter()
        .filter(|c| {
            c.kind == ConversationKind::Channel
                && c.party.is_none()
                && c.network_id == network
                && (if mine {
                    c.joined
                } else {
                    c.member_ids.iter().any(|id| id == member)
                })
        })
        .collect();
    list.sort_by_key(|c| c.name.to_lowercase());
    list.truncate(SHARED_CHANNELS);
    list
}

/// Whether you may mark people as bots on `network`: you own the server
/// it is associated with.
pub fn can_mark_bots(w: &Workspace, network: &str) -> bool {
    w.capabilities
        .moderation
        .as_ref()
        .is_some_and(|m| m.network_id == network)
}

// ---- Status ----

/// A custom status still showing: set, and not past its deadline.
pub fn status_showing(m: &Member, now_ms: i64) -> bool {
    let set = !m.status_text.clone().unwrap_or_default().is_empty()
        || !m.status_emoji.clone().unwrap_or_default().is_empty();
    if !set {
        return false;
    }
    match m.status_expires_at.as_deref().filter(|s| !s.is_empty()) {
        Some(until) => {
            let until = parse_time(until);
            until == 0 || until > now_ms
        }
        None => true,
    }
}

/// "🌴 On holiday", or `None` when no status shows.
pub fn status_label(m: &Member, now_ms: i64) -> Option<String> {
    if !status_showing(m, now_ms) {
        return None;
    }
    let parts: Vec<String> = [m.status_emoji.clone(), m.status_text.clone()]
        .into_iter()
        .flatten()
        .filter(|s| !s.is_empty())
        .collect();
    Some(parts.join(" "))
}

/// The time where someone is, using the reader's clock format.
pub fn local_time(zone: &str, now_ms: i64, clock_12h: bool) -> Option<String> {
    let zone = jiff::tz::TimeZone::get(zone).ok()?;
    let at = jiff::Timestamp::from_millisecond(now_ms).ok()?;
    let time = at.to_zoned_datetime(zone).time();
    let format = if clock_12h { "%I:%M %p" } else { "%H:%M" };
    Some(time.strftime(format).to_string())
}

/// Time-zone identifiers this device can use to show a profile's local time.
pub fn time_zones() -> &'static [String] {
    static ZONES: OnceLock<Vec<String>> = OnceLock::new();
    ZONES.get_or_init(|| {
        let mut zones: Vec<_> = jiff::tz::db()
            .available()
            .map(|zone| zone.as_str().to_owned())
            .collect();
        zones.push("UTC".to_string());
        zones.sort_unstable();
        zones.dedup();
        zones
    })
}

fn canonical_language(tag: &str) -> Option<Locale> {
    let tag = tag.trim();
    if tag.len() > 32
        || !tag
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return None;
    }
    let mut locale: Locale = tag.parse().ok()?;
    LocaleCanonicalizer::new_extended().canonicalize(&mut locale);
    Some(locale)
}

/// Canonicalize a language tag before adding it to a profile.
pub fn language_tag(tag: &str) -> Option<String> {
    let tag = canonical_language(tag)?.to_string();
    (tag.len() <= 32).then_some(tag)
}

/// The display name in the app's English catalog, with the original tag as a fallback.
pub fn language_name(tag: &str) -> String {
    canonical_language(tag)
        .and_then(|language| {
            LanguageIdentifierDisplayName::try_new_heavy(
                locale!("en").into(),
                language.id,
                Default::default(),
            )
            .ok()
        })
        .map_or_else(|| tag.to_string(), |name| name.as_borrowed().to_string())
}

/// When a status set now clears.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Expiry {
    /// Keep the deadline it has.
    Keep,
    Never,
    Minutes30,
    Hour1,
    Hours4,
    /// At the end of today.
    Today,
    Week,
}

impl Expiry {
    pub const CHOICES: [Expiry; 7] = [
        Expiry::Keep,
        Expiry::Never,
        Expiry::Minutes30,
        Expiry::Hour1,
        Expiry::Hours4,
        Expiry::Today,
        Expiry::Week,
    ];

    /// A stable name, for IDs and the catalog.
    pub fn key(self) -> &'static str {
        match self {
            Expiry::Keep => "keep",
            Expiry::Never => "never",
            Expiry::Minutes30 => "30m",
            Expiry::Hour1 => "1h",
            Expiry::Hours4 => "4h",
            Expiry::Today => "today",
            Expiry::Week => "week",
        }
    }

    /// The deadline for a status set at `now`, as RFC 3339; `None` for
    /// one that does not clear (and for [`Expiry::Keep`]).
    pub fn deadline<Tz: TimeZone>(self, now: DateTime<Tz>) -> Option<String> {
        let at = match self {
            Expiry::Keep | Expiry::Never => return None,
            Expiry::Minutes30 => now + Duration::minutes(30),
            Expiry::Hour1 => now + Duration::hours(1),
            Expiry::Hours4 => now + Duration::hours(4),
            Expiry::Week => now + Duration::days(7),
            Expiry::Today => {
                let tz = now.timezone();
                let end = now.date_naive().and_hms_opt(23, 59, 59)?;
                tz.from_local_datetime(&end)
                    .earliest()
                    .unwrap_or_else(|| now.clone() + Duration::hours(1))
            }
        };
        Some(
            at.to_utc()
                .to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        )
    }
}

/// Why a status cannot be saved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusError {
    /// Control characters in the text or emoji.
    Controls,
    TextTooLong,
    EmojiTooLong,
}

/// The profile change that sets your status to `emoji` and `text`, clearing
/// at `expiry`, given the status you have. `Ok(None)` when nothing changes.
pub fn status_update<Tz: TimeZone>(
    current: &Member,
    emoji: &str,
    text: &str,
    expiry: Expiry,
    now: DateTime<Tz>,
) -> Result<Option<ProfileUpdate>, StatusError> {
    let (emoji, text) = (emoji.trim(), text.trim());
    if format!("{emoji}{text}").chars().any(char::is_control) {
        return Err(StatusError::Controls);
    }
    if text.len() > STATUS_TEXT_BYTES {
        return Err(StatusError::TextTooLong);
    }
    if emoji.len() > STATUS_EMOJI_BYTES {
        return Err(StatusError::EmojiTooLong);
    }
    let had_emoji = current.status_emoji.clone().unwrap_or_default();
    let had_text = current.status_text.clone().unwrap_or_default();
    let mut update = ProfileUpdate::default();
    if emoji != had_emoji {
        update.status_emoji = Some(emoji.to_string());
    }
    if text != had_text {
        update.status_text = Some(text.to_string());
    }
    let edited = update.status_emoji.is_some() || update.status_text.is_some();
    if emoji.is_empty() && text.is_empty() {
        if edited || current.status_expires_at.is_some() {
            update.status_expires_at = Some(String::new());
        }
    } else if expiry != Expiry::Keep {
        update.status_expires_at = Some(expiry.deadline(now).unwrap_or_default());
    } else if edited {
        // A new status without a deadline does not keep the old one.
        update.status_expires_at = Some(String::new());
    }
    Ok((update != ProfileUpdate::default()).then_some(update))
}

/// The profile change that clears your status.
pub fn clear_status() -> ProfileUpdate {
    ProfileUpdate {
        status_emoji: Some(String::new()),
        status_text: Some(String::new()),
        status_expires_at: Some(String::new()),
        ..Default::default()
    }
}

// ---- Ignoring ----

fn identity(m: &Member) -> Identity {
    Identity {
        nick: m.nick.clone(),
        user: m.username.clone(),
        host: m.host.clone(),
    }
}

/// The ignore list, once the notification settings are read.
pub fn ignore_list(w: &Workspace) -> Option<Vec<String>> {
    w.documents
        .notification_settings
        .value()
        .map(|s| s.ignore.clone().unwrap_or_default())
}

/// Whether the ignore list hides this person's messages.
pub fn is_ignored(list: &[String], m: &Member) -> bool {
    let who = identity(m);
    list.iter().any(|mask| ignore::mask_matches(mask, &who))
}

/// Ignore them, or stop: the list that leaves and what to say. A person
/// hidden by a wider mask is told so instead of being added.
pub fn toggle_ignore(list: &[String], m: &Member, me: &Member) -> ignore::Outcome {
    let listed = list.iter().any(|e| e.eq_ignore_ascii_case(&m.nick));
    let unignore = listed || is_ignored(list, m);
    ignore::run(
        unignore,
        &m.nick,
        list,
        Some(identity(m)),
        Some(identity(me)),
    )
}

// ---- The member list ----

/// A heading in the member list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GroupKey {
    /// Everyone whose highest channel prefix is this one (`@`, `+`).
    Prefix(char),
    Online,
    Away,
    Offline,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    pub key: GroupKey,
    /// Member IDs with their highest prefix, by name.
    pub members: Vec<(String, String)>,
}

/// Channel prefixes from ISUPPORT `PREFIX=(qaohv)~&@%+`, highest first.
pub fn prefixes(isupport: &BTreeMap<String, String>) -> String {
    isupport
        .get("PREFIX")
        .and_then(|p| p.split_once(')').map(|(_, symbols)| symbols.to_string()))
        .unwrap_or_else(|| "~&@%+".to_string())
}

/// The member list of a conversation as the web client groups it: by
/// highest channel prefix, then online, away and offline, each by name.
/// `filter` keeps those whose nick or display name contains it.
pub fn member_groups(w: &Workspace, conversation: &str, filter: &str) -> Vec<Group> {
    let Some(c) = w.conversation(conversation) else {
        return Vec::new();
    };
    let order = w
        .network(&c.network_id)
        .map(|n| prefixes(&n.isupport))
        .unwrap_or_default();
    let filter = filter.trim().to_lowercase();
    let mut keys: Vec<GroupKey> = order.chars().map(GroupKey::Prefix).collect();
    keys.extend([GroupKey::Online, GroupKey::Away, GroupKey::Offline]);
    let mut buckets: Vec<Vec<(String, String, String)>> = vec![Vec::new(); keys.len()];
    for m in c.member_ids.iter().filter_map(|id| w.members.get(id)) {
        if !filter.is_empty()
            && !format!("{} {}", m.nick, m.display_name)
                .to_lowercase()
                .contains(&filter)
        {
            continue;
        }
        let modes = c.member_modes.get(&m.id).cloned().unwrap_or_default();
        let prefix = order.chars().find(|p| modes.contains(*p));
        let key = match (prefix, m.status) {
            (Some(p), _) => GroupKey::Prefix(p),
            (None, MemberStatus::Online) => GroupKey::Online,
            (None, MemberStatus::Away) => GroupKey::Away,
            (None, _) => GroupKey::Offline,
        };
        let at = keys
            .iter()
            .position(|k| *k == key)
            .unwrap_or(keys.len() - 1);
        buckets[at].push((
            name(m).to_lowercase(),
            m.id.clone(),
            prefix.map(String::from).unwrap_or_default(),
        ));
    }
    keys.into_iter()
        .zip(buckets)
        .filter(|(_, b)| !b.is_empty())
        .map(|(key, mut b)| {
            b.sort();
            Group {
                key,
                members: b.into_iter().map(|(_, id, p)| (id, p)).collect(),
            }
        })
        .collect()
}

/// How many in the conversation are online, as shown in the web header.
pub fn online_count(w: &Workspace, conversation: &str) -> usize {
    w.conversation(conversation)
        .map(|c| {
            c.member_ids
                .iter()
                .filter_map(|id| w.members.get(id))
                .filter(|m| m.status == MemberStatus::Online)
                .count()
        })
        .unwrap_or(0)
}

// ---- Pictures, tones and name colours ----

/// The address to load a person's picture from, or `None` to show their
/// initial: the web client's rule (#278, `instance-image.ts`). Anyone on
/// IRC can point their avatar at a host they control, and loading it would
/// tell that host the viewer's address and when they are online, so only
/// pictures on this IRCord server load. A relative path is on the server.
/// The web's `data:` and `blob:` previews have no counterpart here.
pub fn picture_url(avatar_url: Option<&str>, server: &url::Url) -> Option<url::Url> {
    let value = avatar_url?.trim();
    if value.is_empty() {
        return None;
    }
    let url = server.join(value).ok()?;
    let usable = matches!(url.scheme(), "https" | "http")
        && url.username().is_empty()
        && url.password().is_none();
    (usable && url.origin() == server.origin()).then_some(url)
}

/// The fallback disc's fill and letter colours for a member's tone name, as
/// the web's `identity.css`; an unknown or empty tone is lavender.
pub fn tone(color: &str) -> (u32, u32) {
    match color {
        "peach" => (0xe2b89e, 0x6b4232),
        "rose" => (0xdab6c7, 0x684354),
        "yellow" => (0xd6c69a, 0x65552b),
        "mint" => (0xb8d6c4, 0x355944),
        "blue" => (0xaabfd9, 0x354a65),
        _ => (0xc6b7e8, 0x423456),
    }
}

/// The name colours the web hashes people into, per scheme.
const NAME_PALETTE_DARK: [u32; 8] = [
    0xe7a1bc, 0x98b8f0, 0xc1a4ec, 0xe4b66c, 0x95ceb8, 0xe8ad89, 0x91cbd1, 0xc6c995,
];
const NAME_PALETTE_LIGHT: [u32; 8] = [
    0x984263, 0x3e649d, 0x7852a0, 0x816021, 0x34705a, 0x945c36, 0x307177, 0x676c27,
];

/// `#rrggbb`, the only form a chosen name or accent colour takes.
pub fn parse_hex(value: Option<&str>) -> Option<u32> {
    let digits = value?.strip_prefix('#')?;
    (digits.len() == 6 && digits.chars().all(|c| c.is_ascii_hexdigit()))
        .then(|| u32::from_str_radix(digits, 16).ok())
        .flatten()
}

/// A person's name colour on `background`, as the web's `memberNameColor`
/// (`identity.ts`): their own `nameColor` when set, else a stable colour
/// hashed from `id:account`, moved until it reads at 4.5:1.
pub fn name_color(m: &Member, dark: bool, background: u32) -> u32 {
    use super::appearance::readable_on;
    if let Some(own) = chosen_name_color(m) {
        return readable_on(own, background, 4.5);
    }
    // FNV-1a over each character's first UTF-16 unit (the web's
    // `for … of` with `charCodeAt(0)`), then a murmur finaliser.
    let mut hash: u32 = 2_166_136_261;
    for c in format!("{}:{}", m.id, m.account).chars() {
        let unit = c.encode_utf16(&mut [0; 2])[0];
        hash = (hash ^ u32::from(unit)).wrapping_mul(16_777_619);
    }
    hash = (hash ^ (hash >> 16)).wrapping_mul(0x85eb_ca6b);
    hash = (hash ^ (hash >> 13)).wrapping_mul(0xc2b2_ae35);
    hash ^= hash >> 16;
    let palette = if dark {
        &NAME_PALETTE_DARK
    } else {
        &NAME_PALETTE_LIGHT
    };
    readable_on(palette[hash as usize % palette.len()], background, 4.5)
}

/// The name colour a person picked, which shows even with coloured names
/// off, as on the web.
pub fn chosen_name_color(m: &Member) -> Option<u32> {
    parse_hex(m.name_color.as_deref())
}

/// The accent a person picked for their profile, `#rrggbb`.
pub fn accent_color(m: &Member) -> Option<u32> {
    parse_hex(m.profile_accent_color.as_deref())
}

/// The frames the web draws around a person's picture
/// (`shared/utils/profile.ts`). The backend stores only the preset name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AvatarFrame {
    Ring,
    Glow,
    Gradient,
    Gold,
    Dashed,
}

impl AvatarFrame {
    /// The preset `value` names; `none`, empty or unknown is no frame.
    pub fn parse(value: Option<&str>) -> Option<Self> {
        Some(match value? {
            "ring" => Self::Ring,
            "glow" => Self::Glow,
            "gradient" => Self::Gradient,
            "gold" => Self::Gold,
            "dashed" => Self::Dashed,
            _ => return None,
        })
    }

    /// The preset's name, as the backend stores it.
    pub fn key(self) -> &'static str {
        match self {
            Self::Ring => "ring",
            Self::Glow => "glow",
            Self::Gradient => "gradient",
            Self::Gold => "gold",
            Self::Dashed => "dashed",
        }
    }
}

/// The frame a person chose for their picture.
pub fn avatar_frame(m: &Member) -> Option<AvatarFrame> {
    AvatarFrame::parse(m.avatar_frame.as_deref())
}

/// The nick the backend gives an account before any network names it.
const PLACEHOLDER_NICK: &str = "ircord";

/// Your name on a network, as the web's `selfForNetwork`: the name you set
/// in your profile, else the network's real name, else your nick there.
/// `None` while nothing names you yet (the web shows "You").
pub fn self_name(w: &Workspace, network: &str) -> Option<String> {
    use ircord_contract::MemberNameSource;
    let me = me(w, Some(network));
    let explicit = match me.name_source {
        Some(MemberNameSource::Profile) => me.display_name.clone(),
        Some(_) => String::new(),
        None if me.display_name == "You" || me.display_name == me.nick => String::new(),
        None => me.display_name.clone(),
    };
    if !explicit.trim().is_empty() {
        return Some(explicit);
    }
    fallback_self_name(w.network(network), &me.nick).map(str::to_string)
}

/// What other people see when you leave the profile display name blank.
pub fn fallback_self_name<'a>(
    network: Option<&'a Network>,
    member_nick: &'a str,
) -> Option<&'a str> {
    if let Some(real) = network
        .and_then(|n| n.realname.as_deref())
        .filter(|r| !r.trim().is_empty() && *r != "IRCord")
    {
        return Some(real);
    }
    match network.map(|n| n.nick.as_str()).filter(|n| !n.is_empty()) {
        Some(nick) => Some(nick),
        None if member_nick == PLACEHOLDER_NICK || member_nick.is_empty() => None,
        None => Some(member_nick),
    }
}

/// Your nick on a network: the network's, else your member's.
pub fn self_nick(w: &Workspace, network: &str) -> String {
    w.network(network)
        .map(|n| n.nick.clone())
        .filter(|nick| !nick.is_empty())
        .unwrap_or_else(|| me(w, Some(network)).nick.clone())
}

// ---- Finding people ----

/// People known on `network` whose nick, account or display name matches
/// `query`, best first: a prefix beats a match inside. Everyone in one of
/// the network's conversations counts; you are left out.
pub fn find_people<'a>(
    w: &'a Workspace,
    network: &str,
    query: &str,
    limit: usize,
) -> Vec<&'a Member> {
    let q = query.trim().trim_start_matches('@').to_lowercase();
    let mut seen = std::collections::HashSet::new();
    let mut found: Vec<(u8, String, &Member)> = Vec::new();
    for c in w.conversations.iter().filter(|c| c.network_id == network) {
        for id in &c.member_ids {
            if w.is_self(id, network) || !seen.insert(id.clone()) {
                continue;
            }
            let Some(m) = w.members.get(id) else { continue };
            let fields = [&m.nick, &m.account, &m.display_name];
            let rank = if q.is_empty() {
                Some(2)
            } else if fields.iter().any(|f| f.to_lowercase().starts_with(&q)) {
                Some(0)
            } else if fields.iter().any(|f| f.to_lowercase().contains(&q)) {
                Some(1)
            } else {
                None
            };
            if let Some(rank) = rank {
                found.push((rank, m.nick.to_lowercase(), m));
            }
        }
    }
    found.sort_by(|a, b| (a.0, &a.1).cmp(&(b.0, &b.1)));
    found.into_iter().take(limit).map(|(_, _, m)| m).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{SnapshotBuilder, member};
    use chrono::{FixedOffset, Utc};

    #[test]
    fn profile_time_uses_the_readers_clock_and_the_members_daylight_saving() {
        let before = parse_time("2026-03-29T00:59:00Z");
        let after = parse_time("2026-03-29T01:00:00Z");
        let local = |at, twelve| local_time("Europe/Copenhagen", at, twelve).unwrap();
        assert_eq!(local(before, false), "01:59");
        assert_eq!(local(after, false), "03:00");
        assert_eq!(local(after, true), "03:00 AM");
        assert!(local_time("Not/AZone", after, false).is_none());
        assert!(local_time("", after, false).is_none());
        assert!(time_zones().iter().any(|zone| zone == "Europe/Copenhagen"));
        assert!(time_zones().iter().any(|zone| zone == "UTC"));
    }

    #[test]
    fn profile_language_tags_are_normalized_and_have_readable_names() {
        assert_eq!(language_tag(" en-gb ").as_deref(), Some("en-GB"));
        assert_eq!(language_tag("iw").as_deref(), Some("he"));
        assert!(language_tag("en_US").is_none());
        assert!(language_tag("not a tag").is_none());
        assert!(language_tag("").is_none());
        assert_eq!(language_name("da"), "Danish");
        assert_eq!(language_name("en-GB"), "British English");
        assert_eq!(language_name("not a tag"), "not a tag");
    }

    fn workspace() -> Workspace {
        SnapshotBuilder::new()
            .network("n1", "Home")
            .with_network(|n| {
                n.isupport.insert("PREFIX".into(), "(ov)@+".into());
            })
            .channel("c-general", "general")
            .member("m-bob", "bob")
            .member("m-eve", "eve")
            .member("m-ann", "ann")
            .in_conversation("c-general", "m-bob", "@")
            .in_conversation("c-general", "m-eve", "")
            .in_conversation("c-general", "m-ann", "")
            .channel("c-quiet", "quiet")
            .with_conversation(|c| c.member_ids = vec!["m-eve".into()])
            .direct("d-eve", "m-eve")
            .workspace()
    }

    #[test]
    fn the_frame_and_accent_are_read_as_the_web_reads_them() {
        let mut m = member("m-bob", "bob");
        assert_eq!(avatar_frame(&m), None);
        assert_eq!(accent_color(&m), None);
        for (value, frame) in [
            ("ring", Some(AvatarFrame::Ring)),
            ("glow", Some(AvatarFrame::Glow)),
            ("gradient", Some(AvatarFrame::Gradient)),
            ("gold", Some(AvatarFrame::Gold)),
            ("dashed", Some(AvatarFrame::Dashed)),
            ("none", None),
            ("", None),
            ("sparkles", None),
        ] {
            m.avatar_frame = Some(value.into());
            assert_eq!(avatar_frame(&m), frame, "{value}");
            if let Some(f) = frame {
                assert_eq!(f.key(), value);
            }
        }
        m.profile_accent_color = Some("#22d3ee".into());
        assert_eq!(accent_color(&m), Some(0x22d3ee));
        m.profile_accent_color = Some(String::new());
        assert_eq!(accent_color(&m), None);
    }

    #[test]
    fn statuses_show_until_their_deadline() {
        let mut m = member("m-bob", "bob");
        assert_eq!(status_label(&m, 0), None);
        m.status_emoji = Some("🌴".into());
        m.status_text = Some("On holiday".into());
        assert_eq!(status_label(&m, 0).as_deref(), Some("🌴 On holiday"));
        m.status_expires_at = Some("2026-11-21T12:00:00Z".into());
        let noon = parse_time("2026-11-21T12:00:00Z");
        assert!(status_showing(&m, noon - 1));
        assert!(!status_showing(&m, noon));
    }

    #[test]
    fn a_status_edit_sends_only_what_changed_and_its_deadline() {
        let now = Utc.with_ymd_and_hms(2026, 11, 21, 10, 0, 0).unwrap();
        let me = member("me", "me");
        let set = status_update(&me, "🌴", " On holiday ", Expiry::Hour1, now)
            .unwrap()
            .unwrap();
        assert_eq!(set.status_emoji.as_deref(), Some("🌴"));
        assert_eq!(set.status_text.as_deref(), Some("On holiday"));
        assert_eq!(
            set.status_expires_at.as_deref(),
            Some("2026-11-21T11:00:00Z")
        );

        let mut had = me.clone();
        had.status_text = Some("On holiday".into());
        had.status_expires_at = Some("2026-11-21T11:00:00Z".into());
        assert_eq!(
            status_update(&had, "", "On holiday", Expiry::Keep, now).unwrap(),
            None,
            "nothing changed"
        );
        let edited = status_update(&had, "", "Back soon", Expiry::Keep, now)
            .unwrap()
            .unwrap();
        assert_eq!(edited.status_expires_at.as_deref(), Some(""));
        assert_eq!(edited.status_emoji, None);
        let cleared = status_update(&had, "", "", Expiry::Keep, now)
            .unwrap()
            .unwrap();
        assert_eq!(cleared.status_text.as_deref(), Some(""));
        assert_eq!(cleared.status_expires_at.as_deref(), Some(""));

        assert_eq!(
            status_update(&me, "", "bad\u{7}", Expiry::Keep, now),
            Err(StatusError::Controls)
        );
        assert_eq!(
            status_update(&me, "", &"x".repeat(201), Expiry::Keep, now),
            Err(StatusError::TextTooLong)
        );
        assert_eq!(
            status_update(&me, &"🌴".repeat(17), "", Expiry::Keep, now),
            Err(StatusError::EmojiTooLong)
        );
    }

    #[test]
    fn today_ends_at_local_midnight() {
        let tz = FixedOffset::east_opt(3600).unwrap();
        let now = tz.with_ymd_and_hms(2026, 11, 21, 10, 0, 0).unwrap();
        assert_eq!(
            Expiry::Today.deadline(now).as_deref(),
            Some("2026-11-21T22:59:59Z")
        );
        assert_eq!(Expiry::Never.deadline(now), None);
        assert_eq!(
            Expiry::Week.deadline(now).as_deref(),
            Some("2026-11-28T09:00:00Z")
        );
    }

    #[test]
    fn ignoring_matches_masks_and_toggles() {
        let mut bob = member("m-bob", "bob");
        bob.username = Some("b".into());
        bob.host = Some("bad.host".into());
        let me = member("me", "me");
        assert!(is_ignored(&["BOB".into()], &bob));
        assert!(is_ignored(&["*!*@bad.host".into()], &bob));
        assert!(!is_ignored(&["eve".into()], &bob));

        let out = toggle_ignore(&[], &bob, &me);
        assert_eq!(out.ignore.unwrap(), ["bob"]);
        let out = toggle_ignore(&["bob".into()], &bob, &me);
        assert_eq!(out.ignore.unwrap(), Vec::<String>::new());
        let out = toggle_ignore(&["*!*@bad.host".into()], &bob, &me);
        assert_eq!(out.ignore, None, "a wider mask is not removed");
        assert!(matches!(out.notice, ignore::Notice::Covered { .. }));
        assert_eq!(address(&bob).as_deref(), Some("bob!b@bad.host"));
    }

    #[test]
    fn the_member_list_groups_by_prefix_then_presence_with_a_filter() {
        let mut w = workspace();
        w.members.get_mut("m-eve").unwrap().status = MemberStatus::Away;
        w.members.get_mut("m-ann").unwrap().status = MemberStatus::Online;
        let groups = member_groups(&w, "c-general", "");
        let keys: Vec<_> = groups.iter().map(|g| g.key.clone()).collect();
        assert_eq!(
            keys,
            [GroupKey::Prefix('@'), GroupKey::Online, GroupKey::Away]
        );
        assert_eq!(groups[0].members, [("m-bob".into(), "@".into())]);
        let filtered = member_groups(&w, "c-general", "EV");
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].members[0].0, "m-eve");
        assert_eq!(online_count(&w, "c-general"), 2);
    }

    #[test]
    fn shared_channels_partners_and_finding_people() {
        let w = workspace();
        let names: Vec<_> = shared_channels(&w, "m-eve", "n1")
            .iter()
            .map(|c| c.name.clone())
            .collect();
        assert_eq!(names, ["general", "quiet"]);
        assert_eq!(shared_channels(&w, "m-bob", "n1").len(), 1);
        assert_eq!(
            direct_partner(&w, "d-eve").map(|m| m.nick.as_str()),
            Some("eve")
        );
        assert_eq!(direct_partner(&w, "c-general"), None);
        let found: Vec<_> = find_people(&w, "n1", "e", 10)
            .iter()
            .map(|m| m.nick.clone())
            .collect();
        assert_eq!(found, ["eve"]);
        assert_eq!(find_people(&w, "n1", "", 2).len(), 2);
        assert!(find_people(&w, "n2", "", 10).is_empty());
    }

    #[test]
    fn name_colours_are_the_web_clients() {
        // Vectors from the web's `memberNameColor` (identity.ts) for the
        // same members: light on #ffffff, dark on #1e1e24, light on #e8e5ef.
        let cases: [(&str, &str, Option<&str>, [u32; 3]); 7] = [
            ("m-bob", "bob", None, [0x816021, 0xe4b66c, 0x816021]),
            ("m-eve", "eve", None, [0x34705a, 0x95ceb8, 0x34705a]),
            ("u-me", "me", None, [0x945c36, 0xe8ad89, 0x8d5733]),
            ("m-zoë", "zoë", None, [0x945c36, 0xe8ad89, 0x8d5733]),
            ("m-😀", "", None, [0x307177, 0x91cbd1, 0x2e6b71]),
            (
                "m-bob",
                "bob",
                Some("#ffff00"),
                [0x737300, 0xffff00, 0x666600],
            ),
            (
                "m-bob",
                "bob",
                Some("#FFFF0"),
                [0x816021, 0xe4b66c, 0x816021],
            ),
        ];
        for (id, account, own, want) in cases {
            let mut m = member(id, "x");
            m.account = account.into();
            m.name_color = own.map(String::from);
            let got = [
                name_color(&m, false, 0xffffff),
                name_color(&m, true, 0x1e1e24),
                name_color(&m, false, 0xe8e5ef),
            ];
            assert_eq!(got, want, "{id}:{account} {own:?}");
        }
        let mut m = member("m-bob", "bob");
        m.name_color = Some("lavender".into());
        assert_eq!(chosen_name_color(&m), None, "a tone name is not a colour");
        assert_eq!(tone("mint"), (0xb8d6c4, 0x355944));
        assert_eq!(tone(""), tone("lavender"));
    }

    #[test]
    fn pictures_load_only_from_this_server() {
        // The web's `instance-image.test.ts`, for a native client whose only
        // origin is the server it signed in to.
        let server = url::Url::parse("https://chat.example/").unwrap();
        let load = |v: &str| picture_url(Some(v), &server).map(|u| u.to_string());
        assert_eq!(
            load("/avatars/defaults/fox.webp").as_deref(),
            Some("https://chat.example/avatars/defaults/fox.webp")
        );
        let upload = format!(
            "https://chat.example/files/{}/{}/me.png",
            "a".repeat(32),
            "b".repeat(48)
        );
        assert_eq!(load(&upload), Some(upload.clone()));
        for other in [
            "https://tracker.example/me.png",
            "https://chat.example.tracker.example/me.png",
            "//tracker.example/me.png",
            "https://other.example/files/a/b/me.png",
            "https://chat.example:8443/me.png",
            "http://chat.example/me.png",
            "https://user:pass@chat.example/me.png",
            "javascript:alert(1)",
            "data:image/png;base64,AAAA",
            "",
            "   ",
        ] {
            assert_eq!(load(other), None, "{other}");
        }
        assert_eq!(picture_url(None, &server), None);
        let local = url::Url::parse("http://127.0.0.1:8080/").unwrap();
        assert!(picture_url(Some("http://127.0.0.1:8080/files/x/y/a.png"), &local).is_some());
        assert!(picture_url(Some("http://127.0.0.1:9090/a.png"), &local).is_none());
    }

    #[test]
    fn your_own_name_follows_the_network() {
        let mut w = workspace();
        // The account's member still carries the backend's placeholder nick.
        w.user = member("u-me", "ircord");
        w.networks[0].nick = "captain".into();
        assert_eq!(self_name(&w, "n1").as_deref(), Some("captain"));
        assert_eq!(self_nick(&w, "n1"), "captain");
        w.networks[0].realname = Some("IRCord".into());
        assert_eq!(
            self_name(&w, "n1").as_deref(),
            Some("captain"),
            "the default real name"
        );
        w.networks[0].realname = Some("Karbowiak".into());
        assert_eq!(self_name(&w, "n1").as_deref(), Some("Karbowiak"));
        w.user.display_name = "Kar".into();
        w.user.name_source = Some(ircord_contract::MemberNameSource::Profile);
        assert_eq!(self_name(&w, "n1").as_deref(), Some("Kar"));
        w.user.name_source = Some(ircord_contract::MemberNameSource::Nick);
        assert_eq!(self_name(&w, "n1").as_deref(), Some("Karbowiak"));
        w.networks[0].realname = None;
        w.networks[0].nick.clear();
        assert_eq!(self_name(&w, "n1"), None, "nothing names you yet");
    }

    #[test]
    fn lookups_are_asked_once_while_fresh() {
        let mut w = workspace();
        assert!(begin_whois(&mut w, "n1", "Bob", 0));
        assert!(!begin_whois(&mut w, "n1", "bob", 10), "already asked");
        let details = WhoisDetails {
            found: true,
            nickname: "bob".into(),
            ..Default::default()
        };
        let changes = whois_done(&mut w, "n1", "bob", Ok(details), 100);
        assert!(changes.touches_id(TOPIC, &key("n1", "BOB")));
        assert!(w.people.whois("n1", "bob").unwrap().done().unwrap().found);
        assert!(!begin_whois(&mut w, "n1", "bob", 200));
        assert!(begin_whois(&mut w, "n1", "bob", 100 + WHOIS_FRESH_MS));

        assert!(begin_ctcp(&mut w, "n1", "bob"));
        assert!(!begin_ctcp(&mut w, "n1", "bob"));
        ctcp_done(&mut w, "n1", "bob", Err(ApiError::Network("down".into())));
        assert!(matches!(
            w.people.ctcp("n1", "bob"),
            Some(Lookup::Failed(_))
        ));
        assert!(
            begin_ctcp(&mut w, "n1", "bob"),
            "asked again after a failure"
        );
    }
}

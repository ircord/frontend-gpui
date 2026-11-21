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

use chrono::{DateTime, Duration, TimeZone};
use ircord_contract::event::ServerEvent;
use ircord_contract::{
    CTCPReply, Conversation, ConversationKind, Member, MemberStatus, ProfileUpdate, Snapshot,
    WhoisDetails,
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

/// How many in the conversation are online or away.
pub fn online_count(w: &Workspace, conversation: &str) -> usize {
    w.conversation(conversation)
        .map(|c| {
            c.member_ids
                .iter()
                .filter_map(|id| w.members.get(id))
                .filter(|m| m.status != MemberStatus::Offline)
                .count()
        })
        .unwrap_or(0)
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
        assert_eq!(online_count(&w, "c-general"), 3);
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

//! Administration: `/api/v1/administration/*`, for server owners.
//!
//! Owned by package AD1. The calls follow `shared/api/client.ts` in the web
//! client; the backend checks ownership on every one, so a refusal (403)
//! means the account stopped being an owner ([`refused`]).
//!
//! The rest of this file is what the Administration pages decide without a
//! window, after `AdminStatus.vue`, `AdminDatabase.vue` and
//! `AdminWelcome.vue`: which audit entries were only attempts, the recent
//! connection waits between two database samples, sizes and counts as
//! text, and the channel guide being edited ([`GuideDraft`]).

use ircord_contract as c;

use super::{Api, ApiError, Method, segment};

/// Accounts per page of the user list.
pub const USERS_PAGE: usize = 50;
/// Entries in the recent-actions list.
pub const AUDIT_LIMIT: usize = 30;
/// The guide's limits, as the backend enforces them.
pub const MAX_GROUPS: usize = 12;
pub const MAX_ENTRIES: usize = 50;
pub const MAX_GROUP_NAME: usize = 40;
pub const MAX_DESCRIPTION: usize = 120;

impl Api {
    /// Whether the signed-in account is a server owner.
    pub async fn administration_access(&self) -> Result<c::AdministrationAccess, ApiError> {
        self.get("/api/v1/administration/access").await
    }

    /// The backend's health, versions, totals and the IRC server's.
    pub async fn administration_status(&self) -> Result<c::Status, ApiError> {
        self.get("/api/v1/administration/status").await
    }

    /// The most recent Administration actions, newest first.
    pub async fn administration_audit(&self, limit: usize) -> Result<c::ItemsOwnerEvent, ApiError> {
        self.get_query("/api/v1/administration/audit", &[("limit", limit)])
            .await
    }

    /// Database space, configuration and connection-pool counters.
    pub async fn administration_database(&self) -> Result<c::DatabaseStatus, ApiError> {
        self.get("/api/v1/administration/database").await
    }

    /// One page of accounts, owners first, filtered by `query`.
    pub async fn administration_users(
        &self,
        query: &str,
        offset: usize,
        limit: usize,
    ) -> Result<c::AdminUserPage, ApiError> {
        let mut pairs = vec![("offset", offset.to_string()), ("limit", limit.to_string())];
        if !query.trim().is_empty() {
            pairs.push(("query", query.trim().to_string()));
        }
        self.get_query("/api/v1/administration/users", &pairs).await
    }

    /// Accounts matching a username or a nickname on the IRC server.
    pub async fn administration_identities(
        &self,
        query: &str,
    ) -> Result<c::ItemsIdentityMatch, ApiError> {
        self.get_query(
            "/api/v1/administration/identities",
            &[("query", query.trim())],
        )
        .await
    }

    /// Make the account with ID `account` a server owner.
    pub async fn grant_owner(
        &self,
        account: &str,
        reason: Option<String>,
    ) -> Result<c::AdminUser, ApiError> {
        let body = c::OwnerGrant {
            account: account.to_string(),
            reason: reason.filter(|r| !r.trim().is_empty()),
        };
        self.call(Method::POST, "/api/v1/administration/owners", &body)
            .await
    }

    /// Remove the account with ID `account` as a server owner.
    pub async fn revoke_owner(
        &self,
        account: &str,
        reason: Option<String>,
    ) -> Result<c::AdminUser, ApiError> {
        let path = format!("/api/v1/administration/owners/{}", segment(account));
        let mut request = self.request(Method::DELETE, &path);
        if let Some(reason) = reason.filter(|r| !r.trim().is_empty()) {
            request = request.query(&[("reason", reason)]);
        }
        self.json(request).await
    }

    /// Who may manage the emoji catalog.
    pub async fn emoji_management(&self) -> Result<c::EmojiManagement, ApiError> {
        self.get("/api/v1/administration/emoji").await
    }

    pub async fn save_emoji_management(
        &self,
        update: &c::EmojiManagementUpdate,
    ) -> Result<c::EmojiManagement, ApiError> {
        self.call(Method::PUT, "/api/v1/administration/emoji", update)
            .await
    }

    /// The channel guide as saved, with its warnings and listing requests.
    pub async fn administration_channel_guide(&self) -> Result<c::AdminChannelGuide, ApiError> {
        self.get("/api/v1/administration/channel-guide").await
    }

    /// Replace the guide; `revision` is the one it was loaded at, and a
    /// guide another owner saved since is refused with 409 ([`conflict`]).
    pub async fn save_channel_guide(
        &self,
        revision: i64,
        guide: c::ChannelGuide,
    ) -> Result<c::AdminChannelGuide, ApiError> {
        let body = c::AdminChannelGuideInput { guide, revision };
        self.call(Method::PUT, "/api/v1/administration/channel-guide", &body)
            .await
    }

    /// Public channels on the IRC server matching `query`, to add.
    pub async fn guide_directory(
        &self,
        query: &str,
    ) -> Result<c::ItemsGuideDirectoryChannel, ApiError> {
        self.get_query(
            "/api/v1/administration/channel-guide/channels",
            &[("query", query.trim())],
        )
        .await
    }

    /// Turn down a channel operator's request to list `channel`.
    pub async fn decline_guide_request(&self, channel: &str) -> Result<(), ApiError> {
        self.delete(&format!(
            "/api/v1/administration/channel-guide/requests/{}",
            segment(channel)
        ))
        .await
    }
}

/// The backend refused because the account is no longer an owner.
pub fn refused(error: &ApiError) -> bool {
    matches!(error, ApiError::Http { status: 403, .. })
}

/// Another owner saved the guide since it was loaded.
pub fn conflict(error: &ApiError) -> bool {
    matches!(error, ApiError::Http { status: 409, .. })
}

// ---- Status ----

/// An uptime split for display: `2d 3h 4m` drops the leading zero parts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Uptime {
    pub days: i64,
    pub hours: i64,
    pub minutes: i64,
}

pub fn uptime(seconds: i64) -> Uptime {
    let seconds = seconds.max(0);
    Uptime {
        days: seconds / 86_400,
        hours: seconds % 86_400 / 3_600,
        minutes: seconds % 3_600 / 60,
    }
}

const BOT_MARKS: [&str; 2] = ["bot-mark", "bot-unmark"];
const DELETIONS: [&str; 2] = ["message-delete", "messages-delete-recent"];

/// Whether an audit entry records an attempt rather than something done:
/// deletions succeed only with an outcome starting "deleted" or "started",
/// bot marks only as "marked" or "unmarked"; anything else refused or
/// failed is an attempt.
pub fn audit_failed(event: &c::OwnerEvent) -> bool {
    let outcome = event.outcome.as_str();
    if DELETIONS.contains(&event.action.as_str()) {
        return !outcome.starts_with("deleted") && !outcome.starts_with("started");
    }
    if BOT_MARKS.contains(&event.action.as_str()) {
        return outcome != "marked" && outcome != "unmarked";
    }
    outcome.starts_with("refused") || outcome == "failed"
}

/// Whether the entry shows its outcome beside it (how many were deleted).
pub fn audit_counts(event: &c::OwnerEvent) -> bool {
    DELETIONS.contains(&event.action.as_str()) && !audit_failed(event)
}

/// Connection waits between two samples of the same backend run: waits per
/// minute and milliseconds waited. `None` without two comparable samples
/// (the first sample, a restart, counters that went backwards).
pub fn recent_waits(
    previous: &c::DatabaseStatus,
    current: &c::DatabaseStatus,
) -> Option<(f64, f64)> {
    if previous.started_at != current.started_at
        || current.pool.wait_count < previous.pool.wait_count
    {
        return None;
    }
    let at = |s: &str| chrono::DateTime::parse_from_rfc3339(s).ok();
    let elapsed = (at(&current.sampled_at)? - at(&previous.sampled_at)?).num_milliseconds();
    if elapsed <= 0 {
        return None;
    }
    let waits =
        (current.pool.wait_count - previous.pool.wait_count) as f64 * 60_000.0 / elapsed as f64;
    let waited = (current.pool.wait_total_ms - previous.pool.wait_total_ms).max(0.0);
    Some((waits, waited))
}

/// `1,234,567`.
pub fn count(n: i64) -> String {
    let digits = n.unsigned_abs().to_string();
    let mut out = String::new();
    for (i, d) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(d);
    }
    if n < 0 { format!("-{out}") } else { out }
}

/// A number with at most `digits` decimals and no trailing zeros: `1.5`, `12`.
pub fn decimal(value: f64, digits: usize) -> String {
    let text = format!("{value:.digits$}");
    let text = if text.contains('.') {
        text.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        text
    };
    match text.split_once('.') {
        Some((whole, fraction)) => {
            format!("{}.{fraction}", count(whole.parse().unwrap_or(0)))
        }
        None => count(text.parse().unwrap_or(0)),
    }
}

const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];

fn scale(bytes: i64) -> (f64, usize) {
    let mut value = bytes.max(0) as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    (value, unit)
}

/// The overview's database size: one decimal below ten, `8.4 MB`, `120 MB`.
pub fn overview_bytes(bytes: i64) -> String {
    let (value, unit) = scale(bytes);
    let digits = if value < 10.0 && unit > 0 { 1 } else { 0 };
    format!("{} {}", decimal(value, digits), UNITS[unit])
}

/// The database page's sizes: one decimal for every unit above bytes.
pub fn database_bytes(bytes: i64) -> String {
    let (value, unit) = scale(bytes);
    format!(
        "{} {}",
        decimal(value, if unit > 0 { 1 } else { 0 }),
        UNITS[unit]
    )
}

// ---- Accounts ----

/// How an identity match reads: `nick → account` when they differ.
pub fn describe(m: &c::IdentityMatch) -> String {
    match (&m.nickname, &m.account) {
        (Some(nick), Some(account)) if !nick.eq_ignore_ascii_case(account) => {
            format!("{nick} → {account}")
        }
        (nick, account) => account.clone().or(nick.clone()).unwrap_or_default(),
    }
}

/// Put a changed account back in the list, or at its top when new.
pub fn replace_user(users: &mut Vec<c::AdminUser>, user: c::AdminUser) {
    match users.iter_mut().find(|u| u.id == user.id) {
        Some(existing) => *existing = user,
        None => users.insert(0, user),
    }
}

// ---- The channel guide ----

/// `#name` for what was typed, with or without the `#`.
pub fn channel_name(typed: &str) -> String {
    let name = typed.trim();
    if name.starts_with('#') {
        name.to_string()
    } else {
        format!("#{name}")
    }
}

/// Why a channel could not be added to the guide.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddError {
    /// Nothing was typed.
    Empty,
    /// It is listed already.
    Listed,
    /// The guide holds [`MAX_ENTRIES`] channels.
    Full,
}

/// The guide being edited: its groups' names and its channels, compared with
/// what was loaded to know whether there is anything to save. A channel
/// without a group has an empty `group`, and an empty description is none.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GuideDraft {
    pub groups: Vec<String>,
    pub entries: Vec<c::ChannelGuideEntry>,
}

impl GuideDraft {
    pub fn from_saved(saved: &c::AdminChannelGuide) -> Self {
        Self {
            groups: saved.guide.groups.iter().map(|g| g.name.clone()).collect(),
            entries: saved
                .guide
                .entries
                .iter()
                .map(|e| c::ChannelGuideEntry {
                    channel: e.channel.clone(),
                    group: Some(e.group.clone().unwrap_or_default()),
                    description: Some(e.description.clone().unwrap_or_default()),
                    state: e.state,
                })
                .collect(),
        }
    }

    /// The guide to save: names trimmed, empty groups and descriptions left out.
    pub fn guide(&self) -> c::ChannelGuide {
        c::ChannelGuide {
            groups: self
                .groups
                .iter()
                .map(|name| c::ChannelGuideGroup {
                    name: name.trim().to_string(),
                })
                .collect(),
            entries: self
                .entries
                .iter()
                .map(|e| c::ChannelGuideEntry {
                    channel: e.channel.clone(),
                    group: e.group.clone().filter(|g| !g.is_empty()),
                    description: e
                        .description
                        .as_deref()
                        .map(str::trim)
                        .filter(|d| !d.is_empty())
                        .map(str::to_string),
                    state: e.state,
                })
                .collect(),
        }
    }

    /// Whether the draft differs from `saved`.
    pub fn dirty(&self, saved: &c::AdminChannelGuide) -> bool {
        let mut loaded = saved.guide.clone();
        for e in &mut loaded.entries {
            e.group = e.group.take().filter(|g| !g.is_empty());
            e.description = e.description.take().filter(|d| !d.is_empty());
        }
        self.guide() != loaded
    }

    fn listed(&self, channel: &str) -> bool {
        let key = super::channels::guide_key(channel);
        self.entries
            .iter()
            .any(|e| super::channels::guide_key(&e.channel) == key)
    }

    /// Add a group at the end. Returns false for an empty name, one already
    /// there (in any case) or a full list.
    pub fn add_group(&mut self, name: &str) -> bool {
        let name = name.trim();
        if name.is_empty()
            || self.groups.len() >= MAX_GROUPS
            || self.groups.iter().any(|g| g.eq_ignore_ascii_case(name))
        {
            return false;
        }
        self.groups.push(name.to_string());
        true
    }

    /// Rename a group; its channels follow it.
    pub fn rename_group(&mut self, index: usize, name: &str) {
        let Some(group) = self.groups.get_mut(index) else {
            return;
        };
        let before = std::mem::replace(group, name.to_string());
        for e in &mut self.entries {
            if e.group.as_deref() == Some(before.as_str()) {
                e.group = Some(name.to_string());
            }
        }
    }

    /// Remove a group; its channels are left without one.
    pub fn remove_group(&mut self, index: usize) {
        if index >= self.groups.len() {
            return;
        }
        let name = self.groups.remove(index);
        for e in &mut self.entries {
            if e.group.as_deref() == Some(name.as_str()) {
                e.group = Some(String::new());
            }
        }
    }

    /// Add a channel, listed and without a group, unless it is there already.
    pub fn add_channel(&mut self, typed: &str) -> Result<String, AddError> {
        self.add_entry(typed, String::new(), String::new())
    }

    fn add_entry(
        &mut self,
        typed: &str,
        group: String,
        description: String,
    ) -> Result<String, AddError> {
        let channel = channel_name(typed);
        if channel.len() < 2 {
            return Err(AddError::Empty);
        }
        if self.listed(&channel) {
            return Err(AddError::Listed);
        }
        if self.entries.len() >= MAX_ENTRIES {
            return Err(AddError::Full);
        }
        self.entries.push(c::ChannelGuideEntry {
            channel: channel.clone(),
            group: Some(group),
            description: Some(description),
            state: c::ChannelGuideEntryState::Listed,
        });
        Ok(channel)
    }

    /// Add a requested channel with its group (made when missing, if there
    /// is room) and description.
    pub fn apply_request(&mut self, r: &c::ChannelGuideRequestView) -> Result<String, AddError> {
        let mut group = String::new();
        if let Some(wanted) = r.group.as_deref().filter(|g| !g.trim().is_empty()) {
            match self.groups.iter().find(|g| g.eq_ignore_ascii_case(wanted)) {
                Some(existing) => group = existing.clone(),
                None => {
                    if self.add_group(wanted) {
                        group = wanted.trim().to_string();
                    }
                }
            }
        }
        self.add_entry(&r.channel, group, r.description.clone().unwrap_or_default())
    }

    /// Requests for channels the draft does not list yet.
    pub fn pending<'a>(
        &self,
        saved: &'a c::AdminChannelGuide,
    ) -> Vec<&'a c::ChannelGuideRequestView> {
        saved
            .requests
            .iter()
            .filter(|r| !self.listed(&r.channel))
            .collect()
    }

    /// The guide as a new member's picker shows it.
    pub fn preview(&self) -> (Vec<c::ChannelGuideGroup>, Vec<c::ChannelGuideChannel>) {
        let guide = self.guide();
        let groups = guide
            .groups
            .into_iter()
            .filter(|g| !g.name.is_empty())
            .collect();
        let channels = guide
            .entries
            .into_iter()
            .map(|e| c::ChannelGuideChannel {
                channel: channel_name(&e.channel),
                description: e.description,
                group: e.group,
                state: match e.state {
                    c::ChannelGuideEntryState::Everyone => c::ChannelGuideChannelState::Everyone,
                    c::ChannelGuideEntryState::Suggested => c::ChannelGuideChannelState::Suggested,
                    _ => c::ChannelGuideChannelState::Listed,
                },
                ..Default::default()
            })
            .collect();
        (groups, channels)
    }
}

/// Move the item at `index` by `by` places, if it stays in the list.
pub fn shift<T>(list: &mut Vec<T>, index: usize, by: isize) {
    let to = index as isize + by;
    if index >= list.len() || to < 0 || to as usize >= list.len() {
        return;
    }
    let item = list.remove(index);
    list.insert(to as usize, item);
}

/// The kind of warning the backend gave for `channel`, if any.
pub fn warning(saved: &c::AdminChannelGuide, channel: &str) -> Option<c::ChannelGuideWarningKind> {
    let key = super::channels::guide_key(channel);
    saved
        .warnings
        .iter()
        .find(|w| super::channels::guide_key(&w.channel) == key)
        .map(|w| w.kind)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use serde_json::json;

    use super::*;
    use crate::testing::{CannedServer, Reply};

    fn event(action: &str, outcome: &str) -> c::OwnerEvent {
        c::OwnerEvent {
            action: action.into(),
            outcome: outcome.into(),
            ..Default::default()
        }
    }

    #[test]
    fn audit_entries_that_were_only_attempts() {
        assert!(!audit_failed(&event("owner-grant", "granted")));
        assert!(audit_failed(&event("owner-grant", "refused: last owner")));
        assert!(audit_failed(&event("owner-revoke", "failed")));
        assert!(!audit_failed(&event("message-delete", "deleted 3")));
        assert!(!audit_failed(&event("messages-delete-recent", "started")));
        assert!(audit_failed(&event("message-delete", "already deleted")));
        assert!(!audit_failed(&event("bot-mark", "marked")));
        assert!(audit_failed(&event("bot-unmark", "unconfirmed")));
        assert!(audit_counts(&event("message-delete", "deleted 3")));
        assert!(!audit_counts(&event("owner-grant", "granted")));
    }

    fn sample(started: &str, at: &str, waits: i64, ms: f64) -> c::DatabaseStatus {
        c::DatabaseStatus {
            started_at: started.into(),
            sampled_at: at.into(),
            pool: c::DatabasePool {
                wait_count: waits,
                wait_total_ms: ms,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    fn recent_waits_need_two_samples_of_one_run() {
        let a = sample("2026-11-21T08:00:00Z", "2026-11-21T10:00:00Z", 10, 100.0);
        let b = sample("2026-11-21T08:00:00Z", "2026-11-21T10:00:30Z", 13, 160.0);
        assert_eq!(recent_waits(&a, &b), Some((6.0, 60.0)));
        let restarted = sample("2026-11-21T10:00:10Z", "2026-11-21T10:00:30Z", 1, 1.0);
        assert_eq!(recent_waits(&a, &restarted), None);
        assert_eq!(recent_waits(&b, &a), None, "counters went backwards");
        assert_eq!(recent_waits(&a, &a), None, "no time passed");
    }

    #[test]
    fn numbers_sizes_and_uptime() {
        assert_eq!(count(0), "0");
        assert_eq!(count(1234567), "1,234,567");
        assert_eq!(count(-1000), "-1,000");
        assert_eq!(decimal(1.26, 1), "1.3");
        assert_eq!(decimal(2.0, 1), "2");
        assert_eq!(decimal(12345.5, 1), "12,345.5");
        assert_eq!(overview_bytes(512), "512 B");
        assert_eq!(overview_bytes(8_808_038), "8.4 MB");
        assert_eq!(overview_bytes(125_829_120), "120 MB");
        assert_eq!(database_bytes(125_829_120), "120 MB");
        assert_eq!(database_bytes(126_000_000), "120.2 MB");
        assert_eq!(
            uptime(2 * 86_400 + 3 * 3_600 + 4 * 60 + 5),
            Uptime {
                days: 2,
                hours: 3,
                minutes: 4
            }
        );
    }

    #[test]
    fn identity_matches_read_nick_then_account() {
        let m = |nick: Option<&str>, account: Option<&str>| c::IdentityMatch {
            nickname: nick.map(Into::into),
            account: account.map(Into::into),
            ..Default::default()
        };
        assert_eq!(describe(&m(Some("Bobby"), Some("bob"))), "Bobby → bob");
        assert_eq!(describe(&m(Some("Bob"), Some("bob"))), "bob");
        assert_eq!(describe(&m(Some("ghost"), None)), "ghost");
    }

    fn saved() -> c::AdminChannelGuide {
        serde_json::from_value(json!({
            "defaults": ["#lobby"], "directory": true, "revision": 4, "saved": true,
            "guide": {"groups": [{"name": "Games"}],
                      "entries": [{"channel": "#chess", "group": "Games", "state": "suggested"},
                                  {"channel": "#help", "description": "Ask", "state": "everyone"}]},
            "requests": [{"channel": "#Go", "createdAt": "2026-11-20T10:00:00Z", "requester": "bob",
                          "group": "games", "description": "Baduk"},
                         {"channel": "#chess", "createdAt": "2026-11-20T10:00:00Z", "requester": "eve"}],
            "warnings": [{"channel": "#CHESS", "kind": "unregistered"}]
        }))
        .unwrap()
    }

    #[test]
    fn the_guide_draft_edits_groups_and_channels() {
        let saved = saved();
        let mut draft = GuideDraft::from_saved(&saved);
        assert!(!draft.dirty(&saved));
        assert_eq!(
            warning(&saved, "#chess"),
            Some(c::ChannelGuideWarningKind::Unregistered)
        );

        // Only the request for a channel not listed yet is pending.
        let pending: Vec<_> = draft
            .pending(&saved)
            .iter()
            .map(|r| r.channel.clone())
            .collect();
        assert_eq!(pending, ["#Go"]);

        // Approving joins the existing group, whatever its case.
        assert_eq!(draft.apply_request(&saved.requests[0]), Ok("#Go".into()));
        assert_eq!(draft.entries[2].group.as_deref(), Some("Games"));
        assert!(draft.pending(&saved).is_empty());
        assert!(draft.dirty(&saved));

        assert_eq!(draft.add_channel("CHESS"), Err(AddError::Listed));
        assert_eq!(draft.add_channel("  "), Err(AddError::Empty));
        assert_eq!(draft.add_channel("rust"), Ok("#rust".into()));

        assert!(!draft.add_group("games"), "names are unique in any case");
        assert!(draft.add_group("Code"));
        draft.rename_group(0, "Play");
        assert_eq!(draft.entries[0].group.as_deref(), Some("Play"));
        shift(&mut draft.groups, 1, -1);
        assert_eq!(draft.groups, ["Code", "Play"]);
        draft.remove_group(1);
        assert_eq!(draft.entries[0].group.as_deref(), Some(""));

        let guide = draft.guide();
        assert_eq!(guide.entries[0].group, None, "no group is sent as none");
        assert_eq!(guide.entries[1].description.as_deref(), Some("Ask"));
        assert_eq!(guide.entries[3].description, None);

        let (groups, channels) = draft.preview();
        assert_eq!(groups.len(), 1);
        assert_eq!(channels[1].state, c::ChannelGuideChannelState::Everyone);

        // The guide is full at fifty channels.
        for i in 0..MAX_ENTRIES {
            let _ = draft.add_channel(&format!("c{i}"));
        }
        assert_eq!(draft.entries.len(), MAX_ENTRIES);
        assert_eq!(draft.add_channel("one-more"), Err(AddError::Full));
    }

    #[test]
    fn calls_send_what_the_backend_expects() {
        let runtime = crate::client::runtime().unwrap();
        let server = CannedServer::start(&runtime);
        let user = json!({"id": "a-bob", "username": "bob", "owner": true, "ownerSource": "owner",
                          "createdAt": "2026-01-01T00:00:00Z", "lastSeenAt": "2026-11-21T00:00:00Z",
                          "sessions": 1, "networks": 2});
        server.route("POST", "/api/v1/administration/owners", 200, user.clone());
        server.route("DELETE", "/api/v1/administration/owners/*", 200, user);
        server.route(
            "GET",
            "/api/v1/administration/users",
            200,
            json!({"items": [], "more": false}),
        );
        server.route_reply(
            "PUT",
            "/api/v1/administration/channel-guide",
            Reply::problem(409, "the guide changed"),
        );
        server.route_reply(
            "DELETE",
            "/api/v1/administration/channel-guide/requests/*",
            Reply::empty(),
        );
        server.route_reply(
            "GET",
            "/api/v1/administration/status",
            Reply::problem(403, "not an owner"),
        );
        let api = server.api();
        let wait = Duration::from_secs(5);

        let owner = runtime
            .block_on(api.grant_owner("a-bob", Some("helps out".into())))
            .unwrap();
        assert!(owner.owner);
        let request = server.wait_for("POST", "/api/v1/administration/owners", wait);
        assert_eq!(
            request.json(),
            json!({"account": "a-bob", "reason": "helps out"})
        );

        runtime.block_on(api.revoke_owner("a-bob", None)).unwrap();
        let request = server.wait_for("DELETE", "/api/v1/administration/owners/*", wait);
        assert_eq!(request.path, "/api/v1/administration/owners/a-bob");
        assert_eq!(request.query_param("reason"), None);

        runtime
            .block_on(api.administration_users(" bo ", 50, USERS_PAGE))
            .unwrap();
        let request = server.wait_for("GET", "/api/v1/administration/users", wait);
        assert_eq!(request.query_param("query").as_deref(), Some("bo"));
        assert_eq!(request.query_param("offset").as_deref(), Some("50"));

        let error = runtime
            .block_on(api.save_channel_guide(4, c::ChannelGuide::default()))
            .unwrap_err();
        assert!(conflict(&error));
        let request = server.wait_for("PUT", "/api/v1/administration/channel-guide", wait);
        assert_eq!(request.json()["revision"], 4);

        runtime.block_on(api.decline_guide_request("#go")).unwrap();
        let request = server.wait_for(
            "DELETE",
            "/api/v1/administration/channel-guide/requests/*",
            wait,
        );
        assert_eq!(
            request.path,
            "/api/v1/administration/channel-guide/requests/%23go"
        );

        let error = runtime.block_on(api.administration_status()).unwrap_err();
        assert!(refused(&error));
    }
}

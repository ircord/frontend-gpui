//! Channels: `/networks/{id}/channels`, `/networks/{id}/channel-guide`, `/conversations/{id}/channel-settings`, `/conversations/{id}/lists/{mode}`, `/conversations/{id}/registration`, `/conversations/{id}/access`, `/conversations/{id}/channel-guide`, `/invitations/*`.
//!
//! Owned by package CH1, CH2. Add calls in this `impl Api` block with the
//! helpers in `api/mod.rs` and the types in `ircord_contract`.
//!
//! CH1 joins and creates channels, as the web client's `JoinDialog.vue`,
//! `CreateChannelDialog.vue` and `shared/utils/channel-guide.ts` do: the
//! network's channel list (`/list`), the server's channel guide and joining
//! from it, the name check, and creating a channel from a template.
//! [`CreateForm`] is the create form without its window, and [`remember`]
//! puts a joined or created channel into the workspace.

use std::collections::{BTreeMap, HashSet};

use ircord_contract as c;

#[allow(unused_imports)]
use super::{Api, ApiError, Method, segment};
use crate::store::{Changes, Workspace};

/// How many channels the channel list asks for at once.
pub const LIST_LIMIT: u32 = 50;

impl Api {
    /// The network's channels matching `filter` (a part of the name), with
    /// their topics and how many people are in them.
    pub async fn channel_list(
        &self,
        network: &str,
        filter: &str,
    ) -> Result<Vec<c::ChannelListing>, ApiError> {
        let path = format!("/api/v1/networks/{}/channels", segment(network));
        let limit = LIST_LIMIT.to_string();
        let page: c::ItemsChannelListing = self
            .get_query(&path, &[("q", filter), ("limit", limit.as_str())])
            .await?;
        Ok(page.items)
    }

    /// The server's channel guide; `available` is false on networks other
    /// than the server's own.
    pub async fn channel_guide(&self, network: &str) -> Result<c::ChannelGuideView, ApiError> {
        self.get(&format!(
            "/api/v1/networks/{}/channel-guide",
            segment(network)
        ))
        .await
    }

    /// Join channels from the guide in one request; each one that cannot be
    /// joined is reported without stopping the rest.
    pub async fn join_from_guide(
        &self,
        network: &str,
        channels: &[String],
    ) -> Result<c::ChannelGuideJoinResult, ApiError> {
        let path = format!("/api/v1/networks/{}/channel-guide/join", segment(network));
        let body = c::ChannelGuideJoinInput {
            channels: channels.to_vec(),
        };
        self.call(Method::POST, &path, &body).await
    }

    /// Join (or open) the channel `name`, such as `#lobby`.
    pub async fn join_channel(
        &self,
        network: &str,
        name: &str,
    ) -> Result<c::Conversation, ApiError> {
        let body = c::ConversationInputBody {
            network_id: network.to_string(),
            name: Some(name.to_string()),
            ..Default::default()
        };
        self.call(Method::POST, "/api/v1/conversations", &body)
            .await
    }

    /// Whether nobody uses the channel `name` yet.
    pub async fn channel_availability(
        &self,
        network: &str,
        name: &str,
    ) -> Result<c::ChannelAvailabilityStatus, ApiError> {
        let path = format!(
            "/api/v1/networks/{}/channels/availability",
            segment(network)
        );
        let answer: c::ChannelAvailability = self.get_query(&path, &[("name", name)]).await?;
        Ok(answer.status)
    }

    /// Create a channel: join it, then set its topic and modes, and register
    /// it with services when the preference says so.
    pub async fn create_channel(
        &self,
        network: &str,
        body: &c::ChannelCreate,
    ) -> Result<c::ChannelCreateResult, ApiError> {
        let path = format!("/api/v1/networks/{}/channels", segment(network));
        self.call(Method::POST, &path, body).await
    }
}

// ---- Channel settings and moderation (package CH2) ----
//
// Topics, modes and list entries go out as raw IRC lines
// (`store::server_log::send_raw`), as the web client sends them; the lines
// are built in `crate::channel_modes`. These are the calls with their own
// routes.

fn conversation_path(conversation: &str, rest: &str) -> String {
    format!("/api/v1/conversations/{}/{rest}", segment(conversation))
}

impl Api {
    /// The entries of a list mode (`b`, `e`, `I`, `q`…).
    pub async fn mode_list(
        &self,
        conversation: &str,
        mode: char,
    ) -> Result<Vec<c::ChannelListEntry>, ApiError> {
        let path = conversation_path(
            conversation,
            &format!("lists/{}", segment(&mode.to_string())),
        );
        self.get::<c::ItemsChannelListEntry>(&path)
            .await
            .map(|page| page.items)
    }

    /// Change the channel-wide settings: notifications, group mentions, NSFW.
    pub async fn channel_settings(
        &self,
        conversation: &str,
        body: &c::ChannelSettingsPatchBody,
    ) -> Result<c::Conversation, ApiError> {
        let path = conversation_path(conversation, "channel-settings");
        self.call(Method::PUT, &path, body).await
    }

    /// The channel's registration with services. A network without a
    /// ChanServ IRCord can manage answers 501.
    pub async fn registration(
        &self,
        conversation: &str,
    ) -> Result<c::ChannelRegistration, ApiError> {
        self.get(&conversation_path(conversation, "registration"))
            .await
    }

    /// Register the channel to your services account.
    pub async fn register_channel(
        &self,
        conversation: &str,
    ) -> Result<c::ChannelRegistration, ApiError> {
        let path = conversation_path(conversation, "registration");
        self.json(self.request(Method::POST, &path)).await
    }

    pub async fn unregister_channel(&self, conversation: &str) -> Result<(), ApiError> {
        self.delete(&conversation_path(conversation, "registration"))
            .await
    }

    /// The access list: roles services give by account. Founders and
    /// people with admin or operator access may read it; others get 403.
    pub async fn channel_access(
        &self,
        conversation: &str,
    ) -> Result<Vec<c::AccessEntry>, ApiError> {
        self.get::<c::ItemsAccessEntry>(&conversation_path(conversation, "access"))
            .await
            .map(|page| page.items)
    }

    /// Give `account` a role; the answer is the list as it now stands.
    pub async fn set_channel_access(
        &self,
        conversation: &str,
        account: &str,
        mode: c::AccessChangeBodyMode,
    ) -> Result<Vec<c::AccessEntry>, ApiError> {
        let path = conversation_path(conversation, &format!("access/{}", segment(account)));
        self.call::<_, c::ItemsAccessEntry>(Method::PUT, &path, &c::AccessChangeBody { mode })
            .await
            .map(|page| page.items)
    }

    pub async fn remove_channel_access(
        &self,
        conversation: &str,
        account: &str,
    ) -> Result<Vec<c::AccessEntry>, ApiError> {
        let path = conversation_path(conversation, &format!("access/{}", segment(account)));
        self.json::<c::ItemsAccessEntry>(self.request(Method::DELETE, &path))
            .await
            .map(|page| page.items)
    }

    /// Offer the channel to `account`. `Pending` until they accept.
    pub async fn transfer_channel(
        &self,
        conversation: &str,
        account: &str,
    ) -> Result<c::TransferResult, ApiError> {
        let body = c::TransferChannelRequest {
            account: account.to_string(),
        };
        self.call(
            Method::POST,
            &conversation_path(conversation, "transfer"),
            &body,
        )
        .await
    }

    /// This channel's place in the server's channel guide.
    pub async fn guide_listing(
        &self,
        conversation: &str,
    ) -> Result<c::ChannelGuideListing, ApiError> {
        self.get(&conversation_path(conversation, "channel-guide"))
            .await
    }

    /// Ask the server owners to list the channel (or update the request).
    pub async fn request_guide_listing(
        &self,
        conversation: &str,
        body: &c::ChannelGuideRequestInput,
    ) -> Result<c::ChannelGuideListing, ApiError> {
        let path = conversation_path(conversation, "channel-guide");
        self.call(Method::PUT, &path, body).await
    }

    pub async fn withdraw_guide_listing(
        &self,
        conversation: &str,
    ) -> Result<c::ChannelGuideListing, ApiError> {
        let path = conversation_path(conversation, "channel-guide");
        self.json(self.request(Method::DELETE, &path)).await
    }

    /// Count `member`'s messages in the channel over `range` (`1h`, `24h`, `7d`).
    pub async fn recent_messages(
        &self,
        conversation: &str,
        member: &str,
        range: &str,
    ) -> Result<c::RecentMessagesPreview, ApiError> {
        let path = conversation_path(
            conversation,
            &format!("members/{}/recent-messages", segment(member)),
        );
        self.get_query(&path, &[("range", range)]).await
    }

    /// Start deleting what [`Api::recent_messages`] counted; poll the job.
    pub async fn delete_recent_messages(
        &self,
        conversation: &str,
        member: &str,
        body: &c::DeleteRecentMessagesRequest,
    ) -> Result<c::PurgeJob, ApiError> {
        let path = conversation_path(
            conversation,
            &format!("members/{}/recent-messages/deletion", segment(member)),
        );
        self.call(Method::POST, &path, body).await
    }

    pub async fn recent_messages_deletion(&self, job: &str) -> Result<c::PurgeJob, ApiError> {
        self.get(&format!(
            "/api/v1/recent-messages-deletions/{}",
            segment(job)
        ))
        .await
    }
}

/// Put a joined or created channel into the workspace, if it is not there.
pub fn remember(w: &mut Workspace, conversation: c::Conversation) -> Changes {
    if w.conversation(&conversation.id).is_some() {
        return Changes::default();
    }
    w.conversations.push(conversation);
    Changes {
        sidebar: true,
        ..Default::default()
    }
}

/// What was typed into the channel search: no spaces around, no `#`.
pub fn normalize_filter(value: &str) -> String {
    value.trim().trim_start_matches('#').to_string()
}

/// `general` or `#general` → `#general`; `None` for an empty name.
pub fn channel_name(value: &str) -> Option<String> {
    let bare = normalize_filter(value);
    (!bare.is_empty()).then(|| format!("#{bare}"))
}

/// The channel guide's key for a channel: its name in lower case.
pub fn guide_key(channel: &str) -> String {
    channel.to_lowercase()
}

/// The channels (as `#name` in lower case) you are in on `network`.
pub fn joined(w: &Workspace, network: &str) -> HashSet<String> {
    w.conversations
        .iter()
        .filter(|c| {
            c.network_id == network && c.kind == c::ConversationKind::Channel && c.party.is_none()
        })
        .map(|c| guide_key(c.target.as_deref().unwrap_or(&format!("#{}", c.name))))
        .collect()
}

/// The joined channel `name` (with or without `#`) on `network`, if any.
pub fn existing(w: &Workspace, network: &str, name: &str) -> Option<String> {
    let bare = name.trim_start_matches('#');
    w.conversations
        .iter()
        .find(|c| {
            c.network_id == network
                && c.kind == c::ConversationKind::Channel
                && c.party.is_none()
                && c.name.eq_ignore_ascii_case(bare)
        })
        .map(|c| c.id.clone())
}

/// A part of the channel guide's list.
#[derive(Debug, Clone, PartialEq)]
pub enum SectionKind {
    /// Channels every member joins.
    Everyone,
    /// An owner's group, by name.
    Group(String),
    /// Channels without a group.
    More,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GuideSection {
    pub kind: SectionKind,
    pub channels: Vec<c::ChannelGuideChannel>,
}

/// The guide's layout: channels for everyone first, then the owners' groups
/// in their order, then channels without a group. Empty groups are left out.
pub fn guide_sections(
    groups: &[c::ChannelGuideGroup],
    channels: &[c::ChannelGuideChannel],
) -> Vec<GuideSection> {
    use c::ChannelGuideChannelState::Everyone;
    let mut sections = Vec::new();
    let everyone: Vec<_> = channels
        .iter()
        .filter(|c| c.state == Everyone)
        .cloned()
        .collect();
    if !everyone.is_empty() {
        sections.push(GuideSection {
            kind: SectionKind::Everyone,
            channels: everyone,
        });
    }
    let rest: Vec<_> = channels.iter().filter(|c| c.state != Everyone).collect();
    let in_group = |c: &c::ChannelGuideChannel, name: &str| {
        c.group
            .as_deref()
            .is_some_and(|g| g.to_lowercase() == name.to_lowercase())
    };
    for group in groups {
        let items: Vec<_> = rest
            .iter()
            .filter(|c| in_group(c, &group.name))
            .map(|c| (*c).clone())
            .collect();
        if !items.is_empty() {
            sections.push(GuideSection {
                kind: SectionKind::Group(group.name.clone()),
                channels: items,
            });
        }
    }
    let ungrouped: Vec<_> = rest
        .iter()
        .filter(|c| !groups.iter().any(|g| in_group(c, &g.name)))
        .map(|c| (*c).clone())
        .collect();
    if !ungrouped.is_empty() {
        sections.push(GuideSection {
            kind: SectionKind::More,
            channels: ungrouped,
        });
    }
    sections
}

/// Channels ticked when the guide opens: suggested channels you are not in
/// yet. Sensitive channels never start ticked.
pub fn initial_selection(
    channels: &[c::ChannelGuideChannel],
    joined: &HashSet<String>,
) -> Vec<String> {
    channels
        .iter()
        .filter(|c| {
            c.state == c::ChannelGuideChannelState::Suggested
                && !c.nsfw
                && !joined.contains(&guide_key(&c.channel))
        })
        .map(|c| guide_key(&c.channel))
        .collect()
}

/// Whether a guide channel's box is ticked and cannot be changed: channels
/// for everyone and channels you are in.
pub fn guide_locked(channel: &c::ChannelGuideChannel, joined: &HashSet<String>) -> bool {
    channel.state == c::ChannelGuideChannelState::Everyone
        || joined.contains(&guide_key(&channel.channel))
}

/// Whether the guide has channels to show on this network.
pub fn guide_available(guide: &c::ChannelGuideView) -> bool {
    guide.available && !guide.entries.is_empty()
}

// ---- Creating a channel ----

/// ISUPPORT `CHANMODES=A,B,C,D`: list modes, modes that always take a
/// parameter, modes that take one when set, and plain switches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chanmodes {
    pub list: String,
    pub always: String,
    pub when_set: String,
    pub flags: String,
}

impl Chanmodes {
    pub fn parse(isupport: &BTreeMap<String, String>) -> Self {
        let raw = isupport
            .get("CHANMODES")
            .map(String::as_str)
            .unwrap_or("beI,k,l,imnpst");
        let mut parts = raw.split(',');
        let mut next = |default: &str| parts.next().unwrap_or(default).to_string();
        Self {
            list: next("beI"),
            always: next("k"),
            when_set: next("l"),
            flags: next("imnpst"),
        }
    }

    /// Switches offered up front: invite only, moderated, no outside
    /// messages, private, secret and topic lock, where the server has them.
    pub fn common(&self) -> Vec<char> {
        self.flags
            .chars()
            .filter(|m| "intmsp".contains(*m))
            .collect()
    }

    /// The other switches, folded away.
    pub fn advanced(&self) -> Vec<char> {
        self.flags
            .chars()
            .filter(|m| !"intmsp".contains(*m))
            .collect()
    }

    /// Modes that take a parameter (the key, the user limit, …), each once.
    pub fn parameters(&self) -> Vec<char> {
        let mut seen = Vec::new();
        for m in self.always.chars().chain(self.when_set.chars()) {
            if !seen.contains(&m) {
                seen.push(m);
            }
        }
        seen
    }
}

/// What a new channel starts as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Template {
    /// Invite only and hidden: `+isnt`.
    Private,
    /// Anyone can find and join it: `+nt`.
    Open,
    /// Only operators and voiced people talk: `+mnt`.
    Announcements,
}

impl Template {
    pub const ALL: [Template; 3] = [Template::Private, Template::Open, Template::Announcements];

    /// The switches the template wants.
    pub fn flags(self) -> &'static str {
        match self {
            Template::Private => "isnt",
            Template::Open => "nt",
            Template::Announcements => "mnt",
        }
    }

    pub fn wire(self) -> &'static str {
        match self {
            Template::Private => "private",
            Template::Open => "open",
            Template::Announcements => "announcements",
        }
    }
}

/// The name check's answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Availability {
    /// No name typed.
    Idle,
    Checking,
    Available,
    /// Somebody is in the channel already.
    Unavailable,
    /// The server could not tell.
    Unknown,
}

impl Availability {
    pub fn of(status: c::ChannelAvailabilityStatus) -> Self {
        match status {
            c::ChannelAvailabilityStatus::Available => Availability::Available,
            c::ChannelAvailabilityStatus::Unavailable => Availability::Unavailable,
            _ => Availability::Unknown,
        }
    }
}

/// The create form without its window.
#[derive(Debug, Clone, PartialEq)]
pub struct CreateForm {
    pub name: String,
    pub topic: String,
    pub template: Template,
    /// Switches to turn on.
    pub selected: String,
    /// Switches to turn off.
    pub removed: String,
    /// Parameters by mode letter (`k` the key, `l` the limit).
    pub values: BTreeMap<char, String>,
    pub modes: Chanmodes,
}

impl CreateForm {
    /// A form for a network with `isupport`, on the private template.
    pub fn new(isupport: &BTreeMap<String, String>) -> Self {
        let mut form = Self {
            name: String::new(),
            topic: String::new(),
            template: Template::Private,
            selected: String::new(),
            removed: String::new(),
            values: BTreeMap::new(),
            modes: Chanmodes::parse(isupport),
        };
        form.apply_template(Template::Private);
        form
    }

    /// Pick a template: its switches the server has go on, the other
    /// common ones off.
    pub fn apply_template(&mut self, template: Template) {
        self.template = template;
        let wanted = template.flags();
        let flags = &self.modes.flags;
        self.selected = wanted.chars().filter(|m| flags.contains(*m)).collect();
        self.removed = "imnpst"
            .chars()
            .filter(|m| flags.contains(*m) && !wanted.contains(*m))
            .collect();
    }

    /// Turn a switch on or off.
    pub fn toggle(&mut self, mode: char, on: bool) {
        self.selected.retain(|m| m != mode);
        self.removed.retain(|m| m != mode);
        if on {
            self.selected.push(mode);
        } else {
            self.removed.push(mode);
        }
    }

    pub fn is_on(&self, mode: char) -> bool {
        self.selected.contains(mode)
    }

    /// The template's switches this server does not have, as `+i, +s`.
    pub fn missing_modes(&self) -> String {
        self.template
            .flags()
            .chars()
            .filter(|m| !self.modes.flags.contains(*m))
            .map(|m| format!("+{m}"))
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// `#name`, once one is typed.
    pub fn channel(&self) -> Option<String> {
        channel_name(&self.name)
    }

    /// `+isnt-mp`.
    pub fn flags(&self) -> String {
        let mut flags = String::new();
        if !self.selected.is_empty() {
            flags.push('+');
            flags.push_str(&self.selected);
        }
        if !self.removed.is_empty() {
            flags.push('-');
            flags.push_str(&self.removed);
        }
        flags
    }

    /// The request, or `None` without a name.
    pub fn body(&self) -> Option<c::ChannelCreate> {
        let name = self.channel()?;
        let mode_params: BTreeMap<String, String> = self
            .modes
            .parameters()
            .into_iter()
            .filter_map(|m| {
                let value = self.values.get(&m)?.trim();
                (!value.is_empty()).then(|| (m.to_string(), value.to_string()))
            })
            .collect();
        let topic = self.topic.trim();
        Some(c::ChannelCreate {
            name,
            topic: (!topic.is_empty()).then(|| topic.to_string()),
            flags: Some(self.flags()),
            mode_params: Some(mode_params),
        })
    }

    /// Whether anything differs from `initial`, the form as it opened.
    pub fn dirty(&self, initial: &CreateForm) -> bool {
        let trimmed = |v: &BTreeMap<char, String>| -> BTreeMap<char, String> {
            v.iter()
                .filter(|(_, s)| !s.trim().is_empty())
                .map(|(k, s)| (*k, s.trim().to_string()))
                .collect()
        };
        self.name.trim() != initial.name.trim()
            || self.topic.trim() != initial.topic.trim()
            || self.template != initial.template
            || self.selected != initial.selected
            || self.removed != initial.removed
            || trimmed(&self.values) != trimmed(&initial.values)
    }

    /// Hide the key in a message the server sent back.
    pub fn redact_key(&self, message: &str) -> String {
        match self.values.get(&'k').map(|k| k.trim()) {
            Some(key) if !key.is_empty() => message.replace(key, "••••"),
            _ => message.to_string(),
        }
    }
}

/// Whether the network can create channels from the app: the backend needs
/// `labeled-response` to tell its own answers apart.
pub fn can_create(network: &c::Network) -> bool {
    network.capabilities.iter().any(|c| c == "labeled-response")
}

/// Whether a create answer needs the person to read it before going on:
/// it was not registered or configured as asked, or came with warnings.
pub fn partial(result: &c::ChannelCreateResult) -> bool {
    !result.registered || !result.configured || !result.warnings.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{CannedServer, Reply, SnapshotBuilder};
    use c::ChannelGuideChannelState as State;

    fn entry(channel: &str, state: State, group: Option<&str>) -> c::ChannelGuideChannel {
        c::ChannelGuideChannel {
            channel: channel.into(),
            state,
            group: group.map(str::to_string),
            ..Default::default()
        }
    }

    fn group(name: &str) -> c::ChannelGuideGroup {
        c::ChannelGuideGroup { name: name.into() }
    }

    #[test]
    fn names_and_filters() {
        assert_eq!(normalize_filter("  #rust "), "rust");
        assert_eq!(channel_name("general").as_deref(), Some("#general"));
        assert_eq!(channel_name("#general").as_deref(), Some("#general"));
        assert_eq!(channel_name(" # "), None);
    }

    #[test]
    fn guide_sections_put_everyone_first_then_groups_then_the_rest() {
        let channels = vec![
            entry("#lobby", State::Everyone, None),
            entry("#rust", State::Suggested, Some("Code")),
            entry("#go", State::Listed, Some("code")),
            entry("#music", State::Listed, Some("Gone")),
            entry("#misc", State::Listed, None),
        ];
        let sections = guide_sections(&[group("Empty"), group("Code")], &channels);
        let shape: Vec<_> = sections
            .iter()
            .map(|s| {
                (
                    s.kind.clone(),
                    s.channels
                        .iter()
                        .map(|c| c.channel.as_str())
                        .collect::<Vec<_>>(),
                )
            })
            .collect();
        assert_eq!(
            shape,
            vec![
                (SectionKind::Everyone, vec!["#lobby"]),
                (SectionKind::Group("Code".into()), vec!["#rust", "#go"]),
                (SectionKind::More, vec!["#music", "#misc"]),
            ]
        );
    }

    #[test]
    fn suggested_channels_start_ticked_but_not_sensitive_or_joined_ones() {
        let mut nsfw = entry("#late", State::Suggested, None);
        nsfw.nsfw = true;
        let channels = vec![
            entry("#Rust", State::Suggested, None),
            entry("#go", State::Suggested, None),
            entry("#misc", State::Listed, None),
            entry("#lobby", State::Everyone, None),
            nsfw,
        ];
        let joined: HashSet<String> = ["#go".to_string()].into();
        assert_eq!(initial_selection(&channels, &joined), vec!["#rust"]);
        assert!(guide_locked(&channels[1], &joined));
        assert!(guide_locked(&channels[3], &joined));
        assert!(!guide_locked(&channels[0], &joined));
    }

    #[test]
    fn joined_channels_on_a_network() {
        let w = SnapshotBuilder::new()
            .channel("c1", "General")
            .network("n2", "Other")
            .channel("c2", "elsewhere")
            .workspace();
        let n1: Vec<_> = joined(&w, "n1").into_iter().collect();
        assert_eq!(n1, vec!["#general"]);
        assert_eq!(existing(&w, "n1", "#GENERAL").as_deref(), Some("c1"));
        assert_eq!(existing(&w, "n1", "elsewhere"), None);
    }

    #[test]
    fn chanmodes_default_and_from_isupport() {
        let modes = Chanmodes::parse(&BTreeMap::new());
        assert_eq!(modes.common(), vec!['i', 'm', 'n', 'p', 's', 't']);
        assert!(modes.advanced().is_empty());
        assert_eq!(modes.parameters(), vec!['k', 'l']);
        let isupport = [(
            "CHANMODES".to_string(),
            "beIq,k,flj,CEMRScimnprstuz".to_string(),
        )]
        .into();
        let modes = Chanmodes::parse(&isupport);
        assert_eq!(modes.parameters(), vec!['k', 'f', 'l', 'j']);
        assert_eq!(
            modes.advanced(),
            vec!['C', 'E', 'M', 'R', 'S', 'c', 'r', 'u', 'z']
        );
    }

    #[test]
    fn templates_set_the_switches_the_server_has() {
        let mut form = CreateForm::new(&BTreeMap::new());
        assert_eq!(form.flags(), "+isnt-mp");
        form.apply_template(Template::Announcements);
        assert_eq!(form.flags(), "+mnt-ips");
        form.toggle('s', true);
        assert_eq!(form.flags(), "+mnts-ip");
        assert!(form.missing_modes().is_empty());
        let isupport = [("CHANMODES".to_string(), "b,k,l,nt".to_string())].into();
        let mut small = CreateForm::new(&isupport);
        assert_eq!(small.flags(), "+nt");
        assert_eq!(small.missing_modes(), "+i, +s");
        small.apply_template(Template::Open);
        assert!(small.missing_modes().is_empty());
    }

    #[test]
    fn the_request_carries_the_name_topic_flags_and_parameters() {
        let mut form = CreateForm::new(&BTreeMap::new());
        assert!(form.body().is_none());
        let initial = form.clone();
        form.name = " team ".into();
        form.topic = "  Plans ".into();
        form.values.insert('k', " secret ".into());
        form.values.insert('l', "".into());
        assert!(form.dirty(&initial));
        let body = form.body().unwrap();
        assert_eq!(body.name, "#team");
        assert_eq!(body.topic.as_deref(), Some("Plans"));
        assert_eq!(body.flags.as_deref(), Some("+isnt-mp"));
        assert_eq!(
            body.mode_params.unwrap(),
            BTreeMap::from([("k".to_string(), "secret".to_string())])
        );
        assert_eq!(form.redact_key("bad key secret"), "bad key ••••");
        let mut blank = initial.clone();
        blank.values.insert('k', "  ".into());
        assert!(!blank.dirty(&initial));
    }

    #[test]
    fn creating_needs_labeled_response() {
        let mut network = crate::testing::network("n1", "Home");
        assert!(!can_create(&network));
        network.capabilities.push("labeled-response".into());
        assert!(can_create(&network));
    }

    #[test]
    fn calls_reach_the_server() {
        let rt = crate::client::runtime().unwrap();
        let server = CannedServer::start(&rt);
        server.route_fn("GET", "/api/v1/networks/n1/channels", |request| {
            assert_eq!(request.query_param("q").as_deref(), Some("ru"));
            assert_eq!(request.query_param("limit").as_deref(), Some("50"));
            Reply::json(
                200,
                serde_json::json!({"items": [{"name": "#rust", "topic": "Crabs", "users": 3}]}),
            )
        });
        server.route(
            "GET",
            "/api/v1/networks/n1/channels/availability",
            200,
            serde_json::json!({"status": "unknown"}),
        );
        server.route(
            "POST",
            "/api/v1/networks/n1/channel-guide/join",
            200,
            serde_json::json!({"joined": [], "failed": [{"channel": "#x", "reason": "banned"}],
                               "queued": []}),
        );
        let api = server.api();
        let list = rt.block_on(api.channel_list("n1", "ru")).unwrap();
        assert_eq!(list[0].users, 3);
        let status = rt
            .block_on(api.channel_availability("n1", "#rust"))
            .unwrap();
        assert_eq!(Availability::of(status), Availability::Unknown);
        let joined = rt
            .block_on(api.join_from_guide("n1", &["#x".into()]))
            .unwrap();
        assert_eq!(joined.failed[0].reason, "banned");
        let sent = server.requests();
        let join = sent
            .iter()
            .find(|r| r.path.ends_with("/channel-guide/join"))
            .unwrap();
        assert_eq!(join.json()["channels"], serde_json::json!(["#x"]));
    }
}

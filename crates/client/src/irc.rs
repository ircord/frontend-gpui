//! IRC privileges from ISUPPORT and raw-line rules, ported from the web
//! client's `shared/utils/irc.ts` (frontend `48c500f`). Formatting codes are
//! in [`crate::format`].

use std::collections::BTreeMap;

use ircord_contract::{Conversation, ConversationKind, Network, NetworkStatus};

/// The wire target of a conversation, which may differ from its display name.
pub fn channel_target(conversation: &Conversation) -> String {
    conversation
        .target
        .clone()
        .unwrap_or_else(|| format!("#{}", conversation.name))
}

/// A network's channel prefixes: `PREFIX=(qaohv)~&@%+` gives modes `qaohv`
/// and prefixes `~&@%+`, highest first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prefixes {
    pub modes: Vec<char>,
    pub prefixes: Vec<char>,
}

impl Prefixes {
    /// The network's `PREFIX`, or the usual `(qaohv)~&@%+` when it has none or a broken one.
    pub fn of(isupport: &BTreeMap<String, String>) -> Self {
        isupport
            .get("PREFIX")
            .and_then(|p| Self::parse(p))
            .unwrap_or_else(|| Self::parse("(qaohv)~&@%+").expect("the default parses"))
    }

    fn parse(prefix: &str) -> Option<Self> {
        let rest = prefix.strip_prefix('(')?;
        let (modes, prefixes) = rest.split_once(')')?;
        let modes: Vec<char> = modes.chars().collect();
        let prefixes: Vec<char> = prefixes.chars().collect();
        let valid = !modes.is_empty()
            && modes.len() == prefixes.len()
            && modes.iter().all(char::is_ascii_alphabetic)
            && !prefixes.iter().any(|c| c.is_whitespace());
        valid.then_some(Self { modes, prefixes })
    }

    /// The rank of a member's prefix characters (0 = highest), or `None` without any.
    pub fn rank(&self, prefix: &str) -> Option<usize> {
        prefix
            .chars()
            .filter_map(|c| self.prefixes.iter().position(|p| *p == c))
            .min()
    }

    /// The rank of a channel mode (`o`, `v`...), or `None` if the network has no such prefix.
    pub fn mode_rank(&self, mode: char) -> Option<usize> {
        self.modes.iter().position(|m| *m == mode)
    }
}

/// The rank of a prefix on a network (0 = highest), or `None` without any.
pub fn prefix_rank(prefix: &str, network: Option<&Network>) -> Option<usize> {
    prefixes(network).rank(prefix)
}

fn prefixes(network: Option<&Network>) -> Prefixes {
    network.map_or_else(
        || Prefixes::of(&BTreeMap::new()),
        |n| Prefixes::of(&n.isupport),
    )
}

/// The modes a member key has in a channel (`""` when none).
pub fn member_modes<'a>(member_id: &str, conversation: Option<&'a Conversation>) -> &'a str {
    conversation
        .and_then(|c| c.member_modes.get(member_id))
        .map_or("", String::as_str)
}

/// The highest prefix character a member has in a channel.
pub fn top_prefix(
    member_id: &str,
    conversation: Option<&Conversation>,
    network: Option<&Network>,
) -> Option<char> {
    let own = member_modes(member_id, conversation);
    prefixes(network)
        .prefixes
        .into_iter()
        .find(|c| own.contains(*c))
}

/// What a member may do in a channel. Halfop (`%`) and above can kick, ban
/// and voice; op (`@`) and above can also change ops. Setting the topic needs
/// halfop only when the channel is `+t`, inviting only when it is `+i`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ChannelRights {
    pub kick: bool,
    pub ban: bool,
    pub voice: bool,
    pub op: bool,
    pub invite: bool,
    pub topic: bool,
}

/// The rights of the member whose modes are `modes` in `conversation`.
pub fn channel_rights_for(
    modes: &str,
    conversation: Option<&Conversation>,
    network: Option<&Network>,
) -> ChannelRights {
    let p = prefixes(network);
    let rank = p.rank(modes);
    let at =
        |mode: char| matches!((p.mode_rank(mode), rank), (Some(need), Some(have)) if have <= need);
    let halfop = at('h') || at('o');
    let op = at('o');
    let channel = conversation
        .is_some_and(|c| c.kind == ConversationKind::Channel && c.party.is_none() && c.joined)
        && network.is_none_or(|n| n.status == NetworkStatus::Connected);
    let channel_mode = |m: char| conversation.is_some_and(|c| c.modes.contains(m));
    ChannelRights {
        kick: channel && halfop,
        ban: channel && halfop,
        voice: channel && halfop,
        op: channel && op,
        invite: channel && (!channel_mode('i') || halfop),
        topic: channel && (!channel_mode('t') || halfop),
    }
}

/// The rights of `member_id` in `conversation`.
pub fn channel_rights(
    member_id: &str,
    conversation: Option<&Conversation>,
    network: Option<&Network>,
) -> ChannelRights {
    channel_rights_for(member_modes(member_id, conversation), conversation, network)
}

/// One raw IRC line from the user: not blank, no line breaks or NUL, at most 510 bytes.
pub fn valid_raw_line(line: &str) -> bool {
    !line.trim().is_empty() && !line.contains(['\r', '\n', '\0']) && line.len() <= 510
}

#[cfg(test)]
mod tests {
    use super::*;

    fn network(prefix: Option<&str>) -> Network {
        let mut n = Network {
            status: NetworkStatus::Connected,
            ..Default::default()
        };
        if let Some(p) = prefix {
            n.isupport.insert("PREFIX".into(), p.into());
        }
        n
    }

    fn channel(modes: &str, member: &str, prefix: &str) -> Conversation {
        let mut c = Conversation {
            kind: ConversationKind::Channel,
            name: "alpha".into(),
            joined: true,
            modes: modes.into(),
            ..Default::default()
        };
        c.member_modes.insert(member.into(), prefix.into());
        c
    }

    #[test]
    fn prefixes_come_from_isupport_with_a_fallback() {
        let p = Prefixes::of(&network(Some("(ov)@+")).isupport);
        assert_eq!(p.modes, ['o', 'v']);
        assert_eq!(p.rank("+@"), Some(0));
        assert_eq!(p.rank(""), None);
        // Mismatched lengths fall back to the usual set.
        let p = Prefixes::of(&network(Some("(ov)@")).isupport);
        assert_eq!(p.prefixes, ['~', '&', '@', '%', '+']);
        assert_eq!(prefix_rank("%", None), Some(3));
    }

    #[test]
    fn rights_follow_rank_and_channel_modes() {
        let n = network(None);
        let c = channel("nt", "me", "%");
        let r = channel_rights("me", Some(&c), Some(&n));
        assert!(r.kick && r.ban && r.voice && r.topic && r.invite);
        assert!(!r.op);
        let c = channel("nt", "me", "@");
        assert!(channel_rights("me", Some(&c), Some(&n)).op);
        // A plain member may set the topic only without +t, invite only without +i.
        let c = channel("i", "me", "");
        let r = channel_rights("me", Some(&c), Some(&n));
        assert!(r.topic && !r.invite && !r.kick);
        // Nothing in a channel you have left, or on a network that is down.
        let mut c = channel("", "me", "~");
        c.joined = false;
        assert_eq!(
            channel_rights("me", Some(&c), Some(&n)),
            ChannelRights::default()
        );
        let mut down = network(None);
        down.status = NetworkStatus::Disconnected;
        let c = channel("", "me", "~");
        assert!(!channel_rights("me", Some(&c), Some(&down)).op);
        assert_eq!(top_prefix("me", Some(&c), Some(&n)), Some('~'));
    }

    #[test]
    fn targets_and_raw_lines() {
        let mut c = channel("", "me", "");
        assert_eq!(channel_target(&c), "#alpha");
        c.target = Some("&local".into());
        assert_eq!(channel_target(&c), "&local");
        assert!(valid_raw_line("PRIVMSG #a :hi"));
        assert!(!valid_raw_line("  "));
        assert!(!valid_raw_line("A\r\nQUIT"));
        assert!(!valid_raw_line(&"x".repeat(511)));
    }
}

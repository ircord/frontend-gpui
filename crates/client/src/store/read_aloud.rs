//! Which live highlights this device may read aloud.

use ircord_contract::{
    ChannelSettingsGroupMentions, Conversation, ConversationKind, MemberStatus, Message,
    MessageKind, Network, QuietHours,
};
use jiff::{Timestamp, civil::Time, tz::TimeZone};

use super::{SELF_ID, Workspace, notify_settings, people, sidebar_order};
use crate::format::mentions::{Group, groups_in, is_highlight};
use crate::{format, irc};

/// A highlight to announce. It holds only what the voice needs to say.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Highlight {
    pub sender: Option<String>,
    pub channel: Option<String>,
    pub text: String,
    pub gif: bool,
}

/// Qualify a message under the same rules as the account's notifications.
pub fn incoming(w: &Workspace, message: &Message, now_ms: i64) -> Option<Highlight> {
    if message.kind == MessageKind::Event
        || message.deleted == Some(true)
        || message.historical == Some(true)
        || message.text.trim().is_empty()
    {
        return None;
    }
    let conversation = w.conversation(&message.conversation_id)?;
    let network = w.network(&conversation.network_id)?;
    let settings = w.documents.notification_settings.value()?;
    if w.is_self(&message.sender_id, &network.id)
        || sidebar_order::muted(w, conversation)
        || quiet(&settings.quiet_hours, now_ms)
    {
        return None;
    }
    let sender = w.member(&message.sender_id, &network.id);
    if let Some(ignore) = settings.ignore.as_deref()
        && sender.is_some_and(|m| people::is_ignored(ignore, m))
    {
        return None;
    }
    let nick = people::self_nick(w, &network.id);
    let case_mapping = network
        .isupport
        .get("CASEMAPPING")
        .map_or("rfc1459", String::as_str);
    if !is_highlight(
        &message.text,
        &nick,
        case_mapping,
        notify_settings::highlight_words(settings),
    ) && !group_highlight(w, network, conversation, message)
    {
        return None;
    }
    Some(Highlight {
        sender: sender
            .map(|s| s.nick.clone())
            .filter(|s| !s.trim().is_empty()),
        channel: sidebar_order::channel_wide(conversation).then(|| {
            conversation
                .target
                .clone()
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| format!("#{}", conversation.name))
        }),
        text: shorten_links(&message.text),
        gif: message.gif.is_some(),
    })
}

fn quiet(quiet: &QuietHours, now_ms: i64) -> bool {
    if !quiet.enabled || quiet.start == quiet.end {
        return false;
    }
    let (Ok(zone), Ok(now), Ok(start), Ok(end)) = (
        TimeZone::get(&quiet.time_zone),
        Timestamp::from_millisecond(now_ms),
        quiet.start.parse::<Time>(),
        quiet.end.parse::<Time>(),
    ) else {
        return true;
    };
    let current = now.to_zoned_datetime(zone).time();
    if start < end {
        (start..end).contains(&current)
    } else {
        current >= start || current < end
    }
}

fn group_highlight(w: &Workspace, network: &Network, c: &Conversation, m: &Message) -> bool {
    if c.kind != ConversationKind::Channel {
        return false;
    }
    let mentions = groups_in(&format::strip(&m.text));
    if mentions.is_empty() {
        return false;
    }
    let prefixes = irc::Prefixes::of(&network.isupport);
    let at = |id, mode| {
        let rank = prefixes.rank(irc::member_modes(id, Some(c)));
        matches!((rank, prefixes.mode_rank(mode)), (Some(have), Some(need)) if have <= need)
    };
    let wide = match c.channel_settings.as_ref().map(|s| s.group_mentions) {
        Some(ChannelSettingsGroupMentions::All) => true,
        Some(ChannelSettingsGroupMentions::Off) => false,
        _ => at(&m.sender_id, 'o'),
    };
    let away = w
        .member(SELF_ID, &network.id)
        .is_some_and(|m| m.status == MemberStatus::Away);
    mentions.into_iter().any(|(_, group)| match group {
        Group::Everyone => wide,
        Group::Here | Group::Channel => wide && !away,
        Group::Owner => at(SELF_ID, 'q'),
        Group::Operator => at(SELF_ID, 'o'),
        Group::Voiced => at(SELF_ID, 'v'),
    })
}

fn shorten_links(text: &str) -> String {
    let mut parsed = format::parse(text);
    for (range, link) in parsed.links.into_iter().rev() {
        if let Ok(url) = url::Url::parse(&link)
            && matches!(url.scheme(), "http" | "https")
            && let Some(host) = url.host_str()
        {
            parsed
                .text
                .replace_range(range, &format!("a link to {host}"));
        }
    }
    parsed.text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quiet_hours_wrap_midnight_in_the_saved_time_zone() {
        let hours = QuietHours {
            enabled: true,
            start: "22:00".into(),
            end: "07:00".into(),
            time_zone: "Europe/Copenhagen".into(),
        };
        let at = |s: &str| s.parse::<Timestamp>().unwrap().as_millisecond();
        assert!(quiet(&hours, at("2026-11-22T21:30:00Z")));
        assert!(!quiet(&hours, at("2026-11-23T06:30:00Z")));
    }

    #[test]
    fn speech_excludes_link_paths_queries_and_irc_formatting() {
        assert_eq!(
            shorten_links("\x02hello\x02  https://example.test/private?key=secret"),
            "hello a link to example.test"
        );
    }
}

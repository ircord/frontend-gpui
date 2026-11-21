//! A conversation as a plain-text IRC log, the way irssi or mIRC write them
//! and the web client exports them: `[time] <nick> text`, `* nick action`,
//! `-nick- notice`, `*** event`.

use chrono::{Local, TimeZone};
use ircord_client::contract::{Message, MessageKind};
use ircord_client::store::Workspace;

use super::rows;

/// The most messages one export reads, as the web client does.
pub const MAX_EXPORT: usize = 20_000;

/// Local time as `2026-11-07 10:13`.
pub fn log_time(ms: i64) -> String {
    Local
        .timestamp_millis_opt(ms)
        .single()
        .map(|t| t.format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_default()
}

/// One message as a log line.
pub fn line(w: &Workspace, m: &Message, network: &str) -> String {
    let at = format!(
        "[{}]",
        log_time(ircord_client::store::parse_time(&m.created_at))
    );
    if m.kind == MessageKind::Event || m.event.is_some() {
        return format!("{at} *** {}", rows::event_text(w, m, network));
    }
    let nick = w
        .members
        .get(&m.sender_id)
        .map(|p| p.nick.clone())
        .unwrap_or_else(|| w.display_name(&m.sender_id, network));
    let source = m.gif.as_ref().map_or(m.text.as_str(), |g| g.url.as_str());
    let text = ircord_client::format::parse(source)
        .text
        .replace("\r\n", " ↵ ")
        .replace('\n', " ↵ ");
    let extra: String = m
        .attachments
        .iter()
        .flatten()
        .chain(m.attachment.iter())
        .filter(|a| !text.contains(&a.url))
        .map(|a| format!(" {}", a.url))
        .collect();
    match m.kind {
        MessageKind::Action => format!("{at} * {nick} {text}{extra}"),
        MessageKind::Notice => format!("{at} -{nick}- {text}{extra}"),
        _ => format!("{at} <{nick}> {text}{extra}"),
    }
}

/// The whole log: a heading, a blank line, then one line per message,
/// oldest first. Deleted messages are left out.
pub fn format_log(w: &Workspace, messages: &[Message], network: &str, heading: &str) -> String {
    let mut lines = vec![heading.to_string(), String::new()];
    lines.extend(
        messages
            .iter()
            .filter(|m| m.deleted != Some(true) && m.kind != MessageKind::System)
            .map(|m| line(w, m, network)),
    );
    lines.join("\n") + "\n"
}

/// `home_general_2026-11-21.log`.
pub fn file_name(network: &str, conversation: &str, today: &str) -> String {
    let slug = |s: &str| {
        let s: String = s
            .to_lowercase()
            .chars()
            .map(|c| if c.is_alphanumeric() { c } else { '-' })
            .collect();
        let s = s.trim_matches('-').to_string();
        if s.is_empty() { "irc".to_string() } else { s }
    };
    format!(
        "{}_{}_{today}.log",
        slug(network),
        slug(conversation.trim_start_matches('#'))
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use ircord_client::contract::*;

    #[test]
    fn names_the_file_after_network_conversation_and_day() {
        assert_eq!(
            file_name("Libera Chat", "#rust", "2026-11-21"),
            "libera-chat_rust_2026-11-21.log"
        );
        assert_eq!(file_name("", "bob", "2026-11-21"), "irc_bob_2026-11-21.log");
    }

    #[test]
    fn writes_irc_log_lines() {
        let snapshot: Snapshot = serde_json::from_value(serde_json::json!({
            "version": 1, "cursor": 0, "demo": false,
            "user": {"id": "u-me", "account": "me", "nick": "me", "displayName": "",
                     "bio": "", "color": "", "role": "member", "status": "online"},
            "capabilities": {}, "invitations": [],
            "members": [{"id": "m-bob", "account": "bob", "nick": "bob", "displayName": "Bob B",
                         "bio": "", "color": "", "role": "member", "status": "online"}],
            "networks": [], "conversations": []
        }))
        .unwrap();
        let w = Workspace::from_snapshot(snapshot);
        let m = |id: &str, kind: MessageKind, text: &str| Message {
            id: id.into(),
            conversation_id: "c".into(),
            created_at: "2026-11-20T12:00:00Z".into(),
            kind,
            sender_id: "m-bob".into(),
            text: text.into(),
            ..Default::default()
        };
        let mut join = m("4", MessageKind::Event, "");
        join.event = Some(MembershipEvent {
            actor_id: "m-bob".into(),
            r#type: MembershipEventType::Join,
            ..Default::default()
        });
        let mut gone = m("5", MessageKind::Message, "secret");
        gone.deleted = Some(true);
        let log = format_log(
            &w,
            &[
                m("1", MessageKind::Message, "hi \x02there\x02\nfriend"),
                m("2", MessageKind::Action, "waves"),
                m("3", MessageKind::Notice, "note"),
                join,
                gone,
            ],
            "n1",
            "Log of #x",
        );
        let lines: Vec<&str> = log.lines().collect();
        assert_eq!(lines[0], "Log of #x");
        assert_eq!(lines[1], "");
        assert!(
            lines[2].ends_with("] <bob> hi there ↵ friend"),
            "{}",
            lines[2]
        );
        assert!(lines[3].ends_with("] * bob waves"));
        assert!(lines[4].ends_with("] -bob- note"));
        assert!(lines[5].ends_with("] *** Bob B joined"), "{}", lines[5]);
        assert_eq!(lines.len(), 6, "the deleted message is left out");
        assert!(lines[2].starts_with("[2026-11-20 "));
    }
}

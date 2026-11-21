//! Links that open inside the app: links to one message on this server
//! (`shared/utils/message-links.ts`) and `irc://` links
//! (`shared/utils/irc-url.ts`, draft-butcher-irc-url).

use url::Url;

/// A link to one message: `/channel/<id>?m=<msgId>` or `/dm/<id>?m=<msgId>`
/// on the server's own origin. Opening one finds the message by its ID or by
/// its shared ID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageLink {
    pub conversation_id: String,
    pub message_id: String,
}

/// The conversation and message a URL points to, when it is a message link
/// on `origin`.
pub fn parse_message_link(raw: &str, origin: &str) -> Option<MessageLink> {
    let origin = Url::parse(origin).ok()?;
    let url = origin.join(raw).ok()?;
    if url.origin() != origin.origin() {
        return None;
    }
    let mut segments = url.path_segments()?.filter(|s| !s.is_empty());
    let kind = segments.next()?;
    let id = segments.next()?;
    if !matches!(kind, "channel" | "dm") || segments.next().is_some() {
        return None;
    }
    // A trailing slash is allowed, anything more is another page.
    if url.path().trim_end_matches('/').matches('/').count() != 2 {
        return None;
    }
    let message_id = url
        .query_pairs()
        .find(|(k, _)| k == "m")
        .map(|(_, v)| v.into_owned())
        .filter(|v| !v.is_empty())?;
    let conversation_id = percent_decode(id)?;
    (!conversation_id.is_empty()).then_some(MessageLink {
        conversation_id,
        message_id,
    })
}

fn percent_decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = s.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// An `irc://` or `ircs://` link: `irc://host[:port]/target[,flags][?key]`.
/// Targets are channels (with or without `#`), or nicks with the `isnick` flag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IrcLink {
    pub host: String,
    pub port: u16,
    pub tls: bool,
    /// The channel name with its prefix, such as `#rust`.
    pub channel: Option<String>,
    pub key: Option<String>,
    pub nick: Option<String>,
}

/// Parses an `irc(s)://` URL; `None` when it isn't one or is malformed.
pub fn parse_irc_url(value: &str) -> Option<IrcLink> {
    let value = value.trim();
    let lower = value.to_ascii_lowercase();
    let (tls, rest) = if lower.starts_with("ircs://") {
        (true, &value[7..])
    } else if lower.starts_with("irc://") {
        (false, &value[6..])
    } else {
        return None;
    };
    if rest.contains(char::is_whitespace) {
        return None;
    }
    let host_end = rest.find(['/', ':', '?', '#']).unwrap_or(rest.len());
    let host = rest[..host_end].to_ascii_lowercase();
    if host.is_empty()
        || host.len() > 253
        || !host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
    {
        return None;
    }
    let mut rest = &rest[host_end..];
    let mut port = if tls { 6697 } else { 6667 };
    if let Some(after) = rest.strip_prefix(':') {
        let digits = after.bytes().take_while(u8::is_ascii_digit).count();
        if digits == 0 || digits > 5 {
            return None;
        }
        let n: u32 = after[..digits].parse().ok()?;
        if !(1..=65535).contains(&n) {
            return None;
        }
        port = n as u16;
        rest = &after[digits..];
    }
    let mut target = "";
    if let Some(after) = rest.strip_prefix('/') {
        let end = after.find('?').unwrap_or(after.len());
        target = &after[..end];
        rest = &after[end..];
    }
    let query = match rest {
        "" => None,
        q if q.starts_with('?') => Some(&q[1..]),
        _ => return None,
    };
    let mut link = IrcLink {
        host,
        port,
        tls,
        channel: None,
        key: None,
        nick: None,
    };
    let target = percent_decode(target)?;
    if !target.is_empty() {
        let mut parts = target.split(',');
        let name = parts.next().unwrap_or_default();
        if parts.any(|f| f == "isnick") {
            link.nick = Some(name.to_string());
        } else if !name.is_empty() {
            link.channel = Some(if name.starts_with(['#', '&', '!', '+']) {
                name.to_string()
            } else {
                format!("#{name}")
            });
        }
    }
    if let Some(q) = query.filter(|q| !q.is_empty())
        && link.channel.is_some()
    {
        link.key = Some(percent_decode(q)?);
    }
    Some(link)
}

/// A network name from a host: `irc.libera.chat` → `libera.chat`.
pub fn network_name_from_host(host: &str) -> &str {
    host.strip_prefix("irc.")
        .or_else(|| host.strip_prefix("chat."))
        .unwrap_or(host)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ORIGIN: &str = "https://chat.example";

    #[test]
    fn message_links_on_this_server() {
        assert_eq!(
            parse_message_link("https://chat.example/channel/c%201?m=net:abc", ORIGIN),
            Some(MessageLink {
                conversation_id: "c 1".into(),
                message_id: "net:abc".into()
            })
        );
        assert!(parse_message_link("https://chat.example/dm/d1/?m=x", ORIGIN).is_some());
        assert!(parse_message_link("https://other.example/channel/c1?m=x", ORIGIN).is_none());
        assert!(parse_message_link("https://chat.example/channel/c1", ORIGIN).is_none());
        assert!(parse_message_link("https://chat.example/channel/c1/x?m=y", ORIGIN).is_none());
        assert!(parse_message_link("https://chat.example/search?m=y", ORIGIN).is_none());
    }

    #[test]
    fn irc_urls() {
        let l = parse_irc_url("ircs://irc.libera.chat/rust?secret").unwrap();
        assert_eq!(
            (l.host.as_str(), l.port, l.tls),
            ("irc.libera.chat", 6697, true)
        );
        assert_eq!(l.channel.as_deref(), Some("#rust"));
        assert_eq!(l.key.as_deref(), Some("secret"));
        let l = parse_irc_url("irc://Example.org:7000/%23dev").unwrap();
        assert_eq!((l.host.as_str(), l.port), ("example.org", 7000));
        assert_eq!(l.channel.as_deref(), Some("#dev"));
        let l = parse_irc_url("irc://example.org/bob,isnick").unwrap();
        assert_eq!(l.nick.as_deref(), Some("bob"));
        assert_eq!(l.channel, None);
        assert!(
            parse_irc_url("irc://example.org")
                .unwrap()
                .channel
                .is_none()
        );
        assert!(parse_irc_url("irc://bad host/x").is_none());
        assert!(parse_irc_url("irc://h:70000/x").is_none());
        assert!(parse_irc_url("https://x/y").is_none());
        assert_eq!(network_name_from_host("irc.libera.chat"), "libera.chat");
    }
}

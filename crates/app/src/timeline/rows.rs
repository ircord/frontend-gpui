//! The timeline as rows: day separators, messages (grouped under one header
//! when the same person writes again within a few minutes), join/part and
//! other event rows, and sends still on their way. Pure, so it is tested
//! without a window.

use std::ops::Range;
use std::rc::Rc;

use chrono::{DateTime, Local, TimeZone};
use ircord_client::contract::{MembershipEventType, Message, MessageKind};
use ircord_client::format::{self, Formatted};
use ircord_client::store::{PendingSend, SendState, Workspace};

/// Messages from one sender within this many milliseconds share a header.
const GROUP_MS: i64 = 7 * 60 * 1000;

#[derive(Debug, Clone, PartialEq)]
pub enum Row {
    /// The start of the history: nothing older to load.
    Start,
    /// Older history is loading.
    Loading,
    Day(String),
    Message(Rc<MessageRow>),
    /// Join, part, quit, nick, topic and similar lines.
    Event(Rc<EventRow>),
    Pending(Rc<PendingSend>),
}

impl Row {
    /// Stable identity, for splicing the virtual list.
    pub fn key(&self) -> String {
        match self {
            Row::Start => "start".into(),
            Row::Loading => "loading".into(),
            Row::Day(day) => format!("day:{day}"),
            Row::Message(m) => format!("m:{}:{}", m.id, m.version),
            Row::Event(e) => format!("e:{}", e.id),
            Row::Pending(p) => format!("p:{}:{:?}", p.client_id, p.state),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct MessageRow {
    pub id: String,
    /// Changes when the message is edited or its reactions change.
    pub version: u64,
    /// Show the sender and time above this message.
    pub header: bool,
    pub sender: String,
    pub sender_color: Option<u32>,
    pub time: String,
    pub action: bool,
    pub notice: bool,
    pub own: bool,
    pub body: Formatted,
    pub edited: bool,
    pub deleted: bool,
    pub reply: Option<(String, String)>,
    pub reactions: Vec<(String, usize)>,
    pub images: Vec<Image>,
    /// Other attachments and links: each a row that opens the browser.
    pub links: Vec<LinkCard>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Image {
    pub url: String,
    pub width: u32,
    pub height: u32,
    pub alt: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LinkCard {
    pub url: String,
    pub title: String,
    pub detail: String,
    pub video: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EventRow {
    pub id: String,
    pub text: String,
    pub time: String,
}

pub fn build(w: &Workspace, conversation: &str) -> Vec<Row> {
    let Some(timeline) = w.timelines.get(conversation) else {
        return vec![Row::Loading];
    };
    let network = w
        .conversation(conversation)
        .map(|c| c.network_id.clone())
        .unwrap_or_default();
    let mut rows = Vec::with_capacity(timeline.entries.len() + 8);
    if timeline.loading_older || !timeline.loaded {
        rows.push(Row::Loading);
    } else if timeline.older.is_none() {
        rows.push(Row::Start);
    }
    let mut last_day = String::new();
    let mut last: Option<(String, i64)> = None;
    for entry in &timeline.entries {
        let m = &entry.message;
        let local = local_time(entry.at);
        let day = local.format("%A, %-d %B %Y").to_string();
        if day != last_day {
            rows.push(Row::Day(day.clone()));
            last_day = day;
            last = None;
        }
        let time = local.format("%H:%M").to_string();
        if m.kind == MessageKind::Event || m.kind == MessageKind::System || m.event.is_some() {
            rows.push(Row::Event(Rc::new(EventRow {
                id: m.id.clone(),
                text: event_text(w, m, &network),
                time,
            })));
            last = None;
            continue;
        }
        let header = match &last {
            Some((sender, at)) => {
                *sender != m.sender_id || entry.at - at > GROUP_MS || m.kind == MessageKind::Action
            }
            None => true,
        };
        last = Some((m.sender_id.clone(), entry.at));
        rows.push(Row::Message(Rc::new(message_row(
            w, m, &network, header, time,
        ))));
    }
    // Sends the server has not echoed yet sit at the end.
    for pending in w.pending_in(conversation) {
        let echoed = timeline
            .entries
            .iter()
            .any(|e| e.message.client_id.as_deref() == Some(pending.client_id.as_str()));
        if !echoed {
            rows.push(Row::Pending(Rc::new(pending.clone())));
        }
    }
    rows
}

fn local_time(ms: i64) -> DateTime<Local> {
    Local
        .timestamp_millis_opt(ms)
        .single()
        .unwrap_or_else(|| Local.timestamp_millis_opt(0).unwrap())
}

fn message_row(
    w: &Workspace,
    m: &Message,
    network: &str,
    header: bool,
    time: String,
) -> MessageRow {
    let member = w.member(&m.sender_id, network);
    let sender_color = member
        .and_then(|m| m.name_color.as_deref().or(Some(m.color.as_str())))
        .and_then(parse_hex);
    let body = format::parse(&m.text);
    let mut images = Vec::new();
    let mut links = Vec::new();
    let attachments = m
        .attachments
        .clone()
        .unwrap_or_default()
        .into_iter()
        .chain(m.attachment.clone());
    for a in attachments {
        let content_type = a.content_type.clone().unwrap_or_default();
        if content_type.starts_with("image/") || looks_like_image(&a.url) {
            images.push(Image {
                url: a.url.clone(),
                width: a.width.max(0) as u32,
                height: a.height.max(0) as u32,
                alt: if a.alt.is_empty() {
                    a.name.clone()
                } else {
                    a.alt.clone()
                },
            });
        } else {
            links.push(LinkCard {
                url: a.url.clone(),
                title: a.name.clone(),
                detail: describe_file(&content_type, a.size),
                video: content_type.starts_with("video/"),
            });
        }
    }
    if let Some(gif) = &m.gif {
        images.push(Image {
            url: gif.url.clone(),
            width: gif.width.max(0) as u32,
            height: gif.height.max(0) as u32,
            alt: gif.title.clone(),
        });
    }
    for (_, url) in &body.links {
        if images.iter().any(|i| &i.url == url) || links.iter().any(|l| &l.url == url) {
            continue;
        }
        if looks_like_image(url) {
            let geometry = m
                .embed_geometry
                .as_ref()
                .and_then(|g| g.iter().find(|g| &g.url == url));
            images.push(Image {
                url: url.clone(),
                width: geometry.map_or(0, |g| g.width.max(0) as u32),
                height: geometry.map_or(0, |g| g.height.max(0) as u32),
                alt: String::new(),
            });
            continue;
        }
        let video = is_video(url);
        links.push(LinkCard {
            url: url.clone(),
            title: host(url),
            detail: if video {
                t!("timeline-link-video").into()
            } else {
                t!("timeline-link-opens").into()
            },
            video,
        });
    }
    let version = {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        m.text.hash(&mut h);
        m.edited_at.hash(&mut h);
        m.deleted.hash(&mut h);
        for r in &m.reactions {
            r.emoji.hash(&mut h);
            r.member_ids.len().hash(&mut h);
        }
        header.hash(&mut h);
        h.finish()
    };
    MessageRow {
        id: m.id.clone(),
        version,
        header,
        sender: w.display_name(&m.sender_id, network),
        sender_color,
        time,
        action: m.kind == MessageKind::Action,
        notice: m.kind == MessageKind::Notice,
        own: w.is_self(&m.sender_id, network),
        body,
        edited: m.edited_at.is_some(),
        deleted: m.deleted == Some(true),
        reply: m
            .reply_to
            .as_ref()
            .map(|r| (r.nick.clone(), r.text.clone())),
        reactions: m
            .reactions
            .iter()
            .filter(|r| !r.member_ids.is_empty())
            .map(|r| (r.emoji.clone(), r.member_ids.len()))
            .collect(),
        images,
        links,
    }
}

fn event_text(w: &Workspace, m: &Message, network: &str) -> String {
    let Some(e) = &m.event else {
        return ircord_client::format::parse(&m.text).text;
    };
    let actor = w.display_name(&e.actor_id, network);
    let target = e
        .target_id
        .as_deref()
        .map(|t| w.display_name(t, network))
        .unwrap_or_default();
    let detail = e.text.clone().unwrap_or_default();
    let with = |s: String| {
        if detail.is_empty() {
            s
        } else {
            t!("timeline-event-with-reason", event = s, reason = detail)
        }
    };
    match e.r#type {
        MembershipEventType::Join => t!("timeline-event-join", actor = actor),
        MembershipEventType::Part => with(t!("timeline-event-part", actor = actor)),
        MembershipEventType::Quit => with(t!("timeline-event-quit", actor = actor)),
        MembershipEventType::Nick => t!(
            "timeline-event-nick",
            actor = actor,
            nick = if target.is_empty() {
                detail.clone()
            } else {
                target
            }
        ),
        MembershipEventType::Kick => {
            with(t!("timeline-event-kick", actor = actor, target = target))
        }
        MembershipEventType::Mode => t!("timeline-event-mode", actor = actor, modes = detail)
            .trim_end()
            .to_string(),
        MembershipEventType::Topic => t!("timeline-event-topic", actor = actor, topic = detail),
        MembershipEventType::Invite => {
            t!("timeline-event-invite", actor = actor, target = target)
        }
        _ => {
            let text = ircord_client::format::parse(&m.text).text;
            if text.is_empty() { with(actor) } else { text }
        }
    }
}

fn parse_hex(s: &str) -> Option<u32> {
    let s = s.trim().trim_start_matches('#');
    (s.len() == 6)
        .then(|| u32::from_str_radix(s, 16).ok())
        .flatten()
}

fn path_of(url: &str) -> String {
    url.split(['?', '#'])
        .next()
        .unwrap_or("")
        .to_ascii_lowercase()
}

pub fn looks_like_image(url: &str) -> bool {
    let path = path_of(url);
    [".png", ".jpg", ".jpeg", ".gif", ".webp", ".bmp"]
        .iter()
        .any(|ext| path.ends_with(ext))
}

fn is_video(url: &str) -> bool {
    let path = path_of(url);
    let host = host(url);
    [".mp4", ".webm", ".mov", ".mkv"]
        .iter()
        .any(|e| path.ends_with(e))
        || [
            "youtube.com",
            "youtu.be",
            "twitch.tv",
            "vimeo.com",
            "tiktok.com",
            "streamable.com",
        ]
        .iter()
        .any(|h| host == *h || host.ends_with(&format!(".{h}")))
}

fn host(url: &str) -> String {
    url::Url::parse(url)
        .ok()
        .and_then(|u| {
            u.host_str()
                .map(|h| h.trim_start_matches("www.").to_string())
        })
        .unwrap_or_else(|| url.to_string())
}

fn describe_file(content_type: &str, size: Option<i64>) -> String {
    let kind = if content_type.starts_with("video/") {
        t!("timeline-file-video")
    } else if content_type.starts_with("audio/") {
        t!("timeline-file-audio")
    } else {
        t!("timeline-file-other")
    };
    match size {
        Some(bytes) if bytes > 0 => {
            t!("timeline-file-sized", kind = kind, size = human_size(bytes))
        }
        _ => t!("timeline-file", kind = kind),
    }
}

fn human_size(bytes: i64) -> String {
    let b = bytes as f64;
    if b >= 1e9 {
        format!("{:.1} GB", b / 1e9)
    } else if b >= 1e6 {
        format!("{:.1} MB", b / 1e6)
    } else if b >= 1e3 {
        format!("{:.0} kB", b / 1e3)
    } else {
        format!("{bytes} B")
    }
}

/// The smallest splice turning `old` into `new`: the range of old rows to
/// replace and how many new rows take their place.
pub fn splice(old: &[String], new: &[String]) -> Option<(Range<usize>, usize)> {
    if old == new {
        return None;
    }
    let prefix = old.iter().zip(new).take_while(|(a, b)| a == b).count();
    let max_suffix = old.len().min(new.len()) - prefix;
    let suffix = old
        .iter()
        .rev()
        .zip(new.iter().rev())
        .take(max_suffix)
        .take_while(|(a, b)| a == b)
        .count();
    Some((prefix..old.len() - suffix, new.len() - prefix - suffix))
}

pub fn pending_label(p: &PendingSend) -> &str {
    match &p.state {
        SendState::Sending => t!("timeline-sending"),
        SendState::Failed(reason) => reason,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ircord_client::contract::*;
    use ircord_client::store::Workspace;

    fn workspace() -> Workspace {
        let snapshot: Snapshot = serde_json::from_value(serde_json::json!({
            "version": 1, "cursor": 0, "demo": false,
            "user": {"id": "u-me", "account": "me", "nick": "me", "displayName": "",
                     "bio": "", "color": "", "role": "member", "status": "online"},
            "capabilities": {}, "invitations": [],
            "members": [{"id": "m-bob", "account": "bob", "nick": "bob", "displayName": "",
                         "bio": "", "color": "#ff0000", "role": "member", "status": "online"}],
            "networks": [{"id": "n1", "name": "Home", "nick": "me", "host": "irc", "port": 6667,
                          "tls": false, "status": "connected", "initials": "H", "color": "",
                          "auth": {"method": "none", "passwordSet": false}, "autoJoin": [],
                          "capabilities": [], "isupport": {}, "perform": [], "features": {}}],
            "conversations": [{"id": "c", "networkId": "n1", "kind": "channel", "name": "x",
                 "favorite": false, "joined": true, "memberIds": [], "memberModes": {},
                 "modes": "", "topic": "", "unreadCount": 0, "mentionCount": 0}]
        }))
        .unwrap();
        Workspace::from_snapshot(snapshot)
    }

    fn msg(id: &str, at: &str, sender: &str, text: &str) -> Message {
        Message {
            id: id.into(),
            conversation_id: "c".into(),
            created_at: at.into(),
            kind: MessageKind::Message,
            sender_id: sender.into(),
            text: text.into(),
            ..Default::default()
        }
    }

    fn load(w: &mut Workspace, items: Vec<Message>, older: Option<&str>) {
        w.load_newest(
            "c",
            MessagePage {
                items,
                next_cursor: older.map(str::to_string),
                ..Default::default()
            },
        );
    }

    #[test]
    fn days_groups_and_events() {
        let mut w = workspace();
        let mut join = msg("j", "2026-11-20T12:00:00Z", "m-bob", "");
        join.kind = MessageKind::Event;
        join.event = Some(MembershipEvent {
            actor_id: "m-bob".into(),
            r#type: MembershipEventType::Join,
            ..Default::default()
        });
        load(
            &mut w,
            vec![
                join,
                msg("1", "2026-11-20T12:01:00Z", "m-bob", "hi"),
                msg("2", "2026-11-20T12:02:00Z", "m-bob", "again"),
                msg("3", "2026-11-20T12:30:00Z", "m-bob", "later"),
                msg(
                    "4",
                    "2026-11-23T12:00:00Z",
                    "u-me",
                    "monday https://youtu.be/x",
                ),
            ],
            None,
        );
        let rows = build(&w, "c");
        assert_eq!(rows[0], Row::Start);
        assert!(matches!(rows[1], Row::Day(_)));
        let Row::Event(e) = &rows[2] else {
            panic!("{:?}", rows[2])
        };
        assert_eq!(e.text, "bob joined");
        let headers: Vec<bool> = rows
            .iter()
            .filter_map(|r| match r {
                Row::Message(m) => Some(m.header),
                _ => None,
            })
            .collect();
        assert_eq!(headers, [true, false, true, true]);
        let Some(Row::Message(last)) = rows.last() else {
            panic!()
        };
        assert!(last.own);
        assert_eq!(last.sender, "me");
        assert_eq!(last.links.len(), 1);
        assert!(last.links[0].video);
        assert_eq!(rows.iter().filter(|r| matches!(r, Row::Day(_))).count(), 2);
    }

    #[test]
    fn images_inline_and_files_as_links() {
        let mut w = workspace();
        let mut m = msg(
            "1",
            "2026-11-20T12:00:00Z",
            "m-bob",
            "look https://x.y/cat.png",
        );
        m.attachments = Some(vec![Attachment {
            url: "https://x.y/files/doc.pdf".into(),
            name: "doc.pdf".into(),
            content_type: Some("application/pdf".into()),
            size: Some(2_500_000),
            ..Default::default()
        }]);
        load(&mut w, vec![m], Some("more"));
        let rows = build(&w, "c");
        assert!(!matches!(rows[0], Row::Start), "older history remains");
        let Some(Row::Message(m)) = rows.iter().find(|r| matches!(r, Row::Message(_))) else {
            panic!()
        };
        assert_eq!(m.images.len(), 1);
        assert_eq!(m.links.len(), 1);
        assert_eq!(m.links[0].detail, "File · 2.5 MB · opens in your browser");
        assert_eq!(m.sender_color, Some(0xff0000));
    }

    #[test]
    fn unsent_messages_follow_the_timeline() {
        let mut w = workspace();
        load(&mut w, vec![], None);
        w.queue_send(PendingSend {
            client_id: "k".into(),
            conversation_id: "c".into(),
            text: "hi".into(),
            action: false,
            state: SendState::Failed("Not connected".into()),
        });
        let rows = build(&w, "c");
        let Some(Row::Pending(p)) = rows.last() else {
            panic!()
        };
        assert_eq!(pending_label(p), "Not connected");
    }

    #[test]
    fn edits_change_the_row_key() {
        let mut w = workspace();
        load(
            &mut w,
            vec![msg("1", "2026-11-20T12:00:00Z", "m-bob", "a")],
            None,
        );
        let before = build(&w, "c").last().unwrap().key();
        let mut edited = msg("1", "2026-11-20T12:00:00Z", "m-bob", "b");
        edited.edited_at = Some("2026-11-20T12:01:00Z".into());
        load(&mut w, vec![edited], None);
        // A page never overwrites a copy already held; an update event does.
        w.apply(
            &ircord_client::contract::event::ServerEvent::decode(
                r#"{"v":1,"seq":1,"type":"message.updated","data":{"id":"1","conversationId":"c",
                "createdAt":"2026-11-20T12:00:00Z","kind":"message","senderId":"m-bob","text":"b",
                "editedAt":"2026-11-20T12:01:00Z","reactions":[]}}"#,
            )
            .unwrap(),
            0,
        );
        let rows = build(&w, "c");
        let Some(Row::Message(m)) = rows.last() else {
            panic!()
        };
        assert!(m.edited);
        assert_ne!(rows.last().unwrap().key(), before);
    }

    #[test]
    fn splice_finds_the_changed_middle() {
        let k = |s: &[&str]| s.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert_eq!(splice(&k(&["a", "b"]), &k(&["a", "b"])), None);
        assert_eq!(
            splice(&k(&["a", "b"]), &k(&["a", "b", "c"])),
            Some((2..2, 1))
        );
        assert_eq!(
            splice(&k(&["b", "c"]), &k(&["a", "b", "c"])),
            Some((0..0, 1))
        );
        assert_eq!(
            splice(&k(&["a", "x", "c"]), &k(&["a", "y", "c"])),
            Some((1..2, 1))
        );
        assert_eq!(splice(&k(&["a", "a"]), &k(&["a"])), Some((1..2, 0)));
    }
}

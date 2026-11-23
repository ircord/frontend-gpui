//! The timeline as rows: day separators, messages (grouped under one header
//! when the same person writes again within a few minutes), join/part and
//! other event rows (runs of joins, parts, quits and nick changes folded
//! into one line, or hidden, as the preferences say), the "New messages"
//! line, rows for history that failed to load, and sends still on their
//! way. Pure, so it is tested without a window.
//!
//! As the web client does, `system` messages (the history service's
//! replayed "X joined the channel" lines after a reconnect) are left out:
//! the Server window shows them, and between two joins they would end
//! every fold.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::ops::Range;
use std::rc::Rc;

use chrono::{DateTime, Datelike, Local, NaiveDate, TimeZone};
use ircord_client::api::channels::Chanmodes;
use ircord_client::contract::{
    Attachment, ClientPreferencesEvents, ConversationKind, MembershipEventType, Message,
    MessageKind,
};
use ircord_client::format::{self, Formatted};
use ircord_client::irc::Prefixes;
use ircord_client::store::{PendingSend, SELF_ID, SendState, Workspace};

/// Messages from one sender less than this many milliseconds apart share a
/// header.
const GROUP_MS: i64 = 5 * 60 * 1000;
/// A folded run of joins and parts takes events at most this far apart…
const FOLD_GAP_MS: i64 = 60 * 1000;
/// …spanning at most this long, and at most [`FOLD_MAX`] of them.
const FOLD_SPAN_MS: i64 = 5 * 60 * 1000;
const FOLD_MAX: usize = 50;

/// Parsed message bodies, kept from one build to the next (package P2).
/// Parsing is most of what a rebuild costs, and a body changes only when its
/// text or attachments do, so each entry keeps the input it parsed and is
/// reused only for that same input.
#[derive(Debug, Default)]
pub struct Bodies {
    entries: HashMap<String, (String, Formatted)>,
}

impl Bodies {
    /// The parsed body of `input` as message `id`'s body, from the cache when
    /// the input is unchanged.
    fn parsed(&mut self, id: &str, input: String) -> Formatted {
        if let Some((text, body)) = self.entries.get(id)
            && *text == input
        {
            return body.clone();
        }
        let body = format::parse(&input);
        self.entries.insert(id.to_string(), (input, body.clone()));
        body
    }

    /// Forget the bodies of messages no longer in `live`.
    fn retain(&mut self, live: &HashSet<&str>) {
        self.entries.retain(|id, _| live.contains(id.as_str()));
    }
}

/// What the rows depend on beyond the workspace.
#[derive(Debug, Clone, Copy, Default)]
pub struct Options<'a> {
    /// The message the "New messages" line sits before.
    pub unread: Option<&'a str>,
    /// The message a jump landed on.
    pub focused: Option<&'a str>,
    /// Folded runs (by [`FoldRow::id`]) the person opened.
    pub expanded: Option<&'a HashSet<String>>,
    /// The day that reads "Today"; the local date when unset.
    pub today: Option<NaiveDate>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Row {
    /// The start of the history: nothing older to load.
    Start(Rc<Intro>),
    /// Older history is loading.
    Loading,
    /// History could not finish loading: a row to try again from.
    Retry,
    /// The "New messages" line before the first unread message.
    Unread,
    /// Newer history is loading (in a window into older history).
    NewerLoading,
    /// Newer history could not load: a row to try again from.
    NewerFailed,
    Day(String),
    Message(Rc<MessageRow>),
    /// Join, part, quit, nick, topic and similar lines.
    Event(Rc<EventRow>),
    /// A run of joins, parts, quits and nick changes as one line.
    Fold(Rc<FoldRow>),
    Pending(Rc<PendingRow>),
}

impl Row {
    /// Stable identity, for splicing the virtual list.
    pub fn key(&self) -> String {
        match self {
            Row::Start(i) => {
                use std::hash::{Hash, Hasher};
                let mut h = std::collections::hash_map::DefaultHasher::new();
                i.hash(&mut h);
                format!("start:{:x}", h.finish())
            }
            Row::Loading => "loading".into(),
            Row::Retry => "retry".into(),
            Row::Unread => "unread".into(),
            Row::NewerLoading => "newer-loading".into(),
            Row::NewerFailed => "newer-failed".into(),
            Row::Day(day) => format!("day:{day}"),
            Row::Message(m) => format!("m:{}:{}", m.id, m.version),
            Row::Event(e) => format!("e:{}", e.id),
            Row::Fold(f) => format!("f:{}:{}:{}", f.id, f.events.len(), f.expanded),
            Row::Pending(p) => format!(
                "p:{}:{:?}:{}:{}",
                p.send.client_id, p.send.state, p.header, p.time
            ),
        }
    }
}

/// A send the server has not confirmed yet, drawn after what it stored.
#[derive(Debug, Clone, PartialEq)]
pub struct PendingRow {
    pub send: PendingSend,
    /// Show your name, picture and the time above it: it does not continue
    /// your own message just above, as a message row would not.
    pub header: bool,
    pub time: String,
}

/// What the start of a conversation's history says about it.
#[derive(Debug, Clone, PartialEq, Hash, Default)]
pub struct Intro {
    pub kind: IntroKind,
    /// "#general"; empty for a direct message (its own intro names them).
    pub title: String,
    /// "This is the beginning of #general."
    pub text: String,
    /// A channel's topic, as plain text.
    pub topic: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum IntroKind {
    Channel,
    Direct,
    Group,
    #[default]
    Other,
}

fn intro(w: &Workspace, conversation: &str) -> Intro {
    let Some(c) = w.conversation(conversation) else {
        return Intro {
            text: t!("timeline-start").into(),
            ..Default::default()
        };
    };
    if let Some(party) = &c.party {
        let name = if party.name.is_empty() {
            c.name.clone()
        } else {
            party.name.clone()
        };
        return Intro {
            kind: IntroKind::Group,
            title: name,
            text: t!("timeline-start-group").into(),
            topic: String::new(),
        };
    }
    match c.kind {
        ConversationKind::Channel => {
            let title = if c.name.starts_with('#') {
                c.name.clone()
            } else {
                format!("#{}", c.name)
            };
            Intro {
                kind: IntroKind::Channel,
                text: t!("timeline-start-channel", channel = title.clone()),
                title,
                topic: format::parse(&c.topic).text.trim().to_string(),
            }
        }
        ConversationKind::Direct => Intro {
            kind: IntroKind::Direct,
            title: String::new(),
            text: t!("timeline-start-direct").into(),
            topic: String::new(),
        },
        _ => Intro {
            text: t!("timeline-start").into(),
            ..Default::default()
        },
    }
}

/// "Today", "Yesterday", "Saturday, November 21" and, in another year,
/// "Saturday, November 21, 2025", as the web client labels days.
fn day_label(day: NaiveDate, today: NaiveDate) -> String {
    if day == today {
        t!("timeline-day-today").into()
    } else if today.pred_opt() == Some(day) {
        t!("timeline-day-yesterday").into()
    } else if day.year() == today.year() {
        day.format("%A, %B %-d").to_string()
    } else {
        day.format("%A, %B %-d, %Y").to_string()
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
    /// The sender's member ID, for the profile and ignore filters.
    pub sender_id: String,
    pub sender_color: Option<u32>,
    pub time: String,
    pub action: bool,
    pub notice: bool,
    pub own: bool,
    pub body: Formatted,
    pub edited: bool,
    pub deleted: bool,
    pub reply: Option<(String, String)>,
    /// The message `reply` quotes, to jump to it.
    pub reply_id: Option<String>,
    /// It mentions you or one of your highlight words: drawn marked.
    pub mention: bool,
    /// The message a jump landed on: drawn marked until you move on.
    pub focused: bool,
    pub reactions: Vec<(String, usize)>,
    /// Uploaded files, in the order they were sent (`message/attachments.rs`
    /// draws them as one gallery and file cards). Their URLs are left out of
    /// `body`, as the web client does.
    pub attachments: Vec<Attachment>,
    /// GIFs sent from the picker, drawn inline.
    pub images: Vec<Image>,
    /// Links in the text, linked pictures among them (`message/links.rs`
    /// previews each through the server's link resolver).
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
    /// The link names a picture by its extension.
    pub picture: bool,
    /// The picture's size the server measured (`embedGeometry`), or 0: the
    /// space a preview reserves before it loads.
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EventRow {
    pub id: String,
    pub text: String,
    pub time: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FoldRow {
    /// The first event's ID.
    pub id: String,
    /// "alice and bob joined · carol left".
    pub summary: String,
    pub time: String,
    pub events: Vec<EventRow>,
    /// Opened to show every event.
    pub expanded: bool,
}

/// How join and part events show, from the preferences.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EventsMode {
    Show,
    Fold,
    Hide,
}

pub fn events_mode(w: &Workspace) -> EventsMode {
    let events = w
        .documents
        .preferences
        .value()
        .and_then(|p| p.preferences.as_ref())
        .and_then(|p| p.events);
    match events {
        Some(ClientPreferencesEvents::Show) => EventsMode::Show,
        Some(ClientPreferencesEvents::Hide) => EventsMode::Hide,
        _ => EventsMode::Fold,
    }
}

/// Joins, parts, quits, nick changes and status given or taken (`+o`,
/// `-v`...): the chatter that folds.
fn is_chatter(m: &Message, isupport: &BTreeMap<String, String>) -> bool {
    m.event.as_ref().is_some_and(|e| match e.r#type {
        MembershipEventType::Join
        | MembershipEventType::Part
        | MembershipEventType::Quit
        | MembershipEventType::Nick => true,
        MembershipEventType::Mode => status_only(e.text.as_deref().unwrap_or_default(), isupport),
        _ => false,
    })
}

/// Whether a `MODE` change only gives or takes members' channel status, the
/// modes the network's `PREFIX` lists. Channel-wide modes and ban lists
/// are not chatter, so a change that mixes them in stays visible.
fn status_only(text: &str, isupport: &BTreeMap<String, String>) -> bool {
    let prefixes = Prefixes::of(isupport);
    let flags = text.split_whitespace().next().unwrap_or_default();
    let mut any = false;
    for mode in flags.chars().filter(|c| !matches!(c, '+' | '-')) {
        if !prefixes.modes.contains(&mode) {
            return false;
        }
        any = true;
    }
    any
}

/// Whether `text` mentions `@nick` or one of `words`, as the web client
/// decides it (case-insensitive, on word boundaries).
pub fn mentions(text: &str, nick: &str, words: &[String]) -> bool {
    let plain = format::parse(text).text.to_lowercase();
    let at_nick = (!nick.is_empty()).then(|| format!("@{}", nick.to_lowercase()));
    at_nick.iter().any(|n| contains_word(&plain, n, true))
        || words
            .iter()
            .map(|w| w.trim().to_lowercase())
            .filter(|w| !w.is_empty())
            .any(|w| contains_word(&plain, &w, false))
}

fn contains_word(text: &str, word: &str, mention: bool) -> bool {
    let word_char = |c: char| c.is_alphanumeric() || c == '_' || c == '-';
    // A nick goes on with letters and IRC's nick characters.
    let nick_char = |c: char| word_char(c) || "[]\\`^{}|".contains(c);
    let mut from = 0;
    while let Some(i) = text[from..].find(word).map(|i| i + from) {
        let before = text[..i].chars().next_back();
        let after = text[i + word.len()..].chars().next();
        let starts = before.is_none_or(|c| {
            if mention {
                c.is_whitespace() || "([{<\"'".contains(c)
            } else {
                !word_char(c)
            }
        });
        let ends = after.is_none_or(|c| {
            if mention {
                !nick_char(c)
            } else {
                !word_char(c)
            }
        });
        if starts && ends {
            return true;
        }
        from = i + word.len().max(1);
        while !text.is_char_boundary(from) {
            from += 1;
        }
    }
    false
}

/// The nick to show in event lines, as the web client does: your own
/// network nick, else the member's nick, else their name.
fn nick(w: &Workspace, id: &str, network: &str) -> String {
    if w.is_self(id, network)
        && let Some(n) = w.network(network)
        && !n.nick.is_empty()
    {
        return n.nick.clone();
    }
    match w.member(id, network) {
        Some(m) if !m.nick.is_empty() => m.nick.clone(),
        _ => w.display_name(id, network),
    }
}

/// The summary of a folded run: who joined, left, came back, or changed
/// their nick.
fn fold_summary(w: &Workspace, events: &[&Message], network: &str) -> String {
    #[derive(PartialEq, Clone, Copy)]
    enum Kind {
        Join,
        Leave,
        Nick,
    }
    let mut order: Vec<String> = Vec::new();
    let mut outcomes: std::collections::HashMap<String, (Kind, Kind, Option<String>)> =
        Default::default();
    let mut statuses = 0;
    for m in events {
        let Some(e) = &m.event else { continue };
        if e.r#type == MembershipEventType::Mode {
            statuses += 1;
            continue;
        }
        let kind = match e.r#type {
            MembershipEventType::Join => Kind::Join,
            MembershipEventType::Nick => Kind::Nick,
            _ => Kind::Leave,
        };
        match outcomes.get_mut(&e.actor_id) {
            Some(o) if kind == Kind::Nick => o.2 = e.text.clone(),
            Some(o) => o.1 = kind,
            None => {
                order.push(e.actor_id.clone());
                let nick = (kind == Kind::Nick).then(|| e.text.clone()).flatten();
                outcomes.insert(e.actor_id.clone(), (kind, kind, nick));
            }
        }
    }
    let (mut joined, mut left, mut back, mut joined_left, mut renames) =
        (vec![], vec![], vec![], vec![], vec![]);
    for id in &order {
        let (first, last, nick) = &outcomes[id];
        let name = self::nick(w, id, network);
        if let Some(nick) = nick {
            renames.push(t!(
                "timeline-fold-renamed",
                old = name.clone(),
                new = nick.clone()
            ));
        }
        match (first, last) {
            (Kind::Nick, _) => {}
            (Kind::Leave, Kind::Join) => back.push(name),
            (Kind::Leave, _) => left.push(name),
            (_, Kind::Join) | (_, Kind::Nick) => joined.push(name),
            (Kind::Join, Kind::Leave) => joined_left.push(name),
        }
    }
    let mut parts: Vec<String> = Vec::new();
    if !back.is_empty() {
        parts.push(t!("timeline-fold-reconnected", people = people(&back)));
    }
    if !joined.is_empty() {
        parts.push(t!("timeline-fold-joined", people = people(&joined)));
    }
    if !joined_left.is_empty() {
        parts.push(t!(
            "timeline-fold-joined-left",
            people = people(&joined_left)
        ));
    }
    if !left.is_empty() {
        parts.push(t!("timeline-fold-left", people = people(&left)));
    }
    parts.extend(renames);
    if statuses > 0 {
        parts.push(if statuses == 1 {
            t!("timeline-fold-mode").into()
        } else {
            t!("timeline-fold-modes", count = statuses)
        });
    }
    parts.join(" · ")
}

/// "alice", "alice and bob", "alice, bob and carol", "5 people".
fn people(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [a] => a.clone(),
        [a, b] => t!("timeline-people-two", first = a, second = b),
        [a, b, c] => t!("timeline-people-three", first = a, second = b, third = c),
        _ => t!("timeline-people-many", count = names.len()),
    }
}

pub fn build(w: &Workspace, conversation: &str, options: Options) -> Vec<Row> {
    build_with(w, conversation, options, &mut Bodies::default())
}

/// [`build`], reusing the parsed bodies `bodies` holds from the last build.
pub fn build_with(
    w: &Workspace,
    conversation: &str,
    options: Options,
    bodies: &mut Bodies,
) -> Vec<Row> {
    let Some(timeline) = w.timelines.get(conversation) else {
        return vec![Row::Loading];
    };
    let live: HashSet<&str> = timeline
        .entries
        .iter()
        .map(|e| e.message.id.as_str())
        .collect();
    bodies.retain(&live);
    let network = w
        .conversation(conversation)
        .map(|c| c.network_id.clone())
        .unwrap_or_default();
    let nick = w
        .network(&network)
        .map(|n| n.nick.clone())
        .unwrap_or_default();
    let words: Vec<String> = w
        .documents
        .notification_settings
        .value()
        .and_then(|s| s.highlight_words.clone())
        .unwrap_or_default();
    let mode = events_mode(w);
    let mut rows = Vec::with_capacity(timeline.entries.len() + 8);
    if timeline.loading_older || !timeline.loaded {
        rows.push(Row::Loading);
    } else if timeline.sync_error.is_some() {
        rows.push(Row::Retry);
    } else if timeline.older.is_none() {
        rows.push(Row::Start(Rc::new(intro(w, conversation))));
    }
    let today = options.today.unwrap_or_else(|| Local::now().date_naive());
    let mut last_day: Option<NaiveDate> = None;
    // The sender, time and kind of the message above, for grouping.
    let mut last: Option<(String, i64, MessageKind)> = None;
    // The "New messages" line belongs before a skipped message: it goes
    // before the next row instead.
    let mut unread_pending = false;
    // The run of chatter being folded: its events and their times.
    let mut run: Vec<(&Message, i64)> = Vec::new();
    let flush = |run: &mut Vec<(&Message, i64)>, rows: &mut Vec<Row>| {
        let events: Vec<EventRow> = run
            .iter()
            .map(|(m, at)| EventRow {
                id: m.id.clone(),
                text: event_nicks(w, m, &network),
                time: crate::message::layout::time_label(w, &local_time(*at)),
            })
            .collect();
        match events.len() {
            0 => {}
            1 => rows.push(Row::Event(Rc::new(events.into_iter().next().unwrap()))),
            _ => {
                let messages: Vec<&Message> = run.iter().map(|(m, _)| *m).collect();
                let id = events[0].id.clone();
                rows.push(Row::Fold(Rc::new(FoldRow {
                    summary: fold_summary(w, &messages, &network),
                    time: events[0].time.clone(),
                    expanded: options.expanded.is_some_and(|e| e.contains(&id)),
                    id,
                    events,
                })));
            }
        }
        run.clear();
    };
    for entry in &timeline.entries {
        let m = &entry.message;
        if m.kind == MessageKind::System {
            unread_pending |= options.unread == Some(m.id.as_str());
            continue;
        }
        let local = local_time(entry.at);
        let day = local.date_naive();
        if last_day != Some(day) {
            flush(&mut run, &mut rows);
            rows.push(Row::Day(day_label(day, today)));
            last_day = Some(day);
            last = None;
        }
        if unread_pending || options.unread == Some(m.id.as_str()) {
            unread_pending = false;
            flush(&mut run, &mut rows);
            rows.push(Row::Unread);
            last = None;
        }
        let time = crate::message::layout::time_label(w, &local);
        if m.kind == MessageKind::Event || m.event.is_some() {
            last = None;
            if is_chatter(m, isupport(w, &network)) {
                match mode {
                    EventsMode::Hide => continue,
                    EventsMode::Fold => {
                        let continues = match (run.first(), run.last()) {
                            (Some((_, first)), Some((_, prev))) => {
                                entry.at - prev <= FOLD_GAP_MS
                                    && entry.at - first <= FOLD_SPAN_MS
                                    && run.len() < FOLD_MAX
                            }
                            _ => true,
                        };
                        if !continues {
                            flush(&mut run, &mut rows);
                        }
                        run.push((m, entry.at));
                        continue;
                    }
                    EventsMode::Show => {}
                }
            }
            flush(&mut run, &mut rows);
            rows.push(Row::Event(Rc::new(EventRow {
                id: m.id.clone(),
                text: event_nicks(w, m, &network),
                time,
            })));
            continue;
        }
        flush(&mut run, &mut rows);
        // A new header for another sender, a pause, a reply, a change of
        // kind (message, action, notice) and every action, as the web does.
        let header = match &last {
            Some((sender, at, kind)) => {
                *sender != m.sender_id
                    || entry.at - at >= GROUP_MS
                    || m.reply_to.is_some()
                    || *kind != m.kind
                    || m.kind == MessageKind::Action
            }
            None => true,
        };
        last = Some((m.sender_id.clone(), entry.at, m.kind));
        let mut row = message_row(w, m, &network, header, time, bodies);
        row.mention = !row.own && !row.deleted && mentions(&m.text, &nick, &words);
        row.focused = options.focused == Some(m.id.as_str());
        if row.mention || row.focused {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            (row.version, row.mention, row.focused).hash(&mut h);
            row.version = h.finish();
        }
        rows.push(Row::Message(Rc::new(row)));
    }
    flush(&mut run, &mut rows);
    if timeline.loading_newer {
        rows.push(Row::NewerLoading);
    } else if timeline.newer_failed {
        rows.push(Row::NewerFailed);
    }
    // Sends the server has not echoed yet sit at the end of the present,
    // as the web client keeps its outbox after the stored history, grouped
    // with your messages just above them.
    if !timeline.has_newer {
        if !matches!(rows.last(), Some(Row::Message(_))) {
            last = None;
        }
        for pending in w.pending_in(conversation) {
            let echoed = timeline
                .entries
                .iter()
                .any(|e| e.message.client_id.as_deref() == Some(pending.client_id.as_str()));
            if echoed {
                continue;
            }
            let at = pending.created_ms;
            let kind = if pending.action {
                MessageKind::Action
            } else {
                MessageKind::Message
            };
            let header = match &last {
                Some((sender, prev, prev_kind)) => {
                    !w.is_self(sender, &network)
                        || at - prev >= GROUP_MS
                        || *prev_kind != kind
                        || pending.action
                }
                None => true,
            };
            last = Some((SELF_ID.to_string(), at, kind));
            rows.push(Row::Pending(Rc::new(PendingRow {
                send: pending.clone(),
                header,
                time: crate::message::layout::time_label(w, &local_time(at)),
            })));
        }
    }
    rows
}

/// The row index of `message`, or of the folded run holding it.
pub fn index_of(rows: &[Row], message: &str) -> Option<usize> {
    rows.iter().position(|r| match r {
        Row::Message(m) => m.id == message,
        Row::Event(e) => e.id == message,
        Row::Fold(f) => f.events.iter().any(|e| e.id == message),
        _ => false,
    })
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
    bodies: &mut Bodies,
) -> MessageRow {
    let member = w.member(&m.sender_id, network);
    let sender_color = member
        .and_then(|m| m.name_color.as_deref().or(Some(m.color.as_str())))
        .and_then(parse_hex);
    let attachments: Vec<Attachment> = m
        .attachments
        .clone()
        .unwrap_or_default()
        .into_iter()
        .chain(m.attachment.clone())
        .collect();
    let body = bodies.parsed(&m.id, without_urls(&m.text, &attachments));
    let mut images = Vec::new();
    let mut links: Vec<LinkCard> = Vec::new();
    if let Some(gif) = &m.gif {
        images.push(Image {
            url: gif.url.clone(),
            width: gif.width.max(0) as u32,
            height: gif.height.max(0) as u32,
            alt: gif.title.clone(),
        });
    }
    for (_, url) in &body.links {
        if images.iter().any(|i| &i.url == url)
            || links.iter().any(|l| &l.url == url)
            || attachments.iter().any(|a| &a.url == url)
        {
            continue;
        }
        let geometry = m
            .embed_geometry
            .as_ref()
            .and_then(|g| g.iter().find(|g| &g.url == url));
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
            picture: looks_like_image(url),
            width: geometry.map_or(0, |g| g.width.max(0) as u32),
            height: geometry.map_or(0, |g| g.height.max(0) as u32),
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
        sender_id: m.sender_id.clone(),
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
        reply_id: m.reply_to.as_ref().map(|r| r.id.clone()),
        mention: false,
        focused: false,
        reactions: m
            .reactions
            .iter()
            .filter(|r| !r.member_ids.is_empty())
            .map(|r| (r.emoji.clone(), r.member_ids.len()))
            .collect(),
        attachments,
        images,
        links,
    }
}

/// The text without the uploaded files' URLs, which the attachments show.
fn without_urls(text: &str, attachments: &[Attachment]) -> String {
    if attachments.is_empty() {
        return text.to_string();
    }
    let mut out = text.to_string();
    for a in attachments {
        if !a.url.is_empty() {
            out = out.replace(&a.url, "");
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// An event as a line with people's names (the exported log).
pub(crate) fn event_text(w: &Workspace, m: &Message, network: &str) -> String {
    event_line(m, isupport(w, network), &|id| w.display_name(id, network))
}

/// An event as the timeline shows it: with nicks, as the web client does.
fn event_nicks(w: &Workspace, m: &Message, network: &str) -> String {
    event_line(m, isupport(w, network), &|id| nick(w, id, network))
}

/// The network's ISUPPORT, which says how its mode changes read.
fn isupport<'a>(w: &'a Workspace, network: &str) -> &'a BTreeMap<String, String> {
    static NONE: BTreeMap<String, String> = BTreeMap::new();
    w.network(network).map_or(&NONE, |n| &n.isupport)
}

fn event_line(
    m: &Message,
    isupport: &BTreeMap<String, String>,
    name: &dyn Fn(&str) -> String,
) -> String {
    let Some(e) = &m.event else {
        return ircord_client::format::parse(&m.text).text;
    };
    let actor = name(&e.actor_id);
    let target = e.target_id.as_deref().map(name).unwrap_or_default();
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
        MembershipEventType::Mode => {
            t!(
                "timeline-event-mode",
                actor = actor,
                changes = mode_description(&detail, isupport)
            )
        }
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

/// A `MODE` change in words. Which letters take an argument comes from the
/// network's `PREFIX` and `CHANMODES`, so the arguments stay with their
/// modes; a channel key is consumed without being shown.
fn mode_description(text: &str, isupport: &BTreeMap<String, String>) -> String {
    let chanmodes = Chanmodes::parse(isupport);
    let prefixes = Prefixes::of(isupport);
    let mut words = text.split_whitespace();
    let flags = words.next().unwrap_or_default();
    let mut adding = true;
    let mut phrases = Vec::new();
    let mut raw = Vec::new();
    for mode in flags.chars() {
        if matches!(mode, '+' | '-') {
            adding = mode == '+';
            continue;
        }
        let status = prefixes.modes.contains(&mode);
        let has_arg = status
            || chanmodes.list.contains(mode)
            || chanmodes.always.contains(mode)
            || (adding && chanmodes.when_set.contains(mode));
        let arg = has_arg.then(|| words.next()).flatten();
        let phrase = if status {
            arg.and_then(|nick| status_change(mode, adding, nick))
        } else {
            channel_change(mode, adding, arg, &chanmodes)
        };
        let Some(phrase) = phrase else {
            let sign = if adding { '+' } else { '-' };
            raw.push(match arg {
                Some(_) if mode == 'k' => format!("{sign}{mode}"),
                Some(arg) => format!("{sign}{mode} {arg}"),
                None => format!("{sign}{mode}"),
            });
            continue;
        };
        phrases.push(phrase);
    }
    if !raw.is_empty() {
        phrases.push(t!("timeline-mode-raw", modes = raw.join(" ")));
    }
    match phrases.pop() {
        None => t!("timeline-mode-unknown").into(),
        Some(last) if phrases.is_empty() => last,
        Some(last) => t!(
            "timeline-mode-list",
            previous = phrases.join(", "),
            last = last
        ),
    }
}

/// A status the network's `PREFIX` lists, given to or taken from `nick`.
fn status_change(mode: char, adding: bool, nick: &str) -> Option<String> {
    Some(match (mode, adding) {
        ('q', true) => t!("timeline-mode-founder-add", nick = nick),
        ('q', false) => t!("timeline-mode-founder-remove", nick = nick),
        ('a', true) => t!("timeline-mode-admin-add", nick = nick),
        ('a', false) => t!("timeline-mode-admin-remove", nick = nick),
        ('o', true) => t!("timeline-mode-operator-add", nick = nick),
        ('o', false) => t!("timeline-mode-operator-remove", nick = nick),
        ('h', true) => t!("timeline-mode-half-op-add", nick = nick),
        ('h', false) => t!("timeline-mode-half-op-remove", nick = nick),
        ('v', true) => t!("timeline-mode-voice-add", nick = nick),
        ('v', false) => t!("timeline-mode-voice-remove", nick = nick),
        _ => return None,
    })
}

/// A channel mode in words, or `None` to show it as the server sent it.
fn channel_change(
    mode: char,
    adding: bool,
    arg: Option<&str>,
    chanmodes: &Chanmodes,
) -> Option<String> {
    let listed = chanmodes.list.contains(mode);
    Some(match (mode, adding, arg) {
        ('b', true, Some(mask)) if listed => t!("timeline-mode-ban-add", mask = mask),
        ('b', false, Some(mask)) if listed => t!("timeline-mode-ban-remove", mask = mask),
        ('e', true, Some(mask)) if listed => t!("timeline-mode-ban-exception-add", mask = mask),
        ('e', false, Some(mask)) if listed => {
            t!("timeline-mode-ban-exception-remove", mask = mask)
        }
        ('I', true, Some(mask)) if listed => {
            t!("timeline-mode-invite-exception-add", mask = mask)
        }
        ('I', false, Some(mask)) if listed => {
            t!("timeline-mode-invite-exception-remove", mask = mask)
        }
        ('l', true, Some(count)) => t!("timeline-mode-limit-add", count = count),
        ('l', false, _) => t!("timeline-mode-limit-remove").into(),
        ('k', true, _) => t!("timeline-mode-key-add").into(),
        ('k', false, _) => t!("timeline-mode-key-remove").into(),
        ('i', true, _) => t!("timeline-mode-invite-only-add").into(),
        ('i', false, _) => t!("timeline-mode-invite-only-remove").into(),
        ('m', true, _) => t!("timeline-mode-moderated-add").into(),
        ('m', false, _) => t!("timeline-mode-moderated-remove").into(),
        ('t', true, _) => t!("timeline-mode-topic-add").into(),
        ('t', false, _) => t!("timeline-mode-topic-remove").into(),
        ('n', true, _) => t!("timeline-mode-external-add").into(),
        ('n', false, _) => t!("timeline-mode-external-remove").into(),
        ('s', true, _) => t!("timeline-mode-secret-add").into(),
        ('s', false, _) => t!("timeline-mode-secret-remove").into(),
        ('p', true, _) => t!("timeline-mode-private-add").into(),
        ('p', false, _) => t!("timeline-mode-private-remove").into(),
        ('r', true, _) => t!("timeline-mode-registered-add").into(),
        ('r', false, _) => t!("timeline-mode-registered-remove").into(),
        _ => return None,
    })
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
    fn cached_bodies_follow_the_text() {
        let mut w = workspace();
        load(
            &mut w,
            vec![msg("1", "2026-11-20T12:01:00Z", "m-bob", "*plain* text")],
            None,
        );
        let mut bodies = Bodies::default();
        let first = build_with(&w, "c", Options::default(), &mut bodies);
        assert_eq!(first, build(&w, "c", Options::default()));
        // The same text is served from the cache.
        let again = build_with(&w, "c", Options::default(), &mut bodies);
        assert_eq!(again, first);
        // An edited message is parsed again.
        // An edit replaces the stored text of the same message.
        w.timelines.get_mut("c").unwrap().entries[0].message.text = "changed _text_".into();
        let edited = build_with(&w, "c", Options::default(), &mut bodies);
        assert_eq!(edited, build(&w, "c", Options::default()));
        assert!(
            edited
                .iter()
                .any(|r| matches!(r, Row::Message(m) if m.body.text == "changed _text_"))
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
        let rows = build(&w, "c", Options::default());
        assert!(matches!(&rows[0], Row::Start(i) if i.title == "#x"));
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
    fn uploads_as_attachments_and_links_inline() {
        let mut w = workspace();
        let mut m = msg(
            "1",
            "2026-11-20T12:00:00Z",
            "m-bob",
            "look https://x.y/cat.png https://x.y/files/doc.pdf",
        );
        m.attachments = Some(vec![Attachment {
            url: "https://x.y/files/doc.pdf".into(),
            name: "doc.pdf".into(),
            content_type: Some("application/pdf".into()),
            size: Some(2_500_000),
            ..Default::default()
        }]);
        load(&mut w, vec![m], Some("more"));
        let rows = build(&w, "c", Options::default());
        assert!(!matches!(rows[0], Row::Start(_)), "older history remains");
        let Some(Row::Message(m)) = rows.iter().find(|r| matches!(r, Row::Message(_))) else {
            panic!()
        };
        assert!(m.images.is_empty());
        assert_eq!(m.links.len(), 1);
        assert!(
            m.links[0].picture,
            "a linked picture is previewed as a link"
        );
        assert_eq!(m.attachments.len(), 1);
        assert_eq!(
            m.body.text, "look https://x.y/cat.png",
            "the upload's URL is left out"
        );
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
            created_ms: 0,
            ..Default::default()
        });
        let rows = build(&w, "c", Options::default());
        let Some(Row::Pending(p)) = rows.last() else {
            panic!()
        };
        assert_eq!(pending_label(&p.send), "Not connected");
        assert!(p.header, "nothing of yours above it");
    }

    #[test]
    fn an_unsent_message_groups_with_yours_above_it() {
        let send = |w: &mut Workspace, id: &str, at: &str| {
            w.queue_send(PendingSend {
                client_id: id.into(),
                conversation_id: "c".into(),
                text: "hi".into(),
                action: false,
                state: SendState::Sending,
                created_ms: ms(at),
                ..Default::default()
            });
        };
        let headers = |w: &Workspace| -> Vec<bool> {
            build(w, "c", Options::default())
                .into_iter()
                .filter_map(|r| match r {
                    Row::Pending(p) => Some(p.header),
                    _ => None,
                })
                .collect()
        };
        // After your own message a minute earlier: no header, as on the web.
        let mut w = workspace();
        load(
            &mut w,
            vec![msg("1", "2026-11-20T12:00:00Z", "u-me", "mine")],
            None,
        );
        send(&mut w, "k1", "2026-11-20T12:01:00Z");
        send(&mut w, "k2", "2026-11-20T12:01:30Z");
        assert_eq!(headers(&w), [false, false]);
        // A pause of five minutes or more starts a new group.
        send(&mut w, "k3", "2026-11-20T12:07:00Z");
        assert_eq!(headers(&w), [false, false, true]);
        // An action always has its own header.
        w.queue_send(PendingSend {
            client_id: "k4".into(),
            conversation_id: "c".into(),
            text: "waves".into(),
            action: true,
            created_ms: ms("2026-11-20T12:07:10Z"),
            ..Default::default()
        });
        assert_eq!(headers(&w), [false, false, true, true]);
        // After someone else's message: your name and picture above it.
        let mut w = workspace();
        load(
            &mut w,
            vec![msg("1", "2026-11-20T12:00:00Z", "m-bob", "theirs")],
            None,
        );
        send(&mut w, "k1", "2026-11-20T12:01:00Z");
        assert_eq!(headers(&w), [true]);
    }

    fn ms(at: &str) -> i64 {
        ircord_client::store::parse_time(at)
    }

    #[test]
    fn edits_change_the_row_key() {
        let mut w = workspace();
        load(
            &mut w,
            vec![msg("1", "2026-11-20T12:00:00Z", "m-bob", "a")],
            None,
        );
        let before = build(&w, "c", Options::default()).last().unwrap().key();
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
        let rows = build(&w, "c", Options::default());
        let Some(Row::Message(m)) = rows.last() else {
            panic!()
        };
        assert!(m.edited);
        assert_ne!(rows.last().unwrap().key(), before);
    }

    fn event(id: &str, at: &str, actor: &str, kind: MembershipEventType) -> Message {
        let mut m = msg(id, at, actor, "");
        m.kind = MessageKind::Event;
        m.event = Some(MembershipEvent {
            actor_id: actor.into(),
            r#type: kind,
            ..Default::default()
        });
        m
    }

    fn set_events(w: &mut Workspace, mode: ClientPreferencesEvents) {
        let prefs: StoredPreferences = serde_json::from_value(serde_json::json!({
            "preferences": {"events": mode}, "updatedAt": "2026-11-20T00:00:00Z"
        }))
        .unwrap();
        w.documents.preferences.read_done(Ok(prefs));
    }

    fn joins_and_parts(w: &mut Workspace) {
        load(
            w,
            vec![
                event(
                    "e1",
                    "2026-11-20T12:00:00Z",
                    "m-bob",
                    MembershipEventType::Join,
                ),
                event(
                    "e2",
                    "2026-11-20T12:00:20Z",
                    "m-al",
                    MembershipEventType::Join,
                ),
                event(
                    "e3",
                    "2026-11-20T12:00:40Z",
                    "m-cy",
                    MembershipEventType::Part,
                ),
                event(
                    "e4",
                    "2026-11-20T12:00:50Z",
                    "m-al",
                    MembershipEventType::Quit,
                ),
                msg("1", "2026-11-20T12:01:00Z", "m-bob", "hi"),
                // Too long after the last: a run of its own (one event).
                event(
                    "e5",
                    "2026-11-20T12:10:00Z",
                    "m-cy",
                    MembershipEventType::Join,
                ),
                event(
                    "k",
                    "2026-11-20T12:10:10Z",
                    "m-bob",
                    MembershipEventType::Kick,
                ),
            ],
            None,
        );
    }

    #[test]
    fn runs_of_joins_and_parts_fold_into_one_line() {
        let mut w = workspace();
        joins_and_parts(&mut w);
        let rows = build(&w, "c", Options::default());
        let folds: Vec<&FoldRow> = rows
            .iter()
            .filter_map(|r| match r {
                Row::Fold(f) => Some(f.as_ref()),
                _ => None,
            })
            .collect();
        assert_eq!(folds.len(), 1);
        assert_eq!(folds[0].id, "e1");
        assert_eq!(folds[0].events.len(), 4);
        assert_eq!(
            folds[0].summary, "bob joined · m-al joined and left · m-cy left",
            "{rows:?}"
        );
        // The lone join and the kick stand alone.
        let events: Vec<&str> = rows
            .iter()
            .filter_map(|r| match r {
                Row::Event(e) => Some(e.id.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(events, ["e5", "k"]);
        // Opened, the fold keeps its place and changes its key.
        let open = HashSet::from(["e1".to_string()]);
        let opened = build(
            &w,
            "c",
            Options {
                expanded: Some(&open),
                ..Default::default()
            },
        );
        let i = index_of(&opened, "e3").unwrap();
        let Row::Fold(f) = &opened[i] else { panic!() };
        assert!(f.expanded);
        assert_ne!(opened[i].key(), rows[i].key());
    }

    #[test]
    fn joins_and_parts_show_or_hide_as_the_preferences_say() {
        let mut w = workspace();
        joins_and_parts(&mut w);
        set_events(&mut w, ClientPreferencesEvents::Hide);
        let rows = build(&w, "c", Options::default());
        assert!(!rows.iter().any(|r| matches!(r, Row::Fold(_))));
        let events: Vec<&str> = rows
            .iter()
            .filter_map(|r| match r {
                Row::Event(e) => Some(e.id.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(events, ["k"], "a kick is never hidden");
        set_events(&mut w, ClientPreferencesEvents::Show);
        let rows = build(&w, "c", Options::default());
        assert_eq!(
            rows.iter().filter(|r| matches!(r, Row::Event(_))).count(),
            6
        );
    }

    fn mode(id: &str, at: &str, text: &str) -> Message {
        let mut m = event(id, at, "m-bob", MembershipEventType::Mode);
        m.event.as_mut().unwrap().text = Some(text.into());
        m
    }

    #[test]
    fn status_changes_follow_the_channel_activity_setting() {
        let mut w = workspace();
        load(
            &mut w,
            vec![
                mode("o", "2026-11-20T12:00:00Z", "+o carol"),
                mode("v", "2026-11-20T12:00:10Z", "-v dave"),
                // Channel-wide modes and ban lists are not chatter.
                mode("m", "2026-11-20T12:10:00Z", "+m"),
                mode("b", "2026-11-20T12:20:00Z", "+b *!*@bad"),
                // Two statuses at once are still only status.
                mode("mixed", "2026-11-20T12:30:00Z", "+ov carol dave"),
                mode("with-m", "2026-11-20T12:40:00Z", "+mo carol"),
            ],
            None,
        );
        let ids = |rows: &[Row]| -> Vec<String> {
            rows.iter()
                .filter_map(|r| match r {
                    Row::Event(e) => Some(e.id.clone()),
                    Row::Fold(f) => Some(format!("fold:{}", f.summary)),
                    _ => None,
                })
                .collect()
        };
        set_events(&mut w, ClientPreferencesEvents::Hide);
        assert_eq!(
            ids(&build(&w, "c", Options::default())),
            ["m", "b", "with-m"],
            "status changes hide with the joins and parts"
        );
        set_events(&mut w, ClientPreferencesEvents::Show);
        assert_eq!(
            ids(&build(&w, "c", Options::default())),
            ["o", "v", "m", "b", "mixed", "with-m"]
        );
        set_events(&mut w, ClientPreferencesEvents::Collapse);
        assert_eq!(
            ids(&build(&w, "c", Options::default())),
            ["fold:2 status changes", "m", "b", "mixed", "with-m"]
        );
    }

    #[test]
    fn people_reconnecting_and_renaming() {
        let mut w = workspace();
        let mut nick = event(
            "e3",
            "2026-11-20T12:00:30Z",
            "m-bob",
            MembershipEventType::Nick,
        );
        nick.event.as_mut().unwrap().text = Some("bobby".into());
        load(
            &mut w,
            vec![
                event(
                    "e1",
                    "2026-11-20T12:00:00Z",
                    "m-bob",
                    MembershipEventType::Quit,
                ),
                event(
                    "e2",
                    "2026-11-20T12:00:10Z",
                    "m-bob",
                    MembershipEventType::Join,
                ),
                nick,
                event(
                    "e4",
                    "2026-11-20T12:00:40Z",
                    "m-a",
                    MembershipEventType::Join,
                ),
                event(
                    "e5",
                    "2026-11-20T12:00:41Z",
                    "m-b",
                    MembershipEventType::Join,
                ),
                event(
                    "e6",
                    "2026-11-20T12:00:42Z",
                    "m-c",
                    MembershipEventType::Join,
                ),
                event(
                    "e7",
                    "2026-11-20T12:00:43Z",
                    "m-d",
                    MembershipEventType::Join,
                ),
            ],
            None,
        );
        let rows = build(&w, "c", Options::default());
        let Some(Row::Fold(f)) = rows.iter().find(|r| matches!(r, Row::Fold(_))) else {
            panic!("{rows:?}")
        };
        assert_eq!(
            f.summary,
            "bob reconnected · 4 people joined · bob is now bobby"
        );
    }

    #[test]
    fn the_unread_line_and_the_jump_target() {
        let mut w = workspace();
        load(
            &mut w,
            vec![
                msg("1", "2026-11-20T12:00:00Z", "m-bob", "a"),
                msg("2", "2026-11-20T12:01:00Z", "m-bob", "b"),
                msg("3", "2026-11-20T12:02:00Z", "m-bob", "c"),
            ],
            None,
        );
        let rows = build(
            &w,
            "c",
            Options {
                unread: Some("2"),
                focused: Some("3"),
                ..Default::default()
            },
        );
        let line = rows.iter().position(|r| *r == Row::Unread).unwrap();
        assert_eq!(index_of(&rows, "2"), Some(line + 1));
        // The line starts a new header.
        let Row::Message(two) = &rows[line + 1] else {
            panic!()
        };
        assert!(two.header);
        let Row::Message(three) = &rows[line + 2] else {
            panic!()
        };
        assert!(three.focused && !two.focused);
    }

    #[test]
    fn mentions_of_you_and_highlight_words_are_marked() {
        let words = vec!["deploy".to_string()];
        assert!(mentions("hey @me look", "me", &[]));
        assert!(mentions("(@ME)", "me", &[]));
        assert!(!mentions("hey @meow", "me", &[]));
        assert!(!mentions("mail me@me.org", "me", &[]));
        assert!(!mentions("hey me", "me", &[]));
        assert!(mentions("the Deploy failed", "me", &words));
        assert!(mentions("deploy", "me", &words));
        assert!(!mentions("redeploying", "me", &words));
        assert!(!mentions("pre-deploy", "me", &words));

        let mut w = workspace();
        load(
            &mut w,
            vec![
                msg("1", "2026-11-20T12:00:00Z", "m-bob", "@me hello"),
                msg("2", "2026-11-20T12:01:00Z", "u-me", "@me talking to myself"),
                msg("3", "2026-11-20T12:02:00Z", "m-bob", "nothing"),
            ],
            None,
        );
        let marked: Vec<bool> = build(&w, "c", Options::default())
            .iter()
            .filter_map(|r| match r {
                Row::Message(m) => Some(m.mention),
                _ => None,
            })
            .collect();
        assert_eq!(marked, [true, false, false]);
    }

    #[test]
    fn failed_history_and_a_window_into_older_history() {
        let mut w = workspace();
        load(
            &mut w,
            vec![msg("1", "2026-11-20T12:00:00Z", "m-bob", "a")],
            Some("older"),
        );
        w.begin_older("c").unwrap();
        w.load_older("c", None);
        let rows = build(&w, "c", Options::default());
        assert_eq!(rows[0], Row::Retry);

        let mut w = workspace();
        w.open_around(
            "c",
            "1",
            MessagePage {
                items: vec![msg("1", "2026-11-20T12:00:00Z", "m-bob", "a")],
                has_newer: Some(true),
                ..Default::default()
            },
        );
        w.queue_send(PendingSend {
            client_id: "k".into(),
            conversation_id: "c".into(),
            text: "hi".into(),
            action: false,
            state: SendState::Sending,
            created_ms: 0,
            ..Default::default()
        });
        let b = w.begin_newer("c").unwrap();
        let rows = build(&w, "c", Options::default());
        assert_eq!(rows.last(), Some(&Row::NewerLoading));
        w.load_newer("c", &b, Err("offline".into()));
        let rows = build(&w, "c", Options::default());
        assert_eq!(
            rows.last(),
            Some(&Row::NewerFailed),
            "sends wait for the present"
        );
    }

    /// The shape the live backend returned after a restart
    /// (`live-general-history.json` in the V1 evidence): each replayed join
    /// is a membership event followed by the history service's `system`
    /// line saying the same.
    #[test]
    fn replayed_join_lines_neither_show_nor_break_the_fold() {
        let mut w = workspace();
        let people = ["mika", "jules", "noah", "sage", "ada", "lin", "ellis"];
        let mut items = vec![msg("hi", "2026-11-20T12:00:00Z", "m-bob", "morning")];
        for (i, nick) in people.iter().enumerate() {
            let at = format!("2026-11-20T12:01:{:02}.{:03}Z", i / 2, (i * 51) % 1000);
            items.push(event(
                &format!("j{i}"),
                &at,
                &format!("m-{nick}"),
                MembershipEventType::Join,
            ));
            let mut line = msg(
                &format!("_s{i}"),
                &at,
                "m-histserv",
                &format!("{nick} joined the channel"),
            );
            line.kind = MessageKind::System;
            line.historical = Some(true);
            line.upstream_id = Some(format!("_s{i}"));
            items.push(line);
        }
        load(&mut w, items, None);
        let rows = build(&w, "c", Options::default());
        let folds: Vec<&FoldRow> = rows
            .iter()
            .filter_map(|r| match r {
                Row::Fold(f) => Some(f.as_ref()),
                _ => None,
            })
            .collect();
        assert_eq!(folds.len(), 1, "{rows:?}");
        assert_eq!(folds[0].summary, "7 people joined");
        assert_eq!(folds[0].events.len(), 7);
        assert!(!rows.iter().any(|r| matches!(r, Row::Event(_))), "{rows:?}");
        let shown = format!("{rows:?}");
        assert!(!shown.contains("joined the channel"), "{shown}");

        // The "New messages" line on a skipped line moves to the next row.
        let rows = build(
            &w,
            "c",
            Options {
                unread: Some("_s1"),
                ..Default::default()
            },
        );
        let line = rows
            .iter()
            .position(|r| *r == Row::Unread)
            .expect("the line");
        assert!(matches!(&rows[line - 1], Row::Event(e) if e.id == "j0"));
        assert!(matches!(&rows[line + 1], Row::Fold(f) if f.id == "j1"));
    }

    #[test]
    fn a_pause_a_reply_and_a_change_of_kind_start_a_new_header() {
        let mut w = workspace();
        let mut reply = msg("3", "2026-11-20T12:02:00Z", "m-bob", "re");
        reply.reply_to = Some(Reply {
            id: "1".into(),
            nick: "bob".into(),
            text: "a".into(),
        });
        let mut notice = msg("4", "2026-11-20T12:03:00Z", "m-bob", "n");
        notice.kind = MessageKind::Notice;
        load(
            &mut w,
            vec![
                msg("1", "2026-11-20T12:00:00Z", "m-bob", "a"),
                msg("2", "2026-11-20T12:01:00Z", "m-bob", "b"),
                reply,
                notice,
                msg("5", "2026-11-20T12:04:00Z", "m-bob", "c"),
                msg("6", "2026-11-20T12:08:59Z", "m-bob", "d"),
                // Five minutes after the last: a new header.
                msg("7", "2026-11-20T12:13:59Z", "m-bob", "e"),
            ],
            None,
        );
        let headers: Vec<bool> = build(&w, "c", Options::default())
            .iter()
            .filter_map(|r| match r {
                Row::Message(m) => Some(m.header),
                _ => None,
            })
            .collect();
        assert_eq!(headers, [true, false, true, true, true, false, true]);
    }

    #[test]
    fn days_read_today_yesterday_or_their_date() {
        let today = NaiveDate::from_ymd_opt(2026, 11, 21).unwrap();
        let day = |y, m, d| day_label(NaiveDate::from_ymd_opt(y, m, d).unwrap(), today);
        assert_eq!(day(2026, 11, 21), "Today");
        assert_eq!(day(2026, 11, 20), "Yesterday");
        assert_eq!(day(2026, 11, 19), "Thursday, November 19");
        assert_eq!(day(2025, 12, 31), "Wednesday, December 31, 2025");

        let mut w = workspace();
        load(
            &mut w,
            vec![msg("1", "2026-11-20T12:00:00Z", "m-bob", "a")],
            None,
        );
        let local = local_time(
            chrono::DateTime::parse_from_rfc3339("2026-11-20T12:00:00Z")
                .unwrap()
                .timestamp_millis(),
        )
        .date_naive();
        let rows = build(
            &w,
            "c",
            Options {
                today: Some(local),
                ..Default::default()
            },
        );
        assert!(rows.contains(&Row::Day("Today".into())), "{rows:?}");
    }

    #[test]
    fn the_start_names_the_channel_and_its_topic() {
        let mut w = workspace();
        w.conversations[0].topic = "\x02Welcome\x02 aboard".into();
        load(&mut w, vec![], None);
        let rows = build(&w, "c", Options::default());
        let Row::Start(intro) = &rows[0] else {
            panic!("{rows:?}")
        };
        assert_eq!(intro.kind, IntroKind::Channel);
        assert_eq!(intro.title, "#x");
        assert_eq!(intro.text, "This is the beginning of #x.");
        assert_eq!(intro.topic, "Welcome aboard");
        let before = rows[0].key();
        w.conversations[0].topic = "Changed".into();
        let rows = build(&w, "c", Options::default());
        assert_ne!(rows[0].key(), before, "a new topic redraws the start");

        w.conversations[0].kind = ConversationKind::Direct;
        let rows = build(&w, "c", Options::default());
        let Row::Start(intro) = &rows[0] else {
            panic!()
        };
        assert_eq!(intro.kind, IntroKind::Direct);
        assert_eq!(intro.text, "This is the beginning of your direct messages.");
    }

    #[test]
    fn event_lines_use_nicks() {
        let mut w = workspace();
        w.members.get_mut("m-bob").unwrap().display_name = "Bob Builder".into();
        joins_and_parts(&mut w);
        let rows = build(&w, "c", Options::default());
        let Some(Row::Fold(f)) = rows.iter().find(|r| matches!(r, Row::Fold(_))) else {
            panic!()
        };
        assert!(f.summary.starts_with("bob joined"), "{}", f.summary);
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
#[test]
fn mode_events_describe_changes_without_showing_channel_keys() {
    assert_eq!(
        mode_description("+q PeterPowers", &BTreeMap::new()),
        "made PeterPowers a founder"
    );
    assert_eq!(
        mode_description("+ko-v private-key Alice Bob", &BTreeMap::new()),
        "set a channel key, made Alice an operator and took voice from Bob"
    );
    assert_eq!(mode_description("+X", &BTreeMap::new()), "set mode +X");
}

#[test]
fn mode_arguments_follow_the_networks_chanmodes_and_prefix() {
    let isupport = |pairs: &[(&str, &str)]| -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    };
    // `L` always takes an argument here, so the key stays with `k`.
    let overflow = isupport(&[("CHANMODES", "beI,kL,l,imnpst"), ("PREFIX", "(ov)@+")]);
    let text = mode_description("+Lko #overflow private-key Alice", &overflow);
    assert!(!text.contains("private-key"), "{text}");
    assert_eq!(
        text,
        "set a channel key, made Alice an operator and set mode +L #overflow"
    );
    // `q` is a quiet list where PREFIX has no founder.
    let quiets = isupport(&[("CHANMODES", "beIq,k,l,imnpst"), ("PREFIX", "(ov)@+")]);
    assert_eq!(
        mode_description("+q *!*@spam.example", &quiets),
        "set mode +q *!*@spam.example"
    );
    // A limit is removed without an argument; the next one keeps its own.
    assert_eq!(
        mode_description("-l+v Alice", &BTreeMap::new()),
        "removed the user limit and gave voice to Alice"
    );
}

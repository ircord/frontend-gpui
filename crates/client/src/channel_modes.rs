//! Channel modes, list masks and the mask builder (package CH2), ported from
//! the web client's `shared/utils/channel-modes.ts` and
//! `shared/utils/mask-builder.ts` (frontend `fbf62ee`).
//!
//! ISUPPORT `CHANMODES` says which letters exist and how they take
//! parameters (see [`Chanmodes`]); what a letter means is up to the server
//! software, so common letters are described and the rest are shown as
//! server-specific. Masks are explained before they are added: what typed
//! text becomes ([`to_mask`]), what it covers in words ([`preview_mask`]),
//! and how broad it is ([`broadness`]). The mask builder makes a mask from a
//! person, and reads back which of their parts a mask pins, so the mask stays
//! the one source of truth.
//!
//! Nothing here holds text a person reads: the results are data the app
//! words through its catalog. The IRC lines the settings send are built
//! here too, so every one of them is tested as data.

use std::collections::BTreeMap;

pub use crate::api::channels::Chanmodes;
use crate::format::markdown::markdown_to_irc;

// ---- Modes ----

/// Common letters described by their most common server meaning; the
/// [`varies`] ones mean different things on different server software.
const KNOWN: &str = "ntmisrklRcCSTzgPQFufjL";
const VARIES: &str = "rRcCSTzgPQFufjL";

/// Whether the app describes `mode`; others show as server-specific.
pub fn known(mode: char) -> bool {
    KNOWN.contains(mode)
}

/// Whether what `mode` does depends on the server.
pub fn varies(mode: char) -> bool {
    VARIES.contains(mode)
}

/// A network's switches, the described ones first, each group A to Z.
pub fn sorted_flags(modes: &Chanmodes) -> Vec<char> {
    let mut out: Vec<char> = modes.flags.chars().collect();
    out.sort_by(|a, b| known(*b).cmp(&known(*a)).then(a.cmp(b)));
    out
}

/// The modes that take a value: always, then only when set.
pub fn value_modes(modes: &Chanmodes) -> Vec<char> {
    modes.parameters()
}

/// The list modes the app explains: bans, ban exceptions, invite exceptions
/// and quiets. Other list letters are the server's own lists.
pub fn described_list(mode: char) -> bool {
    matches!(mode, 'b' | 'e' | 'I' | 'q')
}

/// A channel rank, named from the highest prefix someone has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rank {
    Owner,
    Admin,
    Operator,
    HalfOperator,
    Voiced,
    Member,
}

impl Rank {
    pub fn of(prefix: Option<char>) -> Self {
        match prefix {
            Some('~') => Rank::Owner,
            Some('&') => Rank::Admin,
            Some('@') => Rank::Operator,
            Some('%') => Rank::HalfOperator,
            Some('+') => Rank::Voiced,
            _ => Rank::Member,
        }
    }
}

/// The longest topic the network takes (`TOPICLEN`, else 390).
pub fn topic_limit(isupport: &BTreeMap<String, String>) -> usize {
    isupport
        .get("TOPICLEN")
        .and_then(|v| v.trim().parse().ok())
        .filter(|n| *n > 0)
        .unwrap_or(390)
}

// ---- The lines settings send ----

/// `TOPIC #chan :text`, with Markdown sent as IRC formatting.
pub fn topic_line(channel: &str, text: &str) -> String {
    format!("TOPIC {channel} :{}", markdown_to_irc(text.trim()))
}

/// Turn a switch on or off.
pub fn toggle_line(channel: &str, mode: char, on: bool) -> String {
    format!("MODE {channel} {}{mode}", if on { '+' } else { '-' })
}

/// Set a mode with a value; `None` for an empty value.
pub fn set_value_line(channel: &str, mode: char, value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty() && !value.contains(char::is_whitespace))
        .then(|| format!("MODE {channel} +{mode} {value}"))
}

/// Remove a mode with a value. Removing one that always takes a value (the
/// key) needs it on some servers: the current one is sent when known.
pub fn clear_value_line(
    channel: &str,
    mode: char,
    modes: &Chanmodes,
    current: Option<&str>,
) -> String {
    if modes.always.contains(mode) {
        let current = current.filter(|v| !v.is_empty()).unwrap_or("*");
        format!("MODE {channel} -{mode} {current}")
    } else {
        format!("MODE {channel} -{mode}")
    }
}

/// Add a mask to a list mode, or remove one.
pub fn list_line(channel: &str, mode: char, mask: &str, add: bool) -> String {
    format!(
        "MODE {channel} {}{mode} {mask}",
        if add { '+' } else { '-' }
    )
}

/// Give or take a status (`o`, `v`) from someone.
pub fn status_line(channel: &str, mode: char, nick: &str, give: bool) -> String {
    format!(
        "MODE {channel} {}{mode} {nick}",
        if give { '+' } else { '-' }
    )
}

/// `KICK #chan nick :reason`; no reason without one.
pub fn kick_line(channel: &str, nick: &str, reason: &str) -> String {
    let reason = reason.trim();
    if reason.is_empty() {
        format!("KICK {channel} {nick}")
    } else {
        format!("KICK {channel} {nick} :{reason}")
    }
}

/// The ban the member menu sets: their nick, from anywhere.
pub fn ban_line(channel: &str, nick: &str) -> String {
    list_line(channel, 'b', &format!("{nick}!*@*"), true)
}

// ---- Extended masks ----
// Verified against Ergo (v2.19) by the web client: `alice` is stored as
// `alice!*@*`, `bob@host` as `*!bob@host`, `carol!id` as `carol!id@*`, and an
// extended mask the network doesn't advertise is read as a nickname
// (`$a:erin` → `$a:erin!*@*`). Expanding the same way here shows what will
// be stored.

/// ISUPPORT `EXTBAN=<prefix>,<types>`: extended masks such as `$a:account`
/// (Solanum) or `m:` (Ergo).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Extbans {
    pub prefix: String,
    pub types: String,
}

impl Extbans {
    pub fn parse(isupport: &BTreeMap<String, String>) -> Option<Self> {
        let value = isupport.get("EXTBAN")?;
        let (prefix, types) = match value.split_once(',') {
            Some((p, rest)) => (p, rest.split(',').next().unwrap_or("")),
            None => ("", value.as_str()),
        };
        Some(Self {
            prefix: prefix.to_string(),
            types: types.to_string(),
        })
    }

    /// The account extban where it is verified: `$a:` on Solanum and
    /// Charybdis-style servers, `~a:` on UnrealIRCd.
    pub fn account(&self) -> Option<String> {
        ((self.prefix == "$" || self.prefix == "~") && self.types.contains('a'))
            .then(|| format!("{}a:", self.prefix))
    }

    /// The mute extban `m:<mask>` (Ergo, InspIRCd): matching people stay but
    /// cannot talk.
    pub fn mute(&self) -> Option<&'static str> {
        (self.prefix.is_empty() && self.types.contains('m')).then_some("m:")
    }
}

/// The account extban prefix of a network, if it has one.
pub fn account_prefix(extbans: Option<&Extbans>) -> Option<String> {
    extbans.and_then(Extbans::account)
}

/// An extended mask split into its type letter and argument.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Extban {
    pub kind: char,
    pub arg: String,
    /// `$~a`: everyone not matching `$a` (on `$` networks).
    pub negated: bool,
}

/// `value` as an extended mask the network advertises, if it is one.
pub fn extban_of(value: &str, x: Option<&Extbans>) -> Option<Extban> {
    let x = x?;
    if x.types.is_empty() {
        return None;
    }
    let rest = value.strip_prefix(x.prefix.as_str())?;
    let (negated, rest) = match rest.strip_prefix('~') {
        Some(r) if x.prefix == "$" => (true, r),
        _ => (false, rest),
    };
    let mut chars = rest.chars();
    let kind = chars.next()?;
    if !x.types.contains(kind) {
        return None;
    }
    let rest = chars.as_str();
    // Prefixless extbans need the colon (`m:alice`), or every nick starting
    // with m would match.
    if let Some(arg) = rest.strip_prefix(':') {
        return Some(Extban {
            kind,
            arg: arg.to_string(),
            negated,
        });
    }
    (!x.prefix.is_empty() && rest.is_empty()).then(|| Extban {
        kind,
        arg: String::new(),
        negated,
    })
}

/// Solanum's ban forwarding, `mask$#channel`: split off the target so the
/// mask part can still be expanded. Only `$` networks have it.
fn split_forward<'a>(value: &'a str, x: Option<&Extbans>) -> (&'a str, &'a str) {
    if x.is_none_or(|x| x.prefix != "$") {
        return (value, "");
    }
    match value.find("$#") {
        Some(at) if at > 0 => (&value[..at], &value[at + 1..]),
        _ => (value, ""),
    }
}

/// Fill the missing parts of `nick!user@host` with `*`, as servers do.
pub fn expand_mask(value: &str) -> String {
    let v = value.trim();
    let (left, host) = match v.rfind('@') {
        Some(at) => (&v[..at], Some(&v[at + 1..])),
        None => (v, None),
    };
    let (nick, user) = match left.find('!') {
        Some(bang) => (&left[..bang], &left[bang + 1..]),
        // `bob@host` names a user, not a nick.
        None if host.is_some() => ("", left),
        None => (left, ""),
    };
    let or_any = |s: &str| {
        if s.is_empty() {
            "*".to_string()
        } else {
            s.to_string()
        }
    };
    format!(
        "{}!{}@{}",
        or_any(nick),
        or_any(user),
        or_any(host.unwrap_or(""))
    )
}

/// What is sent for typed text. With the network's EXTBAN known, anything
/// starting with its prefix goes as typed (only the prefixless mute
/// extban's mask is expanded, as Ergo does), a `$#channel` forward is kept
/// after the expanded mask, and anything else is a mask. Without EXTBAN,
/// text containing `$` is left alone, since its meaning is unknown.
pub fn to_mask(value: &str, extbans: Option<&Extbans>) -> String {
    let v = value.trim();
    let Some(x) = extbans else {
        return if v.contains('$') {
            v.to_string()
        } else {
            expand_mask(v)
        };
    };
    if let Some(e) = extban_of(v, Some(x)) {
        return if e.kind == 'm' && x.prefix.is_empty() {
            format!("m:{}", expand_mask(&e.arg))
        } else {
            v.to_string()
        };
    }
    if !x.prefix.is_empty() && v.starts_with(x.prefix.as_str()) {
        return v.to_string();
    }
    match split_forward(v, Some(x)) {
        (mask, forward) if !forward.is_empty() => format!("{}${forward}", expand_mask(mask)),
        _ => expand_mask(v),
    }
}

/// IRC wildcard matching: `*` is any run of characters, `?` exactly one;
/// case is ignored.
pub fn mask_matches(mask: &str, identity: &str) -> bool {
    let p: Vec<char> = mask.to_lowercase().chars().collect();
    let s: Vec<char> = identity.to_lowercase().chars().collect();
    let (mut pi, mut si) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while si < s.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == s[si]) {
            pi += 1;
            si += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some((pi, si));
            pi += 1;
        } else if let Some((sp, ss)) = star {
            pi = sp + 1;
            si = ss + 1;
            star = Some((sp, ss + 1));
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|c| *c == '*')
}

fn only_wildcards(part: &str) -> bool {
    part.chars().all(|c| c == '*' || c == '?')
}

fn has_wildcard(part: &str) -> bool {
    part.contains(['*', '?'])
}

/// `nick!user@host` split as the web client splits it: missing parts are `*`.
fn three_parts(mask: &str) -> (&str, &str, &str) {
    let mut bang = mask.split('!');
    let nick = bang.next().unwrap_or("*");
    let rest = bang.next().unwrap_or("*@*");
    let mut at = rest.split('@');
    let user = at.next().unwrap_or("*");
    let host = at.next().unwrap_or("*");
    (nick, user, host)
}

/// Why a mask is broad.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Broad {
    /// It matches everyone, including you.
    Everyone,
    /// The host can match a whole country, provider or address range.
    Host,
    /// The nick matches every nick with those few letters.
    Nick,
}

/// Why a `nick!user@host` mask is broad, if it is: judged by what it pins
/// down — a host with at most one literal label (`*.com`, `203.*`), a
/// country domain (`*.co.uk`), or a nick of one or two letters plus a
/// wildcard with nothing else pinned.
pub fn broadness(mask: &str) -> Option<Broad> {
    let (nick, user, host) = three_parts(mask);
    if only_wildcards(nick) && only_wildcards(user) && only_wildcards(host) {
        return Some(Broad::Everyone);
    }
    if !only_wildcards(user) {
        return None;
    }
    if only_wildcards(nick) && has_wildcard(host) {
        let literal: Vec<&str> = host.split('.').filter(|l| !only_wildcards(l)).collect();
        let country = literal.len() == 2
            && literal[1].len() == 2
            && literal[1].chars().all(|c| c.is_ascii_alphabetic())
            && literal[0]
                .chars()
                .filter(|c| !matches!(c, '*' | '?'))
                .count()
                <= 3;
        let ip = literal.len() <= 2
            && literal
                .iter()
                .all(|l| !l.is_empty() && l.chars().all(|c| c.is_ascii_digit()));
        if literal.len() <= 1 || country || ip {
            return Some(Broad::Host);
        }
    }
    if only_wildcards(host)
        && has_wildcard(nick)
        && nick.chars().filter(|c| !matches!(c, '*' | '?')).count() <= 2
    {
        return Some(Broad::Nick);
    }
    None
}

/// Which part of `nick!user@host` a mask pins.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartKind {
    Nick,
    User,
    Host,
}

/// One part a mask pins: the exact value, or a pattern when it has wildcards.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Part {
    pub kind: PartKind,
    pub value: String,
    pub pattern: bool,
}

/// The parts of `nick!user@host` the mask actually pins down; empty when
/// it matches everyone.
pub fn pinned_parts(mask: &str) -> Vec<Part> {
    let (nick, user, host) = three_parts(mask);
    [
        (PartKind::Nick, nick),
        (PartKind::User, user),
        (PartKind::Host, host),
    ]
    .into_iter()
    .filter(|(_, v)| !only_wildcards(v))
    .map(|(kind, v)| Part {
        kind,
        value: v.to_string(),
        pattern: has_wildcard(v),
    })
    .collect()
}

/// What a mask covers, for the person to read before adding it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scope {
    /// Anyone logged in to `account` (any account when empty), or with
    /// `negated` anyone who is not.
    Account { account: String, negated: bool },
    /// Mutes the people the parts match: they stay but cannot talk.
    Mute(Vec<Part>),
    /// A server-specific extended mask, sent as typed.
    Extended { negated: bool },
    /// Matches the people the parts match.
    Matches(Vec<Part>),
    /// Matches them and sends them to the channel instead.
    MatchesForward(Vec<Part>, String),
}

/// Why typed text cannot be added.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaskError {
    Spaces,
    /// Starts with `:` or has a comma.
    Prefix,
}

/// Something worth knowing before adding a mask.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Warning {
    Broad(Broad),
    /// The network does not advertise extended masks; `$` text is sent as typed.
    UnknownExtban,
    /// The network does not advertise this type after its prefix.
    UnknownType(String),
    /// `$…` or `~…` where the network offers no such extended masks: it is
    /// read as a nickname.
    NoExtendedType(char),
}

/// The kind of an extended mask the network advertises.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtbanKind {
    Account,
    Mute,
    Other,
}

/// Typed text explained before it is added to a list.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MaskPreview {
    /// What Add sends; empty when there is nothing to send.
    pub mask: String,
    pub error: Option<MaskError>,
    /// The typed text was shorthand and was expanded.
    pub expanded: bool,
    pub extban: Option<ExtbanKind>,
    pub scope: Option<Scope>,
    pub warnings: Vec<Warning>,
}

/// Explain what typed text becomes before it is added to a list.
pub fn preview_mask(value: &str, extbans: Option<&Extbans>) -> MaskPreview {
    let v = value.trim();
    if v.is_empty() {
        return MaskPreview::default();
    }
    if v.contains(char::is_whitespace) {
        return MaskPreview {
            error: Some(MaskError::Spaces),
            ..Default::default()
        };
    }
    if v.starts_with(':') || v.contains(',') {
        return MaskPreview {
            error: Some(MaskError::Prefix),
            ..Default::default()
        };
    }
    let mask = to_mask(v, extbans);
    let expanded = mask != v;
    let mut warnings = Vec::new();
    if let Some(x) = extbans
        && let Some(e) = extban_of(v, Some(x))
    {
        if x.account().as_deref() == Some(format!("{}{}:", x.prefix, e.kind).as_str()) {
            return MaskPreview {
                mask,
                expanded,
                extban: Some(ExtbanKind::Account),
                scope: Some(Scope::Account {
                    account: e.arg,
                    negated: e.negated,
                }),
                ..Default::default()
            };
        }
        if e.kind == 'm' && x.mute().is_some() {
            let inner = &mask[2..];
            warnings.extend(broadness(inner).map(Warning::Broad));
            return MaskPreview {
                scope: Some(Scope::Mute(pinned_parts(inner))),
                mask,
                expanded,
                extban: Some(ExtbanKind::Mute),
                warnings,
                ..Default::default()
            };
        }
        return MaskPreview {
            mask,
            expanded,
            extban: Some(ExtbanKind::Other),
            scope: Some(Scope::Extended { negated: e.negated }),
            ..Default::default()
        };
    }
    match extbans {
        None if v.contains('$') => {
            return MaskPreview {
                mask,
                expanded,
                warnings: vec![Warning::UnknownExtban],
                ..Default::default()
            };
        }
        Some(x) if !x.prefix.is_empty() && v.starts_with(x.prefix.as_str()) => {
            return MaskPreview {
                mask,
                expanded,
                extban: Some(ExtbanKind::Other),
                warnings: vec![Warning::UnknownType(x.prefix.clone())],
                ..Default::default()
            };
        }
        Some(_) if v.starts_with(['$', '~']) => {
            warnings.push(Warning::NoExtendedType(v.chars().next().unwrap_or('$')));
        }
        _ => {}
    }
    let (plain, forward) = split_forward(&mask, extbans);
    warnings.extend(broadness(plain).map(Warning::Broad));
    let parts = pinned_parts(plain);
    let scope = if forward.is_empty() {
        Scope::Matches(parts)
    } else {
        Scope::MatchesForward(parts, forward.to_string())
    };
    MaskPreview {
        mask,
        expanded,
        scope: Some(scope),
        warnings,
        ..Default::default()
    }
}

/// Whether someone in the channel matches `mask` now. People whose username
/// and host are not known yet only count when the mask does not look at them.
pub fn matches_person(mask: &str, nick: &str, username: Option<&str>, host: Option<&str>) -> bool {
    let mut split = mask.splitn(2, '!');
    let nick_part = split.next().unwrap_or("*");
    let rest = split.next().unwrap_or("*@*");
    match (username, host) {
        (Some(u), Some(h)) if !u.is_empty() && !h.is_empty() => {
            mask_matches(mask, &format!("{nick}!{u}@{h}"))
        }
        _ => {
            let loose = rest
                .split_once('@')
                .is_some_and(|(u, h)| only_wildcards(u) && only_wildcards(h));
            loose && mask_matches(nick_part, nick)
        }
    }
}

// ---- Building a mask from someone ----

/// The parts of a person a mask can be built from.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MaskPerson {
    pub nick: String,
    pub username: Option<String>,
    pub host: Option<String>,
    pub account: Option<String>,
}

impl MaskPerson {
    /// A services account the network reported; `*` and empty mean not
    /// logged in.
    pub fn known_account(&self) -> Option<&str> {
        self.account
            .as_deref()
            .map(str::trim)
            .filter(|a| !a.is_empty() && *a != "*")
    }

    fn username(&self) -> Option<&str> {
        self.username.as_deref().filter(|u| !u.is_empty())
    }

    fn host(&self) -> Option<&str> {
        self.host.as_deref().filter(|h| !h.is_empty())
    }
}

/// Which of a person's parts a mask pins.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MaskParts {
    pub nick: bool,
    pub user: bool,
    pub host: bool,
    pub account: bool,
}

impl MaskParts {
    pub fn count(&self) -> usize {
        [self.nick, self.user, self.host, self.account]
            .iter()
            .filter(|b| **b)
            .count()
    }

    pub fn get(&self, part: MaskPart) -> bool {
        match part {
            MaskPart::Nick => self.nick,
            MaskPart::User => self.user,
            MaskPart::Host => self.host,
            MaskPart::Account => self.account,
        }
    }
}

/// A part the builder ticks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaskPart {
    Nick,
    User,
    Host,
    Account,
}

/// `nick!user@host` the way [`expand_mask`] writes it; anything else is `None`.
fn split_mask(mask: &str) -> Option<(&str, &str, &str)> {
    let bang = mask.find('!')?;
    let at = mask.rfind('@')?;
    (bang > 0 && at > bang).then(|| (&mask[..bang], &mask[bang + 1..at], &mask[at + 1..]))
}

/// Which parts of `person` the mask pins exactly. A part counts only when it
/// is the person's own value, so a hand-edited wildcard (`*.example.net`)
/// leaves its box unticked.
pub fn parts_of(mask: &str, person: &MaskPerson, account_prefix: Option<&str>) -> MaskParts {
    let m = mask.trim();
    if let (Some(prefix), Some(account)) = (account_prefix, person.known_account())
        && m.to_lowercase() == format!("{prefix}{account}").to_lowercase()
    {
        return MaskParts {
            account: true,
            ..Default::default()
        };
    }
    let Some((nick, user, host)) = split_mask(m) else {
        return MaskParts::default();
    };
    let same = |part: &str, value: Option<&str>| {
        value.is_some_and(|v| !v.is_empty() && part.to_lowercase() == v.to_lowercase())
    };
    MaskParts {
        nick: same(nick, Some(&person.nick)),
        user: same(user, person.username()),
        host: same(host, person.host()),
        account: false,
    }
}

/// The mask for `person` with only the ticked parts pinned and the rest `*`.
/// An account mask stands alone: servers cannot combine it with the others.
pub fn build_mask(person: &MaskPerson, parts: MaskParts, account_prefix: Option<&str>) -> String {
    if parts.account
        && let (Some(prefix), Some(account)) = (account_prefix, person.known_account())
    {
        return format!("{prefix}{account}");
    }
    let nick = if parts.nick {
        person.nick.as_str()
    } else {
        "*"
    };
    let user = person.username().filter(|_| parts.user).unwrap_or("*");
    let host = person.host().filter(|_| parts.host).unwrap_or("*");
    format!("{nick}!{user}@{host}")
}

/// Turn one part on or off, starting from the parts the mask pins. The
/// account replaces the address parts; an address part drops the account.
pub fn toggled_mask(
    mask: &str,
    person: &MaskPerson,
    part: MaskPart,
    on: bool,
    account_prefix: Option<&str>,
) -> String {
    let current = parts_of(mask, person, account_prefix);
    let next = match part {
        MaskPart::Account => MaskParts {
            account: on,
            ..Default::default()
        },
        MaskPart::Nick => MaskParts {
            nick: on,
            account: false,
            ..current
        },
        MaskPart::User => MaskParts {
            user: on,
            account: false,
            ..current
        },
        MaskPart::Host => MaskParts {
            host: on,
            account: false,
            ..current
        },
    };
    build_mask(person, next, account_prefix)
}

/// The mask a newly chosen person starts with: their account where the
/// network has account masks and reports one (it follows them across nicks
/// and hosts), else their host, else their nick.
pub fn default_mask(person: &MaskPerson, account_prefix: Option<&str>) -> String {
    let parts = if account_prefix.is_some() && person.known_account().is_some() {
        MaskParts {
            account: true,
            ..Default::default()
        }
    } else if person.host().is_some() {
        MaskParts {
            host: true,
            ..Default::default()
        }
    } else {
        MaskParts {
            nick: true,
            ..Default::default()
        }
    };
    build_mask(person, parts, account_prefix)
}

#[cfg(test)]
mod tests;

//! The conversation list's own rules, ported from the web client
//! (`shared/utils/sidebar-order.ts`, `sidebar.ts`, `navigation.ts`,
//! `network-glyph.ts` and `composables/workspace/useSidebarOrder.ts`):
//!
//! - Favourites, Channels and Direct messages. Favourites and channels start
//!   A to Z; after you drag or move one, the section keeps your order (stored
//!   on the account, in the preferences document's `sidebarOrder`), and
//!   conversations the order does not list yet go to the bottom, A to Z.
//! - A collapsed section still lists the open conversation and the ones that
//!   want attention. Collapsed sections belong to the device, not the account.
//! - Ctrl+1 to 9 open favourite channels, then channels, in sidebar order;
//!   collapsing and the unread filter never renumber them.
//! - A network without its own icon gets a mirrored 5×5 pattern and a hue
//!   from its name.
//!
//! Owned by package N1. Everything here is a pure function of the
//! workspace, so the app draws from it and tests check it without a window.

use std::cmp::Ordering;
use std::collections::BTreeMap;

use ircord_contract::event::ServerEvent;
use ircord_contract::{
    ChannelSettingsNotifications, ClientPreferences, Conversation, ConversationKind, Network,
    SettingsChannelDefault, SettingsDirectDefault, SettingsLevelsValue, Snapshot,
    StoredPreferences,
};

use super::{Changes, Workspace};

/// The topic this sub-state reports in [`Changes`].
pub const TOPIC: &str = "sidebar_order";

/// Most stored section orders, and conversations per order (the server's limits).
pub const MAX_ORDERS: usize = 50;
pub const MAX_ORDER_IDS: usize = 200;
/// Most collapsed sections remembered on a device; the oldest are forgotten first.
pub const MAX_COLLAPSED: usize = 200;
/// Ctrl+1 to Ctrl+9.
pub const NUMBERED: usize = 9;

#[derive(Debug, Clone, Default)]
pub struct State {}

/// Called for every fresh event after the workspace's own handling.
pub(super) fn apply(_w: &mut Workspace, _event: &ServerEvent, _now_ms: i64) -> Changes {
    Changes::default()
}

/// Called when a bootstrap replaces the workspace, before it is replaced.
pub(super) fn reset(_w: &mut Workspace, _snapshot: &Snapshot) -> Changes {
    Changes::default()
}

/// The sidebar's layout on this device (screens differ, so it is not
/// stored on the account): panel widths, whether the conversation list is
/// docked or folded to an icon rail, collapsed sections and the unread filter.
/// Saved in the device state's `sidebar` section.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Layout {
    pub sidebar_width: f32,
    pub panel_width: f32,
    /// The conversation list is docked; off folds it to an icon rail.
    pub show_list: bool,
    /// Collapsed sections, as [`section_key`].
    pub collapsed: Vec<String>,
    /// Show only conversations with something unread.
    pub unread_only: bool,
}

/// (smallest, largest, default) widths in pixels.
pub const SIDEBAR_WIDTH: (f32, f32, f32) = (170., 360., 200.);
pub const PANEL_WIDTH: (f32, f32, f32) = (180., 340., 208.);

impl Default for Layout {
    fn default() -> Self {
        Self {
            sidebar_width: SIDEBAR_WIDTH.2,
            panel_width: PANEL_WIDTH.2,
            show_list: true,
            collapsed: Vec::new(),
            unread_only: false,
        }
    }
}

/// `value` within `limits`, or the default when it is not a number.
pub fn clamp_width(value: f32, limits: (f32, f32, f32)) -> f32 {
    if value.is_finite() {
        value.clamp(limits.0, limits.1).round()
    } else {
        limits.2
    }
}

impl Layout {
    /// The layout with widths in range and the collapsed list bounded.
    pub fn sanitized(mut self) -> Self {
        self.sidebar_width = clamp_width(self.sidebar_width, SIDEBAR_WIDTH);
        self.panel_width = clamp_width(self.panel_width, PANEL_WIDTH);
        let mut seen = std::collections::BTreeSet::new();
        self.collapsed
            .retain(|k| !k.is_empty() && k.len() <= 300 && seen.insert(k.clone()));
        let extra = self.collapsed.len().saturating_sub(MAX_COLLAPSED);
        self.collapsed.drain(..extra);
        self
    }
}

/// A section of the conversation list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Section {
    Favorites,
    Channels,
    Directs,
}

impl Section {
    pub const ALL: [Section; 3] = [Section::Favorites, Section::Channels, Section::Directs];

    /// The ID used in stored keys, as the web client writes it.
    pub fn id(self) -> &'static str {
        match self {
            Section::Favorites => "favorites",
            Section::Channels => "channels",
            Section::Directs => "directs",
        }
    }

    /// Favourites and channels keep your order; direct messages keep their own.
    pub fn sortable(self) -> bool {
        self != Section::Directs
    }
}

/// Which section lists `c`.
pub fn section_of(c: &Conversation) -> Section {
    if c.favorite {
        Section::Favorites
    } else if c.kind == ConversationKind::Channel && c.party.is_none() {
        Section::Channels
    } else {
        Section::Directs
    }
}

/// `network/section`, the key of a stored order or a collapsed section.
pub fn section_key(network_id: &str, section: Section) -> String {
    format!("{network_id}/{}", section.id())
}

/// Natural, case-insensitive name order ("#chan2" before "#chan10").
pub fn compare_names(a: &str, b: &str) -> Ordering {
    let (lower_a, lower_b) = (a.to_lowercase(), b.to_lowercase());
    let (mut x, mut y) = (lower_a.chars().peekable(), lower_b.chars().peekable());
    loop {
        match (x.peek().copied(), y.peek().copied()) {
            (None, None) => return a.cmp(b),
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(p), Some(q)) if p.is_ascii_digit() && q.is_ascii_digit() => {
                let take = |it: &mut std::iter::Peekable<std::str::Chars>| {
                    let mut digits = String::new();
                    while let Some(d) = it.peek().copied().filter(char::is_ascii_digit) {
                        digits.push(d);
                        it.next();
                    }
                    digits
                };
                let (m, n) = (take(&mut x), take(&mut y));
                let (m, n) = (m.trim_start_matches('0'), n.trim_start_matches('0'));
                let order = m.len().cmp(&n.len()).then_with(|| m.cmp(n));
                if order != Ordering::Equal {
                    return order;
                }
            }
            (Some(p), Some(q)) => {
                if p != q {
                    return p.cmp(&q);
                }
                x.next();
                y.next();
            }
        }
    }
}

/// `items` in display order: by name without a stored order; otherwise the
/// stored order, then what it does not list yet, by name.
pub fn order_items<'a>(
    mut items: Vec<&'a Conversation>,
    order: Option<&[String]>,
) -> Vec<&'a Conversation> {
    items.sort_by(|a, b| compare_names(&a.name, &b.name));
    let Some(order) = order else {
        return items;
    };
    let position = |id: &str| order.iter().position(|o| o == id);
    let (mut known, rest): (Vec<_>, Vec<_>) =
        items.into_iter().partition(|c| position(&c.id).is_some());
    known.sort_by_key(|c| position(&c.id));
    known.extend(rest);
    known
}

/// Where a dragged conversation lands next to its target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Place {
    Before,
    After,
}

/// The IDs with `id` moved next to `target`. Unchanged when either is
/// missing or both are the same.
pub fn move_id(ids: &[String], id: &str, target: &str, place: Place) -> Vec<String> {
    if id == target || !ids.iter().any(|i| i == id) || !ids.iter().any(|i| i == target) {
        return ids.to_vec();
    }
    let mut rest: Vec<String> = ids.iter().filter(|i| *i != id).cloned().collect();
    let at =
        rest.iter().position(|i| i == target).unwrap_or(0) + usize::from(place == Place::After);
    rest.insert(at, id.to_string());
    rest
}

/// The IDs with `id` one step up (-1) or down (1); unchanged at either end.
pub fn step_id(ids: &[String], id: &str, step: i32) -> Vec<String> {
    let Some(index) = ids.iter().position(|i| i == id) else {
        return ids.to_vec();
    };
    let target = index as i64 + i64::from(step);
    match usize::try_from(target).ok().and_then(|t| ids.get(t)) {
        Some(target) => {
            let place = if step < 0 {
                Place::Before
            } else {
                Place::After
            };
            move_id(ids, id, target, place)
        }
        None => ids.to_vec(),
    }
}

/// The account's stored section orders, once the preferences were read.
pub fn stored_orders(w: &Workspace) -> Option<&BTreeMap<String, Option<Vec<String>>>> {
    w.documents
        .preferences
        .value()
        .and_then(|p| p.preferences.as_ref())
        .and_then(|p| p.sidebar_order.as_ref())
}

fn stored<'a>(w: &'a Workspace, network_id: &str, section: Section) -> Option<&'a [String]> {
    stored_orders(w)
        .and_then(|o| o.get(&section_key(network_id, section)))
        .and_then(|ids| ids.as_deref())
}

/// Whether a section keeps your own order rather than A to Z.
pub fn custom(w: &Workspace, network_id: &str, section: Section) -> bool {
    section.sortable() && stored(w, network_id, section).is_some()
}

/// Every conversation of a network's section, in display order. Direct
/// messages are listed by name.
pub fn items<'a>(w: &'a Workspace, network_id: &str, section: Section) -> Vec<&'a Conversation> {
    let members: Vec<&Conversation> = w
        .conversations
        .iter()
        .filter(|c| c.network_id == network_id && section_of(c) == section)
        .collect();
    let order = section
        .sortable()
        .then(|| stored(w, network_id, section))
        .flatten();
    order_items(members, order)
}

/// The IDs of a section in display order.
pub fn ids(w: &Workspace, network_id: &str, section: Section) -> Vec<String> {
    items(w, network_id, section)
        .into_iter()
        .map(|c| c.id.clone())
        .collect()
}

/// Every conversation of a network in sidebar order: favourites, channels,
/// direct messages.
pub fn ordered<'a>(w: &'a Workspace, network_id: &str) -> Vec<&'a Conversation> {
    Section::ALL
        .iter()
        .flat_map(|s| items(w, network_id, *s))
        .collect()
}

/// Where `c` can still move by one step: (up, down).
pub fn steps(w: &Workspace, c: &Conversation) -> (bool, bool) {
    let section = section_of(c);
    if !section.sortable() {
        return (false, false);
    }
    let list = ids(w, &c.network_id, section);
    match list.iter().position(|i| *i == c.id) {
        Some(index) => (index > 0, index + 1 < list.len()),
        None => (false, false),
    }
}

/// The preferences with one section's order set, or forgotten with `None`
/// (back to A to Z). Keeps at most [`MAX_ORDERS`] orders.
pub fn with_order(
    current: &StoredPreferences,
    key: &str,
    order: Option<Vec<String>>,
) -> StoredPreferences {
    let mut next = current.clone();
    let prefs = next
        .preferences
        .get_or_insert_with(ClientPreferences::default);
    let orders = prefs.sidebar_order.get_or_insert_with(BTreeMap::new);
    match order {
        Some(mut ids) => {
            ids.truncate(MAX_ORDER_IDS);
            orders.insert(key.to_string(), Some(ids));
        }
        None => {
            orders.remove(key);
        }
    }
    while orders.len() > MAX_ORDERS {
        let Some(first) = orders.keys().find(|k| k.as_str() != key).cloned() else {
            break;
        };
        orders.remove(&first);
    }
    next
}

/// The new preferences after moving `id` next to `target`, or `None` when
/// nothing changes.
pub fn moved(
    w: &Workspace,
    current: &StoredPreferences,
    network_id: &str,
    section: Section,
    id: &str,
    target: &str,
    place: Place,
) -> Option<StoredPreferences> {
    if !section.sortable() {
        return None;
    }
    let before = ids(w, network_id, section);
    let after = move_id(&before, id, target, place);
    (after != before).then(|| with_order(current, &section_key(network_id, section), Some(after)))
}

/// The new preferences after moving `c` one step, or `None` at either end.
pub fn stepped(
    w: &Workspace,
    current: &StoredPreferences,
    c: &Conversation,
    step: i32,
) -> Option<StoredPreferences> {
    let section = section_of(c);
    if !section.sortable() {
        return None;
    }
    let before = ids(w, &c.network_id, section);
    let after = step_id(&before, &c.id, step);
    (after != before)
        .then(|| with_order(current, &section_key(&c.network_id, section), Some(after)))
}

/// The collapsed list with `key` collapsed (added) or expanded (removed).
pub fn toggle_collapsed(list: &[String], key: &str) -> Vec<String> {
    if list.iter().any(|k| k == key) {
        return list.iter().filter(|k| *k != key).cloned().collect();
    }
    let mut next = list.to_vec();
    next.push(key.to_string());
    let extra = next.len().saturating_sub(MAX_COLLAPSED);
    next.drain(..extra);
    next
}

/// A conversation's notification level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    All,
    Mentions,
    None,
}

/// The level your own settings choose for a direct message, if any.
pub fn own_level(w: &Workspace, c: &Conversation) -> Option<Level> {
    let settings = w.documents.notification_settings.value()?;
    match settings.levels.get(&c.id)? {
        SettingsLevelsValue::All => Some(Level::All),
        SettingsLevelsValue::Mentions => Some(Level::Mentions),
        SettingsLevelsValue::None => Some(Level::None),
        SettingsLevelsValue::Unknown => None,
    }
}

/// The level a channel's operators chose for everyone, if not the default.
pub fn channel_level(c: &Conversation) -> Option<Level> {
    match c.channel_settings.as_ref()?.notifications {
        ChannelSettingsNotifications::All => Some(Level::All),
        ChannelSettingsNotifications::Mentions => Some(Level::Mentions),
        ChannelSettingsNotifications::None => Some(Level::None),
        _ => None,
    }
}

/// Whether a conversation's level is chosen channel-wide (a channel) or by
/// you (a direct message or a group).
pub fn channel_wide(c: &Conversation) -> bool {
    c.kind == ConversationKind::Channel && c.party.is_none()
}

/// The default level for conversations like `c`.
pub fn default_level(w: &Workspace, c: &Conversation) -> Level {
    let settings = w.documents.notification_settings.value();
    if channel_wide(c) {
        match settings.map(|s| s.channel_default) {
            Some(SettingsChannelDefault::All) => Level::All,
            Some(SettingsChannelDefault::None) => Level::None,
            _ => Level::Mentions,
        }
    } else {
        match settings.map(|s| s.direct_default) {
            Some(SettingsDirectDefault::Mentions) => Level::Mentions,
            Some(SettingsDirectDefault::None) => Level::None,
            _ => Level::All,
        }
    }
}

/// The level that applies to `c`.
pub fn level(w: &Workspace, c: &Conversation) -> Level {
    let chosen = if channel_wide(c) {
        channel_level(c)
    } else {
        own_level(w, c)
    };
    chosen.unwrap_or_else(|| default_level(w, c))
}

pub fn muted(w: &Workspace, c: &Conversation) -> bool {
    level(w, c) == Level::None
}

/// Whether a conversation has anything unread, a mention or a manual unread mark.
pub fn has_unread(c: &Conversation) -> bool {
    c.unread_count > 0 || c.mention_count > 0 || c.unread_from.is_some()
}

/// What a network's rail dot shows besides its connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Activity {
    Mention,
    Unread,
    Quiet,
}

/// Mentions anywhere in the network, else unread messages that are not muted.
pub fn network_activity(w: &Workspace, network_id: &str) -> Activity {
    let list = w
        .conversations
        .iter()
        .filter(|c| c.network_id == network_id);
    let mut unread = false;
    for c in list {
        if c.mention_count > 0 {
            return Activity::Mention;
        }
        unread |= c.unread_count > 0 && !muted(w, c);
    }
    if unread {
        Activity::Unread
    } else {
        Activity::Quiet
    }
}

/// The conversations of a network (or every network) with something unread.
pub fn unread_in<'a>(w: &'a Workspace, network_id: Option<&str>) -> Vec<&'a Conversation> {
    w.conversations
        .iter()
        .filter(|c| network_id.is_none_or(|n| c.network_id == n) && has_unread(c))
        .collect()
}

/// One section as the sidebar draws it.
#[derive(Debug, Clone)]
pub struct SectionView<'a> {
    pub section: Section,
    /// Every conversation of the section the filter keeps, in order.
    pub items: Vec<&'a Conversation>,
    /// What shows: all of `items`, or only those that want attention when collapsed.
    pub shown: Vec<&'a Conversation>,
    pub collapsed: bool,
    /// The section keeps your own order (offer Sort A to Z).
    pub custom: bool,
}

/// How the list is filtered on this device.
#[derive(Debug, Clone, Copy, Default)]
pub struct Filter<'a> {
    /// The open conversation, which always shows.
    pub active: Option<&'a str>,
    /// Show only conversations with something unread.
    pub unread_only: bool,
}

/// A network's sections as the sidebar draws them. Favourites shows only
/// when it has something; Channels and Direct messages always show, since
/// their headings offer joining a channel and messaging someone.
pub fn sections<'a>(
    w: &'a Workspace,
    network_id: &str,
    collapsed: &[String],
    filter: Filter,
) -> Vec<SectionView<'a>> {
    let is_active = |c: &Conversation| filter.active == Some(c.id.as_str());
    Section::ALL
        .iter()
        .filter_map(|&section| {
            let items: Vec<&Conversation> = items(w, network_id, section)
                .into_iter()
                .filter(|c| !filter.unread_only || has_unread(c) || is_active(c))
                .collect();
            if items.is_empty() && section == Section::Favorites {
                return None;
            }
            let key = section_key(network_id, section);
            let collapsed = collapsed.contains(&key);
            let shown = if collapsed {
                items
                    .iter()
                    .copied()
                    .filter(|c| {
                        is_active(c) || c.mention_count > 0 || (c.unread_count > 0 && !muted(w, c))
                    })
                    .collect()
            } else {
                items.clone()
            };
            Some(SectionView {
                section,
                items,
                shown,
                collapsed,
                custom: custom(w, network_id, section),
            })
        })
        .collect()
}

/// The conversations Ctrl+1 to 9 open: favourite channels, then channels,
/// each in sidebar order.
pub fn numbered(w: &Workspace, network_id: &str) -> Vec<String> {
    items(w, network_id, Section::Favorites)
        .into_iter()
        .filter(|c| channel_wide(c))
        .chain(items(w, network_id, Section::Channels))
        .take(NUMBERED)
        .map(|c| c.id.clone())
        .collect()
}

/// The conversation after `active` in `direction` within its network, in
/// sidebar order, wrapping around.
pub fn step_conversation(
    w: &Workspace,
    network_id: &str,
    active: Option<&str>,
    direction: i32,
) -> Option<String> {
    let list = ordered(w, network_id);
    if list.is_empty() {
        return None;
    }
    let len = list.len() as i64;
    let index = active
        .and_then(|a| list.iter().position(|c| c.id == a))
        .map(|i| i as i64);
    let next = match index {
        Some(i) => (i + i64::from(direction)).rem_euclid(len),
        None if direction > 0 => 0,
        None => len - 1,
    };
    Some(list[next as usize].id.clone())
}

/// The next conversation needing attention from `active` in `direction`,
/// across every network in rail and sidebar order: those with mentions when
/// there are any, otherwise any unread one. `None` when nothing is unread.
pub fn next_unread(w: &Workspace, active: Option<&str>, direction: i32) -> Option<String> {
    let all: Vec<&Conversation> = w.networks.iter().flat_map(|n| ordered(w, &n.id)).collect();
    let waiting: Vec<&Conversation> = all
        .into_iter()
        .filter(|c| c.mention_count > 0 || c.unread_count > 0)
        .collect();
    if waiting.is_empty() {
        return None;
    }
    let pool: Vec<&Conversation> = if waiting.iter().any(|c| c.mention_count > 0) {
        waiting
            .into_iter()
            .filter(|c| c.mention_count > 0)
            .collect()
    } else {
        waiting
    };
    let len = pool.len() as i64;
    let next = match active.and_then(|a| pool.iter().position(|c| c.id == a)) {
        Some(i) => (i as i64 + i64::from(direction)).rem_euclid(len),
        None if direction > 0 => 0,
        None => len - 1,
    };
    Some(pool[next as usize].id.clone())
}

/// The network after `active` in `direction`, in rail order, wrapping
/// around; `None` without another one.
pub fn next_network<'a>(
    networks: &'a [Network],
    active: Option<&str>,
    direction: i32,
) -> Option<&'a Network> {
    if networks.is_empty() {
        return None;
    }
    let len = networks.len() as i64;
    let next = match active.and_then(|a| networks.iter().position(|n| n.id == a)) {
        Some(i) => (i as i64 + i64::from(direction)).rem_euclid(len),
        None if direction > 0 => 0,
        None => len - 1,
    };
    let next = &networks[next as usize];
    (Some(next.id.as_str()) != active).then_some(next)
}

/// A generated network icon: filled cells of a 5×5 grid and a hue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Glyph {
    /// 0 to 359.
    pub hue: u32,
    /// Filled cells as (column, row), 0 to 4.
    pub cells: Vec<(u8, u8)>,
}

/// The same pattern and hue the web client draws for `name`.
pub fn network_glyph(name: &str) -> Glyph {
    let mut hash: u32 = 2_166_136_261;
    let mut units = [0u16; 2];
    for ch in name.trim().to_lowercase().chars() {
        let unit = ch.encode_utf16(&mut units)[0];
        hash = (hash ^ u32::from(unit)).wrapping_mul(16_777_619);
    }
    hash = (hash ^ (hash >> 16)).wrapping_mul(0x85eb_ca6b);
    hash = (hash ^ (hash >> 13)).wrapping_mul(0xc2b2_ae35);
    hash ^= hash >> 16;
    let mut cells = Vec::new();
    for column in 0..3u8 {
        for row in 0..5u8 {
            if hash & (1 << (column * 5 + row)) != 0 {
                cells.push((column, row));
                if column < 2 {
                    cells.push((4 - column, row));
                }
            }
        }
    }
    if cells.is_empty() {
        cells.extend((0..5).map(|row| (2, row)));
    }
    Glyph {
        hue: (hash >> 15) % 360,
        cells,
    }
}

/// A network's own icon from the preferences (`networkIcons`): the picture
/// bytes and their type, from a `data:image/...;base64,` URL.
pub fn network_icon(w: &Workspace, network_id: &str) -> Option<(String, Vec<u8>)> {
    let url = w
        .documents
        .preferences
        .value()?
        .preferences
        .as_ref()?
        .network_icons
        .as_ref()?
        .get(network_id)?;
    decode_data_url(url)
}

/// The media type and bytes of a base64 `data:` URL.
pub fn decode_data_url(url: &str) -> Option<(String, Vec<u8>)> {
    let rest = url.strip_prefix("data:")?;
    let (meta, data) = rest.split_once(',')?;
    let mime = meta.strip_suffix(";base64")?;
    Some((mime.to_string(), decode_base64(data)?))
}

fn decode_base64(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let mut buffer = 0u32;
    let mut bits = 0;
    for byte in text.bytes() {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' => break,
            b' ' | b'\n' | b'\r' | b'\t' => continue,
            _ => return None,
        };
        buffer = (buffer << 6) | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
            buffer &= (1 << bits) - 1;
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conv(id: &str, name: &str, kind: ConversationKind, favorite: bool) -> Conversation {
        Conversation {
            id: id.into(),
            name: name.into(),
            network_id: "n1".into(),
            kind,
            favorite,
            joined: true,
            ..Default::default()
        }
    }

    fn workspace() -> Workspace {
        use ConversationKind::*;
        let mut w = Workspace {
            networks: vec![
                Network {
                    id: "n1".into(),
                    name: "Home".into(),
                    ..Default::default()
                },
                Network {
                    id: "n2".into(),
                    name: "Away".into(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        w.conversations = vec![
            conv("c10", "chan10", Channel, false),
            conv("c2", "Chan2", Channel, false),
            conv("ca", "alpha", Channel, false),
            conv("fz", "zeta", Channel, true),
            conv("fd", "bob", Direct, true),
            conv("d1", "carol", Direct, false),
        ];
        w
    }

    fn names(list: &[&Conversation]) -> Vec<String> {
        list.iter().map(|c| c.name.clone()).collect()
    }

    fn prefs_with(w: &mut Workspace, key: &str, ids: &[&str]) {
        let prefs = with_order(
            &StoredPreferences::default(),
            key,
            Some(ids.iter().map(|s| s.to_string()).collect()),
        );
        w.documents.preferences.begin_read();
        w.documents.preferences.read_done(Ok(prefs));
    }

    #[test]
    fn names_sort_naturally_and_ignore_case() {
        let w = workspace();
        let list = items(&w, "n1", Section::Channels);
        assert_eq!(names(&list), ["alpha", "Chan2", "chan10"]);
        assert_eq!(compare_names("a1b", "a01b"), Ordering::Greater);
        assert_eq!(compare_names("Chan2", "chan10"), Ordering::Less);
    }

    #[test]
    fn sections_split_favourites_channels_and_directs() {
        let w = workspace();
        let views = sections(&w, "n1", &[], Filter::default());
        let got: Vec<(Section, Vec<String>)> =
            views.iter().map(|v| (v.section, names(&v.shown))).collect();
        assert_eq!(
            got,
            [
                (Section::Favorites, vec!["bob".to_string(), "zeta".into()]),
                (
                    Section::Channels,
                    vec!["alpha".into(), "Chan2".into(), "chan10".into()]
                ),
                (Section::Directs, vec!["carol".into()]),
            ]
        );
        assert!(views.iter().all(|v| !v.custom));
    }

    #[test]
    fn a_stored_order_wins_and_new_ones_go_last() {
        let mut w = workspace();
        prefs_with(&mut w, "n1/channels", &["c10", "gone", "ca"]);
        let list = items(&w, "n1", Section::Channels);
        assert_eq!(names(&list), ["chan10", "alpha", "Chan2"]);
        assert!(custom(&w, "n1", Section::Channels));
        assert!(!custom(&w, "n1", Section::Favorites));
    }

    #[test]
    fn moving_and_stepping() {
        let ids: Vec<String> = ["a", "b", "c", "d"].iter().map(|s| s.to_string()).collect();
        assert_eq!(move_id(&ids, "d", "b", Place::Before), ["a", "d", "b", "c"]);
        assert_eq!(move_id(&ids, "a", "c", Place::After), ["b", "c", "a", "d"]);
        assert_eq!(move_id(&ids, "a", "zz", Place::After), ids);
        assert_eq!(step_id(&ids, "c", -1), ["a", "c", "b", "d"]);
        assert_eq!(step_id(&ids, "d", 1), ids, "unchanged at the end");
        assert_eq!(step_id(&ids, "a", -1), ids, "unchanged at the start");

        let w = workspace();
        let ca = w.conversation("ca").unwrap();
        assert_eq!(steps(&w, ca), (false, true));
        let next = stepped(&w, &StoredPreferences::default(), ca, 1).unwrap();
        let order = next.preferences.unwrap().sidebar_order.unwrap();
        assert_eq!(
            order["n1/channels"].as_deref().unwrap(),
            ["c2", "ca", "c10"]
        );
        assert!(stepped(&w, &StoredPreferences::default(), ca, -1).is_none());
        let d1 = w.conversation("d1").unwrap();
        assert_eq!(
            steps(&w, d1),
            (false, false),
            "direct messages keep their own order"
        );
    }

    #[test]
    fn sort_a_to_z_forgets_the_order_and_keeps_other_preferences() {
        let current = StoredPreferences {
            preferences: Some(ClientPreferences {
                show_nsfw: Some(true),
                ..Default::default()
            }),
            ..Default::default()
        };
        let ordered = with_order(&current, "n1/channels", Some(vec!["a".into()]));
        let back = with_order(&ordered, "n1/channels", None);
        let prefs = back.preferences.unwrap();
        assert_eq!(prefs.show_nsfw, Some(true));
        assert!(prefs.sidebar_order.unwrap().is_empty());
    }

    #[test]
    fn a_collapsed_section_keeps_the_open_one_and_what_wants_attention() {
        let mut w = workspace();
        for c in &mut w.conversations {
            match c.id.as_str() {
                "c2" => c.unread_count = 4,
                "c10" => c.mention_count = 1,
                _ => {}
            }
        }
        let collapsed = toggle_collapsed(&[], "n1/channels");
        let views = sections(
            &w,
            "n1",
            &collapsed,
            Filter {
                active: Some("ca"),
                unread_only: false,
            },
        );
        let channels = views
            .iter()
            .find(|v| v.section == Section::Channels)
            .unwrap();
        assert!(channels.collapsed);
        assert_eq!(names(&channels.shown), ["alpha", "Chan2", "chan10"]);
        let views = sections(&w, "n1", &collapsed, Filter::default());
        let channels = views
            .iter()
            .find(|v| v.section == Section::Channels)
            .unwrap();
        assert_eq!(names(&channels.shown), ["Chan2", "chan10"]);
        assert!(toggle_collapsed(&collapsed, "n1/channels").is_empty());
    }

    #[test]
    fn the_unread_filter_keeps_the_open_conversation() {
        let mut w = workspace();
        w.conversations[0].unread_count = 2;
        let views = sections(
            &w,
            "n1",
            &[],
            Filter {
                active: Some("d1"),
                unread_only: true,
            },
        );
        let shown: Vec<_> = views.iter().map(|v| names(&v.shown)).collect();
        assert_eq!(shown, [vec!["chan10".to_string()], vec!["carol".into()]]);
    }

    #[test]
    fn numbers_go_to_favourite_channels_then_channels() {
        let w = workspace();
        assert_eq!(numbered(&w, "n1"), ["fz", "ca", "c2", "c10"]);
    }

    #[test]
    fn stepping_wraps_within_the_network() {
        let w = workspace();
        let order: Vec<String> = ordered(&w, "n1").iter().map(|c| c.id.clone()).collect();
        assert_eq!(order, ["fd", "fz", "ca", "c2", "c10", "d1"]);
        assert_eq!(
            step_conversation(&w, "n1", Some("d1"), 1).as_deref(),
            Some("fd")
        );
        assert_eq!(
            step_conversation(&w, "n1", Some("fd"), -1).as_deref(),
            Some("d1")
        );
        assert_eq!(step_conversation(&w, "n1", None, -1).as_deref(), Some("d1"));
        assert_eq!(step_conversation(&w, "n2", None, 1), None);
    }

    #[test]
    fn next_unread_prefers_mentions() {
        let mut w = workspace();
        assert_eq!(next_unread(&w, Some("ca"), 1), None);
        for c in &mut w.conversations {
            match c.id.as_str() {
                "c2" | "d1" => c.unread_count = 1,
                _ => {}
            }
        }
        assert_eq!(next_unread(&w, Some("c2"), 1).as_deref(), Some("d1"));
        assert_eq!(next_unread(&w, Some("d1"), 1).as_deref(), Some("c2"));
        assert_eq!(next_unread(&w, None, -1).as_deref(), Some("d1"));
        w.conversations[0].mention_count = 1;
        assert_eq!(next_unread(&w, Some("c2"), 1).as_deref(), Some("c10"));
    }

    #[test]
    fn next_network_wraps_in_rail_order() {
        let w = workspace();
        let id = |n: Option<&Network>| n.map(|n| n.id.clone());
        assert_eq!(
            id(next_network(&w.networks, Some("n1"), 1)),
            Some("n2".into())
        );
        assert_eq!(
            id(next_network(&w.networks, Some("n1"), -1)),
            Some("n2".into())
        );
        assert_eq!(id(next_network(&w.networks, None, -1)), Some("n2".into()));
        assert_eq!(id(next_network(&w.networks[..1], Some("n1"), 1)), None);
    }

    #[test]
    fn mute_comes_from_your_settings_or_the_channel() {
        let mut w = workspace();
        let d1 = w.conversation("d1").unwrap().clone();
        assert_eq!(level(&w, &d1), Level::All);
        let mut settings = ircord_contract::Settings::default();
        settings
            .levels
            .insert("d1".into(), SettingsLevelsValue::None);
        w.documents.notification_settings.begin_read();
        w.documents.notification_settings.read_done(Ok(settings));
        assert!(muted(&w, &d1));
        let mut ca = w.conversation("ca").unwrap().clone();
        assert_eq!(level(&w, &ca), Level::Mentions, "the channel default");
        ca.channel_settings = Some(ircord_contract::ChannelSettings {
            notifications: ChannelSettingsNotifications::None,
            ..Default::default()
        });
        assert!(muted(&w, &ca));
    }

    #[test]
    fn rail_activity() {
        let mut w = workspace();
        assert_eq!(network_activity(&w, "n1"), Activity::Quiet);
        w.conversations[2].unread_count = 3;
        assert_eq!(network_activity(&w, "n1"), Activity::Unread);
        w.conversations[5].mention_count = 1;
        assert_eq!(network_activity(&w, "n1"), Activity::Mention);
        assert_eq!(network_activity(&w, "n2"), Activity::Quiet);
        assert_eq!(unread_in(&w, Some("n1")).len(), 2);
    }

    #[test]
    fn glyphs_match_the_web_client() {
        // Values printed by the web client's networkGlyph.
        let g = network_glyph("Libera.Chat");
        assert_eq!(g.hue, 148);
        assert_eq!(
            g.cells,
            [
                (0, 2),
                (4, 2),
                (0, 3),
                (4, 3),
                (0, 4),
                (4, 4),
                (1, 4),
                (3, 4),
                (2, 4)
            ]
        );
        assert_eq!(network_glyph(" libera.chat "), g);
        assert_eq!(network_glyph("OFTC").hue, 70);
        assert_eq!(network_glyph("EFnet").hue, 320);
        let g = network_glyph("Ünïcode 🎉");
        assert_eq!(g.hue, 199);
        assert_eq!(g.cells.len(), 16);
        for name in ["", "a", "IRCnet", "Rizon"] {
            let g = network_glyph(name);
            assert!(!g.cells.is_empty() && g.hue < 360);
            for (c, r) in &g.cells {
                assert!(g.cells.contains(&(4 - c, *r)), "mirrored");
            }
        }
    }

    #[test]
    fn layout_is_kept_in_range() {
        let l = Layout {
            sidebar_width: 9000.,
            panel_width: f32::NAN,
            collapsed: vec!["a".into(), "a".into(), String::new()],
            ..Default::default()
        }
        .sanitized();
        assert_eq!(l.sidebar_width, 360.);
        assert_eq!(l.panel_width, 208.);
        assert_eq!(l.collapsed, ["a"]);
        let json = serde_json::to_value(&l).unwrap();
        assert_eq!(json["showList"], true);
        let back: Layout = serde_json::from_value(serde_json::json!({"unreadOnly": true})).unwrap();
        assert!(back.unread_only && back.show_list);
    }

    #[test]
    fn data_urls_decode() {
        let (mime, bytes) = decode_data_url("data:image/png;base64,iVBORw0K").unwrap();
        assert_eq!(mime, "image/png");
        assert_eq!(bytes, [0x89, b'P', b'N', b'G', 0x0d, 0x0a]);
        assert_eq!(decode_data_url("data:text/plain,hi"), None);
        assert_eq!(decode_data_url("https://x"), None);
    }
}

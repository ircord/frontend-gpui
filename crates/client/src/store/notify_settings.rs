//! Notification state beyond the settings document, which lives in
//! `Workspace::documents.notification_settings`, and the rules for editing
//! that document, ported from the web client's `shared/notifications.ts`.
//!
//! Owned by package S3. The account's devices and their own notification
//! choices are kept in [`State`] (the workspace's `notify_settings` field):
//! [`keep`] reads them, reads them again on `devices.updated`, and sets this
//! device up once. The server alerts no device whose choices were never set
//! up, so this is what makes notifications show at all on a new sign-in.
//!
//! Every edit below takes the whole document and returns it bounded the way
//! the backend bounds it; the app saves the result with
//! `Client::edit_document(&documents::NOTIFICATION_SETTINGS, …)`, and only
//! once the document was read (a save is a full replacement).

use std::collections::{BTreeMap, BTreeSet};

use ircord_contract::event::ServerEvent;
use ircord_contract::{
    Device, DeviceNotificationsContent as Content, DeviceNotificationsDelivery as Delivery,
    DeviceNotificationsSound as Sound, DevicePreferences, DevicePreferencesContent,
    DevicePreferencesDelivery, DevicePreferencesSound, QuietHours, Settings,
    SettingsChannelDefault, SettingsDirectDefault, SettingsLevelsValue, Snapshot,
};

use super::{Changes, Workspace};
use crate::api::notifications::DeviceSaveError;
use crate::client::{Client, Handle};

/// The topic this sub-state reports in [`Changes`].
pub const TOPIC: &str = "notify_settings";

/// The backend's bounds on the settings document.
pub mod limits {
    pub const HIGHLIGHT_WORDS: usize = 50;
    pub const HIGHLIGHT_WORD_LEN: usize = 64;
    pub const IGNORE: usize = 200;
    pub const IGNORE_ENTRY_LEN: usize = 120;
    pub const LEVELS: usize = 500;
    pub const LAST_DEVICE_DELAY: i64 = 3600;
    pub const ALL_DEVICES_DELAY: i64 = 86400;
    pub const AUTO_AWAY_DELAY: i64 = 86400;
    /// The shortest away delay the switch accepts once on.
    pub const AUTO_AWAY_MIN: i64 = 60;
}

/// What a conversation notifies you about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Level {
    All,
    Mentions,
    None,
}

impl Level {
    pub const EVERY: [Level; 3] = [Level::All, Level::Mentions, Level::None];

    pub fn as_str(self) -> &'static str {
        match self {
            Level::All => "all",
            Level::Mentions => "mentions",
            Level::None => "none",
        }
    }

    pub fn of_channel(v: SettingsChannelDefault) -> Self {
        match v {
            SettingsChannelDefault::All => Level::All,
            SettingsChannelDefault::None => Level::None,
            _ => Level::Mentions,
        }
    }

    pub fn of_direct(v: SettingsDirectDefault) -> Self {
        match v {
            SettingsDirectDefault::Mentions => Level::Mentions,
            SettingsDirectDefault::None => Level::None,
            _ => Level::All,
        }
    }

    pub fn of_value(v: SettingsLevelsValue) -> Option<Self> {
        match v {
            SettingsLevelsValue::All => Some(Level::All),
            SettingsLevelsValue::Mentions => Some(Level::Mentions),
            SettingsLevelsValue::None => Some(Level::None),
            _ => None,
        }
    }

    pub fn channel(self) -> SettingsChannelDefault {
        match self {
            Level::All => SettingsChannelDefault::All,
            Level::Mentions => SettingsChannelDefault::Mentions,
            Level::None => SettingsChannelDefault::None,
        }
    }

    pub fn direct(self) -> SettingsDirectDefault {
        match self {
            Level::All => SettingsDirectDefault::All,
            Level::Mentions => SettingsDirectDefault::Mentions,
            Level::None => SettingsDirectDefault::None,
        }
    }

    pub fn value(self) -> SettingsLevelsValue {
        match self {
            Level::All => SettingsLevelsValue::All,
            Level::Mentions => SettingsLevelsValue::Mentions,
            Level::None => SettingsLevelsValue::None,
        }
    }
}

/// The settings a new account has.
pub fn defaults() -> Settings {
    Settings {
        channel_default: SettingsChannelDefault::Mentions,
        direct_default: SettingsDirectDefault::All,
        levels: BTreeMap::new(),
        highlight_words: Some(Vec::new()),
        ignore: Some(Vec::new()),
        quiet_hours: QuietHours {
            enabled: false,
            start: "22:00".into(),
            end: "07:00".into(),
            time_zone: "UTC".into(),
        },
        last_device_delay: 30,
        all_devices_delay: 300,
        auto_away_delay: 30,
    }
}

fn words(list: Option<&Vec<String>>, limit: usize, max: usize) -> Vec<String> {
    let mut seen = BTreeSet::new();
    list.into_iter()
        .flatten()
        .map(|w| w.trim().to_string())
        .filter(|w| {
            !w.is_empty()
                && w.chars().count() <= max
                && !w.contains(['\r', '\n', '\0'])
                && seen.insert(w.clone())
        })
        .take(limit)
        .collect()
}

/// The document bounded the way the backend bounds it: unknown levels fall
/// back to the defaults, lists are trimmed and deduplicated, times are
/// `HH:MM`, and every device is never alerted before the last-used one.
pub fn sanitize(s: &Settings) -> Settings {
    let d = defaults();
    let seconds = |v: i64, max: i64| v.clamp(0, max);
    let last = seconds(s.last_device_delay, limits::LAST_DEVICE_DELAY);
    let quiet = &s.quiet_hours;
    let time = |t: &str, fallback: &str| {
        if valid_time(t) {
            t.to_string()
        } else {
            fallback.to_string()
        }
    };
    Settings {
        channel_default: Level::of_channel(s.channel_default).channel(),
        direct_default: Level::of_direct(s.direct_default).direct(),
        levels: s
            .levels
            .iter()
            .filter(|(id, v)| !id.is_empty() && id.len() <= 120 && Level::of_value(**v).is_some())
            .take(limits::LEVELS)
            .map(|(id, v)| (id.clone(), *v))
            .collect(),
        highlight_words: Some(words(
            s.highlight_words.as_ref(),
            limits::HIGHLIGHT_WORDS,
            limits::HIGHLIGHT_WORD_LEN,
        )),
        ignore: Some(words(
            s.ignore.as_ref(),
            limits::IGNORE,
            limits::IGNORE_ENTRY_LEN,
        )),
        quiet_hours: QuietHours {
            enabled: quiet.enabled,
            start: time(&quiet.start, &d.quiet_hours.start),
            end: time(&quiet.end, &d.quiet_hours.end),
            time_zone: if quiet.time_zone.trim().is_empty() {
                d.quiet_hours.time_zone
            } else {
                quiet.time_zone.clone()
            },
        },
        last_device_delay: last,
        all_devices_delay: last.max(seconds(s.all_devices_delay, limits::ALL_DEVICES_DELAY)),
        auto_away_delay: seconds(s.auto_away_delay, limits::AUTO_AWAY_DELAY),
    }
}

/// `HH:MM` on a 24-hour clock.
pub fn valid_time(t: &str) -> bool {
    let b = t.as_bytes();
    b.len() == 5
        && b[2] == b':'
        && b[..2].iter().chain(&b[3..]).all(u8::is_ascii_digit)
        && (t[..2].parse::<u32>().unwrap_or(99) < 24)
        && (t[3..].parse::<u32>().unwrap_or(99) < 60)
}

/// A time as typed (`7:30`, `0730`, `7`, `19.05`) as `HH:MM`.
pub fn parse_time(text: &str) -> Option<String> {
    let t = text.trim();
    let (h, m) = match t.split_once([':', '.']) {
        Some((h, m)) => (h, m),
        None if t.len() == 4 => t.split_at(2),
        None if t.len() == 3 => t.split_at(1),
        None => (t, "0"),
    };
    let (h, m): (u32, u32) = (h.trim().parse().ok()?, m.trim().parse().ok()?);
    (h < 24 && m < 60).then(|| format!("{h:02}:{m:02}"))
}

/// This device's time zone name (`Europe/Berlin`), or `UTC`.
pub fn local_time_zone() -> String {
    iana_time_zone::get_timezone()
        .ok()
        .filter(|z| !z.trim().is_empty())
        .unwrap_or_else(|| "UTC".into())
}

/// Quiet hours changed here, in this device's time zone `zone`, as the web
/// client stores them whenever they are edited.
pub fn set_quiet(
    s: &Settings,
    enabled: Option<bool>,
    start: Option<&str>,
    end: Option<&str>,
    zone: &str,
) -> Settings {
    let mut next = s.clone();
    let q = &mut next.quiet_hours;
    if let Some(on) = enabled {
        q.enabled = on;
    }
    if let Some(t) = start {
        q.start = t.into();
    }
    if let Some(t) = end {
        q.end = t.into();
    }
    q.time_zone = zone.into();
    sanitize(&next)
}

/// The last-used device's delay; every device is never alerted before it,
/// so that delay rises along.
pub fn set_last_device(s: &Settings, seconds: i64) -> Settings {
    let mut next = s.clone();
    next.last_device_delay = seconds;
    next.all_devices_delay = s.all_devices_delay.max(seconds);
    sanitize(&next)
}

/// Every device's delay; the last-used device's falls to it if above.
pub fn set_all_devices(s: &Settings, seconds: i64) -> Settings {
    let mut next = s.clone();
    next.all_devices_delay = seconds;
    next.last_device_delay = s.last_device_delay.min(seconds);
    sanitize(&next)
}

/// Both device delays back to the defaults.
pub fn reset_routing(s: &Settings) -> Settings {
    let d = defaults();
    let mut next = s.clone();
    next.last_device_delay = d.last_device_delay;
    next.all_devices_delay = d.all_devices_delay;
    sanitize(&next)
}

pub fn routing_is_default(s: &Settings) -> bool {
    let d = defaults();
    s.last_device_delay == d.last_device_delay && s.all_devices_delay == d.all_devices_delay
}

/// How long only the last-used device is alerted before every device is.
pub fn last_only(s: &Settings) -> i64 {
    (s.all_devices_delay - s.last_device_delay).max(0)
}

/// Auto-away on (with the default delay) or off.
pub fn set_auto_away(s: &Settings, on: bool) -> Settings {
    let mut next = s.clone();
    next.auto_away_delay = if on { defaults().auto_away_delay } else { 0 };
    sanitize(&next)
}

/// The away delay while auto-away is on: at least a minute.
pub fn set_auto_away_delay(s: &Settings, seconds: i64) -> Settings {
    let mut next = s.clone();
    next.auto_away_delay = seconds.max(limits::AUTO_AWAY_MIN);
    sanitize(&next)
}

pub fn set_channel_default(s: &Settings, level: Level) -> Settings {
    let mut next = s.clone();
    next.channel_default = level.channel();
    sanitize(&next)
}

pub fn set_direct_default(s: &Settings, level: Level) -> Settings {
    let mut next = s.clone();
    next.direct_default = level.direct();
    sanitize(&next)
}

/// Your own level for one conversation, or back to the default (`None`).
pub fn set_level(s: &Settings, conversation: &str, level: Option<Level>) -> Settings {
    let mut next = s.clone();
    match level {
        Some(level) => next.levels.insert(conversation.into(), level.value()),
        None => next.levels.remove(conversation),
    };
    sanitize(&next)
}

/// Your own levels, by conversation ID.
pub fn own_levels(s: &Settings) -> Vec<(String, Level)> {
    s.levels
        .iter()
        .filter_map(|(id, v)| Some((id.clone(), Level::of_value(*v)?)))
        .collect()
}

pub fn highlight_words(s: &Settings) -> &[String] {
    s.highlight_words.as_deref().unwrap_or_default()
}

/// Why a highlight word was not added.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WordProblem {
    Empty,
    TooLong,
    Duplicate,
    Full,
}

/// Add a highlight word, refusing what the backend would drop.
pub fn add_highlight(s: &Settings, word: &str) -> Result<Settings, WordProblem> {
    let word = word.trim();
    let list = highlight_words(s);
    if word.is_empty() || word.contains(['\r', '\n', '\0']) {
        return Err(WordProblem::Empty);
    }
    if word.chars().count() > limits::HIGHLIGHT_WORD_LEN {
        return Err(WordProblem::TooLong);
    }
    if list.iter().any(|w| w.eq_ignore_ascii_case(word)) {
        return Err(WordProblem::Duplicate);
    }
    if list.len() >= limits::HIGHLIGHT_WORDS {
        return Err(WordProblem::Full);
    }
    let mut next = s.clone();
    let mut words = list.to_vec();
    words.push(word.into());
    next.highlight_words = Some(words);
    Ok(sanitize(&next))
}

pub fn remove_highlight(s: &Settings, word: &str) -> Settings {
    let mut next = s.clone();
    next.highlight_words = Some(
        highlight_words(s)
            .iter()
            .filter(|w| *w != word)
            .cloned()
            .collect(),
    );
    sanitize(&next)
}

/// A unit of a spelled-out duration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unit {
    Hour,
    Minute,
    Second,
}

/// `270` → `[(4, Minute), (30, Second)]`; zero is `[(0, Second)]`.
pub fn duration_parts(total: i64) -> Vec<(i64, Unit)> {
    let total = total.max(0);
    let (h, m, s) = (total / 3600, total % 3600 / 60, total % 60);
    let mut parts = Vec::new();
    if h > 0 {
        parts.push((h, Unit::Hour));
    }
    if m > 0 {
        parts.push((m, Unit::Minute));
    }
    if s > 0 || parts.is_empty() {
        parts.push((s, Unit::Second));
    }
    parts
}

/// `1h 30m`, `4m30s`, `90`, `5 min`, `2 hours`, `1:30` (minutes and
/// seconds) or `1:00:00` as seconds.
pub fn parse_duration(text: &str) -> Option<i64> {
    let t = text.trim().to_lowercase();
    if t.is_empty() {
        return None;
    }
    if t.contains(':') {
        let parts: Vec<i64> = t
            .split(':')
            .map(|p| p.trim().parse().ok())
            .collect::<Option<_>>()?;
        return match parts[..] {
            [m, s] if s < 60 => Some(m * 60 + s),
            [h, m, s] if m < 60 && s < 60 => Some(h * 3600 + m * 60 + s),
            _ => None,
        };
    }
    if let Ok(seconds) = t.parse::<i64>() {
        return (seconds >= 0).then_some(seconds);
    }
    let mut total = 0i64;
    let mut rest = t.as_str();
    let mut any = false;
    while !rest.trim_start().is_empty() {
        rest = rest.trim_start();
        let digits = rest
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(rest.len());
        let value: i64 = rest[..digits].parse().ok()?;
        rest = rest[digits..].trim_start();
        let unit_len = rest
            .find(|c: char| !c.is_alphabetic())
            .unwrap_or(rest.len());
        let unit = &rest[..unit_len];
        rest = &rest[unit_len..];
        let scale = match unit {
            "h" | "hr" | "hrs" | "hour" | "hours" => 3600,
            "m" | "min" | "mins" | "minute" | "minutes" => 60,
            "s" | "sec" | "secs" | "second" | "seconds" | "" => 1,
            _ => return None,
        };
        total = total.checked_add(value.checked_mul(scale)?)?;
        any = true;
        rest = rest.trim_start_matches([',', ' ']);
        if let Some(r) = rest.strip_prefix("and ") {
            rest = r;
        }
    }
    any.then_some(total)
}

// ---- Devices ---------------------------------------------------------

/// The account's devices and their notification choices.
#[derive(Debug, Clone, Default)]
pub struct State {
    kept: bool,
    /// This device first; `None` until read.
    pub devices: Option<Vec<Device>>,
    pub loading: bool,
    stale: bool,
    /// Why the list could not be read, when there is none to show.
    pub error: Option<String>,
    /// Devices whose choices are being saved.
    pub saving: BTreeSet<String>,
    /// The last failed save of each device.
    pub errors: BTreeMap<String, DeviceSaveError>,
    /// This device was set up (or tried) on this client.
    set_up_tried: bool,
}

impl State {
    /// The device this client runs on, once read.
    pub fn this_device(&self) -> Option<&Device> {
        self.devices.as_ref()?.iter().find(|d| d.current)
    }

    pub fn device(&self, id: &str) -> Option<&Device> {
        self.devices.as_ref()?.iter().find(|d| d.id == id)
    }
}

/// One change to a device's choices; the rest stay as stored.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DeviceChange {
    pub content: Option<Content>,
    pub delivery: Option<Delivery>,
    pub sound: Option<Sound>,
    pub preview: Option<bool>,
}

/// What to send for `device` with `change` applied.
pub fn preferences(device: &Device, change: &DeviceChange) -> DevicePreferences {
    let n = &device.notifications;
    let content = match change.content.unwrap_or(n.content) {
        Content::All => DevicePreferencesContent::All,
        Content::Mentions => DevicePreferencesContent::Mentions,
        Content::None => DevicePreferencesContent::None,
        _ => DevicePreferencesContent::Inherit,
    };
    let delivery = match change.delivery.unwrap_or(n.delivery) {
        Delivery::Open => DevicePreferencesDelivery::Open,
        Delivery::Push => DevicePreferencesDelivery::Push,
        _ => DevicePreferencesDelivery::Off,
    };
    let sound = match change.sound.unwrap_or(n.sound) {
        Sound::Off => DevicePreferencesSound::Off,
        Sound::All => DevicePreferencesSound::All,
        _ => DevicePreferencesSound::Mentions,
    };
    DevicePreferences {
        content,
        delivery,
        sound,
        preview: change.preview.unwrap_or(n.preview),
        revision: n.revision,
        name: device.name.clone(),
        migrate: None,
    }
}

/// The first choices of a device that was never set up: notifications
/// while IRCord runs (a desktop app needs no permission prompt), sounds for
/// mentions, previews on. A migration never overwrites choices made since.
pub fn first_choices(device: &Device) -> DevicePreferences {
    DevicePreferences {
        content: DevicePreferencesContent::Inherit,
        delivery: DevicePreferencesDelivery::Open,
        sound: DevicePreferencesSound::Mentions,
        preview: true,
        revision: device.notifications.revision,
        name: device.name.clone(),
        migrate: Some(true),
    }
}

/// Read the devices now and whenever another device changes them, and set
/// this device up once. Call once per client; later calls do nothing.
pub fn keep(client: &Client) {
    let mut first = false;
    client.change(|w| {
        first = !w.notify_settings.kept;
        w.notify_settings.kept = true;
        Changes::default()
    });
    if !first {
        return;
    }
    client.watch(|handle, changes| {
        if changes.touches(TOPIC) {
            let wanted = {
                let s = &handle.read().notify_settings;
                s.stale && !s.loading
            };
            if wanted {
                load(handle);
            }
        }
    });
    load(client.handle());
}

/// Read the device list.
pub fn load(handle: &Handle) {
    let mut started = false;
    handle.change(|w| {
        let s = &mut w.notify_settings;
        if s.loading {
            return Changes::default();
        }
        started = true;
        s.loading = true;
        s.stale = false;
        Changes::topic(TOPIC)
    });
    if !started {
        return;
    }
    let setup = handle.clone();
    handle.call(
        |api| async move { api.devices().await },
        move |w, result| {
            let s = &mut w.notify_settings;
            s.loading = false;
            match result {
                Ok(devices) => {
                    s.devices = Some(devices);
                    s.error = None;
                }
                Err(e) if s.devices.is_none() => s.error = Some(e.to_string()),
                Err(e) => log::warn!("devices: {e}"),
            }
            let first = (!s.set_up_tried)
                .then(|| s.this_device().filter(|d| !d.notifications.initialized))
                .flatten()
                .map(|d| (d.id.clone(), first_choices(d)));
            if let Some((id, body)) = first {
                s.set_up_tried = true;
                setup.call(
                    move |api| async move { api.save_device_notifications(&id, &body).await },
                    |w, result| {
                        if let Err(e) = result {
                            log::warn!("setting this device up for notifications: {e}");
                        }
                        w.notify_settings.stale = true;
                        Changes::topic(TOPIC)
                    },
                );
            }
            Changes::topic(TOPIC)
        },
    );
}

/// Save one change to a device's choices. A conflict (another device
/// changed them) reads the list again and says so in [`State::errors`].
pub fn save_device(handle: &Handle, id: &str, change: DeviceChange) {
    let mut body = None;
    handle.change(|w| {
        let s = &mut w.notify_settings;
        let Some(device) = s.device(id) else {
            return Changes::default();
        };
        body = Some(preferences(device, &change));
        s.saving.insert(id.into());
        s.errors.remove(id);
        Changes::topic_id(TOPIC, id)
    });
    let Some(body) = body else {
        return;
    };
    let id = id.to_string();
    handle.call(
        {
            let id = id.clone();
            move |api| async move { api.save_device_notifications(&id, &body).await }
        },
        move |w, result| {
            let s = &mut w.notify_settings;
            s.saving.remove(&id);
            match result {
                Ok(saved) => {
                    if let Some(d) = s.devices.iter_mut().flatten().find(|d| d.id == saved.id) {
                        // The answer may leave out what only the list carries.
                        let current = d.current;
                        *d = saved;
                        d.current = current;
                    }
                }
                Err(e) => {
                    let error = DeviceSaveError::of(&e);
                    if error == DeviceSaveError::Changed {
                        s.stale = true;
                    }
                    s.errors.insert(id.clone(), error);
                }
            }
            Changes::topic(TOPIC)
        },
    );
}

/// Called for every fresh event after the workspace's own handling.
pub(super) fn apply(w: &mut Workspace, event: &ServerEvent, _now_ms: i64) -> Changes {
    match event {
        ServerEvent::DevicesUpdated if w.notify_settings.kept => {
            w.notify_settings.stale = true;
            Changes::topic(TOPIC)
        }
        _ => Changes::default(),
    }
}

/// Called when a bootstrap replaces the workspace, before it is replaced.
/// The device list is the account's, not the bootstrap's: it stays, and is
/// read again in case it changed while disconnected.
pub(super) fn reset(w: &mut Workspace, _snapshot: &Snapshot) -> Changes {
    if !w.notify_settings.kept {
        return Changes::default();
    }
    w.notify_settings.stale = true;
    Changes::topic(TOPIC)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{CannedServer, wait_until};
    use serde_json::json;

    const WAIT: std::time::Duration = std::time::Duration::from_secs(5);

    #[test]
    fn sanitize_bounds_like_the_backend() {
        let mut s = defaults();
        s.last_device_delay = 9000;
        s.all_devices_delay = 10;
        s.auto_away_delay = -5;
        s.highlight_words = Some(vec![
            " rust ".into(),
            "rust".into(),
            "".into(),
            "a\nb".into(),
        ]);
        s.quiet_hours.start = "25:00".into();
        s.quiet_hours.time_zone = "".into();
        s.levels.insert("c-1".into(), SettingsLevelsValue::Unknown);
        s.levels.insert("c-2".into(), SettingsLevelsValue::None);
        let s = sanitize(&s);
        assert_eq!(s.last_device_delay, limits::LAST_DEVICE_DELAY);
        assert_eq!(s.all_devices_delay, limits::LAST_DEVICE_DELAY);
        assert_eq!(s.auto_away_delay, 0);
        assert_eq!(highlight_words(&s), ["rust"]);
        assert_eq!(s.quiet_hours.start, "22:00");
        assert_eq!(s.quiet_hours.time_zone, "UTC");
        assert_eq!(own_levels(&s), [("c-2".to_string(), Level::None)]);
    }

    #[test]
    fn device_delays_move_together() {
        let s = defaults();
        let up = set_last_device(&s, 600);
        assert_eq!((up.last_device_delay, up.all_devices_delay), (600, 600));
        assert_eq!(last_only(&up), 0);
        let down = set_all_devices(&s, 10);
        assert_eq!((down.last_device_delay, down.all_devices_delay), (10, 10));
        assert!(!routing_is_default(&down));
        assert!(routing_is_default(&reset_routing(&down)));
        assert_eq!(last_only(&s), 270);
    }

    #[test]
    fn auto_away_and_quiet_hours() {
        let s = set_auto_away(&defaults(), false);
        assert_eq!(s.auto_away_delay, 0);
        assert_eq!(set_auto_away(&s, true).auto_away_delay, 30);
        assert_eq!(set_auto_away_delay(&s, 5).auto_away_delay, 60);
        let q = set_quiet(&s, Some(true), Some("23:15"), None, "Europe/Berlin");
        assert!(q.quiet_hours.enabled);
        assert_eq!(q.quiet_hours.start, "23:15");
        assert_eq!(q.quiet_hours.end, "07:00");
        assert_eq!(q.quiet_hours.time_zone, "Europe/Berlin");
        assert_eq!(parse_time("7:30").as_deref(), Some("07:30"));
        assert_eq!(parse_time("0730").as_deref(), Some("07:30"));
        assert_eq!(parse_time("19.05").as_deref(), Some("19:05"));
        assert_eq!(parse_time("7").as_deref(), Some("07:00"));
        assert_eq!(parse_time("24:00"), None);
        assert!(!local_time_zone().is_empty());
    }

    #[test]
    fn highlight_words_refuse_what_the_backend_drops() {
        let s = add_highlight(&defaults(), " deploy ").unwrap();
        assert_eq!(highlight_words(&s), ["deploy"]);
        assert_eq!(add_highlight(&s, "Deploy"), Err(WordProblem::Duplicate));
        assert_eq!(add_highlight(&s, "  "), Err(WordProblem::Empty));
        assert_eq!(
            add_highlight(&s, &"x".repeat(65)),
            Err(WordProblem::TooLong)
        );
        let mut full = defaults();
        full.highlight_words = Some((0..50).map(|i| format!("w{i}")).collect());
        assert_eq!(add_highlight(&full, "one more"), Err(WordProblem::Full));
        assert!(highlight_words(&remove_highlight(&s, "deploy")).is_empty());
    }

    #[test]
    fn levels_and_defaults() {
        let s = set_channel_default(&defaults(), Level::All);
        assert_eq!(s.channel_default, SettingsChannelDefault::All);
        let s = set_direct_default(&s, Level::None);
        assert_eq!(s.direct_default, SettingsDirectDefault::None);
        let s = set_level(&s, "c-dm", Some(Level::Mentions));
        assert_eq!(own_levels(&s), [("c-dm".to_string(), Level::Mentions)]);
        assert!(own_levels(&set_level(&s, "c-dm", None)).is_empty());
    }

    #[test]
    fn durations_spell_out_and_parse() {
        assert_eq!(duration_parts(270), [(4, Unit::Minute), (30, Unit::Second)]);
        assert_eq!(duration_parts(3600), [(1, Unit::Hour)]);
        assert_eq!(duration_parts(0), [(0, Unit::Second)]);
        for (text, seconds) in [
            ("90", 90),
            ("1m 30s", 90),
            ("4m30s", 270),
            ("5 min", 300),
            ("2 hours", 7200),
            ("1h, 30m", 5400),
            ("1:30", 90),
            ("1:00:00", 3600),
            ("0", 0),
        ] {
            assert_eq!(parse_duration(text), Some(seconds), "{text}");
        }
        for text in ["", "soon", "1:75", "-5", "5 days"] {
            assert_eq!(parse_duration(text), None, "{text}");
        }
    }

    #[test]
    fn a_change_keeps_the_other_choices() {
        let device: Device = serde_json::from_value(json!({
            "id": "d1", "name": "Desk", "notifications": {"initialized": true, "content": "mentions",
            "delivery": "push", "sound": "all", "preview": false, "revision": 7}
        }))
        .unwrap();
        let body = preferences(
            &device,
            &DeviceChange {
                delivery: Some(Delivery::Off),
                ..Default::default()
            },
        );
        assert_eq!(
            serde_json::to_value(&body).unwrap(),
            json!({"content": "mentions", "delivery": "off", "sound": "all", "preview": false,
                   "revision": 7, "name": "Desk"})
        );
        assert_eq!(
            serde_json::to_value(first_choices(&device)).unwrap()["migrate"],
            json!(true)
        );
    }

    fn devices(initialized: bool, revision: i64) -> serde_json::Value {
        json!({"devices": [
            {"id": "d-phone", "current": false, "name": "", "notifications":
                {"initialized": true, "delivery": "push", "content": "inherit", "sound": "mentions",
                 "preview": true, "revision": 2}},
            {"id": "d-here", "current": true, "name": "", "notifications":
                {"initialized": initialized, "delivery": if initialized { "open" } else { "off" },
                 "content": "inherit", "sound": "mentions", "preview": true, "revision": revision}}
        ]})
    }

    #[test]
    fn keeps_the_devices_sets_this_one_up_once_and_saves_a_change() {
        let rt = crate::client::runtime().unwrap();
        let server = CannedServer::start(&rt);
        server.snapshot(&crate::testing::SnapshotBuilder::new().build());
        let set_up = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let seen = set_up.clone();
        server.route_fn("GET", "/api/v1/auth/devices", move |_| {
            let done = seen.load(std::sync::atomic::Ordering::SeqCst);
            crate::testing::Reply::json(200, devices(done, done as i64))
        });
        server.route_fn(
            "PUT",
            "/api/v1/auth/devices/d-here/notifications",
            move |r| {
                set_up.store(true, std::sync::atomic::Ordering::SeqCst);
                let body = r.json();
                crate::testing::Reply::json(
                    200,
                    json!({"id": "d-here", "current": false, "notifications": {
                    "initialized": true, "delivery": body["delivery"], "content": body["content"],
                    "sound": body["sound"], "preview": body["preview"], "revision": 1}}),
                )
            },
        );
        let (client, _updates) = Client::start(rt.clone(), server.api());
        keep(&client);
        keep(&client);
        let first = server.wait_for("PUT", "/api/v1/auth/devices/d-here/notifications", WAIT);
        assert_eq!(first.json()["migrate"], json!(true));
        assert_eq!(first.json()["delivery"], json!("open"));
        // It reads the list again after setting up; now it is set up.
        wait_until(&client, WAIT, "the set-up device", |w| {
            w.notify_settings
                .this_device()
                .is_some_and(|d| d.notifications.initialized)
        });
        save_device(
            client.handle(),
            "d-here",
            DeviceChange {
                preview: Some(false),
                ..Default::default()
            },
        );
        let saved =
            server.wait_for_nth("PUT", "/api/v1/auth/devices/d-here/notifications", 2, WAIT);
        assert_eq!(saved.json()["preview"], json!(false));
        assert_eq!(saved.json().get("migrate"), None);
        wait_until(&client, WAIT, "the saved choice", |w| {
            let s = &w.notify_settings;
            s.saving.is_empty() && s.this_device().is_some_and(|d| !d.notifications.preview)
        });
        assert_eq!(
            server
                .requests_to("PUT", "/api/v1/auth/devices/d-here/notifications")
                .len(),
            2,
            "set up only once"
        );
    }
}

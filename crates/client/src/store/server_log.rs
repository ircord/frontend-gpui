//! Each network's server window: its log (`server.line`,
//! `server-log.updated`), the connection trace, the connection details
//! (`connection.updated`) and the latency of both hops.
//!
//! Owned by package SV1. The state lives in [`State`] (the workspace's
//! `server_log` field); every change is reported as
//! `Changes::topic_id(TOPIC, network)`, and a new device ping as
//! `Changes::topic(TOPIC)`.
//!
//! The requests live here too, as free functions over a [`Handle`]: each runs
//! with [`Handle::call`] and records its answer (or its failure) in [`State`].
//! Folding follows the web client's `shared/utils/server-log.ts`.

use std::collections::HashMap;
use std::time::Instant;

use ircord_contract::event::{Durable, ServerEvent};
use ircord_contract::*;

use super::{Changes, Workspace, parse_time};
use crate::api::{ApiError, Method, segment};
use crate::client::{Handle, random_id};

/// The topic this sub-state reports in [`Changes`].
pub const TOPIC: &str = "server_log";
/// The newest lines kept per log, as the web client keeps them.
pub const KEEP: usize = 1000;
/// Lines asked for per page (the server allows 1 to 100).
pub const PAGE: u32 = 100;
/// A run of this many lines of one family folds into one row.
pub const MIN_GROUP: usize = 3;
/// The longest raw line the server accepts, in bytes.
pub const RAW_MAX: usize = 510;
/// A latency older than this is not shown, in milliseconds.
pub const STALE_MS: i64 = 75_000;
/// How long device pings are kept, in milliseconds (the chart's window).
pub const DEVICE_WINDOW_MS: i64 = 30 * 60_000;
/// How often the app pings the server, in milliseconds.
pub const PING_EVERY_MS: u64 = 20_000;
/// How far apart a device ping and an IRC ping may be to add up, in milliseconds.
const PAIR_MS: i64 = 20_000;

/// Which of a network's two line buffers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Buffer {
    /// The server window: numerics, notices, errors, MOTD, what you sent.
    Log,
    /// The connection trace: everything on the wire, secrets redacted.
    Trace,
}

impl Buffer {
    fn path(self, network: &str) -> String {
        let tail = match self {
            Buffer::Log => "server-log",
            Buffer::Trace => "connection/trace",
        };
        format!("/api/v1/networks/{}/{tail}", segment(network))
    }
}

/// The lines of one buffer, oldest first.
#[derive(Debug, Clone, Default)]
pub struct Log {
    pub lines: Vec<ServerLine>,
    /// The first page has been read.
    pub loaded: bool,
    pub loading: bool,
    /// The server said the log changed (`server-log.updated`): read it again.
    pub stale: bool,
    /// The cursor for older lines, when there are any.
    pub older: Option<String>,
    pub error: Option<String>,
}

impl Log {
    fn push(&mut self, line: ServerLine) -> bool {
        if !self.loaded || self.lines.iter().any(|l| l.id == line.id) {
            return false;
        }
        self.lines.push(line);
        if self.lines.len() > KEEP {
            let extra = self.lines.len() - KEEP;
            self.lines.drain(..extra);
        }
        true
    }
}

/// The round trip from this device to IRCord.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DeviceSample {
    pub at_ms: i64,
    pub ms: f64,
}

/// A request's answer, or why there is none yet.
#[derive(Debug, Clone)]
pub struct Loaded<T> {
    pub value: Option<T>,
    pub loading: bool,
    pub error: Option<String>,
}

impl<T> Default for Loaded<T> {
    fn default() -> Self {
        Self {
            value: None,
            loading: false,
            error: None,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct State {
    /// Network → its server window's log.
    pub logs: HashMap<String, Log>,
    /// Network → its connection trace.
    pub traces: HashMap<String, Log>,
    /// Network → its connection details.
    pub connections: HashMap<String, Loaded<ConnectionInfo>>,
    /// Network → the IRC hop's latency.
    pub latency: HashMap<String, Loaded<ConnectionLatency>>,
    /// The device hop's pings, oldest first.
    pub device: Vec<DeviceSample>,
    /// The last device ping could not reach the server.
    pub ping_failing: bool,
}

impl State {
    pub fn log(&self, network: &str, buffer: Buffer) -> Option<&Log> {
        self.buffer(buffer).get(network)
    }

    fn buffer(&self, buffer: Buffer) -> &HashMap<String, Log> {
        match buffer {
            Buffer::Log => &self.logs,
            Buffer::Trace => &self.traces,
        }
    }

    fn buffer_mut(&mut self, buffer: Buffer) -> &mut HashMap<String, Log> {
        match buffer {
            Buffer::Log => &mut self.logs,
            Buffer::Trace => &mut self.traces,
        }
    }

    /// The newest device ping, if it is recent enough to show.
    pub fn device_latency(&self, now_ms: i64) -> Option<f64> {
        if self.ping_failing {
            return None;
        }
        self.device
            .last()
            .filter(|s| now_ms - s.at_ms <= STALE_MS)
            .map(|s| s.ms)
    }

    /// The newest IRC-hop ping for `network`, if recent enough to show.
    pub fn irc_latency(&self, network: &str, now_ms: i64) -> Option<f64> {
        let samples = &self.latency.get(network)?.value.as_ref()?.samples;
        let last = samples.last()?;
        (now_ms - parse_time(&last.at) <= STALE_MS).then_some(last.ms)
    }

    /// Both hops added up, one point per IRC ping that has a device ping
    /// within twenty seconds of it.
    pub fn totals(&self, network: &str) -> Vec<DeviceSample> {
        let Some(samples) = self
            .latency
            .get(network)
            .and_then(|l| l.value.as_ref())
            .map(|l| &l.samples)
        else {
            return Vec::new();
        };
        totals(samples, &self.device)
    }
}

/// Each IRC sample plus the nearest device sample within [`PAIR_MS`].
pub fn totals(irc: &[LatencySample], device: &[DeviceSample]) -> Vec<DeviceSample> {
    irc.iter()
        .filter_map(|s| {
            let at = parse_time(&s.at);
            device
                .iter()
                .filter(|d| (d.at_ms - at).abs() <= PAIR_MS)
                .min_by_key(|d| (d.at_ms - at).abs())
                .map(|d| DeviceSample {
                    at_ms: at,
                    ms: s.ms + d.ms,
                })
        })
        .collect()
}

/// The average and the peak of `values`.
pub fn average_and_peak(values: impl IntoIterator<Item = f64>) -> Option<(f64, f64)> {
    let (mut sum, mut peak, mut n) = (0.0, f64::MIN, 0usize);
    for v in values {
        sum += v;
        peak = peak.max(v);
        n += 1;
    }
    (n > 0).then(|| (sum / n as f64, peak))
}

/// Called for every fresh event after the workspace's own handling.
pub(super) fn apply(w: &mut Workspace, event: &ServerEvent, _now_ms: i64) -> Changes {
    let ServerEvent::Durable { event, .. } = event else {
        return Changes::default();
    };
    let state = &mut w.server_log;
    match event {
        Durable::ServerLine(line) => {
            let pushed = state
                .logs
                .get_mut(&line.network_id)
                .is_some_and(|log| log.push(line.clone()));
            if pushed {
                Changes::topic_id(TOPIC, &line.network_id)
            } else {
                Changes::default()
            }
        }
        Durable::ServerLogUpdated(n) => match state.logs.get_mut(&n.network_id) {
            Some(log) if log.loaded => {
                log.stale = true;
                Changes::topic_id(TOPIC, &n.network_id)
            }
            _ => Changes::default(),
        },
        Durable::ConnectionUpdated(info) => {
            let entry = state
                .connections
                .entry(info.network_id.clone())
                .or_default();
            entry.value = Some(info.clone());
            entry.error = None;
            Changes::topic_id(TOPIC, &info.network_id)
        }
        _ => Changes::default(),
    }
}

/// Called when a bootstrap replaces the workspace, before it is replaced.
/// The logs are read again when next shown; the device pings stay.
pub(super) fn reset(w: &mut Workspace, _snapshot: &Snapshot) -> Changes {
    let state = &mut w.server_log;
    for log in state.logs.values_mut().chain(state.traces.values_mut()) {
        log.stale = true;
    }
    Changes::topic(TOPIC)
}

// ---- Folding ----

/// Lines that fold together when several come in a row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Family {
    /// Read markers synced with the server.
    Markread,
    Away,
    Ping,
    /// History played back in batches.
    Batch,
    /// Joins, parts, quits and nick changes (also HistServ's notices of them).
    Presence,
}

/// The family a line folds into, if any.
pub fn family(line: &ServerLine) -> Option<Family> {
    let command = line.command.to_ascii_uppercase();
    match command.as_str() {
        "MARKREAD" => return Some(Family::Markread),
        "AWAY" | "305" | "306" => return Some(Family::Away),
        "PING" | "PONG" => return Some(Family::Ping),
        "BATCH" | "CHATHISTORY" => return Some(Family::Batch),
        "JOIN" | "PART" | "QUIT" | "NICK" => return Some(Family::Presence),
        _ => {}
    }
    (line.kind == ServerLineKind::Notice && is_histserv(line) && is_presence_text(&line.text))
        .then_some(Family::Presence)
}

fn is_histserv(line: &ServerLine) -> bool {
    let source = line.source.as_deref().unwrap_or("").to_ascii_lowercase();
    source.strip_prefix("histserv").is_some_and(|rest| {
        rest.chars()
            .next()
            .is_none_or(|c| !(c.is_alphanumeric() || c == '_'))
    })
}

fn is_presence_text(text: &str) -> bool {
    let text = text.to_lowercase();
    let word = |w: &str| {
        text.match_indices(w).any(|(i, _)| {
            let before = text[..i].chars().next_back();
            let after = text[i + w.len()..].chars().next();
            let edge = |c: Option<char>| c.is_none_or(|c| !(c.is_alphanumeric() || c == '_'));
            edge(before) && edge(after)
        })
    };
    text.contains("joined the channel")
        || text.contains("left the channel")
        || text.contains("changed nick")
        || text.contains("changed their nick")
        || word("quit")
        || word("parted")
}

/// One row of the server window.
#[derive(Debug, Clone, PartialEq)]
pub enum Item<'a> {
    Line(&'a ServerLine),
    /// Several lines of one family in a row; the ID is `group:<first line>`.
    Group {
        id: String,
        family: Family,
        lines: Vec<&'a ServerLine>,
    },
}

/// The server's echo of a line you just sent: same command and text,
/// straight after it.
fn is_echo(previous: Option<&ServerLine>, line: &ServerLine) -> bool {
    previous.is_some_and(|p| {
        p.kind == ServerLineKind::Outgoing
            && line.kind != ServerLineKind::Outgoing
            && p.command.eq_ignore_ascii_case(&line.command)
            && p.text == line.text
    })
}

/// Drop echoes and fold runs of at least `min` lines of one family.
pub fn fold(lines: &[ServerLine], min: usize) -> Vec<Item<'_>> {
    fn flush<'a>(
        run: &mut Vec<&'a ServerLine>,
        family: Option<Family>,
        min: usize,
        items: &mut Vec<Item<'a>>,
    ) {
        match family {
            Some(family) if run.len() >= min => items.push(Item::Group {
                id: format!("group:{}", run[0].id),
                family,
                lines: std::mem::take(run),
            }),
            _ => items.extend(run.drain(..).map(Item::Line)),
        }
    }
    let mut items = Vec::new();
    let mut run: Vec<&ServerLine> = Vec::new();
    let mut run_family: Option<Family> = None;
    for (i, line) in lines.iter().enumerate() {
        if is_echo(i.checked_sub(1).map(|p| &lines[p]), line) {
            continue;
        }
        let family = family(line);
        if family.is_some() && family == run_family {
            run.push(line);
            continue;
        }
        flush(&mut run, run_family, min, &mut items);
        run_family = family;
        match family {
            Some(_) => run.push(line),
            None => items.push(Item::Line(line)),
        }
    }
    flush(&mut run, run_family, min, &mut items);
    items
}

/// Whether `line` matches a filter typed in the server window: any part of
/// its command, source or text, ignoring case and formatting.
pub fn matches(line: &ServerLine, query: &str) -> bool {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return true;
    }
    let hay = format!(
        "{} {} {}",
        line.command,
        line.source.as_deref().unwrap_or(""),
        crate::format::strip(&line.text)
    )
    .to_lowercase();
    hay.contains(&query)
}

/// The nick in a source like `nick!user@host`.
pub fn source_nick(line: &ServerLine) -> Option<&str> {
    let source = line.source.as_deref()?;
    let nick = source.split('!').next().unwrap_or(source);
    (!nick.is_empty()).then_some(nick)
}

// ---- Raw lines ----

/// Why a raw line cannot be sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RawProblem {
    Empty,
    /// More than [`RAW_MAX`] bytes.
    TooLong,
    /// A line break would send a second line.
    LineBreak,
}

/// The line to send, trimmed, or why it cannot go.
pub fn check_raw(line: &str) -> Result<String, RawProblem> {
    let line = line.trim();
    if line.is_empty() {
        Err(RawProblem::Empty)
    } else if line.contains(['\r', '\n']) {
        Err(RawProblem::LineBreak)
    } else if line.len() > RAW_MAX {
        Err(RawProblem::TooLong)
    } else {
        Ok(line.to_string())
    }
}

/// Send one raw line to `network`. Failures after the server accepted it
/// arrive as a `command.error` event.
pub fn send_raw(
    handle: &Handle,
    network: &str,
    line: String,
) -> tokio::task::JoinHandle<Result<(), ApiError>> {
    let path = format!("/api/v1/networks/{}/commands", segment(network));
    handle.request(move |api| async move {
        let body = SendRawRequest {
            client_id: random_id(),
            line,
        };
        api.call_empty(Method::POST, &path, &body).await
    })
}

// ---- Requests ----

/// Read the newest page of a buffer, replacing what is held.
pub fn load(handle: &Handle, network: &str, buffer: Buffer) {
    {
        let mut started = false;
        handle.change(|w| {
            let log = w
                .server_log
                .buffer_mut(buffer)
                .entry(network.to_string())
                .or_default();
            if log.loading {
                return Changes::default();
            }
            started = true;
            log.loading = true;
            log.stale = false;
            Changes::topic_id(TOPIC, network)
        });
        if !started {
            return;
        }
    }
    let path = buffer.path(network);
    let network = network.to_string();
    handle.call(
        move |api| async move {
            api.get_query::<ServerLogPage, _>(&path, &[("limit", PAGE.to_string())])
                .await
        },
        move |w, result| {
            let log = w
                .server_log
                .buffer_mut(buffer)
                .entry(network.clone())
                .or_default();
            log.loading = false;
            match result {
                Ok(page) => {
                    // Live lines that arrived while reading stay if newer.
                    let mut lines = page.items;
                    let newest = lines.last().map(|l| parse_time(&l.time)).unwrap_or(0);
                    for line in log.lines.drain(..) {
                        if parse_time(&line.time) >= newest
                            && !lines.iter().any(|l| l.id == line.id)
                        {
                            lines.push(line);
                        }
                    }
                    log.lines = lines;
                    log.older = page.next_cursor;
                    log.loaded = true;
                    log.error = None;
                }
                Err(e) => log.error = Some(e.to_string()),
            }
            Changes::topic_id(TOPIC, network)
        },
    );
}

/// Read the page before the oldest line held.
pub fn load_older(handle: &Handle, network: &str, buffer: Buffer) {
    let mut before = None;
    handle.change(|w| {
        let Some(log) = w.server_log.buffer_mut(buffer).get_mut(network) else {
            return Changes::default();
        };
        if log.loading || log.older.is_none() {
            return Changes::default();
        }
        log.loading = true;
        before = log.older.clone();
        Changes::topic_id(TOPIC, network)
    });
    let Some(before) = before else {
        return;
    };
    let path = buffer.path(network);
    let network = network.to_string();
    handle.call(
        move |api| async move {
            api.get_query::<ServerLogPage, _>(
                &path,
                &[("limit", PAGE.to_string()), ("before", before)],
            )
            .await
        },
        move |w, result| {
            let log = w
                .server_log
                .buffer_mut(buffer)
                .entry(network.clone())
                .or_default();
            log.loading = false;
            match result {
                Ok(page) => {
                    let mut lines: Vec<ServerLine> = page
                        .items
                        .into_iter()
                        .filter(|l| !log.lines.iter().any(|h| h.id == l.id))
                        .collect();
                    lines.append(&mut log.lines);
                    log.lines = lines;
                    log.older = page.next_cursor;
                    log.error = None;
                }
                Err(e) => log.error = Some(e.to_string()),
            }
            Changes::topic_id(TOPIC, network)
        },
    );
}

fn fetch<T>(
    handle: &Handle,
    network: &str,
    path: String,
    slot: fn(&mut State) -> &mut HashMap<String, Loaded<T>>,
) where
    T: serde::de::DeserializeOwned + Send + Sync + 'static,
{
    let mut started = false;
    handle.change(|w| {
        let entry = slot(&mut w.server_log)
            .entry(network.to_string())
            .or_default();
        if entry.loading {
            return Changes::default();
        }
        entry.loading = true;
        started = true;
        Changes::topic_id(TOPIC, network)
    });
    if !started {
        return;
    }
    let network = network.to_string();
    handle.call(
        move |api| async move { api.get::<T>(&path).await },
        move |w, result| {
            let entry = slot(&mut w.server_log).entry(network.clone()).or_default();
            entry.loading = false;
            match result {
                Ok(value) => {
                    entry.value = Some(value);
                    entry.error = None;
                }
                Err(e) => entry.error = Some(e.to_string()),
            }
            Changes::topic_id(TOPIC, network)
        },
    );
}

/// Read the connection details (state, TLS, capabilities, ISUPPORT).
pub fn load_connection(handle: &Handle, network: &str) {
    let path = format!("/api/v1/networks/{}/connection", segment(network));
    fetch(handle, network, path, |s| &mut s.connections);
}

/// Read the IRC hop's latency samples.
pub fn load_latency(handle: &Handle, network: &str) {
    let path = format!("/api/v1/networks/{}/connection/latency", segment(network));
    fetch(handle, network, path, |s| &mut s.latency);
}

/// Time one `GET /api/v1/ping` and keep it as a device sample.
pub fn ping(handle: &Handle) -> tokio::task::JoinHandle<()> {
    handle.call(
        |api| async move {
            let started = Instant::now();
            api.get::<Ping>("/api/v1/ping").await?;
            Ok(started.elapsed().as_secs_f64() * 1000.0)
        },
        |w, result| {
            let now = chrono::Utc::now().timestamp_millis();
            let state = &mut w.server_log;
            match result {
                Ok(ms) => {
                    state.ping_failing = false;
                    state.device.push(DeviceSample { at_ms: now, ms });
                    state.device.retain(|s| now - s.at_ms <= DEVICE_WINDOW_MS);
                }
                // Only an unreachable server makes the connection unstable.
                Err(ApiError::Network(_)) => state.ping_failing = true,
                Err(_) => {}
            }
            Changes::topic(TOPIC)
        },
    )
}

#[cfg(test)]
mod tests;

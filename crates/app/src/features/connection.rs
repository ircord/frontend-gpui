//! Connection details and latency (package SV1).
//!
//! - The indicator under the conversation list: the app's and the IRC
//!   network's state, and the device's ping to IRCord. Clicking it opens
//!   the network's server window.
//! - [`details`]: the server window's Connection tab, with the state, both
//!   hops' latency, the TLS certificate and its grade, capabilities and the
//!   server's settings (ISUPPORT).
//!
//! The app pings IRCord every twenty seconds while signed in, and re-reads
//! the IRC hop's latency while a server window shows it.

use std::time::Duration;

use gpui_kit::TestSupportExt;
use gpui_kit::prelude::*;
use gpui_kit::*;
use ircord_client::contract::{ConnectionInfo, NetworkStatus, TLSInfo, TLSInfoGrade};
use ircord_client::store::server_log::{self, PING_EVERY_MS, average_and_peak};
use ircord_client::store::{Connection, Workspace, parse_time};

use crate::registry::{self, SidebarCtx, Slot};
use crate::route::Route;
use crate::session::{Session, SessionEvent};
use crate::theme::{self, Theme};

pub fn init(cx: &mut App) {
    cx.observe_new::<Session>(|_, _, cx| {
        cx.subscribe_self(|session: &mut Session, event: &SessionEvent, _| {
            // A first ping as soon as the workspace is live, so the
            // indicator has a value before the first tick.
            if let SessionEvent::Connection = event
                && let Some(client) = session.client()
            {
                let first = {
                    let w = client.read();
                    w.connection == Some(Connection::Live) && w.server_log.device.is_empty()
                };
                if first {
                    drop(server_log::ping(client.handle()));
                }
            }
        })
        .detach();
        cx.spawn(async move |session, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(PING_EVERY_MS))
                    .await;
                let Ok(()) = session.update(cx, |s, _| tick(s)) else {
                    break;
                };
            }
        })
        .detach();
    })
    .detach();
    registry::register(cx, |r| {
        r.sidebar_footer
            .add(Slot::new("connection", |ctx: &SidebarCtx, _, cx| {
                Some(indicator(&ctx.session, cx).into_any_element())
            }));
    });
}

/// Every twenty seconds: ping IRCord, and re-read the IRC hop's latency of
/// the server window showing.
fn tick(session: &Session) {
    let Some(client) = session.client() else {
        return;
    };
    if client.read().connection != Some(Connection::Live) {
        return;
    }
    drop(server_log::ping(client.handle()));
    if let Some(Route::ServerWindow(network)) = session.route() {
        server_log::load_latency(client.handle(), &network);
    }
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

// ---- The indicator ----

/// The one indicator, kept for the session it shows.
struct Indicator(Entity<IndicatorView>);
impl Global for Indicator {}

fn indicator(session: &Entity<Session>, cx: &mut App) -> Entity<IndicatorView> {
    if let Some(Indicator(view)) = cx.try_global::<Indicator>()
        && view.read(cx).session == *session
    {
        return view.clone();
    }
    let view = cx.new(|cx| IndicatorView::new(session.clone(), cx));
    cx.set_global(Indicator(view.clone()));
    view
}

pub struct IndicatorView {
    session: Entity<Session>,
    _subscription: Subscription,
}

impl IndicatorView {
    fn new(session: Entity<Session>, cx: &mut Context<Self>) -> Self {
        let subscription = cx.subscribe(&session, |_, _, event: &SessionEvent, cx| match event {
            SessionEvent::Changed(changes)
                if changes.sidebar || changes.touches(server_log::TOPIC) =>
            {
                cx.notify()
            }
            SessionEvent::Connection | SessionEvent::Selection => cx.notify(),
            _ => {}
        });
        Self {
            session,
            _subscription: subscription,
        }
    }
}

/// How the indicator's dot looks.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Health {
    Good,
    Degraded,
    Offline,
}

/// The indicator's words and dot for the app's connection and `network`.
pub fn status(w: &Workspace, network: Option<&str>) -> (&'static str, Health) {
    match &w.connection {
        Some(Connection::Offline { .. }) | None => {
            return (t!("server-window-indicator-offline"), Health::Offline);
        }
        Some(Connection::Replaying) => {
            return (t!("server-window-indicator-catching-up"), Health::Degraded);
        }
        Some(Connection::Connecting) => {
            return (t!("server-window-indicator-connecting"), Health::Degraded);
        }
        Some(Connection::Live) => {}
    }
    if w.server_log.ping_failing {
        return (t!("server-window-indicator-unstable"), Health::Degraded);
    }
    match network.and_then(|n| w.network(n)).map(|n| n.status) {
        Some(NetworkStatus::Error) => (t!("server-window-indicator-irc-failed"), Health::Degraded),
        Some(NetworkStatus::Connecting) => (
            t!("server-window-indicator-irc-connecting"),
            Health::Degraded,
        ),
        Some(NetworkStatus::Disconnected) => (
            t!("server-window-indicator-irc-disconnected"),
            Health::Degraded,
        ),
        _ => (t!("server-window-indicator-connected"), Health::Good),
    }
}

fn health_colour(health: Health, theme: &Theme) -> Hsla {
    match health {
        Health::Good => theme.online,
        Health::Degraded => theme.warning,
        Health::Offline => theme.offline,
    }
}

impl Render for IndicatorView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme::get(cx).clone();
        let session = self.session.read(cx);
        let Some(client) = session.client().cloned() else {
            return div().into_any_element();
        };
        let network = session.network.clone();
        let w = client.read();
        let (label, health) = status(&w, network.as_deref());
        let latency = (health == Health::Good)
            .then(|| w.server_log.device_latency(now_ms()))
            .flatten();
        drop(w);
        let latency_text = latency.map(|ms| t!("server-window-ms", value = ms.round()));
        let accessible = match &latency_text {
            Some(l) => t!("server-window-indicator-label", status = label, latency = l),
            None => label.to_string(),
        };
        let open = self.session.clone();
        div()
            .id("connection-indicator")
            .test_support()
            .role(Role::Button)
            .aria_label(SharedString::from(accessible.clone()))
            .tooltip(crate::ui::tooltip::text(accessible))
            .mx_2()
            .mb_2()
            .px_2()
            .py_1()
            .rounded_md()
            .flex()
            .items_center()
            .gap_2()
            .text_xs()
            .text_color(theme.muted)
            .cursor_pointer()
            .hover(|d| d.bg(theme.hover))
            .child(
                div()
                    .size(px(8.))
                    .rounded_full()
                    .bg(health_colour(health, &theme)),
            )
            .child(div().flex_1().truncate().child(label))
            .when_some(latency_text, |d, text| {
                d.child(
                    div()
                        .id("connection-latency")
                        .test_support()
                        .aria_label(SharedString::from(text.clone()))
                        .px_1p5()
                        .rounded_full()
                        .bg(theme.input)
                        .text_color(theme.text)
                        .child(text),
                )
            })
            .on_click(move |_, _, cx| {
                open.update(cx, |s, cx| {
                    if let Some(network) = s.network.clone() {
                        s.navigate(Route::ServerWindow(network), cx);
                    }
                })
            })
            .into_any_element()
    }
}

// ---- The Connection tab ----

/// Read the details and the latency of `network` unless already read or
/// being read.
pub fn ensure_loaded(session: &Session, network: &str) {
    let Some(client) = session.client() else {
        return;
    };
    let (details, latency) = {
        let w = client.read();
        let s = &w.server_log;
        (
            s.connections
                .get(network)
                .is_none_or(|c| c.value.is_none() && !c.loading),
            s.latency
                .get(network)
                .is_none_or(|c| c.value.is_none() && !c.loading),
        )
    };
    if details {
        server_log::load_connection(client.handle(), network);
    }
    if latency {
        server_log::load_latency(client.handle(), network);
    }
}

fn section(id: &'static str, title: &'static str, theme: &Theme) -> Stateful<Div> {
    div()
        .id(id)
        .role(Role::Region)
        .aria_label(title)
        .flex()
        .flex_col()
        .gap_1()
        .p_3()
        .rounded_md()
        .bg(theme.sidebar)
        .child(
            div()
                .pb_1()
                .text_xs()
                .font_weight(FontWeight::BOLD)
                .text_color(theme.muted)
                .child(title.to_uppercase()),
        )
}

fn pair(label: &'static str, value: impl Into<SharedString>, theme: &Theme) -> Div {
    div()
        .flex()
        .gap_3()
        .text_sm()
        .child(
            div()
                .w(px(140.))
                .flex_none()
                .text_color(theme.muted)
                .child(label),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_color(theme.text)
                .child(value.into()),
        )
}

/// The grade as written: `A+`, `B-`…
pub fn grade_text(grade: TLSInfoGrade) -> &'static str {
    match grade {
        TLSInfoGrade::APlus => "A+",
        TLSInfoGrade::A => "A",
        TLSInfoGrade::AMinus => "A-",
        TLSInfoGrade::BPlus => "B+",
        TLSInfoGrade::B => "B",
        TLSInfoGrade::BMinus => "B-",
        TLSInfoGrade::C => "C",
        TLSInfoGrade::D => "D",
        TLSInfoGrade::F => "F",
        TLSInfoGrade::Unknown => "?",
    }
}

fn grade_colour(grade: &str, theme: &Theme) -> Hsla {
    if grade.starts_with('A') {
        theme.online
    } else if grade.starts_with('B') {
        theme.warning
    } else {
        theme.danger
    }
}

fn local_time(text: &str, format: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(text)
        .map(|t| t.with_timezone(&chrono::Local).format(format).to_string())
        .unwrap_or_else(|_| text.to_string())
}

fn ms(value: Option<f64>) -> String {
    match value {
        Some(v) => t!("server-window-ms", value = v.round()),
        None => t!("server-window-no-value").to_string(),
    }
}

/// The server window's Connection tab for `network`.
pub fn details(session: &Entity<Session>, network: &str, cx: &mut App) -> AnyElement {
    let theme = theme::get(cx).clone();
    let Some(client) = session.read(cx).client().cloned() else {
        return div().into_any_element();
    };
    let w = client.read();
    let Some(net) = w.network(network) else {
        return div().into_any_element();
    };
    let loaded = w.server_log.connections.get(network);
    let info = loaded.and_then(|c| c.value.as_ref());
    let error = loaded.and_then(|c| c.error.clone());
    let connected = net.status == NetworkStatus::Connected;
    let now = now_ms();

    // Status.
    let state = match net.status {
        NetworkStatus::Connected => t!("server-window-state-connected"),
        NetworkStatus::Connecting => t!("server-window-state-connecting"),
        NetworkStatus::Error => t!("server-window-state-failed"),
        _ => t!("server-window-state-disconnected"),
    };
    let server_name = info
        .map(|i| i.server_name.clone())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| net.host.clone());
    let mut status = section("connection-status", t!("server-window-status"), &theme).child(
        div()
            .id("connection-state")
            .test_support()
            .aria_label(SharedString::from(format!("{state} · {server_name}")))
            .flex()
            .items_center()
            .gap_2()
            .text_sm()
            .text_color(theme.text)
            .child(div().size(px(8.)).rounded_full().bg(if connected {
                theme.online
            } else {
                theme.offline
            }))
            .child(format!("{state} · {server_name}")),
    );
    if let Some(since) = info
        .and_then(|i| i.connected_at.as_deref())
        .filter(|_| connected)
    {
        status = status.child(pair(
            t!("server-window-connected-since"),
            local_time(since, "%Y-%m-%d %H:%M"),
            &theme,
        ));
    }
    let last_error = info
        .and_then(|i| i.last_error.clone())
        .or(net.status_detail.clone());
    if !connected && let Some(e) = last_error.filter(|e| !e.is_empty()) {
        status = status.child(div().text_sm().text_color(theme.danger).child(e));
    }
    if let Some(e) = error {
        status = status.child(div().text_sm().text_color(theme.danger).child(e));
    }

    // Latency.
    let device = w.server_log.device_latency(now);
    let irc = w.server_log.irc_latency(network, now);
    let totals = w.server_log.totals(network);
    let total = match (device, irc) {
        (Some(d), Some(i)) => Some(d + i),
        _ => None,
    };
    let latency = section(
        "connection-latency-details",
        t!("server-window-latency"),
        &theme,
    )
    .child(
        div()
            .id("connection-path")
            .test_support()
            .aria_label(SharedString::from(t!(
                "server-window-path-label",
                device = ms(device),
                irc = ms(irc)
            )))
            .flex()
            .items_center()
            .gap_2()
            .text_sm()
            .text_color(theme.text)
            .child(t!("server-window-you"))
            .child(hop(ms(device), &theme))
            .child(t!("core-app-name"))
            .child(hop(ms(irc), &theme))
            .child(server_name.clone()),
    )
    .child(pair(t!("server-window-hop-device"), ms(device), &theme))
    .child(pair(t!("server-window-hop-irc"), ms(irc), &theme))
    .child(pair(t!("server-window-total"), ms(total), &theme))
    .when_some(
        average_and_peak(totals.iter().map(|s| s.ms)),
        |d, (average, peak)| {
            d.child(div().text_xs().text_color(theme.muted).child(t!(
                "server-window-average-peak",
                average = ms(Some(average)),
                peak = ms(Some(peak))
            )))
        },
    )
    .when(!totals.is_empty(), |d| d.child(chart(&totals, &theme)));

    // Security.
    let security = security(info.and_then(|i| i.tls.as_ref()), net.tls, now, &theme);

    // Capabilities and extensions.
    let capabilities = capabilities(info, connected, &theme)
        .child(
            div()
                .pt_2()
                .text_xs()
                .font_weight(FontWeight::BOLD)
                .text_color(theme.muted)
                .child(t!("server-window-extensions").to_uppercase()),
        )
        .child(pair(
            t!("server-window-message-editing"),
            if connected && net.features.edit {
                t!("server-window-available")
            } else {
                t!("server-window-unavailable")
            },
            &theme,
        ));

    // ISUPPORT and the session.
    let mut settings = section("connection-isupport", t!("server-window-isupport"), &theme);
    let isupport = info.map(|i| &i.isupport).unwrap_or(&net.isupport);
    for (key, value) in isupport {
        let value = if value.is_empty() {
            t!("server-window-enabled").to_string()
        } else {
            value.clone()
        };
        settings = settings.child(
            div()
                .flex()
                .gap_3()
                .text_sm()
                .child(
                    div()
                        .w(px(140.))
                        .flex_none()
                        .text_color(theme.muted)
                        .child(key.clone()),
                )
                .child(div().flex_1().min_w_0().text_color(theme.text).child(value)),
        );
    }
    let session_part = section("connection-session", t!("server-window-session"), &theme)
        .child(pair(t!("server-window-nick"), net.nick.clone(), &theme))
        .child(pair(
            t!("server-window-address"),
            format!("{}:{}", net.host, net.port),
            &theme,
        ));

    div()
        .id("connection-details")
        .test_support()
        .role(Role::Region)
        .aria_label(t!("server-window-tab-connection"))
        .flex()
        .flex_col()
        .gap_3()
        .p_4()
        .child(status.test_support())
        .child(latency.test_support())
        .child(security.test_support())
        .child(capabilities.test_support())
        .child(session_part.test_support())
        .child(settings.test_support())
        .into_any_element()
}

fn hop(value: String, theme: &Theme) -> Div {
    div()
        .flex()
        .items_center()
        .gap_1()
        .text_xs()
        .text_color(theme.muted)
        .child(div().w(px(24.)).h(px(1.)).bg(theme.faint))
        .child(value)
        .child(div().w(px(24.)).h(px(1.)).bg(theme.faint))
}

/// Bars of the total latency over the window, newest on the right.
fn chart(totals: &[server_log::DeviceSample], theme: &Theme) -> impl IntoElement {
    let peak = totals.iter().map(|s| s.ms).fold(1.0_f64, f64::max);
    div()
        .id("connection-latency-chart")
        .test_support()
        .role(Role::Image)
        .aria_label(t!("server-window-chart-label", count = totals.len()))
        .h(px(48.))
        .mt_1()
        .flex()
        .items_end()
        .gap_px()
        .children(totals.iter().rev().take(90).rev().map(|s| {
            let h = (s.ms / peak * 46.0).max(2.0) as f32;
            div().w(px(4.)).h(px(h)).rounded_sm().bg(theme.accent)
        }))
}

fn security(tls: Option<&TLSInfo>, wants_tls: bool, now: i64, theme: &Theme) -> Stateful<Div> {
    let mut part = section("connection-security", t!("server-window-security"), theme);
    let Some(tls) = tls else {
        let (grade, line, note) = if wants_tls {
            (
                "",
                t!("server-window-encrypted"),
                t!("server-window-details-later"),
            )
        } else {
            (
                "F",
                t!("server-window-unencrypted"),
                t!("server-window-readable"),
            )
        };
        return part
            .child(grade_row(grade, line, theme))
            .child(div().text_sm().text_color(theme.muted).child(note));
    };
    let grade = grade_text(tls.grade);
    let verification = if tls.verified {
        t!("server-window-verified")
    } else {
        t!("server-window-not-verified")
    };
    part = part.child(grade_row(
        grade,
        &t!(
            "server-window-protocol-line",
            version = tls.version,
            verification = verification
        ),
        theme,
    ));
    if tls.grade_notes.is_empty() {
        part = part.child(
            div()
                .text_sm()
                .text_color(theme.muted)
                .child(t!("server-window-grade-good")),
        );
    }
    for note in &tls.grade_notes {
        part = part.child(
            div()
                .text_sm()
                .text_color(theme.warning)
                .child(note.clone()),
        );
    }
    let yes_no = |b: bool| {
        if b {
            t!("server-window-yes")
        } else {
            t!("server-window-no")
        }
    };
    part = part
        .child(pair(
            t!("server-window-protocol"),
            tls.version.clone(),
            theme,
        ))
        .child(pair(t!("server-window-cipher"), tls.cipher.clone(), theme))
        .child(pair(
            t!("server-window-forward-secrecy"),
            yes_no(tls.forward_secrecy),
            theme,
        ));
    if let Some(algorithm) = tls.key_algorithm {
        let name = format!("{algorithm:?}");
        let key = match tls.key_bits {
            Some(bits) => t!("server-window-key-bits", algorithm = name, bits = bits),
            None => name,
        };
        part = part.child(pair(t!("server-window-key"), key, theme));
    }
    if let Some(s) = &tls.signature_algorithm {
        part = part.child(pair(t!("server-window-signature"), s.clone(), theme));
    }
    if let Some(s) = &tls.subject {
        part = part.child(pair(t!("server-window-issued-to"), s.clone(), theme));
    }
    if let Some(s) = &tls.issuer {
        part = part.child(pair(t!("server-window-issued-by"), s.clone(), theme));
    }
    if let Some(after) = &tls.not_after {
        let days = (parse_time(after) - now).div_euclid(86_400_000);
        let expires = if days < 0 {
            t!("server-window-expired").to_string()
        } else {
            t!("server-window-in-days", days = days)
        };
        part = part.child(pair(
            t!("server-window-valid-until"),
            t!(
                "server-window-valid-until-value",
                date = local_time(after, "%Y-%m-%d"),
                expires = expires
            ),
            theme,
        ));
    }
    if let Some(s) = &tls.certificate_sha256 {
        part = part.child(pair(t!("server-window-sha256"), s.clone(), theme));
    }
    part
}

fn grade_row(grade: &str, line: &str, theme: &Theme) -> Div {
    let grade: SharedString = grade.to_string().into();
    div()
        .flex()
        .items_center()
        .gap_3()
        .child(
            div()
                .id("connection-grade")
                .test_support()
                .aria_label(SharedString::from(t!("server-window-grade", grade = grade)))
                .w(px(40.))
                .text_xl()
                .font_weight(FontWeight::BOLD)
                .text_color(grade_colour(&grade, theme))
                .child(grade.clone()),
        )
        .child(
            div()
                .text_sm()
                .text_color(theme.text)
                .child(line.to_string()),
        )
}

fn capabilities(info: Option<&ConnectionInfo>, connected: bool, theme: &Theme) -> Stateful<Div> {
    let rows = info.map(capability_rows).unwrap_or_default();
    let active = rows.iter().filter(|r| r.2).count();
    let mut part = section(
        "connection-capabilities",
        t!("server-window-capabilities"),
        theme,
    )
    .child(
        div()
            .id("connection-capability-count")
            .test_support()
            .aria_label(SharedString::from(t!(
                "server-window-capability-count",
                active = active,
                total = rows.len()
            )))
            .text_xs()
            .text_color(theme.muted)
            .child(t!(
                "server-window-capability-count",
                active = active,
                total = rows.len()
            )),
    );
    if rows.is_empty() {
        return part.child(
            div()
                .text_sm()
                .text_color(theme.muted)
                .child(t!("server-window-capabilities-later")),
        );
    }
    if !connected {
        part = part.child(
            div()
                .text_sm()
                .text_color(theme.muted)
                .child(t!("server-window-capabilities-last")),
        );
    }
    for (name, value, on) in rows {
        part = part.child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .text_sm()
                .child(div().size(px(6.)).rounded_full().bg(if on && connected {
                    theme.online
                } else {
                    theme.faint
                }))
                .child(div().text_color(theme.text).child(name))
                .when(!value.is_empty(), |d| {
                    d.child(div().truncate().text_color(theme.muted).child(value))
                }),
        );
    }
    part
}

/// Offered and negotiated capabilities, sorted: name, value, active.
pub fn capability_rows(info: &ConnectionInfo) -> Vec<(String, String, bool)> {
    let mut names: Vec<&String> = info
        .offered_capabilities
        .keys()
        .chain(info.negotiated_capabilities.iter())
        .collect();
    names.sort();
    names.dedup();
    names
        .into_iter()
        .map(|name| {
            (
                name.clone(),
                info.offered_capabilities
                    .get(name)
                    .cloned()
                    .unwrap_or_default(),
                info.negotiated_capabilities.contains(name),
            )
        })
        .collect()
}

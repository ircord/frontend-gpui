//! A network's server window and raw input (package SV1).
//!
//! Three tabs over one network: the overview (as the web's: Status, Latency,
//! Security and Session cards over the server log of numerics, notices,
//! errors, the MOTD and what you sent, with runs of read markers, presence,
//! away changes, pings and history folded into one row), the connection details
//! (`features::connection`), and the connection trace (every line on the
//! wire, secrets redacted). Under the log, a box sends one raw IRC line.
//!
//! Opened from the network menu, the connection indicator and `/server`
//! (and after commands whose answer lands here, such as `/whois`).

use crate::menu_order;
use crate::ui::focus::Reachable;
use std::collections::{HashMap, HashSet};

use gpui_kit::TestSupportExt;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::*;
use gpui_kit::*;
use ircord_client::commands::View;
use ircord_client::contract::{NetworkStatus, ServerLine, ServerLineKind};
use ircord_client::store::Connection;
use ircord_client::store::server_log::{
    self, Buffer, Family, Item, MIN_GROUP, RAW_MAX, RawProblem, check_raw, fold, matches,
    source_nick,
};

use crate::features::{commands, connection};
use crate::registry::{self, MenuItem, NetworkCtx, Pane};
use crate::route::{PaneKind, Route};
use crate::session::{Session, SessionEvent};
use crate::theme::{self, Theme};
use crate::ui;

pub fn init(cx: &mut App) {
    registry::register(cx, |r| {
        r.panes
            .add(Pane::new(PaneKind::ServerWindow, |ctx, window, cx| {
                let Route::ServerWindow(network) = &ctx.route else {
                    return div().into_any_element();
                };
                view(&ctx.session, network, window, cx).into_any_element()
            }));
        r.network_menu.add(
            MenuItem::new(
                "server-window",
                t!("server-window-open"),
                |ctx: &NetworkCtx, _, cx| open(&ctx.session, &ctx.network, cx),
            )
            .icon(ui::icon::IconName::SquareTerminal)
            .order(menu_order::network::SERVER_WINDOW),
        );
    });
    commands::on_view(cx, View::ServerWindow, |request, _, cx| {
        open(&request.session, &request.network, cx)
    });
}

/// Show `network`'s server window.
pub fn open(session: &Entity<Session>, network: &str, cx: &mut App) {
    let route = Route::ServerWindow(network.to_string());
    session.update(cx, |s, cx| s.navigate(route, cx));
}

/// The open server windows, kept across visits so the tab, the filter and
/// what is unfolded stay.
#[derive(Default)]
struct Windows {
    session: Option<Entity<Session>>,
    views: HashMap<String, Entity<ServerWindowView>>,
}
impl Global for Windows {}

fn view(
    session: &Entity<Session>,
    network: &str,
    window: &mut Window,
    cx: &mut App,
) -> Entity<ServerWindowView> {
    let windows = cx.default_global::<Windows>();
    if windows.session.as_ref() != Some(session) {
        windows.session = Some(session.clone());
        windows.views.clear();
    }
    if let Some(view) = windows.views.get(network) {
        return view.clone();
    }
    let view = cx.new(|cx| ServerWindowView::new(session.clone(), network.to_string(), window, cx));
    cx.global_mut::<Windows>()
        .views
        .insert(network.to_string(), view.clone());
    view
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tab {
    Log,
    Connection,
    Trace,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CapabilityScope {
    All,
    Active,
    Offered,
}

const WIDE_SERVER_PANE: Pixels = px(1050.);

pub struct ServerWindowView {
    session: Entity<Session>,
    network: String,
    tab: Tab,
    filter: Entity<InputState>,
    capability_filter: Entity<InputState>,
    capability_scope: CapabilityScope,
    capability_groups: [bool; 3],
    wide: bool,
    show_capabilities: bool,
    raw: Entity<InputState>,
    /// Folded groups shown open, by group ID.
    opened: HashSet<String>,
    scroll: ScrollHandle,
    /// Lines in the shown list at the last draw, to follow new ones.
    drawn: usize,
    _subscriptions: Vec<Subscription>,
}

impl ServerWindowView {
    fn new(
        session: Entity<Session>,
        network: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let filter =
            cx.new(|cx| InputState::new(window, cx).placeholder(t!("server-window-filter")));
        let capability_filter = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t!("server-window-capability-filter"))
        });
        let raw = cx
            .new(|cx| InputState::new(window, cx).placeholder(t!("server-window-raw-placeholder")));
        let id = network.clone();
        let subscriptions = vec![
            cx.subscribe(&capability_filter, |_, _, event: &InputEvent, cx| {
                if let InputEvent::Change = event {
                    cx.notify();
                }
            }),
            cx.subscribe(&filter, |_, _, event: &InputEvent, cx| {
                if let InputEvent::Change = event {
                    cx.notify();
                }
            }),
            cx.subscribe_in(&raw, window, |this, _, event: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    this.send(window, cx);
                }
            }),
            cx.subscribe(
                &session,
                move |this, _, event: &SessionEvent, cx| match event {
                    SessionEvent::Changed(changes)
                        if changes.sidebar || changes.touches_id(server_log::TOPIC, &id) =>
                    {
                        this.ensure_loaded(cx);
                        cx.notify();
                    }
                    SessionEvent::Connection | SessionEvent::Route => {
                        this.ensure_loaded(cx);
                        cx.notify();
                    }
                    _ => {}
                },
            ),
        ];
        let mut this = Self {
            session,
            network,
            tab: Tab::Log,
            filter,
            capability_filter,
            capability_scope: CapabilityScope::All,
            capability_groups: [true; 3],
            wide: false,
            show_capabilities: false,
            raw,
            opened: HashSet::new(),
            scroll: ScrollHandle::new(),
            drawn: 0,
            _subscriptions: subscriptions,
        };
        this.ensure_loaded(cx);
        this
    }

    fn showing(&self, cx: &App) -> bool {
        self.session.read(cx).route() == Some(Route::ServerWindow(self.network.clone()))
    }

    /// Read what the open tab shows, if it is missing or out of date.
    fn ensure_loaded(&mut self, cx: &mut Context<Self>) {
        if !self.showing(cx) {
            return;
        }
        let session = self.session.read(cx);
        let Some(client) = session.client() else {
            return;
        };
        if client.read().connection != Some(Connection::Live) {
            return;
        }
        let buffer = match self.tab {
            Tab::Log => {
                // The overview's cards read the details and the latency,
                // once: a failed read is not retried on every change.
                let (details, latency) = {
                    let w = client.read();
                    (
                        w.server_log.connections.contains_key(&self.network),
                        w.server_log.latency.contains_key(&self.network),
                    )
                };
                if !details {
                    server_log::load_connection(client.handle(), &self.network);
                }
                if !latency {
                    server_log::load_latency(client.handle(), &self.network);
                }
                Buffer::Log
            }
            Tab::Trace => Buffer::Trace,
            Tab::Connection => {
                connection::ensure_loaded(session, &self.network);
                return;
            }
        };
        let wanted = client
            .read()
            .server_log
            .log(&self.network, buffer)
            .is_none_or(|l| !l.loading && (l.stale || (!l.loaded && l.error.is_none())));
        if wanted {
            server_log::load(client.handle(), &self.network, buffer);
        }
    }

    pub fn tab(&self) -> Tab {
        self.tab
    }

    pub fn set_tab(&mut self, tab: Tab, cx: &mut Context<Self>) {
        self.tab = tab;
        self.drawn = 0;
        self.ensure_loaded(cx);
        cx.notify();
    }

    fn connected(&self, cx: &App) -> bool {
        let Some(client) = self.session.read(cx).client() else {
            return false;
        };
        let w = client.read();
        w.connection == Some(Connection::Live)
            && w.network(&self.network)
                .is_some_and(|n| n.status == NetworkStatus::Connected)
    }

    /// Send the raw box's line.
    fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.raw.read(cx).value().to_string();
        let line = match check_raw(&text) {
            Ok(line) => line,
            Err(RawProblem::Empty) => return,
            Err(RawProblem::TooLong) => {
                ui::toast::error(window, cx, t!("server-window-raw-too-long", max = RAW_MAX));
                return;
            }
            Err(RawProblem::LineBreak) => {
                ui::toast::error(window, cx, t!("server-window-raw-one-line"));
                return;
            }
        };
        if !self.connected(cx) {
            ui::toast::error(window, cx, t!("server-window-not-connected"));
            return;
        }
        let Some(client) = self.session.read(cx).client().cloned() else {
            return;
        };
        self.raw.update(cx, |s, cx| s.set_value("", window, cx));
        let sent = server_log::send_raw(client.handle(), &self.network, line);
        cx.spawn_in(window, async move |_, cx| {
            let error = match sent.await {
                Ok(Ok(())) => return,
                Ok(Err(e)) => e.to_string(),
                Err(e) => e.to_string(),
            };
            let _ = cx.update(|window, cx| {
                ui::toast::error(window, cx, t!("server-window-raw-failed", error = error))
            });
        })
        .detach();
    }

    fn load_older(&mut self, cx: &mut Context<Self>) {
        let buffer = if self.tab == Tab::Trace {
            Buffer::Trace
        } else {
            Buffer::Log
        };
        if let Some(client) = self.session.read(cx).client() {
            server_log::load_older(client.handle(), &self.network, buffer);
        }
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let Some(client) = self.session.read(cx).client().cloned() else {
            return;
        };
        match self.tab {
            Tab::Log => {
                server_log::load(client.handle(), &self.network, Buffer::Log);
                server_log::load_connection(client.handle(), &self.network);
                server_log::load_latency(client.handle(), &self.network);
            }
            Tab::Trace => server_log::load(client.handle(), &self.network, Buffer::Trace),
            Tab::Connection => {
                server_log::load_connection(client.handle(), &self.network);
                server_log::load_latency(client.handle(), &self.network);
            }
        }
    }

    fn toggle(&mut self, group: String, cx: &mut Context<Self>) {
        if !self.opened.remove(&group) {
            self.opened.insert(group);
        }
        cx.notify();
    }

    /// Keep following the newest line when the list was at its end.
    fn follow(&mut self, count: usize, window: &mut Window, cx: &mut Context<Self>) {
        if count == self.drawn {
            return;
        }
        let at_end = self.scroll.offset().y.abs() + px(40.) >= self.scroll.max_offset().y;
        self.drawn = count;
        if at_end {
            let this = cx.entity().downgrade();
            window.on_next_frame(move |_, cx| {
                let _ = this.update(cx, |this, cx| {
                    this.scroll.scroll_to_bottom();
                    cx.notify();
                });
            });
        }
    }
}

fn family_text(family: Family, count: usize) -> String {
    match family {
        Family::Markread => t!("server-window-group-markread", count = count),
        Family::Presence => t!("server-window-group-presence", count = count),
        Family::Away => t!("server-window-group-away", count = count),
        Family::Ping => t!("server-window-group-ping", count = count),
        Family::Batch => t!("server-window-group-batch", count = count),
    }
}

fn capability_description(name: &str) -> &'static str {
    match name {
        "message-tags" => t!("server-window-cap-desc-message-tags"),
        "echo-message" => t!("server-window-cap-desc-echo-message"),
        "server-time" => t!("server-window-cap-desc-server-time"),
        "batch" => t!("server-window-cap-desc-batch"),
        "labeled-response" => t!("server-window-cap-desc-labeled-response"),
        "draft/chathistory" => t!("server-window-cap-desc-chathistory"),
        "draft/multiline" => t!("server-window-cap-desc-multiline"),
        "draft/metadata-3" => t!("server-window-cap-desc-metadata"),
        "draft/message-redaction" => t!("server-window-cap-desc-redaction"),
        "draft/read-marker" => t!("server-window-cap-desc-read-marker"),
        "setname" => t!("server-window-cap-desc-setname"),
        "account-tag" => t!("server-window-cap-desc-account-tag"),
        "account-notify" => t!("server-window-cap-desc-account-notify"),
        "away-notify" => t!("server-window-cap-desc-away-notify"),
        "extended-join" => t!("server-window-cap-desc-extended-join"),
        "chghost" => t!("server-window-cap-desc-chghost"),
        "multi-prefix" => t!("server-window-cap-desc-multi-prefix"),
        "cap-notify" => t!("server-window-cap-desc-cap-notify"),
        "sasl" => t!("server-window-cap-desc-sasl"),
        _ => "",
    }
}

/// `→ COMMAND` for what you sent, `-nick-` for a notice, else the command.
pub fn label(line: &ServerLine) -> String {
    match line.kind {
        ServerLineKind::Outgoing => format!("→ {}", line.command),
        ServerLineKind::Notice => format!(
            "-{}-",
            source_nick(line).unwrap_or(t!("server-window-server"))
        ),
        _ => line.command.clone(),
    }
}

fn time_of(line: &ServerLine, format: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(&line.time)
        .map(|t| t.with_timezone(&chrono::Local).format(format).to_string())
        .unwrap_or_default()
}

fn kind_colour(kind: ServerLineKind, theme: &Theme) -> Hsla {
    match kind {
        ServerLineKind::Error => theme.danger,
        ServerLineKind::Outgoing => theme.accent,
        ServerLineKind::Notice => theme.warning,
        ServerLineKind::Motd => theme.text,
        _ => theme.muted,
    }
}

fn line_row(line: &ServerLine, indent: bool, theme: &Theme) -> impl IntoElement + use<> {
    let text = ircord_client::format::strip(&line.text);
    let label = label(line);
    div()
        .id(ElementId::Name(format!("server-line-{}", line.id).into()))
        .test_support()
        .role(Role::ListItem)
        .aria_label(SharedString::from(format!("{label} {text}")))
        .px_4()
        .when(indent, |d| d.pl_10())
        .py_0p5()
        .flex()
        .gap_2()
        .text_sm()
        .font_family(theme.mono_font.clone())
        .hover(|d| d.bg(theme.row_hover))
        .child(time_cell(time_of(line, "%H:%M"), theme))
        .child(command_cell(label, kind_colour(line.kind, theme)))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_color(match line.kind {
                    ServerLineKind::Error => theme.danger,
                    ServerLineKind::Outgoing => theme.accent,
                    _ => theme.text,
                })
                .child(text),
        )
}

/// The time column, to the minute as on the web.
fn time_cell(time: String, theme: &Theme) -> Div {
    div()
        .flex_none()
        .w(px(48.))
        .text_color(theme.faint)
        .child(time)
}

/// The command or numeric column, right-aligned against the text.
fn command_cell(label: impl Into<SharedString>, color: Hsla) -> Div {
    div()
        .flex_none()
        .w(px(110.))
        .flex()
        .justify_end()
        .truncate()
        .text_color(color)
        .child(label.into())
}

fn text_button<F: Fn(&ClickEvent, &mut Window, &mut App) + 'static>(
    id: &'static str,
    label: &'static str,
    selected: Option<bool>,
    theme: &Theme,
    on_click: F,
) -> impl IntoElement + use<F> {
    let active = selected == Some(true);
    let mut button = div()
        .id(id)
        .role(if selected.is_some() {
            Role::Tab
        } else {
            Role::Button
        })
        .aria_label(label)
        .px_2()
        .py_1()
        .rounded_md()
        .text_sm()
        .cursor_pointer()
        .text_color(if active { theme.text } else { theme.muted })
        .when(active, |d| d.bg(theme.selected))
        .when(!active, |d| d.hover(|d| d.bg(theme.hover)))
        .child(label)
        .on_click(on_click);
    if let Some(selected) = selected {
        button = button.aria_selected(selected);
    }
    button.test_support().reachable(theme)
}

impl Render for ServerWindowView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme::get(cx).clone();
        let Some(client) = self.session.read(cx).client().cloned() else {
            return div().into_any_element();
        };
        let (name, address) = client
            .read()
            .network(&self.network)
            .map(|n| {
                (
                    n.name.clone(),
                    format!("{}:{} · {}", n.host, n.port, n.nick),
                )
            })
            .unwrap_or_default();
        let title = t!("server-window-title", network = name.clone());

        let tab = |id, label, which: Tab, this: &Self, cx: &mut Context<Self>| {
            text_button(
                id,
                label,
                Some(this.tab == which),
                &theme,
                cx.listener(move |this, _, _, cx| this.set_tab(which, cx)),
            )
            .into_any_element()
        };
        let tabs = div()
            .id("server-window-tabs")
            .test_support()
            .role(Role::TabList)
            .aria_label(t!("server-window-tabs"))
            .flex()
            .flex_none()
            .p_0p5()
            .gap_0p5()
            .rounded_md()
            .border_1()
            .border_color(theme.border)
            .bg(theme.sidebar)
            .child(tab(
                "server-window-tab-log",
                t!("server-window-tab-log"),
                Tab::Log,
                self,
                cx,
            ))
            .child(tab(
                "server-window-tab-connection",
                t!("server-window-tab-connection"),
                Tab::Connection,
                self,
                cx,
            ))
            .child(tab(
                "server-window-tab-trace",
                t!("server-window-tab-trace"),
                Tab::Trace,
                self,
                cx,
            ));
        // As the web's: the network's name with its address and your nick,
        // then the tabs under it.
        let header = div()
            .flex_none()
            .px_4()
            .pt_3()
            .pb_2()
            .flex()
            .flex_col()
            .gap_2()
            .border_b_1()
            .border_color(theme.border)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(ui::icon::icon(
                        ui::icon::IconName::SquareTerminal,
                        theme.muted,
                    ))
                    .child(
                        div()
                            .id("server-window-title")
                            .test_support()
                            .role(Role::Heading)
                            .aria_level(1)
                            .aria_label(SharedString::from(title.clone()))
                            .text_lg()
                            .font_weight(FontWeight::BOLD)
                            .text_color(theme.text)
                            .child(name),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_sm()
                            .text_color(theme.muted)
                            .child(address),
                    )
                    .child(ui::icon::button(
                        "server-window-refresh",
                        ui::icon::IconName::RotateCw,
                        t!("server-window-refresh"),
                        &theme,
                        cx.listener(|this, _, _, cx| this.refresh(cx)),
                    )),
            )
            .child(div().flex().child(tabs));

        let body = match self.tab {
            Tab::Connection => div()
                .id("server-window-connection")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .child(connection::details(&self.session, &self.network, cx))
                .into_any_element(),
            Tab::Log => {
                // Drop the workspace's read lock before the log takes it
                // again: a write queued in between would block the second.
                let cards = {
                    let w = client.read();
                    overview(&w, &self.network, &theme).into_any_element()
                };
                let mut content = div().flex_1().min_h_0().min_w_0().flex();
                if self.wide || !self.show_capabilities {
                    content = content.child(self.render_lines(&client, window, cx));
                }
                if self.wide || self.show_capabilities {
                    content = content.child(
                        div()
                            .when(self.wide, |d| d.w(px(300.)).flex_none().border_l_1())
                            .when(!self.wide, |d| d.flex_1().min_w_0())
                            .h_full()
                            .min_h_0()
                            .border_color(theme.border)
                            .child(self.render_capabilities(&client, cx)),
                    );
                }
                let mut body = div().flex_1().min_h_0().flex().flex_col();
                if !self.wide {
                    let mut views = div()
                        .id("server-overview-views")
                        .test_support()
                        .role(Role::TabList)
                        .aria_label(t!("server-window-overview-views"))
                        .flex()
                        .gap_1()
                        .px_4()
                        .py_1();
                    for (id, label, capabilities) in [
                        ("server-overview-log", t!("server-window-log"), false),
                        (
                            "server-overview-capabilities",
                            t!("server-window-capabilities"),
                            true,
                        ),
                    ] {
                        views = views.child(text_button(
                            id,
                            label,
                            Some(self.show_capabilities == capabilities),
                            &theme,
                            cx.listener(move |this, _, _, cx| {
                                this.show_capabilities = capabilities;
                                cx.notify();
                            }),
                        ));
                    }
                    body = body.child(views);
                }
                if self.wide || !self.show_capabilities {
                    body = body.child(cards);
                }
                body.child(content).into_any_element()
            }
            Tab::Trace => self.render_lines(&client, window, cx),
        };

        let current_wide = self.wide;
        let view = cx.entity().downgrade();
        let measure = canvas(
            move |bounds, _, cx| {
                let wide = bounds.size.width > WIDE_SERVER_PANE;
                if wide != current_wide {
                    let view = view.clone();
                    cx.defer(move |cx| {
                        let _ = view.update(cx, |this, cx| {
                            if this.wide != wide {
                                this.wide = wide;
                                cx.notify();
                            }
                        });
                    });
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full();
        div()
            .id("server-window")
            .test_support()
            .role(Role::Region)
            .aria_label(SharedString::from(title))
            .flex_1()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .relative()
            .bg(theme.main)
            .child(measure)
            .child(header)
            .child(body)
            .into_any_element()
    }
}

impl ServerWindowView {
    fn render_capabilities(
        &mut self,
        client: &std::rc::Rc<ircord_client::Client>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = theme::get(cx).clone();
        let query = self
            .capability_filter
            .read(cx)
            .value()
            .trim()
            .to_lowercase();
        let w = client.read();
        let Some(net) = w.network(&self.network) else {
            return div().into_any_element();
        };
        let connected = net.status == NetworkStatus::Connected;
        let info = w
            .server_log
            .connections
            .get(&self.network)
            .and_then(|c| c.value.as_ref());
        let rows = info.map(connection::capability_rows).unwrap_or_default();
        let active = rows.iter().filter(|row| row.2).count();
        let mut content = div().flex().flex_col().gap_1();
        let group = |id, title: &'static str, index: usize, open: bool, cx: &mut Context<Self>| {
            div()
                .id(id)
                .test_support()
                .role(Role::Button)
                .reachable(&theme)
                .aria_label(title)
                .aria_expanded(open)
                .cursor_pointer()
                .py_1()
                .text_sm()
                .font_weight(FontWeight::BOLD)
                .text_color(theme.text)
                .flex()
                .items_center()
                .gap_1()
                .child(ui::icon::icon(
                    if open {
                        ui::icon::IconName::ChevronDown
                    } else {
                        ui::icon::IconName::ChevronRight
                    },
                    theme.muted,
                ))
                .child(title)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.capability_groups[index] = !this.capability_groups[index];
                    cx.notify();
                }))
        };
        content = content.child(group(
            "server-capabilities-group",
            t!("server-window-capabilities"),
            0,
            self.capability_groups[0],
            cx,
        ));
        if self.capability_groups[0] {
            content = content.child(div().text_xs().text_color(theme.muted).child(t!(
                "server-window-capability-count",
                active = active,
                total = rows.len()
            )));
            if !connected && info.is_some() {
                content = content.child(
                    div()
                        .text_xs()
                        .text_color(theme.muted)
                        .child(t!("server-window-capabilities-last")),
                );
            }
            let mut shown = 0;
            for (name, value, on) in rows {
                let description = capability_description(&name);
                if (self.capability_scope == CapabilityScope::Active && !on)
                    || (self.capability_scope == CapabilityScope::Offered && on)
                    || !format!("{name} {value} {description}")
                        .to_lowercase()
                        .contains(&query)
                {
                    continue;
                }
                shown += 1;
                let state = if on {
                    if connected {
                        t!("server-window-capability-active")
                    } else {
                        t!("server-window-capability-last-used")
                    }
                } else {
                    t!("server-window-capability-offered")
                };
                content = content.child(
                    div()
                        .id(ElementId::Name(format!("server-capability-{name}").into()))
                        .test_support()
                        .role(Role::ListItem)
                        .aria_label(SharedString::from(format!(
                            "{name} {state} {value} {description}"
                        )))
                        .py_1()
                        .text_sm()
                        .text_color(theme.text)
                        .child(
                            div().flex().justify_between().gap_2().child(name).child(
                                div()
                                    .text_xs()
                                    .text_color(if on && connected {
                                        theme.online
                                    } else {
                                        theme.muted
                                    })
                                    .child(state),
                            ),
                        )
                        .when(!description.is_empty(), |d| {
                            d.child(div().text_xs().text_color(theme.muted).child(description))
                        })
                        .when(!value.is_empty(), |d| {
                            d.child(div().text_xs().text_color(theme.muted).child(value))
                        }),
                );
            }
            if shown == 0 {
                content = content.child(div().text_sm().text_color(theme.muted).child(
                    if info.is_none() {
                        t!("server-window-capabilities-later")
                    } else {
                        t!("server-window-capability-no-matches")
                    },
                ));
            }
        }
        content = content.child(group(
            "server-extensions-group",
            t!("server-window-extensions"),
            1,
            self.capability_groups[1],
            cx,
        ));
        let edit_description = if !connected {
            t!("server-window-edit-reconnect")
        } else if net.features.edit {
            t!("server-window-edit-available")
        } else {
            t!("server-window-edit-requirements")
        };
        if self.capability_groups[1]
            && format!(
                "{} +ircord/edit {edit_description}",
                t!("server-window-message-editing")
            )
            .to_lowercase()
            .contains(&query)
        {
            content = content.child(
                div()
                    .text_sm()
                    .text_color(theme.text)
                    .child(t!("server-window-message-editing"))
                    .child(div().text_xs().text_color(theme.muted).child(
                        if connected && net.features.edit {
                            t!("server-window-available")
                        } else {
                            t!("server-window-unavailable")
                        },
                    ))
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted)
                            .child("+ircord/edit"),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted)
                            .child(edit_description),
                    ),
            );
        }
        content = content.child(group(
            "server-isupport-group",
            t!("server-window-isupport"),
            2,
            self.capability_groups[2],
            cx,
        ));
        if self.capability_groups[2]
            && let Some(info) = info
        {
            let mut settings: Vec<_> = info.isupport.iter().collect();
            settings.sort_by_key(|(key, _)| *key);
            for (key, value) in settings {
                if format!("{key} {value}").to_lowercase().contains(&query) {
                    content =
                        content.child(div().text_sm().text_color(theme.muted).child(format!(
                            "{}: {}",
                            key,
                            if value.is_empty() {
                                t!("server-window-enabled")
                            } else {
                                value.as_str()
                            }
                        )));
                }
            }
        }
        drop(w);
        let mut filters = div()
            .id("server-capability-scopes")
            .test_support()
            .role(Role::TabList)
            .aria_label(t!("server-window-capability-status"))
            .flex()
            .flex_wrap()
            .gap_1();
        for (id, label, scope) in [
            (
                "server-capability-all",
                t!("server-window-capability-all"),
                CapabilityScope::All,
            ),
            (
                "server-capability-active",
                if connected {
                    t!("server-window-capability-active")
                } else {
                    t!("server-window-capability-last-used")
                },
                CapabilityScope::Active,
            ),
            (
                "server-capability-offered",
                t!("server-window-capability-offered"),
                CapabilityScope::Offered,
            ),
        ] {
            filters = filters.child(text_button(
                id,
                label,
                Some(self.capability_scope == scope),
                &theme,
                cx.listener(move |this, _, _, cx| {
                    this.capability_scope = scope;
                    cx.notify();
                }),
            ));
        }
        div()
            .id("server-capabilities-panel")
            .test_support()
            .role(Role::Region)
            .aria_label(t!("server-window-capabilities"))
            .size_full()
            .flex()
            .flex_col()
            .p_2()
            .gap_2()
            .child(
                Input::new(&self.capability_filter)
                    .id("server-capability-filter")
                    .aria_label(t!("server-window-capability-filter")),
            )
            .child(filters)
            .child(
                div()
                    .id("server-capabilities-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(content),
            )
            .into_any_element()
    }
    fn render_lines(
        &mut self,
        client: &std::rc::Rc<ircord_client::Client>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = theme::get(cx).clone();
        let buffer = if self.tab == Tab::Trace {
            Buffer::Trace
        } else {
            Buffer::Log
        };
        let query = self.filter.read(cx).value().to_string();
        let w = client.read();
        let log = w.server_log.log(&self.network, buffer);
        let lines: &[ServerLine] = log.map(|l| l.lines.as_slice()).unwrap_or(&[]);
        let older = log.is_some_and(|l| l.older.is_some());
        let loading = log.is_none_or(|l| l.loading);
        let error = log.and_then(|l| l.error.clone());

        let mut rows: Vec<AnyElement> = Vec::new();
        let filtering = !query.trim().is_empty();
        let mut shown = 0;
        if filtering || buffer == Buffer::Trace {
            for line in lines.iter().filter(|l| matches(l, &query)) {
                rows.push(line_row(line, false, &theme).into_any_element());
                shown += 1;
            }
        } else {
            for item in fold(lines, MIN_GROUP) {
                match item {
                    Item::Line(line) => rows.push(line_row(line, false, &theme).into_any_element()),
                    Item::Group { id, family, lines } => {
                        let open = self.opened.contains(&id);
                        let first = time_of(lines[0], "%H:%M");
                        let last = time_of(lines[lines.len() - 1], "%H:%M");
                        let span = if first == last {
                            first.clone()
                        } else {
                            format!("{first}–{last}")
                        };
                        let text = format!("{} · {span}", family_text(family, lines.len()));
                        let toggle = id.clone();
                        rows.push(
                            div()
                                .id(ElementId::Name(format!("server-{id}").into()))
                                .test_support()
                                .role(Role::Button)
                                .reachable(&theme)
                                .aria_label(SharedString::from(text.clone()))
                                .aria_expanded(open)
                                .px_4()
                                .py_0p5()
                                .flex()
                                .gap_2()
                                .text_sm()
                                .cursor_pointer()
                                .text_color(theme.muted)
                                .hover(|d| d.bg(theme.row_hover))
                                .font_family(theme.mono_font.clone())
                                .child(time_cell(time_of(lines[0], "%H:%M"), &theme))
                                .child(command_cell(format!("×{}", lines.len()), theme.muted))
                                .child(
                                    div()
                                        .flex_1()
                                        .flex()
                                        .items_center()
                                        .gap_1()
                                        .child(ui::icon::icon(
                                            if open {
                                                ui::icon::IconName::ChevronDown
                                            } else {
                                                ui::icon::IconName::ChevronRight
                                            },
                                            theme.faint,
                                        ))
                                        .child(text),
                                )
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.toggle(toggle.clone(), cx)
                                }))
                                .into_any_element(),
                        );
                        if open {
                            for line in &lines {
                                rows.push(line_row(line, true, &theme).into_any_element());
                            }
                        }
                    }
                }
                shown += 1;
            }
        }
        let count = lines.len();
        drop(w);
        self.follow(count, window, cx);

        let toolbar = div()
            .flex_none()
            .px_4()
            .py_2()
            .flex()
            .items_center()
            .gap_2()
            .child(
                div().flex_1().child(
                    Input::new(&self.filter)
                        .id("server-window-filter")
                        .aria_label(t!("server-window-filter")),
                ),
            )
            .child(
                div()
                    .id("server-window-count")
                    .test_support()
                    .aria_label(SharedString::from(t!(
                        "server-window-line-count",
                        count = count
                    )))
                    .text_xs()
                    .text_color(theme.muted)
                    .child(t!("server-window-line-count", count = count)),
            );

        let mut list = div()
            .id("server-window-lines")
            .test_support()
            .role(Role::Log)
            .aria_label(if buffer == Buffer::Trace {
                t!("server-window-tab-trace")
            } else {
                t!("server-window-log")
            })
            .track_scroll(&self.scroll)
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .py_1();
        if older {
            list = list.child(div().px_4().py_1().child(text_button(
                "server-window-older",
                t!("server-window-older"),
                None,
                &theme,
                cx.listener(|this, _, _, cx| this.load_older(cx)),
            )));
        }
        if let Some(error) = error {
            list = list.child(
                div()
                    .id("server-window-error")
                    .test_support()
                    .role(Role::Alert)
                    .aria_label(SharedString::from(error.clone()))
                    .px_4()
                    .py_1()
                    .text_sm()
                    .text_color(theme.danger)
                    .child(error),
            );
        }
        if shown == 0 && !loading {
            let empty = if filtering {
                t!("server-window-no-match", query = query.trim())
            } else if buffer == Buffer::Trace {
                t!("server-window-trace-empty").to_string()
            } else {
                t!("server-window-empty").to_string()
            };
            list = list.child(ui::empty::empty("server-window-empty", empty, None, &theme));
        }
        list = list.children(rows);

        let mut column = div()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .flex()
            .flex_col()
            .child(toolbar)
            .child(list);
        if buffer == Buffer::Log {
            column = column.child(
                div()
                    .flex_none()
                    .px_4()
                    .py_3()
                    .border_t_1()
                    .border_color(theme.rail)
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div().flex_1().min_w_0().child(
                            Input::new(&self.raw)
                                .id("server-window-raw")
                                .aria_label(t!("server-window-raw-label")),
                        ),
                    )
                    .child(text_button(
                        "server-window-send",
                        t!("server-window-send"),
                        None,
                        &theme,
                        cx.listener(|this, _, window, cx| this.send(window, cx)),
                    )),
            );
        }
        column.into_any_element()
    }
}

// ---- The overview's cards ----

/// A card with a small icon and title over its body. The caller ends it
/// with `.test_support()`.
fn card(
    id: &'static str,
    icon: ui::icon::IconName,
    title: &'static str,
    theme: &Theme,
) -> Stateful<Div> {
    div()
        .id(id)
        .role(Role::Region)
        .aria_label(title)
        .flex_1()
        .min_w_0()
        .p_3()
        .flex()
        .flex_col()
        .gap_1()
        .rounded(theme.radius_large)
        .border_1()
        .border_color(theme.border)
        .bg(theme.sidebar)
        .child(
            div()
                .flex()
                .items_center()
                .gap_1p5()
                .text_sm()
                .text_color(theme.muted)
                .child(ui::icon::icon(icon, theme.muted))
                .child(title),
        )
}

fn minutes_since(time: &str, now_ms: i64) -> Option<i64> {
    let at = chrono::DateTime::parse_from_rfc3339(time).ok()?;
    Some(((now_ms - at.timestamp_millis()) / 60_000).max(0))
}

/// Status, Latency, Security and Session for `network`, two by two.
fn overview(
    w: &ircord_client::store::Workspace,
    network: &str,
    theme: &Theme,
) -> impl IntoElement + use<> {
    let now = chrono::Utc::now().timestamp_millis();
    let n = w.network(network);
    let info = w
        .server_log
        .connections
        .get(network)
        .and_then(|c| c.value.as_ref());

    // Status: the indicator's words and dot, and for how long.
    let (status, health) = connection::status(w, Some(network));
    let dot = match health {
        connection::Health::Good => theme.online,
        connection::Health::Degraded => theme.warning,
        connection::Health::Offline => theme.offline,
    };
    let server = info.map(|i| i.server_name.clone()).unwrap_or_default();
    let since = info
        .and_then(|i| i.connected_at.as_deref())
        .filter(|_| health == connection::Health::Good)
        .and_then(|t| minutes_since(t, now));
    let status_line = match since {
        Some(minutes) => t!(
            "server-window-for-minutes",
            minutes = minutes,
            server = server.clone()
        ),
        None => server.clone(),
    };
    let status_card = card(
        "server-window-card-status",
        ui::icon::IconName::Info,
        t!("server-window-status"),
        theme,
    )
    .child(
        div()
            .id("server-window-status-value")
            .test_support()
            .aria_label(status)
            .flex()
            .items_center()
            .gap_2()
            .text_lg()
            .font_weight(FontWeight::BOLD)
            .text_color(theme.text)
            .child(div().size(px(8.)).rounded_full().bg(dot))
            .child(status),
    )
    .child(div().text_sm().text_color(theme.muted).child(status_line));

    // Latency: both hops added up, with the last half hour's average and peak.
    let device = w.server_log.device_latency(now);
    let irc = w.server_log.irc_latency(network, now);
    let total = match (device, irc) {
        (Some(a), Some(b)) => Some(a + b),
        (a, b) => a.or(b),
    };
    let totals = w.server_log.totals(network);
    let spread = server_log::average_and_peak(totals.iter().map(|s| s.ms));
    let ms = |v: Option<f64>| match v {
        Some(v) => t!("server-window-ms", value = v.round()),
        None => t!("server-window-no-value").to_string(),
    };
    let total_text = ms(total);
    let mut latency_card = card(
        "server-window-card-latency",
        ui::icon::IconName::Loader,
        t!("server-window-latency"),
        theme,
    )
    .child(
        div()
            .flex()
            .items_center()
            .gap_1()
            .text_xs()
            .text_color(theme.muted)
            .child(t!("server-window-you"))
            .child(format!("— {} —", ms(device)))
            .child("IRCord")
            .child(format!("— {} —", ms(irc)))
            .child(server.clone()),
    )
    .child(
        div()
            .id("server-window-latency-total")
            .test_support()
            .aria_label(SharedString::from(total_text.clone()))
            .flex()
            .items_baseline()
            .gap_1()
            .child(
                div()
                    .text_lg()
                    .font_weight(FontWeight::BOLD)
                    .text_color(theme.text)
                    .child(total_text),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(theme.muted)
                    .child(t!("server-window-total-word")),
            ),
    );
    if let Some((average, peak)) = spread {
        latency_card = latency_card.child(div().text_xs().text_color(theme.muted).child(t!(
            "server-window-average-peak",
            average = ms(Some(average)),
            peak = ms(Some(peak))
        )));
    }

    // Security: the grade as a letter tile, and whether traffic is encrypted.
    let tls = info.and_then(|i| i.tls.as_ref());
    let (grade, headline, detail) = match (n.is_some_and(|n| n.tls), tls) {
        (_, Some(tls)) => (
            connection::grade_text(tls.grade).to_string(),
            t!("server-window-encrypted").to_string(),
            tls.version.clone(),
        ),
        (true, None) => (
            "?".to_string(),
            t!("server-window-encrypted").to_string(),
            t!("server-window-details-later").to_string(),
        ),
        (false, None) => (
            "F".to_string(),
            t!("server-window-unencrypted").to_string(),
            t!("server-window-readable").to_string(),
        ),
    };
    let grade_colour = if grade.starts_with('A') {
        theme.online
    } else if grade.starts_with('B') || grade == "?" {
        theme.warning
    } else {
        theme.danger
    };
    let security_card = card(
        "server-window-card-security",
        ui::icon::IconName::Eye,
        t!("server-window-security"),
        theme,
    )
    .child(
        div()
            .flex()
            .items_center()
            .gap_3()
            .child(
                div()
                    .id("server-window-grade")
                    .test_support()
                    .aria_label(SharedString::from(t!(
                        "server-window-grade",
                        grade = grade.clone()
                    )))
                    .size(px(40.))
                    .flex_none()
                    .rounded(theme.radius)
                    .bg(grade_colour.opacity(0.15))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_lg()
                    .font_weight(FontWeight::BOLD)
                    .text_color(grade_colour)
                    .child(grade),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.text)
                            .child(headline),
                    )
                    .child(div().text_sm().text_color(theme.muted).child(detail)),
            ),
    );

    // Session: your nick here, the address, and how much was negotiated.
    let nick = n.map(|n| n.nick.clone()).unwrap_or_default();
    let address = n
        .map(|n| format!("{}:{}", n.host, n.port))
        .unwrap_or_default();
    let session_line = t!(
        "server-window-session-line",
        address = address,
        capabilities = info.map_or(0, |i| i.negotiated_capabilities.len()),
        settings = info.map_or(0, |i| i.isupport.len())
    );
    let session_card = card(
        "server-window-card-session",
        ui::icon::IconName::User,
        t!("server-window-session"),
        theme,
    )
    .child(
        div()
            .text_lg()
            .font_weight(FontWeight::BOLD)
            .text_color(theme.text)
            .child(nick),
    )
    .child(
        div()
            .text_sm()
            .font_family(theme.mono_font.clone())
            .text_color(theme.muted)
            .child(session_line),
    );

    div()
        .id("server-window-overview")
        .test_support()
        .role(Role::Group)
        .aria_label(t!("server-window-overview"))
        .flex_none()
        .px_4()
        .pt_3()
        .flex()
        .flex_col()
        .gap_3()
        .child(
            div()
                .flex()
                .gap_3()
                .child(status_card.test_support())
                .child(latency_card.test_support()),
        )
        .child(
            div()
                .flex()
                .gap_3()
                .child(security_card.test_support())
                .child(session_card.test_support()),
        )
}

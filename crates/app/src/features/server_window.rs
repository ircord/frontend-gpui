//! A network's server window and raw input (package SV1).
//!
//! Three tabs over one network: the server log (numerics, notices, errors,
//! the MOTD and what you sent, with runs of read markers, presence, away
//! changes, pings and history folded into one row), the connection details
//! (`features::connection`), and the connection trace (every line on the
//! wire, secrets redacted). Under the log, a box sends one raw IRC line.
//!
//! Opened from the network menu, the connection indicator and `/server`
//! (and after commands whose answer lands here, such as `/whois`).

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
            .order(200),
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

pub struct ServerWindowView {
    session: Entity<Session>,
    network: String,
    tab: Tab,
    filter: Entity<InputState>,
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
        let raw = cx
            .new(|cx| InputState::new(window, cx).placeholder(t!("server-window-raw-placeholder")));
        let id = network.clone();
        let subscriptions = vec![
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
            Tab::Log => Buffer::Log,
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
            Tab::Log => server_log::load(client.handle(), &self.network, Buffer::Log),
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
        .font_family("monospace")
        .hover(|d| d.bg(theme.row_hover))
        .child(
            div()
                .flex_none()
                .w(px(64.))
                .text_color(theme.faint)
                .child(time_of(line, "%H:%M:%S")),
        )
        .child(
            div()
                .flex_none()
                .w(px(110.))
                .truncate()
                .text_color(kind_colour(line.kind, theme))
                .child(label),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_color(if line.kind == ServerLineKind::Error {
                    theme.danger
                } else {
                    theme.text
                })
                .child(text),
        )
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
    button.test_support()
}

impl Render for ServerWindowView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme::get(cx).clone();
        let Some(client) = self.session.read(cx).client().cloned() else {
            return div().into_any_element();
        };
        let name = client
            .read()
            .network(&self.network)
            .map(|n| n.name.clone())
            .unwrap_or_default();
        let title = t!("server-window-title", network = name);

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
            .gap_1()
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
            ))
            .child(ui::icon::button(
                "server-window-refresh",
                ui::icon::IconName::Redo,
                t!("server-window-refresh"),
                &theme,
                cx.listener(|this, _, _, cx| this.refresh(cx)),
            ));
        let header = div()
            .h(px(48.))
            .flex_none()
            .px_4()
            .flex()
            .items_center()
            .justify_between()
            .border_b_1()
            .border_color(theme.rail)
            .child(
                div()
                    .font_weight(FontWeight::BOLD)
                    .text_color(theme.text)
                    .child(title.clone()),
            )
            .child(tabs);

        let body = match self.tab {
            Tab::Connection => div()
                .id("server-window-connection")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .child(connection::details(&self.session, &self.network, cx))
                .into_any_element(),
            Tab::Log | Tab::Trace => self.render_lines(&client, window, cx),
        };

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
            .bg(theme.main)
            .child(header)
            .child(body)
            .into_any_element()
    }
}

impl ServerWindowView {
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
                                .child(
                                    div()
                                        .flex_none()
                                        .w(px(64.))
                                        .text_color(theme.faint)
                                        .child(time_of(lines[0], "%H:%M:%S")),
                                )
                                .child(div().flex_none().w(px(110.)).child(format!(
                                    "{} ×{}",
                                    if open { "▾" } else { "▸" },
                                    lines.len()
                                )))
                                .child(div().flex_1().child(text))
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
                t!("server-window-tab-log")
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
                        div().flex_1().child(
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

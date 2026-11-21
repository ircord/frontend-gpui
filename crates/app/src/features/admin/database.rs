//! Administration → Database, after `AdminDatabase.vue`: connection
//! pressure, storage and configuration, sampled again every ten seconds
//! while the section shows (unless that is turned off). The recent waits
//! come from the difference between two samples.

use std::time::Duration;

use gpui_kit::component::switch::Switch;
use gpui_kit::prelude::*;
use gpui_kit::*;
use ircord_client::api::admin as a;
use ircord_client::contract as c;

use super::{Kind, button, client, error_text, group, hint, notice, tile, tiles, when};
use crate::session::Session;
use crate::theme;

/// How often the live sample is read again.
const EVERY: Duration = Duration::from_secs(10);

pub struct DatabasePanel {
    session: Entity<Session>,
    sample: Option<c::DatabaseStatus>,
    /// Waits per minute and milliseconds waited since the previous sample.
    recent: Option<(f64, f64)>,
    error: Option<String>,
    loading: bool,
    live: bool,
    _ticker: Task<()>,
}

impl DatabasePanel {
    pub fn new(session: Entity<Session>, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        let ticker = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(EVERY).await;
                let alive = this.update(cx, |this, cx| {
                    let showing = super::showing(&this.session, cx)
                        && cx
                            .try_global::<super::Page>()
                            .is_some_and(|p| p.0.read(cx).section == super::Section::Database);
                    if this.live && showing {
                        this.load(cx);
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        });
        let mut panel = Self {
            session,
            sample: None,
            recent: None,
            error: None,
            loading: false,
            live: true,
            _ticker: ticker,
        };
        panel.load(cx);
        panel
    }

    pub fn load(&mut self, cx: &mut Context<Self>) {
        if self.loading {
            return;
        }
        let Some(client) = client(&self.session, cx) else {
            return;
        };
        self.loading = true;
        let task = client.request(|api| async move { api.administration_database().await });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                this.loading = false;
                match result {
                    Ok(Ok(sample)) => {
                        this.recent = this
                            .sample
                            .as_ref()
                            .and_then(|previous| a::recent_waits(previous, &sample));
                        this.sample = Some(sample);
                        this.error = None;
                    }
                    Ok(Err(e)) => {
                        super::refused(&client, &e, cx);
                        this.error = Some(error_text(&e));
                    }
                    Err(e) => this.error = Some(e.to_string()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }
}

fn ms(value: f64) -> String {
    t!("admin-db-milliseconds", value = a::decimal(value, 1))
}

fn bytes(value: Option<i64>) -> String {
    value
        .map(a::database_bytes)
        .unwrap_or_else(|| t!("admin-db-not-measured").to_string())
}

fn facts(
    prefix: &str,
    rows: Vec<(&'static str, &'static str, String)>,
    theme: &crate::theme::Theme,
) -> Div {
    tiles(rows.into_iter().map(|(key, label, value)| {
        tile(
            format!("admin-db-{prefix}-{key}"),
            label,
            value,
            None,
            theme,
        )
    }))
}

impl Render for DatabasePanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme::get(cx).clone();
        let top = div()
            .flex()
            .flex_wrap()
            .items_center()
            .justify_between()
            .gap_3()
            .child(
                div()
                    .max_w(px(520.))
                    .child(hint(t!("admin-db-intro"), &theme)),
            )
            .child(button(
                "admin-db-refresh",
                t!("admin-refresh"),
                None,
                Kind::Secondary,
                !self.loading,
                &theme,
                cx.listener(|this, _, _, cx| this.load(cx)),
            ));
        let live = div()
            .flex()
            .flex_wrap()
            .items_center()
            .justify_between()
            .gap_3()
            .text_sm()
            .text_color(theme.muted)
            .child(
                self.sample
                    .as_ref()
                    .map(|s| t!("admin-db-sampled", time = when(&s.sampled_at)))
                    .unwrap_or_default(),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        Switch::new("admin-db-live")
                            .checked(self.live)
                            .accessibility_label(t!("admin-db-live"))
                            .on_click(cx.listener(|this, on: &bool, _, cx| {
                                this.live = *on;
                                cx.notify();
                            })),
                    )
                    .child(t!("admin-db-live")),
            );
        let mut body = div().flex().flex_col().gap_6().child(top).child(live);
        if let Some(error) = &self.error {
            body = body.child(notice("admin-db-error", error.clone(), true, &theme));
        }
        let Some(d) = &self.sample else {
            if self.error.is_none() {
                body = body.child(hint(t!("admin-loading"), &theme));
            }
            return body;
        };
        if !d.available {
            body = body.child(notice(
                "admin-db-unavailable",
                t!("admin-db-unavailable"),
                false,
                &theme,
            ));
        }
        let p = &d.pool;
        let pool = vec![
            ("in-use", t!("admin-db-in-use"), a::count(p.in_use)),
            ("idle", t!("admin-db-idle"), a::count(p.idle)),
            ("open", t!("admin-db-open"), a::count(p.open)),
            (
                "limit",
                t!("admin-db-limit"),
                if p.max_open > 0 {
                    a::count(p.max_open)
                } else {
                    t!("admin-db-unlimited").to_string()
                },
            ),
            ("waits", t!("admin-db-waits"), a::count(p.wait_count)),
            ("wait-time", t!("admin-db-wait-time"), ms(p.wait_total_ms)),
            (
                "recent-waits",
                t!("admin-db-recent-waits"),
                self.recent
                    .map(|(w, _)| a::decimal(w, 1))
                    .unwrap_or_else(|| t!("admin-db-not-measured").to_string()),
            ),
            (
                "recent-wait-time",
                t!("admin-db-recent-wait-time"),
                self.recent
                    .map(|(_, ms_waited)| ms(ms_waited))
                    .unwrap_or_else(|| t!("admin-db-not-measured").to_string()),
            ),
        ];
        body = body.child(group(
            "admin-db-pool",
            t!("admin-db-pool"),
            div()
                .flex()
                .flex_col()
                .gap_2()
                .child(facts("pool", pool, &theme))
                .child(hint(t!("admin-db-wait-help"), &theme))
                .child(
                    div()
                        .text_sm()
                        .text_color(theme.faint)
                        .child(t!("admin-db-since", time = when(&d.started_at))),
                ),
            &theme,
        ));
        if d.available {
            let storage = vec![
                (
                    "allocated",
                    t!("admin-db-allocated"),
                    a::database_bytes(d.allocated_bytes),
                ),
                (
                    "reusable",
                    t!("admin-db-reusable"),
                    a::database_bytes(d.reusable_bytes),
                ),
                ("file", t!("admin-db-file"), bytes(d.file_bytes)),
                ("wal", t!("admin-db-wal"), bytes(d.wal_bytes)),
            ];
            body = body.child(group(
                "admin-db-storage",
                t!("admin-db-storage"),
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(facts("storage", storage, &theme))
                    .child(hint(t!("admin-db-storage-help"), &theme)),
                &theme,
            ));
            let unknown = || t!("admin-db-not-measured").to_string();
            let config = vec![
                (
                    "engine",
                    t!("admin-db-engine"),
                    format!("SQLite {}", d.version.as_deref().unwrap_or_default())
                        .trim()
                        .to_string(),
                ),
                (
                    "journal",
                    t!("admin-db-journal"),
                    d.journal_mode
                        .as_deref()
                        .map(str::to_uppercase)
                        .unwrap_or_else(unknown),
                ),
                (
                    "durability",
                    t!("admin-db-durability"),
                    d.synchronous
                        .as_deref()
                        .map(str::to_uppercase)
                        .unwrap_or_else(unknown),
                ),
                (
                    "foreign-keys",
                    t!("admin-db-foreign-keys"),
                    if d.foreign_keys {
                        t!("admin-db-enabled")
                    } else {
                        t!("admin-db-disabled")
                    }
                    .to_string(),
                ),
                (
                    "busy-timeout",
                    t!("admin-db-busy-timeout"),
                    ms(d.busy_timeout_ms as f64),
                ),
                (
                    "checkpoint",
                    t!("admin-db-checkpoint"),
                    t!("admin-db-pages", count = a::count(d.auto_checkpoint_pages)),
                ),
            ];
            body = body.child(group(
                "admin-db-configuration",
                t!("admin-db-configuration"),
                facts("config", config, &theme),
                &theme,
            ));
        }
        body
    }
}

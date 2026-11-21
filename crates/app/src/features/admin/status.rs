//! Administration → Overview, after `AdminStatus.vue`: is the backend
//! healthy, what is it running and how much is it carrying, kept apart from
//! the associated IRC server's counts; then the recent Administration
//! actions.

use gpui_kit::prelude::*;
use gpui_kit::*;
use ircord_client::api::admin::{self as a, AUDIT_LIMIT};
use ircord_client::contract as c;

use super::{Kind, button, client, error_text, group, hint, notice, tile, tiles, uptime, when};
use crate::session::Session;
use crate::theme;

pub struct StatusPanel {
    session: Entity<Session>,
    status: Option<c::Status>,
    status_error: Option<String>,
    audit: Option<Vec<c::OwnerEvent>>,
    audit_error: Option<String>,
    loading: bool,
    generation: u64,
}

impl StatusPanel {
    pub fn new(session: Entity<Session>, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut panel = Self {
            session,
            status: None,
            status_error: None,
            audit: None,
            audit_error: None,
            loading: false,
            generation: 0,
        };
        panel.load(cx);
        panel
    }

    pub fn load(&mut self, cx: &mut Context<Self>) {
        let Some(client) = client(&self.session, cx) else {
            return;
        };
        self.generation += 1;
        let generation = self.generation;
        self.loading = true;
        let status = client.request(|api| async move { api.administration_status().await });
        let audit =
            client.request(|api| async move { api.administration_audit(AUDIT_LIMIT).await });
        cx.spawn(async move |this, cx| {
            let status = status.await;
            let audit = audit.await;
            this.update(cx, |this, cx| {
                if generation != this.generation {
                    return;
                }
                this.loading = false;
                match status {
                    Ok(Ok(s)) => {
                        this.status = Some(s);
                        this.status_error = None;
                    }
                    Ok(Err(e)) => {
                        super::refused(&client, &e, cx);
                        this.status_error = Some(error_text(&e));
                    }
                    Err(e) => this.status_error = Some(e.to_string()),
                }
                match audit {
                    Ok(Ok(page)) => {
                        this.audit = Some(page.items);
                        this.audit_error = None;
                    }
                    Ok(Err(e)) => this.audit_error = Some(error_text(&e)),
                    Err(e) => this.audit_error = Some(e.to_string()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }
}

fn action(event: &c::OwnerEvent) -> String {
    match event.action.as_str() {
        "owner-grant" => t!("admin-audit-made-owner"),
        "owner-revoke" => t!("admin-audit-removed-owner"),
        "message-delete" => t!("admin-audit-deleted-message"),
        "messages-delete-recent" => t!("admin-audit-deleted-recent"),
        "file-delete" => t!("admin-audit-deleted-file"),
        "bot-mark" => t!("admin-audit-marked-bot"),
        "bot-unmark" => t!("admin-audit-unmarked-bot"),
        "channel-guide-save" => t!("admin-audit-saved-guide"),
        "channel-guide-decline" => t!("admin-audit-declined-listing"),
        "emoji-management" => t!("admin-audit-emoji-management"),
        other => return other.to_string(),
    }
    .to_string()
}

/// "alice bob made an owner", or "alice tried: bob made an owner (refused)".
pub(super) fn audit_line(event: &c::OwnerEvent) -> String {
    let what = action(event);
    let (actor, target, outcome) = (
        event.actor.as_str(),
        event.target.as_str(),
        event.outcome.as_str(),
    );
    let mut line = match (a::audit_failed(event), target.is_empty()) {
        (true, false) => t!(
            "admin-audit-failed",
            actor = actor,
            target = target,
            action = what.as_str(),
            outcome = outcome
        ),
        (true, true) => t!(
            "admin-audit-failed-untargeted",
            actor = actor,
            action = what.as_str(),
            outcome = outcome
        ),
        (false, false) => t!(
            "admin-audit-success",
            actor = actor,
            target = target,
            action = what.as_str()
        ),
        (false, true) => t!(
            "admin-audit-success-untargeted",
            actor = actor,
            action = what.as_str()
        ),
    };
    // How many were deleted, or who manages emoji now.
    if a::audit_counts(event) || (event.action == "emoji-management" && !a::audit_failed(event)) {
        line.push_str(&format!(" ({})", event.outcome));
    }
    if let Some(reason) = event.reason.as_deref().filter(|r| !r.is_empty()) {
        line.push_str(" — ");
        line.push_str(reason);
    }
    line
}

/// "21 Nov 2026 15:23", in local time.
fn short_time(at: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(at)
        .map(|t| {
            t.with_timezone(&chrono::Local)
                .format("%-d %b %Y %H:%M")
                .to_string()
        })
        .unwrap_or_default()
}

impl Render for StatusPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme::get(cx).clone();
        let top = div()
            .flex()
            .flex_wrap()
            .items_center()
            .justify_between()
            .gap_3()
            .child(hint(t!("admin-overview-help"), &theme))
            .child(button(
                "admin-status-refresh",
                t!("admin-refresh"),
                None,
                Kind::Secondary,
                !self.loading,
                &theme,
                cx.listener(|this, _, _, cx| this.load(cx)),
            ));
        let mut body = div().flex().flex_col().gap_6().child(top);
        if let Some(error) = &self.status_error {
            body = body.child(notice("admin-status-error", error.clone(), true, &theme));
        }
        let Some(s) = &self.status else {
            if self.status_error.is_none() {
                body = body.child(hint(t!("admin-loading"), &theme));
            }
            return body;
        };

        let healthy = s.status == c::StatusStatus::Ok;
        let version = if s.commit.is_empty() {
            s.version.clone()
        } else {
            format!("{} ({})", s.version, &s.commit[..s.commit.len().min(7)])
        };
        body = body.child(group(
            "admin-backend",
            t!("admin-backend"),
            tiles([
                tile(
                    "admin-health".into(),
                    t!("admin-health"),
                    if healthy {
                        t!("admin-healthy")
                    } else {
                        t!("admin-degraded")
                    }
                    .into(),
                    None,
                    &theme,
                ),
                tile(
                    "admin-database-state".into(),
                    t!("admin-section-database"),
                    if s.database == c::StatusDatabase::Available {
                        t!("admin-available")
                    } else {
                        t!("admin-unavailable")
                    }
                    .into(),
                    None,
                    &theme,
                ),
                tile(
                    "admin-version".into(),
                    t!("admin-version"),
                    version,
                    None,
                    &theme,
                ),
                tile(
                    "admin-uptime".into(),
                    t!("admin-uptime"),
                    uptime(s.uptime_seconds),
                    Some(t!("admin-since", time = when(&s.started_at))),
                    &theme,
                ),
            ]),
            &theme,
        ));

        if let Some(o) = &s.overview {
            let rows = [
                ("accounts", t!("admin-accounts"), a::count(o.accounts)),
                ("sessions", t!("admin-sessions"), a::count(o.sessions)),
                ("networks", t!("admin-networks"), a::count(o.networks)),
                (
                    "connections",
                    t!("admin-connections"),
                    a::count(o.irc_connections),
                ),
                ("messages", t!("admin-messages"), a::count(o.messages)),
                (
                    "size",
                    t!("admin-database-size"),
                    a::overview_bytes(o.database_bytes),
                ),
            ];
            body = body.child(group(
                "admin-ircord",
                t!("admin-ircord"),
                tiles(rows.map(|(key, label, value)| {
                    tile(format!("admin-overview-{key}"), label, value, None, &theme)
                })),
                &theme,
            ));
        }

        let irc = match &s.ergo {
            None => hint(t!("admin-no-irc-server"), &theme).into_any_element(),
            Some(e) if e.state != c::ErgoStatusState::Ok => notice(
                "admin-irc-error",
                t!(
                    "admin-irc-not-answering",
                    server = e.server.as_str(),
                    error = e.error.as_deref().unwrap_or(t!("admin-unavailable-lower"))
                ),
                true,
                &theme,
            )
            .into_any_element(),
            Some(e) => match &e.statistics {
                Some(st) => tiles([
                    tile(
                        "admin-irc-server".into(),
                        t!("admin-server"),
                        e.server.clone(),
                        None,
                        &theme,
                    ),
                    tile(
                        "admin-irc-version".into(),
                        t!("admin-version"),
                        st.version.clone(),
                        None,
                        &theme,
                    ),
                    tile(
                        "admin-irc-users".into(),
                        t!("admin-users-online"),
                        a::count(st.users.total),
                        None,
                        &theme,
                    ),
                    tile(
                        "admin-irc-channels".into(),
                        t!("admin-channels"),
                        a::count(st.channels),
                        None,
                        &theme,
                    ),
                    tile(
                        "admin-irc-uptime".into(),
                        t!("admin-uptime"),
                        uptime(st.uptime_seconds),
                        None,
                        &theme,
                    ),
                ])
                .into_any_element(),
                None => hint(e.server.clone(), &theme).into_any_element(),
            },
        };
        body = body.child(group("admin-irc", t!("admin-irc-server"), irc, &theme));

        let audit = if let Some(error) = &self.audit_error {
            notice("admin-audit-error", error.clone(), true, &theme).into_any_element()
        } else {
            match &self.audit {
                None => hint(t!("admin-loading"), &theme).into_any_element(),
                Some(items) if items.is_empty() => {
                    notice("admin-audit-empty", t!("admin-no-actions"), false, &theme)
                        .into_any_element()
                }
                Some(items) => {
                    let mut list = div()
                        .id("admin-audit")
                        .test_support()
                        .role(Role::List)
                        .aria_label(t!("admin-recent-actions"))
                        .p_3()
                        .rounded_md()
                        .bg(theme.raised)
                        .border_1()
                        .border_color(theme.border)
                        .flex()
                        .flex_col()
                        .gap_1p5()
                        .text_sm();
                    for (i, event) in items.iter().enumerate() {
                        let line = audit_line(event);
                        list = list.child(
                            div()
                                .id(ElementId::Name(format!("admin-audit-{i}").into()))
                                .test_support()
                                .role(Role::ListItem)
                                .aria_label(line.clone())
                                .flex()
                                .gap_2p5()
                                .child(
                                    div()
                                        .flex_none()
                                        .w(px(150.))
                                        .whitespace_nowrap()
                                        .text_color(theme.faint)
                                        .child(short_time(&event.at)),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .text_color(if a::audit_failed(event) {
                                            theme.muted
                                        } else {
                                            theme.text
                                        })
                                        .child(line),
                                ),
                        );
                    }
                    list.into_any_element()
                }
            }
        };
        body.child(group(
            "admin-recent",
            t!("admin-recent-actions"),
            audit,
            &theme,
        ))
    }
}

use gpui_kit::TestSupportExt;

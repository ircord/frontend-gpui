//! Settings → About: what IRCord is, this app's version and the server's,
//! where to report a problem, the people who build IRCord, and the release
//! notes of both (package S0), as the web client's `AboutSettings.vue`. The
//! app's notes are its own `CHANGELOG.md`, built in; the server's come from
//! `/api/v1/changelog`.

use gpui_kit::TestSupportExt;
use gpui_kit::prelude::*;
use gpui_kit::*;
use ircord_client::contract as c;

use super::block;
use crate::registry::{self, SettingsCtx, SettingsGroup, SettingsRow, SettingsSection};
use crate::session::Session;
use crate::theme::{self, Theme};
use crate::ui;

const WEBSITE_URL: &str = "https://ircord.com";
const ISSUES_URL: &str = "https://github.com/ircord/issues/issues/new/choose";
/// This app's release notes.
const CHANGELOG: &str = include_str!("../../../../CHANGELOG.md");
/// The people who build IRCord, as the web client's `COLLABORATORS.md`.
const COLLABORATORS: &[&str] = &["Michael Karbowiak", "Peter Petermann"];

pub fn init(cx: &mut App) {
    registry::register(cx, |r| {
        r.settings_sections.add(
            SettingsSection::new("about", t!("settings-about"), |ctx, window, cx| {
                let view = view(&ctx.session, window, cx);
                view.update(cx, |v, _| v.ctx = Some(ctx.clone()));
                view.into_any_element()
            })
            .group(SettingsGroup::Server)
            .order(100)
            .row(
                SettingsRow::new("about-ircord", t!("settings-about-label"))
                    .description(t!("settings-about-description"))
                    .keywords(t!("settings-about-keywords")),
            ),
        );
    });
}

/// A release: its version, its date (empty when not released) and its notes
/// as Markdown lines.
#[derive(Clone, Debug, PartialEq)]
pub struct Release {
    pub version: String,
    pub date: String,
    pub notes: String,
}

/// The releases in a `CHANGELOG.md`: each `## version` heading (`## 1.2.0 -
/// 2026-11-01`, `## [1.2.0] — 2026-11-01`, `## Unreleased`) and what is under
/// it. Sections with nothing in them are left out.
pub fn releases(changelog: &str) -> Vec<Release> {
    let mut out: Vec<Release> = Vec::new();
    for line in changelog.lines() {
        if let Some(heading) = line.strip_prefix("## ") {
            let heading = heading.trim();
            let (version, date) = match heading.split_once([' ', '\t']) {
                Some((v, rest)) => (v, rest.trim_start_matches([' ', '-', '—', '–']).trim()),
                None => (heading, ""),
            };
            out.push(Release {
                version: version.trim_matches(['[', ']']).to_string(),
                date: date.to_string(),
                notes: String::new(),
            });
        } else if let Some(release) = out.last_mut() {
            release.notes.push_str(line);
            release.notes.push('\n');
        }
    }
    for release in &mut out {
        release.notes = release.notes.trim().to_string();
    }
    out.retain(|r| !r.notes.is_empty());
    out
}

struct About(Entity<AboutView>);
impl Global for About {}

fn view(session: &Entity<Session>, _window: &mut Window, cx: &mut App) -> Entity<AboutView> {
    if let Some(About(view)) = cx.try_global::<About>()
        && view.read(cx).session == *session
    {
        return view.clone();
    }
    let view = cx.new(|cx| {
        let mut view = AboutView {
            session: session.clone(),
            ctx: None,
            server: Server::Loading,
        };
        view.load(cx);
        view
    });
    cx.set_global(About(view.clone()));
    view
}

enum Server {
    Loading,
    Failed(String),
    Ready(c::BackendChangelog),
}

pub struct AboutView {
    session: Entity<Session>,
    ctx: Option<SettingsCtx>,
    server: Server,
}

impl AboutView {
    fn load(&mut self, cx: &mut Context<Self>) {
        let Some(client) = self.session.read(cx).client().cloned() else {
            return;
        };
        let task = client.request(|api| async move { api.changelog().await });
        cx.spawn(async move |this, cx| {
            let result = task
                .await
                .map_err(|e| e.to_string())
                .and_then(|r| r.map_err(|e| e.to_string()));
            let _ = this.update(cx, |this, cx| {
                this.server = match result {
                    Ok(notes) => Server::Ready(notes),
                    Err(e) => Server::Failed(e),
                };
                cx.notify();
            });
        })
        .detach();
    }
}

fn fact(
    id: &'static str,
    label: &'static str,
    value: String,
    theme: &Theme,
) -> impl IntoElement + use<> {
    let aria: SharedString = format!("{label}: {value}").into();
    div()
        .id(id)
        .role(Role::Group)
        .aria_label(aria)
        .min_w(px(150.))
        .flex_1()
        .px_3()
        .py_2()
        .rounded_md()
        .bg(theme.sidebar)
        .child(
            div()
                .text_xs()
                .font_weight(FontWeight::BOLD)
                .text_color(theme.faint)
                .child(label.to_uppercase()),
        )
        .child(div().text_sm().text_color(theme.text).child(value))
        .test_support()
}

fn link(
    id: &'static str,
    label: &'static str,
    url: &'static str,
    theme: &Theme,
) -> impl IntoElement + use<> {
    let hover = theme.hover;
    div()
        .id(id)
        .role(Role::Link)
        .aria_label(label)
        .px_3()
        .py_2()
        .rounded_md()
        .border_1()
        .border_color(theme.selected)
        .cursor_pointer()
        .hover(move |d| d.bg(hover))
        .flex()
        .items_center()
        .gap_2()
        .text_sm()
        .text_color(theme.text)
        .child(label)
        .child(ui::icon::icon(
            ui::icon::IconName::ExternalLink,
            theme.faint,
        ))
        .on_click(move |_, _, cx| cx.open_url(url))
        .test_support()
}

/// Release notes drawn line by line: `###` headings, `-` items, text.
fn notes(id: String, releases: &[Release], theme: &Theme) -> impl IntoElement + use<> {
    let mut list = div()
        .id(ElementId::Name(id.into()))
        .role(Role::List)
        .flex()
        .flex_col()
        .gap_3();
    for release in releases {
        let heading = if release.version.eq_ignore_ascii_case("unreleased") {
            t!("settings-about-unreleased").to_string()
        } else if release.date.is_empty() {
            release.version.clone()
        } else {
            t!(
                "settings-about-release-heading",
                version = release.version,
                date = release.date
            )
        };
        let mut body = div()
            .id(ElementId::Name(
                format!("release-{}", release.version).into(),
            ))
            .role(Role::ListItem)
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.text)
                    .child(heading),
            );
        for line in release.notes.lines() {
            let line = line.trim_end();
            if line.trim().is_empty() {
                continue;
            }
            body = body.child(if let Some(h) = line.trim().strip_prefix('#') {
                div()
                    .pt_1()
                    .text_xs()
                    .font_weight(FontWeight::BOLD)
                    .text_color(theme.faint)
                    .child(h.trim_start_matches('#').trim().to_uppercase())
            } else if let Some(item) = line.trim_start().strip_prefix("- ") {
                div()
                    .flex()
                    .gap_2()
                    .text_sm()
                    .text_color(theme.muted)
                    .child("•")
                    .child(div().flex_1().min_w_0().child(item.to_string()))
            } else {
                div()
                    .text_sm()
                    .text_color(theme.muted)
                    .child(line.trim().to_string())
            });
        }
        list = list.child(body.test_support());
    }
    list.test_support()
}

impl Render for AboutView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme::get(cx).clone();
        let ctx = self.ctx.clone().unwrap_or(SettingsCtx {
            session: self.session.clone(),
            target: None,
        });
        let server_version = match &self.server {
            Server::Ready(notes) if !notes.version.is_empty() => notes.version.clone(),
            _ => t!("settings-about-server-unknown").to_string(),
        };
        let hero = div()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .text_sm()
                    .text_color(theme.muted)
                    .child(t!("settings-about-tagline")),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .child(fact(
                        "about-app-version",
                        t!("settings-about-app-version"),
                        env!("CARGO_PKG_VERSION").to_string(),
                        &theme,
                    ))
                    .child(fact(
                        "about-server-version",
                        t!("settings-about-server-version"),
                        server_version,
                        &theme,
                    ))
                    .child(fact(
                        "about-license",
                        t!("settings-about-license"),
                        t!("settings-about-license-text").to_string(),
                        &theme,
                    )),
            )
            .child(
                div()
                    .id("about-links")
                    .test_support()
                    .role(Role::Navigation)
                    .aria_label(t!("settings-about-links"))
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .child(link(
                        "about-website",
                        t!("settings-about-website"),
                        WEBSITE_URL,
                        &theme,
                    ))
                    .child(link(
                        "about-report",
                        t!("settings-about-report"),
                        ISSUES_URL,
                        &theme,
                    )),
            );
        let people = div()
            .id("about-collaborators")
            .test_support()
            .role(Role::List)
            .aria_label(t!("settings-about-collaborators"))
            .flex()
            .flex_wrap()
            .gap_2()
            .children(COLLABORATORS.iter().map(|name| {
                div()
                    .id(ElementId::Name(format!("collaborator-{name}").into()))
                    .role(Role::ListItem)
                    .aria_label(*name)
                    .px_3()
                    .py_1()
                    .rounded_md()
                    .bg(theme.sidebar)
                    .text_sm()
                    .text_color(theme.text)
                    .child(*name)
                    .test_support()
            }));
        let app_notes = releases(CHANGELOG);
        let server_notes = match &self.server {
            Server::Loading => div()
                .text_sm()
                .text_color(theme.muted)
                .child(t!("settings-about-notes-loading"))
                .into_any_element(),
            Server::Failed(e) => div()
                .text_sm()
                .text_color(theme.danger)
                .child(t!("settings-about-notes-failed", error = e))
                .into_any_element(),
            Server::Ready(changelog) => {
                let list: Vec<Release> = changelog
                    .releases
                    .iter()
                    .flatten()
                    .map(|r| Release {
                        version: r.version.clone(),
                        date: r.date.clone(),
                        notes: r.notes.clone(),
                    })
                    .collect();
                if list.is_empty() {
                    div()
                        .text_sm()
                        .text_color(theme.muted)
                        .child(t!("settings-about-notes-none"))
                        .into_any_element()
                } else {
                    notes("about-server-notes".into(), &list, &theme).into_any_element()
                }
            }
        };
        let subheading = |text: &'static str| {
            div()
                .text_xs()
                .font_weight(FontWeight::BOLD)
                .text_color(theme.faint)
                .child(text.to_uppercase())
        };
        div()
            .flex()
            .flex_col()
            .gap_4()
            .child(block(
                &ctx,
                "about-ircord",
                t!("core-app-name"),
                hero,
                &theme,
            ))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(subheading(t!("settings-about-collaborators")))
                    .child(people),
            )
            .child(
                div()
                    .id("about-release-notes")
                    .test_support()
                    .role(Role::Region)
                    .aria_label(t!("settings-about-release-notes"))
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.text)
                            .child(t!("settings-about-release-notes")),
                    )
                    .child(subheading(t!("settings-about-release-notes-app")))
                    .child(if app_notes.is_empty() {
                        div()
                            .text_sm()
                            .text_color(theme.muted)
                            .child(t!("settings-about-notes-none"))
                            .into_any_element()
                    } else {
                        notes("about-app-notes".into(), &app_notes, &theme).into_any_element()
                    })
                    .child(subheading(t!("settings-about-release-notes-server")))
                    .child(server_notes),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::{CHANGELOG, releases};

    #[test]
    fn reads_releases_from_a_changelog() {
        let text = "# Changelog\n\n## Unreleased\n\n### New\n\n- One\n\n## [1.2.0] - 2026-11-01\n\n- Two\n\n## 1.1.0\n\n";
        let list = releases(text);
        assert_eq!(list.len(), 2, "an empty release is left out");
        assert_eq!(list[0].version, "Unreleased");
        assert_eq!(list[0].notes, "### New\n\n- One");
        assert_eq!(list[1].version, "1.2.0");
        assert_eq!(list[1].date, "2026-11-01");
    }

    #[test]
    fn the_app_changelog_has_notes() {
        assert!(!releases(CHANGELOG).is_empty());
    }
}

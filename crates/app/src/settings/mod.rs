//! The settings page: the registered sections listed under their group
//! headings on the left, the open one on the right, a search over every
//! section's rows, and whether the latest change is saved.
//!
//! A section lives in its own file and registers itself from its `init`
//! (`registry::SettingsSection`), listing its controls as
//! `registry::SettingsRow`s so search can find them. It draws each control
//! with [`row`] (a label and a control) or [`block`] (a heading over a form
//! or list) under the same row ID, so a search result can open the section
//! and land on the row. Settings the server stores in a revisioned document
//! show their saving state here by themselves; a setting kept on this device
//! calls [`saved`] once it is written.
//!
//! The page opens from Settings in the top bar, with [`open`], or with N1's
//! Ctrl+, (Cmd+, on a Mac); Escape goes back to where it was opened from.
//! As on the web, each section is listed with its icon on a soft tint, and
//! Log out sits at the foot of the list.

pub mod about;
pub mod account;
pub mod appearance;
pub mod device;
pub mod emotes;
pub mod messages;
pub mod networks;
pub mod notifications;
pub mod privacy;
pub mod profile;
pub mod shortcuts;

use crate::ui::focus::Reachable;
use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::TestSupportExt;
use gpui_kit::component::input::{Escape, Input, InputEvent, InputState};
use gpui_kit::prelude::*;
use gpui_kit::*;
use ircord_client::store::{SaveState, Workspace};

use crate::registry::{
    self, Pane, RowTarget, SettingsCtx, SettingsGroup, SettingsRow, SettingsSection,
};
use crate::route::{PaneKind, Route};
use crate::session::{Session, SessionEvent};
use crate::theme::{self, Theme};
use crate::ui;

/// How long "Saved" shows after a change on this device is written.
const SAVED_FOR: Duration = Duration::from_secs(3);

pub fn init(cx: &mut App) {
    about::init(cx);
    account::init(cx);
    appearance::init(cx);
    device::init(cx);
    emotes::init(cx);
    messages::init(cx);
    networks::init(cx);
    notifications::init(cx);
    privacy::init(cx);
    profile::init(cx);
    shortcuts::init(cx);
    registry::register(cx, |r| {
        r.panes
            .add(Pane::new(PaneKind::Settings, |ctx, window, cx| {
                view(&ctx.session, window, cx).into_any_element()
            }));
    });
}

/// Open the settings page, on `row` (a [`SettingsRow`] ID) when given.
pub fn open(session: &Entity<Session>, row: Option<&str>, cx: &mut App) {
    if let Some(row) = row
        && let Some(section) = section_of(row, cx)
    {
        let pending = Some((section, SharedString::from(row.to_string())));
        cx.set_global(Pending(pending));
    }
    session.update(cx, |s, cx| s.navigate(Route::Settings, cx));
}

/// A row to land on once the view exists.
struct Pending(Option<(SharedString, SharedString)>);
impl Global for Pending {}

fn section_of(row: &str, cx: &App) -> Option<SharedString> {
    registry::get(cx)
        .settings_sections
        .iter()
        .find(|s| s.id == row || s.rows.iter().any(|r| r.id == row))
        .map(|s| s.id.clone())
}

/// Say that a change kept on this device was written: the header shows
/// "Saved" for a moment.
pub fn saved(cx: &mut App) {
    if let Some(Settings(view)) = cx.try_global::<Settings>().cloned() {
        view.update(cx, |v, cx| v.flash_saved(cx));
    }
}

/// The one settings view, kept across visits so the open section stays.
#[derive(Clone)]
struct Settings(Entity<SettingsView>);
impl Global for Settings {}

fn view(session: &Entity<Session>, window: &mut Window, cx: &mut App) -> Entity<SettingsView> {
    let view = match cx.try_global::<Settings>() {
        Some(Settings(view)) if view.read(cx).session == *session => view.clone(),
        _ => {
            let view = cx.new(|cx| SettingsView::new(session.clone(), window, cx));
            cx.set_global(Settings(view.clone()));
            view
        }
    };
    if let Some(Pending(Some((section, row)))) = cx.try_global::<Pending>() {
        let (section, row) = (section.clone(), row.clone());
        cx.set_global(Pending(None));
        view.update(cx, |v, cx| v.land(section, Some(row), window, cx));
    }
    view
}

/// What one search finds: a row, or a section without rows that matches.
#[derive(Clone, Debug, PartialEq)]
pub struct Found {
    pub section: SharedString,
    pub section_title: SharedString,
    pub row: Option<SettingsRow>,
}

/// Search every section: rows whose words match, in section order; a
/// section whose title or keywords match lists all its rows (or itself).
pub fn search(sections: &[SettingsSection], query: &str) -> Vec<Found> {
    if query.trim().is_empty() {
        return Vec::new();
    }
    let mut found = Vec::new();
    for section in ordered(sections) {
        let whole = section.matches(query);
        let rows: Vec<_> = section
            .rows
            .iter()
            .filter(|r| whole || r.matches(query))
            .collect();
        if rows.is_empty() && whole {
            found.push(Found {
                section: section.id.clone(),
                section_title: section.title.clone(),
                row: None,
            });
        }
        found.extend(rows.into_iter().map(|r| Found {
            section: section.id.clone(),
            section_title: section.title.clone(),
            row: Some(r.clone()),
        }));
    }
    found
}

/// Sections in nav order: by group, then by their order.
fn ordered(sections: &[SettingsSection]) -> Vec<&SettingsSection> {
    let mut list: Vec<_> = sections.iter().collect();
    list.sort_by_key(|s| s.group);
    list
}

/// A section's icon and its tint in the list, as the web's settings.
fn section_icon(id: &str, theme: &Theme) -> (ui::icon::IconName, Hsla) {
    use ui::icon::IconName as I;
    match id {
        "profile" => (I::User, theme.accent),
        "account" => (I::Settings2, theme.warning),
        "networks" => (I::Globe, theme.online),
        "appearance" => (I::Palette, theme.danger),
        "messages" => (I::Inbox, theme.online),
        "notifications" => (I::Bell, theme.danger),
        "device" => (I::HardDrive, theme.accent),
        "privacy" => (I::Eye, theme.accent),
        "shortcuts" => (I::SquareTerminal, theme.warning),
        "emotes" => (I::Heart, theme.warning),
        "about" => (I::Info, theme.accent),
        _ => (I::Settings, theme.muted),
    }
}

/// An icon on a small rounded tint of its colour.
fn tinted_icon(name: ui::icon::IconName, tint: Hsla, theme: &Theme) -> Div {
    div()
        .size(px(24.))
        .flex_none()
        .rounded(theme.radius)
        .bg(tint.opacity(0.16))
        .flex()
        .items_center()
        .justify_center()
        .child(ui::icon::icon(name, tint))
}

fn group_title(group: SettingsGroup) -> &'static str {
    match group {
        SettingsGroup::User => t!("settings-group-user"),
        SettingsGroup::App => t!("settings-group-app"),
        SettingsGroup::Server => t!("settings-group-server"),
    }
}

pub struct SettingsView {
    session: Entity<Session>,
    /// The open section's ID; the first section when `None`.
    open: Option<SharedString>,
    /// The row the last search result landed on.
    target: Option<SharedString>,
    /// Scroll to the target once it is laid out.
    scroll_pending: Rc<Cell<bool>>,
    query: Entity<InputState>,
    scroll: ScrollHandle,
    /// A change on this device was just written.
    flash: Option<Task<()>>,
    /// The page itself, focused when it is entered so Escape reaches it.
    focus: FocusHandle,
    /// Focus the page at the next draw: it was just entered.
    focus_pending: bool,
    _subscriptions: Vec<Subscription>,
}

impl SettingsView {
    fn new(session: Entity<Session>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let query =
            cx.new(|cx| InputState::new(window, cx).placeholder(t!("settings-search-placeholder")));
        let scroll = ScrollHandle::new();
        let subscriptions = vec![
            cx.subscribe_in(
                &query,
                window,
                |this: &mut Self, _, event, window, cx| match event {
                    InputEvent::Change => cx.notify(),
                    InputEvent::PressEnter { .. } => this.pick_first(window, cx),
                    _ => {}
                },
            ),
            cx.subscribe_in(
                &session,
                window,
                |this, session, event: &SessionEvent, window, cx| {
                    if let SessionEvent::Route = event {
                        let here = session.read(cx).route() == Some(Route::Settings);
                        this.focus_pending = here;
                        // Leaving: a focus left on the hidden page would keep
                        // the app's keys from reaching the window.
                        if !here && this.focus.contains_focused(window, cx) {
                            window.blur(cx);
                        }
                    }
                    if let SessionEvent::Changed(changes) = event
                        && changes
                            .topics
                            .contains_key(ircord_client::store::topic::DOCUMENTS)
                    {
                        cx.notify();
                    }
                },
            ),
        ];
        Self {
            session,
            open: None,
            target: None,
            scroll_pending: Rc::default(),
            query,
            scroll,
            flash: None,
            focus: cx.focus_handle(),
            focus_pending: true,
            _subscriptions: subscriptions,
        }
    }

    /// Open a section; the search closes and the page scrolls to the top.
    pub fn open(&mut self, id: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.open = Some(id.into());
        self.target = None;
        self.scroll.set_offset(Point::default());
        cx.notify();
    }

    /// Open `section` on `row`, clearing the search.
    fn land(
        &mut self,
        section: SharedString,
        row: Option<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open(section, cx);
        self.scroll_pending.set(row.is_some());
        self.target = row;
        self.query.update(cx, |q, cx| q.set_value("", window, cx));
    }

    fn pick_first(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let query = self.query.read(cx).value().to_string();
        let sections = registry::get(cx).settings_sections.to_vec();
        if let Some(found) = search(&sections, &query).into_iter().next() {
            self.land(found.section, found.row.map(|r| r.id), window, cx);
        }
    }

    /// Escape: back to where the page was opened from.
    fn leave(&mut self, cx: &mut Context<Self>) {
        let session = self.session.clone();
        if session.read(cx).route() == Some(Route::Settings) {
            session.update(cx, |s, cx| s.go_back(cx));
        }
    }

    fn flash_saved(&mut self, cx: &mut Context<Self>) {
        self.flash = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SAVED_FOR).await;
            let _ = this.update(cx, |this, cx| {
                this.flash = None;
                cx.notify();
            });
        }));
        cx.notify();
    }

    /// What the header says about saving: the server's documents first, then
    /// a change on this device.
    fn save_state(&self, cx: &App) -> Option<SaveState> {
        let from_documents = self
            .session
            .read(cx)
            .client()
            .and_then(|client| documents_state(&client.read()));
        from_documents.or_else(|| self.flash.as_ref().map(|_| SaveState::Saved))
    }
}

/// The saving state across the server's documents: any saving, else any
/// failure, else any saved.
pub fn documents_state(w: &Workspace) -> Option<SaveState> {
    let d = &w.documents;
    let states = [
        d.preferences.save_state(),
        d.appearance.save_state(),
        d.themes.save_state(),
        d.library.save_state(),
        d.notification_settings.save_state(),
    ];
    if states.contains(&&SaveState::Saving) {
        return Some(SaveState::Saving);
    }
    if let Some(failed) = states.iter().find(|s| matches!(s, SaveState::Failed(_))) {
        return Some((*failed).clone());
    }
    states
        .contains(&&SaveState::Saved)
        .then_some(SaveState::Saved)
}

impl Render for SettingsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme::get(cx).clone();
        let sections = registry::get(cx).settings_sections.to_vec();
        let open = self
            .open
            .clone()
            .filter(|id| sections.iter().any(|s| &s.id == id))
            .or_else(|| ordered(&sections).first().map(|s| s.id.clone()));
        let query = self.query.read(cx).value().to_string();
        let hover_bg = theme.hover;
        if std::mem::take(&mut self.focus_pending) {
            self.focus.focus(window, cx);
        }

        let mut nav = div()
            .id("settings-sections")
            .test_support()
            .role(Role::TabList)
            .aria_label(t!("settings-title"))
            .w(px(220.))
            .flex_none()
            .h_full()
            .overflow_y_scroll()
            .p_2()
            .flex()
            .flex_col()
            .gap_0p5()
            .bg(theme.sidebar)
            .child(
                div().pb_2().child(
                    Input::new(&self.query)
                        .id("settings-search")
                        .aria_label(t!("settings-search")),
                ),
            );
        let mut last_group = None;
        for section in ordered(&sections) {
            if last_group != Some(section.group) {
                last_group = Some(section.group);
                nav = nav.child(
                    div()
                        .pt_2()
                        .pb_1()
                        .px_2()
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.muted)
                        .child(group_title(section.group).to_uppercase()),
                );
            }
            let active = query.trim().is_empty() && open.as_ref() == Some(&section.id);
            let id = section.id.clone();
            let hover = theme.hover;
            nav = nav.child(
                div()
                    .id(ElementId::Name(
                        format!("settings-section-{}", section.id).into(),
                    ))
                    .test_support()
                    .role(Role::Tab)
                    .reachable(&theme)
                    .aria_label(section.title.clone())
                    .aria_selected(active)
                    .px_2()
                    .py_1()
                    .flex()
                    .items_center()
                    .gap_2()
                    .rounded_md()
                    .cursor_pointer()
                    .text_color(theme.text)
                    .when(active, |d| d.bg(theme.selected))
                    .when(!active, |d| d.hover(move |d| d.bg(hover)))
                    .child({
                        let (icon, tint) = section_icon(&section.id, &theme);
                        tinted_icon(icon, tint, &theme)
                    })
                    .child(section.title.clone())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        let id = id.clone();
                        this.land(id, None, window, cx);
                    })),
            );
        }

        // Log out, at the foot of the list.
        let session = self.session.clone();
        nav = nav.child(div().flex_1().min_h(px(16.))).child(
            div()
                .id("settings-log-out")
                .test_support()
                .role(Role::Button)
                .reachable(&theme)
                .aria_label(t!("settings-account-log-out"))
                .px_2()
                .py_1()
                .flex()
                .items_center()
                .gap_2()
                .rounded_md()
                .cursor_pointer()
                .text_color(theme.danger)
                .hover(move |d| d.bg(hover_bg))
                .child(tinted_icon(
                    ui::icon::IconName::ArrowLeft,
                    theme.danger,
                    &theme,
                ))
                .child(t!("settings-account-log-out"))
                .on_click(move |_, window, cx| {
                    let session = session.clone();
                    ui::confirm::confirm(
                        window,
                        cx,
                        ui::confirm::Confirm {
                            title: t!("settings-account-log-out-title").into(),
                            body: t!("settings-account-log-out-body").into(),
                            ok: t!("settings-account-log-out").into(),
                            danger: true,
                        },
                        move |_, cx| session.update(cx, |s, cx| s.sign_out(cx)),
                    );
                }),
        );

        let content = if !query.trim().is_empty() {
            self.results(&sections, &query, &theme, cx)
                .into_any_element()
        } else {
            match sections.iter().find(|s| Some(&s.id) == open.as_ref()) {
                Some(section) => {
                    let ctx = SettingsCtx {
                        session: self.session.clone(),
                        target: self.target.clone().map(|id| RowTarget {
                            id,
                            reached: scroll_to(self.scroll.clone(), self.scroll_pending.clone()),
                        }),
                    };
                    div()
                        .id("settings-content")
                        .test_support()
                        .role(Role::TabPanel)
                        .aria_label(section.title.clone())
                        .size_full()
                        .overflow_y_scroll()
                        .track_scroll(&self.scroll)
                        .child(
                            div()
                                .max_w(px(760.))
                                .p_6()
                                .flex()
                                .flex_col()
                                .gap_4()
                                .child(
                                    div()
                                        .text_xl()
                                        .font_weight(FontWeight::BOLD)
                                        .text_color(theme.text)
                                        .child(section.title.clone()),
                                )
                                .child((section.render)(&ctx, window, cx)),
                        )
                        .into_any_element()
                }
                None => ui::empty::empty("settings-empty", t!("settings-empty"), None, &theme)
                    .into_any_element(),
            }
        };
        let mut page = ui::page::Page::new(
            "settings",
            t!("settings-title"),
            div()
                .size_full()
                .flex()
                .child(nav)
                .child(div().flex_1().min_w_0().h_full().child(content)),
        );
        if let Some(state) = self.save_state(cx) {
            page = page.action(save_indicator(&state, &theme));
        }
        div()
            .size_full()
            .track_focus(&self.focus)
            .capture_action(cx.listener(|this, _: &Escape, _, cx| {
                this.leave(cx);
                cx.stop_propagation();
            }))
            // Escape is the app's "mark read" key; on this page it leaves.
            .capture_action(
                cx.listener(|this, _: &crate::features::shortcuts::MarkRead, _, cx| {
                    this.leave(cx);
                    cx.stop_propagation();
                }),
            )
            .child(page.render(&theme))
    }
}

impl SettingsView {
    fn results(
        &self,
        sections: &[SettingsSection],
        query: &str,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let found = search(sections, query);
        if found.is_empty() {
            return ui::empty::empty(
                "settings-no-matches",
                t!("settings-no-matches", query = query.trim()),
                Some(t!("settings-no-matches-hint").into()),
                theme,
            )
            .into_any_element();
        }
        let mut list = div()
            .id("settings-results")
            .test_support()
            .role(Role::List)
            .aria_label(t!("settings-results"))
            .size_full()
            .overflow_y_scroll()
            .p_4()
            .flex()
            .flex_col()
            .gap_1();
        for found in found {
            let (id, label, description) = match &found.row {
                Some(row) => (
                    format!("settings-result-{}", row.id),
                    row.label.clone(),
                    row.description.clone(),
                ),
                None => (
                    format!("settings-result-{}", found.section),
                    found.section_title.clone(),
                    None,
                ),
            };
            let aria = t!(
                "settings-result-label",
                label = label,
                section = found.section_title
            );
            let (section, row) = (found.section.clone(), found.row.map(|r| r.id));
            let hover = theme.hover;
            list = list.child(
                div()
                    .id(ElementId::Name(id.into()))
                    .test_support()
                    .role(Role::ListItem)
                    .aria_label(SharedString::from(aria))
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .cursor_pointer()
                    .hover(move |d| d.bg(hover))
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(div().text_color(theme.text).child(label))
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(theme.faint)
                                    .child(found.section_title.clone()),
                            ),
                    )
                    .when_some(description, |d, text| {
                        d.child(div().text_sm().text_color(theme.muted).child(text))
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.land(section.clone(), row.clone(), window, cx)
                    })),
            );
        }
        list.into_any_element()
    }
}

fn save_indicator(state: &SaveState, theme: &Theme) -> impl IntoElement + use<> {
    let (text, color): (SharedString, Hsla) = match state {
        SaveState::Idle => ("".into(), theme.muted),
        SaveState::Saving => (t!("settings-saving").into(), theme.muted),
        SaveState::Saved => (t!("settings-saved").into(), theme.online),
        SaveState::Failed(error) => (
            t!("settings-save-failed", error = error).into(),
            theme.danger,
        ),
    };
    div()
        .id("settings-save-state")
        .test_support()
        .role(Role::Status)
        .aria_label(text.clone())
        .text_sm()
        .text_color(color)
        .child(text)
}

/// How the sample messages are drawn: the message settings' font, size,
/// clock and name colours; the defaults are the app's.
#[derive(Clone, Debug)]
pub struct PreviewLook {
    pub font: Option<&'static str>,
    pub size: f32,
    pub clock_12h: bool,
    pub colored_names: bool,
}

impl Default for PreviewLook {
    fn default() -> Self {
        Self {
            font: None,
            size: 14.,
            clock_12h: false,
            colored_names: true,
        }
    }
}

/// Two sample messages drawn with the active theme, as the web's preview
/// at the top of Appearance and Messages, so a change shows at once.
pub fn preview(theme: &Theme, look: PreviewLook) -> impl IntoElement + use<> {
    let time: SharedString = if look.clock_12h {
        t!("settings-preview-time-12h").into()
    } else {
        t!("settings-preview-time").into()
    };
    let message = |id: &'static str,
                   account: &'static str,
                   tone: &'static str,
                   name: &'static str,
                   text: Div| {
        let member = ircord_client::contract::Member {
            id: id.to_string(),
            account: account.to_string(),
            nick: name.to_lowercase(),
            display_name: name.to_string(),
            ..Default::default()
        };
        let color = ui::avatar::name_color(&member, look.colored_names, theme)
            .map(theme::hex)
            .unwrap_or(theme.text);
        let (fill, ink) = ircord_client::store::people::tone(tone);
        div()
            .flex()
            .gap_3()
            .child(
                div()
                    .size(px(36.))
                    .flex_none()
                    .rounded_full()
                    .bg(theme::hex(fill))
                    .flex()
                    .items_center()
                    .justify_center()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme::hex(ink))
                    .child(name.chars().next().unwrap_or('?').to_string()),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .flex()
                            .items_baseline()
                            .gap_2()
                            .child(
                                div()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(color)
                                    .child(name),
                            )
                            .child(div().text_xs().text_color(theme.faint).child(time.clone())),
                    )
                    .child(text.text_color(theme.body)),
            )
    };
    div()
        .id("settings-preview")
        .test_support()
        .role(Role::Group)
        .aria_label(t!("settings-preview"))
        .p_4()
        .flex()
        .flex_col()
        .gap_3()
        .rounded(theme.radius_large)
        .border_1()
        .border_color(theme.border)
        .bg(theme.main)
        .shadow_sm()
        .text_size(px(look.size))
        .when_some(look.font, |d, family| d.font_family(family))
        .child(message(
            "preview-jules",
            "jules",
            "mint",
            t!("settings-preview-first-name"),
            div().child(t!("settings-preview-first")),
        ))
        .child(message(
            "preview-noah",
            "noah",
            "blue",
            t!("settings-preview-second-name"),
            div()
                .flex()
                .gap_1()
                .child(t!("settings-preview-second"))
                .child(
                    div()
                        .text_color(theme.link)
                        .child(t!("settings-preview-channel")),
                ),
        ))
}

/// A setting: its label (and a short description) beside its control, under
/// the row ID settings search lands on.
pub fn row<L: Into<SharedString>, C: IntoElement>(
    ctx: &SettingsCtx,
    id: &str,
    label: L,
    description: Option<SharedString>,
    control: C,
    theme: &Theme,
) -> impl IntoElement + use<L, C> {
    let label: SharedString = label.into();
    frame(ctx, id, label.clone(), theme)
        .flex()
        .items_center()
        .justify_between()
        .gap_4()
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .child(div().text_color(theme.text).child(label))
                .when_some(description, |d, text| {
                    d.child(div().text_sm().text_color(theme.muted).child(text))
                }),
        )
        .child(div().flex_none().child(control))
        .test_support()
}

/// A heading over a form or a list, under the row ID settings search lands on.
pub fn block<H: Into<SharedString>, B: IntoElement>(
    ctx: &SettingsCtx,
    id: &str,
    heading: H,
    body: B,
    theme: &Theme,
) -> impl IntoElement + use<H, B> {
    let heading: SharedString = heading.into();
    frame(ctx, id, heading.clone(), theme)
        .flex()
        .flex_col()
        .gap_2()
        .child(
            div()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.text)
                .child(heading),
        )
        .child(body)
        .test_support()
}

/// The row's frame: its ID, its name, and the highlight and scroll anchor
/// when a search landed on it. The caller ends it with `.test_support()`.
fn frame(ctx: &SettingsCtx, id: &str, label: SharedString, theme: &Theme) -> Stateful<Div> {
    let target = ctx.target.as_ref().filter(|t| t.id.as_ref() == id);
    div()
        .id(ElementId::Name(format!("setting-{id}").into()))
        .role(Role::Group)
        .aria_label(label)
        // Flat, as the web's settings: no box per row; a search landing
        // on a row frames it in the accent.
        .px_3()
        .py_3()
        .rounded_md()
        .border_1()
        .border_color(if target.is_some() {
            theme.accent
        } else {
            gpui_kit::transparent_black()
        })
        .when(target.is_some(), |d| d.bg(theme.selected))
        .when_some(target, |d, target| {
            let reached = target.reached.clone();
            d.relative().child(
                canvas(
                    move |bounds, window, cx| reached(bounds, window, cx),
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            )
        })
}

/// What the landed-on row calls with its bounds: once, scroll the page so
/// the row sits near the top, and draw again.
#[allow(clippy::type_complexity)]
fn scroll_to(
    scroll: ScrollHandle,
    pending: Rc<Cell<bool>>,
) -> Rc<dyn Fn(Bounds<Pixels>, &mut Window, &mut App)> {
    Rc::new(move |row, window, _| {
        if !pending.replace(false) {
            return;
        }
        let offset = scroll.offset();
        let top = scroll.bounds().origin.y + px(16.);
        let y = (offset.y - (row.origin.y - top)).min(px(0.));
        let y = y.max(-scroll.max_offset().y);
        scroll.set_offset(point(offset.x, y));
        window.refresh();
    })
}

#[cfg(test)]
mod tests {
    use super::{SettingsGroup, SettingsRow, SettingsSection, Workspace, documents_state, search};
    use gpui_kit::{IntoElement, div};

    fn section(id: &str, group: SettingsGroup, rows: &[(&str, &str, &str)]) -> SettingsSection {
        let mut s = SettingsSection::new(id.to_string(), id.to_string(), |_, _, _| {
            div().into_any_element()
        })
        .group(group);
        for (rid, label, keywords) in rows {
            s = s.row(SettingsRow::new(rid.to_string(), label.to_string()).keywords(keywords));
        }
        s
    }

    #[test]
    fn search_lists_rows_in_nav_order() {
        let sections = vec![
            section("about", SettingsGroup::Server, &[]),
            section(
                "account",
                SettingsGroup::User,
                &[
                    ("password", "Change password", "security"),
                    ("log-out", "Log out", "sign out"),
                ],
            ),
            section(
                "notifications",
                SettingsGroup::App,
                &[("sound", "Notification sound", "")],
            ),
        ];
        let rows = |q| -> Vec<String> {
            search(&sections, q)
                .into_iter()
                .map(|f| f.row.map(|r| r.id.to_string()).unwrap_or(f.section.into()))
                .collect()
        };
        assert_eq!(rows("password"), ["password"]);
        assert_eq!(rows("out"), ["log-out", "about"], "about has 'out' in it");
        assert_eq!(
            rows("account"),
            ["password", "log-out"],
            "a section's title"
        );
        assert_eq!(rows("sound"), ["sound"]);
        assert!(rows("  ").is_empty());
        assert!(rows("nothing like this").is_empty());
    }

    #[test]
    fn the_saving_state_follows_the_documents() {
        let w = Workspace::default();
        assert_eq!(documents_state(&w), None);
    }
}

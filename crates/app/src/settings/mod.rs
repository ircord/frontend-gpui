//! The settings page: every registered section, listed on the left and
//! drawn on the right. Each section lives in its own file and registers
//! itself from its `init`, so a section can be built and tested before the
//! settings shell (package S0) lands. This minimal shell is S0's to replace.

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

use gpui_kit::TestSupportExt;
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::registry::{self, Pane, SettingsCtx};
use crate::route::PaneKind;
use crate::session::Session;
use crate::theme;
use crate::ui;

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
        r.panes.add(Pane::new(PaneKind::Settings, |ctx, _, cx| {
            view(&ctx.session, cx).into_any_element()
        }));
    });
}

/// The one settings view, kept across visits so the open section stays.
struct Settings(Entity<SettingsView>);
impl Global for Settings {}

fn view(session: &Entity<Session>, cx: &mut App) -> Entity<SettingsView> {
    if let Some(Settings(view)) = cx.try_global::<Settings>()
        && view.read(cx).session == *session
    {
        return view.clone();
    }
    let view = cx.new(|_| SettingsView {
        session: session.clone(),
        open: None,
    });
    cx.set_global(Settings(view.clone()));
    view
}

pub struct SettingsView {
    session: Entity<Session>,
    /// The open section's ID; the first section when `None`.
    open: Option<SharedString>,
}

impl SettingsView {
    pub fn open(&mut self, id: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.open = Some(id.into());
        cx.notify();
    }
}

impl Render for SettingsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme::get(cx).clone();
        let sections = registry::get(cx).settings_sections.to_vec();
        let open = self
            .open
            .clone()
            .filter(|id| sections.iter().any(|s| &s.id == id))
            .or_else(|| sections.first().map(|s| s.id.clone()));
        let mut nav = div()
            .id("settings-sections")
            .test_support()
            .role(Role::TabList)
            .aria_label(t!("settings-title"))
            .w(px(200.))
            .flex_none()
            .p_2()
            .flex()
            .flex_col()
            .gap_0p5()
            .bg(theme.sidebar);
        for section in &sections {
            let active = open.as_ref() == Some(&section.id);
            let id = section.id.clone();
            let hover = theme.hover;
            nav = nav.child(
                div()
                    .id(ElementId::Name(
                        format!("settings-section-{}", section.id).into(),
                    ))
                    .test_support()
                    .role(Role::Tab)
                    .aria_label(section.title.clone())
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .cursor_pointer()
                    .text_color(if active { theme.text } else { theme.muted })
                    .when(active, |d| d.bg(theme.selected))
                    .when(!active, |d| d.hover(move |d| d.bg(hover)))
                    .child(section.title.clone())
                    .on_click(cx.listener(move |this, _, _, cx| this.open(id.clone(), cx))),
            );
        }
        let ctx = SettingsCtx {
            session: self.session.clone(),
            target: None,
        };
        let content = match sections.iter().find(|s| Some(&s.id) == open.as_ref()) {
            Some(section) => div()
                .id("settings-content")
                .test_support()
                .role(Role::TabPanel)
                .aria_label(section.title.clone())
                .flex_1()
                .min_w_0()
                .p_4()
                .child((section.render)(&ctx, window, cx))
                .into_any_element(),
            None => ui::empty::empty("settings-empty", t!("settings-empty"), None, &theme)
                .into_any_element(),
        };
        ui::page::Page::new(
            "settings",
            t!("settings-title"),
            div().size_full().flex().child(nav).child(content),
        )
        .render(&theme)
    }
}

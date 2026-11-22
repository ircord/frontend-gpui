//! The drafts page (package D1): every unsent draft, newest first, with the
//! conversation it belongs to and when it was last edited. A row opens its
//! conversation with the text back in the box; a draft can be copied, or
//! discarded and the discard undone. Drafts of conversations that are gone
//! stay listed so the text can still be copied.
//!
//! This file also keeps the drafts on this device: they are read once the
//! account is known and written shortly after each change (see
//! `ircord_client::store::drafts`). The message box side is in
//! `composer/drafts.rs`. The page opens from a row under the conversation
//! list (while there are drafts), your own menu and a key.

use crate::menu_order;
use crate::ui::focus::Reachable;
use std::cell::Cell;
use std::time::Duration;

use gpui_kit::TestSupportExt;
use gpui_kit::prelude::*;
use gpui_kit::*;
use ircord_client::store::Changes;
use ircord_client::store::drafts::{self, Draft, Place, Restore, Row};

use crate::composer::ComposerView;
use crate::composer::drafts::Shown;
use crate::registry::{self, KeyDefault, MenuItem, Pane, PaneCtx, SidebarCtx, Slot};
use crate::route::{PaneKind, Route};
use crate::session::{Phase, Session, SessionEvent};
use crate::theme::{self, Theme};
use crate::ui;
use crate::ui::icon::IconName;

gpui_kit::actions!(
    drafts,
    [
        /// Open the drafts page.
        OpenDrafts,
    ]
);

/// How long after the last change the drafts are written.
const SAVE_DELAY: Duration = Duration::from_millis(300);

#[derive(Default)]
struct Drafts {
    session: Option<WeakEntity<Session>>,
    /// Every message box and the conversation its text belongs to.
    composers: Vec<(WeakEntity<ComposerView>, Shown)>,
    /// Bumped by every change; a save runs only for the newest one.
    generation: u64,
    /// The last write failed: the drafts are kept only until the app closes.
    failed: bool,
    /// The last discard, kept so it can be undone while the page is open,
    /// and what undoing it did when it could not put the text back.
    discarded: Option<(String, Draft, Option<Restore>)>,
    page: Option<Entity<DraftsView>>,
    footer: Option<Entity<FooterView>>,
}

impl Global for Drafts {}

pub fn init(cx: &mut App) {
    cx.default_global::<Drafts>();
    cx.observe_new::<Session>(|_, _, cx| {
        let weak = cx.weak_entity();
        let d = cx.global_mut::<Drafts>();
        d.session = Some(weak);
        d.discarded = None;
        cx.subscribe_self(|session: &mut Session, event: &SessionEvent, _| {
            if let SessionEvent::Changed(changes) = event
                && changes.reset
            {
                ensure_loaded(session);
            }
        })
        .detach();
    })
    .detach();
    cx.observe_new::<ComposerView>(|composer, window, cx| {
        let session = cx
            .try_global::<Drafts>()
            .and_then(|d| d.session.as_ref()?.upgrade());
        if let (Some(session), Some(window)) = (session, window) {
            crate::composer::drafts::attach(composer, session, window, cx);
        }
    })
    .detach();
    cx.on_action(|_: &OpenDrafts, cx| {
        if let Some(session) = session(cx) {
            open(&session, cx);
        }
    });
    registry::register(cx, |r| {
        r.panes
            .add(Pane::new(PaneKind::Drafts, |ctx: &PaneCtx, _, cx| {
                page(&ctx.session, cx).into_any_element()
            }));
        r.sidebar_footer.add(
            Slot::new("drafts", |ctx: &SidebarCtx, _, cx| {
                (count(&ctx.session, cx) > 0).then(|| footer(&ctx.session, cx).into_any_element())
            })
            .order(-10),
        );
        r.self_menu.add(
            MenuItem::new("drafts", t!("drafts-title"), |ctx: &SidebarCtx, _, cx| {
                open(&ctx.session, cx)
            })
            .icon(IconName::FileText)
            .order(menu_order::own::DRAFTS),
        );
        r.keys.add(KeyDefault::new(
            "drafts",
            t!("drafts-key"),
            "",
            Some("Root"),
            OpenDrafts,
        ));
    });
}

fn session(cx: &App) -> Option<Entity<Session>> {
    cx.try_global::<Drafts>()?.session.as_ref()?.upgrade()
}

/// Show the drafts page.
pub fn open(session: &Entity<Session>, cx: &mut App) {
    session.update(cx, |s, cx| s.navigate(Route::Drafts, cx));
}

pub(crate) fn remember_composer(composer: WeakEntity<ComposerView>, shown: Shown, cx: &mut App) {
    let d = cx.default_global::<Drafts>();
    d.composers.retain(|(c, _)| c.upgrade().is_some());
    d.composers.push((composer, shown));
}

/// The draft of `conversation`.
pub fn text(session: &Entity<Session>, conversation: &str, cx: &App) -> String {
    session
        .read(cx)
        .client()
        .map(|c| c.read().drafts.text(conversation).to_string())
        .unwrap_or_default()
}

/// Drafts with visible text.
pub fn count(session: &Entity<Session>, cx: &App) -> usize {
    session
        .read(cx)
        .client()
        .map_or(0, |c| c.read().drafts.count())
}

/// Keep `text` as the draft of `conversation`, and save it shortly.
pub fn set(session: &Entity<Session>, conversation: &str, text: &str, cx: &mut App) {
    let Some(client) = session.read(cx).client().cloned() else {
        return;
    };
    let changed = Cell::new(false);
    client.change(|w| {
        let changes = drafts::set(w, conversation, text, now_ms());
        changed.set(!changes.is_empty());
        changes
    });
    if changed.get() {
        save_soon(cx);
    }
}

/// Read this account's drafts from the device, once per client.
pub fn ensure_loaded(s: &Session) {
    let Phase::SignedIn { server, client } = &s.phase else {
        return;
    };
    let key = {
        let w = client.read();
        if w.drafts.loaded() || w.user.id.is_empty() {
            return;
        }
        drafts::account_key(&server.origin(), &w)
    };
    let saved = drafts::read(&s.config, &key);
    client.change(|w| drafts::load(w, saved));
}

/// Write the drafts once changes stop for [`SAVE_DELAY`].
fn save_soon(cx: &mut App) {
    let d = cx.default_global::<Drafts>();
    d.generation += 1;
    let generation = d.generation;
    cx.spawn(async move |cx| {
        cx.background_executor().timer(SAVE_DELAY).await;
        cx.update(|cx| {
            if cx.global::<Drafts>().generation == generation {
                save_now(cx);
            }
        });
    })
    .detach();
}

fn save_now(cx: &mut App) {
    let Some(session) = session(cx) else {
        return;
    };
    let s = session.read(cx);
    let Phase::SignedIn { server, client } = &s.phase else {
        return;
    };
    let (key, saved) = {
        let w = client.read();
        if !w.drafts.loaded() {
            return;
        }
        (drafts::account_key(&server.origin(), &w), w.drafts.saved())
    };
    let result = drafts::write(&s.config, &key, saved);
    if let Err(e) = &result {
        log::warn!("save the drafts: {e}");
    }
    let failed = result.is_err();
    let d = cx.global_mut::<Drafts>();
    if d.failed != failed {
        d.failed = failed;
        cx.refresh_windows();
    }
}

/// Give every box showing `conversation` its draft again (after a discard
/// or an undo from the page).
fn refill(session: &Entity<Session>, conversation: &str, window: &mut Window, cx: &mut App) {
    let text = text(session, conversation, cx);
    let composers: Vec<_> = cx
        .global::<Drafts>()
        .composers
        .iter()
        .filter(|(_, shown)| shown.borrow().as_deref() == Some(conversation))
        .filter_map(|(c, _)| c.upgrade())
        .collect();
    for composer in composers {
        composer.update(cx, |c, cx| c.set_text_at(&text, text.len(), window, cx));
    }
}

fn discard(session: &Entity<Session>, row: &Row, window: &mut Window, cx: &mut App) {
    let Some(client) = session.read(cx).client().cloned() else {
        return;
    };
    let kept = Cell::new(None);
    client.change(|w| {
        let draft = w.drafts.get(&row.conversation).cloned();
        if w.drafts.discard(&row.conversation, &row.text) {
            kept.set(draft);
            Changes::topic_id(drafts::TOPIC, row.conversation.clone())
        } else {
            Changes::default()
        }
    });
    let Some(draft) = kept.take() else {
        return ui::toast::info(window, cx, t!("drafts-changed"));
    };
    cx.global_mut::<Drafts>().discarded = Some((row.conversation.clone(), draft, None));
    save_soon(cx);
    refill(session, &row.conversation, window, cx);
}

fn undo(session: &Entity<Session>, window: &mut Window, cx: &mut App) {
    let Some((conversation, draft, _)) = cx.global::<Drafts>().discarded.clone() else {
        return;
    };
    let Some(client) = session.read(cx).client().cloned() else {
        return;
    };
    let result = Cell::new(Restore::Conflict);
    client.change(|w| {
        result.set(w.drafts.restore(&conversation, draft.clone()));
        if result.get() == Restore::Restored {
            Changes::topic_id(drafts::TOPIC, conversation.clone())
        } else {
            Changes::default()
        }
    });
    let result = result.get();
    let d = cx.global_mut::<Drafts>();
    if result == Restore::Restored {
        d.discarded = None;
        save_soon(cx);
        refill(session, &conversation, window, cx);
    } else {
        d.discarded = Some((conversation, draft, Some(result)));
    }
    cx.refresh_windows();
}

fn copy(text: &str, window: &mut Window, cx: &mut App) {
    cx.write_to_clipboard(ClipboardItem::new_string(text.to_string()));
    ui::toast::success(window, cx, t!("drafts-copied"));
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// Where a row's draft belongs, as a person reads it.
fn place(row: &Row) -> String {
    let title = match (&row.name, row.place) {
        (Some(name), Place::Channel) => format!("#{name}"),
        (Some(name), Place::Direct) => name.clone(),
        (None, _) => t!("drafts-gone-title").to_string(),
    };
    match &row.network {
        Some(network) => t!("drafts-on-network", conversation = title, network = network),
        None => title,
    }
}

fn edited(row: &Row) -> String {
    match row
        .edited_ms
        .and_then(chrono::DateTime::from_timestamp_millis)
    {
        Some(t) => t
            .with_timezone(&chrono::Local)
            .format("%d %b %H:%M")
            .to_string(),
        None => t!("drafts-edited-earlier").to_string(),
    }
}

// ---- The page ----

/// The page, kept for the session it shows.
fn page(session: &Entity<Session>, cx: &mut App) -> Entity<DraftsView> {
    if let Some(view) = &cx.global::<Drafts>().page
        && view.read(cx).session == *session
    {
        return view.clone();
    }
    let view = cx.new(|cx| DraftsView::new(session.clone(), cx));
    cx.global_mut::<Drafts>().page = Some(view.clone());
    view
}

/// Redraw on a change to the drafts, the conversations or a new bootstrap.
fn redraws(event: &SessionEvent) -> bool {
    match event {
        SessionEvent::Changed(changes) => {
            changes.reset || changes.sidebar || changes.topics.contains_key(drafts::TOPIC)
        }
        SessionEvent::Phase | SessionEvent::Route => true,
        _ => false,
    }
}

pub struct DraftsView {
    session: Entity<Session>,
    _subscription: Subscription,
}

impl DraftsView {
    fn new(session: Entity<Session>, cx: &mut Context<Self>) -> Self {
        let subscription = cx.subscribe(&session, |_, _, event: &SessionEvent, cx| {
            if redraws(event) {
                cx.notify();
            }
        });
        Self {
            session,
            _subscription: subscription,
        }
    }

    fn row(&self, row: Row, theme: &Theme) -> impl IntoElement {
        let id = row.conversation.clone();
        let place = place(&row);
        let available = row.available();
        let hover = theme.row_hover;
        let title_color = if available { theme.text } else { theme.muted };
        let session = self.session.clone();
        let open = {
            let (session, id) = (session.clone(), id.clone());
            move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                session.update(cx, |s, cx| s.navigate(Route::Conversation(id.clone()), cx));
            }
        };
        let text = row.text.clone();
        let discarded = row.clone();
        let mut body = div()
            .id(ElementId::Name(format!("draft-open-{id}").into()))
            .test_support()
            .role(Role::Button)
            .reachable(theme)
            .aria_label(t!("drafts-open", place = place.clone()))
            .when(!available, |d| d.aria_description(t!("drafts-gone")))
            .flex_1()
            .min_w_0()
            .px_3()
            .py_2()
            .rounded_md()
            .flex()
            .flex_col()
            .gap_0p5()
            .child(
                div()
                    .flex()
                    .items_baseline()
                    .gap_2()
                    .text_xs()
                    .text_color(theme.muted)
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(title_color)
                            .child(place.clone()),
                    )
                    .child(div().ml_auto().text_color(theme.faint).child(edited(&row))),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(theme.text)
                    .line_clamp(3)
                    .child(row.preview.clone()),
            )
            .when(!available, |d| {
                d.child(
                    div()
                        .text_xs()
                        .text_color(theme.muted)
                        .child(t!("drafts-gone")),
                )
            });
        if available {
            body = body
                .cursor_pointer()
                .hover(move |d| d.bg(hover))
                .on_click(open);
        }
        div()
            .id(ElementId::Name(format!("draft-{id}").into()))
            .test_support()
            .role(Role::Group)
            .aria_label(place.clone())
            .flex()
            .items_start()
            .gap_1()
            .child(body)
            .child(
                div()
                    .flex()
                    .gap_0p5()
                    .pt_2()
                    .pr_2()
                    .child(ui::icon::button(
                        ElementId::Name(format!("draft-copy-{id}").into()),
                        IconName::Copy,
                        t!("drafts-copy", place = place.clone()),
                        theme,
                        move |_, window, cx| copy(&text, window, cx),
                    ))
                    .child(ui::icon::button(
                        ElementId::Name(format!("draft-discard-{id}").into()),
                        IconName::Delete,
                        t!("drafts-discard", place = place),
                        theme,
                        move |_, window, cx| discard(&session, &discarded, window, cx),
                    )),
            )
    }

    /// The bar after a discard: undo it, or why it could not be undone.
    fn notice(
        &self,
        discarded: Option<(String, Draft, Option<Restore>)>,
        theme: &Theme,
    ) -> Option<impl IntoElement + use<>> {
        let (_, draft, refused) = discarded?;
        let session = self.session.clone();
        let text = draft.text.clone();
        let (message, button) = match refused {
            None => (
                t!("drafts-discarded").to_string(),
                text_button(
                    "drafts-undo",
                    t!("drafts-undo"),
                    theme,
                    move |window, cx| undo(&session, window, cx),
                )
                .into_any_element(),
            ),
            Some(why) => (
                if why == Restore::Full {
                    t!("drafts-undo-full").to_string()
                } else {
                    t!("drafts-undo-conflict").to_string()
                },
                text_button(
                    "drafts-copy-discarded",
                    t!("drafts-copy-discarded"),
                    theme,
                    move |window, cx| copy(&text, window, cx),
                )
                .into_any_element(),
            ),
        };
        Some(
            div()
                .id("drafts-notice")
                .test_support()
                .role(Role::Status)
                .aria_label(message.clone())
                .mx_4()
                .mb_2()
                .px_3()
                .py_2()
                .rounded_md()
                .bg(theme.sidebar)
                .flex()
                .items_center()
                .gap_2()
                .text_sm()
                .child(div().flex_1().text_color(theme.text).child(message))
                .child(button),
        )
    }
}

impl Render for DraftsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme::get(cx).clone();
        let (rows, loaded) = match self.session.read(cx).client() {
            Some(client) => {
                let w = client.read();
                (drafts::rows(&w), w.drafts.loaded())
            }
            None => (Vec::new(), false),
        };
        let d = cx.global::<Drafts>();
        let (failed, discarded) = (d.failed, d.discarded.clone());
        let mut body = div().flex().flex_col().pb_4();
        body = body.child(
            div()
                .px_4()
                .pb_2()
                .text_xs()
                .text_color(theme.muted)
                .child(t!("drafts-scope")),
        );
        if failed {
            body = body.child(
                div()
                    .id("drafts-not-saved")
                    .test_support()
                    .role(Role::Alert)
                    .aria_label(t!("drafts-not-saved"))
                    .mx_4()
                    .mb_2()
                    .text_sm()
                    .text_color(theme.danger)
                    .child(t!("drafts-not-saved")),
            );
        }
        if let Some(notice) = self.notice(discarded, &theme) {
            body = body.child(notice);
        }
        if rows.is_empty() {
            let title = if loaded {
                t!("drafts-empty-title")
            } else {
                t!("drafts-loading")
            };
            body = body.child(ui::empty::with_icon(
                "drafts-empty",
                loaded.then_some(IconName::FileText),
                title,
                loaded.then(|| t!("drafts-empty").into()),
                &theme,
            ));
        } else {
            let mut list = div()
                .id("drafts-list")
                .test_support()
                .role(Role::List)
                .aria_label(t!("drafts-title"))
                .px_2()
                .flex()
                .flex_col()
                .gap_0p5();
            for row in rows {
                list = list.child(self.row(row, &theme));
            }
            body = body.child(list);
        }
        let total = count(&self.session, cx);
        let session = self.session.clone();
        let mut page = ui::page::Page::new("drafts-page", t!("drafts-title"), body)
            .description(t!("drafts-description"))
            .close(t!("drafts-return"), move |_, _, cx| {
                ui::page::leave(&session, cx)
            });
        if total > 0 {
            let meta = if total == 1 {
                t!("drafts-meta-one").to_string()
            } else {
                t!("drafts-meta", count = total)
            };
            page = page.meta("drafts-meta", meta);
        }
        page.render(&theme)
    }
}

fn text_button(
    id: &'static str,
    label: impl Into<SharedString>,
    theme: &Theme,
    on_click: impl Fn(&mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let hover = theme.hover;
    let label: SharedString = label.into();
    div()
        .id(id)
        .test_support()
        .role(Role::Button)
        .reachable(theme)
        .aria_label(label.clone())
        .px_2()
        .rounded_md()
        .cursor_pointer()
        .text_color(theme.link)
        .hover(move |d| d.bg(hover))
        .child(label)
        .on_click(move |_, window, cx| on_click(window, cx))
}

// ---- The row under the conversation list ----

fn footer(session: &Entity<Session>, cx: &mut App) -> Entity<FooterView> {
    if let Some(view) = &cx.global::<Drafts>().footer
        && view.read(cx).session == *session
    {
        return view.clone();
    }
    let view = cx.new(|cx| FooterView::new(session.clone(), cx));
    cx.global_mut::<Drafts>().footer = Some(view.clone());
    view
}

pub struct FooterView {
    session: Entity<Session>,
    _subscription: Subscription,
}

impl FooterView {
    fn new(session: Entity<Session>, cx: &mut Context<Self>) -> Self {
        let subscription = cx.subscribe(&session, |_, _, event: &SessionEvent, cx| {
            if redraws(event) {
                cx.notify();
            }
        });
        Self {
            session,
            _subscription: subscription,
        }
    }
}

impl Render for FooterView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme::get(cx).clone();
        let total = count(&self.session, cx);
        let active = self.session.read(cx).route() == Some(Route::Drafts);
        let label = t!("drafts-sidebar", count = total);
        let session = self.session.clone();
        let hover = theme.hover;
        div()
            .id("sidebar-drafts")
            .test_support()
            .role(Role::Button)
            .reachable(&theme)
            .aria_label(label.clone())
            .aria_selected(active)
            .mx_2()
            .px_2()
            .py_1()
            .rounded_md()
            .flex()
            .items_center()
            .gap_2()
            .text_sm()
            .cursor_pointer()
            .text_color(if active { theme.text } else { theme.muted })
            .when(active, |d| d.bg(theme.selected))
            .hover(move |d| d.bg(hover))
            .child(ui::icon::icon(IconName::FileText, theme.muted))
            .child(div().flex_1().truncate().child(t!("drafts-title")))
            .child(div().text_xs().child(total.to_string()))
            .on_click(move |_, _, cx| open(&session, cx))
    }
}

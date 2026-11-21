//! Saved messages and Save for later (package FD2).
//!
//! "Save for later" (and "Remove from saved") in the message menu keeps a
//! message on the Saved messages page, which only you can see: newest saved
//! first under day headings, with a filter on the text. Opening an item
//! jumps to it in its conversation; an edited or deleted source changes the
//! saved copy with it. The button in the conversation header opens it.
//! Follows the web client's `SavedPage.vue` and `queries/saved.ts`.

use gpui_kit::TestSupportExt;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::*;
use gpui_kit::*;
use ircord_client::contract::{MessageKind, SavedItem, SavedItemStatus};
use ircord_client::format;
use ircord_client::store::saved;
use ircord_client::store::{Connection, Workspace};

use crate::features::inbox::{
    author, day_and_time, day_heading, link_button, loading, more, network_name, place,
};
use crate::registry::{self, HeaderCtx, MenuItem, MessageCtx, Pane, Slot};
use crate::route::{PaneKind, Route};
use crate::session::{Session, SessionEvent};
use crate::theme::{self, Theme};
use crate::ui;
use crate::ui::icon::IconName;

pub fn init(cx: &mut App) {
    registry::register(cx, |r| {
        r.panes.add(Pane::new(PaneKind::Saved, |ctx, window, cx| {
            view(&ctx.session, window, cx).into_any_element()
        }));
        r.header_buttons.add(
            Slot::new("saved", |ctx: &HeaderCtx, _, cx| {
                let session = ctx.session.clone();
                if let Some(client) = session.read(cx).client() {
                    saved::keep(client);
                }
                Some(
                    ui::icon::button(
                        "open-saved",
                        IconName::BookOpen,
                        t!("inbox-saved-saved"),
                        theme::get(cx),
                        move |_, _, cx| open(&session, cx),
                    )
                    .into_any_element(),
                )
            })
            .order(81),
        );
        r.message_menu.add(
            MenuItem::new(
                "save-for-later",
                t!("inbox-saved-save-later"),
                |ctx: &MessageCtx, window: &mut Window, cx: &mut App| {
                    set_saved(
                        &ctx.session,
                        &ctx.conversation,
                        &ctx.message,
                        true,
                        window,
                        cx,
                    )
                },
            )
            .icon(IconName::BookOpen)
            .order(45)
            .when(|ctx: &MessageCtx, cx: &App| saveable(ctx, cx) == Some(false)),
        );
        r.message_menu.add(
            MenuItem::new(
                "remove-from-saved",
                t!("inbox-saved-remove"),
                |ctx: &MessageCtx, window: &mut Window, cx: &mut App| {
                    set_saved(
                        &ctx.session,
                        &ctx.conversation,
                        &ctx.message,
                        false,
                        window,
                        cx,
                    )
                },
            )
            .icon(IconName::BookOpen)
            .order(45)
            .when(|ctx: &MessageCtx, cx: &App| saveable(ctx, cx) == Some(true)),
        );
    });
}

/// Show the saved messages.
pub fn open(session: &Entity<Session>, cx: &mut App) {
    session.update(cx, |s, cx| s.navigate(Route::Saved, cx));
}

/// Whether the message is saved, or `None` when it cannot be (one still on
/// its way, a deleted one).
fn saveable(ctx: &MessageCtx, cx: &App) -> Option<bool> {
    let session = ctx.session.read(cx);
    let client = session.client()?;
    let w = client.read();
    let message = w.message(&ctx.conversation, &ctx.message)?;
    if message.deleted == Some(true) || w.outbox.iter().any(|p| p.client_id == ctx.message) {
        return None;
    }
    Some(w.saved.is_saved(&ctx.message))
}

/// Save a message for later, or remove it from the saved messages, saying
/// how that went.
pub fn set_saved(
    session: &Entity<Session>,
    conversation: &str,
    message: &str,
    save: bool,
    window: &mut Window,
    cx: &mut App,
) {
    let Some(client) = session.read(cx).client().cloned() else {
        return;
    };
    saved::keep(&client);
    if client.read().connection != Some(Connection::Live) {
        return ui::toast::info(window, cx, t!("inbox-saved-save-offline"));
    }
    let answer = saved::set_saved(client.handle(), conversation, message, save);
    window
        .spawn(cx, async move |cx| {
            let result = answer.await;
            cx.update(|window, cx| match result {
                Ok(Ok(())) if save => ui::toast::success(window, cx, t!("inbox-saved-saved-toast")),
                Ok(Ok(())) => ui::toast::success(window, cx, t!("inbox-saved-removed-toast")),
                Ok(Err(e)) => {
                    let text = if save {
                        t!("inbox-saved-save-failed", error = e)
                    } else {
                        t!("inbox-saved-remove-failed", error = e)
                    };
                    ui::toast::error(window, cx, text);
                }
                Err(_) => {}
            })
            .ok();
        })
        .detach();
}

// ---- The page ----

/// The saved messages page, kept across visits so the filter stays.
struct Page(Entity<SavedView>);
impl Global for Page {}

fn view(session: &Entity<Session>, window: &mut Window, cx: &mut App) -> Entity<SavedView> {
    if let Some(Page(view)) = cx.try_global::<Page>()
        && view.read(cx).session == *session
    {
        return view.clone();
    }
    let view = cx.new(|cx| SavedView::new(session.clone(), window, cx));
    cx.set_global(Page(view.clone()));
    view
}

pub struct SavedView {
    session: Entity<Session>,
    filter: Entity<InputState>,
    _subscriptions: Vec<Subscription>,
}

impl SavedView {
    fn new(session: Entity<Session>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder(t!("inbox-saved-filter")));
        let subscriptions = vec![
            cx.subscribe(&filter, |this, _, event: &InputEvent, cx| {
                if let InputEvent::Change = event {
                    this.reload(cx);
                }
            }),
            cx.subscribe(&session, |this, _, event: &SessionEvent, cx| match event {
                SessionEvent::Changed(changes)
                    if changes.touches(saved::TOPIC) || changes.sidebar =>
                {
                    this.ensure_loaded(cx);
                    cx.notify();
                }
                SessionEvent::Connection | SessionEvent::Route => {
                    this.ensure_loaded(cx);
                    cx.notify();
                }
                _ => {}
            }),
        ];
        let mut this = Self {
            session,
            filter,
            _subscriptions: subscriptions,
        };
        this.ensure_loaded(cx);
        this
    }

    fn query(&self, cx: &App) -> String {
        self.filter.read(cx).value().trim().to_string()
    }

    fn live(&self, cx: &App) -> Option<std::rc::Rc<ircord_client::Client>> {
        let session = self.session.read(cx);
        if session.route() != Some(Route::Saved) {
            return None;
        }
        let client = session.client()?;
        (client.read().connection == Some(Connection::Live)).then(|| client.clone())
    }

    /// Read the list if it is missing, out of date, or for another filter.
    fn ensure_loaded(&mut self, cx: &mut Context<Self>) {
        let Some(client) = self.live(cx) else {
            return;
        };
        saved::keep(&client);
        let query = self.query(cx);
        let wanted = {
            let w = client.read();
            let l = &w.saved.list;
            !l.loading && (l.stale || l.query != query || (!l.loaded && l.error.is_none()))
        };
        if wanted {
            saved::load(client.handle(), &query);
        }
    }

    /// The filter changed: read the list for it.
    fn reload(&mut self, cx: &mut Context<Self>) {
        if let Some(client) = self.live(cx) {
            saved::load(client.handle(), &self.query(cx));
        }
        cx.notify();
    }

    fn retry(&mut self, cx: &mut Context<Self>) {
        if let Some(client) = self.session.read(cx).client() {
            saved::load(client.handle(), &self.query(cx));
        }
    }

    fn load_more(&mut self, cx: &mut Context<Self>) {
        if let Some(client) = self.session.read(cx).client() {
            saved::load_more(client.handle());
        }
    }

    fn row(&self, w: &Workspace, item: &SavedItem, pending: bool, theme: &Theme) -> AnyElement {
        let id = item.message_id.clone();
        let gone = item.status == SavedItemStatus::Unavailable;
        let available = item.status == SavedItemStatus::Available;
        let message = item.message.as_ref();
        let place = place(w, &item.conversation_id, gone);
        let network = network_name(w, &item.network_id);
        let author = message
            .filter(|_| !gone)
            .map(|m| author(w, &m.sender_id, &item.network_id))
            .unwrap_or_default();
        let text: SharedString = match (item.status, message) {
            (SavedItemStatus::Available, Some(m)) => format::parse(&m.text).text.into(),
            (SavedItemStatus::Deleted, _) => t!("inbox-saved-deleted").into(),
            _ => t!("inbox-saved-unavailable").into(),
        };
        let action = message.is_some_and(|m| m.kind == MessageKind::Action);
        let label = t!(
            "inbox-saved-item",
            author = author.clone(),
            place = place.clone(),
            text = text.clone()
        );
        let (_, time) = day_and_time(message.map(|m| m.created_at.as_str()).unwrap_or(""));

        let (session, conversation, target) = (
            self.session.clone(),
            item.conversation_id.clone(),
            item.message_id.clone(),
        );
        let open = move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
            if gone {
                return ui::toast::info(window, cx, t!("inbox-saved-closed"));
            }
            crate::timeline::reveal(&session, &conversation, &target, window, cx);
        };
        let (session, conversation, target) = (
            self.session.clone(),
            item.conversation_id.clone(),
            item.message_id.clone(),
        );
        let remove = ui::icon::button(
            ElementId::Name(format!("saved-remove-{id}").into()),
            IconName::Close,
            t!("inbox-saved-remove"),
            theme,
            move |_, window, cx| {
                cx.stop_propagation();
                if !pending {
                    set_saved(&session, &conversation, &target, false, window, cx);
                }
            },
        );

        div()
            .id(ElementId::Name(format!("saved-row-{id}").into()))
            .flex()
            .items_start()
            .gap_2()
            .px_2()
            .rounded_md()
            .hover(|d| d.bg(theme.row_hover))
            .child(
                div()
                    .id(ElementId::Name(format!("saved-item-{id}").into()))
                    .test_support()
                    .role(Role::Button)
                    .aria_label(SharedString::from(label))
                    .flex_1()
                    .min_w_0()
                    .py_1p5()
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .cursor_pointer()
                    .on_click(open)
                    .child(
                        div()
                            .flex()
                            .gap_1p5()
                            .text_xs()
                            .text_color(theme.muted)
                            .child(div().truncate().child(format!("{place} · {network}")))
                            .child(div().flex_1())
                            .child(div().text_color(theme.faint).child(time)),
                    )
                    .child(
                        div()
                            .flex()
                            .gap_1()
                            .text_sm()
                            .min_w_0()
                            .when(!author.is_empty(), |d| {
                                d.child(
                                    div()
                                        .flex_none()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(theme.text)
                                        .child(author.clone()),
                                )
                            })
                            .child(
                                div()
                                    .truncate()
                                    .text_color(if available { theme.text } else { theme.muted })
                                    .when(!available || action, |d| d.italic())
                                    .child(text),
                            ),
                    ),
            )
            .child(div().pt_1().child(remove))
            .into_any_element()
    }
}

impl Render for SavedView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme::get(cx).clone();
        let Some(client) = self.session.read(cx).client().cloned() else {
            return div().into_any_element();
        };
        let query = self.query(cx);
        let w = client.read();
        let state = &w.saved;
        let l = &state.list;
        let count = l.items.len();
        let meta: SharedString = if l.next.is_some() {
            t!("inbox-saved-saved-count", count = format!("{count}+")).into()
        } else {
            t!("inbox-saved-saved-count", count = count).into()
        };

        let body: AnyElement = if !l.loaded && l.error.is_some() {
            let text = t!(
                "inbox-saved-saved-failed",
                error = l.error.clone().unwrap_or_default()
            );
            div()
                .id("saved-failed")
                .test_support()
                .role(Role::Alert)
                .aria_label(SharedString::from(text.clone()))
                .p_8()
                .flex()
                .flex_col()
                .items_center()
                .gap_2()
                .text_color(theme.muted)
                .child(text)
                .child(link_button(
                    "saved-retry",
                    t!("inbox-saved-try-again"),
                    &theme,
                    cx.listener(|this, _, _, cx| this.retry(cx)),
                ))
                .into_any_element()
        } else if !l.loaded || l.query != query {
            loading("saved-loading", t!("inbox-saved-saved-loading"), &theme)
        } else if l.items.is_empty() {
            if l.query.is_empty() {
                ui::empty::empty(
                    "saved-empty",
                    t!("inbox-saved-empty-title"),
                    Some(t!("inbox-saved-empty").into()),
                    &theme,
                )
                .into_any_element()
            } else {
                ui::empty::empty(
                    "saved-empty",
                    t!("inbox-saved-no-match", query = l.query.clone()),
                    None,
                    &theme,
                )
                .into_any_element()
            }
        } else {
            let mut groups: Vec<(String, Vec<AnyElement>)> = Vec::new();
            for item in &l.items {
                let (day, _) = day_and_time(&item.saved_at);
                let row = self.row(&w, item, state.pending.contains(&item.message_id), &theme);
                match groups.last_mut() {
                    Some((last, rows)) if *last == day => rows.push(row),
                    _ => groups.push((day, vec![row])),
                }
            }
            let more = more(
                "saved-load-more",
                t!("inbox-saved-load-older-saved"),
                l.next.is_some(),
                l.loading_more,
                l.more_error.as_deref(),
                &theme,
                cx.listener(|this, _, _, cx| this.load_more(cx)),
            );
            div()
                .id("saved-list")
                .test_support()
                .role(Role::List)
                .aria_label(t!("inbox-saved-saved"))
                .flex()
                .flex_col()
                .gap_3()
                .children(groups.into_iter().map(|(day, rows)| {
                    div()
                        .flex()
                        .flex_col()
                        .gap_0p5()
                        .child(day_heading(day, &theme))
                        .children(rows)
                }))
                .children(more)
                .into_any_element()
        };
        drop(w);

        let page_body = div()
            .px_4()
            .py_3()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .text_sm()
                    .text_color(theme.muted)
                    .child(t!("inbox-saved-saved-description")),
            )
            .child(
                Input::new(&self.filter)
                    .id("saved-filter")
                    .aria_label(t!("inbox-saved-filter")),
            )
            .child(body);
        ui::page::Page::new("saved-page", t!("inbox-saved-saved"), page_body)
            .action(
                div()
                    .id("saved-meta")
                    .test_support()
                    .role(Role::Status)
                    .aria_label(meta.clone())
                    .text_sm()
                    .text_color(theme.muted)
                    .child(meta),
            )
            .render(&theme)
            .into_any_element()
    }
}

//! Tests for package D1: drafts kept per conversation and the drafts page.

use std::sync::Arc;

use gpui_kit::test::{TestQueryExt, TestWindowExt};
use gpui_kit::{AppContext, Bounds, Entity, Point, TestAppContext, WindowBounds, WindowOptions};
use gpui_kit::{px, size};
use ircord_app::MainView;
use ircord_app::features::message_actions;
use ircord_app::route::Route;
use ircord_app::session::Session;
use ircord_client::credentials::{CredentialStore, MemoryStore};
use ircord_client::store::drafts;
use ircord_client::testing::CANNED_COOKIE;

use crate::fixture::{Fixture, bootstrap, open, wait};

fn type_in_box(f: &Fixture, cx: &mut TestAppContext, text: &str) {
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.click("composer-input", cx);
        window.input(text, cx);
    })
    .unwrap();
    cx.run_until_parked();
}

fn select(session: &Entity<Session>, cx: &mut TestAppContext, id: &str) {
    session.update(cx, |s, cx| s.select_conversation(id, cx));
    cx.run_until_parked();
}

fn stored(session: &Entity<Session>, cx: &mut TestAppContext, id: &str) -> String {
    cx.update(|cx| {
        let s = session.read(cx);
        s.client().unwrap().read().drafts.text(id).to_string()
    })
}

fn render(f: &Fixture, cx: &mut TestAppContext) {
    cx.update_window(f.window, |_, window, cx| window.render_frame(cx))
        .unwrap();
}

#[gpui_kit::test]
fn a_draft_survives_switching_conversations(cx: &mut TestAppContext) {
    let f = open(cx, true);
    bootstrap(&f, cx);
    type_in_box(&f, cx, "half a thought");
    assert_eq!(stored(&f.session, cx, "c-general"), "half a thought");

    select(&f.session, cx, "c-random");
    render(&f, cx);
    cx.update(|cx| assert_eq!(message_actions::box_text(cx), "", "#random has no draft"));
    type_in_box(&f, cx, "other");

    select(&f.session, cx, "c-general");
    render(&f, cx);
    cx.update(|cx| assert_eq!(message_actions::box_text(cx), "half a thought"));
    assert_eq!(stored(&f.session, cx, "c-random"), "other");

    // Sending clears the draft.
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.press("enter", cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(stored(&f.session, cx, "c-general"), "");
}

#[gpui_kit::test]
fn a_draft_survives_a_restart(cx: &mut TestAppContext) {
    let f = open(cx, true);
    bootstrap(&f, cx);
    type_in_box(&f, cx, "kept for later");
    let (config, runtime, key) = cx.update(|cx| {
        let s = f.session.read(cx);
        let origin = f.server.address().origin();
        let key = drafts::account_key(&origin, &s.client().unwrap().read());
        (s.config.clone(), s.runtime.clone(), key)
    });
    cx.executor()
        .advance_clock(std::time::Duration::from_secs(1));
    wait(cx, "the draft on disk", |_| {
        drafts::read(&config, &key)
            .drafts
            .get("c-general")
            .map(String::as_str)
            == Some("kept for later")
    });

    // A new session and window over the same device state: the app again.
    let credentials: Arc<dyn CredentialStore> = Arc::new(MemoryStore::default());
    credentials
        .save(&f.server.address(), CANNED_COOKIE)
        .unwrap();
    let session = cx.new(|cx| Session::new(runtime, config.clone(), credentials, cx));
    let (window, _view) = cx.update(|cx| {
        gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: Point::default(),
                    size: size(px(1200.), px(800.)),
                })),
                ..Default::default()
            },
            cx,
            |window, cx| cx.new(|cx| MainView::new(session.clone(), window, cx)),
        )
        .unwrap()
    });
    wait(cx, "the second bootstrap with the draft read in", |cx| {
        let s = session.read(cx);
        s.conversation.as_deref() == Some("c-general")
            && s.client()
                .is_some_and(|c| c.read().drafts.text("c-general") == "kept for later")
    });
    cx.update_window(window, |_, window, cx| window.render_frame(cx))
        .unwrap();
    cx.run_until_parked();
    cx.update_window(window, |_, window, cx| window.render_frame(cx))
        .unwrap();
    cx.update(|cx| assert_eq!(message_actions::box_text(cx), "kept for later"));
}

#[gpui_kit::test]
fn the_page_lists_a_draft_and_opens_it(cx: &mut TestAppContext) {
    let f = open(cx, true);
    bootstrap(&f, cx);
    type_in_box(&f, cx, "about the release");
    select(&f.session, cx, "c-random");
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.find("composer-input").visible());
        window.click("sidebar-drafts", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update(|cx| assert_eq!(f.session.read(cx).route(), Some(Route::Drafts)));
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.find("drafts-page").visible());
        assert_eq!(window.find("drafts-meta").label(), Some("1 unsent draft"));
        assert!(window.find("drafts-page-close").visible());
        assert!(window.find_by_label("#general on Home").visible());
        window.click("draft-open-c-general", cx);
    })
    .unwrap();
    cx.run_until_parked();
    render(&f, cx);
    cx.update(|cx| {
        assert_eq!(
            f.session.read(cx).route(),
            Some(Route::Conversation("c-general".into()))
        );
        assert_eq!(message_actions::box_text(cx), "about the release");
    });
}

#[gpui_kit::test]
fn the_outbox_can_be_opened_without_a_draft(cx: &mut TestAppContext) {
    use ircord_client::store::{PendingSend, SendState};

    let f = open(cx, true);
    bootstrap(&f, cx);
    cx.update(|cx| {
        let client = f.session.read(cx).client().unwrap();
        client.change(|w| {
            w.queue_send(PendingSend {
                client_id: "failed-1".into(),
                conversation_id: "c-random".into(),
                text: "Still on my device".into(),
                action: false,
                state: SendState::Failed("Could not send".into()),
                created_ms: 0,
                ..Default::default()
            })
        });
    });
    cx.run_until_parked();
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find("sidebar-drafts").label(),
            Some("Drafts and outbox, 1 unsent")
        );
        window.click("sidebar-drafts", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.find("outbox-heading").visible());
        assert!(window.find("outbox-retry-failed-1").visible());
        window.click("outbox-open-failed-1", cx);
    })
    .unwrap();
    cx.update(|cx| {
        assert_eq!(f.session.read(cx).conversation.as_deref(), Some("c-random"));
    });
}

#[gpui_kit::test]
fn discard_from_the_page_and_undo(cx: &mut TestAppContext) {
    let f = open(cx, true);
    bootstrap(&f, cx);
    type_in_box(&f, cx, "maybe not");
    f.session.update(cx, |s, cx| s.navigate(Route::Drafts, cx));
    cx.run_until_parked();
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.click("draft-discard-c-general", cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(stored(&f.session, cx, "c-general"), "");
    cx.update(|cx| assert_eq!(message_actions::box_text(cx), "", "the box let go too"));
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.find("drafts-empty").visible());
        assert!(window.find_by_label("Draft discarded.").visible());
        window.click("drafts-undo", cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(stored(&f.session, cx, "c-general"), "maybe not");
    render(&f, cx);
    cx.update(|cx| assert_eq!(message_actions::box_text(cx), "maybe not"));
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.find("draft-c-general").visible());
        assert!(window.try_find("drafts-notice").is_none());
    })
    .unwrap();
}

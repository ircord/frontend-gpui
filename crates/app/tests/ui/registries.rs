//! The registration points, driven by a sample feature: what it registers
//! shows up in the window and works.

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::TestSupportExt;
use gpui_kit::prelude::*;
use gpui_kit::test::{TestEventExt, TestQueryExt, TestWindowExt};
use gpui_kit::{App, MouseButton, TestAppContext, div};
use ircord_app::registry::{
    self, MenuItem, MessageCtx, Pane, Panel, RowFilter, SendFilter, SendOutcome, SettingsSection,
    SidebarCtx, Slot,
};
use ircord_app::route::{PaneKind, Route};
use ircord_app::timeline::rows::Row;
use ircord_app::ui::icon::IconName;

use crate::fixture::{bootstrap, open, open_with};
use ircord_client::testing::Reply;

/// What the sample feature saw happen.
#[derive(Default)]
struct Seen {
    menu: Cell<u32>,
    sent: Cell<u32>,
}

fn sample_feature(cx: &mut App, seen: Rc<Seen>) {
    let menu_seen = seen.clone();
    registry::register(cx, |r| {
        r.message_menu.add(
            MenuItem::new("sample", "Sample action", move |m: &MessageCtx, _, _| {
                assert_eq!(m.conversation, "c-general");
                menu_seen.menu.set(menu_seen.menu.get() + 1);
            })
            .icon(IconName::Star)
            .toolbar(),
        );
        r.message_menu.add(
            MenuItem::new("own-only", "Only on my messages", |_: &MessageCtx, _, _| {})
                .when(|m, _| m.own),
        );
        r.settings_sections.add(SettingsSection::new(
            "sample",
            "Sample section",
            |_, _, _| {
                div()
                    .id("sample-settings-body")
                    .test_support()
                    .child("sample")
                    .into_any_element()
            },
        ));
        r.composer.buttons.add(Slot::new("sample", |_, _, _| {
            Some(
                div()
                    .id("sample-composer-button")
                    .test_support()
                    .child("S")
                    .into_any_element(),
            )
        }));
        r.composer.above.add(Slot::new("sample", |_, _, _| {
            Some(
                div()
                    .id("sample-composer-bar")
                    .test_support()
                    .child("bar")
                    .into_any_element(),
            )
        }));
        let send_seen = seen.clone();
        r.composer.send_filters.add(SendFilter {
            id: "sample".into(),
            order: 0,
            filter: Rc::new(move |_, text, _, _| {
                if text == "/sample" {
                    send_seen.sent.set(send_seen.sent.get() + 1);
                    SendOutcome::Handled
                } else {
                    SendOutcome::Continue(text.to_uppercase())
                }
            }),
        });
        r.header_buttons.add(Slot::new("sample", |_, _, _| {
            Some(
                div()
                    .id("sample-header-button")
                    .test_support()
                    .child("H")
                    .into_any_element(),
            )
        }));
        r.sidebar_footer
            .add(Slot::new("sample", |ctx: &SidebarCtx, _, _| {
                Some(
                    div()
                        .id("sample-sidebar-slot")
                        .test_support()
                        .child(ctx.network.clone().unwrap_or_default())
                        .into_any_element(),
                )
            }));
        r.row_filters.add(RowFilter {
            id: "sample".into(),
            order: 0,
            keep: Rc::new(|row, _, _| !matches!(row, Row::Message(m) if m.id == "2")),
        });
        r.panes.add(Pane::new(PaneKind::Search, |_, _, _| {
            div()
                .id("sample-search-pane")
                .test_support()
                .child("sample")
                .into_any_element()
        }));
        r.panels.add(Panel::new("sample", |_, _, _| {
            div()
                .id("sample-panel")
                .test_support()
                .child("sample")
                .into_any_element()
        }));
    });
}

#[gpui_kit::test]
fn a_sample_feature_shows_in_the_window(cx: &mut TestAppContext) {
    let seen = Rc::new(Seen::default());
    let f = open(cx, true);
    cx.update(|cx| sample_feature(cx, seen.clone()));
    bootstrap(&f, cx);
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        // Slots.
        assert!(window.find("sample-composer-button").visible());
        assert!(window.find("sample-composer-bar").visible());
        assert!(window.find("sample-header-button").visible());
        assert!(window.find("sample-sidebar-slot").visible());
        // The row filter hid message 2 and kept message 1.
        assert!(window.find("message-1").visible());
        assert!(window.try_find("message-2").is_none());
        // The hover toolbar shows toolbar items for the message under the pointer.
        window.hover("message-1", cx);
        assert!(window.find("message-toolbar-sample").visible());
        window.click("message-toolbar-sample", cx);
    })
    .unwrap();
    assert_eq!(seen.menu.get(), 1);
    cx.update_window(f.window, |_, window, cx| {
        // The message menu, on right click; `own-only` is hidden on bob's message.
        window.right_click("message-1", cx);
        window.render_frame(cx);
        assert!(window.find_by_label("Sample action").visible());
        assert!(window.try_find_by_label("Only on my messages").is_none());
        let item = window.find_by_label("Sample action").bounds().center();
        window.pointer_down(item, MouseButton::Left, cx);
        window.pointer_up(item, MouseButton::Left, cx);
    })
    .unwrap();
    assert_eq!(seen.menu.get(), 2);
}

#[gpui_kit::test]
fn send_filters_run_before_sending(cx: &mut TestAppContext) {
    let seen = Rc::new(Seen::default());
    let f = open(cx, true);
    cx.update(|cx| sample_feature(cx, seen.clone()));
    bootstrap(&f, cx);
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.click("composer-input", cx);
        window.input("/sample", cx);
        window.press("enter", cx);
    })
    .unwrap();
    // Each Enter is handled once the window update ends.
    cx.run_until_parked();
    cx.update_window(f.window, |_, window, cx| {
        window.input("quiet", cx);
        window.press("enter", cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(seen.sent.get(), 1, "the filter handled /sample");
    cx.update(|cx| {
        let session = f.session.read(cx);
        let w = session.client().unwrap().read();
        let pending: Vec<_> = w
            .pending_in("c-general")
            .iter()
            .map(|p| p.text.clone())
            .collect();
        assert_eq!(
            pending,
            ["QUIET"],
            "the filter changed the text it passed on"
        );
    });
}

#[gpui_kit::test]
fn settings_panes_panels_and_history(cx: &mut TestAppContext) {
    let f = open(cx, true);
    cx.update(|cx| sample_feature(cx, Rc::default()));
    bootstrap(&f, cx);
    f.session
        .update(cx, |s, cx| s.navigate(Route::Settings, cx));
    cx.run_until_parked();
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.find("settings-section-sample").visible());
        // Account comes first; the sample is under "App settings".
        window.click("settings-section-sample", cx);
        window.render_frame(cx);
        assert!(window.find("sample-settings-body").visible());
        assert!(
            window.try_find("timeline").is_none(),
            "the settings page replaced it"
        );
    })
    .unwrap();

    f.session.update(cx, |s, cx| s.navigate(Route::Search, cx));
    f.session
        .update(cx, |s, cx| s.set_panel(Some("sample".into()), cx));
    cx.run_until_parked();
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.find("sample-search-pane").visible());
        assert!(window.find("sample-panel").visible());
        assert!(window.try_find("member-list").is_none());
    })
    .unwrap();

    // Back through search and settings to the conversation.
    f.session.update(cx, |s, cx| {
        s.set_panel(None, cx);
        s.go_back(cx);
        s.go_back(cx);
    });
    cx.run_until_parked();
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.find("timeline").visible());
        assert!(window.find("member-list").visible());
    })
    .unwrap();
    cx.update(|cx| {
        assert_eq!(
            f.session.read(cx).route(),
            Some(Route::Conversation("c-general".into()))
        );
    });
}

#[gpui_kit::test]
fn a_route_without_a_pane_says_so(cx: &mut TestAppContext) {
    let f = open(cx, true);
    bootstrap(&f, cx);
    f.session.update(cx, |s, cx| s.navigate(Route::Inbox, cx));
    cx.run_until_parked();
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.find("pane-missing").visible());
    })
    .unwrap();
}

#[gpui_kit::test]
fn the_loading_screen_shows_until_the_first_bootstrap(cx: &mut TestAppContext) {
    // The bootstrap answers only once the test lets it.
    let (release, gate) = std::sync::mpsc::channel::<()>();
    let gate = std::sync::Mutex::new(gate);
    let f = open_with(cx, true, |server| {
        let snapshot = serde_json::to_value(crate::fixture::snapshot()).unwrap();
        server.route_fn("GET", "/api/v1/bootstrap", move |_| {
            let _ = gate.lock().unwrap().recv();
            Reply::json(200, snapshot.clone())
        });
    });
    cx.run_until_parked();
    cx.update(|cx| {
        assert!(matches!(
            f.view.read(cx).screen(),
            ircord_app::Screen::Loading(_)
        ));
    });
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.find("loading-status").visible());
        assert!(window.find("loading-sign-out").visible());
    })
    .unwrap();
    release.send(()).unwrap();
    bootstrap(&f, cx);
    cx.update(|cx| {
        let screen = match f.view.read(cx).screen() {
            ircord_app::Screen::SignIn(_) => "sign-in",
            ircord_app::Screen::Loading(_) => "loading",
            ircord_app::Screen::Workspace(_) => "workspace",
        };
        assert_eq!(screen, "workspace");
    });
}

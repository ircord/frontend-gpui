//! The parity audit (Q1): every view's controls have a role and a name and
//! are reached with Tab, and new messages are announced once per pace.

use std::time::{Duration, Instant};

use gpui_kit::test::{TestQueryExt, TestWindowExt};
use gpui_kit::{AppContext, ElementId, Role, TestAppContext, Window};
use ircord_app::Screen;
use ircord_app::features::{admin, network_form};
use ircord_app::route::Route;
use ircord_client::testing::frame;

use crate::fixture::{Fixture, bootstrap, message, open, wait};

/// Roles a person operates. List options and menu items are left out: their
/// list or menu holds focus and the arrow keys move through them.
const OPERATED: &[Role] = &[
    Role::Button,
    Role::Link,
    Role::Tab,
    Role::CheckBox,
    Role::Switch,
    Role::RadioButton,
    Role::TextInput,
    Role::MultilineTextInput,
    Role::SearchInput,
    Role::ComboBox,
    Role::Slider,
];

fn path_text(path: &[ElementId]) -> String {
    path.iter()
        .filter_map(|id| match id {
            ElementId::Name(name) => Some(name.to_string()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// The deepest element that holds the keyboard focus.
fn focused(window: &Window) -> Option<Vec<ElementId>> {
    window
        .elements()
        .into_iter()
        .filter(|e| e.focused() == Some(true))
        .max_by_key(|e| e.path().len())
        .map(|e| e.path().to_vec())
}

/// Check the view on screen: each control it shows has a role and a name
/// and can hold focus, and Tab reaches each of them. Returns what is wrong.
pub(crate) fn audit(view: &str, window: &mut Window, cx: &mut gpui_kit::App) -> Vec<String> {
    audit_where(view, window, cx, |_| true)
}

/// Whether an element is drawn inside an open dialog.
fn in_dialog(path: &[ElementId]) -> bool {
    path.iter()
        .any(|id| matches!(id, ElementId::Name(n) if n.starts_with("dialog-")))
}

/// [`audit`] for the controls `inside` accepts, such as a dialog's: what
/// is behind a dialog is out of the keyboard's reach while it is open.
fn audit_where(
    view: &str,
    window: &mut Window,
    cx: &mut gpui_kit::App,
    inside: impl Fn(&[ElementId]) -> bool,
) -> Vec<String> {
    window.render_frame(cx);
    let mut problems = Vec::new();
    let mut required = Vec::new();
    for e in window.elements() {
        if !e.visible() || !inside(e.path()) {
            continue;
        }
        let Some(role) = e.role() else { continue };
        if !OPERATED.contains(&role) {
            continue;
        }
        let name = path_text(e.path());
        if e.label().is_none_or(str::is_empty) {
            problems.push(format!("{view}: {role:?} {name} has no name"));
        }
        // A disabled control is passed over until it can be used.
        if e.disabled() == Some(true) {
            continue;
        }
        match e.focused() {
            None => problems.push(format!("{view}: {role:?} {name} cannot take focus")),
            Some(_) => required.push(e.path().to_vec()),
        }
    }
    // Tab through the view and see what holds focus each time. Outside a
    // dialog the order runs through the whole window, so the walk is as
    // long as everything that can take focus.
    let stops = window
        .elements()
        .iter()
        .filter(|e| e.visible() && e.focused().is_some())
        .count();
    let mut waiting: Vec<Vec<ElementId>> = required;
    for _ in 0..stops * 2 + 4 {
        if waiting.is_empty() {
            break;
        }
        window.press("tab", cx);
        if let Some(path) = focused(window) {
            waiting.retain(|w| !path.starts_with(w));
        }
    }
    for path in waiting {
        problems.push(format!("{view}: Tab never reaches {}", path_text(&path)));
    }
    problems
}

fn audit_route(f: &Fixture, cx: &mut TestAppContext, view: &str, route: Route) -> Vec<String> {
    f.session.update(cx, |s, cx| s.navigate(route, cx));
    cx.run_until_parked();
    cx.update_window(f.window, |_, window, cx| audit(view, window, cx))
        .unwrap()
}

#[gpui_kit::test]
fn tab_reaches_every_control_in_every_view(cx: &mut TestAppContext) {
    let f = open(cx, true);
    bootstrap(&f, cx);
    let mut problems = Vec::new();
    for (view, route) in [
        ("conversation", Route::Conversation("c-general".into())),
        ("server window", Route::ServerWindow("n1".into())),
        ("search", Route::Search),
        ("inbox", Route::Inbox),
        ("saved", Route::Saved),
        ("drafts", Route::Drafts),
        ("links", Route::Links(None)),
        (
            "network set-up",
            Route::Page(network_form::SETUP_PAGE.into()),
        ),
        ("administration", Route::Page(admin::PAGE.into())),
    ] {
        problems.extend(audit_route(&f, cx, view, route));
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

/// Settings, one section at a time: the sections whose place in the list
/// is `part` modulo `parts`, so the sections are checked in parallel tests.
fn audit_settings(cx: &mut TestAppContext, part: usize, parts: usize) {
    let f = open(cx, true);
    bootstrap(&f, cx);
    f.session
        .update(cx, |s, cx| s.navigate(Route::Settings, cx));
    cx.run_until_parked();
    let sections: Vec<String> = cx
        .update_window(f.window, |_, window, cx| {
            window.render_frame(cx);
            window
                .elements()
                .into_iter()
                .filter_map(|e| match e.path().last() {
                    Some(ElementId::Name(n)) if n.starts_with("settings-section-") => {
                        Some(n.to_string())
                    }
                    _ => None,
                })
                .collect()
        })
        .unwrap();
    assert!(sections.len() >= 10, "settings sections: {sections:?}");
    let mut problems = Vec::new();
    for section in sections.into_iter().skip(part).step_by(parts) {
        let id: &'static str = Box::leak(section.clone().into_boxed_str());
        cx.update_window(f.window, |_, window, cx| {
            window.render_frame(cx);
            assert!(window.find(id).visible(), "{id} is out of view");
            window.click(id, cx);
        })
        .unwrap();
        cx.run_until_parked();
        problems.extend(
            cx.update_window(f.window, |_, window, cx| audit(&section, window, cx))
                .unwrap(),
        );
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

#[gpui_kit::test]
fn tab_reaches_every_control_in_settings_1(cx: &mut TestAppContext) {
    audit_settings(cx, 0, 3);
}

#[gpui_kit::test]
fn tab_reaches_every_control_in_settings_2(cx: &mut TestAppContext) {
    audit_settings(cx, 1, 3);
}

#[gpui_kit::test]
fn tab_reaches_every_control_in_settings_3(cx: &mut TestAppContext) {
    audit_settings(cx, 2, 3);
}

#[gpui_kit::test]
fn tab_reaches_every_control_on_the_sign_in_screen(cx: &mut TestAppContext) {
    let f = open(cx, false);
    cx.run_until_parked();
    let problems = cx
        .update_window(f.window, |_, window, cx| audit("sign-in", window, cx))
        .unwrap();
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

fn announcement(f: &Fixture, cx: &mut TestAppContext) -> String {
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window
            .find("timeline-announcement")
            .label()
            .unwrap_or_default()
            .to_string()
    })
    .unwrap()
}

#[gpui_kit::test]
fn new_messages_are_announced_once_per_pace(cx: &mut TestAppContext) {
    let f = open(cx, true);
    bootstrap(&f, cx);
    // The history that was there is not news.
    assert_eq!(announcement(&f, cx), "");
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        let live = window.find("timeline-announcement");
        assert_eq!(live.role(), Some(Role::Status));
        let log = window.find("timeline");
        assert_eq!(log.role(), Some(Role::Log));
        assert_eq!(window.find("message-1").role(), Some(Role::Article));
        assert!(
            window
                .find("message-1")
                .label()
                .is_some_and(|l| l.starts_with("bob, "))
        );
    })
    .unwrap();

    f.server.push(frame(
        4,
        "message.created",
        &message("3", "c-general", "are you \x02there\x02?"),
    ));
    wait(cx, "the first announcement", |cx| {
        match f.view.read(cx).screen() {
            Screen::Workspace(view) => {
                view.read(cx).timeline.read(cx).announcement() == "bob: are you there?"
            }
            _ => false,
        }
    });

    // Two more right away wait for the pace, then are read as one.
    f.server.push(frame(
        5,
        "message.created",
        &message("4", "c-general", "one"),
    ));
    f.server.push(frame(
        6,
        "message.created",
        &message("5", "c-general", "two"),
    ));
    wait(cx, "both messages", |cx| {
        let session = f.session.read(cx);
        let w = session.client().unwrap().read();
        w.timelines["c-general"].entries.len() == 5
    });
    assert_eq!(announcement(&f, cx), "bob: are you there?");
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        cx.run_until_parked();
        if announcement(&f, cx) == "2 new messages, latest from bob: two" {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the paced announcement never came"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[gpui_kit::test]
fn tab_reaches_every_control_in_the_dialogs(cx: &mut TestAppContext) {
    let f = open(cx, true);
    bootstrap(&f, cx);
    let session = f.session.clone();
    type Opener = Box<dyn Fn(&mut Window, &mut gpui_kit::App)>;
    let click = |id: &'static str| -> Opener { Box::new(move |window, cx| window.click(id, cx)) };
    let dialogs: Vec<(&str, Opener)> = vec![
        ("join a channel", click("section-channels-join")),
        ("message someone", click("section-directs-new")),
        ("find a conversation", click("sidebar-find")),
        (
            "keyboard shortcuts",
            Box::new(ircord_app::features::shortcuts::open_help),
        ),
        ("add a network", {
            let session = session.clone();
            Box::new(move |window, cx| network_form::open_add(&session, window, cx))
        }),
        ("create a channel", {
            let session = session.clone();
            Box::new(move |window, cx| {
                ircord_app::features::create_channel::open(&session, "n1", window, cx)
            })
        }),
    ];
    let mut problems = Vec::new();
    for (view, opener) in dialogs {
        cx.update_window(f.window, |_, window, cx| {
            window.render_frame(cx);
            opener(window, cx);
        })
        .unwrap();
        cx.run_until_parked();
        problems.extend(
            cx.update_window(f.window, |_, window, cx| {
                window.render_frame(cx);
                assert!(
                    window.elements().iter().any(|e| in_dialog(e.path())),
                    "{view}: no dialog opened"
                );
                audit_where(view, window, cx, in_dialog)
            })
            .unwrap(),
        );
        cx.update_window(f.window, |_, window, cx| {
            ircord_app::ui::dialog::close(window, cx)
        })
        .unwrap();
        cx.run_until_parked();
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

/// Whether an element is drawn inside the element named `scope`.
fn under(scope: &'static str) -> impl Fn(&[ElementId]) -> bool {
    move |path| {
        path.iter()
            .any(|id| matches!(id, ElementId::Name(n) if n.as_ref() == scope))
    }
}

#[gpui_kit::test]
fn tab_reaches_every_control_in_the_popovers(cx: &mut TestAppContext) {
    let f = crate::fixture::open_with(cx, true, |server| {
        server.route(
            "GET",
            "/api/v1/networks/n1/whois",
            200,
            serde_json::json!({"found": false, "nickname": "bob", "channels": [],
                               "secure": false, "operator": false}),
        );
    });
    bootstrap(&f, cx);
    let mut problems = Vec::new();
    for (view, opener, scope) in [
        ("profile card", "sender-1", "profile-card"),
        ("emoji picker", "composer-emoji", "expression-picker"),
    ] {
        cx.update_window(f.window, |_, window, cx| {
            window.render_frame(cx);
            window.click(opener, cx);
        })
        .unwrap();
        wait(cx, view, |_| true);
        problems.extend(
            cx.update_window(f.window, |_, window, cx| {
                window.render_frame(cx);
                assert!(window.try_find(scope).is_some(), "{view} did not open");
                audit_where(view, window, cx, under(scope))
            })
            .unwrap(),
        );
        cx.update_window(f.window, |_, window, cx| {
            window.press("escape", cx);
            window.render_frame(cx);
        })
        .unwrap();
        cx.run_until_parked();
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

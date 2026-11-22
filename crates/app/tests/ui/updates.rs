//! Tests for package B2: checking for a newer IRCord from Settings → About
//! and at the first check of a start, against a fake updater. The platform's
//! own check is tested in `ircord-platform`; the install itself needs a
//! person on each platform.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Result, anyhow};
use gpui_kit::test::{TestEventExt, TestQueryExt, TestWindowExt};
use gpui_kit::{AnyWindowHandle, App, AppContext, MouseButton, TestAppContext};
use ircord_app::features::updates::use_updater;
use ircord_app::route::Route;
use ircord_platform::update::{Checked, Update, Updater};

use crate::fixture::{Fixture, bootstrap, open, wait};

/// Answers each check with what the test last set, and records installs.
struct Fake {
    answer: Mutex<std::result::Result<Checked, String>>,
    installs: Mutex<Vec<String>>,
    checks: Mutex<usize>,
    /// While set, a check waits before it answers: the Checking state shows.
    held: AtomicBool,
}

impl Fake {
    fn answering(answer: std::result::Result<Checked, String>) -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self {
            answer: Mutex::new(answer),
            installs: Mutex::new(Vec::new()),
            checks: Mutex::new(0),
            held: AtomicBool::new(false),
        })
    }

    fn set(&self, answer: std::result::Result<Checked, String>) {
        *self.answer.lock().unwrap() = answer;
    }

    fn hold(&self, held: bool) {
        self.held.store(held, Ordering::SeqCst);
    }

    fn checks(&self) -> usize {
        *self.checks.lock().unwrap()
    }

    fn installed(&self) -> Vec<String> {
        self.installs.lock().unwrap().clone()
    }
}

impl Updater for Fake {
    fn check(&self) -> Result<Checked> {
        *self.checks.lock().unwrap() += 1;
        while self.held.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(5));
        }
        self.answer.lock().unwrap().clone().map_err(|e| anyhow!(e))
    }

    fn install(&self, update: &Update) -> Result<()> {
        self.installs
            .lock()
            .unwrap()
            .push(update.version.to_string());
        Ok(())
    }
}

fn release(version: &str) -> Checked {
    Checked::Available(Update {
        version: version.parse().unwrap(),
    })
}

fn route(f: &Fixture, cx: &mut TestAppContext) -> Option<Route> {
    cx.update(|cx| f.session.read(cx).route())
}

/// Draw the window and click the element with `id`.
fn click(f: &Fixture, cx: &mut TestAppContext, id: &'static str) {
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.click(id, cx);
    })
    .unwrap();
}

/// Draw the window and press at the centre of the element labelled `label`,
/// as a person does (dialog buttons have no ID of their own).
fn press_label(f: &Fixture, cx: &mut TestAppContext, label: &str) {
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        let at = window.find_by_label(label).bounds().center();
        window.pointer_down(at, MouseButton::Left, cx);
        window.pointer_up(at, MouseButton::Left, cx);
    })
    .unwrap();
}

/// Whether an element labelled `label` is drawn in `window` (a dialog's
/// button, say).
fn label_shown(window: AnyWindowHandle, cx: &mut App, label: &str) -> bool {
    window
        .update(cx, |_, window, cx| {
            window.render_frame(cx);
            window.try_find_by_label(label).is_some()
        })
        .unwrap_or(false)
}

fn shows(f: &Fixture, cx: &mut TestAppContext, id: &'static str) {
    wait(cx, id, move |cx| {
        cx.update_window(f.window, |_, window, cx| {
            window.render_frame(cx);
            window.try_find(id).is_some()
        })
        .unwrap_or(false)
    });
}

/// The aria label of the About page's update status line, once it reads
/// `text`.
fn status_reads(f: &Fixture, cx: &mut TestAppContext, text: &str) {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        cx.run_until_parked();
        let shown = cx
            .update_window(f.window, |_, window, cx| {
                window.render_frame(cx);
                window
                    .try_find("about-update-status")
                    .and_then(|e| e.label().map(str::to_string))
            })
            .unwrap_or(None);
        if shown.as_deref().is_some_and(|label| label.contains(text)) {
            return;
        }
        if std::time::Instant::now() > deadline {
            let status = cx.update(|cx| {
                ircord_app::features::updates::tracked(cx).map(|t| t.read(cx).status.clone())
            });
            panic!("wanted {text:?}; the page shows {shown:?}; the state is {status:?}");
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn open_about(f: &Fixture, cx: &mut TestAppContext) {
    click(f, cx, "open-settings");
    assert_eq!(route(f, cx), Some(Route::Settings));
    shows(f, cx, "settings-section-about");
    click(f, cx, "settings-section-about");
    shows(f, cx, "about-check-updates");
}

#[gpui_kit::test]
fn about_checks_and_says_what_it_found(cx: &mut TestAppContext) {
    let fake = Fake::answering(Ok(Checked::UpToDate));
    // The start-up check waits an hour, so only the button checks here.
    cx.update(|cx| use_updater(cx, fake.clone(), Duration::from_secs(3600)));
    let f = open(cx, true);
    bootstrap(&f, cx);
    open_about(&f, cx);

    click(&f, cx, "about-check-updates");
    let fake2 = fake.clone();
    wait(cx, "the fake to be asked", move |_| fake2.checks() >= 1);
    status_reads(&f, cx, "IRCord is up to date.");

    fake.set(Ok(release("9.9.9")));
    click(&f, cx, "about-check-updates");
    status_reads(&f, cx, "IRCord 9.9.9 is available.");
    shows(&f, cx, "about-install-update");

    fake.set(Err("the server did not answer".into()));
    click(&f, cx, "about-check-updates");
    status_reads(&f, cx, "the server did not answer");
    assert!(fake.installed().is_empty(), "a check installs nothing");
}

#[gpui_kit::test]
fn the_start_up_check_offers_a_release_and_later_installs_nothing(cx: &mut TestAppContext) {
    let fake = Fake::answering(Ok(release("9.9.9")));
    cx.update(|cx| use_updater(cx, fake.clone(), Duration::ZERO));
    let f = open(cx, true);
    bootstrap(&f, cx);

    let window = f.window;
    wait(cx, "the update dialog", move |cx| {
        label_shown(window, cx, "Later")
    });
    press_label(&f, cx, "Later");
    wait(cx, "the dialog to close", move |cx| {
        !label_shown(window, cx, "Later")
    });
    assert!(fake.installed().is_empty(), "Later installs nothing");
}

#[gpui_kit::test]
fn about_shows_checking_while_the_check_runs(cx: &mut TestAppContext) {
    let fake = Fake::answering(Ok(Checked::UpToDate));
    fake.hold(true);
    cx.update(|cx| use_updater(cx, fake.clone(), Duration::from_secs(3600)));
    let f = open(cx, true);
    bootstrap(&f, cx);
    open_about(&f, cx);

    click(&f, cx, "about-check-updates");
    status_reads(&f, cx, "Checking for updates…");
    fake.hold(false);
    status_reads(&f, cx, "IRCord is up to date.");
}

/// On Linux the prompt's OK answer opens the release page, so it closes the
/// dialog and installs nothing; Windows and macOS install instead.
#[cfg(not(any(windows, target_os = "macos")))]
#[gpui_kit::test]
fn the_start_up_prompt_answered_yes_opens_the_release_page(cx: &mut TestAppContext) {
    let fake = Fake::answering(Ok(release("9.9.9")));
    cx.update(|cx| use_updater(cx, fake.clone(), Duration::ZERO));
    let f = open(cx, true);
    bootstrap(&f, cx);

    let window = f.window;
    wait(cx, "the update dialog", move |cx| {
        label_shown(window, cx, "Open release page")
    });
    press_label(&f, cx, "Open release page");
    wait(cx, "the dialog to close", move |cx| {
        !label_shown(window, cx, "Open release page")
    });
    assert!(fake.installed().is_empty(), "Linux installs nothing");
}

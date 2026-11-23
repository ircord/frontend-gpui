//! `mermaid` fences drawn as diagrams: the picture fits the message, a
//! source that cannot be drawn shows as code with a note, the toggle shows
//! the source, Show fullscreen opens it at natural size, scrollable both
//! ways and closed by Escape, and the controls pass the audit.

use std::time::{Duration, Instant};

use gpui_kit::test::{TestQueryExt, TestWindowExt};
use gpui_kit::{AppContext, ScrollDelta, TestAppContext, point, px};
use ircord_client::contract::*;
use ircord_client::testing::Reply;
use serde_json::json;

use crate::fixture::{Fixture, message, open_with, snapshot, wait};

fn history() -> Vec<Message> {
    let wide = (0..16)
        .map(|i| format!("N{i}[Step number {i}]"))
        .collect::<Vec<_>>()
        .join(" --> ");
    vec![
        message(
            "1",
            "c-general",
            "```mermaid\nflowchart TD\n  A[unterminated --> B\n```",
        ),
        message(
            "2",
            "c-general",
            &format!("```mermaid\nflowchart LR\n  {wide}\n```"),
        ),
        message(
            "3",
            "c-general",
            "```mermaid\nflowchart TD\n  A[Start] -->|scrolled into view| B{Ready?}\n  B -->|yes| C[Draw]\n```",
        ),
    ]
}

/// One diagram wider and taller than the screen.
fn grid() -> Vec<Message> {
    // Four rows of eight steps, each label long enough to wrap onto
    // several lines. Kept small: the layout has two seconds, and the test
    // build lays out without optimisation while every other test runs.
    let grid = (0..4)
        .map(|r| {
            (0..8)
                .map(|c| {
                    format!(
                        "R{r}C{c}[Row {r} step {c}: a label long enough to wrap onto several lines in its box]"
                    )
                })
                .collect::<Vec<_>>()
                .join(" --> ")
        })
        .collect::<Vec<_>>()
        .join("\n  ");
    vec![message(
        "4",
        "c-general",
        &format!("```mermaid\nflowchart LR\n  {grid}\n```"),
    )]
}

fn open(cx: &mut TestAppContext) -> Fixture {
    open_on(cx, history())
}

fn open_on(cx: &mut TestAppContext, items: Vec<Message>) -> Fixture {
    let count = items.len();
    let f = open_with(cx, true, |server| {
        server.snapshot(&snapshot());
        let page = serde_json::to_value(MessagePage {
            items,
            ..Default::default()
        })
        .unwrap();
        server.route_fn("GET", "/api/v1/conversations/*/messages", move |r| {
            if r.path.ends_with("/c-general/messages") {
                Reply::json(200, page.clone())
            } else {
                Reply::json(200, json!({"items": []}))
            }
        });
    });
    wait(cx, "#general's history", |cx| {
        let session = f.session.read(cx);
        let Some(client) = session.client() else {
            return false;
        };
        let w = client.read();
        w.timelines
            .get("c-general")
            .is_some_and(|t| t.loaded && t.entries.len() == count)
    });
    f
}

/// Draw until `id` is on screen: the layout and the picture are made off
/// the main thread, and the picture's width comes from the frame before.
fn drawn(f: &Fixture, cx: &mut TestAppContext, id: &'static str) {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        cx.run_until_parked();
        let found = cx
            .update_window(f.window, |_, window, cx| {
                window.render_frame(cx);
                window.render_frame(cx);
                window.try_find(id).is_some()
            })
            .unwrap();
        if found {
            return;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {id}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[gpui_kit::test]
fn a_fence_is_a_picture_no_wider_than_the_message(cx: &mut TestAppContext) {
    let f = open(cx);
    drawn(&f, cx, "diagram-3-0-picture");
    drawn(&f, cx, "diagram-2-0-picture");
    cx.update_window(f.window, |_, window, _| {
        for (id, picture) in [
            ("diagram-2-0", "diagram-2-0-picture"),
            ("diagram-3-0", "diagram-3-0-picture"),
        ] {
            let frame = window.find(id).bounds();
            let picture = window.find(picture).bounds();
            assert!(picture.size.width > gpui_kit::px(40.), "{id}: {picture:?}");
            assert!(
                picture.origin.x >= frame.origin.x && picture.right() <= frame.right(),
                "{id}: the picture {picture:?} runs past its frame {frame:?}"
            );
        }
        // The sixteen-step chain is far wider than a message: it is drawn
        // at the frame's width.
        let wide = window.find("diagram-2-0").bounds();
        let picture = window.find("diagram-2-0-picture").bounds();
        assert!(
            picture.size.width >= wide.size.width - gpui_kit::px(40.),
            "{picture:?} in {wide:?}"
        );
        assert!(window.find("diagram-3-0-toggle").visible());
    })
    .unwrap();
}

#[gpui_kit::test]
fn a_broken_fence_shows_its_source_and_a_note(cx: &mut TestAppContext) {
    let f = open(cx);
    drawn(&f, cx, "diagram-1-0-note");
    cx.update_window(f.window, |_, window, _| {
        assert!(window.try_find("code-1-0").is_some());
        assert!(window.try_find("diagram-1-0").is_none());
    })
    .unwrap();
}

#[gpui_kit::test]
fn the_toggle_shows_the_source_and_the_audit_passes(cx: &mut TestAppContext) {
    let f = open(cx);
    drawn(&f, cx, "diagram-3-0-picture");
    let problems = cx
        .update_window(f.window, |_, window, cx| {
            crate::audit::audit("diagrams", window, cx)
        })
        .unwrap();
    assert!(problems.is_empty(), "{problems:#?}");
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.click("diagram-3-0-toggle", cx);
    })
    .unwrap();
    drawn(&f, cx, "diagram-3-0-source");
    cx.update_window(f.window, |_, window, _| {
        assert!(window.try_find("diagram-3-0-picture").is_none());
        assert!(window.find_by_label("Show diagram").visible());
    })
    .unwrap();
}

#[gpui_kit::test]
fn fullscreen_shows_the_natural_size_scrolls_and_closes_on_escape(cx: &mut TestAppContext) {
    let f = open_on(cx, grid());
    drawn(&f, cx, "diagram-4-0-picture");
    let inline = cx
        .update_window(f.window, |_, window, cx| {
            window.render_frame(cx);
            let inline = window.find("diagram-4-0-picture").bounds();
            window.click("diagram-4-0-fullscreen", cx);
            inline
        })
        .unwrap();
    drawn(&f, cx, "diagram-viewer-picture");
    let before = cx
        .update_window(f.window, |_, window, cx| {
            window.render_frame(cx);
            let stage = window.find("diagram-viewer-stage").bounds();
            let picture = window.find("diagram-viewer-picture").bounds();
            assert!(
                picture.size.width > inline.size.width,
                "drawn at its natural size, {picture:?}, not the message's {inline:?}"
            );
            assert!(
                picture.size.width > stage.size.width && picture.size.height > stage.size.height,
                "the grid {picture:?} is larger than the stage {stage:?}"
            );
            window.scroll(
                "diagram-viewer-stage",
                ScrollDelta::Pixels(point(px(-300.), px(0.))),
                cx,
            );
            window.scroll(
                "diagram-viewer-stage",
                ScrollDelta::Pixels(point(px(0.), px(-300.))),
                cx,
            );
            picture.origin
        })
        .unwrap();
    cx.run_until_parked();
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        let after = window.find("diagram-viewer-picture").bounds().origin;
        assert!(
            after.x < before.x,
            "scrolls across: {before:?} to {after:?}"
        );
        assert!(after.y < before.y, "scrolls down: {before:?} to {after:?}");
        window.press("escape", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find("diagram-viewer-stage").is_none(),
            "Escape closes it"
        );
    })
    .unwrap();
}

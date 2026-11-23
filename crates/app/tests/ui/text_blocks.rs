//! Tests for the message blocks that are not code, quotes or lists: pipe
//! tables, headings, rules and task boxes (ircord/issues#231).

use gpui_kit::test::{TestQueryExt, TestWindowExt};
use gpui_kit::{AppContext, Pixels, TestAppContext};
use ircord_client::contract::*;
use ircord_client::testing::Reply;
use serde_json::json;

use ircord_app::features::message_actions;

use crate::fixture::{Fixture, message, open_with, snapshot, wait};

/// A table, headings with a rule and a task list, and a table with one very
/// long cell.
fn history() -> Vec<Message> {
    let long = "word ".repeat(120);
    vec![
        message(
            "1",
            "c-general",
            "see:\n| Name | Notes | Count |\n|:-----|:-----:|------:|\n\
             | sage | \x02bold\x02 https://ircord.com | 1 |\n| bob | plain | 22 |\nafter",
        ),
        message(
            "2",
            "c-general",
            &format!("| a | b |\n|---|---|\n| x | {long}|"),
        ),
        message(
            "3",
            "c-general",
            "# Big\n## Medium\n#### Small\n---\n- [ ] todo\n- [x] done\n- plain",
        ),
    ]
}

fn open(cx: &mut TestAppContext) -> Fixture {
    let f = open_with(cx, true, |server| {
        server.snapshot(&snapshot());
        let page = serde_json::to_value(MessagePage {
            items: history(),
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
        session.conversation.as_deref() == Some("c-general")
            && w.timelines
                .get("c-general")
                .is_some_and(|t| t.loaded && t.entries.len() == 3)
    });
    // Pills are placed from the layout of the frame before.
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.render_frame(cx);
    })
    .unwrap();
    cx.run_until_parked();
    f
}

/// Where the element with this ID is.
fn at(window: &gpui_kit::Window, id: String) -> gpui_kit::Bounds<Pixels> {
    window.find(gpui_kit::ElementId::Name(id.into())).bounds()
}

fn px(p: Pixels) -> f32 {
    f32::from(p)
}

#[gpui_kit::test]
fn a_table_is_a_grid_that_sizes_to_its_content(cx: &mut TestAppContext) {
    let f = open(cx);
    cx.update_window(f.window, |_, window, _| {
        assert!(window.find("table-1-0").visible());
        assert!(window.find_by_label("Table, 3 columns, 2 rows").visible());
        // A column's cells share their left edge and width.
        for column in 0..3 {
            let top = at(window, format!("table-1-0-0-{column}"));
            for row in 1..=2 {
                let cell = at(window, format!("table-1-0-{row}-{column}"));
                assert_eq!(px(cell.origin.x), px(top.origin.x), "column {column}");
                assert_eq!(px(cell.size.width), px(top.size.width), "column {column}");
            }
        }
        // A row's cells share their top.
        for row in 0..=2 {
            let first = at(window, format!("table-1-0-{row}-0"));
            for column in 1..3 {
                let cell = at(window, format!("table-1-0-{row}-{column}"));
                assert_eq!(px(cell.origin.y), px(first.origin.y), "row {row}");
            }
        }
        // Columns are as wide as their content, so this table is narrower than a long one.
        let short = window.find("table-1-0").bounds();
        let long = window.find("table-2-0").bounds();
        assert!(px(short.size.width) < px(long.size.width) / 2.);
        // The header is a header; the text after the table is its own line.
        assert!(window.try_find("table-1-0-0-0").is_some());
        assert!(window.find_by_label("Open https://ircord.com").visible());
    })
    .unwrap();
}

#[gpui_kit::test]
fn a_long_cell_wraps_inside_the_message(cx: &mut TestAppContext) {
    let f = open(cx);
    cx.update_window(f.window, |_, window, _| {
        let table = window.find("table-2-0").bounds();
        let window_width = px(window.viewport_size().width);
        assert!(
            px(table.size.width) < window_width * 0.8,
            "the table stays inside the message column"
        );
        let long = window.find("table-2-0-1-1").bounds();
        let short = window.find("table-2-0-0-0").bounds();
        assert!(
            px(long.size.height) > px(short.size.height) * 2.5,
            "the long cell wraps"
        );
        assert!(px(long.size.width) > px(short.size.width));
    })
    .unwrap();
}

#[gpui_kit::test]
fn headings_rules_and_task_boxes_are_drawn(cx: &mut TestAppContext) {
    let f = open(cx);
    cx.update_window(f.window, |_, window, _| {
        let big = window.find("heading-3-0").bounds();
        let medium = window.find("heading-3-1").bounds();
        let small = window.find("heading-3-2").bounds();
        assert!(px(big.size.height) > px(medium.size.height));
        assert!(px(medium.size.height) > px(small.size.height));
        // The task list is a list whose boxed items say their state.
        assert!(window.find("list-3-0").visible());
        assert!(window.find_by_label("To do").visible());
        assert!(window.find_by_label("Done").visible());
        // The box is no control, so the keyboard has nothing new to reach.
        assert_eq!(window.find_all_by_label("To do").len(), 1);
    })
    .unwrap();
}

#[gpui_kit::test]
fn editing_a_message_gives_back_its_markdown(cx: &mut TestAppContext) {
    let f = open(cx);
    cx.update_window(f.window, |_, window, cx| {
        message_actions::start_edit("c-general", "1", window, cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update(|cx| {
        let text = message_actions::box_text(cx);
        assert!(
            text.contains("| sage | **bold** https://ircord.com | 1 |"),
            "{text}"
        );
        assert!(!text.contains('\x02'));
    });
}

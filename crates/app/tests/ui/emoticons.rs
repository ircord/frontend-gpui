//! Tests for package P4: typed emoticons such as `:)` and `<3` become emoji
//! in the message box, as the web's composer does. Covered: conversion once
//! the space after one is typed, Backspace putting the text back, sending
//! converting a final emoticon (also in a command), emoticons left alone
//! inside a word or code, the emoji search leaving emoticons alone, and an
//! emoticon next to a server emoji's `:name:` token.

use std::time::Duration;

use gpui_kit::test::TestWindowExt;
use gpui_kit::{AppContext, TestAppContext};

use crate::emoji::serve;
use crate::fixture::{Fixture, open_with, wait};

const WAIT: Duration = Duration::from_secs(5);

/// One thing the person does in the box: typing text, or pressing a key.
enum Step {
    Type(&'static str),
    Key(&'static str),
}

use Step::{Key, Type};

/// The box with the custom emoji, the emoji catalog loaded, and #general's
/// history showing, as the emoji tests open it.
fn open(cx: &mut TestAppContext) -> Fixture {
    let f = open_with(cx, true, serve);
    wait(
        cx,
        "the history, the live socket and the emoji catalogs",
        |cx| {
            let session = f.session.read(cx);
            let Some(client) = session.client() else {
                return false;
            };
            let w = client.read();
            session.conversation.as_deref() == Some("c-general")
                && w.connection == Some(ircord_client::store::Connection::Live)
                && w.emoji.is_loaded()
                && w.documents.library.is_loaded()
                && w.timelines
                    .get("c-general")
                    .is_some_and(|t| t.entries.len() == 2)
        },
    );
    f
}

/// Type or press each step in turn, drawing the frame before it as a key
/// press would have left the box.
fn run(f: &Fixture, cx: &mut TestAppContext, steps: &[Step]) {
    for (i, step) in steps.iter().enumerate() {
        cx.update_window(f.window, |_, window, cx| {
            window.render_frame(cx);
            if i == 0 {
                window.click("composer-input", cx);
            }
            match step {
                Type(text) => window.input(text, cx),
                Key(key) => window.press(key, cx),
            }
        })
        .unwrap();
        cx.run_until_parked();
    }
}

/// The text the server received for the one message sent.
fn sent(f: &Fixture) -> String {
    f.server
        .wait_for("POST", "/api/v1/conversations/c-general/messages", WAIT)
        .json()["text"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

fn suggestions_shown(f: &Fixture, cx: &mut TestAppContext) -> bool {
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.try_find("composer-suggestions").is_some()
    })
    .unwrap()
}

#[gpui_kit::test]
fn a_typed_emoticon_becomes_emoji_once_the_space_after_it_is_typed(cx: &mut TestAppContext) {
    let f = open(cx);
    run(
        &f,
        cx,
        &[Type("hi :)"), Type(" "), Type("there"), Key("enter")],
    );
    assert_eq!(sent(&f), "hi 🙂 there");
}

#[gpui_kit::test]
fn backspace_straight_after_the_conversion_puts_the_emoticon_back(cx: &mut TestAppContext) {
    let f = open(cx);
    run(
        &f,
        cx,
        &[
            Type("hi :)"),
            Type(" "),
            Key("backspace"),
            Type("x"),
            Key("enter"),
        ],
    );
    assert_eq!(sent(&f), "hi :) x");
}

#[gpui_kit::test]
fn sending_turns_a_final_emoticon_into_emoji(cx: &mut TestAppContext) {
    let f = open(cx);
    run(&f, cx, &[Type("see you :P"), Key("enter")]);
    assert_eq!(sent(&f), "see you 😛");
}

#[gpui_kit::test]
fn a_command_line_is_converted_before_it_runs(cx: &mut TestAppContext) {
    let f = open(cx);
    run(&f, cx, &[Type("/me waves :D"), Key("enter")]);
    assert!(sent(&f).contains("waves 😄"));
}

#[gpui_kit::test]
fn an_emoticon_inside_a_word_stays_as_typed(cx: &mut TestAppContext) {
    let f = open(cx);
    run(&f, cx, &[Type("word:( "), Key("enter")]);
    assert_eq!(sent(&f).trim_end(), "word:(");
}

#[gpui_kit::test]
fn an_emoticon_inside_code_stays_as_typed(cx: &mut TestAppContext) {
    let f = open(cx);
    run(&f, cx, &[Type("`a :) "), Key("enter")]);
    let text = sent(&f);
    assert!(text.contains(":)"), "{text:?}");
    assert!(!text.contains('🙂'), "{text:?}");
}

#[gpui_kit::test]
fn the_emoji_search_leaves_emoticons_alone(cx: &mut TestAppContext) {
    let f = open(cx);
    run(&f, cx, &[Type(":-D")]);
    assert!(!suggestions_shown(&f, cx), ":-D is not an emoji search");
    // The list does show for a person, so the check above is not vacuous.
    run(&f, cx, &[Type(" @b")]);
    assert!(suggestions_shown(&f, cx));
}

#[gpui_kit::test]
fn an_emoticon_after_a_server_emoji_converts_and_the_emoji_stays(cx: &mut TestAppContext) {
    let f = open(cx);
    run(&f, cx, &[Type(":party: :)"), Type(" "), Key("enter")]);
    assert_eq!(sent(&f).trim_end(), ":party: 🙂");
}

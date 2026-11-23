//! Tests for package CH2 against the canned server: the channel settings
//! (topic, channel-wide settings, modes, leaving), the list tabs with the
//! mask builder, registration, and moderation from the member menu.
//! Topics, modes and list entries go out as raw lines to
//! `/networks/{id}/commands`; the tests read the lines sent there.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui_kit::test::{TestEventExt, TestQueryExt, TestWindowExt};
use gpui_kit::{AppContext, MouseButton, ScrollDelta, TestAppContext, point, px};
use ircord_app::features::channel_settings::{self, Tab};
use ircord_client::contract::*;
use ircord_client::store::Connection;
use ircord_client::testing::{CannedServer, Reply};
use serde_json::{Value, json};

use crate::fixture::{Fixture, open_with, snapshot, wait};

const WAIT: Duration = crate::fixture::HANG_GUARD;
const COMMANDS: &str = "/api/v1/networks/n1/commands";

/// The fixture's workspace with you as `modes` in #general, which has the
/// channel-wide settings and a topic; bob has a known address.
fn workspace(modes: &str) -> Snapshot {
    let mut s = snapshot();
    let general = s
        .conversations
        .iter_mut()
        .find(|c| c.id == "c-general")
        .unwrap();
    general.member_ids.push("u-me".into());
    general.member_modes.insert("u-me".into(), modes.into());
    general.modes = "nt".into();
    general.topic_set_by = Some("bob".into());
    general.channel_settings = Some(ChannelSettings {
        group_mentions: ChannelSettingsGroupMentions::Ops,
        notifications: ChannelSettingsNotifications::Default,
        nsfw: false,
        ..Default::default()
    });
    s.networks[0]
        .isupport
        .insert("CHANMODES".into(), "beIq,k,l,imnpst".into());
    s.networks[0].isupport.insert("EXTBAN".into(), "$,a".into());
    s.networks[0].account = Some("me".into());
    s.networks[0].features.redact = true;
    for m in &mut s.members {
        if m.id == "m-bob" {
            m.username = Some("b".into());
            m.host = Some("example.org".into());
        }
    }
    s
}

fn open(cx: &mut TestAppContext, modes: &str, setup: impl FnOnce(&CannedServer)) -> Fixture {
    let s = workspace(modes);
    let f = open_with(cx, true, |server| {
        server.snapshot(&s);
        server.route("POST", COMMANDS, 202, json!({"clientId": "x"}));
        server.route(
            "GET",
            "/api/v1/conversations/c-general/channel-guide",
            200,
            json!({"available": false, "groups": [], "operator": false, "status": "none"}),
        );
        setup(server);
    });
    wait(cx, "#general, live", |cx| {
        let session = f.session.read(cx);
        let Some(client) = session.client() else {
            return false;
        };
        let w = client.read();
        session.conversation.as_deref() == Some("c-general")
            && w.timelines.get("c-general").is_some_and(|t| t.loaded)
            && w.connection == Some(Connection::Live)
    });
    f
}

fn frame(
    f: &Fixture,
    cx: &mut TestAppContext,
    run: impl FnOnce(&mut gpui_kit::Window, &mut gpui_kit::App),
) {
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        run(window, cx);
    })
    .unwrap();
    cx.run_until_parked();
}

/// Click `id`, scrolling the settings until it is in view.
fn click(f: &Fixture, cx: &mut TestAppContext, id: &'static str) {
    into_view(f, cx, id);
    frame(f, cx, |window, cx| window.click(id, cx));
}

/// Scroll the settings' content until `id` is in view.
fn into_view(f: &Fixture, cx: &mut TestAppContext, id: &'static str) {
    for _ in 0..40 {
        let mut state = (false, false);
        frame(f, cx, |window, _| {
            state = (
                window.try_find(id).is_some_and(|e| e.visible()),
                window.try_find("channel-settings-content").is_some(),
            )
        });
        match state {
            (true, _) | (false, false) => return,
            _ => frame(f, cx, |window, cx| {
                window.scroll(
                    "channel-settings-content",
                    ScrollDelta::Pixels(point(px(0.), px(-120.))),
                    cx,
                )
            }),
        }
    }
}

fn click_label(f: &Fixture, cx: &mut TestAppContext, label: &str) {
    frame(f, cx, |window, cx| {
        let at = window.find_by_label(label).bounds().center();
        window.pointer_down(at, MouseButton::Left, cx);
        window.pointer_up(at, MouseButton::Left, cx);
    });
}

/// Right-click `id` and pick the menu entry labelled `label`.
fn menu(f: &Fixture, cx: &mut TestAppContext, id: &'static str, label: &str) {
    frame(f, cx, |window, cx| {
        window.hover(id, cx);
        window.right_click(id, cx);
    });
    click_label(f, cx, label);
}

/// Replace the text of the input `id`.
fn type_into(f: &Fixture, cx: &mut TestAppContext, id: &'static str, text: &str) {
    into_view(f, cx, id);
    frame(f, cx, |window, cx| {
        window.click(id, cx);
        window.press("ctrl-a", cx);
        window.press("backspace", cx);
        window.input(text, cx);
    });
}

fn shows(f: &Fixture, cx: &mut TestAppContext, id: &'static str) -> bool {
    into_view(f, cx, id);
    let mut found = false;
    frame(f, cx, |window, _| {
        found = window.try_find(id).is_some_and(|e| e.visible())
    });
    found
}

fn has_label(f: &Fixture, cx: &mut TestAppContext, label: &str) -> bool {
    let mut found = false;
    frame(f, cx, |window, _| {
        found = window.try_find_by_label(label).is_some()
    });
    found
}

fn label(f: &Fixture, cx: &mut TestAppContext, id: &'static str) -> String {
    let mut text = String::new();
    frame(f, cx, |window, _| {
        text = window.find(id).label().unwrap_or_default().to_string()
    });
    text
}

fn wait_for(f: &Fixture, cx: &mut TestAppContext, id: &'static str) {
    let deadline = std::time::Instant::now() + WAIT;
    while !shows(f, cx, id) {
        assert!(std::time::Instant::now() < deadline, "{id} never showed");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn wait_label(f: &Fixture, cx: &mut TestAppContext, id: &'static str, want: &str) {
    let deadline = std::time::Instant::now() + WAIT;
    loop {
        if shows(f, cx, id) && label(f, cx, id) == want {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "{id} never read {want:?}, last {:?}",
            shows(f, cx, id).then(|| label(f, cx, id))
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn open_settings(f: &Fixture, cx: &mut TestAppContext, tab: Tab) {
    let session = f.session.clone();
    frame(f, cx, |window, cx| {
        channel_settings::open(&session, "c-general", tab, window, cx)
    });
    wait_for(f, cx, "channel-settings-dialog");
}

/// The `n`th raw line sent (1 is the first).
fn line(f: &Fixture, n: usize) -> String {
    f.server.wait_for_nth("POST", COMMANDS, n, WAIT).json()["line"]
        .as_str()
        .unwrap()
        .to_string()
}

#[gpui_kit::test]
fn the_overview_saves_the_topic_and_the_channel_wide_settings(cx: &mut TestAppContext) {
    let f = open(cx, "@", |server| {
        server.route_fn(
            "PUT",
            "/api/v1/conversations/c-general/channel-settings",
            |r| {
                assert_eq!(r.json(), json!({"nsfw": true}));
                let mut c = workspace("@").conversations.remove(0);
                c.channel_settings.as_mut().unwrap().nsfw = true;
                Reply::json(200, serde_json::to_value(c).unwrap())
            },
        );
    });
    open_settings(&f, cx, Tab::Overview);
    assert_eq!(
        label(&f, cx, "channel-settings-rank-note"),
        "You’re an operator here and can change everything below."
    );
    type_into(&f, cx, "channel-settings-topic", "Ship **it**");
    click(&f, cx, "channel-settings-topic-save");
    assert_eq!(line(&f, 1), "TOPIC #general :Ship \x02it\x02");

    click(&f, cx, "channel-settings-nsfw");
    f.server.wait_for(
        "PUT",
        "/api/v1/conversations/c-general/channel-settings",
        WAIT,
    );
    wait(cx, "#general marked NSFW", |cx| {
        let session = f.session.read(cx);
        let w = session.client().unwrap().read();
        w.conversation("c-general")
            .and_then(|c| c.channel_settings.as_ref())
            .is_some_and(|s| s.nsfw)
    });
}

#[gpui_kit::test]
fn leaving_from_the_settings_asks_first(cx: &mut TestAppContext) {
    let f = open(cx, "", |server| {
        server.route_reply("DELETE", "/api/v1/conversations/c-general", Reply::empty());
    });
    open_settings(&f, cx, Tab::Overview);
    // A plain member reads the settings but cannot change them.
    assert_eq!(
        label(&f, cx, "channel-settings-rank-note"),
        "Read-only: changing these needs operator status (@) in #general."
    );
    click(&f, cx, "channel-settings-leave");
    assert!(
        f.server
            .requests_to("DELETE", "/api/v1/conversations/c-general")
            .is_empty()
    );
    click_label(&f, cx, "Leave");
    f.server
        .wait_for("DELETE", "/api/v1/conversations/c-general", WAIT);
}

#[gpui_kit::test]
fn modes_switch_and_take_values(cx: &mut TestAppContext) {
    let f = open(cx, "@", |_| {});
    open_settings(&f, cx, Tab::Modes);
    // Moderated is off: switching it on sends +m; topic lock is on.
    click(&f, cx, "channel-settings-flag-m");
    assert_eq!(line(&f, 1), "MODE #general +m");
    click(&f, cx, "channel-settings-flag-t");
    assert_eq!(line(&f, 2), "MODE #general -t");
    type_into(&f, cx, "channel-settings-value-l", "25");
    click(&f, cx, "channel-settings-value-l-set");
    assert_eq!(line(&f, 3), "MODE #general +l 25");
    // The list tabs come from CHANMODES.
    assert!(shows(&f, cx, "channel-settings-tab-list-upper-i"));
    assert!(shows(&f, cx, "channel-settings-tab-list-q"));
}

#[gpui_kit::test]
fn the_ban_list_shows_removes_and_adds_with_the_mask_builder(cx: &mut TestAppContext) {
    let f = open(cx, "@", |server| {
        server.route(
            "GET",
            "/api/v1/conversations/c-general/lists/b",
            200,
            json!({"items": [{"mask": "spam!*@*", "setBy": "bob",
                              "setAt": "2026-11-20T10:00:00Z"}]}),
        );
    });
    open_settings(&f, cx, Tab::List('b'));
    wait_label(&f, cx, "channel-settings-list-entry-0", "spam!*@*");

    click(&f, cx, "channel-settings-list-remove-0");
    click_label(&f, cx, "Remove");
    assert_eq!(line(&f, 1), "MODE #general -b spam!*@*");

    // Typed by hand under Advanced, explained before it is added.
    click(&f, cx, "channel-settings-mask-advanced");
    type_into(&f, cx, "channel-settings-mask", "alice");
    assert_eq!(
        label(&f, cx, "channel-settings-mask-preview"),
        "Adds alice!*@* for “alice”. Matches anyone with the nick alice."
    );
    assert_eq!(
        label(&f, cx, "channel-settings-mask-matches"),
        "No people match here"
    );
    click(&f, cx, "channel-settings-list-add-button");
    assert_eq!(line(&f, 2), "MODE #general +b alice!*@*");

    // Built from bob: his account first, then his host and nick.
    type_into(&f, cx, "people-picker-input", "bob");
    wait_label(&f, cx, "people-picker-option-0", "bob");
    click(&f, cx, "people-picker-option-0");
    wait_for(&f, cx, "channel-settings-mask-parts");
    assert_eq!(
        label(&f, cx, "channel-settings-mask-preview"),
        "Adds $a:bob. Anyone logged in to the account bob, whatever nick they use."
    );
    click(&f, cx, "channel-settings-mask-part-host");
    assert_eq!(
        label(&f, cx, "channel-settings-mask-preview"),
        "Adds *!*@example.org. Matches anyone with the host example.org."
    );
    assert_eq!(
        label(&f, cx, "channel-settings-mask-matches"),
        "Matches 1 person here now: bob"
    );
    click(&f, cx, "channel-settings-mask-part-nick");
    click(&f, cx, "channel-settings-list-add-button");
    assert_eq!(line(&f, 3), "MODE #general +b bob!*@example.org");
}

#[gpui_kit::test]
fn a_broad_mask_is_flagged(cx: &mut TestAppContext) {
    let f = open(cx, "@", |server| {
        server.route(
            "GET",
            "/api/v1/conversations/c-general/lists/q",
            200,
            json!({"items": []}),
        );
    });
    open_settings(&f, cx, Tab::List('q'));
    wait_for(&f, cx, "channel-settings-list-empty");
    click(&f, cx, "channel-settings-mask-advanced");
    type_into(&f, cx, "channel-settings-mask", "*!*@*.com");
    assert_eq!(
        label(&f, cx, "channel-settings-mask-warning-0"),
        "The host part is very broad: it can match a whole country, provider or address range."
    );
    type_into(&f, cx, "channel-settings-mask", "a b");
    assert_eq!(
        label(&f, cx, "channel-settings-mask-preview"),
        "Masks can’t contain spaces."
    );
}

#[gpui_kit::test]
fn the_member_menu_moderates_by_rank(cx: &mut TestAppContext) {
    let f = open(cx, "@", |_| {});
    menu(&f, cx, "member-m-eve", "Give voice");
    assert_eq!(line(&f, 1), "MODE #general +v eve");
    menu(&f, cx, "member-m-bob", "Remove operator");
    assert_eq!(line(&f, 2), "MODE #general -o bob");
    menu(&f, cx, "member-m-eve", "Kick…");
    type_into(&f, cx, "prompt-input", "be nice");
    click_label(&f, cx, "Kick");
    assert_eq!(line(&f, 3), "KICK #general eve :be nice");
    menu(&f, cx, "member-m-eve", "Ban…");
    click_label(&f, cx, "Ban");
    assert_eq!(line(&f, 4), "MODE #general +b eve!*@*");
}

#[gpui_kit::test]
fn a_plain_member_gets_no_moderation(cx: &mut TestAppContext) {
    let f = open(cx, "", |_| {});
    frame(&f, cx, |window, cx| {
        window.hover("member-m-eve", cx);
        window.right_click("member-m-eve", cx);
    });
    assert!(has_label(&f, cx, "Mention"));
    assert!(!has_label(&f, cx, "Give voice"));
    assert!(!has_label(&f, cx, "Kick…"));
    assert!(!has_label(&f, cx, "Delete recent messages…"));
}

#[gpui_kit::test]
fn recent_messages_are_counted_then_deleted(cx: &mut TestAppContext) {
    let f =
        open(cx, "@", |server| {
            server.route_fn(
            "GET",
            "/api/v1/conversations/c-general/members/m-eve/recent-messages",
            |r| {
                let count = if r.query_param("range").as_deref() == Some("7d") { 5 } else { 3 };
                Reply::json(
                    200,
                    json!({"authority": "operator", "count": count, "limit": 500, "nick": "eve"}),
                )
            },
        );
            server.route_fn(
                "POST",
                "/api/v1/conversations/c-general/members/m-eve/recent-messages/deletion",
                |r| {
                    assert_eq!(r.json()["range"], "7d");
                    Reply::json(202, job("running", 0))
                },
            );
            server.route(
                "GET",
                "/api/v1/recent-messages-deletions/j1",
                200,
                job("done", 5),
            );
        });
    menu(&f, cx, "member-m-eve", "Delete recent messages…");
    wait_label(
        &f,
        cx,
        "channel-settings-recent-count",
        "3 messages in #general will be deleted for everyone.",
    );
    click_label(&f, cx, "Last 7 days");
    wait_label(
        &f,
        cx,
        "channel-settings-recent-count",
        "5 messages in #general will be deleted for everyone.",
    );
    click(&f, cx, "channel-settings-recent-delete");
    f.server.wait_for(
        "POST",
        "/api/v1/conversations/c-general/members/m-eve/recent-messages/deletion",
        WAIT,
    );
    f.server
        .wait_for("GET", "/api/v1/recent-messages-deletions/j1", WAIT);
}

fn job(state: &str, deleted: i64) -> Value {
    json!({"authority": "operator", "channel": "#general", "conversationId": "c-general",
           "deleted": deleted, "failed": 0, "id": "j1", "nick": "eve",
           "startedAt": "2026-11-21T10:00:00Z", "state": state, "tooOld": 0, "total": 5})
}

#[gpui_kit::test]
fn registration_registers_and_lists_access(cx: &mut TestAppContext) {
    let registered = Arc::new(Mutex::new(false));
    let f = open(cx, "@", |server| {
        let read = registered.clone();
        server.route_fn(
            "GET",
            "/api/v1/conversations/c-general/registration",
            move |_| {
                let on = *read.lock().unwrap();
                Reply::json(
                    200,
                    if on {
                        json!({"registered": true, "founder": "me",
                           "registeredAt": "2026-11-21T10:00:00Z"})
                    } else {
                        json!({"registered": false})
                    },
                )
            },
        );
        let set = registered.clone();
        server.route_fn(
            "POST",
            "/api/v1/conversations/c-general/registration",
            move |_| {
                *set.lock().unwrap() = true;
                Reply::json(200, json!({"registered": true, "founder": "me"}))
            },
        );
        server.route(
            "GET",
            "/api/v1/conversations/c-general/access",
            200,
            json!({"items": [{"account": "me", "mode": "q"}, {"account": "bob", "mode": "o"}]}),
        );
        server.route_fn("PUT", "/api/v1/conversations/c-general/access/eve", |r| {
            assert_eq!(r.json(), json!({"mode": "o"}));
            Reply::json(200, json!({"items": []}))
        });
    });
    open_settings(&f, cx, Tab::Registration);
    wait_for(&f, cx, "channel-settings-registration-none");
    click(&f, cx, "channel-settings-register");
    f.server
        .wait_for("POST", "/api/v1/conversations/c-general/registration", WAIT);
    wait_label(
        &f,
        cx,
        "channel-settings-registration-saved",
        "#general is registered to you.",
    );
    wait_label(&f, cx, "channel-settings-access-1", "bob · Operator");
    // The founder's own entry cannot be changed; bob's can.
    assert!(!shows(&f, cx, "channel-settings-access-0-remove"));
    assert!(shows(&f, cx, "channel-settings-access-1-remove"));
    type_into(&f, cx, "channel-settings-access-account", "eve");
    click(&f, cx, "channel-settings-access-add-button");
    f.server
        .wait_for("PUT", "/api/v1/conversations/c-general/access/eve", WAIT);
}

#[gpui_kit::test]
fn a_network_without_chanserv_says_so(cx: &mut TestAppContext) {
    let f = open(cx, "@", |server| {
        server.route_reply(
            "GET",
            "/api/v1/conversations/c-general/registration",
            Reply::problem(501, "no services"),
        );
    });
    open_settings(&f, cx, Tab::Registration);
    wait_label(
        &f,
        cx,
        "channel-settings-registration-unsupported",
        "Home doesn’t have a ChanServ that IRCord can manage.",
    );
}

#[gpui_kit::test]
fn the_channel_guide_tab_asks_to_be_listed(cx: &mut TestAppContext) {
    let f = open(cx, "@", |server| {
        server.route(
            "GET",
            "/api/v1/conversations/c-general/channel-guide",
            200,
            json!({"available": true, "groups": ["Games"], "operator": true, "status": "none"}),
        );
        server.route_fn(
            "PUT",
            "/api/v1/conversations/c-general/channel-guide",
            |r| {
                assert_eq!(
                    r.json(),
                    json!({"group": "Games", "description": "We play"})
                );
                Reply::json(
                    200,
                    json!({"available": true, "groups": ["Games"], "operator": true,
                       "status": "pending", "group": "Games", "description": "We play"}),
                )
            },
        );
    });
    open_settings(&f, cx, Tab::Overview);
    wait_for(&f, cx, "channel-settings-tab-guide");
    click(&f, cx, "channel-settings-tab-guide");
    type_into(&f, cx, "channel-settings-guide-group", "Games");
    type_into(&f, cx, "channel-settings-guide-description", "We play");
    click(&f, cx, "channel-settings-guide-request");
    wait_label(
        &f,
        cx,
        "channel-settings-guide-status",
        "Waiting for a server owner",
    );
    assert!(shows(&f, cx, "channel-settings-guide-withdraw"));
}

/// #general's history: bob's message with a network ID.
fn bobs_message(server: &CannedServer) {
    let mut one = crate::fixture::message("1", "c-general", "buy now");
    one.upstream_id = Some("up-1".into());
    let page = serde_json::to_value(MessagePage {
        items: vec![one],
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
}

#[gpui_kit::test]
fn an_operator_deletes_someone_elses_message(cx: &mut TestAppContext) {
    let f = open(cx, "@", |server| {
        bobs_message(server);
        server.route_fn(
            "DELETE",
            "/api/v1/conversations/c-general/messages/1",
            |r| {
                // An operator gives no reason; that is for server owners.
                assert_eq!(r.query_param("reason"), None);
                let mut gone = crate::fixture::message("1", "c-general", "");
                gone.deleted = Some(true);
                Reply::json(
                    200,
                    json!({"attachmentRetained": false, "authority": "operator", "message": gone}),
                )
            },
        );
    });
    wait_for(&f, cx, "message-1");
    menu(&f, cx, "message-1", "Delete message…");
    wait_for(&f, cx, "channel-settings-delete");
    assert!(!shows(&f, cx, "channel-settings-delete-reason"));
    click(&f, cx, "channel-settings-delete-confirm");
    f.server
        .wait_for("DELETE", "/api/v1/conversations/c-general/messages/1", WAIT);
    wait(cx, "message 1 deleted", |cx| {
        let session = f.session.read(cx);
        let w = session.client().unwrap().read();
        w.message("c-general", "1")
            .is_none_or(|m| m.deleted == Some(true))
    });
}

#[gpui_kit::test]
fn a_plain_member_cannot_delete_others_messages(cx: &mut TestAppContext) {
    let f = open(cx, "", bobs_message);
    wait_for(&f, cx, "message-1");
    frame(&f, cx, |window, cx| {
        window.hover("message-1", cx);
        window.right_click("message-1", cx);
    });
    assert!(!has_label(&f, cx, "Delete message…"));
}

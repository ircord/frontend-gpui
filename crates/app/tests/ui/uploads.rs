//! Package U1: the upload tray, attachments in the timeline and the image viewer.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use gpui_kit::test::{TestEventExt, TestQueryExt, TestWindowExt};
use gpui_kit::{AppContext, ClipboardItem, Image, ImageFormat, MouseButton, TestAppContext};
use ircord_app::composer::expressions::Tab;
use ircord_app::composer::uploads::{self as tray, Status, Uploads};
use ircord_app::features::{klipy, message_actions};
use ircord_client::api::uploads::FileData;
use ircord_client::contract::*;
use ircord_client::testing::{CannedServer, Reply};
use serde_json::json;

use crate::fixture::{Fixture, bootstrap, held_reply, message, open_with, snapshot, wait};

/// The server accepts pictures and PDFs up to 1 MB; `#general` is not
/// sensitive, unless `nsfw`.
fn upload_snapshot(nsfw: Option<bool>) -> Snapshot {
    let mut s = snapshot();
    s.capabilities.upload = Some(UploadCapability {
        max_bytes: 1_000_000,
        types: vec!["image/*".into(), "application/pdf".into()],
    });
    if let Some(nsfw) = nsfw {
        s.conversations[0].channel_settings = Some(ChannelSettings {
            nsfw,
            ..Default::default()
        });
    }
    s
}

fn server(server: &CannedServer, nsfw: Option<bool>) {
    server.snapshot(&upload_snapshot(nsfw));
}

/// Each upload answers with a new ID, after `delay`.
fn uploads_answer(server: &CannedServer, delay: Duration) {
    let n = Arc::new(AtomicUsize::new(0));
    server.route_fn("POST", "/api/v1/uploads", move |request| {
        std::thread::sleep(delay);
        let i = n.fetch_add(1, Ordering::SeqCst) + 1;
        assert!(
            request
                .header("content-type")
                .is_some_and(|t| t.starts_with("multipart/form-data; boundary=")),
            "uploads are multipart"
        );
        assert_eq!(request.query_param("networkId").as_deref(), Some("n1"));
        Reply::json(
            201,
            json!({"id": format!("u{i}"), "url": format!("/files/u{i}/tok/f{i}.png"),
                   "name": format!("f{i}.png"), "size": 4, "contentType": "image/png",
                   "width": 10, "height": 10}),
        )
    });
    server.route_fn("POST", "/api/v1/conversations/*/messages", |request| {
        let body = request.json();
        Reply::json(
            200,
            json!({"clientId": body["clientId"], "conversationId": "c-general",
                   "state": "queued"}),
        )
    });
}

fn png(name: &str) -> FileData {
    FileData::named(name, vec![0x89, b'P', b'N', b'G'])
}

fn add(f: &Fixture, cx: &mut TestAppContext, files: Vec<FileData>) {
    let session = f.session.clone();
    cx.update_window(f.window, |_, window, cx| {
        tray::add(
            &session,
            "c-general",
            files.into_iter().map(|f| (f, None)).collect(),
            window,
            cx,
        );
        window.render_frame(cx);
    })
    .unwrap();
}

fn pending(cx: &mut TestAppContext) -> Vec<tray::Pending> {
    cx.update(|cx| Uploads::pending(cx, "c-general"))
}

fn attach_in_a_reply(cx: &mut TestAppContext) -> Fixture {
    let f = open_with(cx, true, |server| {
        let mut snapshot = upload_snapshot(Some(false));
        snapshot.networks[0].account = Some("me".into());
        snapshot.networks[0].features.replies = true;
        server.snapshot(&snapshot);
        let mut first = message("1", "c-general", "Can you share the picture?");
        first.upstream_id = Some("up-1".into());
        server.route(
            "GET",
            "/api/v1/conversations/*/messages",
            200,
            json!({
                "items": [first, message("2", "c-general", "Thanks")]
            }),
        );
        uploads_answer(server, Duration::ZERO);
    });
    bootstrap(&f, cx);
    add(&f, cx, vec![png("picture.png")]);
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.hover("message-1", cx);
        window.click("message-toolbar-reply", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update(|cx| {
        assert!(matches!(message_actions::mode("c-general", cx),
        Some(message_actions::Mode::Reply { message, .. }) if message == "1"))
    });
    f
}

#[gpui_kit::test]
fn attachments_without_text_are_sent_as_the_reply(cx: &mut TestAppContext) {
    let f = attach_in_a_reply(cx);
    cx.update_window(f.window, |_, window, cx| window.click("composer-send", cx))
        .unwrap();
    const PATH: &str = "/api/v1/conversations/c-general/messages";
    wait(cx, "the attachment reply", |_| {
        !f.server.requests_to("POST", PATH).is_empty()
    });
    let messages = f.server.requests_to("POST", PATH);
    assert_eq!(messages.len(), 1);
    let body = messages[0].json();
    assert_eq!(body["replyToId"], "1");
    assert_eq!(body["attachments"], json!(["u1"]));
    cx.update(|cx| assert_eq!(message_actions::mode("c-general", cx), None));
}

#[gpui_kit::test]
fn a_text_reply_also_sends_queued_files_as_a_separate_message(cx: &mut TestAppContext) {
    let f = attach_in_a_reply(cx);
    cx.update_window(f.window, |_, window, cx| {
        window.input("Here it is", cx);
        window.press("enter", cx);
    })
    .unwrap();
    const PATH: &str = "/api/v1/conversations/c-general/messages";
    wait(cx, "the reply and the file message", |_| {
        f.server.requests_to("POST", PATH).len() >= 2
    });
    let sent: Vec<_> = f
        .server
        .requests_to("POST", PATH)
        .iter()
        .map(|request| request.json())
        .collect();
    assert_eq!(sent.len(), 2);
    let reply = sent
        .iter()
        .find(|body| body["text"] == "Here it is")
        .unwrap();
    assert_eq!(reply["replyToId"], "1");
    assert!(reply["attachments"].is_null());
    let files = sent
        .iter()
        .find(|body| body["attachments"] == json!(["u1"]))
        .unwrap();
    assert!(files["replyToId"].is_null());
    cx.update(|cx| assert_eq!(message_actions::mode("c-general", cx), None));
}

fn choose_attachment_action(f: &Fixture, cx: &mut TestAppContext, label: &str) {
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.click("composer-attach", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        let item = window.find_by_label(label);
        assert!(item.visible(), "{label} is visible");
        let at = item.bounds().center();
        window.pointer_down(at, MouseButton::Left, cx);
        window.pointer_up(at, MouseButton::Left, cx);
    })
    .unwrap();
    cx.run_until_parked();
}

#[gpui_kit::test]
fn the_attachment_menu_opens_the_file_picker_gifs_and_commands(cx: &mut TestAppContext) {
    let f = open_with(cx, true, |s| server(s, Some(false)));
    bootstrap(&f, cx);
    let path = cx.update(|cx| f.session.read(cx).config.path().join("menu.pdf"));
    std::fs::write(&path, b"%PDF-1.7\n").unwrap();

    let limit = ircord_client::api::uploads::human_size(1_000_000);
    choose_attachment_action(&f, cx, &format!("Upload files (up to {limit} each)"));
    assert!(cx.did_prompt_for_paths());
    cx.simulate_path_prompt_response(move |_| Some(vec![path]));
    wait(cx, "the selected file in the tray", |cx| {
        Uploads::pending(cx, "c-general")
            .iter()
            .any(|file| file.file.name == "menu.pdf")
    });

    choose_attachment_action(&f, cx, "Send a GIF");
    cx.update(|cx| assert_eq!(klipy::open_tab(cx), Some(Tab::Gifs)));
    cx.update_window(f.window, |_, window, cx| {
        window.press("escape", cx);
        window.click("composer-input", cx);
        window.input("Keep this draft", cx);
    })
    .unwrap();
    cx.run_until_parked();
    choose_attachment_action(&f, cx, "Use a slash command");
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.find("composer-suggestions").visible());
        assert_eq!(window.find_by_label("Message #general").value(), Some("/"));
        window.press(
            if cfg!(target_os = "macos") {
                "cmd-z"
            } else {
                "ctrl-z"
            },
            cx,
        );
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find_by_label("Message #general").value(),
            Some("Keep this draft")
        );
    })
    .unwrap();
    assert!(f.server.requests_to("POST", "/api/v1/uploads").is_empty());
    assert!(
        f.server
            .requests_to("POST", "/api/v1/conversations/c-general/messages")
            .is_empty()
    );
}

#[gpui_kit::test]
fn the_attachment_menu_keeps_gifs_and_commands_available_without_uploads(cx: &mut TestAppContext) {
    let f = open_with(cx, true, |_| {});
    bootstrap(&f, cx);
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.click("composer-attach", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.find_by_label("Send a GIF").visible());
        assert!(window.find_by_label("Use a slash command").visible());
        let at = window.find_by_label("Upload files").bounds().center();
        window.pointer_down(at, MouseButton::Left, cx);
        window.pointer_up(at, MouseButton::Left, cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert!(!cx.did_prompt_for_paths());
}

#[gpui_kit::test]
fn a_batch_uploads_and_sends_as_one_message(cx: &mut TestAppContext) {
    let f = open_with(cx, true, |s| {
        server(s, Some(false));
        uploads_answer(s, Duration::ZERO);
    });
    bootstrap(&f, cx);
    add(&f, cx, vec![png("one.png"), png("two.png")]);
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.find("upload-tray").visible());
        assert!(window.find("upload-item-1").visible());
        assert_eq!(window.find("upload-status-2").label(), Some("4 B"));
        assert!(window.find("composer-attach").visible());
        // The Send button appears for queued files even with an empty message.
        assert!(window.find("composer-send").visible());
        window.click("composer-send", cx);
    })
    .unwrap();
    wait(cx, "the message with the files", |_| {
        !f.server
            .requests_to("POST", "/api/v1/conversations/c-general/messages")
            .is_empty()
    });
    let sent = f
        .server
        .requests_to("POST", "/api/v1/conversations/c-general/messages")
        .remove(0);
    let body = sent.json();
    assert_eq!(body["attachments"], json!(["u1", "u2"]));
    let origin = f.server.address().origin();
    assert_eq!(
        body["text"],
        json!(format!(
            "{origin}/files/u1/tok/f1.png {origin}/files/u2/tok/f2.png"
        ))
    );
    assert_eq!(f.server.requests_to("POST", "/api/v1/uploads").len(), 2);
    wait(cx, "the tray to empty", |cx| {
        Uploads::pending(cx, "c-general").is_empty()
    });
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("upload-tray").is_none());
    })
    .unwrap();
}

#[gpui_kit::test]
fn a_message_upload_is_marked_as_one(cx: &mut TestAppContext) {
    let f = open_with(cx, true, |s| {
        server(s, Some(false));
        uploads_answer(s, Duration::ZERO);
    });
    bootstrap(&f, cx);
    add(&f, cx, vec![png("one.png")]);
    cx.update_window(f.window, |_, window, cx| {
        window.click("composer-input", cx);
        window.press("enter", cx);
    })
    .unwrap();
    wait(cx, "the upload", |_| {
        !f.server.requests_to("POST", "/api/v1/uploads").is_empty()
    });
    let upload = f.server.requests_to("POST", "/api/v1/uploads").remove(0);
    assert_eq!(upload.query_param("use").as_deref(), Some("message"));
}

#[gpui_kit::test]
fn the_networks_own_limits_decide_what_joins(cx: &mut TestAppContext) {
    // The server takes pictures and PDFs up to 1 MB; this network's files go
    // to a host that takes only PDFs, up to 100 bytes.
    let f = open_with(cx, true, |s| {
        server(s, Some(false));
        s.route(
            "GET",
            "/api/v1/networks/n1/uploads",
            200,
            json!({"available": true, "canDelete": true, "maxBytes": 100,
                   "provider": "upstream", "types": ["application/pdf"]}),
        );
    });
    bootstrap(&f, cx);
    // Drawing the composer asks the network; wait until its answer is used,
    // which shows as the attach button naming the network's size.
    wait(cx, "the network's limits on the attach button", |cx| {
        cx.refresh_windows();
        cx.update_window(f.window, |_, window, cx| {
            window.render_frame(cx);
            window
                .try_find("composer-attach")
                .is_some_and(|b| b.description().is_some_and(|l| l.contains("100 B")))
        })
        .unwrap_or(false)
    });
    let before = pending(cx).len();
    add(&f, cx, vec![png("late.png")]);
    // A picture no longer fits: this network takes PDFs only.
    assert_eq!(pending(cx).len(), before, "the picture is refused");
    add(
        &f,
        cx,
        vec![FileData::new("small.pdf", "application/pdf", vec![0; 50])],
    );
    assert_eq!(pending(cx).len(), before + 1, "a small PDF joins");
    add(
        &f,
        cx,
        vec![FileData::new("big.pdf", "application/pdf", vec![0; 500])],
    );
    assert_eq!(
        pending(cx).len(),
        before + 1,
        "a PDF over the network's size is refused"
    );
}

#[gpui_kit::test]
fn files_the_server_does_not_take_stay_out(cx: &mut TestAppContext) {
    let f = open_with(cx, true, |s| server(s, Some(false)));
    bootstrap(&f, cx);
    add(
        &f,
        cx,
        vec![
            FileData::named("notes.txt", b"hi".to_vec()),
            FileData::named("big.png", vec![0; 1_000_001]),
            FileData::named("doc.pdf", b"%PDF".to_vec()),
        ],
    );
    let items = pending(cx);
    assert_eq!(items.len(), 1, "only the PDF joins");
    assert_eq!(items[0].file.name, "doc.pdf");
    assert_eq!(items[0].file.content_type, "application/pdf");
}

#[gpui_kit::test]
fn a_pasted_picture_joins_the_batch_and_text_pastes_as_usual(cx: &mut TestAppContext) {
    let f = open_with(cx, true, |s| server(s, Some(false)));
    bootstrap(&f, cx);
    cx.write_to_clipboard(ClipboardItem::new_image(&Image::from_bytes(
        ImageFormat::Png,
        vec![0x89, b'P', b'N', b'G'],
    )));
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        window.click("composer-input", cx);
        window.press("ctrl-v", cx);
    })
    .unwrap();
    let items = pending(cx);
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].file.name, "pasted-image.png");
    assert!(items[0].preview.is_some());

    cx.write_to_clipboard(ClipboardItem::new_string("just text".into()));
    cx.update_window(f.window, |_, window, cx| {
        window.press("ctrl-v", cx);
    })
    .unwrap();
    assert_eq!(pending(cx).len(), 1, "text is not a file");
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        let input = window.find_by_label("Message #general");
        assert_eq!(input.value(), Some("just text"));
    })
    .unwrap();
}

#[gpui_kit::test]
fn a_stalled_upload_says_so(cx: &mut TestAppContext) {
    // The server takes the file and does not answer until the test lets go of
    // it, so the stall watchdog is the only way out however slow the machine
    // is; a reply on a timer would race the watchdog.
    let (release, held) = held_reply(|| Reply::problem(503, "The upload server went away."));
    let f = open_with(cx, true, move |s| {
        server(s, Some(false));
        s.route_fn("POST", "/api/v1/uploads", held);
    });
    bootstrap(&f, cx);
    cx.update(|cx| Uploads::set_stall(cx, Duration::from_millis(200)));
    add(&f, cx, vec![png("slow.png")]);
    cx.update_window(f.window, |_, window, cx| {
        window.click("composer-input", cx);
        window.press("enter", cx);
    })
    .unwrap();
    wait(cx, "the stall", |cx| {
        Uploads::pending(cx, "c-general")
            .first()
            .is_some_and(|p| matches!(p.status, Status::Failed(_)))
    });
    let items = pending(cx);
    assert_eq!(
        items[0].status,
        Status::Failed("The upload stopped making progress. Try again.".into())
    );
    assert!(
        f.server
            .requests_to("POST", "/api/v1/conversations/c-general/messages")
            .is_empty()
    );
    drop(release);
}

#[gpui_kit::test]
fn a_refused_upload_shows_the_servers_reason(cx: &mut TestAppContext) {
    let f = open_with(cx, true, |s| {
        server(s, Some(false));
        s.route_reply(
            "POST",
            "/api/v1/uploads",
            Reply::problem(413, "Your upload quota is used up."),
        );
    });
    bootstrap(&f, cx);
    add(&f, cx, vec![png("one.png")]);
    cx.update_window(f.window, |_, window, cx| {
        window.click("composer-input", cx);
        window.press("enter", cx);
    })
    .unwrap();
    wait(cx, "the refusal", |cx| {
        Uploads::pending(cx, "c-general")
            .first()
            .is_some_and(|p| matches!(p.status, Status::Failed(_)))
    });
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find("upload-status-1").label(),
            Some("Your upload quota is used up.")
        );
    })
    .unwrap();
    // The file stays in the tray; once the server takes it, Enter sends it.
    uploads_answer(&f.server, Duration::ZERO);
    cx.update_window(f.window, |_, window, cx| {
        window.press("enter", cx);
    })
    .unwrap();
    wait(cx, "the retried batch", |cx| {
        Uploads::pending(cx, "c-general").is_empty()
    });
    let sent = f
        .server
        .requests_to("POST", "/api/v1/conversations/c-general/messages");
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].json()["attachments"], json!(["u1"]));
}

#[gpui_kit::test]
fn removing_an_uploading_file_cancels_it(cx: &mut TestAppContext) {
    // The upload is still in flight when the test cancels it: the server holds
    // its answer until the cancel is done.
    let (release, held) = held_reply(|| {
        Reply::json(
            201,
            json!({"id": "u1", "url": "/files/u1/tok/f1.png", "name": "f1.png",
                   "size": 4, "contentType": "image/png", "width": 10, "height": 10}),
        )
    });
    let f = open_with(cx, true, move |s| {
        server(s, Some(false));
        s.route_fn("POST", "/api/v1/uploads", held);
        s.route_fn("POST", "/api/v1/conversations/*/messages", |request| {
            Reply::json(
                200,
                json!({"clientId": request.json()["clientId"], "conversationId": "c-general",
                       "state": "queued"}),
            )
        });
    });
    bootstrap(&f, cx);
    add(&f, cx, vec![png("one.png")]);
    cx.update_window(f.window, |_, window, cx| {
        window.click("composer-input", cx);
        window.press("enter", cx);
    })
    .unwrap();
    wait(cx, "the upload to start", |_| {
        !f.server.requests_to("POST", "/api/v1/uploads").is_empty()
    });
    assert_eq!(pending(cx)[0].status, Status::Uploading);
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find("upload-remove-1").label(),
            Some("Cancel upload of one.png")
        );
        window.click("upload-remove-1", cx);
    })
    .unwrap();
    assert!(pending(cx).is_empty());
    drop(release);
    std::thread::sleep(Duration::from_millis(1200));
    cx.run_until_parked();
    assert!(
        f.server
            .requests_to("POST", "/api/v1/conversations/c-general/messages")
            .is_empty(),
        "nothing is sent"
    );
}

/// `#general`'s history: one message with two pictures and a PDF.
fn gallery_history(server: &CannedServer) {
    server.route_fn("GET", "/api/v1/conversations/*/messages", |_| {
        let mut m = message(
            "g1",
            "c-general",
            "https://x.test/files/a/t/a.png https://x.test/files/b/t/b.gif https://x.test/files/c/t/c.pdf",
        );
        let picture = |id: &str, kind: &str| Attachment {
            url: format!("https://x.test/files/{id}/t/{id}.{kind}"),
            name: format!("{id}.{kind}"),
            width: 800,
            height: 600,
            content_type: Some(format!("image/{kind}")),
            size: Some(1000),
            ..Default::default()
        };
        m.attachments = Some(vec![
            picture("a", "png"),
            picture("b", "gif"),
            Attachment {
                url: "https://x.test/files/c/t/c.pdf".into(),
                name: "c.pdf".into(),
                content_type: Some("application/pdf".into()),
                size: Some(2_500_000),
                ..Default::default()
            },
        ]);
        Reply::json(
            200,
            serde_json::to_value(MessagePage {
                items: vec![m],
                ..Default::default()
            })
            .unwrap(),
        )
    });
}

fn bootstrap_one(f: &Fixture, cx: &mut TestAppContext) {
    wait(cx, "#general's history", |cx| {
        let session = f.session.read(cx);
        session.client().is_some_and(|c| {
            c.read()
                .timelines
                .get("c-general")
                .is_some_and(|t| t.loaded && t.entries.len() == 1)
        })
    });
}

#[gpui_kit::test]
fn a_gallery_opens_the_viewer_and_pages_through(cx: &mut TestAppContext) {
    let f = open_with(cx, true, |s| {
        server(s, Some(false));
        gallery_history(s);
    });
    bootstrap_one(&f, cx);
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        let gallery = window.find("gallery-g1");
        assert_eq!(gallery.label(), Some("Gallery of 2 files"));
        let size = gallery.bounds().size;
        assert_eq!(
            (size.width, size.height),
            (gpui_kit::px(440.), gpui_kit::px(280.)),
            "the space is reserved"
        );
        assert!(window.find("attachment-g1-1").visible());
        assert_eq!(
            window.find("file-g1-0-detail").label(),
            Some("PDF document · 2.5 MB")
        );
        assert!(window.find("file-g1-0-download").visible());
        // The text without the uploads' URLs is empty.
        window.click("attachment-g1-0", cx);
        window.render_frame(cx);
        assert_eq!(
            window.find("viewer-position").label(),
            Some("a.png · 1 of 2")
        );
        window.press("right", cx);
        assert_eq!(
            window.find("viewer-position").label(),
            Some("b.gif · 2 of 2")
        );
        window.click("viewer-zoom-in", cx);
        window.render_frame(cx);
        assert_eq!(window.find("viewer-zoom").label(), Some("125%"));
        window.click("viewer-copy", cx);
    })
    .unwrap();
    assert_eq!(
        cx.read_from_clipboard().and_then(|c| c.text()).as_deref(),
        Some("https://x.test/files/b/t/b.gif")
    );
    cx.update_window(f.window, |_, window, cx| {
        window.press("escape", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("viewer").is_none(), "Escape closes it");
    })
    .unwrap();
}

#[gpui_kit::test]
fn sensitive_media_stays_covered_until_shown(cx: &mut TestAppContext) {
    let f = open_with(cx, true, |s| {
        server(s, Some(true));
        gallery_history(s);
    });
    bootstrap_one(&f, cx);
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("media-cover-g1").label(), Some("NSFW media"));
        assert!(window.try_find("gallery-g1").is_none());
        window.click("media-reveal-g1", cx);
        window.render_frame(cx);
        assert!(window.find("gallery-g1").visible());
    })
    .unwrap();
}

#[gpui_kit::test]
fn unknown_channel_settings_cover_media_too(cx: &mut TestAppContext) {
    let f = open_with(cx, true, |s| {
        server(s, None);
        gallery_history(s);
    });
    bootstrap_one(&f, cx);
    cx.update_window(f.window, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find("media-cover-g1").label(),
            Some("Channel settings unavailable")
        );
    })
    .unwrap();
}

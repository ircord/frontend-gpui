//! Live cases for package U1: upload a file into a message.
//!
//! Start each case with `let Some((server, irc)) = crate::support::environment() else { return };`.

use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::StreamExt;
use ircord_client::api::Api;
use ircord_client::api::uploads::{self, FileData, Progress};
use ircord_client::client::{random_id, runtime, sign_in};
use ircord_client::credentials::{CredentialStore, MemoryStore};
use ircord_client::store::Connection;
use ircord_client::{Client, ServerAddress};

use crate::support::{environment, setup_account, unique, wait_for_backend};

/// A 1×1 transparent PNG.
const PIXEL: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4,
    0x89, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0x00, 0x01, 0x00, 0x00,
    0x05, 0x00, 0x01, 0x0d, 0x0a, 0x2d, 0xb4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae,
    0x42, 0x60, 0x82,
];

#[test]
fn uploads_a_picture_and_sees_it_in_the_message() {
    let Some((server, irc)) = environment() else {
        return;
    };
    let channel = format!("#{}", unique("up"));
    let rt = runtime().unwrap();
    let address = ServerAddress::parse(&server).unwrap();
    let username = unique("gpui");
    let password = random_id();
    wait_for_backend(&rt, &address);
    rt.block_on(setup_account(
        &address, &username, &password, &irc, &channel,
    ));

    let api = Api::new(address.clone());
    let store: Arc<dyn CredentialStore> = Arc::new(MemoryStore::default());
    rt.block_on(sign_in(&api, store.as_ref(), &username, &password))
        .expect("sign in");
    let (client, mut updates) = Client::start(rt.clone(), api.clone());
    let mut pump = |what: &str, deadline: Instant, done: &dyn Fn(&Client) -> bool| {
        while !done(&client) {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            rt.block_on(async {
                let _ = tokio::time::timeout(Duration::from_millis(300), updates.next()).await;
            });
        }
    };
    pump("live", Instant::now() + Duration::from_secs(30), &|c| {
        c.read().connection == Some(Connection::Live)
    });
    let name = channel.trim_start_matches('#').to_string();
    pump(
        "the channel",
        Instant::now() + Duration::from_secs(45),
        &|c| {
            c.read()
                .conversations
                .iter()
                .any(|x| x.name.eq_ignore_ascii_case(&name) && x.joined)
        },
    );
    let (conversation, network) = {
        let w = client.read();
        let c = w
            .conversations
            .iter()
            .find(|x| x.name.eq_ignore_ascii_case(&name))
            .unwrap();
        (c.id.clone(), c.network_id.clone())
    };
    let capability = client
        .read()
        .capabilities
        .upload
        .clone()
        .expect("the test backend takes uploads");
    let file = FileData::named("pixel.png", PIXEL.to_vec());
    assert_eq!(
        uploads::check(Some(&capability), 0, &file.content_type, file.size()),
        Ok(())
    );

    // Upload, counting progress, then send it as a one-file batch.
    let progress = Progress::default();
    let upload = rt
        .block_on(api.upload_file(&network, &file, &progress, uploads::STALL))
        .expect("upload");
    assert_eq!(progress.sent(), PIXEL.len() as u64, "every byte counted");
    assert_eq!(upload.content_type, "image/png");
    let url = api.file_url(&upload.url);
    let body = uploads::batch_message(random_id(), &[(upload.clone(), url.clone())], None);
    let answer = rt
        .block_on(api.send_message(&conversation, &body))
        .expect("send the batch");
    client.change(|w| w.send_answered(&answer));

    client.open(&conversation);
    pump(
        "the message with the file",
        Instant::now() + Duration::from_secs(20),
        &|c| {
            c.read().timelines.get(&conversation).is_some_and(|t| {
                t.entries.iter().any(|e| {
                    e.message
                        .attachment
                        .as_ref()
                        .is_some_and(|a| a.name == "pixel.png" && a.url.contains(&upload.id))
                })
            })
        },
    );

    // The file is there to download.
    let (status, content_type, bytes) = rt
        .block_on(ircord_client::api::fetch(&url, 1 << 20))
        .expect("fetch the file");
    assert_eq!(status, 200);
    assert_eq!(bytes, PIXEL);
    assert!(content_type.is_some_and(|t| t.starts_with("image/png")));

    // An upload that is never sent can be deleted.
    let spare = rt
        .block_on(api.upload_file(&network, &file, &Progress::default(), uploads::STALL))
        .expect("second upload");
    rt.block_on(api.delete_upload(&spare.id))
        .expect("delete the unsent upload");
}

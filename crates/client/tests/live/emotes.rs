//! Live cases for package S5: upload, rename, remove an emoji.
//! And for package E1, which shares the emotes area: read the catalogs,
//! count an emoji pick, save and forget a GIF, send a GIF message.
//!
//! Start each case with `let Some((server, irc)) = crate::support::environment() else { return };`.

use std::time::{Duration, Instant};

use futures::StreamExt;
use ircord_client::api::Api;
use ircord_client::client::{random_id, runtime};
use ircord_client::contract as c;
use ircord_client::store::Connection;
use ircord_client::store::emoji;
use ircord_client::{Client, ServerAddress};

use crate::support::{environment, setup_account, unique, wait_for_backend};

fn klipy_gif(id: &str) -> c::GIF {
    c::GIF {
        id: id.into(),
        slug: format!("cat-{id}"),
        title: "Cat".into(),
        url: format!("https://static.klipy.com/ii/{id}/hd.gif"),
        preview_url: format!("https://static.klipy.com/ii/{id}/sm.webp"),
        still_url: format!("https://static.klipy.com/ii/{id}/sm.jpg"),
        width: 400,
        height: 300,
        provider: "klipy".into(),
    }
}

#[test]
fn e1_catalogs_library_and_a_gif_message() {
    let Some((server, irc)) = environment() else {
        return;
    };
    let channel = format!("#{}", unique("emoji"));
    let rt = runtime().unwrap();
    let address = ServerAddress::parse(&server).unwrap();
    let username = unique("gpui");
    let password = random_id();
    wait_for_backend(&rt, &address);
    // The sign-up's session is enough: no second sign-in counts against
    // the backend's limit on attempts from this address.
    let cookie = rt.block_on(setup_account(
        &address, &username, &password, &irc, &channel,
    ));
    let api = Api::new(address.clone());
    api.set_cookie(Some(cookie));

    // Both catalogs read, and fill the store.
    let instance = rt.block_on(api.emotes()).expect("the emote catalog");
    let bundled = rt
        .block_on(api.app_emoji_catalog())
        .expect("the bundled emoji catalog");
    assert!(
        bundled.items.iter().any(|e| e.shortcode == "ircord"),
        "the workspace emoji"
    );
    let mut state = emoji::State::default();
    assert!(state.begin_load());
    state.load_done(Ok(instance), Ok(bundled));
    assert!(state.by_shortcode("ircord").is_some());

    // A pick counts toward "Frequently used".
    let patch = c::LibraryPatch {
        recent: Some(vec!["1F600".into()]),
        uses: Some([("1F600".to_string(), 1)].into()),
        ..Default::default()
    };
    let library = rt
        .block_on(api.update_library(&patch))
        .expect("count a pick");
    assert!(
        library
            .emoji_usage
            .iter()
            .any(|u| u.id == "1F600" && u.count == 1)
    );

    // A GIF is saved and forgotten.
    let gif = klipy_gif("e1-live");
    let save = c::SavePickerGIF {
        id: gif.id.clone(),
        slug: gif.slug.clone(),
        title: gif.title.clone(),
        url: gif.url.clone(),
        preview_url: gif.preview_url.clone(),
        still_url: gif.still_url.clone(),
        width: gif.width,
        height: gif.height,
        provider: gif.provider.clone(),
    };
    let library = rt.block_on(api.save_gif(&save)).expect("save a GIF");
    assert_eq!(library.gifs.first().map(|g| g.id.as_str()), Some("e1-live"));
    let library = rt
        .block_on(api.delete_gif(&gif.id))
        .expect("forget the GIF");
    assert!(library.gifs.iter().all(|g| g.id != gif.id));

    // A GIF message goes out with its GIF.
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
    let conversation = client
        .read()
        .conversations
        .iter()
        .find(|x| x.name.eq_ignore_ascii_case(&name))
        .unwrap()
        .id
        .clone();
    let body = c::MessageInputBody {
        client_id: random_id(),
        text: gif.url.clone(),
        kind: Some(c::MessageInputBodyKind::Message),
        gif: Some(gif.clone()),
        ..Default::default()
    };
    let answer = rt
        .block_on(api.send_message(&conversation, &body))
        .expect("send the GIF");
    client.change(|w| w.send_answered(&answer));
    client.open(&conversation);
    pump(
        "the GIF message",
        Instant::now() + Duration::from_secs(20),
        &|c| {
            c.read().timelines.get(&conversation).is_some_and(|t| {
                t.entries
                    .iter()
                    .any(|e| e.message.gif.as_ref().is_some_and(|g| g.id == "e1-live"))
            })
        },
    );
}

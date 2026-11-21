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

/// An 8×8 red PNG.
const RED_PNG: [u8; 75] = [
    137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 8, 0, 0, 0, 8, 8, 6, 0,
    0, 0, 196, 15, 190, 139, 0, 0, 0, 18, 73, 68, 65, 84, 120, 156, 99, 248, 207, 192, 240, 31, 31,
    102, 24, 25, 10, 0, 194, 215, 127, 129, 47, 113, 224, 1, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66,
    96, 130,
];

#[test]
fn s5_upload_rename_and_remove_an_emoji() {
    use ircord_client::api::emotes::{NewEmote, image_type};

    let Some((server, irc)) = environment() else {
        return;
    };
    let channel = format!("#{}", unique("emotes"));
    let rt = runtime().unwrap();
    let address = ServerAddress::parse(&server).unwrap();
    wait_for_backend(&rt, &address);
    let cookie = rt.block_on(setup_account(
        &address,
        &unique("gpui"),
        &random_id(),
        &irc,
        &channel,
    ));
    let api = Api::new(address);
    api.set_cookie(Some(cookie));

    // Any member may add; what they add is theirs to manage.
    let catalog = rt.block_on(api.emotes()).expect("the emote catalog");
    assert!(catalog.can_add, "a member may add emoji");
    let content_type = image_type(&RED_PNG).unwrap();
    assert!(
        catalog.types.iter().flatten().any(|t| t == content_type),
        "the server takes PNG"
    );

    // `s5` and eight hex digits: a valid shortcode.
    let code = unique("s5");
    let uploaded = rt
        .block_on(api.upload_emote(&NewEmote {
            file_name: "red.png".into(),
            content_type: content_type.into(),
            bytes: RED_PNG.to_vec(),
            still: None,
            shortcode: code.clone(),
            aliases: vec![format!("{code}_a")],
        }))
        .expect("upload an emoji");
    assert_eq!(uploaded.shortcode, code);
    assert!(uploaded.can_manage && uploaded.enabled);
    let has_alias = |e: &c::Emote, alias: &str| e.aliases.iter().flatten().any(|a| a == alias);
    assert!(has_alias(&uploaded, &format!("{code}_a")));

    let listed = |api: &Api| -> Vec<c::Emote> {
        rt.block_on(api.emotes())
            .expect("the emote catalog")
            .items
            .unwrap_or_default()
    };
    assert!(listed(&api).iter().any(|e| e.id == uploaded.id));

    // Renamed: the old name still draws (as the name kept or a former one).
    let renamed_code = format!("{code}_r");
    let renamed = rt
        .block_on(api.update_emote(
            &uploaded.id,
            &c::EmotePatch {
                shortcode: Some(renamed_code.clone()),
                ..Default::default()
            },
        ))
        .expect("rename the emoji");
    assert_eq!(renamed.shortcode, renamed_code);
    assert!(
        renamed.retired.iter().flatten().any(|r| r == &code) || has_alias(&renamed, &code),
        "the former name is kept for old messages"
    );

    // New aliases, then disabled.
    let edited = rt
        .block_on(api.update_emote(
            &uploaded.id,
            &c::EmotePatch {
                aliases: Some(vec![format!("{code}_b")]),
                ..Default::default()
            },
        ))
        .expect("change the aliases");
    assert!(has_alias(&edited, &format!("{code}_b")));
    assert!(!has_alias(&edited, &format!("{code}_a")));
    let disabled = rt
        .block_on(api.update_emote(
            &uploaded.id,
            &c::EmotePatch {
                enabled: Some(false),
                ..Default::default()
            },
        ))
        .expect("disable the emoji");
    assert!(!disabled.enabled);

    // Removed.
    rt.block_on(api.delete_emote(&uploaded.id))
        .expect("remove the emoji");
    assert!(listed(&api).iter().all(|e| e.id != uploaded.id));
}

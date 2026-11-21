//! Live cases for package CH2: topic, modes, bans.
//!
//! Start each case with `let Some((server, irc)) = crate::support::environment() else { return };`.

use std::time::{Duration, Instant};

use futures::StreamExt;
use ircord_client::api::{Api, ApiError};
use ircord_client::channel_modes as modes;
use ircord_client::client::{random_id, runtime};
use ircord_client::contract as c;
use ircord_client::store::{Connection, server_log};
use ircord_client::{Client, ServerAddress};

use crate::support::{environment, setup_account, unique, wait_for_backend};

#[test]
fn an_operator_sets_the_topic_a_mode_and_a_ban() {
    let Some((server, irc)) = environment() else {
        return;
    };
    let home = format!("#{}", unique("cs"));
    let rt = runtime().unwrap();
    let address = ServerAddress::parse(&server).unwrap();
    let username = unique("gpui");
    let password = random_id();
    wait_for_backend(&rt, &address);
    let cookie = rt.block_on(setup_account(&address, &username, &password, &irc, &home));
    let api = Api::new(address.clone());
    api.set_cookie(Some(cookie));
    let (client, mut updates) = Client::start(rt.clone(), api.clone());
    let mut pump = |what: &str, done: &dyn Fn(&Client) -> bool| {
        let deadline = Instant::now() + Duration::from_secs(45);
        while !done(&client) {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            rt.block_on(async {
                let _ = tokio::time::timeout(Duration::from_millis(300), updates.next()).await;
            });
        }
    };
    pump("live", &|c| c.read().connection == Some(Connection::Live));
    // The first in a new channel is its operator.
    pump("operator in the home channel", &|c| {
        let w = c.read();
        w.conversations.iter().any(|x| {
            x.name.eq_ignore_ascii_case(&home[1..])
                && x.joined
                && x.member_modes.values().any(|m| m.contains('@'))
        })
    });
    let (id, network) = {
        let w = client.read();
        let x = w
            .conversations
            .iter()
            .find(|x| x.name.eq_ignore_ascii_case(&home[1..]))
            .unwrap();
        (x.id.clone(), x.network_id.clone())
    };
    let send = |line: String| {
        rt.block_on(server_log::send_raw(client.handle(), &network, line))
            .unwrap()
            .expect("send");
    };

    send(modes::topic_line(&home, "Set from **GPUI**"));
    pump("the topic", &|c| {
        c.read()
            .conversation(&id)
            .is_some_and(|x| x.topic == "Set from \x02GPUI\x02")
    });

    send(modes::toggle_line(&home, 'm', true));
    pump("moderated", &|c| {
        c.read()
            .conversation(&id)
            .is_some_and(|x| x.modes.contains('m'))
    });
    let isupport = client.read().networks[0].isupport.clone();
    let chanmodes = modes::Chanmodes::parse(&isupport);
    send(modes::set_value_line(&home, 'l', "42").unwrap());
    pump("the user limit", &|c| {
        c.read().conversation(&id).is_some_and(|x| {
            x.mode_params
                .as_ref()
                .and_then(|p| p.get("l"))
                .is_some_and(|v| v == "42")
        })
    });
    // The network takes the removal, but the backend's channel state (girc
    // 1.1.3 `CModes.Apply`) keeps modes a `-` removes until the channel is
    // joined again, so only that the line went out is checked here.
    send(modes::clear_value_line(&home, 'l', &chanmodes, Some("42")));

    // A ban typed as a nick is stored the way the preview says.
    let extbans = modes::Extbans::parse(&isupport);
    let preview = modes::preview_mask("spammer", extbans.as_ref());
    assert_eq!(preview.mask, "spammer!*@*");
    send(modes::list_line(&home, 'b', &preview.mask, true));
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let bans = rt.block_on(api.mode_list(&id, 'b')).expect("bans");
        if bans.iter().any(|b| b.mask == "spammer!*@*") {
            break;
        }
        assert!(Instant::now() < deadline, "the ban never listed: {bans:?}");
        std::thread::sleep(Duration::from_millis(300));
    }
    send(modes::list_line(&home, 'b', "spammer!*@*", false));
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let bans = rt.block_on(api.mode_list(&id, 'b')).expect("bans");
        if bans.is_empty() {
            break;
        }
        assert!(Instant::now() < deadline, "the ban stayed: {bans:?}");
        std::thread::sleep(Duration::from_millis(300));
    }

    // The channel-wide settings, as an operator.
    let changed = rt
        .block_on(api.channel_settings(
            &id,
            &c::ChannelSettingsPatchBody {
                nsfw: Some(true),
                group_mentions: Some(c::ChannelSettingsPatchBodyGroupMentions::All),
                ..Default::default()
            },
        ))
        .expect("channel settings");
    let shared = changed.channel_settings.expect("settings");
    assert!(shared.nsfw);
    assert_eq!(shared.group_mentions, c::ChannelSettingsGroupMentions::All);

    // Not on the server's own network: no guide tab.
    let listing = rt.block_on(api.guide_listing(&id)).expect("guide listing");
    assert!(!listing.available);
    // Not logged in to services: not registered, or no ChanServ to manage.
    match rt.block_on(api.channel_registration(&id)) {
        Ok(r) => assert!(!r.registered, "{r:?}"),
        Err(ApiError::Http { status, .. }) => assert!(matches!(status, 501 | 403), "{status}"),
        Err(e) => panic!("{e}"),
    }
}

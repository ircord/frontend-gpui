//! Live cases for package P1: whois and status, through a real backend and
//! IRC server — look up a plain IRC user, find them with the people lookup,
//! set and clear a status that clears after an hour, and go away and back.

use std::time::{Duration, Instant};

use ircord_client::ServerAddress;
use ircord_client::api::Api;
use ircord_client::api::people::{PeopleQuery, Purpose};
use ircord_client::client::runtime;
use ircord_client::store::people::{self, Expiry};
use ircord_contract as c;

use crate::support::{
    environment, irc_host_for_test, irc_user, setup_account, unique, wait_for_backend,
};

const CHANNEL: &str = "#gpui-p1";

#[test]
fn whois_and_status() {
    let Some((server, irc)) = environment() else {
        return;
    };
    let rt = runtime().unwrap();
    let address = ServerAddress::parse(&server).unwrap();
    wait_for_backend(&rt, &address);
    let username = unique("p1");
    let cookie = rt.block_on(setup_account(
        &address,
        &username,
        "long-enough-password-p1",
        &irc_host_for_test(&irc),
        CHANNEL,
    ));
    let api = Api::new(address);
    api.set_cookie(Some(cookie));
    let them = unique("pal");

    rt.block_on(async {
        // A plain IRC user in the same channel.
        let (_lines, _writer) = irc_user(&irc_host_for_test(&irc), &them, CHANNEL).await;

        // Wait for the network to connect and the channel to be joined.
        let deadline = Instant::now() + Duration::from_secs(30);
        let network = loop {
            let snapshot: c::Snapshot = api.get("/api/v1/bootstrap").await.unwrap();
            let joined = snapshot.conversations.iter().find(|c| {
                c.name.trim_start_matches('#') == CHANNEL.trim_start_matches('#') && c.joined
            });
            let up = snapshot
                .networks
                .iter()
                .find(|n| n.status == c::NetworkStatus::Connected);
            if let (Some(_), Some(n)) = (joined, up) {
                break n.id.clone();
            }
            assert!(Instant::now() < deadline, "{CHANNEL} was never joined");
            tokio::time::sleep(Duration::from_millis(250)).await;
        };

        // WHOIS the IRC user, and someone nobody uses.
        let found = api.whois(&network, &them).await.unwrap();
        assert!(found.found, "{them} was not found: {found:?}");
        assert_eq!(found.nickname.to_lowercase(), them.to_lowercase());
        assert!(
            found.channels.iter().any(|ch| ch.ends_with(CHANNEL)),
            "{:?}",
            found.channels
        );
        let nobody = api.whois(&network, &unique("nobody")).await.unwrap();
        assert!(!nobody.found);

        // The people lookup offers them for a direct message.
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let page = api
                .people(&PeopleQuery {
                    network_id: network.clone(),
                    purpose: Purpose::Dm,
                    conversation_id: None,
                    query: them[..5].to_string(),
                    limit: 20,
                })
                .await
                .unwrap();
            let listed = page.items.iter().any(|p| {
                p.nickname
                    .as_deref()
                    .is_some_and(|n| n.eq_ignore_ascii_case(&them))
            });
            if listed {
                break;
            }
            assert!(Instant::now() < deadline, "{them} never showed: {page:?}");
            tokio::time::sleep(Duration::from_millis(250)).await;
        }

        // Set a status that clears in an hour, as the editor sends it.
        let me: c::Member = api
            .get(&format!(
                "/api/v1/networks/{}/profile",
                ircord_client::api::segment(&network)
            ))
            .await
            .unwrap();
        let update =
            people::status_update(&me, "🌴", "Testing P1", Expiry::Hour1, chrono::Utc::now())
                .unwrap()
                .expect("a change");
        let set = api.update_network_profile(&network, &update).await.unwrap();
        assert_eq!(set.status_text.as_deref(), Some("Testing P1"));
        assert_eq!(set.status_emoji.as_deref(), Some("🌴"));
        assert!(
            set.status_expires_at
                .as_deref()
                .is_some_and(|s| !s.is_empty())
        );
        assert!(people::status_showing(
            &set,
            chrono::Utc::now().timestamp_millis()
        ));

        // Clear it.
        let cleared = api
            .update_network_profile(&network, &people::clear_status())
            .await
            .unwrap();
        assert!(
            cleared
                .status_text
                .as_deref()
                .unwrap_or_default()
                .is_empty()
        );
        assert!(!people::status_showing(
            &cleared,
            chrono::Utc::now().timestamp_millis()
        ));

        // Away and back.
        api.set_away(&network, Some("Away")).await.unwrap();
        api.set_away(&network, None).await.unwrap();
    });
}

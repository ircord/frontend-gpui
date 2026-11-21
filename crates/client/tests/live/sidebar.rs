//! Live cases for package N1: the calls behind the sidebar's menus and its
//! order, through a real backend — favourite a channel, keep a section
//! order in the preferences, leave the channel, disconnect and connect the
//! network and remove it.

use std::time::{Duration, Instant};

use ircord_client::ServerAddress;
use ircord_client::api::{Api, Method, segment};
use ircord_client::client::runtime;
use ircord_contract as c;

use crate::support::{environment, irc_host_for_test, setup_account, unique, wait_for_backend};

const CHANNEL: &str = "#gpui-n1";

#[test]
fn sidebar_menus_and_order() {
    let Some((server, irc)) = environment() else {
        return;
    };
    let rt = runtime().unwrap();
    let address = ServerAddress::parse(&server).unwrap();
    wait_for_backend(&rt, &address);
    let username = unique("n1");
    let cookie = rt.block_on(setup_account(
        &address,
        &username,
        "long-enough-password-n1",
        &irc_host_for_test(&irc),
        CHANNEL,
    ));
    let api = Api::new(address);
    api.set_cookie(Some(cookie));

    rt.block_on(async {
        // Wait for the auto-joined channel.
        let deadline = Instant::now() + Duration::from_secs(30);
        let (network, channel) = loop {
            let snapshot: c::Snapshot = api.get("/api/v1/bootstrap").await.unwrap();
            let found = snapshot
                .conversations
                .iter()
                .find(|c| c.name.trim_start_matches('#') == CHANNEL.trim_start_matches('#'))
                .map(|c| (c.network_id.clone(), c.id.clone()));
            if let Some(found) = found {
                break found;
            }
            assert!(Instant::now() < deadline, "{CHANNEL} never appeared");
            tokio::time::sleep(Duration::from_millis(250)).await;
        };
        let conversation = format!("/api/v1/conversations/{}", segment(&channel));

        // Favourite, and back.
        let body = c::ConversationPatchBody {
            favorite: Some(true),
            ..Default::default()
        };
        let favourite: c::Conversation =
            api.call(Method::PATCH, &conversation, &body).await.unwrap();
        assert!(favourite.favorite);
        let body = c::ConversationPatchBody {
            favorite: Some(false),
            ..Default::default()
        };
        let plain: c::Conversation = api.call(Method::PATCH, &conversation, &body).await.unwrap();
        assert!(!plain.favorite);

        // A section order follows the account.
        let key = format!("{network}/channels");
        let mut order = std::collections::BTreeMap::new();
        order.insert(key.clone(), Some(vec![channel.clone()]));
        let body = c::SaveAppPreferencesRequest {
            preferences: c::ClientPreferences {
                sidebar_order: Some(order),
                ..Default::default()
            },
        };
        let _: c::StoredPreferences = api
            .call(Method::PUT, "/api/v1/app/preferences", &body)
            .await
            .unwrap();
        let stored: c::StoredPreferences = api.get("/api/v1/app/preferences").await.unwrap();
        let saved = stored.preferences.unwrap().sidebar_order.unwrap();
        assert_eq!(saved[&key].as_deref(), Some(&[channel.clone()][..]));

        // Leave the channel while the network is connected.
        api.delete(&conversation).await.unwrap();

        // Disconnect, and once the backend says so, connect again.
        let path = format!("/api/v1/networks/{}", segment(&network));
        let status = |status| c::NetworkUpdate {
            status: Some(status),
            ..Default::default()
        };
        let _: c::Network = api
            .call(
                Method::PATCH,
                &path,
                &status(c::NetworkUpdateStatus::Disconnected),
            )
            .await
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let snapshot: c::Snapshot = api.get("/api/v1/bootstrap").await.unwrap();
            let down = snapshot
                .networks
                .iter()
                .any(|n| n.id == network && n.status == c::NetworkStatus::Disconnected);
            if down {
                break;
            }
            assert!(Instant::now() < deadline, "the network never disconnected");
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        let _: c::Network = api
            .call(
                Method::PATCH,
                &path,
                &status(c::NetworkUpdateStatus::Connected),
            )
            .await
            .unwrap();

        // Remove the network.
        api.delete(&path).await.unwrap();
        let snapshot: c::Snapshot = api.get("/api/v1/bootstrap").await.unwrap();
        assert!(snapshot.networks.iter().all(|n| n.id != network));
        assert!(snapshot.conversations.iter().all(|c| c.id != channel));
    });
}

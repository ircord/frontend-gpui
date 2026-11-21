//! Live cases for package PT1: invitations between two people, and groups.
//!
//! The pinned backend no longer creates groups (`POST /parties` answers 410,
//! "Create an ordinary channel instead"), so a group between two accounts
//! cannot be made here; the case lists your groups (none) and runs the
//! invitation path the banner uses on a channel: someone invites you, you
//! accept and are in the channel; invited again elsewhere, you decline and
//! the invitation is gone.

use std::time::{Duration, Instant};

use ircord_client::ServerAddress;
use ircord_client::api::Api;
use ircord_client::client::runtime;
use ircord_contract as c;
use tokio::io::AsyncWriteExt;

use crate::support::{
    environment, irc_host_for_test, irc_user, setup_account, unique, wait_for_backend,
};

const HOME: &str = "#gpui-pt1";

async fn invitation(api: &Api, channel: &str) -> c::Invitation {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let snapshot: c::Snapshot = api.get("/api/v1/bootstrap").await.unwrap();
        if let Some(i) = snapshot
            .invitations
            .into_iter()
            .find(|i| i.channel.eq_ignore_ascii_case(channel))
        {
            return i;
        }
        assert!(
            Instant::now() < deadline,
            "the invitation to {channel} never arrived"
        );
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

#[test]
fn invitations_accepted_and_declined() {
    let Some((server, irc)) = environment() else {
        return;
    };
    let rt = runtime().unwrap();
    let address = ServerAddress::parse(&server).unwrap();
    wait_for_backend(&rt, &address);
    let username = unique("pt");
    let cookie = rt.block_on(setup_account(
        &address,
        &username,
        "long-enough-password-pt1",
        &irc_host_for_test(&irc),
        HOME,
    ));
    let api = Api::new(address);
    api.set_cookie(Some(cookie));
    let them = unique("host");
    let open = format!("#gpui-pt1-{}", unique(""));
    let other = format!("#gpui-pt1-{}", unique(""));

    rt.block_on(async {
        // Wait for the network to connect and the home channel to be joined.
        let deadline = Instant::now() + Duration::from_secs(30);
        let nick = loop {
            let snapshot: c::Snapshot = api.get("/api/v1/bootstrap").await.unwrap();
            let joined = snapshot.conversations.iter().any(|c| {
                c.name.trim_start_matches('#') == HOME.trim_start_matches('#') && c.joined
            });
            let up = snapshot
                .networks
                .iter()
                .find(|n| n.status == c::NetworkStatus::Connected);
            if let (true, Some(n)) = (joined, up) {
                break n.nick.clone();
            }
            assert!(Instant::now() < deadline, "{HOME} was never joined");
            tokio::time::sleep(Duration::from_millis(250)).await;
        };

        // No groups for a new account.
        assert!(api.parties().await.unwrap().is_empty());

        // Someone opens two channels (so they are operator) and invites you.
        let (_lines, mut writer) = irc_user(&irc_host_for_test(&irc), &them, &open).await;
        writer
            .write_all(format!("JOIN {other}\r\nINVITE {nick} {open}\r\n").as_bytes())
            .await
            .unwrap();

        // Accept: you are in the channel, and the invitation is gone.
        let invited = invitation(&api, &open).await;
        assert!(invited.party.is_none(), "a channel invitation");
        assert!(invited.inviter_nick.eq_ignore_ascii_case(&them));
        let joined = api.accept_invitation(&invited.id).await.unwrap();
        assert!(
            joined
                .name
                .trim_start_matches('#')
                .eq_ignore_ascii_case(open.trim_start_matches('#')),
            "{joined:?}"
        );
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let snapshot: c::Snapshot = api.get("/api/v1/bootstrap").await.unwrap();
            let inside = snapshot
                .conversations
                .iter()
                .any(|c| c.id == joined.id && c.joined);
            let pending = snapshot.invitations.iter().any(|i| i.id == invited.id);
            if inside && !pending {
                break;
            }
            assert!(Instant::now() < deadline, "joining {open} never finished");
            tokio::time::sleep(Duration::from_millis(250)).await;
        }

        // Decline the next one: it is gone, and you did not join.
        writer
            .write_all(format!("INVITE {nick} {other}\r\n").as_bytes())
            .await
            .unwrap();
        let invited = invitation(&api, &other).await;
        api.decline_invitation(&invited.id).await.unwrap();
        let snapshot: c::Snapshot = api.get("/api/v1/bootstrap").await.unwrap();
        assert!(snapshot.invitations.iter().all(|i| i.id != invited.id));
        assert!(!snapshot.conversations.iter().any(|c| {
            c.name
                .trim_start_matches('#')
                .eq_ignore_ascii_case(other.trim_start_matches('#'))
                && c.joined
        }));
    });
}

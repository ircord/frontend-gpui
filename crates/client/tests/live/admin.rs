//! Live cases for package AD1: administration as an owner.
//!
//! Start each case with `let Some((server, irc)) = crate::support::environment() else { return };`.
//!
//! The channel guide and emoji management are the server's own, shared by
//! every case running at the same time, so this case saves them back as it
//! found them.

use std::sync::Arc;

use ircord_client::ServerAddress;
use ircord_client::api::Api;
use ircord_client::api::admin::{self as a, GuideDraft};
use ircord_client::client::{random_id, runtime, sign_in};
use ircord_client::contract as c;
use ircord_client::credentials::{CredentialStore, MemoryStore};

use crate::support::{environment, make_owner, setup_account, unique, wait_for_backend};

/// Sign up `username` without a network.
async fn sign_up(address: &ServerAddress, username: &str, password: &str) {
    let response = reqwest::Client::new()
        .post(address.join("/api/v1/auth/signup"))
        .header("Origin", address.origin())
        .json(&serde_json::json!({"username": username, "password": password}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 201, "sign up {username}");
}

#[test]
fn an_owner_reads_status_and_manages_owners_the_guide_and_emoji() {
    let Some((server, irc)) = environment() else {
        return;
    };
    let rt = runtime().unwrap();
    let address = ServerAddress::parse(&server).unwrap();
    let (username, password) = (unique("adm"), random_id());
    wait_for_backend(&rt, &address);
    rt.block_on(setup_account(
        &address,
        &username,
        &password,
        &irc,
        &format!("#{}", unique("ad")),
    ));
    let api = Api::new(address.clone());
    let store: Arc<dyn CredentialStore> = Arc::new(MemoryStore::default());
    rt.block_on(sign_in(&api, store.as_ref(), &username, &password))
        .expect("sign in");

    // Not an owner yet: the panel's requests are refused.
    assert!(!rt.block_on(api.administration_access()).unwrap().owner);
    let refused = rt.block_on(api.administration_status()).unwrap_err();
    assert!(a::refused(&refused), "{refused:?}");

    if rt.block_on(make_owner(&address, &username)).is_none() {
        eprintln!("skipped the owner part: no IRCORD_IT_ADMIN_TOKEN");
        return;
    }
    assert!(rt.block_on(api.administration_access()).unwrap().owner);

    // Overview and database.
    let status = rt.block_on(api.administration_status()).expect("status");
    assert_eq!(status.status, c::StatusStatus::Ok);
    assert!(status.overview.is_some_and(|o| o.accounts >= 1));
    let audit = rt
        .block_on(api.administration_audit(a::AUDIT_LIMIT))
        .expect("audit");
    assert!(audit.items.len() <= a::AUDIT_LIMIT);
    let database = rt
        .block_on(api.administration_database())
        .expect("database");
    assert!(database.available);
    assert!(database.allocated_bytes > 0);

    // You are listed as an owner, granted by the operator.
    let users = rt
        .block_on(api.administration_users(&username, 0, a::USERS_PAGE))
        .expect("users");
    let me = users
        .items
        .iter()
        .find(|u| u.username == username)
        .expect("listed");
    assert!(me.owner);
    assert_eq!(me.owner_source, Some(c::AdminUserOwnerSource::Operator));

    // Make another account an owner, found by its username, and remove it.
    let other = unique("adb");
    rt.block_on(sign_up(&address, &other, &random_id()));
    let found = rt
        .block_on(api.administration_identities(&other))
        .expect("identities");
    let user = found
        .items
        .iter()
        .find(|m| m.eligible)
        .and_then(|m| m.user.clone())
        .expect("an eligible match");
    assert_eq!(user.username, other);
    let granted = rt
        .block_on(api.grant_owner(&user.id, Some("live test".into())))
        .expect("grant");
    assert!(granted.owner);
    assert_eq!(granted.owner_source, Some(c::AdminUserOwnerSource::Owner));
    let revoked = rt
        .block_on(api.revoke_owner(&user.id, Some("done".into())))
        .expect("revoke");
    assert!(!revoked.owner);
    let audit = rt
        .block_on(api.administration_audit(a::AUDIT_LIMIT))
        .expect("audit");
    assert!(
        audit
            .items
            .iter()
            .any(|e| e.action == "owner-revoke" && e.target == other && !a::audit_failed(e)),
        "{:?}",
        audit.items
    );

    // Save the guide back unchanged: the revision moves on, and the old one
    // is refused as a conflict.
    let guide = rt
        .block_on(api.administration_channel_guide())
        .expect("guide");
    let draft = GuideDraft::from_saved(&guide);
    let saved = rt
        .block_on(api.save_channel_guide(guide.revision, draft.guide()))
        .expect("save the guide");
    assert!(saved.saved);
    assert!(saved.revision > guide.revision);
    assert!(!GuideDraft::from_saved(&saved).dirty(&saved));
    if guide.revision > 0 {
        let stale = rt
            .block_on(api.save_channel_guide(guide.revision, draft.guide()))
            .unwrap_err();
        assert!(a::conflict(&stale), "{stale:?}");
    }

    // Add and remove an emoji manager, keeping whether it is limited.
    let emoji = rt.block_on(api.emoji_management()).expect("emoji");
    let mut managers: Vec<String> = emoji.managers.iter().map(|m| m.id.clone()).collect();
    managers.push(user.id.clone());
    let added = rt
        .block_on(api.save_emoji_management(&c::EmojiManagementUpdate {
            restricted: emoji.restricted,
            managers: managers.clone(),
        }))
        .expect("add a manager");
    assert!(added.managers.iter().any(|m| m.id == user.id));
    managers.retain(|id| id != &user.id);
    let removed = rt
        .block_on(api.save_emoji_management(&c::EmojiManagementUpdate {
            restricted: emoji.restricted,
            managers,
        }))
        .expect("remove the manager");
    assert!(!removed.managers.iter().any(|m| m.id == user.id));
}

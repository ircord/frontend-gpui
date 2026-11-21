//! Live cases for package S4: sign up and onboarding.
//!
//! Start each case with `let Some((server, irc)) = crate::support::environment() else { return };`.
//!
//! The disposable backend has no associated Ergo server, so it provisions no
//! home network and no account is offered the profile setup; completing it
//! is still accepted and leaves the account without one. The setup dialog
//! itself is tested headless against the canned server.

use ircord_client::ServerAddress;
use ircord_client::api::{Api, ApiError};
use ircord_client::client::{random_id, runtime};
use ircord_client::contract::{ProfileUpdate, SignupCredentials};

use crate::support::{environment, unique, wait_for_backend};

#[test]
fn signs_up_through_the_client_sets_a_profile_and_completes_onboarding() {
    let Some((server, _irc)) = environment() else {
        return;
    };
    let rt = runtime().unwrap();
    let address = ServerAddress::parse(&server).unwrap();
    wait_for_backend(&rt, &address);
    let api = Api::new(address.clone());

    let policy = rt.block_on(api.registration()).expect("the policy");
    assert!(policy.registration_enabled);
    assert!(!policy.registration_key_required);

    let username = unique("gpuisignup");
    let check = rt
        .block_on(api.check_username(&username, None))
        .expect("the check");
    assert!(check.available, "{check:?}");
    let bad = rt
        .block_on(api.check_username("has space", None))
        .expect("the check");
    assert!(!bad.available && bad.reason.is_some(), "{bad:?}");

    let password = random_id();
    let (cookie, account) = rt
        .block_on(api.signup(&SignupCredentials {
            username: username.clone(),
            password: password.clone(),
            display_name: Some("New Person".into()),
            registration_key: None,
        }))
        .expect("sign up");
    assert!(cookie.starts_with("ircord_session="));
    assert_eq!(account.username, username);
    assert_eq!(account.onboarding_network_id, None, "no home network here");

    // The session works and the name is now taken.
    let me = rt
        .block_on(api.account())
        .expect("the account")
        .expect("signed in");
    assert_eq!(me.id, account.id);
    let again = rt
        .block_on(Api::new(address.clone()).check_username(&username, None))
        .expect("the check");
    assert!(!again.available, "{again:?}");
    let twice = rt
        .block_on(Api::new(address.clone()).signup(&SignupCredentials {
            username: username.clone(),
            password: random_id(),
            display_name: None,
            registration_key: None,
        }))
        .unwrap_err();
    assert!(
        matches!(
            twice,
            ApiError::Http { status: 409, .. } | ApiError::Http { status: 422, .. }
        ),
        "{twice:?}"
    );

    // The account's own profile, set from this client.
    let saved = rt
        .block_on(api.update_profile(&ProfileUpdate {
            display_name: Some("Renamed".into()),
            bio: Some("Hello from the desktop".into()),
            pronouns: Some("they/them".into()),
            name_color: Some("#3366cc".into()),
            ..Default::default()
        }))
        .expect("save the profile");
    assert_eq!(saved.display_name, "Renamed");
    assert_eq!(saved.bio, "Hello from the desktop");
    assert_eq!(saved.pronouns.as_deref(), Some("they/them"));
    assert_eq!(saved.name_color.as_deref(), Some("#3366cc"));

    rt.block_on(api.complete_onboarding())
        .expect("complete onboarding");
    let after = rt
        .block_on(api.account())
        .expect("the account")
        .expect("still signed in");
    assert_eq!(after.onboarding_network_id, None);

    // The same username and password sign in.
    let other = Api::new(address);
    rt.block_on(other.login(&username, &password))
        .expect("sign in with the new account");
}

//! Live cases for package S0: the account, password, devices, deleting the
//! account and the server's release notes.
//!
//! Start each case with `let Some((server, irc)) = crate::support::environment() else { return };`.

use ircord_client::api::{Api, ApiError};
use ircord_client::client::{random_id, runtime};
use ircord_client::{ServerAddress, contract::DeviceFormFactor};

use crate::support::{environment, setup_account, unique, wait_for_backend};

#[test]
fn changes_the_password_lists_devices_and_deletes_the_account() {
    let Some((server, irc)) = environment() else {
        return;
    };
    let rt = runtime().unwrap();
    let address = ServerAddress::parse(&server).unwrap();
    let username = unique("gpui");
    let password = random_id();
    wait_for_backend(&rt, &address);
    let channel = format!("#{}", unique("acct"));
    rt.block_on(setup_account(
        &address, &username, &password, &irc, &channel,
    ));

    // Sign in as this app: the device list shows it as this device.
    let api = Api::new(address.clone());
    rt.block_on(api.login(&username, &password))
        .expect("sign in");
    let account = rt
        .block_on(api.account())
        .expect("the account")
        .expect("signed in");
    assert_eq!(account.username, username);
    assert!(!account.created_at.is_empty() && !account.expires_at.is_empty());
    let devices = rt.block_on(api.devices()).expect("the devices");
    assert!(devices.len() >= 2, "the sign-up's session and this one");
    let here = &devices[0];
    assert!(here.current, "this device comes first");
    // The backend does not tell this app's form factor from its user agent.
    assert_ne!(here.form_factor, DeviceFormFactor::Phone);
    assert!(
        format!("{} {}", here.browser, here.os).contains("Linux")
            || format!("{} {}", here.browser, here.os).contains("IRCord"),
        "named from this app's user agent: {} / {}",
        here.browser,
        here.os
    );

    // A wrong current password is refused and changes nothing.
    let wrong = rt
        .block_on(api.change_password("not the password", "whatever-new-1"))
        .unwrap_err();
    assert!(
        matches!(wrong, ApiError::Http { status: 403, .. }),
        "{wrong:?}"
    );
    let new_password = format!("{}-new", random_id());
    rt.block_on(api.change_password(&password, &new_password))
        .expect("change the password");
    let old = Api::new(address.clone());
    assert!(
        rt.block_on(old.login(&username, &password)).is_err(),
        "the old password no longer signs in"
    );
    let other = Api::new(address.clone());
    rt.block_on(other.login(&username, &new_password))
        .expect("the new password signs in");

    // Sign the other session out from this one.
    let other_id = rt
        .block_on(api.devices())
        .unwrap()
        .into_iter()
        .find(|d| !d.current && d.signed_in_at >= here.signed_in_at)
        .expect("the second sign-in is listed")
        .id;
    assert!(rt.block_on(api.sign_out_device(&other_id)).unwrap());
    assert!(
        !rt.block_on(api.sign_out_device(&other_id)).unwrap(),
        "already signed out"
    );

    // The server's release notes.
    let notes = rt.block_on(api.changelog()).expect("the changelog");
    assert!(!notes.version.is_empty());

    // Deleting needs the right password, and ends the session.
    let refused = rt.block_on(api.delete_account("not the password"));
    assert!(
        matches!(refused, Err(ApiError::Http { status: 403, .. })),
        "{refused:?}"
    );
    rt.block_on(api.delete_account(&new_password))
        .expect("delete the account");
    let gone = Api::new(address.clone());
    assert!(
        rt.block_on(gone.login(&username, &new_password)).is_err(),
        "the account is gone"
    );
}

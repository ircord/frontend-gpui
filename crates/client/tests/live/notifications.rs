//! Live cases for package S3: change a level.
//!
//! Start each case with `let Some((server, irc)) = crate::support::environment() else { return };`.

use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::StreamExt;
use ircord_client::api::Api;
use ircord_client::client::{random_id, runtime, sign_in};
use ircord_client::contract::{
    DeviceNotificationsContent, DeviceNotificationsDelivery, Settings, SettingsChannelDefault,
    SettingsLevelsValue,
};
use ircord_client::credentials::{CredentialStore, MemoryStore};
use ircord_client::documents::NOTIFICATION_SETTINGS;
use ircord_client::store::notify_settings::{self as ns, DeviceChange, Level};
use ircord_client::store::{Connection, SaveState};
use ircord_client::{Client, ServerAddress};

use crate::support::{environment, setup_account, unique, wait_for_backend};

#[test]
fn sets_this_device_up_and_changes_levels_and_device_choices() {
    let Some((server, irc)) = environment() else {
        return;
    };
    let rt = runtime().unwrap();
    let address = ServerAddress::parse(&server).unwrap();
    let username = unique("gpui");
    let password = random_id();
    wait_for_backend(&rt, &address);
    let channel = format!("#{}", unique("ntf"));
    rt.block_on(setup_account(
        &address, &username, &password, &irc, &channel,
    ));
    let api = Api::new(address.clone());
    let store: Arc<dyn CredentialStore> = Arc::new(MemoryStore::default());
    rt.block_on(sign_in(&api, store.as_ref(), &username, &password))
        .expect("sign in");
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

    // A new sign-in is never set up for notifications; the client does it,
    // so the server alerts this device while it runs.
    ns::keep(&client);
    pump("this device set up", &|c| {
        c.read()
            .notify_settings
            .this_device()
            .is_some_and(|d| d.notifications.initialized)
    });
    let here = client
        .read()
        .notify_settings
        .this_device()
        .cloned()
        .unwrap();
    assert_eq!(
        here.notifications.delivery,
        DeviceNotificationsDelivery::Open
    );
    assert!(!here.notifications.explicit, "a set-up, not a choice");

    // This device's own choice.
    ns::save_device(
        client.handle(),
        &here.id,
        DeviceChange {
            content: Some(DeviceNotificationsContent::Mentions),
            ..Default::default()
        },
    );
    pump("the saved device choice", &|c| {
        let w = c.read();
        w.notify_settings.saving.is_empty()
            && w.notify_settings
                .this_device()
                .is_some_and(|d| d.notifications.content == DeviceNotificationsContent::Mentions)
    });
    let listed = rt.block_on(api.devices()).unwrap();
    assert_eq!(
        listed[0].notifications.content,
        DeviceNotificationsContent::Mentions
    );
    assert!(listed[0].notifications.explicit);

    // The account's rules: the channel default, a conversation's own level,
    // a highlight word and the device timing, saved through the document.
    client.watch_document(&NOTIFICATION_SETTINGS);
    pump("the settings", &|c| {
        c.read().documents.notification_settings.is_loaded()
    });
    let conversation = {
        let w = client.read();
        w.conversations
            .iter()
            .find(|c| c.name.eq_ignore_ascii_case(&channel))
            .or(w.conversations.first())
            .map(|c| c.id.clone())
    };
    let current = ns::sanitize(
        client
            .read()
            .documents
            .notification_settings
            .value()
            .unwrap(),
    );
    let mut next = ns::set_channel_default(&current, Level::All);
    if let Some(id) = &conversation {
        next = ns::set_level(&next, id, Some(Level::None));
    }
    next = ns::add_highlight(&next, "deploy").unwrap();
    next = ns::set_last_device(&next, 90);
    client.edit_document(&NOTIFICATION_SETTINGS, next);
    pump("the saved settings", &|c| {
        *c.read().documents.notification_settings.save_state() == SaveState::Saved
    });
    let stored: Settings = rt
        .block_on(api.get("/api/v1/notifications/settings"))
        .unwrap();
    assert_eq!(stored.channel_default, SettingsChannelDefault::All);
    assert_eq!(ns::highlight_words(&stored), ["deploy"]);
    assert_eq!(stored.last_device_delay, 90);
    assert!(stored.all_devices_delay >= 90);
    if let Some(id) = &conversation {
        assert_eq!(stored.levels.get(id), Some(&SettingsLevelsValue::None));
    }
}

//! Live cases for package CH1: create and join.
//!
//! Start each case with `let Some((server, irc)) = crate::support::environment() else { return };`.

use std::time::{Duration, Instant};

use futures::StreamExt;
use ircord_client::api::channels::{self as ch, Availability, CreateForm, Template};
use ircord_client::api::{Api, ApiError};
use ircord_client::client::{random_id, runtime};
use ircord_client::store::{Connection, server_log};
use ircord_client::{Client, ServerAddress};

use crate::support::{environment, setup_account, unique, wait_for_backend};

#[test]
fn a_channel_is_checked_created_listed_and_joined() {
    let Some((server, irc)) = environment() else {
        return;
    };
    let home = format!("#{}", unique("ch"));
    let rt = runtime().unwrap();
    let address = ServerAddress::parse(&server).unwrap();
    let username = unique("gpui");
    let password = random_id();
    wait_for_backend(&rt, &address);
    let cookie = rt.block_on(setup_account(&address, &username, &password, &irc, &home));
    // The sign-up's session, rather than a second sign-in: the backend
    // allows an address only so many of either a minute.
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
    pump("the home channel", &|c| {
        c.read()
            .conversations
            .iter()
            .any(|x| x.name.eq_ignore_ascii_case(&home[1..]) && x.joined)
    });
    let network = client.read().networks[0].clone();
    pump("the network's capabilities", &|c| {
        c.read().networks.first().is_some_and(ch::can_create)
    });

    // The guide is only on the server's own network.
    let guide = rt.block_on(api.channel_guide(&network.id)).expect("guide");
    assert!(!ch::guide_available(&guide));

    // The name check: the home channel is taken, a new name is free.
    let taken = rt
        .block_on(api.channel_availability(&network.id, &home))
        .expect("availability");
    assert_eq!(Availability::of(taken), Availability::Unavailable);
    let name = unique("chnew");
    let free = rt
        .block_on(api.channel_availability(&network.id, &format!("#{name}")))
        .expect("availability");
    // Ergo cannot always tell a free name from a hidden one: available or unknown.
    assert_ne!(Availability::of(free), Availability::Unavailable);

    // Create it as an announcements channel with a topic.
    let isupport = client.read().networks[0].isupport.clone();
    let mut form = CreateForm::new(&isupport);
    form.name = name.clone();
    form.topic = "Made by the GPUI client".into();
    form.apply_template(Template::Announcements);
    let body = form.body().expect("a name");
    // Created channels are registered unless turned off, and that needs a
    // services login this account does not have: the server says so.
    let refused = rt.block_on(api.create_channel(&network.id, &body));
    assert!(
        matches!(&refused, Err(ApiError::Http { status: 403, detail }) if !detail.is_empty()),
        "{refused:?}"
    );
    // Register the nick with NickServ, which logs the network in.
    rt.block_on(server_log::send_raw(
        client.handle(),
        &network.id,
        format!("PRIVMSG NickServ :REGISTER {}", random_id()),
    ))
    .unwrap()
    .expect("send REGISTER");
    pump("the services login", &|c| {
        c.read().networks[0]
            .account
            .as_deref()
            .is_some_and(|a| !a.is_empty())
    });
    let created = rt
        .block_on(api.create_channel(&network.id, &body))
        .expect("create");
    assert!(
        created.conversation.name.eq_ignore_ascii_case(&name),
        "{:?}",
        created.conversation
    );
    assert!(created.configured, "warnings: {:?}", created.warnings);
    client.change(|w| ch::remember(w, created.conversation.clone()));
    let id = created.conversation.id.clone();
    pump("the topic and moderated mode", &|c| {
        c.read()
            .conversation(&id)
            .is_some_and(|x| x.topic == "Made by the GPUI client" && x.modes.contains('m'))
    });

    // It shows in the channel list, and the name is taken now.
    let listed = rt
        .block_on(api.channel_list(&network.id, &name))
        .expect("list");
    assert!(
        listed
            .iter()
            .any(|l| l.name.eq_ignore_ascii_case(&format!("#{name}"))),
        "{listed:?}"
    );
    let now = rt
        .block_on(api.channel_availability(&network.id, &format!("#{name}")))
        .expect("availability");
    assert_eq!(Availability::of(now), Availability::Unavailable);

    // Joining a channel by name, as the join dialog does.
    let other = format!("#{}", unique("chj"));
    let joined = rt
        .block_on(api.join_channel(&network.id, &other))
        .expect("join");
    client.change(|w| ch::remember(w, joined.clone()));
    pump("the joined channel", &|c| {
        c.read().conversation(&joined.id).is_some_and(|x| x.joined)
    });
    assert_eq!(
        ch::existing(&client.read(), &network.id, &other).as_deref(),
        Some(joined.id.as_str())
    );
}

//! Shared set-up for the live cases: the environment `cargo xtask
//! integration` provides, a fresh account with an IRC network, and a plain
//! IRC user. Cases run in parallel against one backend and one IRC server,
//! so each picks its own account ([`unique`]), nicks and channels.

use std::time::{Duration, Instant};

use ircord_client::ServerAddress;
use ircord_client::api::Api;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

/// The backend URL and the IRC server (`host:port` as the backend sees it),
/// or `None` outside `cargo xtask integration`, where every case is skipped.
pub fn environment() -> Option<(String, String)> {
    match (
        std::env::var("IRCORD_IT_SERVER"),
        std::env::var("IRCORD_IT_IRC"),
    ) {
        (Ok(server), Ok(irc)) => Some((server, irc)),
        _ => {
            eprintln!("skipped: run `cargo xtask integration` to start a backend");
            None
        }
    }
}

/// Wait until the backend answers; it may still be starting.
pub fn wait_for_backend(rt: &tokio::runtime::Runtime, address: &ServerAddress) {
    let probe = Api::new(address.clone());
    let deadline = Instant::now() + Duration::from_secs(60);
    while let Err(e) = rt.block_on(probe.verify_server()) {
        assert!(Instant::now() < deadline, "the backend never answered: {e}");
        std::thread::sleep(Duration::from_millis(500));
    }
}

/// A name nobody else in this run uses: `prefix` and eight random hex digits.
pub fn unique(prefix: &str) -> String {
    format!("{prefix}{}", &ircord_client::client::random_id()[..8])
}

/// The IRC server as this test reaches it (it shares the backend's network).
pub fn irc_host_for_test(irc: &str) -> String {
    std::env::var("IRCORD_IT_IRC_FROM_TEST").unwrap_or_else(|_| irc.to_string())
}

/// Sign up `username` over plain HTTP and add the IRC network, joining
/// `channel`. Returns the session cookie pair.
pub async fn setup_account(
    address: &ServerAddress,
    username: &str,
    password: &str,
    irc: &str,
    channel: &str,
) -> String {
    let http = reqwest::Client::new();
    let origin = address.origin();
    let signup = http
        .post(address.join("/api/v1/auth/signup"))
        .header("Origin", &origin)
        .json(&serde_json::json!({"username": username, "password": password}))
        .send()
        .await
        .unwrap();
    assert_eq!(
        signup.status(),
        201,
        "signup: {}",
        signup.text().await.unwrap_or_default()
    );
    let cookie = signup
        .headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find(|v| v.starts_with("ircord_session="))
        .map(|v| v.split(';').next().unwrap().to_string())
        .expect("signup sets a session");
    let (host, port) = irc.rsplit_once(':').unwrap();
    let network = http
        .post(address.join("/api/v1/networks"))
        .header("Origin", &origin)
        .header("Cookie", &cookie)
        .json(&serde_json::json!({
            "name": "Test", "host": host, "port": port.parse::<u16>().unwrap(), "tls": false,
            "nick": username, "autoJoin": [channel]
        }))
        .send()
        .await
        .unwrap();
    let status = network.status();
    assert!(
        status.is_success(),
        "add network: {status} {}",
        network.text().await.unwrap_or_default()
    );
    cookie
}

/// A plain IRC user: connect as `nick`, join `channel`, return its lines and writer.
pub async fn irc_user(
    irc: &str,
    nick: &str,
    channel: &str,
) -> (
    tokio::io::Lines<BufReader<tokio::net::tcp::OwnedReadHalf>>,
    tokio::net::tcp::OwnedWriteHalf,
) {
    let stream = tokio::net::TcpStream::connect(irc)
        .await
        .expect("reach the IRC server");
    let (read, mut write) = stream.into_split();
    write
        .write_all(format!("NICK {nick}\r\nUSER {nick} 0 * :{nick}\r\n").as_bytes())
        .await
        .unwrap();
    let mut lines = BufReader::new(read).lines();
    // Wait for the welcome, answer pings, then join.
    let joined = tokio::time::timeout(Duration::from_secs(20), async {
        while let Ok(Some(line)) = lines.next_line().await {
            if line.starts_with("PING") {
                let pong = line.replacen("PING", "PONG", 1) + "\r\n";
                write.write_all(pong.as_bytes()).await.unwrap();
            }
            if line.split(' ').nth(1) == Some("001") {
                write
                    .write_all(format!("JOIN {channel}\r\n").as_bytes())
                    .await
                    .unwrap();
            }
            if line.contains(" 366 ") {
                return true;
            }
        }
        false
    })
    .await
    .unwrap_or(false);
    assert!(joined, "the IRC user could not join {channel}");
    (lines, write)
}

//! An in-process IRCord server that answers canned routes over real HTTP and
//! serves the event socket, so a [`crate::Client`] runs against it unchanged.
//!
//! - Routes are `(method, path)` pairs; a path segment `*` matches any one
//!   segment. A later [`CannedServer::route`] for the same pair replaces the
//!   earlier one. Anything unrouted answers 404 with a problem document.
//! - Every request is recorded; [`CannedServer::wait_for`] waits for one.
//! - The socket sends `session.ready`, waits for `session.resume`, sends
//!   `session.synced`, then every frame passed to [`CannedServer::push`].
//!   Frames the client sends are kept in [`CannedServer::client_frames`].
//!
//! The defaults make a whole session work: `GET /api/v1/auth/session`,
//! `POST /api/v1/auth/login` (sets a session cookie), `POST /api/v1/auth/logout`,
//! `GET /api/v1/bootstrap` (the snapshot from [`CannedServer::snapshot`]),
//! `POST /api/v1/activity` and `PATCH /api/v1/conversations/*`.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures::{SinkExt, StreamExt};
use ircord_contract::Snapshot;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::runtime::Runtime;
use tokio::sync::broadcast;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::handshake::derive_accept_key;
use tokio_tungstenite::tungstenite::protocol::Role;
use tokio_tungstenite::tungstenite::{self, Message as WsMessage};

use crate::address::ServerAddress;
use crate::api::{Api, SESSION_COOKIE};

/// The session cookie the canned server hands out and expects.
pub const CANNED_COOKIE: &str = "ircord_session=canned";

/// One request as the server received it.
#[derive(Debug, Clone, PartialEq)]
pub struct Recorded {
    pub method: String,
    /// The path without the query.
    pub path: String,
    pub query: Option<String>,
    /// Header names in lower case.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Recorded {
    /// The body as JSON (`null` when empty or not JSON).
    pub fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        let name = name.to_ascii_lowercase();
        self.headers
            .iter()
            .find(|(k, _)| *k == name)
            .map(|(_, v)| v.as_str())
    }

    /// One query parameter, decoded.
    pub fn query_param(&self, name: &str) -> Option<String> {
        let query = self.query.as_deref()?;
        url::form_urlencoded::parse(query.as_bytes())
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.into_owned())
    }
}

/// A canned answer.
#[derive(Debug, Clone, PartialEq)]
pub struct Reply {
    pub status: u16,
    pub body: Value,
    pub headers: Vec<(String, String)>,
    /// A raw body (a picture, say), sent instead of `body` when set.
    pub raw: Option<Vec<u8>>,
}

impl Reply {
    pub fn json(status: u16, body: Value) -> Self {
        Reply {
            status,
            body,
            headers: Vec::new(),
            raw: None,
        }
    }

    /// A raw body with its content type, such as a picture.
    pub fn bytes(status: u16, content_type: &str, body: Vec<u8>) -> Self {
        Reply {
            status,
            body: Value::Null,
            headers: vec![("Content-Type".to_string(), content_type.to_string())],
            raw: Some(body),
        }
    }

    /// `204 No Content`.
    pub fn empty() -> Self {
        Reply::json(204, Value::Null)
    }

    /// A problem document, as the backend sends for errors.
    pub fn problem(status: u16, detail: &str) -> Self {
        Reply::json(
            status,
            json!({"status": status, "title": reason(status), "detail": detail}),
        )
    }

    pub fn with_header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_string(), value.to_string()));
        self
    }
}

type Handler = Arc<dyn Fn(&Recorded) -> Reply + Send + Sync>;

struct Route {
    method: String,
    path: Vec<String>,
    handler: Handler,
}

#[derive(Default)]
struct State {
    routes: Vec<Route>,
    requests: Vec<Recorded>,
    client_frames: Vec<String>,
    cursor: i64,
    sockets_open: usize,
}

/// See the module docs.
pub struct CannedServer {
    address: ServerAddress,
    state: Arc<Mutex<State>>,
    frames: broadcast::Sender<String>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for CannedServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl CannedServer {
    /// Listen on a free local port.
    pub fn start(runtime: &Runtime) -> Self {
        let listener = runtime
            .block_on(TcpListener::bind("127.0.0.1:0"))
            .expect("bind a local port");
        let port = listener.local_addr().unwrap().port();
        let address = ServerAddress::parse(&format!("http://127.0.0.1:{port}")).unwrap();
        let state = Arc::new(Mutex::new(State::default()));
        let (frames, _) = broadcast::channel(1024);
        let task = runtime.spawn(accept(listener, state.clone(), frames.clone()));
        let server = CannedServer {
            address,
            state,
            frames,
            task,
        };
        server.defaults();
        server
    }

    pub fn address(&self) -> ServerAddress {
        self.address.clone()
    }

    /// An [`Api`] for this server, already signed in.
    pub fn api(&self) -> Api {
        let api = Api::new(self.address());
        api.set_cookie(Some(CANNED_COOKIE.to_string()));
        api
    }

    /// Answer `method path` with `status` and a JSON body.
    pub fn route(&self, method: &str, path: &str, status: u16, body: Value) {
        self.route_reply(method, path, Reply::json(status, body));
    }

    pub fn route_reply(&self, method: &str, path: &str, reply: Reply) {
        self.route_fn(method, path, move |_| reply.clone());
    }

    /// Answer `method path` by calling `f` with the request.
    pub fn route_fn(
        &self,
        method: &str,
        path: &str,
        f: impl Fn(&Recorded) -> Reply + Send + Sync + 'static,
    ) {
        let method = method.to_ascii_uppercase();
        let path = segments(path);
        let mut state = self.state.lock().unwrap();
        state
            .routes
            .retain(|r| !(r.method == method && r.path == path));
        state.routes.push(Route {
            method,
            path,
            handler: Arc::new(f),
        });
    }

    /// Serve `snapshot` from `GET /api/v1/bootstrap` and start the socket at its cursor.
    pub fn snapshot(&self, snapshot: &Snapshot) {
        self.state.lock().unwrap().cursor = snapshot.cursor;
        self.route(
            "GET",
            "/api/v1/bootstrap",
            200,
            serde_json::to_value(snapshot).unwrap(),
        );
    }

    /// Send a frame to every open event socket.
    pub fn push(&self, frame: impl Into<String>) {
        let _ = self.frames.send(frame.into());
    }

    /// Event sockets currently past `session.resume`.
    pub fn sockets_open(&self) -> usize {
        self.state.lock().unwrap().sockets_open
    }

    /// Every request so far, oldest first.
    pub fn requests(&self) -> Vec<Recorded> {
        self.state.lock().unwrap().requests.clone()
    }

    /// Requests to `method path` (with `*` segments) so far.
    pub fn requests_to(&self, method: &str, path: &str) -> Vec<Recorded> {
        let method = method.to_ascii_uppercase();
        let pattern = segments(path);
        self.requests()
            .into_iter()
            .filter(|r| r.method == method && matches(&pattern, &segments(&r.path)))
            .collect()
    }

    /// Wait for the `n`th (from 1) request to `method path`. Panics after `timeout`.
    pub fn wait_for_nth(&self, method: &str, path: &str, n: usize, timeout: Duration) -> Recorded {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(r) = self.requests_to(method, path).into_iter().nth(n - 1) {
                return r;
            }
            assert!(
                Instant::now() < deadline,
                "no request #{n} to {method} {path}; saw {:?}",
                self.requests()
                    .iter()
                    .map(|r| format!("{} {}", r.method, r.path))
                    .collect::<Vec<_>>()
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Wait for the first request to `method path`. Panics after `timeout`.
    pub fn wait_for(&self, method: &str, path: &str, timeout: Duration) -> Recorded {
        self.wait_for_nth(method, path, 1, timeout)
    }

    /// Frames the client sent on the event socket, `session.resume` included.
    pub fn client_frames(&self) -> Vec<String> {
        self.state.lock().unwrap().client_frames.clone()
    }

    fn defaults(&self) {
        self.route("GET", "/api/v1/auth/session", 200, json!({"account": null}));
        self.route_reply(
            "POST",
            "/api/v1/auth/login",
            Reply::json(200, json!({"account": {"username": "me"}})).with_header(
                "Set-Cookie",
                &format!("{CANNED_COOKIE}; Path=/; HttpOnly; SameSite=Lax"),
            ),
        );
        self.route_reply("POST", "/api/v1/auth/logout", Reply::empty());
        self.route_reply("POST", "/api/v1/activity", Reply::empty());
        self.route_reply("PATCH", "/api/v1/conversations/*", Reply::empty());
        self.snapshot(&super::SnapshotBuilder::new().build());
    }
}

fn segments(path: &str) -> Vec<String> {
    path.split('/')
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

fn matches(pattern: &[String], path: &[String]) -> bool {
    pattern.len() == path.len() && pattern.iter().zip(path).all(|(p, s)| p == "*" || p == s)
}

fn reason(status: u16) -> &'static str {
    http::StatusCode::from_u16(status)
        .ok()
        .and_then(|s| s.canonical_reason())
        .unwrap_or("Unknown")
}

async fn accept(
    listener: TcpListener,
    state: Arc<Mutex<State>>,
    frames: broadcast::Sender<String>,
) {
    while let Ok((stream, _)) = listener.accept().await {
        tokio::spawn(serve(stream, state.clone(), frames.clone()));
    }
}

async fn serve(mut stream: TcpStream, state: Arc<Mutex<State>>, frames: broadcast::Sender<String>) {
    let Some(request) = read_request(&mut stream).await else {
        return;
    };
    let upgrade = request
        .header("upgrade")
        .is_some_and(|v| v.eq_ignore_ascii_case("websocket"));
    if upgrade && request.path == "/api/v1/events" {
        events(stream, request, state, frames).await;
        return;
    }
    let reply = {
        let mut s = state.lock().unwrap();
        s.requests.push(request.clone());
        let path = segments(&request.path);
        s.routes
            .iter()
            .rev()
            .find(|r| r.method == request.method && matches(&r.path, &path))
            .map(|r| r.handler.clone())
    };
    let reply = match reply {
        // A handler may block (a reply held back, a slow server): off the
        // runtime's workers, or it strands the client's own tasks and timers
        // queued behind it.
        Some(handler) => {
            let asked = request.clone();
            tokio::task::spawn_blocking(move || handler(&asked))
                .await
                .unwrap_or_else(|_| Reply::problem(500, "the canned handler panicked"))
        }
        None => Reply::problem(
            404,
            &format!("no canned route for {} {}", request.method, request.path),
        ),
    };
    let body = match &reply.raw {
        Some(raw) => raw.clone(),
        None if reply.body.is_null() => Vec::new(),
        None => serde_json::to_vec(&reply.body).unwrap(),
    };
    let mut head = format!(
        "HTTP/1.1 {} {}\r\nContent-Length: {}\r\nConnection: close\r\n",
        reply.status,
        reason(reply.status),
        body.len()
    );
    if !body.is_empty() && reply.raw.is_none() {
        let kind = if reply.status >= 400 {
            "application/problem+json"
        } else {
            "application/json"
        };
        head.push_str(&format!("Content-Type: {kind}\r\n"));
    }
    for (k, v) in &reply.headers {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    head.push_str("\r\n");
    let _ = stream.write_all(head.as_bytes()).await;
    let _ = stream.write_all(&body).await;
    let _ = stream.shutdown().await;
}

async fn read_request(stream: &mut TcpStream) -> Option<Recorded> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let end = loop {
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i;
        }
        let n = stream.read(&mut chunk).await.ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
    };
    let head = String::from_utf8_lossy(&buf[..end]).to_string();
    let mut lines = head.split("\r\n");
    let mut first = lines.next()?.split(' ');
    let method = first.next()?.to_string();
    let target = first.next()?.to_string();
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string()))
        .collect();
    let length: usize = headers
        .iter()
        .find(|(k, _)| k == "content-length")
        .and_then(|(_, v)| v.parse().ok())
        .unwrap_or(0);
    let mut body = buf[end + 4..].to_vec();
    while body.len() < length {
        let n = stream.read(&mut chunk).await.ok()?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    let (path, query) = match target.split_once('?') {
        Some((p, q)) => (p.to_string(), Some(q.to_string())),
        None => (target, None),
    };
    Some(Recorded {
        method,
        path,
        query,
        headers,
        body,
    })
}

async fn events(
    mut stream: TcpStream,
    request: Recorded,
    state: Arc<Mutex<State>>,
    frames: broadcast::Sender<String>,
) {
    state.lock().unwrap().requests.push(request.clone());
    if request.header("cookie") != Some(CANNED_COOKIE)
        && !request
            .header("cookie")
            .is_some_and(|c| c.contains(&format!("{SESSION_COOKIE}=")))
    {
        let _ = stream
            .write_all(
                b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            )
            .await;
        return;
    }
    let Some(key) = request.header("sec-websocket-key") else {
        return;
    };
    let head = format!(
        "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {}\r\n\r\n",
        derive_accept_key(key.as_bytes())
    );
    if stream.write_all(head.as_bytes()).await.is_err() {
        return;
    }
    let mut socket = WebSocketStream::from_raw_socket(stream, Role::Server, None).await;
    let mut pushed = frames.subscribe();
    let cursor = state.lock().unwrap().cursor;
    let send = |text: String| WsMessage::Text(text.into());
    if socket
        .send(send(
            json!({"v": 1, "type": "session.ready", "cursor": cursor}).to_string(),
        ))
        .await
        .is_err()
    {
        return;
    }
    // Wait for the client to resume before replaying anything.
    loop {
        match socket.next().await {
            Some(Ok(WsMessage::Text(text))) => {
                let text = text.to_string();
                let resumed = text.contains("session.resume");
                state.lock().unwrap().client_frames.push(text);
                if resumed {
                    break;
                }
            }
            Some(Ok(_)) => continue,
            _ => return,
        }
    }
    if socket
        .send(send(
            json!({"v": 1, "type": "session.synced", "cursor": cursor}).to_string(),
        ))
        .await
        .is_err()
    {
        return;
    }
    state.lock().unwrap().sockets_open += 1;
    loop {
        tokio::select! {
            frame = pushed.recv() => match frame {
                Ok(text) => if socket.send(send(text)).await.is_err() { break },
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
            },
            incoming = socket.next() => match incoming {
                Some(Ok(WsMessage::Text(text))) => {
                    state.lock().unwrap().client_frames.push(text.to_string());
                }
                Some(Ok(WsMessage::Close(_))) | None | Some(Err(_)) => break,
                Some(Ok(_)) => {}
            },
        }
    }
    state.lock().unwrap().sockets_open -= 1;
    let _: Result<(), tungstenite::Error> = socket.close(None).await;
}

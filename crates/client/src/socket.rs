//! The real transport: HTTP bootstrap and a WebSocket over TCP, with TLS from
//! rustls and the WebPKI roots for `wss://`.

use std::sync::Arc;
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use http::header;
use ircord_contract::Snapshot;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::{self, Message, client::IntoClientRequest};

use crate::api::{Api, ApiError, user_agent};
use crate::events::{Conn, Transport};

trait Io: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Io for T {}

pub struct WsTransport {
    pub api: Api,
}

pub struct WsConn(WebSocketStream<Box<dyn Io>>);

impl Transport for WsTransport {
    type Conn = WsConn;

    async fn bootstrap(&self) -> Result<Snapshot, ApiError> {
        self.api.bootstrap().await
    }

    async fn connect(&self) -> Result<WsConn, ApiError> {
        tokio::time::timeout(Duration::from_secs(15), open(&self.api))
            .await
            .map_err(|_| ApiError::Network("the event connection timed out".into()))?
            .map(WsConn)
    }
}

impl Conn for WsConn {
    async fn recv(&mut self) -> Option<Result<String, String>> {
        loop {
            match self.0.next().await? {
                Ok(Message::Text(text)) => return Some(Ok(text.to_string())),
                Ok(Message::Close(_)) => return None,
                Ok(_) => continue,
                Err(e) => return Some(Err(e.to_string())),
            }
        }
    }

    async fn send(&mut self, text: String) -> Result<(), String> {
        self.0
            .send(Message::Text(text.into()))
            .await
            .map_err(|e| e.to_string())
    }
}

pub fn tls_config() -> Arc<rustls::ClientConfig> {
    let roots = rustls::RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    };
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .expect("ring supports the default protocol versions")
    .with_root_certificates(roots)
    .with_no_client_auth();
    Arc::new(config)
}

async fn open(api: &Api) -> Result<WebSocketStream<Box<dyn Io>>, ApiError> {
    let net = |e: std::io::Error| ApiError::Network(e.to_string());
    let url = api.server().events_url();
    let host = url.host_str().unwrap_or_default().to_string();
    let port = url.port_or_known_default().unwrap_or(443);
    let tcp = TcpStream::connect((host.as_str(), port))
        .await
        .map_err(net)?;
    tcp.set_nodelay(true).ok();
    let io: Box<dyn Io> = if url.scheme() == "wss" {
        let name = rustls::pki_types::ServerName::try_from(host.clone())
            .map_err(|e| ApiError::Network(e.to_string()))?;
        let tls = tokio_rustls::TlsConnector::from(tls_config())
            .connect(name, tcp)
            .await
            .map_err(net)?;
        Box::new(tls)
    } else {
        Box::new(tcp)
    };
    let mut request = url
        .as_str()
        .into_client_request()
        .map_err(|e| ApiError::Network(e.to_string()))?;
    let headers = request.headers_mut();
    let value =
        |s: &str| http::HeaderValue::from_str(s).map_err(|e| ApiError::Network(e.to_string()));
    headers.insert(header::ORIGIN, value(&api.server().origin())?);
    headers.insert(header::USER_AGENT, value(&user_agent())?);
    if let Some(cookie) = api.cookie() {
        headers.insert(header::COOKIE, value(&cookie)?);
    }
    match tokio_tungstenite::client_async(request, io).await {
        Ok((socket, _)) => Ok(socket),
        Err(tungstenite::Error::Http(response)) if response.status() == 401 => {
            Err(ApiError::Unauthorized)
        }
        Err(tungstenite::Error::Http(response)) => Err(ApiError::Http {
            status: response.status().as_u16(),
            detail: format!("The event connection was refused ({}).", response.status()),
        }),
        Err(e) => Err(ApiError::Network(e.to_string())),
    }
}

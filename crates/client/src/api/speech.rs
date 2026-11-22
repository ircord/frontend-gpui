//! The OpenAI-compatible speech service configured by the user. This
//! transport is separate from the IRCord session and sends only its own key.

use std::net::IpAddr;
use std::sync::LazyLock;
use std::time::Duration;

use ircord_contract::SpeechService;
use reqwest::{Client, header, redirect};
use url::Url;

const MAX_AUDIO: usize = 4 << 20;

static CLIENT: LazyLock<Client> = LazyLock::new(|| {
    Client::builder()
        .redirect(redirect::Policy::none())
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .build()
        .expect("valid speech client configuration")
});

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    Address,
    ModelOrVoice,
    Key,
    Input,
    Network,
    Http(u16),
    TooLarge,
}

/// A validated endpoint with the configured model and voice.
#[derive(Clone, PartialEq, Eq)]
pub struct Endpoint {
    url: Url,
    model: String,
    voice: String,
}

impl Endpoint {
    pub fn new(service: &SpeechService) -> Result<Self, Error> {
        let base = service.base_url.as_deref().ok_or(Error::Address)?;
        if base.len() > 500
            || base.chars().any(|c| {
                c.is_whitespace() || matches!(c, ';' | ',' | '\'' | '"' | '\\' | '*' | '?' | '#')
            })
        {
            return Err(Error::Address);
        }
        let mut url = Url::parse(base).map_err(|_| Error::Address)?;
        let host = url.host_str().ok_or(Error::Address)?;
        let loopback = host == "localhost"
            || host.ends_with(".localhost")
            || host
                .trim_matches(['[', ']'])
                .parse::<IpAddr>()
                .is_ok_and(|ip| ip.is_loopback());
        if !(url.scheme() == "https" || (url.scheme() == "http" && loopback))
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return Err(Error::Address);
        }
        url.path_segments_mut()
            .map_err(|_| Error::Address)?
            .pop_if_empty()
            .extend(["audio", "speech"]);
        let name = |value: Option<&str>| {
            value
                .map(str::trim)
                .filter(|s| {
                    !s.is_empty()
                        && s.encode_utf16().count() <= 100
                        && !s.chars().any(char::is_control)
                })
                .map(str::to_string)
                .ok_or(Error::ModelOrVoice)
        };
        Ok(Self {
            url,
            model: name(service.model.as_deref())?,
            voice: name(service.voice.as_deref())?,
        })
    }

    pub fn origin(&self) -> String {
        self.url.origin().ascii_serialization()
    }

    pub async fn synthesize(&self, text: &str, key: &str) -> Result<Vec<u8>, Error> {
        if text.is_empty() || text.encode_utf16().count() > 1_000 {
            return Err(Error::Input);
        }
        if !valid_key(key) {
            return Err(Error::Key);
        }
        let mut request = CLIENT
            .post(self.url.clone())
            .header(header::ACCEPT, "audio/mpeg")
            .json(&serde_json::json!({
                "model": self.model,
                "voice": self.voice,
                "input": text,
                "response_format": "mp3",
            }));
        if !key.is_empty() {
            request = request.bearer_auth(key);
        }
        let mut response = request.send().await.map_err(|_| Error::Network)?;
        if !response.status().is_success() {
            return Err(Error::Http(response.status().as_u16()));
        }
        if response
            .content_length()
            .is_some_and(|n| n > MAX_AUDIO as u64)
        {
            return Err(Error::TooLarge);
        }
        let mut audio = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| Error::Network)? {
            if audio.len() + chunk.len() > MAX_AUDIO {
                return Err(Error::TooLarge);
            }
            audio.extend_from_slice(&chunk);
        }
        Ok(audio)
    }
}

pub fn valid_key(key: &str) -> bool {
    key.len() <= 500 && key.bytes().all(|b| b.is_ascii_graphic())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{CannedServer, Reply};

    #[test]
    fn checks_the_service_and_keeps_its_path() {
        let mut service = SpeechService {
            base_url: Some("http://[::1]:8090/v1/".into()),
            model: Some("model".into()),
            voice: Some("voice".into()),
        };
        assert_eq!(
            Endpoint::new(&service).unwrap().url.as_str(),
            "http://[::1]:8090/v1/audio/speech"
        );
        for address in [
            "http://speech.example/v1",
            "https://user:secret@speech.example/v1",
            "https://speech.example/v1?key=secret",
            "https://speech.example/v1#fragment",
        ] {
            service.base_url = Some(address.into());
            assert!(matches!(Endpoint::new(&service), Err(Error::Address)));
        }
    }

    #[test]
    fn sends_only_the_speech_key_and_stops_at_redirects() {
        let runtime = crate::client::runtime().unwrap();
        let server = CannedServer::start(&runtime);
        let mut redirect = Reply::empty();
        redirect.status = 307;
        redirect.headers.push(("Location".into(), "/other".into()));
        server.route_reply("POST", "/v1/audio/speech", redirect);
        let endpoint = Endpoint::new(&SpeechService {
            base_url: Some(format!("{}/v1", server.address().origin())),
            model: Some("model".into()),
            voice: Some("voice".into()),
        })
        .unwrap();
        let result = runtime.block_on(endpoint.synthesize("Example", "speech-key"));
        assert_eq!(result, Err(Error::Http(307)));
        let requests = server.requests_to("POST", "/v1/audio/speech");
        assert_eq!(requests.len(), 1);
        assert_eq!(
            requests[0].header("authorization"),
            Some("Bearer speech-key")
        );
        assert_eq!(requests[0].header("cookie"), None);
        assert!(server.requests_to("POST", "/other").is_empty());
        assert_eq!(requests[0].json()["input"], "Example");
    }
}

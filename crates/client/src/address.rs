//! Server addresses: the same rules as the Tauri app (`ircord-desktop`'s
//! `server.rs`). HTTPS anywhere, or plain HTTP to this computer only.

use url::Url;

/// A validated server origin such as `https://chat.ircord.com`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ServerAddress(Url);

impl ServerAddress {
    pub fn parse(address: &str) -> Result<Self, &'static str> {
        let address = address.trim();
        if address.is_empty() {
            return Err("Enter the address of your IRCord server.");
        }
        let mut url = Url::parse(address).map_err(|_| "Enter a complete server address.")?;
        if !url.username().is_empty()
            || url.password().is_some()
            || url.path() != "/"
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err("Enter the server address without a path or sign-in details.");
        }
        // Both spellings of the local backend share one session.
        if url.scheme() == "http" && url.host_str() == Some("localhost") {
            url.set_host(Some("127.0.0.1"))
                .map_err(|_| "Enter a complete server address.")?;
        }
        if !(url.scheme() == "https"
            || (url.scheme() == "http" && url.host_str() == Some("127.0.0.1")))
        {
            return Err("Use HTTPS, or use HTTP for a local server.");
        }
        Ok(Self(url))
    }

    /// `https://host[:port]`, the form the server checks the Origin header against.
    pub fn origin(&self) -> String {
        self.0.origin().ascii_serialization()
    }

    pub fn url(&self) -> &Url {
        &self.0
    }

    pub fn join(&self, path: &str) -> Url {
        self.0.join(path).expect("paths are static and valid")
    }

    /// The event socket URL: `wss://host/api/v1/events?sync=1`.
    pub fn events_url(&self) -> Url {
        let mut url = self.join("/api/v1/events?sync=1");
        let scheme = if self.0.scheme() == "https" {
            "wss"
        } else {
            "ws"
        };
        url.set_scheme(scheme)
            .expect("ws and wss are valid schemes");
        url
    }
}

impl std::fmt::Display for ServerAddress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.origin())
    }
}

#[cfg(test)]
mod tests {
    use super::ServerAddress;

    #[test]
    fn accepts_a_public_or_local_ircord_origin() {
        for address in [
            "https://chat.ircord.com",
            "https://myamazingsetup.com:8443/",
            "http://127.0.0.1:8080",
            "  https://chat.ircord.com  ",
        ] {
            assert!(ServerAddress::parse(address).is_ok(), "{address}");
        }
    }

    #[test]
    fn localhost_uses_the_same_origin_as_the_loopback_address() {
        assert_eq!(
            ServerAddress::parse("http://localhost:8080").unwrap(),
            ServerAddress::parse("http://127.0.0.1:8080").unwrap()
        );
    }

    #[test]
    fn requires_an_origin_with_safe_transport() {
        for address in [
            "",
            "chat.ircord.com",
            "http://chat.ircord.com",
            "http://localhost.example.com:8080",
            "file:///",
            "https://name:password@example.com",
            "https://example.com/chat",
            "https://example.com?access=key",
            "https://example.com/#here",
        ] {
            assert!(ServerAddress::parse(address).is_err(), "{address}");
        }
    }

    #[test]
    fn origin_and_socket_urls() {
        let a = ServerAddress::parse("https://chat.ircord.com/").unwrap();
        assert_eq!(a.origin(), "https://chat.ircord.com");
        assert_eq!(
            a.events_url().as_str(),
            "wss://chat.ircord.com/api/v1/events?sync=1"
        );
        let local = ServerAddress::parse("http://127.0.0.1:8080").unwrap();
        assert_eq!(
            local.events_url().as_str(),
            "ws://127.0.0.1:8080/api/v1/events?sync=1"
        );
    }
}

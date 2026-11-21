//! Networks: `/networks`, `/networks/{id}`, `/networks/check`, `/networks/presets`, `/networks/{id}/connection*`, `/networks/{id}/server-log`, `/networks/{id}/commands`, `/networks/{id}/ctcp`, `/networks/{id}/self`.
//!
//! Owned by package NW1, SV1. Add calls in this `impl Api` block with the
//! helpers in `api/mod.rs` and the types in `ircord_contract`.
//!
//! NW1 adds, edits, removes, connects and disconnects networks, checks a
//! server address, reads the presets and changes your nick and real name on
//! a network. [`Form`] is the add and edit form without its window, the way
//! the web client's `useNetworkForm.ts` keeps it, and [`saved`] and
//! [`removed`] put the server's answers into the workspace.

use std::net::{Ipv4Addr, Ipv6Addr};

use ircord_contract as c;

#[allow(unused_imports)]
use super::{Api, ApiError, Method, segment};
use crate::commands::NickError;
use crate::commands::run::{nickname_error, nickname_limit};
use crate::store::{Changes, Workspace};

impl Api {
    /// The server's suggested networks; `configured` is false when the
    /// server has none of its own and the app's [`default_presets`] apply.
    pub async fn network_presets(&self) -> Result<c::NetworkPresetPage, ApiError> {
        self.get("/api/v1/networks/presets").await
    }

    /// Whether the server can reach `host:port`, with a sentence saying why not.
    pub async fn check_network(
        &self,
        host: &str,
        port: u16,
        tls: bool,
    ) -> Result<c::NetworkCheck, ApiError> {
        let body = c::NetworkCheckInputBody {
            host: host.to_string(),
            port: port.into(),
            tls,
        };
        self.call(Method::POST, "/api/v1/networks/check", &body)
            .await
    }

    pub async fn add_network(&self, config: &c::NetworkConfig) -> Result<c::Network, ApiError> {
        self.call(Method::POST, "/api/v1/networks", config).await
    }

    pub async fn update_network(
        &self,
        id: &str,
        update: &c::NetworkUpdate,
    ) -> Result<c::Network, ApiError> {
        let path = format!("/api/v1/networks/{}", segment(id));
        self.call(Method::PATCH, &path, update).await
    }

    /// Connect (`true`) or disconnect; disconnecting also cancels a
    /// connection attempt.
    pub async fn connect_network(&self, id: &str, connect: bool) -> Result<c::Network, ApiError> {
        let update = c::NetworkUpdate {
            status: Some(if connect {
                c::NetworkUpdateStatus::Connected
            } else {
                c::NetworkUpdateStatus::Disconnected
            }),
            ..Default::default()
        };
        self.update_network(id, &update).await
    }

    /// Remove a network with its conversations and history.
    pub async fn remove_network(&self, id: &str) -> Result<(), ApiError> {
        self.delete(&format!("/api/v1/networks/{}", segment(id)))
            .await
    }

    /// Change your nick or real name on a connected network.
    pub async fn update_self(
        &self,
        id: &str,
        update: &c::SelfUpdate,
    ) -> Result<c::Network, ApiError> {
        let path = format!("/api/v1/networks/{}/self", segment(id));
        self.call(Method::PATCH, &path, update).await
    }
}

/// Put a network the server answered with into the workspace.
pub fn saved(w: &mut Workspace, network: c::Network) -> Changes {
    match w.networks.iter_mut().find(|n| n.id == network.id) {
        Some(existing) => *existing = network,
        None => w.networks.push(network),
    }
    Changes {
        sidebar: true,
        ..Default::default()
    }
}

/// Take a removed network and its conversations out of the workspace.
pub fn removed(w: &mut Workspace, id: &str) -> Changes {
    w.networks.retain(|n| n.id != id);
    let gone: Vec<String> = w
        .conversations
        .iter()
        .filter(|c| c.network_id == id)
        .map(|c| c.id.clone())
        .collect();
    w.conversations.retain(|c| c.network_id != id);
    for conversation in &gone {
        w.timelines.remove(conversation);
    }
    Changes {
        sidebar: true,
        timelines: gone.into_iter().collect(),
        ..Default::default()
    }
}

/// The networks the web client suggests when the server names none.
pub fn default_presets() -> Vec<c::NetworkPreset> {
    use c::NetworkPresetColor as Color;
    [
        ("Libera.Chat", "irc.libera.chat", Color::Mint, "L"),
        ("OFTC", "irc.oftc.net", Color::Peach, "O"),
        ("Rizon", "irc.rizon.net", Color::Blue, "R"),
        ("EFnet", "irc.efnet.org", Color::Yellow, "E"),
    ]
    .into_iter()
    .map(|(name, host, color, initial)| c::NetworkPreset {
        auth_method: c::NetworkPresetAuthMethod::None,
        auto_join: Vec::new(),
        color,
        host: host.into(),
        initial: initial.into(),
        name: name.into(),
        password_provided: false,
        port: 6697,
        tls: true,
    })
    .collect()
}

/// The presets to offer from the server's answer: its own when it has
/// them, else [`default_presets`].
pub fn presets(page: Option<c::NetworkPresetPage>) -> Vec<c::NetworkPreset> {
    match page {
        Some(page) if page.configured => page.items,
        _ => default_presets(),
    }
}

/// A server name or address as the backend accepts it: lower case, no
/// trailing dot, IPv6 without brackets and IPv4-mapped IPv6 as IPv4. `None`
/// for anything else (a URL, spaces, credentials).
pub fn normalize_host(input: &str) -> Option<String> {
    if input.is_empty()
        || input != input.trim()
        || input
            .chars()
            .any(|c| c.is_whitespace() || "/\\?#@".contains(c) || c.is_control())
    {
        return None;
    }
    if input.contains(':') || input.starts_with('[') || input.ends_with(']') {
        let literal = input
            .strip_prefix('[')
            .and_then(|s| s.strip_suffix(']'))
            .unwrap_or(input);
        if !literal.contains(':') || literal.contains(['[', ']', '%']) {
            return None;
        }
        let address: Ipv6Addr = literal.parse().ok()?;
        return Some(match address.to_ipv4_mapped() {
            Some(v4) => v4.to_string(),
            None => address.to_string(),
        });
    }
    let host = input.to_lowercase();
    let host = host.strip_suffix('.').unwrap_or(&host);
    if host.is_empty()
        || host.len() > 253
        || !host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
    {
        return None;
    }
    if host
        .split('.')
        .any(|l| l.is_empty() || l.len() > 63 || l.starts_with('-') || l.ends_with('-'))
    {
        return None;
    }
    if host.contains('.') && host.chars().all(|c| c.is_ascii_digit() || c == '.') {
        host.parse::<Ipv4Addr>().ok()?;
        if host.split('.').any(|p| p.len() > 1 && p.starts_with('0')) {
            return None;
        }
    }
    Some(host.to_string())
}

/// The non-empty, trimmed lines of a text box.
pub fn lines(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}

/// Channels to join on connect, one per line, `#` added where missing
/// (`rust` → `#rust`; a key after a space stays).
pub fn auto_join(text: &str) -> Vec<String> {
    lines(text)
        .into_iter()
        .map(|l| {
            if l.starts_with(['#', '&']) {
                l
            } else {
                format!("#{l}")
            }
        })
        .collect()
}

/// How you sign in to a network. SASL EXTERNAL needs a client certificate,
/// which neither client can set up, so the form offers the other four.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Login {
    None,
    Sasl,
    NickServ,
    ServerPassword,
}

impl Login {
    pub const ALL: [Login; 4] = [
        Login::None,
        Login::Sasl,
        Login::NickServ,
        Login::ServerPassword,
    ];

    pub fn method(self) -> c::AuthInputMethod {
        match self {
            Login::None => c::AuthInputMethod::None,
            Login::Sasl => c::AuthInputMethod::SaslMinusPlain,
            Login::NickServ => c::AuthInputMethod::Nickserv,
            Login::ServerPassword => c::AuthInputMethod::ServerMinusPassword,
        }
    }

    pub fn of(method: c::NetworkAuthMethod) -> Login {
        match method {
            c::NetworkAuthMethod::SaslMinusPlain => Login::Sasl,
            c::NetworkAuthMethod::Nickserv => Login::NickServ,
            c::NetworkAuthMethod::ServerMinusPassword => Login::ServerPassword,
            _ => Login::None,
        }
    }

    fn of_preset(method: c::NetworkPresetAuthMethod) -> Login {
        match method {
            c::NetworkPresetAuthMethod::SaslMinusPlain => Login::Sasl,
            c::NetworkPresetAuthMethod::Nickserv => Login::NickServ,
            c::NetworkPresetAuthMethod::ServerMinusPassword => Login::ServerPassword,
            _ => Login::None,
        }
    }

    /// The IRC wording, as the network list shows it.
    pub fn wire(self) -> &'static str {
        match self {
            Login::None => "none",
            Login::Sasl => "sasl-plain",
            Login::NickServ => "nickserv",
            Login::ServerPassword => "server-password",
        }
    }
}

/// The add and edit form's values, as typed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Form {
    pub name: String,
    pub host: String,
    /// As typed, so a half-typed port can be shown and refused.
    pub port: String,
    pub tls: bool,
    pub nick: String,
    pub alt_nick: String,
    pub username: String,
    pub realname: String,
    pub login: Login,
    /// The SASL account.
    pub account: String,
    /// Empty keeps a saved password unless [`Form::clear_password`].
    pub password: String,
    pub clear_password: bool,
    pub auto_join: String,
    pub perform: String,
}

/// What is wrong with a form, field by field.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Problems {
    pub name: bool,
    /// The address is not a host name or IP address.
    pub host: bool,
    pub port: bool,
    pub nick: Option<NickError>,
    pub alt_nick: Option<NickError>,
    pub account: bool,
    pub password: bool,
}

impl Problems {
    pub fn any(&self) -> bool {
        *self != Problems::default()
    }
}

impl Form {
    /// A new network: TLS on 6697, your account's nick.
    pub fn new(nick: &str) -> Self {
        Self {
            name: String::new(),
            host: String::new(),
            port: "6697".into(),
            tls: true,
            nick: nick.into(),
            alt_nick: String::new(),
            username: String::new(),
            realname: String::new(),
            login: Login::None,
            account: String::new(),
            password: String::new(),
            clear_password: false,
            auto_join: String::new(),
            perform: String::new(),
        }
    }

    /// An existing network, to edit. Passwords never come back from the server.
    pub fn of(n: &c::Network) -> Self {
        let login = Login::of(n.auth.method);
        Self {
            name: n.name.clone(),
            host: n.host.clone(),
            port: n.port.to_string(),
            tls: n.tls,
            nick: n.nick.clone(),
            alt_nick: n.alt_nick.clone().unwrap_or_default(),
            username: n.username.clone().unwrap_or_default(),
            realname: n.realname.clone().unwrap_or_default(),
            login,
            account: if login == Login::Sasl {
                n.auth.account.clone().unwrap_or_default()
            } else {
                String::new()
            },
            password: String::new(),
            clear_password: false,
            auto_join: n.auto_join.join("\n"),
            perform: n.perform.join("\n"),
        }
    }

    /// Fill the server part from a preset; your identity stays.
    pub fn apply(&mut self, preset: &c::NetworkPreset) {
        self.name = preset.name.clone();
        self.host = preset.host.clone();
        self.port = preset.port.to_string();
        self.tls = preset.tls;
        self.login = Login::of_preset(preset.auth_method);
        self.auto_join = preset.auto_join.join("\n");
        self.password.clear();
        self.account.clear();
        self.clear_password = false;
    }

    pub fn port(&self) -> Option<u16> {
        self.port.trim().parse::<u16>().ok().filter(|p| *p > 0)
    }

    /// The normalised address to check, when it is a valid one.
    pub fn endpoint(&self) -> Option<(String, u16, bool)> {
        Some((normalize_host(self.host.trim())?, self.port()?, self.tls))
    }

    /// Whether the server must check the address before saving: always for a
    /// new network, and when an edit changes the address.
    pub fn needs_check(&self, existing: Option<&c::Network>) -> bool {
        let Some(n) = existing else {
            return true;
        };
        normalize_host(self.host.trim()).as_deref() != normalize_host(&n.host).as_deref()
            || self.port() != u16::try_from(n.port).ok()
            || self.tls != n.tls
    }

    /// Whether a password is already stored for the chosen login: saved on
    /// the network, or provided by the server's preset for this address.
    pub fn password_set(
        &self,
        existing: Option<&c::Network>,
        presets: &[c::NetworkPreset],
    ) -> bool {
        match existing {
            Some(n) => n.auth.password_set && Login::of(n.auth.method) == self.login,
            None => presets.iter().any(|p| {
                p.password_provided
                    && normalize_host(&p.host) == normalize_host(self.host.trim())
                    && Some(p.port) == self.port().map(i64::from)
                    && p.tls == self.tls
                    && Login::of_preset(p.auth_method) == self.login
            }),
        }
    }

    /// What must be fixed before saving. `isupport` gives the nick length.
    pub fn problems(
        &self,
        existing: Option<&c::Network>,
        presets: &[c::NetworkPreset],
    ) -> Problems {
        let max = existing.map_or(32, |n| nickname_limit(&n.isupport));
        let password_set = self.password_set(existing, presets) && !self.clear_password;
        Problems {
            name: self.name.trim().is_empty(),
            host: normalize_host(self.host.trim()).is_none(),
            port: self.port().is_none(),
            nick: nickname_error(&self.nick, max),
            alt_nick: if self.alt_nick.is_empty() {
                None
            } else {
                nickname_error(&self.alt_nick, max)
            },
            account: self.login == Login::Sasl && self.account.trim().is_empty(),
            password: self.login != Login::None && self.password.is_empty() && !password_set,
        }
    }

    /// The login part: a typed password replaces the saved one, an empty one
    /// keeps it, and "forget" sends an empty one.
    fn auth(&self) -> c::AuthInput {
        let password = if !self.password.is_empty() {
            Some(self.password.clone())
        } else if self.clear_password {
            Some(String::new())
        } else {
            None
        };
        match self.login {
            Login::None => c::AuthInput {
                method: c::AuthInputMethod::None,
                ..Default::default()
            },
            Login::Sasl => c::AuthInput {
                method: self.login.method(),
                account: Some(self.account.trim().to_string()),
                password,
            },
            Login::NickServ | Login::ServerPassword => c::AuthInput {
                method: self.login.method(),
                account: None,
                password,
            },
        }
    }

    fn optional(text: &str) -> Option<String> {
        let text = text.trim();
        (!text.is_empty()).then(|| text.to_string())
    }

    fn host(&self) -> String {
        normalize_host(self.host.trim()).unwrap_or_else(|| self.host.trim().to_string())
    }

    /// The request that adds this network.
    pub fn config(&self) -> c::NetworkConfig {
        c::NetworkConfig {
            name: self.name.trim().to_string(),
            host: self.host(),
            port: self.port().map_or(0, i64::from),
            tls: self.tls,
            nick: self.nick.clone(),
            alt_nick: Self::optional(&self.alt_nick),
            username: Self::optional(&self.username),
            realname: Self::optional(&self.realname),
            auth: Some(self.auth()),
            auto_join: Some(auto_join(&self.auto_join)),
            perform: Some(lines(&self.perform)),
            ..Default::default()
        }
    }

    /// The request that saves this form over an existing network.
    pub fn update(&self) -> c::NetworkUpdate {
        c::NetworkUpdate {
            name: Some(self.name.trim().to_string()),
            host: Some(self.host()),
            port: self.port().map(i64::from),
            tls: Some(self.tls),
            nick: Some(self.nick.clone()),
            alt_nick: Some(self.alt_nick.trim().to_string()),
            username: Some(self.username.trim().to_string()),
            realname: Some(self.realname.trim().to_string()),
            auth: Some(self.auth()),
            auto_join: Some(auto_join(&self.auto_join)),
            perform: Some(lines(&self.perform)),
            status: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn network() -> c::Network {
        c::Network {
            id: "n1".into(),
            name: "Libera".into(),
            host: "irc.libera.chat".into(),
            port: 6697,
            tls: true,
            nick: "me".into(),
            auth: c::NetworkAuth {
                account: Some("me".into()),
                method: c::NetworkAuthMethod::SaslMinusPlain,
                password_set: true,
            },
            auto_join: vec!["#rust".into(), "#irc key".into()],
            perform: vec!["MODE me +x".into()],
            ..Default::default()
        }
    }

    #[test]
    fn hosts_are_normalised_like_the_server_does() {
        assert_eq!(
            normalize_host("IRC.Libera.Chat.").as_deref(),
            Some("irc.libera.chat")
        );
        assert_eq!(normalize_host("localhost").as_deref(), Some("localhost"));
        assert_eq!(
            normalize_host("192.168.1.10").as_deref(),
            Some("192.168.1.10")
        );
        assert_eq!(
            normalize_host("[2001:DB8::1]").as_deref(),
            Some("2001:db8::1")
        );
        assert_eq!(
            normalize_host("::ffff:c000:0201").as_deref(),
            Some("192.0.2.1")
        );
        for bad in [
            "",
            " irc.example.org",
            "irc.example.org/",
            "ircs://irc.example.org",
            "me@irc.example.org",
            "irc..example.org",
            "-irc.example.org",
            "300.1.1.1",
            "01.2.3.4",
            "irc.example.org:6697",
            "[fe80::1%eth0]",
            "irc_example.org",
        ] {
            assert_eq!(normalize_host(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn auto_join_adds_the_hash_and_keeps_keys() {
        assert_eq!(
            auto_join("rust\n  #irc key \n\n&local"),
            ["#rust", "#irc key", "&local"]
        );
    }

    #[test]
    fn an_edit_keeps_the_saved_password_unless_typed_or_forgotten() {
        let n = network();
        let mut form = Form::of(&n);
        assert_eq!(form.login, Login::Sasl);
        assert_eq!(form.account, "me");
        assert_eq!(form.auto_join, "#rust\n#irc key");
        assert!(
            !form.problems(Some(&n), &[]).any(),
            "the saved password counts"
        );
        let update = form.update();
        assert_eq!(update.auth.as_ref().unwrap().password, None, "kept");
        assert_eq!(update.status, None);
        form.clear_password = true;
        assert!(
            form.problems(Some(&n), &[]).password,
            "forgotten, so one is needed"
        );
        assert_eq!(form.update().auth.unwrap().password.as_deref(), Some(""));
        form.password = "hunter2".into();
        assert!(!form.problems(Some(&n), &[]).any());
        assert_eq!(
            form.update().auth.unwrap().password.as_deref(),
            Some("hunter2")
        );
        // Another method has no saved password.
        form.password.clear();
        form.clear_password = false;
        form.login = Login::NickServ;
        assert!(form.problems(Some(&n), &[]).password);
    }

    #[test]
    fn a_new_network_needs_a_valid_address_nick_and_password() {
        let mut form = Form::new("me");
        let p = form.problems(None, &[]);
        assert!(p.name && p.host && !p.port && p.nick.is_none());
        form.name = "Home".into();
        form.host = "IRC.Example.org".into();
        form.port = "70000".into();
        form.nick = "1abc".into();
        form.alt_nick = "has space".into();
        form.login = Login::Sasl;
        let p = form.problems(None, &[]);
        assert!(!p.name && !p.host);
        assert!(p.port && p.account && p.password);
        assert_eq!(p.nick, Some(NickError::Syntax));
        assert_eq!(p.alt_nick, Some(NickError::Spaces));
        form.port = "6697".into();
        form.nick = "me".into();
        form.alt_nick = "me_".into();
        form.account = "me".into();
        form.password = "pw".into();
        assert!(!form.problems(None, &[]).any());
        form.auto_join = "rust".into();
        let config = form.config();
        assert_eq!(config.host, "irc.example.org");
        assert_eq!(config.port, 6697);
        assert_eq!(config.alt_nick.as_deref(), Some("me_"));
        assert_eq!(config.username, None);
        assert_eq!(
            config.auto_join.as_deref(),
            Some(&["#rust".to_string()][..])
        );
        let auth = config.auth.unwrap();
        assert_eq!(auth.method, c::AuthInputMethod::SaslMinusPlain);
        assert_eq!(auth.account.as_deref(), Some("me"));
        assert_eq!(auth.password.as_deref(), Some("pw"));
    }

    #[test]
    fn presets_fill_the_server_and_may_provide_the_password() {
        let preset = c::NetworkPreset {
            auth_method: c::NetworkPresetAuthMethod::ServerMinusPassword,
            auto_join: vec!["#lobby".into()],
            host: "irc.example.org".into(),
            name: "Example".into(),
            password_provided: true,
            port: 6697,
            tls: true,
            ..Default::default()
        };
        let mut form = Form::new("me");
        form.password = "typed".into();
        form.apply(&preset);
        assert_eq!(form.name, "Example");
        assert_eq!(form.login, Login::ServerPassword);
        assert_eq!(form.auto_join, "#lobby");
        assert_eq!(
            form.password, "",
            "a preset never carries a typed password over"
        );
        assert!(
            !form.problems(None, std::slice::from_ref(&preset)).password,
            "provided"
        );
        form.tls = false;
        assert!(
            form.problems(None, &[preset]).password,
            "only for the preset's own address"
        );
        assert_eq!(presets(None).len(), 4);
        assert_eq!(
            presets(Some(c::NetworkPresetPage {
                configured: true,
                items: Vec::new()
            }))
            .len(),
            0,
            "a server that configures none offers none"
        );
    }

    #[test]
    fn only_a_changed_address_is_checked_again() {
        let n = network();
        let mut form = Form::of(&n);
        assert!(!form.needs_check(Some(&n)));
        form.host = "IRC.LIBERA.CHAT".into();
        assert!(!form.needs_check(Some(&n)), "the same host, as typed");
        form.port = "6667".into();
        assert!(form.needs_check(Some(&n)));
        assert!(Form::new("me").needs_check(None));
    }

    #[test]
    fn saved_and_removed_change_the_workspace() {
        let mut w = Workspace::default();
        let changes = saved(&mut w, network());
        assert!(changes.sidebar);
        assert_eq!(w.networks.len(), 1);
        let mut renamed = network();
        renamed.name = "Renamed".into();
        saved(&mut w, renamed);
        assert_eq!(w.networks.len(), 1);
        assert_eq!(w.networks[0].name, "Renamed");
        w.conversations.push(c::Conversation {
            id: "c1".into(),
            network_id: "n1".into(),
            ..Default::default()
        });
        let changes = removed(&mut w, "n1");
        assert!(w.networks.is_empty() && w.conversations.is_empty());
        assert!(changes.timelines.contains("c1"));
    }
}

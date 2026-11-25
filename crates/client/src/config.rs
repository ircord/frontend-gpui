//! Files the app keeps between launches, under the platform config directory
//! in `com.ircord.gpui` (so it sits beside the Tauri app without sharing files):
//!
//! - `server`: the server origin, one line.
//! - `state.json`: the last conversation viewed, each network's last channel,
//!   the window size, and one section per feature (see [`ConfigDir::section`]).
//!
//! The sign-in itself is not here: it lives in the operating system's
//! credential store (see [`crate::credentials`]).

use crate::address::ServerAddress;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

pub const APP_ID: &str = "com.ircord.gpui";

/// The key in `state.json` that holds the features' sections.
const FEATURES: &str = "features";

#[derive(Debug, Clone)]
pub struct ConfigDir(PathBuf);

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AppState {
    pub last_conversation: Option<String>,
    last_channels: BTreeMap<String, BTreeMap<String, String>>,
    pub window_width: Option<f32>,
    pub window_height: Option<f32>,
}

impl AppState {
    pub fn last_channel(
        &self,
        server: &ServerAddress,
        account: &str,
        network: &str,
    ) -> Option<&str> {
        self.last_channels
            .get(&format!("{} {account}", server.origin()))?
            .get(network)
            .map(String::as_str)
    }

    pub fn remember_channel(
        &mut self,
        server: &ServerAddress,
        account: &str,
        network: &str,
        channel: &str,
    ) {
        self.last_channels
            .entry(format!("{} {account}", server.origin()))
            .or_default()
            .insert(network.to_string(), channel.to_string());
    }
}

impl ConfigDir {
    /// The platform directory, e.g. `%APPDATA%\com.ircord.gpui` on Windows.
    pub fn platform() -> io::Result<Self> {
        let base = dirs::config_dir()
            .ok_or_else(|| io::Error::other("this system has no configuration directory"))?;
        Ok(Self(base.join(APP_ID)))
    }

    pub fn at(path: impl Into<PathBuf>) -> Self {
        Self(path.into())
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    /// The saved server, if any. `IRCORD_SERVER` overrides it for development.
    pub fn server(&self) -> io::Result<Option<ServerAddress>> {
        if let Ok(address) = std::env::var("IRCORD_SERVER") {
            return ServerAddress::parse(&address)
                .map(Some)
                .map_err(io::Error::other);
        }
        match std::fs::read_to_string(self.0.join("server")) {
            Ok(text) => Ok(ServerAddress::parse(&text).ok()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    pub fn save_server(&self, address: &ServerAddress) -> io::Result<()> {
        self.write("server", address.origin().as_bytes())
    }

    pub fn forget_server(&self) -> io::Result<()> {
        match std::fs::remove_file(self.0.join("server")) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }

    pub fn state(&self) -> AppState {
        std::fs::read(self.0.join("state.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    /// Save the app's own state, keeping every feature's section.
    pub fn save_state(&self, state: &AppState) -> io::Result<()> {
        let mut doc = self.state_document();
        let Value::Object(fields) = serde_json::to_value(state)? else {
            unreachable!("AppState serialises to an object")
        };
        doc.extend(fields);
        self.write("state.json", &serde_json::to_vec_pretty(&doc)?)
    }

    /// A feature's own section of the device state, by a name the feature
    /// picks (its module name). A missing or unreadable section is the default.
    pub fn section<T: DeserializeOwned + Default>(&self, name: &str) -> T {
        self.state_document()
            .get(FEATURES)
            .and_then(|f| f.get(name))
            .and_then(|v| T::deserialize(v).ok())
            .unwrap_or_default()
    }

    /// Replace a feature's section, keeping everything else in the file.
    pub fn save_section<T: Serialize>(&self, name: &str, value: &T) -> io::Result<()> {
        let mut doc = self.state_document();
        let features = doc
            .entry(FEATURES)
            .or_insert_with(|| Value::Object(Map::new()));
        if !features.is_object() {
            *features = Value::Object(Map::new());
        }
        features
            .as_object_mut()
            .expect("made an object above")
            .insert(name.to_string(), serde_json::to_value(value)?);
        self.write("state.json", &serde_json::to_vec_pretty(&doc)?)
    }

    fn state_document(&self) -> Map<String, Value> {
        std::fs::read(self.0.join("state.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    /// Write through a temporary file and rename, so a crash never leaves half a file.
    fn write(&self, name: &str, bytes: &[u8]) -> io::Result<()> {
        std::fs::create_dir_all(&self.0)?;
        let path = self.0.join(name);
        let staging = path.with_extension(format!("{}.tmp", std::process::id()));
        std::fs::write(&staging, bytes)?;
        std::fs::rename(staging, path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_and_state_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let config = ConfigDir::at(dir.path().join("app"));
        // SAFETY-free: the variable is only read, never set, by these tests.
        if std::env::var("IRCORD_SERVER").is_ok() {
            return;
        }
        assert_eq!(config.server().unwrap(), None);
        let address = ServerAddress::parse("https://chat.ircord.com").unwrap();
        config.save_server(&address).unwrap();
        assert_eq!(config.server().unwrap(), Some(address));
        config.forget_server().unwrap();
        assert_eq!(config.server().unwrap(), None);

        assert_eq!(config.state(), AppState::default());
        let state = AppState {
            last_conversation: Some("c1".into()),
            window_width: Some(1200.0),
            ..Default::default()
        };
        config.save_state(&state).unwrap();
        assert_eq!(config.state(), state);
    }

    #[test]
    fn last_channels_survive_relaunch_and_are_scoped_by_server_and_account() {
        let dir = tempfile::tempdir().unwrap();
        let config = ConfigDir::at(dir.path());
        let first = ServerAddress::parse("https://chat.example.org").unwrap();
        let second = ServerAddress::parse("https://other.example.org").unwrap();
        let mut state: AppState = serde_json::from_str(r#"{"lastConversation":"old"}"#).unwrap();
        assert_eq!(state.last_conversation.as_deref(), Some("old"));
        assert_eq!(state.last_channel(&first, "me", "home"), None);
        state.remember_channel(&first, "me", "home", "c1");
        state.remember_channel(&first, "me", "away", "c2");
        state.remember_channel(&first, "someone-else", "home", "c3");
        state.remember_channel(&second, "me", "home", "c4");
        config.save_state(&state).unwrap();

        let restored = config.state();
        assert_eq!(restored.last_channel(&first, "me", "home"), Some("c1"));
        assert_eq!(restored.last_channel(&first, "me", "away"), Some("c2"));
        assert_eq!(
            restored.last_channel(&first, "someone-else", "home"),
            Some("c3")
        );
        assert_eq!(restored.last_channel(&second, "me", "home"), Some("c4"));
    }

    #[test]
    fn feature_sections_survive_each_other_and_the_app_state() {
        #[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
        struct Drafts {
            text: String,
        }
        let dir = tempfile::tempdir().unwrap();
        let config = ConfigDir::at(dir.path());
        assert_eq!(config.section::<Drafts>("drafts"), Drafts::default());
        let drafts = Drafts { text: "hi".into() };
        config.save_section("drafts", &drafts).unwrap();
        config.save_section("sidebar", &vec!["c1"]).unwrap();
        config
            .save_state(&AppState {
                last_conversation: Some("c1".into()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(config.section::<Drafts>("drafts"), drafts);
        assert_eq!(config.section::<Vec<String>>("sidebar"), ["c1"]);
        assert_eq!(config.state().last_conversation.as_deref(), Some("c1"));
        // A section of the wrong shape reads as the default.
        assert_eq!(config.section::<u32>("drafts"), 0);
    }

    #[test]
    fn a_damaged_server_file_reads_as_no_server() {
        let dir = tempfile::tempdir().unwrap();
        if std::env::var("IRCORD_SERVER").is_ok() {
            return;
        }
        std::fs::write(dir.path().join("server"), "not an address").unwrap();
        assert_eq!(ConfigDir::at(dir.path()).server().unwrap(), None);
    }
}

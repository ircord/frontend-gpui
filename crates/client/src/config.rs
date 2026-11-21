//! Files the app keeps between launches, under the platform config directory
//! in `com.ircord.gpui` (so it sits beside the Tauri app without sharing files):
//!
//! - `server`: the server origin, one line.
//! - `state.json`: the last conversation viewed and the window size.
//!
//! The sign-in itself is not here: it lives in the operating system's
//! credential store (see [`crate::credentials`]).

use crate::address::ServerAddress;
use serde::{Deserialize, Serialize};
use std::io;
use std::path::{Path, PathBuf};

pub const APP_ID: &str = "com.ircord.gpui";

#[derive(Debug, Clone)]
pub struct ConfigDir(PathBuf);

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AppState {
    pub last_conversation: Option<String>,
    pub window_width: Option<f32>,
    pub window_height: Option<f32>,
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

    pub fn save_state(&self, state: &AppState) -> io::Result<()> {
        self.write("state.json", &serde_json::to_vec_pretty(state)?)
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
            window_height: None,
        };
        config.save_state(&state).unwrap();
        assert_eq!(config.state(), state);
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

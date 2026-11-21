//! Where the sign-in is kept: the session cookie for each server.
//!
//! The app uses [`SystemStore`], the operating system's credential store
//! (Windows Credential Manager, the macOS keychain, the Secret Service on
//! Linux). A Linux session without a keyring service reports that plainly as
//! an error; there is deliberately no fallback to a file. Tests use
//! [`MemoryStore`].

use std::collections::HashMap;
use std::sync::Mutex;

use crate::address::ServerAddress;
use crate::config::APP_ID;

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct StoreError(pub String);

pub trait CredentialStore: Send + Sync + 'static {
    /// The saved session for this server, if any.
    fn load(&self, server: &ServerAddress) -> Result<Option<String>, StoreError>;
    fn save(&self, server: &ServerAddress, session: &str) -> Result<(), StoreError>;
    /// Forget the session. Forgetting one that is not there is not an error.
    fn clear(&self, server: &ServerAddress) -> Result<(), StoreError>;
}

/// The operating system's credential store, one entry per server origin.
#[derive(Debug, Default)]
pub struct SystemStore;

impl SystemStore {
    fn entry(server: &ServerAddress) -> Result<keyring::Entry, StoreError> {
        keyring::Entry::new(APP_ID, &server.origin()).map_err(explain)
    }
}

fn explain(e: keyring::Error) -> StoreError {
    match e {
        keyring::Error::NoDefaultStore | keyring::Error::NoStorageAccess(_) => StoreError(format!(
            "This computer's credential store is not available, so the sign-in cannot be kept: {e}. \
             On Linux, start a keyring service such as GNOME Keyring or KWallet."
        )),
        e => StoreError(format!("The credential store refused the sign-in: {e}")),
    }
}

impl CredentialStore for SystemStore {
    fn load(&self, server: &ServerAddress) -> Result<Option<String>, StoreError> {
        match Self::entry(server)?.get_password() {
            Ok(secret) => Ok(Some(secret)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(explain(e)),
        }
    }

    fn save(&self, server: &ServerAddress, session: &str) -> Result<(), StoreError> {
        Self::entry(server)?.set_password(session).map_err(explain)
    }

    fn clear(&self, server: &ServerAddress) -> Result<(), StoreError> {
        match Self::entry(server)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(explain(e)),
        }
    }
}

/// An in-memory store for tests.
#[derive(Debug, Default)]
pub struct MemoryStore(Mutex<HashMap<String, String>>);

impl CredentialStore for MemoryStore {
    fn load(&self, server: &ServerAddress) -> Result<Option<String>, StoreError> {
        Ok(self.0.lock().unwrap().get(&server.origin()).cloned())
    }

    fn save(&self, server: &ServerAddress, session: &str) -> Result<(), StoreError> {
        self.0
            .lock()
            .unwrap()
            .insert(server.origin(), session.to_string());
        Ok(())
    }

    fn clear(&self, server: &ServerAddress) -> Result<(), StoreError> {
        self.0.lock().unwrap().remove(&server.origin());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_store_keeps_one_session_per_server() {
        let store = MemoryStore::default();
        let a = ServerAddress::parse("https://a.example").unwrap();
        let b = ServerAddress::parse("https://b.example").unwrap();
        assert_eq!(store.load(&a).unwrap(), None);
        store.save(&a, "ircord_session=1").unwrap();
        store.save(&b, "ircord_session=2").unwrap();
        assert_eq!(store.load(&a).unwrap().as_deref(), Some("ircord_session=1"));
        store.clear(&a).unwrap();
        store.clear(&a).unwrap();
        assert_eq!(store.load(&a).unwrap(), None);
        assert_eq!(store.load(&b).unwrap().as_deref(), Some("ircord_session=2"));
    }
}

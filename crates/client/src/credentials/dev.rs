//! The macOS development launcher has its own sign-in, so rebuilding it
//! never requires authorization to read the installed app's Keychain item.

use std::fs::{self, DirBuilder, OpenOptions};
use std::io::ErrorKind;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

use rusqlite::{Connection, OpenFlags, OptionalExtension, params};

use super::{CredentialStore, ServerAddress, StoreError};

pub struct DevStore(Mutex<Connection>);

impl DevStore {
    pub fn open(directory: &Path) -> Result<Self, StoreError> {
        if let Some(parent) = directory.parent() {
            fs::create_dir_all(parent).map_err(store_error)?;
        }
        match DirBuilder::new().mode(0o700).create(directory) {
            Ok(()) => {}
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
            Err(error) => return Err(store_error(error)),
        }
        let directory_meta = fs::symlink_metadata(directory).map_err(store_error)?;
        if !directory_meta.is_dir() || directory_meta.permissions().mode() & 0o077 != 0 {
            return Err(store_error(
                "the directory must be private and cannot be a symbolic link",
            ));
        }

        // macOS gives temporary directories a /var path, where /var itself
        // is a symlink. Resolve the ancestors before opening with NOFOLLOW.
        let path = directory
            .canonicalize()
            .map_err(store_error)?
            .join("credentials.sqlite3");
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
        {
            Ok(_) => {}
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
            Err(error) => return Err(store_error(error)),
        }
        let file_meta = fs::symlink_metadata(&path).map_err(store_error)?;
        if !file_meta.is_file() || file_meta.permissions().mode() & 0o077 != 0 {
            return Err(store_error("the database must be a private regular file"));
        }
        let connection = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )
        .map_err(store_error)?;
        connection
            .busy_timeout(Duration::from_secs(2))
            .map_err(store_error)?;
        connection
            .execute_batch(
                "PRAGMA secure_delete = ON;
                 CREATE TABLE IF NOT EXISTS credentials (
                     kind TEXT NOT NULL,
                     scope TEXT NOT NULL,
                     secret TEXT NOT NULL,
                     PRIMARY KEY (kind, scope)
                 )",
            )
            .map_err(store_error)?;
        Ok(Self(Mutex::new(connection)))
    }

    fn read(&self, kind: &str, scope: &str) -> Result<Option<String>, StoreError> {
        self.0
            .lock()
            .expect("development credential database lock")
            .query_row(
                "SELECT secret FROM credentials WHERE kind = ?1 AND scope = ?2",
                params![kind, scope],
                |row| row.get(0),
            )
            .optional()
            .map_err(store_error)
    }

    fn write(&self, kind: &str, scope: &str, value: Option<&str>) -> Result<(), StoreError> {
        let db = self.0.lock().expect("development credential database lock");
        match value {
            Some(secret) => db.execute(
                "INSERT INTO credentials (kind, scope, secret) VALUES (?1, ?2, ?3)
                 ON CONFLICT (kind, scope) DO UPDATE SET secret = excluded.secret",
                params![kind, scope, secret],
            ),
            None => db.execute(
                "DELETE FROM credentials WHERE kind = ?1 AND scope = ?2",
                params![kind, scope],
            ),
        }
        .map(|_| ())
        .map_err(store_error)
    }
}

fn store_error(error: impl std::fmt::Display) -> StoreError {
    StoreError(format!(
        "The development credential store is unavailable: {error}"
    ))
}

impl CredentialStore for DevStore {
    fn load(&self, server: &ServerAddress) -> Result<Option<String>, StoreError> {
        self.read("session", &server.origin())
    }

    fn save(&self, server: &ServerAddress, session: &str) -> Result<(), StoreError> {
        self.write("session", &server.origin(), Some(session))
    }

    fn clear(&self, server: &ServerAddress) -> Result<(), StoreError> {
        self.write("session", &server.origin(), None)
    }

    fn speech_key(&self, scope: &str) -> Result<Option<String>, StoreError> {
        self.read("speech", scope)
    }

    fn save_speech_key(&self, scope: &str, key: Option<&str>) -> Result<(), StoreError> {
        self.write("speech", scope, key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_in_survives_reopening_and_stays_scoped() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dev");
        let first = ServerAddress::parse("https://first.example").unwrap();
        let other = ServerAddress::parse("https://second.example").unwrap();
        let store = DevStore::open(&path).unwrap();
        store.save(&first, "first").unwrap();
        store.save(&other, "other").unwrap();
        store
            .save_speech_key(&first.origin(), Some("speech"))
            .unwrap();
        drop(store);

        let store = DevStore::open(&path).unwrap();
        assert_eq!(store.load(&first).unwrap().as_deref(), Some("first"));
        store.save(&first, "replaced").unwrap();
        assert_eq!(store.load(&first).unwrap().as_deref(), Some("replaced"));
        store.clear(&first).unwrap();
        assert_eq!(store.load(&first).unwrap(), None);
        assert_eq!(store.load(&other).unwrap().as_deref(), Some("other"));
        assert_eq!(
            store.speech_key(&first.origin()).unwrap().as_deref(),
            Some("speech")
        );
        assert_eq!(
            fs::metadata(path.join("credentials.sqlite3"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }

    #[test]
    fn rejects_a_shared_credential_directory() {
        let dir = tempfile::tempdir().unwrap();
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o755)).unwrap();
        assert!(DevStore::open(dir.path()).is_err());
    }
}

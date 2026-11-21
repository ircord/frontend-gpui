//! The server's revisioned documents (preferences, appearance, themes,
//! library, notification settings), the way the web client keeps them
//! (`app/stores/preferences.ts`):
//!
//! - An edit shows at once and is saved after a short delay; edits made while
//!   a save runs are coalesced into the next one, latest wins.
//! - A `*.updated` event means another device changed the document: read it
//!   again, but not while a save runs; a save that overlapped such an event is
//!   followed by a read, because its answer may be older than the event. An
//!   event during a read is followed by another read, for the same reason.
//! - A save waits for a running read: an answer that left the server before
//!   the save would otherwise replace it.
//! - A failed save falls back to the server's copy and reads it again.
//!
//! [`Revisioned`] is the pure state and [`Documents`] holds the five in the
//! workspace; `Client::watch_document` and `Client::edit_document` drive one
//! against the server with its spec from `crate::documents`.

use ircord_contract::event::Document;
use ircord_contract::{Library, Settings, StoredAppearance, StoredPreferences, Themes};

/// The account's revisioned documents, as stored on the server. Each stays
/// unread until a feature watches it.
#[derive(Debug, Clone, Default)]
pub struct Documents {
    pub preferences: Revisioned<StoredPreferences>,
    pub appearance: Revisioned<StoredAppearance>,
    pub themes: Revisioned<Themes>,
    pub library: Revisioned<Library>,
    pub notification_settings: Revisioned<Settings>,
}

/// A short, stable name for a document: the ID reported under
/// [`super::topic::DOCUMENTS`].
pub fn document_id(document: Document) -> &'static str {
    match document {
        Document::Preferences => "preferences",
        Document::Appearance => "appearance",
        Document::Themes => "themes",
        Document::Library => "library",
        Document::NotificationSettings => "notification_settings",
    }
}

/// Where the newest edit is.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum SaveState {
    /// Nothing edited since the document was read.
    #[default]
    Idle,
    /// An edit is waiting or being sent.
    Saving,
    /// The newest edit is stored on the server.
    Saved,
    /// The server refused the newest edit; the server's copy is shown again.
    Failed(String),
}

/// One revisioned document and its unsaved edit.
#[derive(Debug, Clone)]
pub struct Revisioned<T> {
    value: Option<T>,
    server: Option<T>,
    save: SaveState,
    pending: Option<T>,
    in_flight: bool,
    reading: bool,
    stale: bool,
    remote_during_save: bool,
}

impl<T> Default for Revisioned<T> {
    fn default() -> Self {
        Self {
            value: None,
            server: None,
            save: SaveState::Idle,
            pending: None,
            in_flight: false,
            reading: false,
            stale: false,
            remote_during_save: false,
        }
    }
}

impl<T: Clone> Revisioned<T> {
    /// What to show: the newest edit, or the server's copy.
    pub fn value(&self) -> Option<&T> {
        self.value.as_ref()
    }

    /// The server's last copy.
    pub fn server(&self) -> Option<&T> {
        self.server.as_ref()
    }

    pub fn save_state(&self) -> &SaveState {
        &self.save
    }

    /// The document was read at least once.
    pub fn is_loaded(&self) -> bool {
        self.server.is_some()
    }

    /// Whether a read should start now: never read, or the server has a newer
    /// revision, and no read or save is running.
    pub fn needs_read(&self) -> bool {
        !self.reading && !self.in_flight && (self.server.is_none() || self.stale)
    }

    /// Whether an edit is waiting to be sent and no save or read is running.
    pub fn needs_save(&self) -> bool {
        !self.in_flight && !self.reading && self.pending.is_some()
    }

    /// Start a read if one is needed. True when the caller should read.
    pub fn begin_read(&mut self) -> bool {
        if !self.needs_read() {
            return false;
        }
        self.reading = true;
        self.stale = false;
        true
    }

    /// A read finished. A failed read leaves the document stale for the next try.
    pub fn read_done(&mut self, result: Result<T, String>) {
        self.reading = false;
        match result {
            Ok(value) => {
                if self.pending.is_none() && !self.in_flight {
                    self.value = Some(value.clone());
                }
                self.server = Some(value);
            }
            Err(_) => self.stale = true,
        }
    }

    /// The user changed the document: show it now, save it soon.
    pub fn edit(&mut self, value: T) {
        self.value = Some(value.clone());
        self.pending = Some(value);
        self.save = SaveState::Saving;
    }

    /// Take the edit to send, if one waits and no save or read is running.
    pub fn begin_save(&mut self) -> Option<T> {
        if self.in_flight || self.reading {
            return None;
        }
        let value = self.pending.take()?;
        self.in_flight = true;
        self.remote_during_save = false;
        Some(value)
    }

    /// A save finished with the server's answer.
    pub fn save_done(&mut self, result: Result<T, String>) {
        self.in_flight = false;
        match result {
            Ok(saved) => {
                if self.remote_during_save {
                    // Another device's change may be newer than this answer.
                    self.stale = true;
                } else {
                    if self.pending.is_none() {
                        self.value = Some(saved.clone());
                    }
                    self.server = Some(saved);
                }
                self.save = if self.pending.is_some() {
                    SaveState::Saving
                } else {
                    SaveState::Saved
                };
            }
            Err(reason) => {
                // A failed edit must not mask later changes from another device.
                self.pending = None;
                self.value = self.server.clone();
                self.save = SaveState::Failed(reason);
                self.stale = true;
            }
        }
        self.remote_during_save = false;
    }

    /// The server announced a newer revision.
    pub fn remote(&mut self) {
        if self.in_flight {
            self.remote_during_save = true;
        } else {
            self.stale = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_once_then_only_when_stale() {
        let mut d = Revisioned::<u32>::default();
        assert!(d.begin_read());
        assert!(!d.begin_read(), "one read at a time");
        d.read_done(Ok(1));
        assert_eq!(d.value(), Some(&1));
        assert!(!d.needs_read());
        d.remote();
        assert!(d.begin_read());
        d.read_done(Err("offline".into()));
        assert!(d.needs_read(), "a failed read is tried again");
    }

    #[test]
    fn edits_coalesce_and_the_latest_wins() {
        let mut d = Revisioned::default();
        d.begin_read();
        d.read_done(Ok(1));
        d.edit(2);
        assert_eq!(d.value(), Some(&2));
        assert_eq!(d.save_state(), &SaveState::Saving);
        assert_eq!(d.begin_save(), Some(2));
        d.edit(3);
        d.edit(4);
        assert_eq!(d.begin_save(), None, "one save at a time");
        d.save_done(Ok(2));
        assert_eq!(d.value(), Some(&4), "the newer edit stays shown");
        assert_eq!(d.save_state(), &SaveState::Saving);
        assert_eq!(d.begin_save(), Some(4));
        d.save_done(Ok(4));
        assert_eq!(d.save_state(), &SaveState::Saved);
        assert_eq!(d.server(), Some(&4));
        assert!(!d.needs_read());
    }

    #[test]
    fn a_remote_change_during_a_save_reads_afterwards() {
        let mut d = Revisioned::default();
        d.begin_read();
        d.read_done(Ok(1));
        d.edit(2);
        d.begin_save();
        d.remote();
        assert!(!d.needs_read(), "not while the save runs");
        d.save_done(Ok(2));
        assert!(d.begin_read(), "the answer may be older than the event");
        d.read_done(Ok(7));
        assert_eq!(d.value(), Some(&7));
    }

    #[test]
    fn a_save_waits_for_a_running_read() {
        let mut d = Revisioned::default();
        d.begin_read();
        d.read_done(Ok(1));
        d.remote();
        assert!(d.begin_read());
        d.edit(2);
        assert!(!d.needs_save(), "not while the read runs");
        assert_eq!(d.begin_save(), None);
        // The read left the server before the edit: it must not replace it.
        d.read_done(Ok(1));
        assert_eq!(d.value(), Some(&2));
        assert!(d.needs_save());
        assert_eq!(d.begin_save(), Some(2));
        d.save_done(Ok(2));
        assert_eq!(d.value(), Some(&2));
        assert!(!d.needs_read());
    }

    #[test]
    fn a_remote_change_during_a_read_reads_again() {
        let mut d = Revisioned::default();
        assert!(d.begin_read());
        d.remote();
        d.read_done(Ok(1));
        assert!(d.needs_read(), "the answer may be older than the event");
        assert!(d.begin_read());
        d.read_done(Ok(7));
        assert_eq!(d.value(), Some(&7));
        assert!(!d.needs_read());
    }

    #[test]
    fn a_failed_save_shows_the_server_copy_and_reads_again() {
        let mut d = Revisioned::default();
        d.begin_read();
        d.read_done(Ok(1));
        d.edit(2);
        d.begin_save();
        d.save_done(Err("too large".into()));
        assert_eq!(d.value(), Some(&1));
        assert_eq!(d.save_state(), &SaveState::Failed("too large".into()));
        assert!(d.needs_read());
    }
}

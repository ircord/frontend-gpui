//! The last unsaved Theme Studio draft for each server and account on this device.

use std::{collections::BTreeMap, io};

use ircord_contract::Theme;
use serde::{Deserialize, Serialize};

use super::Mode;
use crate::{ServerAddress, config::ConfigDir};

const SECTION: &str = "theme-studio";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Draft {
    pub theme: Theme,
    pub mode: Mode,
    pub new_theme: bool,
}

pub struct Store {
    config: ConfigDir,
    owner: String,
}

impl Store {
    pub fn new(config: &ConfigDir, server: &ServerAddress, account: &str) -> Option<Self> {
        if account.is_empty() {
            return None;
        }
        Some(Self {
            config: config.clone(),
            owner: format!("{} {account}", server.origin()),
        })
    }

    pub fn get(&self) -> Option<Draft> {
        self.config
            .section::<BTreeMap<String, Draft>>(SECTION)
            .remove(&self.owner)
    }

    pub fn put(&self, draft: Draft) -> io::Result<()> {
        let mut drafts: BTreeMap<String, Draft> = self.config.section(SECTION);
        drafts.insert(self.owner.clone(), draft);
        self.config.save_section(SECTION, &drafts)
    }

    /// A discard or completed save only clears a draft for that exact theme.
    pub fn remove(&self, id: &str) -> io::Result<()> {
        let mut drafts: BTreeMap<String, Draft> = self.config.section(SECTION);
        if drafts
            .get(&self.owner)
            .is_some_and(|draft| draft.theme.id.as_deref() == Some(id))
        {
            drafts.remove(&self.owner);
            self.config.save_section(SECTION, &drafts)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drafts_survive_reopening_and_are_isolated_by_account_and_server() {
        let dir = tempfile::tempdir().unwrap();
        let config = ConfigDir::at(dir.path());
        let primary = ServerAddress::parse("https://chat.example.org").unwrap();
        let another = ServerAddress::parse("https://other.example.org").unwrap();
        let store = Store::new(&config, &primary, "person").unwrap();
        let draft = Draft {
            theme: Theme {
                id: Some("t-existing".into()),
                updated_at: Some("original-revision".into()),
                css: Some(":root { --accent: #40c0ff }".into()),
                ..Default::default()
            },
            mode: Mode::Light,
            new_theme: false,
        };
        store.put(draft.clone()).unwrap();
        assert!(Store::new(&config, &primary, "").is_none());
        assert!(
            Store::new(&config, &primary, "other")
                .unwrap()
                .get()
                .is_none()
        );
        assert!(
            Store::new(&config, &another, "person")
                .unwrap()
                .get()
                .is_none()
        );

        let reopened = Store::new(&config, &primary, "person").unwrap();
        assert_eq!(reopened.get(), Some(draft.clone()));
        reopened.remove("t-different").unwrap();
        assert_eq!(reopened.get(), Some(draft));
        reopened.remove("t-existing").unwrap();
        assert!(reopened.get().is_none());
    }
}

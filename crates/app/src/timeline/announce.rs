//! What a screen reader hears about new messages: one polite live region
//! under the timeline, updated at most every few seconds, as the web client
//! does. While following the newest message a busy channel becomes "4 new
//! messages, latest from jules: …"; scrolled up, only the count is read.
//!
//! Only messages that arrive after the conversation's history is on screen
//! count: loaded history, older pages and the switch to another
//! conversation are not news. Your own messages, events and people you
//! ignore are left out. The rows themselves stay silent, so nothing is read
//! twice.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use ircord_client::contract::MessageKind;
use ircord_client::store::{Workspace, people};

/// The least time between two announcements.
pub const PACE: Duration = Duration::from_secs(4);
/// How much of the latest message is read.
const TEXT_CHARS: usize = 140;

/// One message of the conversation, in timeline order.
#[derive(Clone, Debug, PartialEq)]
pub struct Seen {
    pub id: String,
    /// It would be read: not yours, not an event, not from someone ignored.
    pub heard: bool,
    pub who: String,
    pub text: String,
}

/// The conversation's messages as [`Announcer::observe`] wants them.
pub fn seen(w: &Workspace, conversation: &str) -> Vec<Seen> {
    let Some(timeline) = w.timelines.get(conversation) else {
        return Vec::new();
    };
    let network = w
        .conversation(conversation)
        .map(|c| c.network_id.clone())
        .unwrap_or_default();
    let ignore = people::ignore_list(w).unwrap_or_default();
    timeline
        .entries
        .iter()
        .map(|e| {
            let m = &e.message;
            let member = w.member(&m.sender_id, &network);
            let heard = !w.is_self(&m.sender_id, &network)
                && !matches!(m.kind, MessageKind::Event | MessageKind::System)
                && m.deleted != Some(true)
                && !member.is_some_and(|member| people::is_ignored(&ignore, member));
            Seen {
                id: m.id.clone(),
                heard,
                who: member.map(|m| m.nick.clone()).unwrap_or_default(),
                text: ircord_client::format::parse(&m.text).text,
            }
        })
        .collect()
}

/// What to say, built from the catalog by the caller.
#[derive(Clone, Debug, PartialEq)]
pub enum Say {
    /// One new message while following: who and what.
    Latest { who: String, text: String },
    /// Several while following: how many, and the latest.
    Several {
        count: usize,
        who: String,
        text: String,
    },
    /// Scrolled up: only how many.
    Count(usize),
}

#[derive(Default)]
pub struct Announcer {
    conversation: Option<String>,
    known: HashSet<String>,
    /// New messages not yet announced.
    pending: usize,
    /// Whether the reader follows the newest message.
    following: bool,
    /// The latest heard message.
    latest: Option<(String, String)>,
    last: Option<Instant>,
}

impl Announcer {
    /// Take in the conversation's messages after a change. Returns true when
    /// something new waits to be announced.
    pub fn observe(
        &mut self,
        conversation: Option<&str>,
        messages: &[Seen],
        following: bool,
    ) -> bool {
        if conversation != self.conversation.as_deref() {
            self.conversation = conversation.map(str::to_string);
            self.known = messages.iter().map(|m| m.id.clone()).collect();
            self.pending = 0;
            self.latest = None;
            return false;
        }
        // History is read before the first known message, or replaces an
        // empty list: neither is news.
        let first = messages.iter().position(|m| self.known.contains(&m.id));
        let added: Vec<&Seen> = match first {
            Some(first) => messages[first..]
                .iter()
                .filter(|m| !self.known.contains(&m.id) && m.heard)
                .collect(),
            None => Vec::new(),
        };
        self.known = messages.iter().map(|m| m.id.clone()).collect();
        if added.is_empty() {
            return false;
        }
        self.pending += added.len();
        self.following = following;
        if let Some(last) = messages.iter().rev().find(|m| m.heard) {
            self.latest = Some((last.who.clone(), last.text.clone()));
        }
        true
    }

    /// How long until the next announcement may be made; zero when now.
    pub fn wait(&self, now: Instant) -> Duration {
        self.last
            .map(|last| (last + PACE).saturating_duration_since(now))
            .unwrap_or_default()
    }

    /// The announcement due at `now`, if one waits and the pace allows it.
    pub fn take(&mut self, now: Instant) -> Option<Say> {
        if self.pending == 0 || !self.wait(now).is_zero() {
            return None;
        }
        let count = std::mem::take(&mut self.pending);
        self.last = Some(now);
        let (who, text) = self.latest.clone().unwrap_or_default();
        let text: String = text.chars().take(TEXT_CHARS).collect();
        Some(if !self.following {
            Say::Count(count)
        } else if count == 1 {
            Say::Latest { who, text }
        } else {
            Say::Several { count, who, text }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(id: &str, heard: bool) -> Seen {
        Seen {
            id: id.into(),
            heard,
            who: "bob".into(),
            text: format!("text {id}"),
        }
    }

    #[test]
    fn history_and_switching_are_not_news() {
        let mut a = Announcer::default();
        assert!(!a.observe(Some("c"), &[m("1", true), m("2", true)], true));
        // Older history arrives above.
        assert!(!a.observe(Some("c"), &[m("0", true), m("1", true), m("2", true)], true));
        // Another conversation.
        assert!(!a.observe(Some("d"), &[m("9", true)], true));
        assert_eq!(a.take(Instant::now()), None);
    }

    #[test]
    fn one_new_message_is_read_with_its_sender() {
        let mut a = Announcer::default();
        a.observe(Some("c"), &[m("1", true)], true);
        assert!(a.observe(Some("c"), &[m("1", true), m("2", true)], true));
        assert_eq!(
            a.take(Instant::now()),
            Some(Say::Latest {
                who: "bob".into(),
                text: "text 2".into()
            })
        );
    }

    #[test]
    fn a_burst_is_one_announcement_within_the_pace() {
        let mut a = Announcer::default();
        let start = Instant::now();
        a.observe(Some("c"), &[m("1", true)], true);
        a.observe(Some("c"), &[m("1", true), m("2", true)], true);
        assert!(a.take(start).is_some());
        a.observe(Some("c"), &[m("1", true), m("2", true), m("3", true)], true);
        a.observe(
            Some("c"),
            &[m("1", true), m("2", true), m("3", true), m("4", true)],
            true,
        );
        // Too soon: waits for the pace.
        assert_eq!(a.take(start + Duration::from_secs(1)), None);
        assert_eq!(
            a.wait(start + Duration::from_secs(1)),
            Duration::from_secs(3)
        );
        assert_eq!(
            a.take(start + PACE),
            Some(Say::Several {
                count: 2,
                who: "bob".into(),
                text: "text 4".into()
            })
        );
    }

    #[test]
    fn own_messages_and_events_are_not_read_and_scrolled_up_reads_the_count() {
        let mut a = Announcer::default();
        a.observe(Some("c"), &[m("1", true)], true);
        assert!(!a.observe(Some("c"), &[m("1", true), m("2", false)], true));
        assert!(a.observe(
            Some("c"),
            &[m("1", true), m("2", false), m("3", true)],
            false
        ));
        assert_eq!(a.take(Instant::now()), Some(Say::Count(1)));
    }
}

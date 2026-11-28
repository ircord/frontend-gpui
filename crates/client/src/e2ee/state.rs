//! How a direct conversation's encryption follows the control events a
//! device reads. A device reads them again from its vault on every start,
//! and a device that was offline reads them long after they were sent, so
//! the rules give the same state in any reading order. The web client keeps
//! the same rules (`shared/e2ee/state.ts` in ircord/frontend).
//!
//! Each "turn on" makes a new group, so one group is turned on once and off
//! at most once. An "off" from the conversation's current group is final,
//! however late it is read: a group that was turned off never comes back.
//! Between different groups, the newer "on" wins by time.

use super::vault::Conversation;

/// What a control event in `group` does to the conversation's record:
/// `Some(on)` when it turns encryption on or off, `None` when it changes
/// nothing.
pub fn control_change(
    current: Option<&Conversation>,
    group: &str,
    on: bool,
    at: i64,
) -> Option<bool> {
    let same = current.is_some_and(|c| c.group.as_deref() == Some(group));
    if !on {
        return (same && current.is_some_and(|c| c.on)).then_some(false);
    }
    // The current group is on already, or was turned off and stays off.
    if same {
        return None;
    }
    // An older group's "on" read again after a newer choice changes nothing.
    if current.is_some_and(|c| c.since > at) {
        return None;
    }
    Some(true)
}

/// Whether a Welcome may bind the conversation to its group on the
/// server: not while this device has the conversation turned off. Then the
/// group's "on" decides, once it is read, so the server never refuses
/// plaintext from a side that shows encryption as off.
pub fn welcome_binds(current: Option<&Conversation>) -> bool {
    current.is_none_or(|c| c.on)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(group: &str, on: bool, since: i64) -> Conversation {
        Conversation {
            conversation: "dm".into(),
            peer: "alice".into(),
            group: Some(group.into()),
            on,
            since,
            by: "alice".into(),
        }
    }

    /// Applies (group, on, at) events in the order a device reads them.
    fn replay(start: Option<Conversation>, events: &[(&str, bool, i64)]) -> Option<Conversation> {
        let mut current = start;
        for &(group, on, at) in events {
            match control_change(current.as_ref(), group, on, at) {
                Some(true) => current = Some(record(group, true, at)),
                Some(false) => {
                    let c = current.as_mut().unwrap();
                    c.on = false;
                    c.since = at;
                }
                None => {}
            }
        }
        current
    }

    #[test]
    fn on_off_and_on_again_in_a_new_group() {
        let events = [("g1", true, 100), ("g1", false, 120), ("g2", true, 130)];
        assert_eq!(replay(None, &events), Some(record("g2", true, 130)));
    }

    #[test]
    fn an_off_read_after_a_later_join_still_turns_it_off() {
        // The device was offline: it joined g1 and stamped its record late.
        let joined = record("g1", true, 5_000);
        let after = replay(Some(joined), &[("g1", true, 100), ("g1", false, 120)]);
        assert!(!after.unwrap().on);
    }

    #[test]
    fn history_read_again_does_not_turn_a_group_back_on() {
        let off = record("g1", false, 120);
        assert_eq!(control_change(Some(&off), "g1", true, 100), None);
        // Even with a clock that put the off before the on.
        let early = record("g1", false, 50);
        assert_eq!(control_change(Some(&early), "g1", true, 100), None);
    }

    #[test]
    fn an_older_group_does_not_replace_or_end_a_newer_one() {
        let current = record("g2", true, 130);
        assert_eq!(control_change(Some(&current), "g1", true, 100), None);
        assert_eq!(control_change(Some(&current), "g1", false, 120), None);
        let off = record("g2", false, 140);
        assert_eq!(control_change(Some(&off), "g1", false, 120), None);
    }

    #[test]
    fn reading_the_whole_history_again_ends_where_it_ended() {
        let events = [
            ("g1", true, 100),
            ("g1", false, 120),
            ("g2", true, 130),
            ("g2", false, 140),
        ];
        assert!(!replay(None, &events).unwrap().on);
        let end = record("g2", false, 140);
        assert_eq!(replay(Some(end.clone()), &events), Some(end));
        let on = record("g2", true, 130);
        assert_eq!(replay(Some(on.clone()), &events[..3]), Some(on));
    }

    #[test]
    fn a_welcome_binds_nothing_while_encryption_is_off() {
        assert!(welcome_binds(None));
        assert!(welcome_binds(Some(&record("g1", true, 90))));
        assert!(!welcome_binds(Some(&record("g0", false, 90))));
        assert!(!welcome_binds(Some(&record("g1", false, 120))));
    }
}

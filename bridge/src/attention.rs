//! When the human last touched an entity, and whether they have seen where it
//! got to. Two facts the rail orders and colours itself by, kept here as a pure
//! domain type so the policy is testable without a store, a clock, or an RPC.
//!
//! **Interaction** is the human acting, never the agent. An agent commit moves
//! `updated_at`; it must not move anything here, or the rail would reshuffle
//! itself while work happens — which is the one thing the rail must not do.
//!
//! **The resume point** is the sort key. It is the most recent interaction that
//! followed a gap of at least [`RESUME_GAP`] — the moment you *picked this up
//! again*, rather than the last time you poked it. Working on something all
//! afternoon leaves its position alone; coming back to something you had left
//! moves it to the bottom of the list. It is maintained in O(1): each stamp
//! either starts a new stretch of work or extends the current one, so no history
//! is kept.
//!
//! **Seen** is versioned against the entity's own state clock, not a bare flag:
//! having read a run on Monday says nothing about the failure it hit on Tuesday.

use serde::{Deserialize, Serialize};
use time::format_description::well_known::Rfc3339;
use time::{Duration, OffsetDateTime};

/// A break long enough to count as putting something down. Eight hours: a night,
/// a weekend, or a full working day away from one thread of work — but not lunch,
/// a meeting, or the gap between two reviews of the same diff.
pub const RESUME_GAP: Duration = Duration::hours(8);

/// The human's relationship with one run or plan.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attention {
    /// The last time the human acted on this entity (RFC3339). `None` = never.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_interaction_at: Option<String>,
    /// The interaction that began the current stretch of work on it (RFC3339) —
    /// the rail's sort key. `None` = never touched.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume_at: Option<String>,
    /// The entity's `state_changed_at` as of the last time the human looked at
    /// it (RFC3339). `None` = never looked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seen_state_at: Option<String>,
    /// How far into the entity's conversation the human has read. Unread is
    /// derived against this: an attention-class item created past it is what
    /// makes an entry unread, so a conversation that only reports progress
    /// leaves the entry alone however much it says.
    ///
    /// 0 = never read anything, which is also what every record written before
    /// this field says.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub last_read_sequence: u64,
}

fn is_zero(sequence: &u64) -> bool {
    *sequence == 0
}

fn parse(at: &str) -> Option<OffsetDateTime> {
    OffsetDateTime::parse(at, &Rfc3339).ok()
}

impl Attention {
    /// Record an interaction at `now`. Starts a new stretch of work when the
    /// previous interaction is at least [`RESUME_GAP`] old (or there was none),
    /// otherwise extends the current one — leaving the sort key untouched.
    ///
    /// A stamp that arrives out of order (a clock step, a replayed action) never
    /// moves `last_interaction_at` backwards, so it cannot manufacture a resume
    /// gap that did not happen.
    pub fn interact(&mut self, now: &str) {
        let resumed = match self.last_interaction_at.as_deref().and_then(parse) {
            Some(previous) => match parse(now) {
                Some(current) => current - previous >= RESUME_GAP,
                None => false,
            },
            None => true,
        };
        if resumed {
            self.resume_at = Some(now.to_string());
        }
        let goes_forward = match (
            self.last_interaction_at.as_deref().and_then(parse),
            parse(now),
        ) {
            (Some(previous), Some(current)) => current >= previous,
            _ => true,
        };
        if goes_forward {
            self.last_interaction_at = Some(now.to_string());
        }
        if self.resume_at.is_none() {
            self.resume_at = self.last_interaction_at.clone();
        }
    }

    /// Record that the human has seen the entity as of `state_changed_at`.
    pub fn see(&mut self, state_changed_at: &str) {
        self.seen_state_at = Some(state_changed_at.to_string());
    }

    /// Record that the human has read the conversation through `sequence`.
    ///
    /// Never rewinds: a stale cursor (a second tab still holding the sequence
    /// it loaded with) would otherwise resurrect a badge the human already
    /// cleared.
    pub fn read_through(&mut self, sequence: u64) {
        self.last_read_sequence = self.last_read_sequence.max(sequence);
    }

    /// Whether the human has seen the entity's current state. An entity that has
    /// moved since they looked is unseen again — that is the whole point of
    /// versioning this instead of keeping a flag.
    pub fn has_seen(&self, state_changed_at: &str) -> bool {
        match (
            self.seen_state_at.as_deref().and_then(parse),
            parse(state_changed_at),
        ) {
            (Some(seen), Some(current)) => seen >= current,
            // An unparseable clock on either side is not evidence of having seen
            // it; unseen is the safe answer, since it only means "look again".
            _ => false,
        }
    }

    /// The rail's sort key: when this stretch of work began, falling back to
    /// `created_at` for something never touched, so an untouched entity sorts by
    /// its own age rather than jumping to either end.
    pub fn sort_key(&self, created_at: &str) -> String {
        self.resume_at
            .clone()
            .unwrap_or_else(|| created_at.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MON_09: &str = "2026-07-27T09:00:00Z";
    const MON_09_05: &str = "2026-07-27T09:05:00Z";
    const MON_17: &str = "2026-07-27T17:00:00Z";
    const TUE_09: &str = "2026-07-28T09:00:00Z";

    #[test]
    fn a_first_interaction_starts_a_stretch() {
        let mut attention = Attention::default();
        attention.interact(MON_09);
        assert_eq!(attention.resume_at.as_deref(), Some(MON_09));
        assert_eq!(attention.last_interaction_at.as_deref(), Some(MON_09));
    }

    /// The rule the whole rail rests on: a day of work on one thing must not move
    /// it, or the list reshuffles under the hands of the person using it.
    #[test]
    fn working_on_something_all_day_does_not_move_its_sort_key() {
        let mut attention = Attention::default();
        attention.interact(MON_09);
        attention.interact(MON_09_05);
        attention.interact(MON_17); // 8h after 09:00, but only 7h55 after 09:05
        assert_eq!(attention.resume_at.as_deref(), Some(MON_09));
        assert_eq!(attention.last_interaction_at.as_deref(), Some(MON_17));
    }

    #[test]
    fn picking_something_up_after_a_break_moves_it() {
        let mut attention = Attention::default();
        attention.interact(MON_09);
        attention.interact(TUE_09); // 24h later
        assert_eq!(attention.resume_at.as_deref(), Some(TUE_09));
    }

    #[test]
    fn the_gap_is_measured_from_the_last_interaction_not_the_stretch_start() {
        let mut attention = Attention::default();
        attention.interact(MON_09);
        // A chain of touches, none of them 8h apart: still one stretch.
        for at in [
            "2026-07-27T16:00:00Z",
            "2026-07-27T23:00:00Z",
            "2026-07-28T06:00:00Z",
        ] {
            attention.interact(at);
        }
        assert_eq!(attention.resume_at.as_deref(), Some(MON_09));
    }

    #[test]
    fn exactly_the_gap_counts_as_a_resume() {
        let mut attention = Attention::default();
        attention.interact(MON_09);
        attention.interact(MON_17); // exactly 8h
        assert_eq!(attention.resume_at.as_deref(), Some(MON_17));
    }

    #[test]
    fn an_out_of_order_stamp_never_rewinds_the_clock() {
        let mut attention = Attention::default();
        attention.interact(TUE_09);
        attention.interact(MON_09); // a clock step or a replayed action
        assert_eq!(attention.last_interaction_at.as_deref(), Some(TUE_09));
        assert_eq!(attention.resume_at.as_deref(), Some(TUE_09));
    }

    #[test]
    fn seeing_is_versioned_against_the_state_clock() {
        let mut attention = Attention::default();
        assert!(!attention.has_seen(MON_09), "never looked");
        attention.see(MON_09);
        assert!(attention.has_seen(MON_09));
        // It moved after they looked: unseen again.
        assert!(!attention.has_seen(MON_17));
        attention.see(MON_17);
        assert!(attention.has_seen(MON_17));
    }

    #[test]
    fn an_unreadable_clock_reads_as_unseen() {
        let mut attention = Attention::default();
        attention.see("not a timestamp");
        assert!(!attention.has_seen(MON_09));
        attention.see(MON_09);
        assert!(!attention.has_seen("not a timestamp"));
    }

    #[test]
    fn the_read_cursor_advances_and_never_rewinds() {
        let mut attention = Attention::default();
        assert_eq!(attention.last_read_sequence, 0, "never read anything");
        attention.read_through(7);
        assert_eq!(attention.last_read_sequence, 7);
        attention.read_through(3); // a second tab holding an older cursor
        assert_eq!(attention.last_read_sequence, 7);
        attention.read_through(12);
        assert_eq!(attention.last_read_sequence, 12);
    }

    /// The cursor is new; every record on disk predates it and must still load,
    /// and an entity nobody has read must not pay for the field.
    #[test]
    fn a_record_written_before_the_cursor_existed_reads_as_never_read() {
        let stored = serde_json::json!({ "last_interaction_at": MON_09 });
        let attention: Attention = serde_json::from_value(stored).expect("an old record loads");
        assert_eq!(attention.last_read_sequence, 0);
        let wire = serde_json::to_value(&attention).unwrap();
        assert!(wire.get("last_read_sequence").is_none(), "{wire:?}");
    }

    #[test]
    fn an_untouched_entity_sorts_by_its_own_age() {
        let attention = Attention::default();
        assert_eq!(attention.sort_key(MON_09), MON_09);
    }

    #[test]
    fn a_touched_entity_sorts_by_its_resume_point() {
        let mut attention = Attention::default();
        attention.interact(MON_09);
        attention.interact(MON_09_05);
        assert_eq!(attention.sort_key("2020-01-01T00:00:00Z"), MON_09);
    }
}

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
//!
//! **The anchor** is the inbox's sort key, and it is deliberately not the resume
//! point above. The rail orders by what the human last touched; the inbox orders
//! by when each thing was *taken on*, oldest first, so a list read top to bottom
//! is a list read in the order it arrived. Only one thing moves an anchor: the
//! user saying something after [`ANCHOR_GAP`] of saying nothing — picking the
//! work back up. Agents never move it, reading never moves it, and neither does
//! the work itself.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use time::format_description::well_known::Rfc3339;
use time::{Duration, OffsetDateTime};

/// A break long enough to count as putting something down. Eight hours: a night,
/// a weekend, or a full working day away from one thread of work — but not lunch,
/// a meeting, or the gap between two reviews of the same diff.
pub const RESUME_GAP: Duration = Duration::hours(8);

/// How long the user must have said nothing about a thing before the next thing
/// they say counts as picking it up again — and moves its anchor to the bottom
/// of the inbox. Twelve hours: a night away, or a morning and an afternoon, but
/// never two messages in one sitting.
pub const ANCHOR_GAP: Duration = Duration::hours(12);

/// What every attention key for a row with no entity behind it starts with.
/// Nothing else in the map can collide with it: entity ids are minted with
/// their own prefixes (`run-`, `plan-`, `wt-`).
const ROW_KEY_PREFIX: &str = "row:";

/// The attention key for a branch row that no entity stands behind — a branch
/// checked out somewhere Build never cut, which has no run and no id of its
/// own to be cleared by.
///
/// Keyed on what the row IS: its project and its branch. That outlives the
/// checkout the row was read off, and it is the same identity the feed folds
/// on, so a dismissal written here is about the row the human was looking at
/// rather than about a path that happened to hold it.
pub fn branch_row_key(project_id: &str, branch: &str) -> String {
    format!("{ROW_KEY_PREFIX}{project_id}:branch:{branch}")
}

/// The attention key for a project's primary-checkout row.
///
/// The project IS the row, so it is keyed on nothing narrower: the checkout
/// moving to another branch is the project doing something new, not a
/// different row appearing.
pub fn primary_row_key(project_id: &str) -> String {
    format!("{ROW_KEY_PREFIX}{project_id}:primary")
}

/// What a row key names, read back off the key — which is all the pruner has
/// to decide whether the row it belongs to can still exist.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowKey<'a> {
    Branch {
        project_id: &'a str,
        branch: &'a str,
    },
    Primary {
        project_id: &'a str,
    },
}

impl<'a> RowKey<'a> {
    /// The row a key names, or `None` for a key that names an entity instead.
    ///
    /// Neither a project id (`proj-N`) nor a git branch name can hold a `:`, so
    /// the first one after the prefix ends the project and the second ends the
    /// shape.
    pub fn parse(key: &'a str) -> Option<Self> {
        let (project_id, rest) = key.strip_prefix(ROW_KEY_PREFIX)?.split_once(':')?;
        match rest.split_once(':') {
            Some(("branch", branch)) if !branch.is_empty() => {
                Some(RowKey::Branch { project_id, branch })
            }
            None if rest == "primary" => Some(RowKey::Primary { project_id }),
            _ => None,
        }
    }

    pub fn project_id(&self) -> &'a str {
        match self {
            RowKey::Branch { project_id, .. } | RowKey::Primary { project_id } => project_id,
        }
    }
}

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
    /// How far into the ENTITY's conversation the human had read, before
    /// conversations belonged to agents. Folded onto the entity's first agent
    /// by [`adopt_legacy_cursor`](Self::adopt_legacy_cursor) and zero
    /// thereafter.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub last_read_sequence: u64,
    /// How far into each AGENT's conversation the human has read. Unread is
    /// derived against these: an attention-class item created past an agent's
    /// cursor is what makes its bubble — and the entry above it — unread, so a
    /// conversation that only reports progress leaves both alone however much
    /// it says.
    ///
    /// A missing agent = never read anything of theirs, which is what a record
    /// written before agents existed says about every agent but the first.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub agent_read_sequences: HashMap<String, u64>,
    /// Whether the human has told this entry to stop asking. A muted entry
    /// keeps its place in the inbox with live status, and pushes no badge and
    /// no notification.
    #[serde(default, skip_serializing_if = "is_false")]
    pub muted: bool,
    /// How far into the ENTITY's conversation the human had told the inbox to
    /// stop showing the row, before dismissal belonged to agents. Read as the
    /// first agent's line by
    /// [`dismissed_line_for`](Self::dismissed_line_for) and written by nothing:
    /// records on disk carry it, and it is the only thing that keeps a row
    /// cleared before this existed cleared now. 0 = never dismissed.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub dismissed_through: u64,
    /// How far into each AGENT's conversation the human has told the inbox to
    /// stop showing the row. The row stays out of the list until something
    /// attention-class arrives past one of these lines — which is why there is
    /// no un-dismiss: the work speaking again is what brings it back.
    ///
    /// Per agent because sequences are: every agent numbers its own
    /// conversation from 1, so one number could never say where the human had
    /// got to in another's. A missing agent = never dismissed for that agent,
    /// the same rule the read cursor above follows — and an agent that had
    /// nothing to say when the row was cleared is recorded at 0 rather than
    /// left out, because clearing the row IS drawing its line.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub agent_dismissed_through: HashMap<String, u64>,
    /// The commit a row with no conversation was sitting on when the human
    /// cleared it. A bare checkout and a project's primary checkout hold no
    /// conversation to draw a line in, so their line is drawn in git instead:
    /// the row stays out of the inbox until its HEAD moves. `None` = never
    /// cleared this way; `Some("")` = cleared while its history could not be
    /// read at all.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dismissed_at_head: Option<String>,
    /// Where this entity sits in the inbox (RFC3339): the moment the user took
    /// it on. Seeded at creation and moved only by
    /// [`note_user_message`](Self::note_user_message). `None` = never seeded,
    /// which reads as the entity's creation time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor_at: Option<String>,
    /// When the user last said something about this entity (RFC3339) — what the
    /// next message's silence is measured against. Seeded to the creation time,
    /// because creating a thing IS the first thing the user said about it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_user_message_at: Option<String>,
}

fn is_zero(sequence: &u64) -> bool {
    *sequence == 0
}

fn is_false(muted: &bool) -> bool {
    !*muted
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

    /// Record that the human has read one agent's conversation through
    /// `sequence`.
    ///
    /// Never rewinds: a stale cursor (a second tab still holding the sequence
    /// it loaded with) would otherwise resurrect a badge the human already
    /// cleared.
    pub fn read_through(&mut self, agent_id: &str, sequence: u64) {
        let cursor = self
            .agent_read_sequences
            .entry(agent_id.to_string())
            .or_default();
        *cursor = (*cursor).max(sequence);
    }

    /// How far the human has read one agent's conversation. 0 = never.
    pub fn cursor_for(&self, agent_id: &str) -> u64 {
        self.agent_read_sequences
            .get(agent_id)
            .copied()
            .unwrap_or(0)
    }

    /// Fold the pre-agent, entity-wide cursor onto the entity's first agent —
    /// the agent that inherited that conversation.
    ///
    /// Idempotent: the legacy value is consumed, so a second call moves
    /// nothing. It is never applied to any other agent, whose conversations
    /// began after the human last read anything.
    pub fn adopt_legacy_cursor(&mut self, first_agent_id: &str) {
        let legacy = std::mem::take(&mut self.last_read_sequence);
        if legacy > 0 {
            self.read_through(first_agent_id, legacy);
        }
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

    /// Seed the anchor at the entity's creation, which counts as the first
    /// thing the user said about it.
    ///
    /// Idempotent, and it is the boot migration's entry point as much as
    /// creation's: an anchored record is never re-seeded, so a restart cannot
    /// throw away where the inbox had put something.
    pub fn seed_anchor(&mut self, created_at: &str) {
        if self.anchor_at.is_none() {
            self.anchor_at = Some(created_at.to_string());
        }
        if self.last_user_message_at.is_none() {
            self.last_user_message_at = Some(created_at.to_string());
        }
    }

    /// Take a capture's anchor as this entity's own.
    ///
    /// What the user said and the work it became are one thing in the inbox, so
    /// the work keeps the capture's place in the list instead of arriving at
    /// the bottom of it as something new. Overwrites: routing follows creation
    /// by seconds, and the capture's anchor is the older, truer one.
    pub fn inherit_anchor(&mut self, anchor_at: &str, last_user_message_at: Option<&str>) {
        self.anchor_at = Some(anchor_at.to_string());
        self.last_user_message_at = Some(last_user_message_at.unwrap_or(anchor_at).to_string());
    }

    /// Record that the USER said something about this entity at `now`, and move
    /// the anchor if that message follows [`ANCHOR_GAP`] of their silence.
    ///
    /// Only user messages reach here. An agent talking all night is the work
    /// happening, and the work happening must never reorder the inbox.
    ///
    /// A stamp that arrives out of order (a clock step, a replayed action)
    /// never rewinds the silence clock, so it cannot manufacture a gap that did
    /// not happen — the same rule [`interact`](Self::interact) follows.
    pub fn note_user_message(&mut self, now: &str) {
        let picked_up_again = match self.last_user_message_at.as_deref().and_then(parse) {
            Some(previous) => match parse(now) {
                Some(current) => current - previous >= ANCHOR_GAP,
                // An unreadable clock is not evidence of a gap.
                None => false,
            },
            // Nothing said before: this message is where the entity's stretch
            // starts. Only an unseeded record gets here.
            None => true,
        };
        if picked_up_again || self.anchor_at.is_none() {
            self.anchor_at = Some(now.to_string());
        }
        let goes_forward = match (
            self.last_user_message_at.as_deref().and_then(parse),
            parse(now),
        ) {
            (Some(previous), Some(current)) => current >= previous,
            _ => true,
        };
        if goes_forward {
            self.last_user_message_at = Some(now.to_string());
        }
    }

    /// Take this row out of the inbox until ONE agent's conversation gets past
    /// `last_attention_sequence` — where that agent had got to when the human
    /// cleared the row. Every agent on the entity gets its own line, and the
    /// row is out of the list only while all of them hold.
    ///
    /// Never rewinds, for the same reason the read cursor does not: a second
    /// tab dismissing with the sequence it loaded with would otherwise put back
    /// a row the human had already cleared past.
    pub fn dismiss_agent_through(&mut self, agent_id: &str, last_attention_sequence: u64) {
        let line = self
            .agent_dismissed_through
            .entry(agent_id.to_string())
            .or_default();
        *line = (*line).max(last_attention_sequence);
    }

    /// The line one agent's conversation was cleared through, or `None` for an
    /// agent nobody has cleared — which is what a record written before agents
    /// existed says about every agent but the first.
    ///
    /// `first_agent` folds the pre-agent, entity-wide line onto the agent that
    /// inherited that conversation, exactly as the read cursor does, and never
    /// onto any other: their conversations have sequence spaces that scalar
    /// never saw. The two are maxed rather than replaced, so neither can rewind
    /// the other.
    pub fn dismissed_line_for(&self, agent_id: &str, first_agent: bool) -> Option<u64> {
        let inherited =
            (first_agent && self.dismissed_through > 0).then_some(self.dismissed_through);
        match (
            self.agent_dismissed_through.get(agent_id).copied(),
            inherited,
        ) {
            (Some(line), Some(legacy)) => Some(line.max(legacy)),
            (line, legacy) => line.or(legacy),
        }
    }

    /// Whether one agent's conversation is still cleared: the human dismissed
    /// the row and this agent has not needed them since.
    /// `latest_attention_sequence` is 0 for a conversation that has never asked
    /// for anything.
    ///
    /// An agent nobody cleared says no, whatever its conversation holds —
    /// "never dismissed" must not read as "dismissed through the beginning of
    /// time".
    pub fn is_dismissed_for(
        &self,
        agent_id: &str,
        first_agent: bool,
        latest_attention_sequence: u64,
    ) -> bool {
        self.dismissed_line_for(agent_id, first_agent)
            .is_some_and(|line| line >= latest_attention_sequence)
    }

    /// Take a row with no conversation out of the inbox until its history moves
    /// past `head` — the commit it is sitting on at this moment.
    ///
    /// A row whose HEAD cannot be read is still cleared: an empty line is a
    /// line, and the first commit anyone can read brings the row back.
    pub fn dismiss_at_head(&mut self, head: Option<&str>) {
        self.dismissed_at_head = Some(head.unwrap_or_default().to_string());
    }

    /// Whether that row is still cleared: the human dismissed it, and it is
    /// sitting on the commit they left it on. A new commit brings it back on
    /// its own — the same rule [`is_dismissed_for`](Self::is_dismissed_for)
    /// applies to a conversation, said in the only language a bare checkout
    /// speaks.
    pub fn is_dismissed_at_head(&self, head: Option<&str>) -> bool {
        self.dismissed_at_head.as_deref() == Some(head.unwrap_or_default())
    }

    /// The inbox's sort key: the anchor, falling back to `created_at` for a
    /// record written before anchors existed and not yet seeded.
    pub fn anchor(&self, created_at: &str) -> String {
        self.anchor_at
            .clone()
            .unwrap_or_else(|| created_at.to_string())
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
        assert_eq!(attention.cursor_for("agent-one"), 0, "never read anything");
        attention.read_through("agent-one", 7);
        assert_eq!(attention.cursor_for("agent-one"), 7);
        attention.read_through("agent-one", 3); // a second tab, older cursor
        assert_eq!(attention.cursor_for("agent-one"), 7);
        attention.read_through("agent-one", 12);
        assert_eq!(attention.cursor_for("agent-one"), 12);
    }

    /// Each agent's badge clears on its own. Reading one conversation through
    /// must not claim the human has read another.
    #[test]
    fn each_agent_carries_its_own_cursor() {
        let mut attention = Attention::default();
        attention.read_through("agent-one", 9);
        assert_eq!(attention.cursor_for("agent-one"), 9);
        assert_eq!(attention.cursor_for("agent-two"), 0);
    }

    /// The pre-agent cursor belongs to the agent that inherited the entity's
    /// conversation — and to no one else.
    #[test]
    fn the_pre_agent_cursor_folds_onto_the_first_agent_once() {
        let stored = serde_json::json!({ "last_read_sequence": 5 });
        let mut attention: Attention =
            serde_json::from_value(stored).expect("a pre-agent record loads");
        attention.adopt_legacy_cursor("agent-first");
        assert_eq!(attention.cursor_for("agent-first"), 5);
        assert_eq!(attention.last_read_sequence, 0, "consumed");

        attention.read_through("agent-first", 11);
        attention.adopt_legacy_cursor("agent-first");
        assert_eq!(
            attention.cursor_for("agent-first"),
            11,
            "a second fold moves nothing"
        );
        assert_eq!(attention.cursor_for("agent-second"), 0);
    }

    /// The cursor is new; every record on disk predates it and must still load,
    /// and an entity nobody has read must not pay for the field.
    #[test]
    fn a_record_written_before_the_cursor_existed_reads_as_never_read() {
        let stored = serde_json::json!({ "last_interaction_at": MON_09 });
        let attention: Attention = serde_json::from_value(stored).expect("an old record loads");
        assert_eq!(attention.cursor_for("agent-anything"), 0);
        let wire = serde_json::to_value(&attention).unwrap();
        assert!(wire.get("last_read_sequence").is_none(), "{wire:?}");
        assert!(wire.get("agent_read_sequences").is_none(), "{wire:?}");
    }

    /// Mute is a switch over the record, not a second read cursor. Silencing an
    /// entry and un-silencing it must leave what was waiting exactly where it
    /// was, or "stop asking" would quietly mean "mark it all read".
    #[test]
    fn muting_says_nothing_about_what_has_been_read() {
        let mut attention = Attention::default();
        attention.read_through("agent-one", 4);
        assert!(!attention.muted, "an entry asks until it is told not to");

        attention.muted = true;
        assert_eq!(attention.cursor_for("agent-one"), 4);
        attention.muted = false;
        assert_eq!(attention.cursor_for("agent-one"), 4);
    }

    /// Muted is new and rare: every record on disk predates it, and an entry
    /// nobody silenced must not pay for the field.
    #[test]
    fn a_record_written_before_mute_existed_reads_as_unmuted() {
        let stored = serde_json::json!({ "last_interaction_at": MON_09 });
        let attention: Attention = serde_json::from_value(stored).expect("an old record loads");
        assert!(!attention.muted);
        let wire = serde_json::to_value(&attention).unwrap();
        assert!(wire.get("muted").is_none(), "{wire:?}");

        let silenced = Attention {
            muted: true,
            ..attention
        };
        let wire = serde_json::to_value(&silenced).unwrap();
        assert_eq!(wire["muted"], true, "{wire:?}");
        let reloaded: Attention = serde_json::from_value(wire).expect("a muted record loads");
        assert!(reloaded.muted);
    }

    // ================== Dismissal ==================

    /// Whether the agent under test is the roster's first — the one that
    /// inherited the entity's own conversation, and the only one the pre-agent
    /// line folds onto.
    const FIRST: bool = true;

    /// The feature in one test: a dismissed row is gone until the work speaks
    /// past the line it was dismissed at, and then it is back on its own.
    #[test]
    fn a_dismissed_row_comes_back_when_the_conversation_passes_the_line() {
        let mut attention = Attention::default();
        assert!(
            !attention.is_dismissed_for("agent-one", FIRST, 7),
            "nobody dismissed it"
        );

        attention.dismiss_agent_through("agent-one", 7);
        assert!(
            attention.is_dismissed_for("agent-one", FIRST, 7),
            "nothing has arrived since"
        );
        assert!(
            !attention.is_dismissed_for("agent-one", FIRST, 8),
            "an attention item past the line brings the row back"
        );

        attention.dismiss_agent_through("agent-one", 8);
        assert!(
            attention.is_dismissed_for("agent-one", FIRST, 8),
            "dismissed again, past the new one"
        );
    }

    /// A conversation that has never needed the human has no attention item to
    /// measure against, and a dismissal there still hides the row.
    #[test]
    fn dismissing_a_conversation_that_never_asked_for_anything_hides_it() {
        let mut attention = Attention::default();
        attention.dismiss_agent_through("agent-one", 0);
        assert!(attention.is_dismissed_for("agent-one", FIRST, 0));
    }

    /// A stale dismissal — a second tab acting on the sequence it loaded with —
    /// must not put back a row the human already cleared past.
    #[test]
    fn dismissing_never_rewinds_the_line() {
        let mut attention = Attention::default();
        attention.dismiss_agent_through("agent-one", 12);
        attention.dismiss_agent_through("agent-one", 5);
        assert_eq!(attention.dismissed_line_for("agent-one", FIRST), Some(12));
    }

    /// Each agent numbers its own conversation from 1, so one agent's line says
    /// nothing about another's: an agent nobody has cleared is not cleared,
    /// whatever was done to the agent beside it. This is the whole of the
    /// multi-agent bug — a row cleared off agent one used to stay cleared while
    /// agent two was still talking.
    #[test]
    fn each_agent_carries_its_own_dismissal_line() {
        let mut attention = Attention::default();
        attention.dismiss_agent_through("agent-one", 9);
        assert!(attention.is_dismissed_for("agent-one", FIRST, 9));
        assert_eq!(
            attention.dismissed_line_for("agent-two", !FIRST),
            None,
            "nobody cleared agent two"
        );
        assert!(
            !attention.is_dismissed_for("agent-two", !FIRST, 3),
            "and an agent nobody cleared is on the list"
        );
        assert!(
            !attention.is_dismissed_for("agent-two", !FIRST, 0),
            "even with nothing to say for itself"
        );
    }

    /// The pre-agent line belongs to the agent that inherited the entity's
    /// conversation — and to no one else, exactly like the read cursor it is
    /// modelled on. A row cleared before agents existed stays cleared for its
    /// first agent after this ships.
    #[test]
    fn the_pre_agent_dismissal_folds_onto_the_first_agent_only() {
        let stored = serde_json::json!({ "dismissed_through": 24 });
        let attention: Attention = serde_json::from_value(stored).expect("an old record loads");
        assert_eq!(attention.dismissed_line_for("agent-first", FIRST), Some(24));
        assert!(attention.is_dismissed_for("agent-first", FIRST, 24));
        assert!(
            !attention.is_dismissed_for("agent-first", FIRST, 25),
            "an attention item past the line brings the row back"
        );
        assert_eq!(
            attention.dismissed_line_for("agent-second", !FIRST),
            None,
            "the second agent's own sequences were never in that scalar"
        );
        assert!(!attention.is_dismissed_for("agent-second", !FIRST, 3));
    }

    /// 0 in the legacy scalar means "never dismissed", and must not read as
    /// "dismissed through the beginning of time".
    #[test]
    fn a_zero_legacy_line_is_never_dismissed() {
        let attention = Attention::default();
        assert_eq!(attention.dismissed_line_for("agent-first", FIRST), None);
        assert!(!attention.is_dismissed_for("agent-first", FIRST, 0));
    }

    /// The line the human draws today wins over the one they drew before agents
    /// existed, and neither can rewind the other.
    #[test]
    fn the_per_agent_line_and_the_legacy_line_are_maxed_not_replaced() {
        let mut attention = Attention {
            dismissed_through: 24,
            ..Default::default()
        };
        attention.dismiss_agent_through("agent-first", 7);
        assert_eq!(
            attention.dismissed_line_for("agent-first", FIRST),
            Some(24),
            "the older, higher line still holds"
        );
        attention.dismiss_agent_through("agent-first", 30);
        assert_eq!(attention.dismissed_line_for("agent-first", FIRST), Some(30));
    }

    /// Mute and dismiss are two different things the human can do to one row:
    /// neither implies the other, and neither is a read cursor.
    #[test]
    fn dismissing_says_nothing_about_mute_or_what_has_been_read() {
        let mut attention = Attention::default();
        attention.read_through("agent-one", 4);
        attention.dismiss_agent_through("agent-one", 9);
        assert!(!attention.muted, "dismissing does not silence");
        assert_eq!(
            attention.cursor_for("agent-one"),
            4,
            "nor mark anything read"
        );

        let silenced = Attention {
            muted: true,
            ..Default::default()
        };
        assert!(
            !silenced.is_dismissed_for("agent-one", FIRST, 0),
            "muting does not take the row out of the list"
        );
    }

    /// Dismissal is new: every record on disk predates it, must load, and a row
    /// nobody cleared must not pay for either field.
    #[test]
    fn a_record_written_before_dismissal_existed_reads_as_never_dismissed() {
        let stored = serde_json::json!({ "last_interaction_at": MON_09 });
        let attention: Attention = serde_json::from_value(stored).expect("an old record loads");
        assert_eq!(attention.dismissed_through, 0);
        assert!(!attention.is_dismissed_for("agent-one", FIRST, 0));
        let wire = serde_json::to_value(&attention).unwrap();
        assert!(wire.get("dismissed_through").is_none(), "{wire:?}");
        assert!(wire.get("agent_dismissed_through").is_none(), "{wire:?}");

        // The legacy scalar is still written by nobody and read by everybody:
        // records on disk carry it, and they have to load unchanged.
        let cleared = Attention {
            dismissed_through: 9,
            ..attention
        };
        let wire = serde_json::to_value(&cleared).unwrap();
        assert_eq!(wire["dismissed_through"], 9, "{wire:?}");
        let reloaded: Attention =
            serde_json::from_value(wire).expect("a dismissed record loads back");
        assert_eq!(reloaded, cleared);
    }

    /// A per-agent dismissal round-trips, including the silent agent whose line
    /// is 0 — a record that dropped it would put the row back on every poll.
    #[test]
    fn a_per_agent_dismissal_round_trips() {
        let mut attention = Attention::default();
        attention.dismiss_agent_through("agent-one", 9);
        attention.dismiss_agent_through("agent-two", 0);
        let wire = serde_json::to_value(&attention).unwrap();
        assert_eq!(wire["agent_dismissed_through"]["agent-one"], 9, "{wire:?}");
        assert_eq!(wire["agent_dismissed_through"]["agent-two"], 0, "{wire:?}");
        let reloaded: Attention = serde_json::from_value(wire).expect("it loads back");
        assert_eq!(reloaded, attention);
        assert!(reloaded.is_dismissed_for("agent-two", !FIRST, 0));
    }

    // ================== Dismissing a row with no conversation ==================

    /// The same feature for a row that has no conversation to draw a line in:
    /// it is cleared at the commit it was sitting on, and the next commit
    /// brings it back with nobody having to un-dismiss it.
    #[test]
    fn a_row_with_no_conversation_stays_cleared_until_its_head_moves() {
        let mut attention = Attention::default();
        assert!(
            !attention.is_dismissed_at_head(Some("abc123")),
            "nobody cleared it"
        );

        attention.dismiss_at_head(Some("abc123"));
        assert!(attention.is_dismissed_at_head(Some("abc123")));
        assert!(
            !attention.is_dismissed_at_head(Some("def456")),
            "a commit past the line brings the row back"
        );

        attention.dismiss_at_head(Some("def456"));
        assert!(attention.is_dismissed_at_head(Some("def456")));
    }

    /// A checkout whose history cannot be read is still a row the human can
    /// clear — and the first commit anyone can read is news, so it comes back.
    #[test]
    fn a_row_cleared_with_no_readable_head_comes_back_when_one_appears() {
        let mut attention = Attention::default();
        attention.dismiss_at_head(None);
        assert!(attention.is_dismissed_at_head(None));
        assert!(!attention.is_dismissed_at_head(Some("abc123")));
    }

    /// The two lines are drawn in different places and neither reads as the
    /// other: an entity's conversation cannot clear a checkout, and a commit
    /// cannot clear a run.
    #[test]
    fn the_conversation_line_and_the_commit_line_are_independent() {
        let mut cleared_row = Attention::default();
        cleared_row.dismiss_at_head(Some("abc123"));
        assert!(
            !cleared_row.is_dismissed_for("agent-one", FIRST, 0),
            "no conversation was cleared"
        );

        let mut cleared_entity = Attention::default();
        cleared_entity.dismiss_agent_through("agent-one", 4);
        assert!(
            !cleared_entity.is_dismissed_at_head(Some("abc123")),
            "no row was cleared"
        );
    }

    /// Clearing a row is new: every record on disk predates it, must load, and
    /// a row nobody cleared must not pay for the field.
    #[test]
    fn a_record_written_before_row_dismissal_existed_reads_as_never_cleared() {
        let stored = serde_json::json!({ "last_interaction_at": MON_09 });
        let attention: Attention = serde_json::from_value(stored).expect("an old record loads");
        assert!(attention.dismissed_at_head.is_none());
        assert!(!attention.is_dismissed_at_head(Some("abc123")));
        assert!(
            !attention.is_dismissed_at_head(None),
            "never cleared is not cleared at nothing"
        );
        let wire = serde_json::to_value(&attention).unwrap();
        assert!(wire.get("dismissed_at_head").is_none(), "{wire:?}");

        let mut cleared = attention;
        cleared.dismiss_at_head(Some("abc123"));
        let wire = serde_json::to_value(&cleared).unwrap();
        assert_eq!(wire["dismissed_at_head"], "abc123", "{wire:?}");
        let reloaded: Attention = serde_json::from_value(wire).expect("a cleared row loads back");
        assert_eq!(reloaded, cleared);
    }

    /// Row keys name a row rather than an entity, and every one reads back as
    /// the row it names — which is how the pruner knows what to keep.
    #[test]
    fn a_row_key_reads_back_as_the_row_it_names() {
        assert_eq!(
            RowKey::parse(&branch_row_key("proj-1", "feature/thing")),
            Some(RowKey::Branch {
                project_id: "proj-1",
                branch: "feature/thing",
            })
        );
        assert_eq!(
            RowKey::parse(&primary_row_key("proj-2")),
            Some(RowKey::Primary {
                project_id: "proj-2"
            })
        );
        assert_ne!(
            branch_row_key("proj-1", "main"),
            primary_row_key("proj-1"),
            "a branch called main is not the primary checkout row"
        );
        assert_eq!(
            RowKey::parse(&branch_row_key("proj-1", "main"))
                .expect("a branch key")
                .project_id(),
            "proj-1"
        );
        for entity_id in ["run-1", "plan-1", "wt-abc123", "proj-1", "row:proj-1"] {
            assert_eq!(RowKey::parse(entity_id), None, "{entity_id}");
        }
    }

    // ================== The inbox anchor ==================

    const MON_21: &str = "2026-07-27T21:00:00Z";
    const TUE_08_59: &str = "2026-07-28T08:59:59Z";
    const WED_09: &str = "2026-07-29T09:00:00Z";

    /// Creation is the first thing the user said, so it is where the entity
    /// enters the list.
    #[test]
    fn creation_seeds_the_anchor() {
        let mut attention = Attention::default();
        attention.seed_anchor(MON_09);
        assert_eq!(attention.anchor_at.as_deref(), Some(MON_09));
        assert_eq!(attention.last_user_message_at.as_deref(), Some(MON_09));
        assert_eq!(attention.anchor(WED_09), MON_09);
    }

    /// The boot migration seeds every record it finds; a second boot must not
    /// move anything the first one placed.
    #[test]
    fn seeding_an_anchored_record_moves_nothing() {
        let mut attention = Attention::default();
        attention.seed_anchor(MON_09);
        attention.note_user_message(TUE_09);
        attention.seed_anchor("2020-01-01T00:00:00Z");
        assert_eq!(attention.anchor_at.as_deref(), Some(TUE_09));
        assert_eq!(attention.last_user_message_at.as_deref(), Some(TUE_09));
    }

    /// A conversation held in one sitting keeps its place: this is the whole
    /// point of the gap, and the inbox must not reshuffle while you type.
    #[test]
    fn talking_to_something_all_day_does_not_move_its_anchor() {
        let mut attention = Attention::default();
        attention.seed_anchor(MON_09);
        for at in [MON_09_05, MON_17, MON_21] {
            attention.note_user_message(at);
        }
        assert_eq!(attention.anchor_at.as_deref(), Some(MON_09));
        assert_eq!(attention.last_user_message_at.as_deref(), Some(MON_21));
    }

    /// Exactly twelve hours is a pickup — the boundary is inclusive, and one
    /// second under it is not.
    #[test]
    fn the_twelve_hour_boundary_is_inclusive() {
        let mut just_under = Attention::default();
        just_under.seed_anchor(MON_21);
        just_under.note_user_message(TUE_08_59); // 11h59m59s
        assert_eq!(
            just_under.anchor_at.as_deref(),
            Some(MON_21),
            "a second short of the gap is the same stretch of work"
        );

        let mut exactly = Attention::default();
        exactly.seed_anchor(MON_09);
        exactly.note_user_message(MON_21); // exactly 12h
        assert_eq!(exactly.anchor_at.as_deref(), Some(MON_21));
    }

    /// The gap is measured from the last thing the user SAID, not from the
    /// anchor: a chain of messages twelve hours apart moves it every time.
    #[test]
    fn the_gap_is_measured_from_the_last_user_message() {
        let mut attention = Attention::default();
        attention.seed_anchor(MON_09);
        attention.note_user_message(MON_21);
        assert_eq!(attention.anchor_at.as_deref(), Some(MON_21));
        attention.note_user_message(TUE_09);
        assert_eq!(attention.anchor_at.as_deref(), Some(TUE_09));
    }

    /// Everything the human does that is not saying something — reading,
    /// approving, opening a doc — leaves the anchor where it is. Only
    /// `note_user_message` can move it, and only `interact` moves the rail's
    /// resume point: the two clocks are independent on purpose.
    #[test]
    fn interacting_never_moves_the_anchor() {
        let mut attention = Attention::default();
        attention.seed_anchor(MON_09);
        attention.interact(TUE_09);
        attention.see(TUE_09);
        attention.read_through("agent-one", 12);
        assert_eq!(attention.anchor_at.as_deref(), Some(MON_09));
        assert_eq!(attention.last_user_message_at.as_deref(), Some(MON_09));
        assert_eq!(
            attention.resume_at.as_deref(),
            Some(TUE_09),
            "the rail's own clock still moves"
        );
    }

    #[test]
    fn an_out_of_order_message_never_rewinds_the_silence_clock() {
        let mut attention = Attention::default();
        attention.seed_anchor(MON_09);
        attention.note_user_message(WED_09);
        attention.note_user_message(MON_17); // a clock step or a replayed post
        assert_eq!(attention.last_user_message_at.as_deref(), Some(WED_09));
        assert_eq!(
            attention.anchor_at.as_deref(),
            Some(WED_09),
            "the late-arriving old message must not manufacture a pickup"
        );
    }

    /// A record that predates anchors, whose first user message arrives before
    /// anything seeded it: the message itself starts the stretch rather than
    /// leaving the entity unanchored.
    #[test]
    fn an_unseeded_record_anchors_on_the_first_thing_said_to_it() {
        let mut attention = Attention::default();
        attention.note_user_message(TUE_09);
        assert_eq!(attention.anchor_at.as_deref(), Some(TUE_09));
    }

    /// What the user said and the work it became are one thing in the list.
    #[test]
    fn routing_hands_the_captures_anchor_to_the_work() {
        let mut work = Attention::default();
        work.seed_anchor(WED_09); // the issue was minted just now
        work.inherit_anchor(MON_09, None);
        assert_eq!(work.anchor_at.as_deref(), Some(MON_09));
        assert_eq!(work.last_user_message_at.as_deref(), Some(MON_09));

        // The next message is measured against the capture's clock, not the
        // entity's: an inherited anchor brings the silence with it.
        work.note_user_message(MON_17);
        assert_eq!(work.anchor_at.as_deref(), Some(MON_09));
    }

    /// Anchors are new: every record on disk predates them, must load, and must
    /// not pay for the fields until something seeds them.
    #[test]
    fn a_record_written_before_anchors_existed_loads_unanchored() {
        let stored = serde_json::json!({ "last_interaction_at": MON_09 });
        let attention: Attention = serde_json::from_value(stored).expect("an old record loads");
        assert!(attention.anchor_at.is_none());
        assert_eq!(attention.anchor(MON_17), MON_17, "it reads as its own age");
        let wire = serde_json::to_value(&attention).unwrap();
        assert!(wire.get("anchor_at").is_none(), "{wire:?}");
        assert!(wire.get("last_user_message_at").is_none(), "{wire:?}");
    }

    #[test]
    fn an_anchored_record_round_trips() {
        let mut attention = Attention::default();
        attention.seed_anchor(MON_09);
        attention.note_user_message(TUE_09);
        let wire = serde_json::to_value(&attention).unwrap();
        let reloaded: Attention = serde_json::from_value(wire).expect("it loads");
        assert_eq!(reloaded, attention);
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

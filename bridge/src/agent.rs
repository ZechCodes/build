//! Agents: the thing a conversation belongs to.
//!
//! An agent is a first-class, durable identity — not a harness process. The
//! process dies on every restart and context clear; the agent, its provider
//! choice, its display ordinal and above all its conversation outlive it.
//!
//! One conversation per agent (spec: UX Redesign Decisions, "Agents and
//! conversations"). A branch can carry several agents working the same
//! checkout; an issue carries exactly one. So an entity holds a
//! [`AgentRoster`] rather than a thread, and the roster's FIRST agent is the
//! one every entity-level event still speaks to.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::models::ModelChoice;
use crate::thread::Thread;

/// What every agent id starts with, so an id read off the wire says what kind
/// of thing it names without a lookup.
pub const AGENT_ID_PREFIX: &str = "agent-";

/// Crockford base32 — ULID's alphabet: no I, L, O or U, so an id read aloud or
/// typed by hand cannot become a different id.
const CROCKFORD: [u8; 32] = *b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// The character length of a ULID body (128 bits in base32).
const ULID_LEN: usize = 26;

/// Where an agent is in its life. Lifecycle, never process state: `Live` means
/// a harness is running for it right now, `Idle` that it exists and is not
/// running, `Ended` that it has been retired and will not run again.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentLifecycle {
    Live,
    #[default]
    Idle,
    Ended,
}

impl AgentLifecycle {
    pub fn as_str(self) -> &'static str {
        match self {
            AgentLifecycle::Live => "live",
            AgentLifecycle::Idle => "idle",
            AgentLifecycle::Ended => "ended",
        }
    }
}

/// One agent of one entity: its identity, what it runs on, where it sits in the
/// rail, and the conversation it owns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Agent {
    pub id: String,
    /// The plan/run this agent belongs to. An agent is never free-floating: it
    /// is reachable only through its branch or issue.
    pub owner_id: String,
    #[serde(default)]
    pub choice: ModelChoice,
    /// 1-based display position in the agent rail. Stable for the agent's life:
    /// removing an agent must not renumber the ones beside it.
    pub ordinal: u32,
    pub created_at: String,
    #[serde(default)]
    pub state: AgentLifecycle,
    /// The agent's own conversation. `thread:<agent_id>`, always.
    #[serde(default)]
    pub thread: Thread,
    /// The name the agent's last session gave the conversation it was having,
    /// for a respawn to resume BY NAME rather than by guessing the newest
    /// transcript in the checkout.
    ///
    /// `None` for a carrier that names no conversation, for an agent that has
    /// never run, and for one whose session died before it could announce
    /// itself — all three of which fall back to the transcript probe, which is
    /// what shipped before this and still works. Never an error: this is a
    /// sharpening of a path that already answers.
    #[serde(default)]
    pub resume_session_id: Option<String>,
}

impl Agent {
    /// A brand new agent with an empty conversation keyed to its own id.
    pub fn new(
        id: impl Into<String>,
        owner_id: impl Into<String>,
        choice: ModelChoice,
        ordinal: u32,
        created_at: impl Into<String>,
    ) -> Agent {
        let id = id.into();
        Agent {
            thread: Thread::for_agent(&id),
            id,
            owner_id: owner_id.into(),
            choice,
            ordinal,
            created_at: created_at.into(),
            state: AgentLifecycle::Idle,
            resume_session_id: None,
        }
    }
}

/// An entity's agents, in rail order, guaranteed non-empty.
///
/// The roster dereferences to its first agent's conversation: entity-level
/// events (lifecycle, git) have no agent of their own to speak to, so they
/// speak to the first one — the agent the entity was created with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AgentRoster {
    agents: Vec<Agent>,
}

impl std::ops::Deref for AgentRoster {
    type Target = Thread;

    fn deref(&self) -> &Thread {
        &self.first().thread
    }
}

impl std::ops::DerefMut for AgentRoster {
    fn deref_mut(&mut self) -> &mut Thread {
        &mut self.first_mut().thread
    }
}

impl AgentRoster {
    /// A roster holding one freshly minted agent — what a new entity gets.
    pub fn with_first(owner_id: &str, choice: ModelChoice, now: &str) -> AgentRoster {
        AgentRoster {
            agents: vec![Agent::new(
                derived_agent_id(owner_id),
                owner_id,
                choice,
                1,
                now,
            )],
        }
    }

    /// Rebuild a roster from what was persisted.
    ///
    /// `legacy` is the entity-keyed conversation records written before agents
    /// existed carry. It becomes the first agent's conversation, re-keyed to
    /// that agent — and because the first agent's id is DERIVED from the owner,
    /// running this again on an already-migrated record produces the same
    /// roster rather than a second agent.
    pub fn restore(
        owner_id: &str,
        agents: Vec<Agent>,
        legacy: Thread,
        choice: ModelChoice,
        created_at: &str,
    ) -> AgentRoster {
        if agents.is_empty() {
            let id = derived_agent_id(owner_id);
            let mut thread = legacy;
            thread.rekey_to_agent(&id);
            let mut first = Agent::new(id, owner_id, choice, 1, created_at);
            first.thread = thread;
            return AgentRoster {
                agents: vec![first],
            };
        }
        let mut agents = agents;
        for agent in &mut agents {
            let id = agent.id.clone();
            agent.thread.rekey_to_agent(&id);
        }
        AgentRoster { agents }
    }

    /// The agent every entity-level event speaks to.
    pub fn first(&self) -> &Agent {
        self.agents
            .first()
            .expect("a roster always holds at least one agent")
    }

    pub fn first_mut(&mut self) -> &mut Agent {
        self.agents
            .first_mut()
            .expect("a roster always holds at least one agent")
    }

    pub fn agents(&self) -> &[Agent] {
        &self.agents
    }

    pub fn iter(&self) -> std::slice::Iter<'_, Agent> {
        self.agents.iter()
    }

    pub fn iter_mut(&mut self) -> std::slice::IterMut<'_, Agent> {
        self.agents.iter_mut()
    }

    pub fn len(&self) -> usize {
        self.agents.len()
    }

    /// Never empty — the invariant this whole type exists to hold. Present so
    /// `len()` does not read as a collection that might be.
    pub fn is_empty(&self) -> bool {
        false
    }

    pub fn by_id(&self, agent_id: &str) -> Option<&Agent> {
        self.agents.iter().find(|agent| agent.id == agent_id)
    }

    pub fn by_id_mut(&mut self, agent_id: &str) -> Option<&mut Agent> {
        self.agents.iter_mut().find(|agent| agent.id == agent_id)
    }

    /// Which agent a verb means: the one it named, or the first — so every verb
    /// that predates agents keeps addressing the conversation it always did.
    pub fn resolve(&self, agent_id: Option<&str>) -> Result<&Agent, String> {
        match agent_id.filter(|id| !id.is_empty()) {
            None => Ok(self.first()),
            Some(id) => self
                .by_id(id)
                .ok_or_else(|| format!("unknown agent_id: {id}")),
        }
    }

    pub fn resolve_mut(&mut self, agent_id: Option<&str>) -> Result<&mut Agent, String> {
        match agent_id.filter(|id| !id.is_empty()) {
            None => Ok(self.first_mut()),
            Some(id) => {
                if self.by_id(id).is_none() {
                    return Err(format!("unknown agent_id: {id}"));
                }
                Ok(self.by_id_mut(id).expect("just checked"))
            }
        }
    }

    /// Add an agent to the entity, with its own empty conversation. The ordinal
    /// continues past the highest one ever handed out here, so a rail label is
    /// never reused.
    pub fn add(&mut self, owner_id: &str, choice: ModelChoice, now: &str) -> &Agent {
        let ordinal = self
            .agents
            .iter()
            .map(|agent| agent.ordinal)
            .max()
            .unwrap_or(0)
            + 1;
        self.agents
            .push(Agent::new(new_agent_id(), owner_id, choice, ordinal, now));
        self.agents.last().expect("just pushed")
    }

    /// Take an agent off the entity, handing back the record that was removed
    /// (its conversation included, for the caller to do the last rites on).
    ///
    /// The FIRST agent is never removable. It owns the conversation this roster
    /// dereferences to — the one every entity-level event speaks to — so
    /// removing it would silently re-home that conversation onto an agent that
    /// never heard a word of it. It is also what makes the non-empty invariant
    /// hold without a second rule: the last agent left is always the first.
    ///
    /// The ordinals of the agents beside it are left alone. An ordinal is the
    /// rail's label for an agent, not its position, and a label that shifted
    /// when a neighbour went away would rename a conversation the human is in
    /// the middle of reading.
    pub fn remove(&mut self, agent_id: &str) -> Result<Agent, String> {
        let index = self
            .agents
            .iter()
            .position(|agent| agent.id == agent_id)
            .ok_or_else(|| format!("unknown agent_id: {agent_id}"))?;
        if index == 0 {
            return Err(format!(
                "{agent_id} is the first agent of {} and owns its conversation, so it cannot be \
                 removed — remove the agents added beside it instead",
                self.agents[0].owner_id
            ));
        }
        Ok(self.agents.remove(index))
    }
}

/// A fresh, time-ordered agent id. Two agents minted in the same millisecond
/// still differ (80 bits of randomness), and ids minted later sort later.
pub fn new_agent_id() -> String {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_millis())
        .unwrap_or(0);
    mint_agent_id(now_ms, uuid::Uuid::new_v4().as_u128())
}

/// The ULID rule, factored out so time and randomness can be supplied by a
/// test: 48 bits of milliseconds, then 80 bits of randomness.
pub fn mint_agent_id(now_ms: u128, randomness: u128) -> String {
    let time = (now_ms & ((1u128 << 48) - 1)) << 80;
    let random = randomness & ((1u128 << 80) - 1);
    format!("{AGENT_ID_PREFIX}{}", crockford_base32(time | random))
}

/// The first agent of `owner_id`, derived rather than minted.
///
/// A boot migration has to name the agent it is moving an entity's existing
/// conversation onto, and has to name the SAME one if it runs again. Deriving
/// that id from the owner is what makes re-migration a no-op instead of a
/// second agent holding a second copy of the conversation.
pub fn derived_agent_id(owner_id: &str) -> String {
    let digest = Sha256::digest(format!("build-agent-1:{owner_id}").as_bytes());
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    format!(
        "{AGENT_ID_PREFIX}{}",
        crockford_base32(u128::from_be_bytes(bytes))
    )
}

/// 128 bits as 26 Crockford base32 characters, most significant first.
fn crockford_base32(value: u128) -> String {
    let mut out = [b'0'; ULID_LEN];
    let mut remaining = value;
    for slot in out.iter_mut().rev() {
        *slot = CROCKFORD[(remaining & 0x1f) as usize];
        remaining >>= 5;
    }
    String::from_utf8(out.to_vec()).expect("Crockford digits are ASCII")
}

#[cfg(test)]
mod id_tests {
    use super::*;

    fn body(id: &str) -> &str {
        id.strip_prefix(AGENT_ID_PREFIX)
            .unwrap_or_else(|| panic!("{id} is not an agent id"))
    }

    #[test]
    fn an_agent_id_is_a_prefixed_ulid() {
        let id = new_agent_id();
        let body = body(&id);
        assert_eq!(body.len(), ULID_LEN, "{id}");
        assert!(
            body.bytes().all(|c| CROCKFORD.contains(&c)),
            "{id} uses characters outside Crockford base32"
        );
    }

    /// The rail orders agents by when they were added, and a durable id that
    /// sorts by mint time means that ordering survives without a join.
    #[test]
    fn ids_minted_later_sort_later() {
        let earlier = mint_agent_id(1_760_000_000_000, u128::MAX);
        let later = mint_agent_id(1_760_000_000_001, 0);
        assert!(earlier < later, "{earlier} should sort before {later}");
    }

    #[test]
    fn two_ids_from_the_same_millisecond_still_differ() {
        let one = mint_agent_id(1_760_000_000_000, 1);
        let two = mint_agent_id(1_760_000_000_000, 2);
        assert_ne!(one, two);
    }

    /// The whole point of deriving: the migration can run twice and name the
    /// same agent both times.
    #[test]
    fn a_derived_id_is_stable_for_one_owner_and_unique_across_owners() {
        assert_eq!(derived_agent_id("run-1"), derived_agent_id("run-1"));
        assert_ne!(derived_agent_id("run-1"), derived_agent_id("run-2"));
        assert_eq!(body(&derived_agent_id("run-1")).len(), ULID_LEN);
        assert!(body(&derived_agent_id("run-1"))
            .bytes()
            .all(|c| CROCKFORD.contains(&c)));
    }
}

#[cfg(test)]
mod roster_tests {
    use super::*;
    use crate::models::AgentProvider;

    const NOW: &str = "2026-08-13T09:00:00Z";

    fn codex() -> ModelChoice {
        ModelChoice {
            provider: AgentProvider::Codex,
            model: Some("gpt-5.6-sol".into()),
            effort: Some("high".into()),
        }
    }

    #[test]
    fn the_first_agent_owns_the_entitys_conversation() {
        let roster = AgentRoster::with_first("run-1", ModelChoice::default(), NOW);
        let first = roster.first();
        assert_eq!(first.ordinal, 1);
        assert_eq!(first.owner_id, "run-1");
        assert_eq!(first.state, AgentLifecycle::Idle);
        assert_eq!(first.thread.id, format!("thread:{}", first.id));
        assert_eq!(first.thread.agent.id, first.id);
        // The roster IS that conversation to everything that has no agent of
        // its own to speak to.
        assert_eq!(roster.id, first.thread.id);
    }

    #[test]
    fn a_second_agent_gets_its_own_conversation_and_the_next_ordinal() {
        let mut roster = AgentRoster::with_first("run-1", ModelChoice::default(), NOW);
        let first_id = roster.first().id.clone();
        let added = roster.add("run-1", codex(), "2026-08-13T10:00:00Z").clone();

        assert_eq!(added.ordinal, 2);
        assert_eq!(added.choice, codex());
        assert_ne!(added.id, first_id);
        assert_eq!(added.thread.id, format!("thread:{}", added.id));
        assert_eq!(roster.len(), 2);

        // Two conversations, not one shared one.
        roster
            .by_id_mut(&added.id)
            .expect("the agent is on the roster")
            .thread
            .post_user("only agent two hears this", None, NOW);
        assert_eq!(roster.first().thread.items.len(), 0);
        assert_eq!(roster.by_id(&added.id).unwrap().thread.items.len(), 1);
    }

    #[test]
    fn resolve_defaults_to_the_first_agent_and_rejects_an_unknown_one() {
        let mut roster = AgentRoster::with_first("run-1", ModelChoice::default(), NOW);
        let second = roster.add("run-1", ModelChoice::default(), NOW).id.clone();

        assert_eq!(roster.resolve(None).unwrap().ordinal, 1);
        assert_eq!(roster.resolve(Some("")).unwrap().ordinal, 1);
        assert_eq!(roster.resolve(Some(&second)).unwrap().ordinal, 2);
        assert!(roster
            .resolve(Some("agent-NOPE"))
            .unwrap_err()
            .contains("unknown agent_id"));
    }

    /// The migration contract: an entity's conversation becomes its first
    /// agent's, keeps every item, and re-running the restore changes nothing.
    #[test]
    fn restoring_a_pre_agent_record_adopts_its_conversation_idempotently() {
        let mut legacy = Thread::new("run-1");
        legacy.post_user("carried across", None, NOW);
        legacy.post_agent("and answered", None, NOW);

        let roster = AgentRoster::restore(
            "run-1",
            Vec::new(),
            legacy.clone(),
            ModelChoice::default(),
            NOW,
        );
        let first = roster.first();
        assert_eq!(first.id, derived_agent_id("run-1"));
        assert_eq!(first.thread.id, format!("thread:{}", first.id));
        assert_eq!(first.thread.agent.id, first.id);
        assert_eq!(first.thread.items, legacy.items);

        let again = AgentRoster::restore(
            "run-1",
            roster.agents().to_vec(),
            Thread::default(),
            ModelChoice::default(),
            NOW,
        );
        assert_eq!(again, roster, "a second migration is a no-op");
        assert_eq!(again.len(), 1);
    }

    /// Removing an agent takes it off the rail and leaves every other label
    /// exactly where it was — the ordinal is a name, not a position.
    #[test]
    fn remove_takes_one_agent_off_without_renumbering_the_others() {
        let mut roster = AgentRoster::with_first("run-1", ModelChoice::default(), NOW);
        let first = roster.first().id.clone();
        let second = roster.add("run-1", codex(), NOW).id.clone();
        let third = roster.add("run-1", ModelChoice::default(), NOW).id.clone();

        let removed = roster
            .remove(&second)
            .expect("the second agent is removable");
        assert_eq!(removed.id, second);
        assert_eq!(removed.ordinal, 2);
        assert_eq!(roster.len(), 2);
        assert!(roster.by_id(&second).is_none());
        assert_eq!(roster.first().id, first);
        assert_eq!(roster.by_id(&third).expect("still here").ordinal, 3);

        // The next agent continues past the highest ordinal ever handed out
        // here, so the freed label is never reused.
        assert_eq!(roster.add("run-1", ModelChoice::default(), NOW).ordinal, 4);
    }

    /// The first agent owns the entity's conversation, so it is not removable —
    /// and because the last agent left is always the first, that is also what
    /// keeps a roster from ever being emptied.
    #[test]
    fn the_first_agent_is_not_removable_and_neither_is_the_only_one() {
        let mut roster = AgentRoster::with_first("run-1", ModelChoice::default(), NOW);
        let only = roster.first().id.clone();
        let refused = roster.remove(&only).unwrap_err();
        assert!(refused.contains("conversation"), "{refused}");
        assert_eq!(roster.len(), 1);

        roster.add("run-1", codex(), NOW);
        assert!(roster.remove(&only).is_err(), "still the first agent");
        assert_eq!(roster.len(), 2);
    }

    #[test]
    fn removing_an_agent_that_is_not_on_the_roster_is_an_error() {
        let mut roster = AgentRoster::with_first("run-1", ModelChoice::default(), NOW);
        roster.add("run-1", codex(), NOW);
        assert!(roster
            .remove("agent-NOPE")
            .unwrap_err()
            .contains("unknown agent_id"));
        assert_eq!(roster.len(), 2);
    }

    #[test]
    fn a_roster_round_trips_as_a_plain_array() {
        let mut roster = AgentRoster::with_first("run-1", ModelChoice::default(), NOW);
        roster.add("run-1", codex(), NOW);
        let wire = serde_json::to_value(&roster).expect("a roster serializes");
        assert!(wire.is_array(), "{wire:?}");
        assert_eq!(wire[1]["ordinal"], 2);
        assert_eq!(wire[1]["state"], "idle");
        assert_eq!(
            serde_json::from_value::<AgentRoster>(wire).expect("a roster reloads"),
            roster
        );
    }
}

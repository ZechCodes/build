//! A harness out of usage, as this device knows it (issue #58).
//!
//! The session that hit the limit says so on its own status snapshot (see
//! [`crate::harness::usage_limit`]). That is one agent's account; the limit is
//! not. Every agent on this device using the same harness is out of usage with
//! it, so the device keeps one record per harness, the wire carries it on the
//! board item, and delivery reads it before starting a turn.
//!
//! The record lives from the first turn that stopped at the limit until a turn
//! on that harness RUNS again. Not until its stated reset passes: the harness
//! answering is better evidence than any clock it named. The reset instant
//! only decides when the bridge stops holding turns back and tries again.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

use crate::app::{AppState, PendingAgentTurn, TurnText, NEW_THREAD_MESSAGES_PROMPT};
use crate::harness::usage_limit::UsageLimited;
use crate::models::AgentProvider;

/// An agent, by the entity it belongs to and its own id.
type AgentKey = (String, String);

/// One harness out of usage on this device.
#[derive(Debug, Clone, PartialEq, Eq)]
struct DeviceUsageLimit {
    harness: AgentProvider,
    /// When the device first saw this limit.
    since: OffsetDateTime,
    resets_at: Option<OffsetDateTime>,
    /// The harness's own sentence, shown behind the banner.
    said: String,
    /// Whether turns on this harness may start again. False while a known
    /// reset is still ahead; true once it has passed, and from the start when
    /// the harness named no reset — there is nothing to wait for then, and a
    /// turn that runs is the only way to learn the limit is over.
    released: bool,
    /// Whether a drain has asked to be woken at `resets_at`.
    wake_asked: bool,
    /// The conversations already told a turn is waiting on this limit, so
    /// each is told once.
    told: BTreeSet<AgentKey>,
}

impl DeviceUsageLimit {
    fn render(&self) -> Value {
        json!({
            "harness": self.harness.wire_id(),
            "since": rfc3339(self.since),
            "resets_at": self.resets_at.map(rfc3339),
            "said": self.said,
        })
    }
}

/// An agent whose turn stopped at a limit and has not run since.
#[derive(Debug, Clone, PartialEq, Eq)]
struct StoppedAgent {
    harness: AgentProvider,
    said: String,
    /// A resume turn has been queued for it.
    resumed: bool,
}

/// What a limit lifting asks of the bridge: tell each agent whose turn died at
/// it, and start it again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::app) struct Resume {
    pub owner: String,
    pub agent_id: String,
    pub harness: AgentProvider,
    pub said: String,
}

/// Every harness out of usage on this device, and the agents it stopped.
#[derive(Debug, Default)]
pub(in crate::app) struct UsageLimits {
    by_harness: BTreeMap<&'static str, DeviceUsageLimit>,
    /// Kept apart from the record because it outlives it: a turn on another
    /// agent that clears the limit does not finish THIS agent's work, and it
    /// still wants starting again.
    stopped: BTreeMap<AgentKey, StoppedAgent>,
}

impl UsageLimits {
    /// An agent's session reported that its turn stopped at a limit. True when
    /// the device's record moved, which is when the board item has news.
    pub(in crate::app) fn observe(
        &mut self,
        harness: AgentProvider,
        owner: &str,
        agent_id: &str,
        limit: &UsageLimited,
        now: OffsetDateTime,
    ) -> bool {
        self.stopped.insert(
            (owner.to_string(), agent_id.to_string()),
            StoppedAgent {
                harness,
                said: limit.said.clone(),
                resumed: false,
            },
        );
        let released = limit.resets_at.is_none_or(|at| at <= now);
        match self.by_harness.get_mut(harness.wire_id()) {
            Some(record) if record.said == limit.said && record.resets_at == limit.resets_at => {
                false
            }
            Some(record) => {
                record.said = limit.said.clone();
                record.resets_at = limit.resets_at;
                record.released = released;
                record.wake_asked = false;
                true
            }
            None => {
                self.by_harness.insert(
                    harness.wire_id(),
                    DeviceUsageLimit {
                        harness,
                        since: now,
                        resets_at: limit.resets_at,
                        said: limit.said.clone(),
                        released,
                        wake_asked: false,
                        told: BTreeSet::new(),
                    },
                );
                true
            }
        }
    }

    /// A turn is running on `harness` for this agent: the harness is
    /// answering, so the device's limit on it is over, and this agent is no
    /// longer stopped. Answers the device record's change (true when there was
    /// one to clear) and the other agents the limit stopped that nothing has
    /// started again yet.
    pub(in crate::app) fn turn_ran(
        &mut self,
        harness: AgentProvider,
        owner: &str,
        agent_id: &str,
    ) -> (bool, Vec<Resume>) {
        self.stopped
            .remove(&(owner.to_string(), agent_id.to_string()));
        let cleared = self.by_harness.remove(harness.wire_id()).is_some();
        let resumes = if cleared {
            self.take_resumes(harness)
        } else {
            Vec::new()
        };
        (cleared, resumes)
    }

    /// Whether a turn on `harness` waits for its limit to reset.
    pub(in crate::app) fn holds(&self, harness: AgentProvider) -> bool {
        self.by_harness
            .get(harness.wire_id())
            .is_some_and(|record| !record.released)
    }

    /// Whether this agent's last turn stopped at a limit and nothing has run
    /// on it since.
    pub(in crate::app) fn stopped(&self, owner: &str, agent_id: &str) -> bool {
        self.stopped
            .contains_key(&(owner.to_string(), agent_id.to_string()))
    }

    /// The harness's sentence, the first time this conversation has a turn
    /// held behind the limit on `harness`; `None` every time after.
    pub(in crate::app) fn first_hold(
        &mut self,
        harness: AgentProvider,
        owner: &str,
        agent_id: &str,
    ) -> Option<String> {
        let record = self
            .by_harness
            .get_mut(harness.wire_id())
            .filter(|record| !record.released)?;
        record
            .told
            .insert((owner.to_string(), agent_id.to_string()))
            .then(|| record.said.clone())
    }

    /// Let turns start again on every harness whose reset has passed, and
    /// answer the agents that stopped at it and want starting again.
    pub(in crate::app) fn release_due(&mut self, now: OffsetDateTime) -> Vec<Resume> {
        let due: Vec<AgentProvider> = self
            .by_harness
            .values_mut()
            .filter(|record| !record.released)
            .filter(|record| record.resets_at.is_some_and(|at| at <= now))
            .map(|record| {
                record.released = true;
                record.harness
            })
            .collect();
        due.into_iter()
            .flat_map(|harness| self.take_resumes(harness))
            .collect()
    }

    /// The earliest reset nobody has asked to be woken for yet, marked asked.
    pub(in crate::app) fn wake_due(&mut self) -> Option<OffsetDateTime> {
        self.by_harness
            .values_mut()
            .filter(|record| !record.released && !record.wake_asked)
            .filter_map(|record| {
                record.wake_asked = true;
                record.resets_at
            })
            .min()
    }

    /// The device's limits as the wire carries them: one entry per harness.
    pub(in crate::app) fn render(&self) -> Value {
        Value::Array(
            self.by_harness
                .values()
                .map(DeviceUsageLimit::render)
                .collect(),
        )
    }

    fn take_resumes(&mut self, harness: AgentProvider) -> Vec<Resume> {
        self.stopped
            .iter_mut()
            .filter(|(_, stopped)| stopped.harness == harness && !stopped.resumed)
            .map(|((owner, agent_id), stopped)| {
                stopped.resumed = true;
                Resume {
                    owner: owner.clone(),
                    agent_id: agent_id.clone(),
                    harness,
                    said: stopped.said.clone(),
                }
            })
            .collect()
    }
}

fn rfc3339(at: OffsetDateTime) -> String {
    at.format(&Rfc3339).unwrap_or_default()
}

/// What a conversation is told when a turn to it is held behind the limit.
///
/// The harness's own sentence rather than a reset time restated: it names the
/// time in the zone the human reads, and restating it would mean choosing one.
fn held_notice(harness: AgentProvider, said: &str) -> String {
    let label = crate::harness::harness_for(harness).label();
    format!(
        "{label} has run out of usage on this device (“{said}”). Messages to this agent \
         stay queued and are delivered in order when it resets."
    )
}

/// What an agent whose turn died at the limit is told when it lifts.
fn resume_notice(harness: AgentProvider, said: &str) -> String {
    let label = crate::harness::harness_for(harness).label();
    format!(
        "{label}'s usage limit has reset. Your last turn stopped when it ran out \
         (“{said}”), so it did not finish: pick up where you left off, starting \
         from what is in your working tree."
    )
}

impl AppState {
    /// The harness an agent runs on, from its record.
    fn agent_harness(&self, owner: &str, agent_id: &str) -> Option<AgentProvider> {
        self.entity_agents(owner)
            .ok()?
            .by_id(agent_id)
            .map(|agent| agent.choice.provider)
    }

    /// Record what one agent's status snapshot says about usage, against the
    /// device. `recorded` is what this agent's pump last recorded, so a limit
    /// carried on every later snapshot of an idle session is recorded once
    /// and not again after another agent's turn has cleared it.
    ///
    /// Answers whether the delivery queue should be drained: a new limit wants
    /// a wake at its reset, and a cleared one may have agents to start again.
    pub(in crate::app) fn record_usage_limit(
        &mut self,
        owner: &str,
        agent_id: &str,
        snapshot: &crate::harness::SessionStatusSnapshot,
        recorded: &mut Option<UsageLimited>,
    ) -> bool {
        let Some(harness) = self.agent_harness(owner, agent_id) else {
            return false;
        };
        // The limit first: a turn that stops at it says so while its status
        // still reads Working, and that is not a turn running. A turn that
        // really starts clears the limit off its own snapshot.
        match (&snapshot.usage_limit, snapshot.status) {
            (Some(limit), _) => self.record_limit_seen(harness, owner, agent_id, limit, recorded),
            (None, crate::harness::AgentStatus::Working) => {
                *recorded = None;
                self.record_turn_ran(harness, owner, agent_id)
            }
            (None, _) => false,
        }
    }

    fn record_limit_seen(
        &mut self,
        harness: AgentProvider,
        owner: &str,
        agent_id: &str,
        limit: &UsageLimited,
        recorded: &mut Option<UsageLimited>,
    ) -> bool {
        if recorded.as_ref() == Some(limit) {
            return false;
        }
        *recorded = Some(limit.clone());
        let now = OffsetDateTime::now_utc();
        if self
            .usage_limits
            .observe(harness, owner, agent_id, limit, now)
        {
            eprintln!(
                "usage limit recorded: harness={} agent={agent_id} resets_at={:?}",
                harness.wire_id(),
                limit.resets_at.map(rfc3339)
            );
            self.note_usage_limits_changed();
        }
        true
    }

    fn record_turn_ran(&mut self, harness: AgentProvider, owner: &str, agent_id: &str) -> bool {
        let (cleared, resumes) = self.usage_limits.turn_ran(harness, owner, agent_id);
        if cleared {
            eprintln!(
                "usage limit cleared: harness={} agent={agent_id}",
                harness.wire_id()
            );
            self.note_usage_limits_changed();
        }
        self.resume_after_usage_limit(resumes)
    }

    /// Before a drain takes its turns: lift every limit whose reset has passed
    /// and queue the agents it stopped.
    pub(in crate::app) fn release_due_usage_limits(&mut self) {
        self.release_usage_limits_due_at(OffsetDateTime::now_utc());
    }

    pub(in crate::app) fn release_usage_limits_due_at(&mut self, now: OffsetDateTime) {
        let resumes = self.usage_limits.release_due(now);
        self.resume_after_usage_limit(resumes);
    }

    /// After a drain has taken its turns: tell each conversation with a turn
    /// held behind a limit, once.
    pub(in crate::app) fn tell_turns_held_by_usage_limits(&mut self) {
        let held: Vec<(AgentProvider, String, String)> = self
            .delivery_queue
            .queued()
            .filter(|turn| self.usage_limits.holds(turn.model_choice.provider))
            .map(|turn| {
                (
                    turn.model_choice.provider,
                    turn.owner.clone(),
                    turn.agent_id.clone(),
                )
            })
            .collect();
        for (harness, owner, agent_id) in held {
            let Some(said) = self.usage_limits.first_hold(harness, &owner, &agent_id) else {
                continue;
            };
            let notice = held_notice(harness, &said);
            let now = crate::store::now_rfc3339();
            if let Err(why) = self.edit_agent_conversation(&owner, &agent_id, |thread, _| {
                thread.post_user_from_build(notice, &now);
                Ok(Value::Null)
            }) {
                eprintln!(
                    "usage limit: {agent_id} on {owner} was not told its turn is held: {why}"
                );
            }
        }
    }

    /// When the next limit's reset is due, if a drain has not already asked.
    pub(in crate::app) fn usage_limit_wake_due(&mut self) -> Option<OffsetDateTime> {
        self.usage_limits.wake_due()
    }

    /// Tell each agent its limit has lifted, and queue a turn to start it
    /// again — the way the resume roster starts an agent whose turn a restart
    /// cut short. An agent with a turn already queued is only told: that turn
    /// starts it. Answers whether anything was queued.
    fn resume_after_usage_limit(&mut self, resumes: Vec<Resume>) -> bool {
        let mut queued = false;
        for resume in resumes {
            match self.resume_one_after_usage_limit(&resume) {
                Ok(started) => queued |= started,
                Err(why) => eprintln!(
                    "usage limit: {} on {} was not started again: {why}",
                    resume.agent_id, resume.owner
                ),
            }
        }
        queued
    }

    fn resume_one_after_usage_limit(&mut self, resume: &Resume) -> Result<bool, String> {
        let addressed = self.addressed_agent(&json!({
            "id": resume.owner,
            "agent_id": resume.agent_id,
        }))?;
        let notice = resume_notice(resume.harness, &resume.said);
        let now = crate::store::now_rfc3339();
        self.edit_agent_conversation(&resume.owner, &resume.agent_id, |thread, _| {
            thread.post_user_from_build(notice, &now);
            Ok(Value::Null)
        })?;
        let already_queued = self
            .delivery_queue
            .queued()
            .any(|turn| turn.owner == resume.owner && turn.agent_id == resume.agent_id);
        if already_queued {
            return Ok(false);
        }
        self.delivery_queue.enqueue(PendingAgentTurn {
            operation_id: None,
            root: addressed.root.clone(),
            owner: addressed.entity_id.clone(),
            agent_id: addressed.agent_id.clone(),
            conversation_id: addressed.conversation_id.clone(),
            model_choice: addressed.model_choice.clone(),
            choice_revision: addressed.choice_revision,
            interrupt: false,
            // The notice is on the thread; the turn is the nudge to read it.
            // Cold when the session went with the limit, warm when it is still
            // there and remembers the turn that stopped.
            say: Some(TurnText {
                cold: crate::orchestrator::conversation_prompt(NEW_THREAD_MESSAGES_PROMPT),
                warm: NEW_THREAD_MESSAGES_PROMPT.to_string(),
            }),
            phase: "resume",
            wants_catch_up: true,
            survives_refusal: false,
        });
        self.touch_attention(&addressed.entity_id);
        Ok(true)
    }

    fn note_usage_limits_changed(&self) {
        self.note_board_lists_changed(crate::changes::BoardLists::USAGE_LIMITS);
    }

    /// The device's limits, as `board.list` and the board item carry them.
    pub(in crate::app) fn usage_limits_json(&self) -> Value {
        self.usage_limits.render()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    const HARNESS: AgentProvider = AgentProvider::ClaudeAdk;
    const SAID: &str = "You've hit your session limit · resets 6:20pm (America/New_York)";

    fn limit(resets_at: Option<OffsetDateTime>) -> UsageLimited {
        UsageLimited {
            said: SAID.to_string(),
            resets_at,
        }
    }

    #[test]
    fn a_limit_is_one_record_per_harness_however_many_agents_hit_it() {
        let mut limits = UsageLimits::default();
        let now = datetime!(2026-09-20 21:30 UTC);
        let reset = Some(datetime!(2026-09-20 22:20 UTC));
        assert!(limits.observe(HARNESS, "run-1", "agent-a", &limit(reset), now));
        assert!(!limits.observe(HARNESS, "run-2", "agent-b", &limit(reset), now));
        let rendered = limits.render();
        assert_eq!(rendered.as_array().unwrap().len(), 1);
        assert_eq!(rendered[0]["harness"], "claude_adk");
        assert_eq!(rendered[0]["since"], "2026-09-20T21:30:00Z");
        assert_eq!(rendered[0]["resets_at"], "2026-09-20T22:20:00Z");
        assert_eq!(rendered[0]["said"], SAID);
        assert!(limits.stopped("run-1", "agent-a"));
        assert!(limits.stopped("run-2", "agent-b"));
    }

    #[test]
    fn a_known_reset_holds_turns_until_it_passes_and_then_starts_the_stopped_agents() {
        let mut limits = UsageLimits::default();
        let now = datetime!(2026-09-20 21:30 UTC);
        let reset = datetime!(2026-09-20 22:20 UTC);
        limits.observe(HARNESS, "run-1", "agent-a", &limit(Some(reset)), now);
        assert!(limits.holds(HARNESS));
        assert!(!limits.holds(AgentProvider::CodexAppServer));
        assert_eq!(limits.wake_due(), Some(reset));
        assert_eq!(limits.wake_due(), None, "asked once");

        assert!(limits
            .release_due(datetime!(2026-09-20 22:19 UTC))
            .is_empty());
        assert!(limits.holds(HARNESS));

        let resumes = limits.release_due(reset);
        assert_eq!(
            resumes,
            vec![Resume {
                owner: "run-1".into(),
                agent_id: "agent-a".into(),
                harness: HARNESS,
                said: SAID.into(),
            }]
        );
        assert!(!limits.holds(HARNESS));
        // Released is not cleared: the banner stays until a turn runs.
        assert_eq!(limits.render().as_array().unwrap().len(), 1);
        assert!(limits.release_due(reset).is_empty(), "started once");
    }

    #[test]
    fn no_reset_named_holds_nothing_and_waits_for_a_turn_to_run() {
        let mut limits = UsageLimits::default();
        let now = datetime!(2026-09-20 21:30 UTC);
        assert!(limits.observe(HARNESS, "run-1", "agent-a", &limit(None), now));
        assert!(!limits.holds(HARNESS));
        assert_eq!(limits.wake_due(), None);
        assert_eq!(limits.render()[0]["resets_at"], Value::Null);
        assert!(limits.release_due(now + time::Duration::days(2)).is_empty());
    }

    #[test]
    fn a_turn_running_clears_the_record_and_starts_the_other_stopped_agents() {
        let mut limits = UsageLimits::default();
        let now = datetime!(2026-09-20 21:30 UTC);
        let reset = Some(datetime!(2026-09-20 22:20 UTC));
        limits.observe(HARNESS, "run-1", "agent-a", &limit(reset), now);
        limits.observe(HARNESS, "run-2", "agent-b", &limit(reset), now);

        let (cleared, resumes) = limits.turn_ran(HARNESS, "run-1", "agent-a");
        assert!(cleared);
        assert_eq!(limits.render(), json!([]));
        assert!(!limits.holds(HARNESS));
        assert!(!limits.stopped("run-1", "agent-a"));
        assert_eq!(resumes.len(), 1);
        assert_eq!(resumes[0].agent_id, "agent-b");
        // Still stopped until its own turn runs, so nothing reads it as quiet.
        assert!(limits.stopped("run-2", "agent-b"));

        let (cleared, resumes) = limits.turn_ran(HARNESS, "run-2", "agent-b");
        assert!(!cleared, "already clear");
        assert!(resumes.is_empty());
        assert!(!limits.stopped("run-2", "agent-b"));
    }

    #[test]
    fn a_turn_on_another_harness_clears_nothing() {
        let mut limits = UsageLimits::default();
        let now = datetime!(2026-09-20 21:30 UTC);
        limits.observe(HARNESS, "run-1", "agent-a", &limit(None), now);
        let (cleared, _) = limits.turn_ran(AgentProvider::CodexAppServer, "run-2", "agent-c");
        assert!(!cleared);
        assert_eq!(limits.render().as_array().unwrap().len(), 1);
    }

    #[test]
    fn each_conversation_is_told_once_that_its_turn_is_held() {
        let mut limits = UsageLimits::default();
        let now = datetime!(2026-09-20 21:30 UTC);
        let reset = Some(datetime!(2026-09-20 22:20 UTC));
        limits.observe(HARNESS, "run-1", "agent-a", &limit(reset), now);
        assert_eq!(
            limits.first_hold(HARNESS, "run-2", "agent-b").as_deref(),
            Some(SAID)
        );
        assert_eq!(limits.first_hold(HARNESS, "run-2", "agent-b"), None);
        assert!(limits.first_hold(HARNESS, "run-3", "agent-c").is_some());
        assert_eq!(
            limits.first_hold(AgentProvider::CodexAppServer, "run-3", "agent-c"),
            None
        );
    }

    #[test]
    fn a_later_reset_holds_again_and_asks_for_a_new_wake() {
        let mut limits = UsageLimits::default();
        let now = datetime!(2026-09-20 21:30 UTC);
        let first = datetime!(2026-09-20 22:20 UTC);
        limits.observe(HARNESS, "run-1", "agent-a", &limit(Some(first)), now);
        limits.wake_due();
        limits.release_due(first);
        let later = datetime!(2026-09-21 03:20 UTC);
        let again = UsageLimited {
            said: "You've hit your session limit · resets 11:20pm (America/New_York)".into(),
            resets_at: Some(later),
        };
        assert!(limits.observe(HARNESS, "run-1", "agent-a", &again, first));
        assert!(limits.holds(HARNESS));
        assert_eq!(limits.wake_due(), Some(later));
        assert_eq!(limits.render()[0]["since"], "2026-09-20T21:30:00Z");
    }

    #[test]
    fn the_notices_quote_the_harness() {
        let held = held_notice(HARNESS, SAID);
        assert!(held.starts_with("Claude Code has run out of usage on this device"));
        assert!(held.contains(SAID));
        assert!(held.contains("delivered in order when it resets"));
        let resume = resume_notice(HARNESS, SAID);
        assert!(resume.contains("usage limit has reset"));
        assert!(resume.contains(SAID));
    }
}

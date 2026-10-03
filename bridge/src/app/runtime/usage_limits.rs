//! Usage reports shown on the board, with one automatic retry at a known reset.
//!
//! A report describes one session's last failure, not whether another request
//! can succeed. Users may change models or accounts, reset limits, or add credits,
//! so remembered limits never hold delivery. A successful response clears the
//! report; starting a request is not evidence that the provider accepted it.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

use crate::app::{AppState, PendingAgentTurn, TurnText, NEW_THREAD_MESSAGES_PROMPT};
use crate::harness::usage_limit::UsageLimited;
use crate::models::AgentProvider;

/// An agent, by the entity it belongs to and its own id.
type AgentKey = (String, String);

/// The latest observed usage failure for one harness on this device.
#[derive(Debug, Clone, PartialEq, Eq)]
struct DeviceUsageLimit {
    harness: AgentProvider,
    /// When the device first saw this limit.
    since: OffsetDateTime,
    resets_at: Option<OffsetDateTime>,
    /// The harness's own sentence, shown behind the banner.
    said: String,
    /// Whether the single automatic retry at this reset has been handled.
    /// No known future reset means there is no automatic retry to schedule.
    reset_retry_done: bool,
    /// Whether a drain has asked to be woken at `resets_at`.
    wake_asked: bool,
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

/// An agent whose turn stopped at a limit and has not succeeded since.
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

/// Usage reports for the board and agents whose work stopped at a limit.
#[derive(Debug, Default)]
pub(in crate::app) struct UsageLimits {
    by_harness: BTreeMap<&'static str, DeviceUsageLimit>,
    /// Kept apart from the record because it outlives it: a turn on another
    /// agent that clears the limit does not finish THIS agent's work, and it
    /// still wants starting again.
    stopped: BTreeMap<AgentKey, StoppedAgent>,
}

impl UsageLimits {
    pub(in crate::app) fn forget_agent(&mut self, owner: &str, agent_id: &str) {
        self.stopped
            .remove(&(owner.to_string(), agent_id.to_string()));
    }
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
        let reset_retry_done = limit.resets_at.is_none_or(|at| at <= now);
        match self.by_harness.get_mut(harness.wire_id()) {
            Some(record) if record.said == limit.said && record.resets_at == limit.resets_at => {
                false
            }
            Some(record) => {
                record.said = limit.said.clone();
                record.resets_at = limit.resets_at;
                record.reset_retry_done = reset_retry_done;
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
                        reset_retry_done,
                        wake_asked: false,
                    },
                );
                true
            }
        }
    }

    /// A response succeeded on `harness` for this agent: clear its report and
    /// mark this agent as no longer stopped. Answers whether a record was
    /// cleared and which other stopped agents have not been retried yet.
    pub(in crate::app) fn response_succeeded(
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

    /// Whether this agent stopped at a limit and has not succeeded since.
    pub(in crate::app) fn stopped(&self, owner: &str, agent_id: &str) -> bool {
        self.stopped
            .contains_key(&(owner.to_string(), agent_id.to_string()))
    }

    /// Retry stopped agents once when their reported reset has passed.
    pub(in crate::app) fn release_due(&mut self, now: OffsetDateTime) -> Vec<Resume> {
        let due: Vec<AgentProvider> = self
            .by_harness
            .values_mut()
            .filter(|record| !record.reset_retry_done)
            .filter(|record| record.resets_at.is_some_and(|at| at <= now))
            .map(|record| {
                record.reset_retry_done = true;
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
            .filter(|record| !record.reset_retry_done && !record.wake_asked)
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

/// What an agent whose turn died at the limit is told when it lifts.
fn resume_notice(harness: AgentProvider, said: &str) -> String {
    let label = crate::harness::harness_for(harness).label();
    format!(
        "Retrying {label} after its reported usage limit. Your last turn stopped \
         (“{said}”), so it did not finish: pick up where you left off, starting \
         from what is in your working tree."
    )
}

/// What this session's status pump has already observed. Success counters are
/// local to a session, so a replacement session starts with a fresh observation.
#[derive(Default)]
pub(in crate::app) struct UsageObservation {
    limit: Option<UsageLimited>,
    successful_response_count: u64,
    usage_limit_count: u64,
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
        recorded: &mut UsageObservation,
    ) -> bool {
        let Some(harness) = self.agent_harness(owner, agent_id) else {
            return false;
        };
        let succeeded = snapshot.successful_response_count > recorded.successful_response_count;
        recorded.successful_response_count = snapshot.successful_response_count;
        if snapshot.usage_limit_count > recorded.usage_limit_count {
            // Another session may have cleared the board since this agent last
            // failed. A new rejection is news even when its wording is identical.
            recorded.limit = None;
        }
        recorded.usage_limit_count = snapshot.usage_limit_count;
        // A newer limit wins if watch coalesced a success and a later failure.
        if let Some(limit) = &snapshot.usage_limit {
            return self.record_limit_seen(harness, owner, agent_id, limit, &mut recorded.limit);
        }
        if succeeded {
            recorded.limit = None;
            return self.record_response_succeeded(harness, owner, agent_id);
        }
        false
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

    fn record_response_succeeded(
        &mut self,
        harness: AgentProvider,
        owner: &str,
        agent_id: &str,
    ) -> bool {
        let (cleared, resumes) = self
            .usage_limits
            .response_succeeded(harness, owner, agent_id);
        if cleared {
            eprintln!(
                "usage limit cleared: harness={} agent={agent_id}",
                harness.wire_id()
            );
            self.note_usage_limits_changed();
        }
        self.resume_after_usage_limit(resumes)
    }

    /// Before a drain takes its turns, queue automatic retries whose reset is due.
    pub(in crate::app) fn release_due_usage_limits(&mut self) {
        self.release_usage_limits_due_at(OffsetDateTime::now_utc());
    }

    pub(in crate::app) fn release_usage_limits_due_at(&mut self, now: OffsetDateTime) {
        let resumes = self.usage_limits.release_due(now);
        self.resume_after_usage_limit(resumes);
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
    fn a_known_reset_retries_the_stopped_agents_once_when_it_passes() {
        let mut limits = UsageLimits::default();
        let now = datetime!(2026-09-20 21:30 UTC);
        let reset = datetime!(2026-09-20 22:20 UTC);
        limits.observe(HARNESS, "run-1", "agent-a", &limit(Some(reset)), now);
        assert_eq!(limits.wake_due(), Some(reset));
        assert_eq!(limits.wake_due(), None, "asked once");

        assert!(limits
            .release_due(datetime!(2026-09-20 22:19 UTC))
            .is_empty());

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
        // A scheduled retry does not clear the report; a response must succeed.
        assert_eq!(limits.render().as_array().unwrap().len(), 1);
        assert!(limits.release_due(reset).is_empty(), "started once");
    }

    #[test]
    fn no_reset_named_schedules_no_automatic_retry() {
        let mut limits = UsageLimits::default();
        let now = datetime!(2026-09-20 21:30 UTC);
        assert!(limits.observe(HARNESS, "run-1", "agent-a", &limit(None), now));
        assert_eq!(limits.wake_due(), None);
        assert_eq!(limits.render()[0]["resets_at"], Value::Null);
        assert!(limits.release_due(now + time::Duration::days(2)).is_empty());
    }

    #[test]
    fn a_successful_response_clears_the_record_and_starts_the_other_stopped_agents() {
        let mut limits = UsageLimits::default();
        let now = datetime!(2026-09-20 21:30 UTC);
        let reset = Some(datetime!(2026-09-20 22:20 UTC));
        limits.observe(HARNESS, "run-1", "agent-a", &limit(reset), now);
        limits.observe(HARNESS, "run-2", "agent-b", &limit(reset), now);

        let (cleared, resumes) = limits.response_succeeded(HARNESS, "run-1", "agent-a");
        assert!(cleared);
        assert_eq!(limits.render(), json!([]));
        assert!(!limits.stopped("run-1", "agent-a"));
        assert_eq!(resumes.len(), 1);
        assert_eq!(resumes[0].agent_id, "agent-b");
        // Still stopped until its own response succeeds.
        assert!(limits.stopped("run-2", "agent-b"));

        let (cleared, resumes) = limits.response_succeeded(HARNESS, "run-2", "agent-b");
        assert!(!cleared, "already clear");
        assert!(resumes.is_empty());
        assert!(!limits.stopped("run-2", "agent-b"));
    }

    #[test]
    fn a_success_on_another_harness_clears_nothing() {
        let mut limits = UsageLimits::default();
        let now = datetime!(2026-09-20 21:30 UTC);
        limits.observe(HARNESS, "run-1", "agent-a", &limit(None), now);
        let (cleared, _) =
            limits.response_succeeded(AgentProvider::CodexAppServer, "run-2", "agent-c");
        assert!(!cleared);
        assert_eq!(limits.render().as_array().unwrap().len(), 1);
    }

    #[test]
    fn a_later_reset_asks_for_a_new_wake() {
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
        assert_eq!(limits.wake_due(), Some(later));
        assert_eq!(limits.render()[0]["since"], "2026-09-20T21:30:00Z");
    }

    #[test]
    fn the_notices_quote_the_harness() {
        let resume = resume_notice(HARNESS, SAID);
        assert!(resume.contains("after its reported usage limit"));
        assert!(resume.contains(SAID));
    }
}

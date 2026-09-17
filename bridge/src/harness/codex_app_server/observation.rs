use serde::Deserialize;
use serde_json::Value;

use super::protocol::ThreadGoal;
use super::subagents::CodexSubagents;
use crate::harness::surfaces::{
    AgentSurfaces, ChecklistCollection, ChecklistProvenance, ChecklistSource, ChecklistState,
    GoalState, SurfaceChecklistItem, SurfaceCoverage, SurfaceGoal, SurfaceObservation,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoalReadFence {
    pub thread_id: String,
    pub generation: u64,
    pub revision: u64,
}

#[derive(Debug, Default)]
pub struct CodexSurfaces {
    pub observation: CodexObservation,
    pub subagents: CodexSubagents,
}

impl CodexSurfaces {
    pub fn snapshot(&self) -> Option<AgentSurfaces> {
        let mut surfaces = self.observation.snapshot();
        if let Some(execution) = self.subagents.snapshot() {
            surfaces.subagents = execution.subagents;
        }
        (!surfaces.is_empty()).then_some(surfaces)
    }
}

#[derive(Debug, Default)]
pub struct CodexObservation {
    thread_id: Option<String>,
    generation: u64,
    goal_revision: u64,
    active_goal_read: Option<GoalReadFence>,
    collection_epoch: u64,
    observed_turns: Vec<String>,
    current_turn: Option<String>,
    surfaces: AgentSurfaces,
    checklist: ChecklistCollection,
}

impl CodexObservation {
    pub fn open_thread(&mut self, thread_id: String, now: String) -> GoalReadFence {
        self.generation = self.generation.wrapping_add(1);
        self.goal_revision = 0;
        self.active_goal_read = None;
        self.thread_id = Some(thread_id.clone());
        self.observed_turns.clear();
        self.current_turn = None;
        self.collection_epoch = 0;
        self.checklist = ChecklistCollection::default();
        self.surfaces.goal = None;
        self.surfaces.observations.goal = Some(SurfaceObservation::loading());
        self.surfaces.observations.checklist = Some(SurfaceObservation::supported_loading());
        self.surfaces.observations.subagents =
            Some(SurfaceObservation::current(SurfaceCoverage::Complete, now));
        self.surfaces.observations.workflows = Some(SurfaceObservation::unsupported());
        self.surfaces.observations.shells = Some(SurfaceObservation::unsupported());
        GoalReadFence {
            thread_id,
            generation: self.generation,
            revision: 0,
        }
    }

    pub fn fence(&self) -> Option<GoalReadFence> {
        Some(GoalReadFence {
            thread_id: self.thread_id.clone()?,
            generation: self.generation,
            revision: self.goal_revision,
        })
    }

    pub fn begin_goal_read(&mut self, fence: &GoalReadFence) -> bool {
        if self.active_goal_read.is_some() || !self.is_current(fence) {
            return false;
        }
        self.active_goal_read = Some(fence.clone());
        true
    }

    pub fn finish_goal_read(&mut self, fence: &GoalReadFence) -> bool {
        if self.active_goal_read.as_ref() != Some(fence) {
            return false;
        }
        self.active_goal_read = None;
        true
    }

    pub fn observe_turn(&mut self, turn_id: &str) -> bool {
        if self.observed_turns.iter().any(|seen| seen == turn_id) {
            if self.current_turn.as_deref() != Some(turn_id) {
                return false;
            }
        } else {
            if self.observed_turns.len() == 256 {
                self.observed_turns.remove(0);
            }
            self.observed_turns.push(turn_id.to_string());
        }
        self.current_turn = Some(turn_id.to_string());
        let carried = self
            .checklist
            .provenance()
            .and_then(|value| value.turn_id.as_deref())
            != Some(turn_id);
        self.checklist.set_carried_from_prior_turn(carried)
    }

    pub fn apply_goal_read(
        &mut self,
        fence: &GoalReadFence,
        goal: Option<ThreadGoal>,
        now: String,
    ) -> bool {
        if !self.matches(fence)
            || fence.revision != self.goal_revision
            || goal
                .as_ref()
                .is_some_and(|goal| goal.thread_id != fence.thread_id)
        {
            return false;
        }
        let changed = self.replace_goal(goal, now);
        self.goal_revision = self.goal_revision.wrapping_add(1);
        changed
    }

    pub fn apply_goal_updated(&mut self, params: &Value, now: String) -> Result<bool, String> {
        self.goal_revision = self.goal_revision.wrapping_add(1);
        let update: GoalUpdated = serde_json::from_value(params.clone())
            .map_err(|error| format!("thread/goal/updated has the wrong body: {error}"))?;
        if self.thread_id.as_deref() != Some(&update.thread_id) {
            return Ok(false);
        }
        if update.goal.thread_id != update.thread_id {
            return Err("thread/goal/updated goal belongs to another thread".to_string());
        }
        Ok(self.replace_goal(Some(update.goal), now))
    }

    pub fn apply_goal_cleared(&mut self, params: &Value, now: String) -> Result<bool, String> {
        self.goal_revision = self.goal_revision.wrapping_add(1);
        let clear: GoalCleared = serde_json::from_value(params.clone())
            .map_err(|error| format!("thread/goal/cleared has the wrong body: {error}"))?;
        if self.thread_id.as_deref() != Some(&clear.thread_id) {
            return Ok(false);
        }
        Ok(self.replace_goal(None, now))
    }

    pub fn mark_goal_stale(&mut self) -> bool {
        self.goal_revision = self.goal_revision.wrapping_add(1);
        let held = self.surfaces.observations.goal.as_ref();
        let stale = held
            .map(SurfaceObservation::as_stale)
            .unwrap_or_else(SurfaceObservation::unknown_stale);
        if held == Some(&stale) {
            return false;
        }
        self.surfaces.observations.goal = Some(stale);
        true
    }

    pub fn mark_goal_unsupported(&mut self, fence: &GoalReadFence) -> bool {
        if !self.is_current(fence) {
            return false;
        }
        self.goal_revision = self.goal_revision.wrapping_add(1);
        let unsupported = SurfaceObservation::unsupported();
        let changed = self.surfaces.goal.is_some()
            || self.surfaces.observations.goal.as_ref() != Some(&unsupported);
        self.surfaces.goal = None;
        self.surfaces.observations.goal = Some(unsupported);
        changed
    }

    pub fn apply_plan(&mut self, params: &Value, now: String) -> Result<bool, String> {
        let update: PlanUpdated = serde_json::from_value(params.clone())
            .map_err(|error| format!("turn/plan/updated has the wrong body: {error}"))?;
        if self.thread_id.as_deref() != Some(&update.thread_id)
            || self.plan_is_older(&update.turn_id)
        {
            return Ok(false);
        }
        if !self
            .observed_turns
            .iter()
            .any(|turn| turn == &update.turn_id)
        {
            if self.observed_turns.len() == 256 {
                self.observed_turns.remove(0);
            }
            self.observed_turns.push(update.turn_id.clone());
        }
        self.current_turn = Some(update.turn_id.clone());
        let original_count = update.plan.len();
        let mut state_truncated = false;
        let checklist = update
            .plan
            .into_iter()
            .enumerate()
            .map(|(index, entry)| {
                let (state, state_cut) = ChecklistState::from_provider_bounded(&entry.status);
                state_truncated |= state_cut;
                SurfaceChecklistItem {
                    id: format!("{}:{index}", update.turn_id),
                    subject: entry.step,
                    description: None,
                    state: Some(state),
                }
            })
            .collect::<Vec<_>>();
        let carried = self.current_turn.as_deref() != Some(&update.turn_id);
        let provenance = ChecklistProvenance::new(
            ChecklistSource::TurnPlan,
            self.generation,
            Some(update.turn_id.clone()),
            self.collection_epoch,
        );
        let (mut provenance, explanation_cut) = provenance.with_explanation(update.explanation);
        provenance.carried_from_prior_turn = carried;
        let evidence_coverage = if state_truncated || explanation_cut {
            SurfaceCoverage::Partial
        } else {
            SurfaceCoverage::Complete
        };
        let changed = self.checklist.replace_with_coverage(
            checklist.clone(),
            provenance.clone(),
            now.clone(),
            Some(original_count),
            evidence_coverage,
        );
        if !changed {
            return Ok(false);
        }
        self.collection_epoch = self.collection_epoch.wrapping_add(1);
        provenance.collection_epoch = self.collection_epoch;
        self.checklist.replace_with_coverage(
            checklist,
            provenance,
            now,
            Some(original_count),
            evidence_coverage,
        );
        Ok(true)
    }

    pub fn snapshot(&self) -> AgentSurfaces {
        let mut surfaces = self.surfaces.clone();
        surfaces.checklist = self.checklist.items().to_vec();
        surfaces.checklist_provenance = self.checklist.provenance().cloned();
        surfaces.observations.checklist = self
            .checklist
            .observation()
            .cloned()
            .or_else(|| surfaces.observations.checklist.clone());
        surfaces
    }

    pub fn is_current(&self, fence: &GoalReadFence) -> bool {
        self.matches(fence) && self.goal_revision == fence.revision
    }

    pub fn mark_checklist_stale(&mut self) -> bool {
        self.checklist.mark_stale()
    }

    pub fn mark_terminal_stale(&mut self) -> bool {
        let mut changed = self.mark_goal_stale();
        changed |= self.checklist.mark_stale();
        if let Some(observation) = self.surfaces.observations.subagents.as_mut() {
            let stale = observation.as_stale();
            if *observation != stale {
                *observation = stale;
                changed = true;
            }
        }
        changed
    }

    fn replace_goal(&mut self, goal: Option<ThreadGoal>, now: String) -> bool {
        let (goal, truncated) = goal.map(to_surface_goal).unzip();
        let coverage = if truncated.unwrap_or(false) {
            SurfaceCoverage::Partial
        } else {
            SurfaceCoverage::Complete
        };
        let observation = SurfaceObservation::current(coverage, now);
        if self.surfaces.goal == goal
            && self
                .surfaces
                .observations
                .goal
                .as_ref()
                .is_some_and(|held| {
                    held.support() == observation.support()
                        && held.freshness() == observation.freshness()
                        && held.coverage() == observation.coverage()
                })
        {
            return false;
        }
        self.surfaces.goal = goal;
        self.surfaces.observations.goal = Some(observation);
        true
    }

    fn plan_is_older(&self, candidate: &str) -> bool {
        let current_index = self
            .current_turn
            .as_ref()
            .and_then(|current| self.observed_turns.iter().position(|turn| turn == current));
        let candidate_index = self
            .observed_turns
            .iter()
            .position(|turn| turn == candidate);
        if matches!((current_index, candidate_index), (Some(current), Some(candidate)) if candidate < current)
        {
            return true;
        }
        let held = self
            .checklist
            .provenance()
            .and_then(|value| value.turn_id.as_ref());
        let held_index =
            held.and_then(|held| self.observed_turns.iter().position(|turn| turn == held));
        matches!((held_index, candidate_index), (Some(held), Some(candidate)) if candidate < held)
    }

    fn matches(&self, fence: &GoalReadFence) -> bool {
        self.generation == fence.generation && self.thread_id.as_ref() == Some(&fence.thread_id)
    }
}

fn to_surface_goal(goal: ThreadGoal) -> (SurfaceGoal, bool) {
    let (state, state_cut) = GoalState::from_provider_bounded(&goal.status);
    let (mut surface, objective_cut) = SurfaceGoal::new(goal.objective, state);
    surface.token_budget = goal.token_budget;
    surface.tokens_used = Some(goal.tokens_used);
    surface.time_used_seconds = Some(goal.time_used_seconds);
    // Codex 0.154.0 emits chrono `timestamp()` seconds; Build's wire contract is milliseconds.
    surface.created_at = goal.created_at.checked_mul(1000);
    surface.updated_at = goal.updated_at.checked_mul(1000);
    let invalid_timestamps = surface.created_at.is_none() || surface.updated_at.is_none();
    (surface, objective_cut || state_cut || invalid_timestamps)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GoalUpdated {
    thread_id: String,
    goal: ThreadGoal,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GoalCleared {
    thread_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PlanUpdated {
    thread_id: String,
    turn_id: String,
    explanation: Option<String>,
    plan: Vec<RawPlanEntry>,
}

#[derive(Deserialize)]
struct RawPlanEntry {
    step: String,
    status: String,
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;
    use crate::harness::codex_app_server::connection::AppServerConnection;
    use crate::harness::codex_app_server::limits::ConnectionLimits;
    use crate::harness::codex_app_server::protocol::{OperationResult, PendingOperation};
    use serde_json::json;

    fn goal(objective: &str) -> ThreadGoal {
        ThreadGoal {
            thread_id: "parent".into(),
            objective: objective.into(),
            status: "active".into(),
            token_budget: None,
            tokens_used: 4,
            time_used_seconds: 3,
            created_at: 1_750_000_000,
            updated_at: 1_750_000_001,
        }
    }

    #[test]
    fn notification_and_clear_fence_an_older_read() {
        let mut reducer = CodexObservation::default();
        let old = reducer.open_thread("parent".into(), "open".into());
        reducer
            .apply_goal_updated(
                &json!({"threadId":"parent","goal":goal("new")}),
                "now".into(),
            )
            .unwrap();
        assert!(!reducer.apply_goal_read(&old, Some(goal("old")), "later".into()));
        reducer
            .apply_goal_cleared(&json!({"threadId":"parent"}), "clear".into())
            .unwrap();
        assert!(!reducer.apply_goal_read(&old, Some(goal("older")), "latest".into()));
        assert!(reducer.snapshot().goal.is_none());
    }

    #[test]
    fn observed_turn_order_rejects_late_old_plan_and_empty_replaces() {
        let mut reducer = CodexObservation::default();
        reducer.open_thread("parent".into(), "open".into());
        reducer.observe_turn("z");
        reducer.apply_plan(&json!({"threadId":"parent","turnId":"z","plan":[{"step":"old","status":"pending"}]}), "one".into()).unwrap();
        reducer.observe_turn("a");
        reducer
            .apply_plan(
                &json!({"threadId":"parent","turnId":"a","plan":[]}),
                "two".into(),
            )
            .unwrap();
        assert!(!reducer.apply_plan(&json!({"threadId":"parent","turnId":"z","plan":[{"step":"late","status":"completed"}]}), "three".into()).unwrap());
        assert!(reducer.snapshot().checklist.is_empty());
    }

    #[test]
    fn late_plan_cannot_clear_prior_turn_context_before_new_turn_has_a_plan() {
        let mut reducer = CodexObservation::default();
        reducer.open_thread("parent".into(), "open".into());
        reducer.observe_turn("turn-a");
        reducer
            .apply_plan(
                &json!({"threadId":"parent","turnId":"turn-a","plan":[{"step":"a","status":"pending"}]}),
                "first".into(),
            )
            .unwrap();
        assert!(reducer.observe_turn("turn-b"));
        assert!(
            reducer
                .snapshot()
                .checklist_provenance
                .unwrap()
                .carried_from_prior_turn
        );
        assert!(!reducer
            .apply_plan(
                &json!({"threadId":"parent","turnId":"turn-a","plan":[{"step":"late a","status":"completed"}]}),
                "late".into(),
            )
            .unwrap());
        let carried = reducer.snapshot();
        assert_eq!(carried.checklist[0].subject, "a");
        assert!(
            carried
                .checklist_provenance
                .unwrap()
                .carried_from_prior_turn
        );
        assert!(reducer
            .apply_plan(
                &json!({"threadId":"parent","turnId":"turn-b","plan":[{"step":"b","status":"inProgress"}]}),
                "second".into(),
            )
            .unwrap());
        let current = reducer.snapshot();
        assert_eq!(current.checklist[0].subject, "b");
        assert!(
            !current
                .checklist_provenance
                .unwrap()
                .carried_from_prior_turn
        );
    }

    #[test]
    fn goal_seconds_are_normalized_to_milliseconds() {
        let mut reducer = CodexObservation::default();
        let fence = reducer.open_thread("parent".into(), "open".into());
        reducer.apply_goal_read(&fence, Some(goal("work")), "now".into());
        let goal = reducer.snapshot().goal.unwrap();
        assert_eq!(goal.created_at, Some(1_750_000_000_000));
        assert_eq!(goal.time_used_seconds, Some(3));
    }

    #[test]
    fn goal_get_requires_the_nullable_goal_member() {
        let operation = PendingOperation::ReadGoal {
            thread_id: "parent".into(),
            generation: 1,
            revision: 0,
            attempt: 0,
        };
        assert!(
            matches!(operation.decode_result(&json!({"goal":null})), Ok(OperationResult::GoalRead(result)) if result.goal.is_none())
        );
        assert!(operation.decode_result(&json!({})).is_err());
    }

    #[test]
    fn malformed_goal_success_is_an_isolated_rpc_error() {
        let connection = AppServerConnection::memory(ConnectionLimits {
            inbound_frame_bytes: 4096,
            outbound_frame_bytes: 4096,
            pending_requests: 4,
        });
        connection
            .request(PendingOperation::ReadGoal {
                thread_id: "parent".into(),
                generation: 1,
                revision: 0,
                attempt: 0,
            })
            .unwrap();
        let event = connection
            .read_event(&mut Cursor::new(b"{\"id\":1,\"result\":{}}\n"))
            .unwrap()
            .unwrap();
        assert!(matches!(
            event,
            crate::harness::codex_app_server::protocol::ConnectionEvent::Response {
                operation: PendingOperation::ReadGoal { .. },
                result: Err(_)
            }
        ));
        connection
            .request(PendingOperation::ReadGoal {
                thread_id: "parent".into(),
                generation: 1,
                revision: 1,
                attempt: 1,
            })
            .unwrap();
        let malformed_error = connection
            .read_event(&mut Cursor::new(b"{\"id\":2,\"error\":{}}\n"))
            .unwrap()
            .unwrap();
        assert!(matches!(
            malformed_error,
            crate::harness::codex_app_server::protocol::ConnectionEvent::Response {
                operation: PendingOperation::ReadGoal { .. },
                result: Err(_)
            }
        ));
    }

    #[test]
    fn goal_get_uses_the_stable_non_experimental_request_shape() {
        let operation = PendingOperation::ReadGoal {
            thread_id: "parent".into(),
            generation: 1,
            revision: 0,
            attempt: 0,
        };
        let mut bytes = Vec::new();
        operation.serialize_request(9, &mut bytes).unwrap();
        let request: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            request,
            json!({"id":9,"method":"thread/goal/get","params":{"threadId":"parent"}})
        );
    }

    #[test]
    fn only_one_goal_read_is_in_flight_and_old_fences_expire() {
        let mut reducer = CodexObservation::default();
        let fence = reducer.open_thread("parent".into(), "open".into());
        assert!(reducer.begin_goal_read(&fence));
        assert!(!reducer.begin_goal_read(&fence));
        reducer
            .apply_goal_updated(
                &json!({"threadId":"parent","goal":goal("new")}),
                "now".into(),
            )
            .unwrap();
        assert!(!reducer.is_current(&fence));
        assert!(reducer.finish_goal_read(&fence));
        let current = reducer.fence().unwrap();
        assert!(reducer.begin_goal_read(&current));
    }

    #[test]
    fn successful_reads_rearm_more_than_three_recovery_episodes() {
        let mut reducer = CodexObservation::default();
        reducer.open_thread("parent".into(), "open".into());
        for episode in 0..10 {
            let fence = reducer.fence().unwrap();
            assert!(reducer.begin_goal_read(&fence));
            assert!(reducer.finish_goal_read(&fence));
            assert!(reducer.apply_goal_read(
                &fence,
                Some(goal(&format!("episode {episode}"))),
                format!("receipt {episode}"),
            ));
        }
        assert_eq!(reducer.snapshot().goal.unwrap().objective, "episode 9");
    }

    #[test]
    fn mismatched_nested_goal_owner_is_malformed_parent_evidence() {
        let mut reducer = CodexObservation::default();
        reducer.open_thread("parent".into(), "open".into());
        let mut foreign = goal("foreign");
        foreign.thread_id = "child".into();
        assert!(reducer
            .apply_goal_updated(&json!({"threadId":"parent","goal":foreign}), "now".into(),)
            .is_err());
    }
}

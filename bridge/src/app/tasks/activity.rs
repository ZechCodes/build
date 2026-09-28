//! A legacy Task's recorded implementation activity, kept current as its
//! implementation reports.
//!
//! The scheduler that armed an intent and cut checkouts for it was cut
//! (#207). Stored Tasks still load with whatever intent and activity they
//! were left with, and a run implementing one still reports, so the record
//! keeps following its run: nothing here starts work.

use crate::app::{next_unsettled_stage, record_current_stage_started, run_state_str, AppState};
use crate::plan::{ImplementationActivity, ImplementationIntent, StageDoc, StageDocState};
use crate::run::RunState;

impl AppState {
    fn set_task_activity(
        &mut self,
        task_id: &str,
        intent: Option<ImplementationIntent>,
        activity: ImplementationActivity,
    ) -> Result<(), String> {
        let mut task = self.take_plan(task_id)?;
        if let Some(intent) = intent {
            task.plan.implementation_intent = intent;
        }
        task.plan.implementation_activity = activity;
        self.finish_plan_mutation(task_id.to_string(), task)
    }

    pub(in crate::app) fn refresh_task_activity(&mut self, task_id: &str) -> Result<(), String> {
        let Some(task) = self.plans.get(task_id) else {
            return Err("unknown task_id".to_string());
        };
        if task.plan.implementation_intent == ImplementationIntent::None {
            return Ok(());
        }
        let Some(run) = self.current_task_implementation(task_id) else {
            return Ok(());
        };
        let (intent, activity) = match run.run.state {
            RunState::Review | RunState::Merged => (
                Some(ImplementationIntent::None),
                ImplementationActivity::Idle,
            ),
            RunState::StageGate => {
                let next = next_unsettled_stage(&task.stages, Some(run))
                    .map(|doc| (doc.id.clone(), doc.state));
                match next {
                    Some((stage_id, StageDocState::Planned)) => {
                        (None, ImplementationActivity::WaitingApproval(stage_id))
                    }
                    Some((stage_id, StageDocState::Approved)) => {
                        (None, ImplementationActivity::Running(stage_id))
                    }
                    None => (
                        Some(ImplementationIntent::None),
                        ImplementationActivity::Idle,
                    ),
                }
            }
            RunState::Building => (
                None,
                ImplementationActivity::Running(
                    run.current_stage_id
                        .clone()
                        .unwrap_or_else(|| "implementation".into()),
                ),
            ),
            RunState::Blocked
            | RunState::Failed
            | RunState::IdleUnreported
            | RunState::Interrupted
            | RunState::Abandoned
            | RunState::Archived => (
                None,
                ImplementationActivity::Blocked {
                    stage_id: run.current_stage_id.clone().unwrap_or_default(),
                    reason: run.last_error.clone().unwrap_or_else(|| {
                        format!("implementation is {}", run_state_str(&run.run.state))
                    }),
                },
            ),
            RunState::Created => (None, ImplementationActivity::Preparing),
        };
        self.set_task_activity(task_id, intent, activity)
    }

    pub(in crate::app) fn record_task_current_stage_started(
        &mut self,
        run_id: &str,
        stages: &[StageDoc],
    ) -> Result<(), String> {
        let task_id = self
            .runs
            .get(run_id)
            .and_then(|run| run.run.plan_id.as_ref())
            .map(|id| id.0.clone());
        let Some(task_id) = task_id else {
            return Ok(());
        };
        let mut task = self.take_plan(&task_id)?;
        if let Some(run) = self.runs.get(run_id) {
            record_current_stage_started(task.agents.sole_thread_mut(), run, stages);
        }
        self.finish_plan_mutation(task_id, task)
    }
}

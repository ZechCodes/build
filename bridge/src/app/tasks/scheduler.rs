use crate::app::WorktreeLifecycleJob;
use crate::app::{
    next_unsettled_stage, record_current_stage_started, require_str, run_state_str, thread_detail,
    AppState, ImplementationCaller,
};
use crate::lifecycle::{PendingRow, RestoreImplementationCheckout};
use crate::plan::{
    ImplementationActivity, ImplementationIntent, PlanState, StageDoc, StageDocState,
};
use crate::run::RunState;
use serde_json::{json, Value};

fn ensure_task_scheduler_available() -> Result<(), String> {
    Err(super::TASKS_RETIRED_ERROR.to_string())
}

/// A Task's scheduler asked, on its way to a stage: it carries on from where
/// the git stopped it, and answers with the Task rather than the run — the
/// scheduler is what the frame called, and the run is an implementation detail
/// of the stage it was after.
pub(in crate::app) struct TaskSchedulerWaiting {
    pub(in crate::app) task_id: String,
    /// The stage a failure is recorded against. `None` for run-all, which
    /// blocks on whichever stage the Task is standing at.
    pub(in crate::app) blocked_stage: Option<String>,
}

impl ImplementationCaller for TaskSchedulerWaiting {
    fn opened(self: Box<Self>, _state: &mut AppState, _run_id: &str) -> Result<Value, String> {
        ensure_task_scheduler_available()?;
        Ok(Value::Null)
    }

    fn refused(self: Box<Self>, state: &mut AppState, error: String) -> String {
        // The Task said it was preparing something. Nothing is preparing it
        // now, and a spinner nothing will ever clear is worse than the failure.
        state.block_task_scheduler(&self.task_id, self.blocked_stage, &error);
        error
    }
}

/// What a Task's scheduler is asked with when the frame that woke it was
/// about something else: the Task to advance, and the thread paging that
/// frame's own answer is cut to.
pub(in crate::app) fn scheduler_request(task_id: &str, params: &Value) -> Value {
    json!({ "task_id": task_id, "thread_limit": params.get("thread_limit") })
}

impl AppState {
    pub(crate) fn task_implement_all(&mut self, params: &Value) -> Result<Value, String> {
        let task_id = require_str(params, "task_id")?;
        self.arm_task_scheduler(&task_id, ImplementationIntent::All)?;
        self.implement_task(&task_id, params, None)
    }

    /// Advance one Task's scheduler for a frame: either it is settled here and
    /// the Task is the answer, or it is waiting on git, which goes to the
    /// drain and answers for it. A refusal before any git blocks the scheduler,
    /// the same way the job's own refusal does.
    pub(in crate::app) fn implement_task(
        &mut self,
        task_id: &str,
        params: &Value,
        blocked_stage: Option<String>,
    ) -> Result<Value, String> {
        match self.defer_task_scheduler(task_id, params, blocked_stage)? {
            Some(placeholder) => Ok(placeholder),
            None => self.task_view_full(task_id, thread_detail(params)),
        }
    }

    /// Advance one Task's scheduler and hand whatever git it owes to the
    /// drain. `Some` is the placeholder the drain replaces with the job's own
    /// answer; `None` means the pass settled here and the caller answers.
    ///
    /// Every caller that HAS a drain comes through here — a frame asking for
    /// an implementation, a stage approval that wakes a parked scheduler, an
    /// agent's own recovery report — so no request cuts a checkout under the
    /// app mutex. A refusal before any git blocks the scheduler, the same way
    /// the job's own refusal does.
    pub(in crate::app) fn defer_task_scheduler(
        &mut self,
        task_id: &str,
        request: &Value,
        blocked_stage: Option<String>,
    ) -> Result<Option<Value>, String> {
        match self.advance_task_scheduler(task_id, request) {
            Ok(Some(job)) => Ok(Some(self.defer_job(job))),
            Ok(None) => Ok(None),
            Err(error) => {
                self.block_task_scheduler(task_id, blocked_stage, &error);
                Err(error)
            }
        }
    }

    pub(crate) fn task_implement_stage(&mut self, params: &Value) -> Result<Value, String> {
        let task_id = require_str(params, "task_id")?;
        let stage_id = require_str(params, "stage_id")?;
        let task = self.plans.get(&task_id).ok_or("unknown task_id")?;
        let index = task
            .stages
            .iter()
            .position(|stage| stage.id == stage_id)
            .ok_or_else(|| format!("unknown stage_id: {stage_id}"))?;
        if task.stages[index].state != StageDocState::Approved {
            return Err(format!("stage {stage_id} is not approved"));
        }
        if self.current_task_implementation_id(&task_id).is_none() && index != 0 {
            return Err(format!(
                "cannot implement stage {stage_id}: no current implementation contains its completed predecessors"
            ));
        }
        self.arm_task_scheduler(&task_id, ImplementationIntent::Stage(stage_id.clone()))?;
        self.implement_task(&task_id, params, Some(stage_id))
    }

    /// Persist scheduler intent before any worktree/git/agent side effect. The
    /// Task record is the recovery journal; run.auto_advance is only the live
    /// implementation's execution flag.
    pub(in crate::app) fn arm_task_scheduler(
        &mut self,
        task_id: &str,
        intent: ImplementationIntent,
    ) -> Result<(), String> {
        let mut task = self.take_plan(task_id)?;
        if task.plan.state != PlanState::Approved {
            self.plans.insert(task_id.to_string(), task);
            return Err("only a ready Task can be implemented".to_string());
        }
        task.plan.implementation_intent = intent;
        task.plan.implementation_activity = ImplementationActivity::Preparing;
        self.finish_plan_mutation(task_id.to_string(), task)
    }

    pub(in crate::app) fn set_task_scheduler_activity(
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

    pub(in crate::app) fn block_task_scheduler(
        &mut self,
        task_id: &str,
        stage_id: Option<String>,
        reason: &str,
    ) {
        let stage_id = stage_id
            .or_else(|| {
                self.plans
                    .get(task_id)
                    .and_then(|task| task.stages.first())
                    .map(|stage| stage.id.clone())
            })
            .unwrap_or_default();
        if let Err(error) = self.set_task_scheduler_activity(
            task_id,
            None,
            ImplementationActivity::Blocked {
                stage_id,
                reason: reason.to_string(),
            },
        ) {
            eprintln!("task scheduler {task_id}: could not persist failure: {error}");
        }
    }

    /// Reconcile one Task's durable intent with its implementation lineage.
    /// This is deliberately idempotent: boot, approval, and completion may all
    /// call it, but the single-active-writer gate prevents duplicate checkouts.
    ///
    /// What comes back is the git the next step needs — cutting the checkout,
    /// or putting back one that was deleted — reserved but not yet run. The
    /// job's own epilogue carries on from where this stopped, so the caller
    /// decides only where the git runs, never what happens after it.
    pub(in crate::app) fn advance_task_scheduler(
        &mut self,
        task_id: &str,
        request: &Value,
    ) -> Result<Option<WorktreeLifecycleJob>, String> {
        ensure_task_scheduler_available()?;
        let intent = self
            .plans
            .get(task_id)
            .ok_or("unknown task_id")?
            .plan
            .implementation_intent
            .clone();
        if intent == ImplementationIntent::None {
            return Ok(None);
        }

        let target_stage = match &intent {
            ImplementationIntent::Stage(stage_id) => Some(stage_id.clone()),
            ImplementationIntent::All => next_unsettled_stage(
                &self.plans[task_id].stages,
                self.current_task_implementation(task_id),
            )
            .map(|doc| doc.id.clone()),
            ImplementationIntent::None => None,
        };
        let Some(target_stage) = target_stage else {
            return self
                .set_task_scheduler_activity(
                    task_id,
                    Some(ImplementationIntent::None),
                    ImplementationActivity::Idle,
                )
                .map(|()| None);
        };
        let approved = self.plans[task_id]
            .stages
            .iter()
            .find(|stage| stage.id == target_stage)
            .is_some_and(|stage| stage.state == StageDocState::Approved);
        if !approved {
            return self
                .set_task_scheduler_activity(
                    task_id,
                    None,
                    ImplementationActivity::WaitingApproval(target_stage),
                )
                .map(|()| None);
        }

        // No implementation yet: cutting its checkout is the next step, and
        // the job's epilogue resumes this scheduler on the run it opened.
        let Some(run_id) = self.current_task_implementation_id(task_id) else {
            self.set_task_scheduler_activity(task_id, None, ImplementationActivity::Preparing)?;
            let waiting = self.task_scheduler_waiting_on(task_id, &intent);
            return self
                .open_implementation(task_id, request, waiting)
                .map(Some);
        };

        let waiting = self.task_scheduler_waiting_on(task_id, &intent);
        if let Some(job) = self.ensure_task_implementation_worktree(task_id, &run_id, waiting)? {
            return Ok(Some(job));
        }
        self.dispatch_ready_stage(task_id, &run_id, request)
            .map(|()| None)
    }

    /// Who the scheduler is: what it hears when the checkout it is waiting on
    /// exists, and which stage it marks blocked if that checkout never comes.
    pub(in crate::app) fn task_scheduler_waiting_on(
        &self,
        task_id: &str,
        intent: &ImplementationIntent,
    ) -> Box<dyn ImplementationCaller> {
        Box::new(TaskSchedulerWaiting {
            task_id: task_id.to_string(),
            blocked_stage: match intent {
                ImplementationIntent::Stage(stage_id) => Some(stage_id.clone()),
                ImplementationIntent::All | ImplementationIntent::None => None,
            },
        })
    }

    /// The rest of one scheduler pass, once the implementation's checkout is on
    /// disk: dispatch the stage the intent named, or arm run-all and let the
    /// run chain through the stages itself.
    ///
    /// This is where a pass that had to stop for git resumes — the job's
    /// epilogue calls it with the run the git settled.
    pub(in crate::app) fn dispatch_ready_stage(
        &mut self,
        task_id: &str,
        run_id: &str,
        request: &Value,
    ) -> Result<(), String> {
        let intent = self
            .plans
            .get(task_id)
            .ok_or("unknown task_id")?
            .plan
            .implementation_intent
            .clone();
        let run_id = run_id.to_string();
        match intent {
            ImplementationIntent::Stage(stage_id) => {
                let already_started = self
                    .runs
                    .get(&run_id)
                    .ok_or("unknown run_id")?
                    .stage_progress(&stage_id)
                    .is_some();
                if !already_started {
                    let mut params = request.clone();
                    let object = params
                        .as_object_mut()
                        .ok_or("task params must be an object")?;
                    object.insert("run_id".to_string(), json!(run_id));
                    object.insert("stage_id".to_string(), json!(stage_id));
                    self.run_stage_dispatch(&params)?;
                }
                self.set_task_scheduler_activity(
                    task_id,
                    Some(ImplementationIntent::None),
                    ImplementationActivity::Idle,
                )
            }
            ImplementationIntent::All => {
                let mut params = request.clone();
                let object = params
                    .as_object_mut()
                    .ok_or("task params must be an object")?;
                object.insert("run_id".to_string(), json!(run_id.clone()));
                object.insert("enabled".to_string(), json!(true));
                self.run_set_auto_advance(&params)?;
                self.refresh_task_scheduler_activity(task_id)
            }
            ImplementationIntent::None => Ok(()),
        }
    }

    /// Make sure the implementation's checkout is where its run says it is.
    /// `None` means it already was; a job means it is being put back with
    /// `git worktree add` — and, when the branch is only on a remote, a fetch —
    /// and the scheduler carries on from that job's epilogue.
    pub(in crate::app) fn ensure_task_implementation_worktree(
        &mut self,
        task_id: &str,
        run_id: &str,
        caller: Box<dyn ImplementationCaller>,
    ) -> Result<Option<WorktreeLifecycleJob>, String> {
        let (adopted, checkout_stood, worktree, title) = {
            let active = self
                .runs
                .get(run_id)
                .ok_or_else(|| format!("unknown run_id: {run_id}"))?;
            (
                active.adopted,
                active.worktree.path.exists(),
                active.worktree.clone(),
                active.run.goal.clone(),
            )
        };
        // An adopted checkout is somebody else's directory: Build never cut it,
        // so it cannot cut it again. Standing is all this can ask of one.
        if adopted {
            if checkout_stood {
                return Ok(None);
            }
            return Err(
                "Build cannot recreate an adopted worktree that is missing. \
                        Restore the checkout, then try again."
                    .to_string(),
            );
        }
        let project_id = self.project_of(run_id)?;
        let resolved = self.resolved_isolation(&project_id);
        let project = self.orch_for(&project_id)?.clone();
        let mut row = PendingRow::creating(run_id.to_string(), Some(project_id), title)
            .on_checkout(crate::worktree::external_worktree_id(&worktree.path))
            .implementing(task_id.to_string());
        // A checkout that is still standing is verified and reused, not made,
        // and what is on disk describes itself: only a restore that has to put
        // one back names the isolation it is putting back.
        if !checkout_stood {
            row = row.isolated_as(resolved.isolation);
        }
        self.reserve_lifecycle(
            row,
            RestoreImplementationCheckout {
                project,
                worktree,
                checkout_stood,
                resolved,
            },
            crate::app::runtime::lifecycle::RestoreImplementationSettlement {
                task_id: task_id.to_string(),
                run_id: run_id.to_string(),
                caller,
            },
        )
        .map(Some)
    }

    pub(in crate::app) fn refresh_task_scheduler_activity(
        &mut self,
        task_id: &str,
    ) -> Result<(), String> {
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
        self.set_task_scheduler_activity(task_id, intent, activity)
    }

    pub(crate) fn task_set_auto_advance(&mut self, params: &Value) -> Result<Value, String> {
        let task_id = require_str(params, "task_id")?;
        let run_id = self
            .current_task_implementation_id(&task_id)
            .ok_or("task has no active implementation")?;
        let mut run_params = params.clone();
        run_params
            .as_object_mut()
            .ok_or("task params must be an object")?
            .insert("run_id".to_string(), json!(run_id));
        self.run_set_auto_advance(&run_params)?;
        self.task_view_full(&task_id, thread_detail(params))
    }

    pub(crate) fn task_stage_diff(&mut self, params: &Value) -> Result<Value, String> {
        let task_id = require_str(params, "task_id")?;
        let stage_id = require_str(params, "stage_id")?;
        // Resolve the lineage that actually owns this immutable boundary, not
        // merely the newest attempt. A later failed/restarted implementation
        // must not hide a completed stage from an earlier retained lineage.
        let mut lineages = self
            .runs
            .values()
            .filter(|run| run.run.plan_id.as_ref().map(|id| id.0.as_str()) == Some(&task_id))
            .filter(|run| {
                run.stage_progress(&stage_id).is_some_and(|progress| {
                    progress.start_sha.is_some() && progress.completion_sha.is_some()
                })
            })
            .collect::<Vec<_>>();
        lineages.sort_by_key(|run| {
            self.board
                .attention()
                .clock(&run.run.id.0)
                .created_at
                .unwrap_or_default()
        });
        let run_id = lineages
            .last()
            .map(|run| run.run.id.0.clone())
            .or_else(|| {
                self.current_task_implementation(&task_id)
                    .map(|run| run.run.id.0.clone())
            })
            .ok_or("task has no implementation lineage")?;
        let mut run_params = params.clone();
        run_params
            .as_object_mut()
            .ok_or("task params must be an object")?
            .insert("run_id".to_string(), json!(run_id));
        self.plan_run_stage_diff(&run_params, Some(task_id))
    }

    pub(crate) fn task_run_action(
        &mut self,
        params: &Value,
        action: &str,
    ) -> Result<Value, String> {
        let task_id = require_str(params, "task_id")?;
        let run_id = self
            .current_task_implementation_id(&task_id)
            .ok_or("task has no active implementation")?;
        let mut run_params = params.clone();
        let object = run_params
            .as_object_mut()
            .ok_or("task params must be an object")?;
        object.insert("run_id".to_string(), json!(run_id));
        // An `agent_id` a Task surface sends names the TASK's one agent,
        // which is not on the implementation's roster: the implementation agent
        // it maps to is that run's first, which is what the verb defaults to.
        object.remove("agent_id");
        match action {
            "diff" => return self.plan_run_diff(&run_params, Some(task_id)),
            "request_changes" => self.run_request_changes(&run_params)?,
            "git_action" => self.run_git_action(&run_params)?,
            _ => unreachable!("known task run action"),
        };
        self.task_view_full(&task_id, thread_detail(params))
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

use crate::app::{
    comment_json, require_str, run_state_str, view_thread_detail, AppState, DigestScope,
};
use crate::orchestrator::{ActivePlan, ActiveRun};
use crate::plan::{PlanState, StageDoc, StageDocState};
use crate::run::{RunState, StageProgress, StageProgressState, StagePublication};
use crate::thread::ThreadDetail;
use serde_json::{json, Value};

/// The wire view of a plan stage doc: id/title/summary/path, its plan-side
/// review sub-state, and its open-comment count.
pub(in crate::app) fn plan_stage_json(active: &ActivePlan, doc: &StageDoc) -> Value {
    let open_comments = active
        .agents
        .sole_thread()
        .open_doc_comments_for(&doc.id)
        .len();
    json!({
        "id": doc.id,
        "title": doc.title,
        "summary": doc.summary,
        "path": doc.path,
        "state": stage_doc_state_str(&doc.state),
        "open_comments": open_comments,
    })
}

/// The first stage a Task still owes work on: the earliest one this run has
/// recorded no progress against, or that has not completed, or whose
/// completion a later change invalidated. `None` once every stage of the
/// manifest has settled. A run that does not exist yet has settled nothing, so
/// the first stage is the answer.
///
/// One predicate, three readers — boot's activity reconstruction, the
/// activity a report refreshes, and the Task's rendered activity — because
/// what counts as settled has to move for all of them at once.
pub(in crate::app) fn next_unsettled_stage<'a>(
    stages: &'a [StageDoc],
    run: Option<&ActiveRun>,
) -> Option<&'a StageDoc> {
    stages.iter().find(|doc| {
        run.and_then(|run| run.stage_progress(&doc.id))
            .is_none_or(|progress| {
                progress.state != StageProgressState::Completed
                    || progress.invalidation_reason.is_some()
            })
    })
}

/// The first stage `run.stage_dispatch` would currently accept for a run: the
/// earliest stage not yet complete — provided its plan doc is `Approved` and
/// every earlier stage already completed on this run.
/// `None` when nothing is dispatchable right now (mirrors `dispatch_run_stage`).
pub(in crate::app) fn dispatchable_next_run_stage(
    run: &ActiveRun,
    plan_docs: &[StageDoc],
) -> Option<String> {
    let passed = |stage_id: &str| {
        run.stage_progress(stage_id).map(|p| p.state) == Some(StageProgressState::Completed)
    };
    for (index, doc) in plan_docs.iter().enumerate() {
        if passed(&doc.id) {
            continue;
        }
        return (doc.state == StageDocState::Approved
            && plan_docs[..index].iter().all(|d| passed(&d.id)))
        .then(|| doc.id.clone());
    }
    None
}

/// The wire string for a plan state (snake_case, matching the SPA's buckets).
pub(in crate::app) fn plan_state_str(state: &PlanState) -> String {
    match state {
        PlanState::Created => "created",
        PlanState::Drafting => "drafting",
        PlanState::PlanReview => "plan_review",
        PlanState::Approved => "approved",
        PlanState::Blocked => "blocked",
        PlanState::Failed => "failed",
        PlanState::IdleUnreported => "idle_unreported",
        PlanState::Interrupted => "interrupted",
        PlanState::Abandoned => "abandoned",
    }
    .to_string()
}

/// The wire string for a plan-side stage doc state.
pub(in crate::app) fn stage_doc_state_str(state: &StageDocState) -> String {
    match state {
        StageDocState::Planned => "planned",
        StageDocState::Approved => "approved",
    }
    .to_string()
}

pub(in crate::app) fn canonical_stage_execution(progress: &StageProgress) -> &'static str {
    match progress.state {
        _ if progress.invalidation_reason.is_some() => "incomplete",
        StageProgressState::Building => "building",
        StageProgressState::Completed if progress.completion_sha.is_some() => "complete",
        StageProgressState::Completed => "legacy_unpinned",
    }
}

impl AppState {
    pub(crate) fn plan_get(&mut self, params: &Value) -> Result<Value, String> {
        let plan_id = require_str(params, "plan_id")?;
        let active = self.plans.get(&plan_id).ok_or("unknown plan_id")?;
        // See `run_get`. A task carries exactly one agent, so naming it is a
        // check rather than a choice — but the check still holds.
        let detail_thread = self.detail_thread_value(&plan_id, params)?;
        let mut view = self.plan_view(
            &plan_id,
            active,
            view_thread_detail(&detail_thread, params),
            DigestScope::Detail,
        );
        if let Some(thread) = detail_thread {
            view.as_object_mut()
                .expect("plan_view returns an object")
                .insert("thread".to_string(), thread);
        }
        Ok(view)
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

    pub(crate) fn task_stages(&mut self, params: &Value) -> Result<Value, String> {
        let task_id = require_str(params, "task_id")?;
        let task = self.plans.get(&task_id).ok_or("unknown task_id")?;
        // A task with no stages yet answers with the empty list that is the
        // truth: the surface asks task.get and task.stages together on every
        // poll, and refusing here left a fresh task's page loading forever.
        let implementation = self.current_task_implementation(&task_id);
        let stages = task
            .stages
            .iter()
            .map(|doc| {
                let progress = implementation.and_then(|run| run.stage_progress(&doc.id));
                let mut stage = plan_stage_json(task, doc);
                let object = stage
                    .as_object_mut()
                    .expect("plan_stage_json returns an object");
                object.insert(
                    "approval".to_string(),
                    json!(stage_doc_state_str(&doc.state)),
                );
                object.insert(
                    "execution".to_string(),
                    json!(progress.map_or("pending", canonical_stage_execution)),
                );
                object.insert(
                    "start_sha".to_string(),
                    json!(progress.and_then(|p| p.start_sha.as_ref())),
                );
                object.insert(
                    "built_sha".to_string(),
                    json!(progress.and_then(|p| p.built_sha.as_ref())),
                );
                object.insert(
                    "completion_sha".to_string(),
                    json!(progress.and_then(|p| p.completion_sha.as_ref())),
                );
                object.insert(
                    "publication".to_string(),
                    json!(progress.map(|p| p.publication)),
                );
                object.insert(
                    "invalidation_reason".to_string(),
                    json!(progress.and_then(|p| p.invalidation_reason.as_ref())),
                );
                object.insert(
                    "comments".to_string(),
                    json!(task
                        .agents
                        .sole_thread()
                        .doc_comments()
                        .iter()
                        .filter(|comment| comment.stage_id == doc.id)
                        .map(comment_json)
                        .collect::<Vec<_>>()),
                );
                stage
            })
            .collect::<Vec<_>>();
        Ok(json!({
            "task_id": task_id,
            "plan_id": task_id,
            "implementation_id": implementation.map(|run| run.run.id.0.clone()),
            "auto_advance": implementation.is_some_and(|run| run.auto_advance),
            "stages": stages,
        }))
    }

    pub(in crate::app) fn current_task_implementation_id(&self, task_id: &str) -> Option<String> {
        self.current_task_implementation(task_id)
            .filter(|run| !run.run.state.is_terminal())
            .map(|run| run.run.id.0.clone())
    }

    pub(in crate::app) fn current_task_implementation(&self, task_id: &str) -> Option<&ActiveRun> {
        let mut implementations = self
            .runs
            .values()
            .filter(|run| run.run.plan_id.as_ref().map(|id| id.0.as_str()) == Some(task_id))
            .collect::<Vec<_>>();
        implementations.sort_by_key(|run| {
            self.board
                .attention()
                .clock(&run.run.id.0)
                .created_at
                .unwrap_or_default()
        });
        implementations
            .iter()
            .rev()
            .find(|run| !run.run.state.is_terminal())
            .copied()
            .or_else(|| implementations.last().copied())
    }

    /// The run agent currently executing a Task's canonical conversation.
    /// The UI addresses chat and settings to this identity while retaining the
    /// Task conversation id as a stale-binding guard. Once the alias agent is
    /// removed there is no replacement: a private secondary never inherits it.
    pub(in crate::app) fn task_execution_context(&self, task_id: &str) -> Option<Value> {
        let conversation_id = self.plans.get(task_id)?.agents.sole().conversation_id();
        let run = self
            .current_task_implementation(task_id)
            .filter(|run| !run.run.state.is_terminal())?;
        let agent = run
            .agents
            .iter()
            .find(|agent| agent.conversation_id() == conversation_id)?;
        let root = Some(AppState::canonical_root(&run.worktree.path));
        Some(json!({
            "entity_id": run.run.id.0,
            "agent_id": agent.id,
            "conversation_id": conversation_id,
            "agent": self.agent_digest(
                &run.run.id.0,
                agent,
                root.as_deref(),
                DigestScope::List,
            ),
        }))
    }

    pub(in crate::app) fn plan_view(
        &self,
        plan_id: &str,
        active: &ActivePlan,
        thread_detail: ThreadDetail,
        scope: DigestScope,
    ) -> Value {
        let project_id = self
            .projects
            .project_id_of(plan_id)
            .unwrap_or_default()
            .to_string();
        let project = self
            .projects
            .iter()
            .find(|p| p.id == project_id)
            .map(|p| p.name.clone())
            .unwrap_or_default();
        let active_run_id = self.runs.iter().find_map(|(id, run)| {
            (run.run.plan_id.as_ref().map(|p| &p.0) == Some(&plan_id.to_string())
                && !run.run.state.is_terminal())
            .then(|| id.clone())
        });
        let implementation_complete = self.plan_implementation_complete(plan_id, active);
        let current_implementation = self.current_task_implementation(plan_id);
        // Narrower than the newest implementation: a merged or abandoned branch
        // has stopped speaking for its task.
        let live_implementation = current_implementation.filter(|run| !run.run.state.is_terminal());
        let execution_context = self.task_execution_context(plan_id);
        let mut implementation_lineage = self
            .runs
            .values()
            .filter(|run| run.run.plan_id.as_ref().map(|id| id.0.as_str()) == Some(plan_id))
            .map(|run| {
                json!({
                    "implementation_id": run.run.id.0,
                    "run_id": run.run.id.0,
                    "state": run_state_str(&run.run.state),
                    "branch": run.worktree.branch(),
                    "worktree_path": run.worktree.path.display().to_string(),
                    "created_at": self.board.attention().clock(&run.run.id.0).created_at,
                })
            })
            .collect::<Vec<_>>();
        implementation_lineage.sort_by_key(|implementation| {
            implementation["created_at"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        });
        let unread = self.unread_for(plan_id, Some(active.agents.sole_thread()));
        json!({
            "task_id": plan_id,
            "plan_id": plan_id,
            "goal": active.plan.goal,
            "state": plan_state_str(&active.plan.state),
            // Event-driven, and `needs_attention` is the same fact under the
            // name the SPA already reads.
            "needs_attention": unread.is_unread(),
            "unread": unread.is_unread(),
            "unread_count": unread.count,
            "unread_reason": unread.reason,
            // See `run_view`.
            "muted": self.is_muted(plan_id),
            "dismissed": self.is_dismissed(plan_id),
            "attention": self.attention_json(plan_id),
            "summary": active.last_summary,
            "last_error": active.last_error,
            "project": project,
            "project_id": project_id,
            "base_branch": active.base_branch,
            "plan_path": active.plan_path,
            "harness": if self.qa_agent { self.harness.as_str() } else { active.model_choice.provider.label() },
            "provider": active.model_choice.provider,
            "model": active.model_choice.model,
            "effort": active.model_choice.effort,
            "thread": match thread_detail {
                ThreadDetail::Digest => active.agents.sole_thread().digest_value(),
                ThreadDetail::Full => active.agents.sole_thread().wire_value(),
                ThreadDetail::Page(limit) => self.first_thread_page(active.agents.sole_thread(), limit),
            },
            // The rail's bubble strip: one entry per agent, on every surface
            // that renders an entity, so status stays legible fully collapsed.
            "agents": self.agent_digests(plan_id, scope),
            "execution_context": execution_context,
            "active_run_id": active_run_id,
            // Whether a branch is implementing this task RIGHT NOW, and which
            // one. The same fact that hides the task's row behind that
            // branch's in the feed, said out loud: a task that has gone quiet
            // because something is being built for it must be able to say so
            // rather than simply vanish.
            "implementation_active": live_implementation.is_some(),
            "implementing_branch": live_implementation.map(|run| run.worktree.branch()),
            "current_implementation_id": current_implementation.map(|run| run.run.id.0.clone()),
            "current_implementation": current_implementation.map(|run| json!({
                "implementation_id": run.run.id.0,
                "run_id": run.run.id.0,
                "state": run_state_str(&run.run.state),
                "branch": run.worktree.branch(),
                "worktree_path": run.worktree.path.display().to_string(),
            })),
            "implementation_lineage": implementation_lineage,
            "implementation_intent": active.plan.implementation_intent,
            "implementation_activity": active.plan.implementation_activity,
            "implementation_complete": implementation_complete,
            // Done on a task archives it, whatever was or was not built for
            // it: a task only stops being archivable once it already is.
            "can_archive": active.plan.archived_at.is_none(),
            // …and what archiving would gloss over rides along, so the surface
            // that offers Done can say it before the user confirms.
            "finish": { "warnings": crate::branch::warnings_json(
                &crate::branch::task_finish_warnings(current_implementation.is_some()),
            ) },
            "archived_at": active.plan.archived_at,
            // False when the store holds no docs (a migrated plan whose docs
            // were unrecoverable): the client disables doc reads + Implement
            // instead of retrying reads that can never succeed.
            "docs_available": self
                .store
                .as_ref()
                .is_some_and(|store| store.has_plan_docs(plan_id)),
            "created_at": self.board.attention().clock(plan_id).created_at,
            "updated_at": self.board.attention().clock(plan_id).updated_at,
            "state_changed_at": self.board.attention().clock(plan_id).state_changed_at,
            "stages": active
                .stages
                .iter()
                .map(|doc| plan_stage_json(active, doc))
                .collect::<Vec<_>>(),
        })
    }

    /// Server-derived implementation completion. Multi-stage completion must
    /// be proven by one linked run that passed every manifest stage and reached
    /// Review/Merged; single-doc plans only need that run-level human gate.
    pub(in crate::app) fn plan_implementation_complete(
        &self,
        plan_id: &str,
        plan: &ActivePlan,
    ) -> bool {
        self.runs.values().any(|run| {
            if run.run.plan_id.as_ref().map(|id| id.0.as_str()) != Some(plan_id) {
                return false;
            }
            let passed_human_gate = matches!(run.run.state, RunState::Review | RunState::Merged);
            if plan.stages.is_empty() {
                return passed_human_gate;
            }
            matches!(
                run.run.state,
                RunState::Review | RunState::Merged | RunState::Archived
            ) && plan.stages.iter().all(|stage| {
                run.stages.iter().any(|progress| {
                    progress.stage_id == stage.id
                        && progress.state == StageProgressState::Completed
                        && (progress.completion_sha.is_some()
                            || progress.publication == StagePublication::LegacyUnknown)
                        && progress.invalidation_reason.is_none()
                })
            })
        })
    }

    pub(in crate::app) fn plan_revision_contents(
        &self,
        plan_id: &str,
        active: &ActivePlan,
    ) -> Option<String> {
        let store = self.store.as_ref()?;
        if active.stages.is_empty() {
            return store.read_plan_doc(plan_id, &active.plan_path);
        }
        let mut combined = String::new();
        for stage in &active.stages {
            let contents = store.read_plan_doc(plan_id, &stage.path)?;
            combined.push_str(&stage.path);
            combined.push('\n');
            combined.push_str(&contents);
            combined.push('\n');
        }
        Some(combined)
    }
}

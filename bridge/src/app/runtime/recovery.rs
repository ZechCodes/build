#[cfg(test)]
use crate::app::OffLockGate;
use crate::app::{
    err, finish_git_steps_are_complete, next_unsettled_stage, plan_state_str,
    recovery_agent_prompt, run_state_str, AppState, ImplementationCaller, OffLockJob,
    PendingAgentTurn, SESSION_DIED_SUMMARY,
};
use crate::mcp::{DoneReport, DoneStatus};
use crate::operation::{OperationReceipt, OperationStatus};
use crate::orchestrator::{ActivePlan, ActiveRun};
use crate::plan::{ImplementationIntent, PlanEvent};
use crate::run::{RunEvent, RunState, StageProgressState, StagePublication};
use crate::store::{
    now_rfc3339, PersistedArchivedWorktree, PersistedPlan, PersistedRun, Store,
    WorktreeFinishStatus,
};
use serde_json::{json, Value};
use std::collections::HashMap;

pub(in crate::app) struct StoredTasks {
    pub(in crate::app) store: Store,
    pub(in crate::app) plans: Vec<PersistedPlan>,
    pub(in crate::app) runs: Vec<PersistedRun>,
    pub(in crate::app) archived_worktrees: Vec<PersistedArchivedWorktree>,
    pub(in crate::app) captures: Vec<crate::capture::Capture>,
    pub(in crate::app) attention: HashMap<String, crate::attention::Attention>,
    pub(in crate::app) operations: Vec<OperationReceipt>,
}

pub(in crate::app) use crate::lifecycle::{
    classify_stage_publication, StagePublicationQuery, StagePublications,
};

/// One vanished run, with git's verdict on its stages already in hand.
pub(in crate::app) struct DecidedVanishedRun {
    pub(in crate::app) run_id: String,
    pub(in crate::app) published: StagePublications,
}

impl DecidedVanishedRun {
    pub(in crate::app) fn decide(query: StagePublicationQuery) -> DecidedVanishedRun {
        DecidedVanishedRun {
            published: query.classify(),
            run_id: query.run_id,
        }
    }
}

/// The vanished runs one board read found, on their way to a verdict.
pub(in crate::app) struct VanishedRunSweep {
    pub(in crate::app) queries: Vec<StagePublicationQuery>,
    #[cfg(test)]
    pub(in crate::app) gate: Option<OffLockGate>,
}

impl OffLockJob for VanishedRunSweep {
    type Claim = ();
    type Decided = Vec<DecidedVanishedRun>;

    fn claim(&mut self) {}

    /// The git half — a bounded fetch and two graph walks per completed stage,
    /// per run.
    fn decide(self) -> Vec<DecidedVanishedRun> {
        #[cfg(test)]
        if let Some(gate) = self.gate {
            gate.arrive();
        }
        self.queries
            .into_iter()
            .map(DecidedVanishedRun::decide)
            .collect()
    }

    fn apply(state: &mut AppState, (): (), decided: Vec<DecidedVanishedRun>) {
        state.archive_vanished_runs(decided);
    }

    fn abandon(state: &mut AppState, (): ()) {
        state.vanished_run_sweep_in_flight = false;
    }
}

/// Write git's verdict onto a vanished run's stages: one whose commits never
/// left this machine, or that was never validated, is marked incomplete.
/// Returns the stages that were. Pure bookkeeping — the git it judges by is
/// [`StagePublicationQuery::classify`].
pub(in crate::app) fn reconcile_missing_run_worktree(
    active: &mut ActiveRun,
    published: &StagePublications,
) -> Vec<String> {
    let mut affected = Vec::new();
    for progress in &mut active.stages {
        let publication = published.of(&progress.stage_id);
        progress.publication = publication;
        let in_flight = !matches!(
            progress.state,
            StageProgressState::Validated { passed: true }
        );
        if publication == StagePublication::Local || in_flight {
            progress.invalidation_reason = Some(
                "Issue worktree disappeared before this stage's commits were verified pushed or merged"
                    .to_string(),
            );
            affected.push(progress.stage_id.clone());
        }
    }
    affected
}

/// What the checkout's archive record adds to an archived row: how it was
/// finished, and where it stood when it was. Overwrites only the keys it owns,
/// so a run's own facts (title, state, the branch it ran on) survive.
pub(in crate::app) fn merge_archived_worktree_facts(
    row: &mut serde_json::Map<String, Value>,
    record: &PersistedArchivedWorktree,
) {
    row.insert("worktree_id".into(), json!(record.worktree_id));
    row.insert("action".into(), json!(record.action));
    row.insert("head_sha".into(), json!(record.head_sha));
    row.insert("upstream".into(), json!(record.upstream));
    row.insert("unpushed".into(), json!(record.unpushed));
    row.insert("dirty_files".into(), json!(record.dirty_files));
    if row.get("finished_at").is_none_or(Value::is_null) {
        row.insert("finished_at".into(), json!(record.archived_at));
    }
}

pub(in crate::app) fn archived_worktree_json(record: &PersistedArchivedWorktree) -> Value {
    json!({
        "worktree_id": record.worktree_id,
        "name": record.worktree_name,
        "path": record.worktree_path,
        "branch": record.branch,
        "head_sha": record.head_sha,
        "upstream": record.upstream,
        "unpushed": record.unpushed,
        "dirty_files": record.dirty_files,
        "uncommitted": {
            "files_changed": record.uncommitted_files,
            "insertions": record.uncommitted_insertions,
            "deletions": record.uncommitted_deletions,
        },
        "action": record.action,
        "archived_at": record.archived_at,
    })
}

/// The stage a verified recovery is being asked to prove: the one the Issue's
/// durable intent names, the first stage it still owes work on when the intent
/// is the whole Issue, or — with nothing armed — whatever the run was last
/// building.
pub(in crate::app) fn recovery_target_stage(issue: &ActivePlan, active: &ActiveRun) -> String {
    match &issue.plan.implementation_intent {
        ImplementationIntent::Stage(stage_id) => stage_id.clone(),
        ImplementationIntent::All => next_unsettled_stage(&issue.stages, Some(active))
            .map(|doc| doc.id.clone())
            .unwrap_or_default(),
        ImplementationIntent::None => active.current_stage_id.clone().unwrap_or_default(),
    }
}

/// What a conversation says when the agent's PROCESS ended with a turn still in
/// flight: nobody is coming back to hand it over, so the turn is closed here.
///
/// `Interrupted` rather than `IdleUnreported`, because the two differ by whether
/// the agent is still there. `IdleUnreported` reads "went quiet without
/// reporting done" — an agent alive at its prompt with nothing to say. A killed
/// harness is not quiet, it is gone, which is exactly what `Interrupted` already
/// means ("the session did not survive"); boot recovery writes the same event
/// for the same reason after a daemon restart. It is attention-class, and that
/// class is what ENDS a turn — so `working_since` goes `None`, the row and the
/// agent's bubble stop claiming work is happening, and the entry says why.
pub(in crate::app) fn record_session_death_in_thread(
    thread: &mut crate::thread::Thread,
    now: &str,
) {
    thread.push_event(
        crate::thread::ThreadEventKind::Interrupted,
        Some(SESSION_DIED_SUMMARY.to_string()),
        None,
        None,
        now,
    );
}

pub(in crate::app) fn load_stored_tasks(dir: std::path::PathBuf) -> Result<StoredTasks, String> {
    let store = Store::new(dir).map_err(|error| error.to_string())?;
    store
        .refuse_a_rolled_back_store()
        .map_err(|error| error.to_string())?;
    match store.import_json_store() {
        Ok(0) => {}
        Ok(imported) => eprintln!("store: imported {imported} records from the JSON store"),
        Err(error) => return Err(format!("store import failed: {error}")),
    }
    Ok(StoredTasks {
        plans: store.load_all_plans().map_err(|error| error.to_string())?,
        runs: store.load_all_runs().map_err(|error| error.to_string())?,
        archived_worktrees: store
            .load_all_archived_worktrees()
            .map_err(|error| error.to_string())?,
        captures: store
            .load_all_captures()
            .map_err(|error| error.to_string())?,
        attention: store.load_attention(),
        operations: store
            .recover_operations()
            .map_err(|error| error.to_string())?,
        store,
    })
}

/// What a run can prove about its own checkout when git's registration for it
/// is gone. A run Build dispatched works in a checkout
/// [`crate::worktree::WorktreeManager::create`] made and nothing else, so the
/// value the pruned registration carried is known. An adopted run's checkout
/// may be one Build only checked out over somebody's branch, and with the
/// registration gone nothing on disk says which — so it is not restored at
/// all, rather than restored under a guess that could delete the branch.
pub(in crate::app) fn unregistered_restore_for(
    active: &ActiveRun,
) -> crate::worktree::UnregisteredRestore {
    if active.adopted {
        crate::worktree::UnregisteredRestore::Refuse
    } else {
        crate::worktree::UnregisteredRestore::Write(crate::worktree::BranchTeardown::DeletesBranch)
    }
}

impl AppState {
    pub(in crate::app) fn restore_stored_tasks(
        &mut self,
        stored: StoredTasks,
    ) -> Result<(), String> {
        let StoredTasks {
            store,
            plans,
            runs,
            archived_worktrees,
            captures,
            attention,
            operations,
        } = stored;
        self.board.attention_mut().replace_entries(attention);
        self.store = Some(store);
        self.board.replace_archived(
            archived_worktrees
                .into_iter()
                .map(|record| (record.worktree_id.clone(), record))
                .collect(),
        );
        self.recover_captures(captures)?;
        self.recover_completed_worktree_finishes();
        self.restore_plans_before_runs(plans, runs)?;
        self.restore_operations(operations);
        self.seed_conversation_attention_sequences();
        self.seed_anchors_for_records_without_one();
        self.migrate_legacy_dismissals();
        self.close_recovered_working_intervals();
        self.resume_stored_issue_schedulers()
    }

    pub(in crate::app) fn restore_operations(&mut self, operations: Vec<OperationReceipt>) {
        for receipt in operations {
            if receipt.status == OperationStatus::Queued {
                if let Some(turn) = PendingAgentTurn::for_delivery_operation(&receipt) {
                    self.delivery_queue.enqueue(turn);
                }
            }
        }
    }

    pub(in crate::app) fn restore_plans_before_runs(
        &mut self,
        plans: Vec<PersistedPlan>,
        runs: Vec<PersistedRun>,
    ) -> Result<(), String> {
        for record in plans {
            self.recover_plan(record)?;
        }
        for record in runs {
            self.recover_run(record)?;
        }
        Ok(())
    }

    pub(in crate::app) fn resume_stored_issue_schedulers(&mut self) -> Result<(), String> {
        let issue_ids = self
            .plans
            .iter()
            .filter(|(_, issue)| issue.plan.implementation_intent != ImplementationIntent::None)
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();
        for issue_id in issue_ids {
            self.advance_issue_scheduler_here(&issue_id, &json!({ "issue_id": issue_id }))?;
        }
        Ok(())
    }

    /// Re-attach one persisted plan on boot. The canonical docs live in the
    /// store, so a vanished scratch docs dir never abandons or archives a
    /// plan — a plan that was mid-draft simply surfaces `Interrupted` (its
    /// session died with the daemon); the next revision dispatch remakes the
    /// workspace from the store. Recovery never abandons a plan.
    pub(in crate::app) fn recover_plan(&mut self, record: PersistedPlan) -> Result<(), String> {
        let plan_id = record.id.clone();
        let mut active = ActivePlan::reattach(&record);

        let mut state_changed = false;
        if active.plan.state.is_working() {
            // The drafting session died with the daemon; the docs are safe in
            // the store. Surface it interrupted (persisted so the verdict
            // survives the next restart too).
            active
                .plan
                .apply(PlanEvent::Interrupt)
                .map_err(|e| format!("recover {plan_id}: {e}"))?;
            // Say so on the conversation: unread is event-driven, so a parked
            // plan whose session died goes quiet unless the event exists.
            active.agents.sole_thread_mut().push_event(
                crate::thread::ThreadEventKind::Interrupted,
                Some("Build restarted; the drafting session did not survive".to_string()),
                None,
                None,
                now_rfc3339(),
            );
            state_changed = true;
        }

        // Project ids are re-minted each boot, so resolve by repo path. Retain
        // the record's path unconditionally so a parked repo-missing plan keeps
        // a real path to un-park to.
        self.projects
            .retain_entity_path(plan_id.clone(), record.project_path.clone());
        let repo_path = std::path::PathBuf::from(&record.project_path);
        if repo_path.exists() {
            let project_id = self.add_project(repo_path, record.base_branch);
            self.projects.bind_entity(plan_id.clone(), project_id);
        } else {
            // The repo is gone; the plan can't be re-dispatched, but its docs
            // remain readable from the store. Keep it legible with a reason.
            eprintln!(
                "recover {plan_id}: project repo {} is gone; plan kept (docs live in the store)",
                record.project_path
            );
            if !active.plan.state.is_terminal() {
                active.last_error =
                    Some(format!("project repo missing at {}", record.project_path));
                state_changed = true;
            }
        }

        // A boot transition (e.g. drafting → interrupted) is a real state
        // change and stamps now; otherwise keep the record's stamp. Old
        // records carry none — fall back to their updated_at. Seed the
        // last-observed state from the (post-recovery) plan so the first
        // post-boot mutation in the same state doesn't false-stamp.
        let state_changed_at = if state_changed {
            now_rfc3339()
        } else {
            record
                .state_changed_at
                .unwrap_or_else(|| record.updated_at.clone())
        };
        self.board.attention_mut().restore_entity_clocks(
            plan_id.clone(),
            record.created_at,
            record.updated_at,
            state_changed_at,
            plan_state_str(&active.plan.state),
        );
        if state_changed {
            self.persist_plan_record(&plan_id, &active)?;
        }
        self.plans.insert(plan_id, active);
        Ok(())
    }

    /// Re-attach one persisted run on boot. The worktree survived on disk (or
    /// it did not); the PTY session died with the previous daemon. A working
    /// state becomes `Interrupted`; a non-terminal run whose worktree vanished
    /// is `Abandoned` (worktree gone, branch kept — exactly what `Abandoned`
    /// means); the repo-missing arms park an adopted run needs-attention and
    /// abandon a native one, both with a reason.
    #[allow(clippy::cognitive_complexity)] // ratchet: recover_run is at 29, threshold 15 — bring it under, then remove
    pub(in crate::app) fn recover_run(&mut self, record: PersistedRun) -> Result<(), String> {
        let run_id = record.id.clone();
        // Re-derive `plan_path` from the owning plan (recovered first); adopted
        // runs and orphaned links fall back to the convention default.
        let plan_path = record
            .plan_id
            .as_ref()
            .and_then(|pid| self.plans.get(pid))
            .map(|plan| plan.plan_path.clone())
            .unwrap_or_else(|| crate::templates::DEFAULT_PLAN_PATH.to_string());
        let mut active = ActiveRun::reattach(&record, plan_path);

        self.projects
            .retain_entity_path(run_id.clone(), record.project_path.clone());
        let repo_path = std::path::PathBuf::from(&record.project_path);
        let project_id = if repo_path.exists() {
            let project_id = self.add_project(repo_path.clone(), record.base_branch.clone());
            self.projects
                .bind_entity(run_id.clone(), project_id.clone());
            Some(project_id)
        } else {
            None
        };

        let mut state_changed = false;
        let mut recovery_event = None;

        // Resolve an interrupted publication from repository evidence before
        // considering worktree recovery. The write-ahead record names the exact
        // candidate, and classification refreshes its configured remote ref.
        if let Some(attempt) = active.publication_attempt.clone() {
            let publication = project_id
                .as_deref()
                .and_then(|id| self.orch_for(id).ok())
                .map_or(StagePublication::Local, |orch| {
                    classify_stage_publication(
                        orch.worktrees(),
                        &active.worktree.path,
                        &active.worktree.branch(),
                        &active.worktree.base_branch,
                        &attempt.candidate_sha,
                    )
                });
            let proven = match attempt.action.as_str() {
                "push" => matches!(
                    publication,
                    StagePublication::Pushed | StagePublication::Merged
                ),
                "merge" | "merge_push" => publication == StagePublication::Merged,
                _ => false,
            };
            if proven {
                for progress in &mut active.stages {
                    if progress.completion_sha.is_some() {
                        progress.publication = publication;
                        progress.invalidation_reason = None;
                    }
                }
                if matches!(attempt.action.as_str(), "merge" | "merge_push") {
                    active.run.state = RunState::Merged;
                }
                active.publication_attempt = None;
                active.last_error = None;
                recovery_event = Some((
                    if publication == StagePublication::Merged {
                        crate::thread::ThreadEventKind::Merged
                    } else {
                        crate::thread::ThreadEventKind::Pushed
                    },
                    format!(
                        "Recovered interrupted {} from verified refs at {}",
                        attempt.action, attempt.candidate_sha
                    ),
                ));
                state_changed = true;
            } else {
                active.last_error = Some(format!(
                    "interrupted {} is not yet proven by configured refs (candidate {})",
                    attempt.action, attempt.candidate_sha
                ));
                state_changed = true;
            }
        }

        if !active.run.state.is_terminal() && !active.worktree.path.exists() {
            let restored = project_id
                .as_deref()
                .filter(|_| !active.adopted)
                .ok_or_else(|| "the original project/branch is unavailable".to_string())
                .and_then(|project_id| {
                    let resolved = self.resolved_isolation(project_id);
                    self.orch_for(project_id)?
                        .restore_run_worktree(
                            &active.worktree,
                            unregistered_restore_for(&active),
                            resolved.isolation,
                        )
                        .map(|worktree| (worktree, resolved.downgrade))
                        .map_err(err)
                });
            match restored {
                Ok((worktree, downgrade)) => {
                    active.worktree = worktree;
                    if let Some(reason) = downgrade {
                        self.note_isolation_downgrade(&run_id, &mut active, &reason);
                    }
                    recovery_event = Some((
                        crate::thread::ThreadEventKind::WorktreeRecreated,
                        format!(
                            "Recreated the Issue worktree from branch {}",
                            active.worktree.branch()
                        ),
                    ));
                    state_changed = true;
                }
                Err(error) if active.run.plan_id.is_some() && project_id.is_some() => {
                    let issue_id = active.run.plan_id.as_ref().expect("guarded").0.clone();
                    let existing = active
                        .recovery
                        .as_ref()
                        .filter(|attempt| attempt.state == crate::run::RecoveryState::Started)
                        .cloned();
                    let requested_stage_id = existing
                        .as_ref()
                        .map(|attempt| attempt.requested_stage_id.clone())
                        .or_else(|| {
                            self.plans.get(&issue_id).and_then(|issue| {
                                match &issue.plan.implementation_intent {
                                    ImplementationIntent::Stage(stage_id) => Some(stage_id.clone()),
                                    ImplementationIntent::All => {
                                        next_unsettled_stage(&issue.stages, Some(&active))
                                            .map(|doc| doc.id.clone())
                                    }
                                    ImplementationIntent::None => active.current_stage_id.clone(),
                                }
                            })
                        })
                        .unwrap_or_default();
                    let recovery_id = existing
                        .as_ref()
                        .map(|attempt| attempt.id.clone())
                        .unwrap_or_else(|| format!("recovery-{}", uuid::Uuid::new_v4()));
                    let started_at = existing
                        .as_ref()
                        .map(|attempt| attempt.started_at.clone())
                        .unwrap_or_else(now_rfc3339);
                    if existing.is_none() {
                        active.recovery = Some(crate::run::RecoveryAttempt {
                            id: recovery_id.clone(),
                            requested_stage_id: requested_stage_id.clone(),
                            branch: active.worktree.recorded_branch.clone(),
                            state: crate::run::RecoveryState::Started,
                            report: None,
                            started_at: started_at.clone(),
                            completed_at: None,
                        });
                        recovery_event = Some((
                            crate::thread::ThreadEventKind::RecoveryStarted,
                            format!("Verified recovery {recovery_id} started: {error}"),
                        ));
                    }
                    active.last_error = Some(format!(
                        "automatic branch restoration failed: {error}; verified recovery {recovery_id} is running"
                    ));
                    let stages = self
                        .plans
                        .get(&issue_id)
                        .map(|issue| issue.stages.clone())
                        .unwrap_or_default();
                    let prompt = recovery_agent_prompt(
                        &recovery_id,
                        &issue_id,
                        &run_id,
                        &requested_stage_id,
                        &active.worktree,
                        &error,
                        &stages,
                    );
                    match PendingAgentTurn::for_recovery(&run_id, &active, &repo_path, prompt) {
                        Some(turn) => self.delivery_queue.enqueue(turn),
                        None => eprintln!("recover {run_id}: no agent to hand the recovery to"),
                    }
                    state_changed = true;
                }
                Err(error) => {
                    let published = self.classify_stages_now(&run_id, &active);
                    let affected = reconcile_missing_run_worktree(&mut active, &published);
                    active
                        .run
                        .apply(RunEvent::Abandon)
                        .map_err(|e| format!("recover {run_id}: {e}"))?;
                    active.last_error = Some(format!("worktree recovery failed: {error}"));
                    recovery_event = Some((
                        crate::thread::ThreadEventKind::RecoveryFailed,
                        format!(
                            "Could not recover the worktree: {error}. {} stage(s) were marked incomplete",
                            affected.len()
                        ),
                    ));
                    state_changed = true;
                }
            }
        }
        let mut interrupted = false;
        if !active.run.state.is_terminal() && active.run.state.is_working() {
            active
                .run
                .apply(RunEvent::Interrupt)
                .map_err(|e| format!("recover {run_id}: {e}"))?;
            interrupted = true;
            state_changed = true;
        }

        if project_id.is_none() && !active.run.state.is_terminal() {
            if active.adopted {
                // Automated actions never touch an adopted worktree: park the
                // run needs-attention instead of abandoning.
                eprintln!(
                    "recover {run_id}: project repo {} is gone; parking adopted run",
                    record.project_path
                );
                if active.run.state.is_working() {
                    active
                        .run
                        .apply(RunEvent::Interrupt)
                        .map_err(|e| format!("recover {run_id}: {e}"))?;
                    interrupted = true;
                }
                active.last_error =
                    Some(format!("project repo missing at {}", record.project_path));
                state_changed = true;
            } else {
                // No project to route to and no repo to advance on: abandon it
                // so it stays legible with a reason rather than an orphan.
                eprintln!(
                    "recover {run_id}: project repo {} is gone; abandoning",
                    record.project_path
                );
                active
                    .run
                    .apply(RunEvent::Abandon)
                    .map_err(|e| format!("recover {run_id}: {e}"))?;
                active.last_error =
                    Some(format!("project repo missing at {}", record.project_path));
                state_changed = true;
            }
        } else if project_id.is_none() {
            eprintln!(
                "recover {run_id}: project repo {} is gone; run kept as history",
                record.project_path
            );
        }

        if let (Some(issue_id), Some((event, summary))) = (
            active.run.plan_id.as_ref().map(|id| id.0.clone()),
            recovery_event,
        ) {
            if let Ok(mut issue) = self.take_plan(&issue_id) {
                let mut links = vec![crate::thread::ThreadLink::Implementation {
                    issue_id: issue_id.clone(),
                    implementation_id: run_id.clone(),
                }];
                if let Some(recovery) = &active.recovery {
                    links.push(crate::thread::ThreadLink::Recovery {
                        recovery_id: recovery.id.clone(),
                    });
                    if let Some(stage) = issue
                        .stages
                        .iter()
                        .find(|stage| stage.id == recovery.requested_stage_id)
                    {
                        links.push(crate::thread::ThreadLink::IssueStage {
                            issue_id: issue_id.clone(),
                            stage_id: stage.id.clone(),
                            path: stage.path.clone(),
                        });
                    }
                }
                if active.worktree.path.exists() {
                    links.push(crate::thread::ThreadLink::Worktree {
                        worktree_id: crate::worktree::external_worktree_id(&active.worktree.path),
                    });
                }
                issue.agents.sole_thread_mut().push_event_with_links(
                    event,
                    Some(summary),
                    None,
                    None,
                    links,
                    now_rfc3339(),
                );
                let persisted = self.finish_plan_mutation(issue_id, issue);
                persisted?;
            }
        }

        if interrupted {
            self.record_on_run_conversation(&mut active, |thread| {
                thread.push_event(
                    crate::thread::ThreadEventKind::Interrupted,
                    Some("Build restarted; the agent session did not survive".to_string()),
                    None,
                    None,
                    now_rfc3339(),
                );
            })?;
        }

        // Same restore discipline as recover_plan: a boot transition stamps
        // now, otherwise keep the record's stamp (falling back to updated_at
        // for pre-field records); seed last-state from the recovered run.
        let state_changed_at = if state_changed {
            now_rfc3339()
        } else {
            record
                .state_changed_at
                .unwrap_or_else(|| record.updated_at.clone())
        };
        self.board.attention_mut().restore_entity_clocks(
            run_id.clone(),
            record.created_at,
            record.updated_at,
            state_changed_at,
            run_state_str(&active.run.state),
        );
        if state_changed {
            self.persist_run_record(&run_id, &active)?;
        }
        self.runs.insert(run_id, active);
        Ok(())
    }

    pub(in crate::app) fn close_recovered_working_intervals(&mut self) {
        if self
            .board
            .attention_mut()
            .close_recovered_working_intervals()
        {
            self.persist_attention();
        }
    }

    pub(in crate::app) fn recover_completed_worktree_finishes(&mut self) {
        let recoverable = self
            .board
            .archived_values()
            .filter(|record| {
                record.status == WorktreeFinishStatus::Pending
                    && finish_git_steps_are_complete(record)
            })
            .map(|record| record.worktree_id.clone())
            .collect::<Vec<_>>();
        for worktree_id in recoverable {
            let mut record = self
                .board
                .archived(&worktree_id)
                .expect("collected archived worktree must remain present")
                .clone();
            record.status = WorktreeFinishStatus::Archived;
            record.archived_at = Some(now_rfc3339());
            let result = self
                .store
                .as_ref()
                .expect("recovery only runs with a store")
                .save_archived_worktree(&record);
            match result {
                Ok(()) => {
                    self.board.insert_archived(record);
                }
                Err(error) => {
                    eprintln!("recover worktree finish {worktree_id}: {error}");
                }
            }
        }
    }

    /// Hand a run whose checkout could not be put back to the verified recovery
    /// agent: a nonce-bound attempt on the record, the prompt that asks the
    /// agent to prove the exact lineage, and the Issue told what happened. What
    /// comes back is the message the caller refuses with.
    pub(in crate::app) fn start_checkout_recovery(
        &mut self,
        issue_id: &str,
        run_id: &str,
        error: &str,
    ) -> Result<String, String> {
        let project_id = self.project_of(run_id)?;
        let mut active = self.take_run(run_id)?;
        if active
            .recovery
            .as_ref()
            .is_some_and(|attempt| attempt.state == crate::run::RecoveryState::Started)
        {
            self.runs.insert(run_id.to_string(), active);
            return Ok("verified Issue recovery is already running".to_string());
        }
        let issue = self.plans.get(issue_id).ok_or("unknown issue_id")?;
        let requested_stage_id = recovery_target_stage(issue, &active);
        let recovery_id = format!("recovery-{}", uuid::Uuid::new_v4());
        let started_at = now_rfc3339();
        let prompt = recovery_agent_prompt(
            &recovery_id,
            issue_id,
            run_id,
            &requested_stage_id,
            &active.worktree,
            error,
            &issue.stages,
        );
        active.recovery = Some(crate::run::RecoveryAttempt {
            id: recovery_id.clone(),
            requested_stage_id: requested_stage_id.clone(),
            branch: active.worktree.recorded_branch.clone(),
            state: crate::run::RecoveryState::Started,
            report: None,
            started_at: started_at.clone(),
            completed_at: None,
        });
        active.last_error = Some(format!(
            "automatic branch restoration failed: {error}; verified recovery agent started"
        ));
        self.queue_recovery_turn(run_id, &active, &project_id, prompt)?;
        self.finish_run_mutation(run_id.to_string(), active)?;
        self.note_recovery_started(
            issue_id,
            run_id,
            &recovery_id,
            &requested_stage_id,
            error,
            started_at,
        )?;
        Ok(format!(
            "automatic restore failed; verified recovery {recovery_id} started"
        ))
    }

    /// Hand the recovery prompt to the agent that will prove the lineage. An
    /// agentless run is logged rather than refused: the attempt is on the
    /// record either way, and a human can start an agent against it.
    pub(in crate::app) fn queue_recovery_turn(
        &mut self,
        run_id: &str,
        active: &ActiveRun,
        project_id: &str,
        prompt: String,
    ) -> Result<(), String> {
        let project_root = self
            .projects
            .iter()
            .find(|project| project.id == project_id)
            .map(|project| project.repo_path.clone())
            .ok_or("unknown project_id")?;
        match PendingAgentTurn::for_recovery(run_id, active, &project_root, prompt) {
            Some(turn) => self.delivery_queue.enqueue(turn),
            None => eprintln!("recover {run_id}: no agent to hand the recovery to"),
        }
        Ok(())
    }

    /// Tell the Issue's conversation that a verified recovery is running,
    /// linked to the implementation it is for, the attempt itself, and the
    /// stage the agent is being asked to prove.
    pub(in crate::app) fn note_recovery_started(
        &mut self,
        issue_id: &str,
        run_id: &str,
        recovery_id: &str,
        requested_stage_id: &str,
        error: &str,
        started_at: String,
    ) -> Result<(), String> {
        let mut issue = self.take_plan(issue_id)?;
        let mut links = vec![
            crate::thread::ThreadLink::Implementation {
                issue_id: issue_id.to_string(),
                implementation_id: run_id.to_string(),
            },
            crate::thread::ThreadLink::Recovery {
                recovery_id: recovery_id.to_string(),
            },
        ];
        if let Some(stage) = issue
            .stages
            .iter()
            .find(|stage| stage.id == requested_stage_id)
        {
            links.push(crate::thread::ThreadLink::IssueStage {
                issue_id: issue_id.to_string(),
                stage_id: stage.id.clone(),
                path: stage.path.clone(),
            });
        }
        issue.agents.sole_thread_mut().push_event_with_links(
            crate::thread::ThreadEventKind::RecoveryStarted,
            Some(format!(
                "Verified recovery started after automatic restore failed: {error}"
            )),
            None,
            None,
            links,
            started_at,
        );
        self.finish_plan_mutation(issue_id.to_string(), issue)
    }

    /// Write down what the restore found: the checkout is back (or was never
    /// really gone), and the Issue's conversation says which. A restore that
    /// failed hands the run to the verified recovery agent instead — the run's
    /// exact lineage is what is at stake, and only an agent can confirm it.
    pub(in crate::app) fn settle_restored_checkout(
        &mut self,
        issue_id: String,
        run_id: String,
        restored: crate::lifecycle::RestoredCheckout,
        caller: Box<dyn ImplementationCaller>,
    ) -> Result<Value, String> {
        let crate::lifecycle::RestoredCheckout {
            checkout_stood,
            restored,
            downgrade,
        } = restored;
        let worktree = match restored {
            Ok(worktree) => worktree,
            Err(error) => {
                let recovering = self
                    .start_checkout_recovery(&issue_id, &run_id, &error)
                    .unwrap_or_else(|persist_failure| persist_failure);
                return caller.settle(self, Err(recovering));
            }
        };
        let settled = (|| -> Result<(), String> {
            let mut active = self.take_run(&run_id)?;
            active.worktree = worktree;
            active.last_error = None;
            if let Some(reason) = downgrade {
                self.note_isolation_downgrade(&run_id, &mut active, &reason);
            }
            let worktree_id = crate::worktree::external_worktree_id(&active.worktree.path);
            self.finish_run_mutation(run_id.clone(), active)?;
            let mut issue = self.take_plan(&issue_id)?;
            issue.agents.sole_thread_mut().push_event_with_links(
                if checkout_stood {
                    crate::thread::ThreadEventKind::WorktreeReused
                } else {
                    crate::thread::ThreadEventKind::WorktreeRecreated
                },
                Some(if checkout_stood {
                    "Verified and reused the original Issue worktree".to_string()
                } else {
                    "Recreated the Issue worktree from its original branch".to_string()
                }),
                None,
                None,
                vec![
                    crate::thread::ThreadLink::Implementation {
                        issue_id: issue_id.clone(),
                        implementation_id: run_id.clone(),
                    },
                    crate::thread::ThreadLink::Worktree { worktree_id },
                ],
                now_rfc3339(),
            );
            self.finish_plan_mutation(issue_id.clone(), issue)
        })();
        caller.settle(self, settled.map(|()| run_id.as_str()))
    }

    #[allow(clippy::cognitive_complexity)] // ratchet: consume_recovery_report is at 16, threshold 15 — bring it under, then remove
    pub(in crate::app) fn consume_recovery_report(
        &mut self,
        run_id: &str,
        mut active: ActiveRun,
        report: DoneReport,
    ) {
        let issue_id = active.run.plan_id.as_ref().map(|id| id.0.clone());
        let now = now_rfc3339();
        let reported = report.outputs.recovery.clone();
        let mut restored_isolation_downgrade = None;
        let verification = (|| -> Result<crate::mcp::RecoveryReport, String> {
            if report.status != DoneStatus::Completed {
                return Err(format!(
                    "recovery agent reported {:?}: {}",
                    report.status, report.summary
                ));
            }
            let reported = reported.clone().ok_or("recovery report missing")?;
            let attempt = active
                .recovery
                .as_ref()
                .filter(|attempt| attempt.state == crate::run::RecoveryState::Started)
                .ok_or("no recovery attempt is awaiting a report")?;
            if reported.recovery_id != attempt.id {
                return Err("recovery nonce does not match the persisted attempt".to_string());
            }
            if !reported.recovered {
                return Err(format!(
                    "exact lineage was not recovered: {}",
                    reported.findings
                ));
            }
            if reported.branch != active.worktree.recorded_branch {
                return Err("recovery report names a different branch".to_string());
            }
            let project_id = self.project_of(run_id)?;
            let resolved = self.resolved_isolation(&project_id);
            let worktree = self
                .orch_for(&project_id)?
                .restore_run_worktree(
                    &active.worktree,
                    unregistered_restore_for(&active),
                    resolved.isolation,
                )
                .map_err(err)?;
            restored_isolation_downgrade = resolved.downgrade;
            let checkout =
                git2::Repository::open(&worktree.path).map_err(|error| error.to_string())?;
            let verified_head = checkout
                .head()
                .and_then(|head| head.peel_to_commit())
                .map_err(|error| error.to_string())?
                .id()
                .to_string();
            if reported.head_sha != verified_head {
                return Err(format!(
                    "recovery HEAD verification failed: agent reported {}, checkout is {verified_head}",
                    reported.head_sha
                ));
            }
            active.worktree = worktree;
            Ok(reported)
        })();
        if let Some(reason) = restored_isolation_downgrade {
            self.note_isolation_downgrade(run_id, &mut active, &reason);
        }

        let (event, summary, recovery_id, requested_stage_id) = match verification {
            Ok(verified) => {
                let attempt = active.recovery.as_mut().expect("verified attempt exists");
                attempt.state = crate::run::RecoveryState::Succeeded;
                attempt.report = Some(verified.clone());
                attempt.completed_at = Some(now.clone());
                active.last_error = None;
                (
                    crate::thread::ThreadEventKind::RecoverySucceeded,
                    format!(
                        "Verified recovery restored {} at {}",
                        verified.branch, verified.head_sha
                    ),
                    attempt.id.clone(),
                    attempt.requested_stage_id.clone(),
                )
            }
            Err(reason) => {
                let (recovery_id, requested_stage_id) = active
                    .recovery
                    .as_ref()
                    .map(|attempt| (attempt.id.clone(), attempt.requested_stage_id.clone()))
                    .unwrap_or_else(|| ("recovery-unmatched".to_string(), String::new()));
                if let Some(attempt) = active.recovery.as_mut() {
                    attempt.state = crate::run::RecoveryState::Failed;
                    attempt.report = reported;
                    attempt.completed_at = Some(now.clone());
                }
                active.last_error = Some(format!("verified recovery failed: {reason}"));
                if let Some(issue_id) = issue_id.as_deref() {
                    if let Some(issue) = self.plans.get(issue_id) {
                        if let Some(index) = issue
                            .stages
                            .iter()
                            .position(|stage| stage.id == requested_stage_id)
                        {
                            if let Some(predecessor) = index.checked_sub(1).and_then(|previous| {
                                active
                                    .stages
                                    .iter_mut()
                                    .find(|progress| progress.stage_id == issue.stages[previous].id)
                            }) {
                                if matches!(
                                    predecessor.publication,
                                    StagePublication::Local | StagePublication::LegacyUnknown
                                ) {
                                    predecessor.invalidation_reason = Some(format!(
                                        "preceding unpublished stage invalidated after verified recovery failed: {reason}"
                                    ));
                                }
                            }
                        }
                    }
                }
                (
                    crate::thread::ThreadEventKind::RecoveryFailed,
                    format!("Verified recovery failed: {reason}"),
                    recovery_id,
                    requested_stage_id,
                )
            }
        };
        let succeeded = event == crate::thread::ThreadEventKind::RecoverySucceeded;
        let persisted = self.finish_run_mutation(run_id.to_string(), active);
        if let Err(error) = persisted {
            eprintln!("recovery {run_id}: run persist failed: {error}");
            return;
        }
        if let Some(issue_id) = issue_id {
            if let Ok(mut issue) = self.take_plan(&issue_id) {
                let mut links = vec![
                    crate::thread::ThreadLink::Implementation {
                        issue_id: issue_id.clone(),
                        implementation_id: run_id.to_string(),
                    },
                    crate::thread::ThreadLink::Recovery { recovery_id },
                ];
                if let Some(stage) = issue
                    .stages
                    .iter()
                    .find(|stage| stage.id == requested_stage_id)
                {
                    links.push(crate::thread::ThreadLink::IssueStage {
                        issue_id: issue_id.clone(),
                        stage_id: stage.id.clone(),
                        path: stage.path.clone(),
                    });
                }
                issue.agents.sole_thread_mut().push_event_with_links(
                    event,
                    Some(summary),
                    None,
                    None,
                    links,
                    &now,
                );
                let persisted = self.finish_plan_mutation(issue_id.clone(), issue);
                if let Err(error) = persisted {
                    eprintln!("recovery {run_id}: Issue persist failed: {error}");
                    return;
                }
            }
            if succeeded {
                // The stage this recovery was for is next, and reaching it
                // cuts or puts back a checkout. The socket that carried this
                // report runs that git with the guard released, as a frame
                // does; a refusal is already written onto the Issue here.
                if let Err(error) =
                    self.defer_issue_scheduler(&issue_id, &json!({ "issue_id": issue_id }), None)
                {
                    eprintln!("recovery {run_id}: scheduler blocked: {error}");
                }
            } else if let Err(error) = self.refresh_issue_scheduler_activity(&issue_id) {
                eprintln!("recovery {run_id}: scheduler refresh failed: {error}");
            }
        }
    }

    /// Runs whose checkout vanished, on their way to Archived, decided off the
    /// state lock.
    ///
    /// Whether a stage's commits ever left this machine is a fetch and two
    /// graph walks per stage, and every board read used to pay for it inline.
    /// Now the board answers with the runs it still has and the sweep archives
    /// them behind it — the same bargain the scan and the diffstats make.
    /// Single-flight: a sweep already running absorbs the next poll's.
    pub(in crate::app) fn sweep_vanished_runs(&mut self) {
        if self.vanished_run_sweep_in_flight {
            return;
        }
        let queries: Vec<StagePublicationQuery> = self
            .runs
            .iter()
            .filter(|(_, active)| {
                !active.run.state.is_terminal()
                    && active.run.state != RunState::Created
                    && active.recovery.is_none()
                    && !active.worktree.path.exists()
            })
            .map(|(run_id, active)| self.stage_publication_query(run_id, active))
            .collect();
        if queries.is_empty() {
            return;
        }
        self.vanished_run_sweep_in_flight = true;
        self.run_off_lock(VanishedRunSweep {
            queries,
            #[cfg(test)]
            gate: self.off_lock_gate.clone(),
        });
    }

    /// A run the user deletes must disappear from Build. Any live run whose
    /// worktree vanished retires to Archived: session ended, git's stale
    /// worktree record pruned — the record stays as quiet history. `Created` is
    /// exempt (its worktree may legitimately not exist yet).
    pub(in crate::app) fn archive_vanished_runs(&mut self, decided: Vec<DecidedVanishedRun>) {
        self.vanished_run_sweep_in_flight = false;
        for DecidedVanishedRun { run_id, published } in decided {
            // The checkout may have come back, or the run may have been
            // abandoned outright, while the sweep was asking git.
            if self
                .runs
                .get(&run_id)
                .is_none_or(|active| active.worktree.path.exists())
            {
                continue;
            }
            let Ok(mut active) = self.take_run(&run_id) else {
                continue;
            };
            let issue_id = active.run.plan_id.as_ref().map(|id| id.0.clone());
            let branch = active.worktree.branch();
            let affected_stages = reconcile_missing_run_worktree(&mut active, &published);
            let worktree_id = crate::worktree::external_worktree_id(&active.worktree.path);
            match active.run.apply(RunEvent::Archive) {
                Ok(_) => {
                    self.prune_worktree_records(&run_id);
                    eprintln!(
                        "archived {run_id}: its worktree {} was deleted outside Build",
                        active.worktree.path.display()
                    );
                }
                Err(e) => eprintln!("archive {run_id}: {e}"),
            }
            let persisted = self.finish_run_mutation(run_id.clone(), active);
            if let Err(e) = persisted {
                eprintln!("archive {run_id}: {e}");
            }
            if let Some(issue_id) = issue_id {
                if let Ok(mut issue) = self.take_plan(&issue_id) {
                    issue.agents.sole_thread_mut().push_event_with_links(
                        crate::thread::ThreadEventKind::WorktreeDeleted,
                        Some(format!(
                            "Implementation worktree disappeared; {} stage(s) were reconciled",
                            affected_stages.len()
                        )),
                        None,
                        None,
                        vec![
                            crate::thread::ThreadLink::Implementation {
                                issue_id: issue_id.clone(),
                                implementation_id: run_id.clone(),
                            },
                            crate::thread::ThreadLink::Worktree {
                                worktree_id: worktree_id.clone(),
                            },
                        ],
                        now_rfc3339(),
                    );
                    for stage_id in &affected_stages {
                        if let Some(stage) = issue.stages.iter().find(|stage| &stage.id == stage_id)
                        {
                            issue.agents.sole_thread_mut().push_event_with_links(
                                crate::thread::ThreadEventKind::StageInvalidated,
                                Some(format!("Stage “{}” is incomplete", stage.title)),
                                None,
                                None,
                                vec![
                                    crate::thread::ThreadLink::IssueStage {
                                        issue_id: issue_id.clone(),
                                        stage_id: stage.id.clone(),
                                        path: stage.path.clone(),
                                    },
                                    crate::thread::ThreadLink::Implementation {
                                        issue_id: issue_id.clone(),
                                        implementation_id: run_id.clone(),
                                    },
                                ],
                                now_rfc3339(),
                            );
                        }
                    }
                    let persisted = self.finish_plan_mutation(issue_id.clone(), issue);
                    if let Err(e) = persisted {
                        eprintln!("archive {run_id}: issue event persist failed: {e}");
                    }
                    // The checkout went away under Build with nothing merged,
                    // so the issue is back in the inbox. Say which branch it
                    // lost, or its reappearance is unexplained.
                    self.note_implementation_abandoned(
                        &issue_id,
                        &run_id,
                        &branch,
                        "deleted outside Build",
                    );
                }
            }
        }
    }

    /// Clear every stale record of a checkout in the run's project — the
    /// sweep after one went away outside Build. Best effort, which is the
    /// façade's own policy for it: nothing the caller asked for depends on it.
    pub(in crate::app) fn prune_worktree_records(&self, run_id: &str) {
        let Some(project) = self
            .projects
            .project_id_of(run_id)
            .and_then(|project_id| self.projects.get(project_id))
        else {
            return;
        };
        project.orch.worktrees().prune();
    }

    /// Ask git about this run's stages here and now, with the state lock in
    /// hand. Boot's failed-recovery arm alone: it decides against refs that are
    /// about to be deleted, and runs before the first frame is served, so
    /// nothing waits on the mutex it holds. Every other caller — `run.abandon`,
    /// the vanished-run sweep — asks through [`StagePublicationQuery::classify`]
    /// in a lock-free run phase.
    pub(in crate::app) fn classify_stages_now(
        &self,
        run_id: &str,
        active: &ActiveRun,
    ) -> StagePublications {
        self.stage_publication_query(run_id, active).classify()
    }

    /// What one run's stages have to be judged against, taken under the lock so
    /// [`StagePublicationQuery::classify`] can ask git without it.
    pub(in crate::app) fn stage_publication_query(
        &self,
        run_id: &str,
        active: &ActiveRun,
    ) -> StagePublicationQuery {
        StagePublicationQuery {
            run_id: run_id.to_string(),
            worktrees: self
                .projects
                .project_id_of(run_id)
                .and_then(|project_id| self.projects.get(project_id))
                .map(|project| project.orch.worktrees().clone()),
            checkout: active.worktree.path.clone(),
            branch: active.worktree.branch(),
            base_branch: active.worktree.base_branch.clone(),
            completions: active
                .stages
                .iter()
                .filter_map(|progress| {
                    Some((progress.stage_id.clone(), progress.completion_sha.clone()?))
                })
                .collect(),
        }
    }
}

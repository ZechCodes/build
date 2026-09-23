#[cfg(test)]
use crate::app::OffLockGate;
use crate::app::{
    err, plan_state_str, run_state_str, AppState, ImplementationCaller, OffLockJob,
    PendingAgentTurn, SESSION_DIED_SUMMARY,
};
use crate::operation::{OperationReceipt, OperationStatus};
use crate::orchestrator::{ActivePlan, ActiveRun};
use crate::plan::PlanEvent;
use crate::run::{RunEvent, RunState, StageProgressState, StagePublication};
use crate::store::{now_rfc3339, PersistedArchivedWorktree, PersistedPlan, PersistedRun, Store};
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
/// left this machine, or that never completed, is marked incomplete.
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
        let in_flight = progress.state != StageProgressState::Completed;
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
    // Recovery mutates operation-owned messages and the thread sequence kept
    // in each agent skeleton. Read plans and runs only after those writes, or
    // restoration would hydrate the pre-recovery copies and later overwrite
    // the recovered status with stale state.
    let operations = store
        .recover_operations()
        .map_err(|error| error.to_string())?;
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
        operations,
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
        self.restore_plans_before_runs(plans, runs)?;
        self.rebuild_session_summaries()?;
        self.restore_operations(operations);
        self.seed_conversation_attention_sequences();
        self.seed_anchors_for_records_without_one();
        self.migrate_legacy_dismissals();
        self.close_recovered_working_intervals();
        Ok(())
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

        // A `proj-N` id is not durable across boots, so resolve by repo path.
        // Retain the record's path unconditionally so a parked repo-missing
        // plan keeps a real path to un-park to.
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
                // An Issue's implementation keeps its record: its branch is
                // the lineage the Issue's stages were built on, and only the
                // human can say how to get it back.
                Err(error) if active.run.plan_id.is_some() && project_id.is_some() => {
                    active.last_error =
                        Some(format!("Build could not restore this worktree: {error}"));
                    recovery_event = Some((
                        crate::thread::ThreadEventKind::RecoveryFailed,
                        format!("Could not restore the worktree: {error}"),
                    ));
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

    /// Write down what the restore found: the checkout is back (or was never
    /// really gone), and the Issue's conversation says which. A restore that
    /// failed is refused with git's reason.
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
                return caller.settle(
                    self,
                    Err(format!("Build could not restore this worktree: {error}")),
                );
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

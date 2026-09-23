use crate::app::{
    append_user_thread_messages, dispatchable_next_run_stage, err, has_agent_choice,
    model_choice_from, parse_thread_inputs, require_str, thread_detail, AppState, DigestScope,
    PendingAgentTurn, ReadSubject, NEW_THREAD_MESSAGES_PROMPT,
};
use crate::run::{PublicationAttempt, RunState, StagePublication};
use crate::store::now_rfc3339;
use crate::worktree::{git_stdout, Worktree};
use serde_json::{json, Value};

/// What happens to the worktree + branch after a user-approved merge lands
/// (spec §5.7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::app) enum MergeCleanup {
    /// Remove the worktree + branch — today's unconditional behavior.
    Prune,
    /// Merge into the base but keep the worktree and branch alive.
    Keep,
    /// Merge, then drop the task record entirely (un-adopt): the worktree and
    /// branch survive and resurface as an external card.
    Release,
}

/// Parse the optional `cleanup` param. Absent → `Prune` (today's behavior).
/// `"release"` is only meaningful for adopted tasks.
pub(in crate::app) fn merge_cleanup_from(
    params: &Value,
    adopted: bool,
) -> Result<MergeCleanup, String> {
    // Distinguish "absent" (→ Prune, today's default) from "present but not a
    // string" — a non-string value must fail fast, never silently collapse to the
    // destructive Prune default (on an adopted task Prune deletes files Build did
    // not create).
    let cleanup = match params.get("cleanup") {
        None | Some(Value::Null) => return Ok(MergeCleanup::Prune),
        Some(value) => value
            .as_str()
            .ok_or_else(|| format!("invalid cleanup: {value} (expected prune|keep|release)"))?,
    };
    match cleanup {
        "prune" => Ok(MergeCleanup::Prune),
        "keep" => Ok(MergeCleanup::Keep),
        "release" if adopted => Ok(MergeCleanup::Release),
        "release" => Err("cleanup: \"release\" is only valid for adopted tasks".to_string()),
        other => Err(format!(
            "invalid cleanup: {other:?} (expected prune|keep|release)"
        )),
    }
}

impl AppState {
    pub(crate) fn run_diff(&mut self, params: &Value) -> Result<Value, String> {
        self.plan_run_diff(params, None)
    }

    /// `run.diff`, with the issue that asked for it when an issue surface did.
    pub(in crate::app) fn plan_run_diff(
        &mut self,
        params: &Value,
        issue_id: Option<String>,
    ) -> Result<Value, String> {
        let run_id = require_str(params, "run_id")?;
        self.project_of(&run_id)?;
        let active = self.runs.get(&run_id).ok_or("unknown run_id")?;
        let subject = ReadSubject::Run {
            worktree_path: self.run_git_root(&run_id, &active.worktree.path),
            base_sha: active.base_sha.clone(),
            base_branch: self.run_base_branch(&run_id, &active.worktree.base_branch),
        };
        Ok(self.defer_conditional_read(
            subject,
            issue_id,
            params.get("if_diff_key").and_then(Value::as_str),
            crate::app::wants_patch(params),
        ))
    }

    /// Immutable stage review surface. Unlike `run.diff`, this never reads the
    /// working directory or current HEAD: it resolves only the two object ids
    /// persisted when the stage was dispatched and successfully validated.
    pub(crate) fn run_stage_diff(&mut self, params: &Value) -> Result<Value, String> {
        self.plan_run_stage_diff(params, None)
    }

    /// `run.stage_diff`, with the issue that asked for it when an issue
    /// surface did.
    pub(in crate::app) fn plan_run_stage_diff(
        &mut self,
        params: &Value,
        issue_id: Option<String>,
    ) -> Result<Value, String> {
        let run_id = require_str(params, "run_id")?;
        let stage_id = require_str(params, "stage_id")?;
        let active = self.runs.get(&run_id).ok_or("unknown run_id")?;
        let progress = active
            .stage_progress(&stage_id)
            .ok_or_else(|| format!("unknown stage_id: {stage_id}"))?;
        let (Some(start_sha), Some(completion_sha)) =
            (progress.start_sha.clone(), progress.completion_sha.clone())
        else {
            let mut unavailable = json!({
                "run_id": run_id,
                "stage_id": stage_id,
                "status": "unavailable",
                "reason": if progress.publication == crate::run::StagePublication::LegacyUnknown {
                    "legacy_unpinned"
                } else {
                    "stage_not_complete"
                },
                "start_sha": progress.start_sha,
                "completion_sha": progress.completion_sha,
            });
            if let Some(issue_id) = issue_id {
                unavailable
                    .as_object_mut()
                    .expect("built as an object")
                    .insert("issue_id".to_string(), json!(issue_id));
            }
            return Ok(unavailable);
        };
        let worktree_path = active.worktree.path.clone();
        let object_database = if worktree_path.exists() {
            worktree_path
        } else {
            std::path::PathBuf::from(self.project_path_for(&run_id))
        };
        let subject = ReadSubject::Stage {
            run_id,
            stage_id,
            object_database,
            start_sha,
            completion_sha,
        };
        Ok(self.defer_read(subject, issue_id))
    }

    /// Send diff comments to the coding agent — from `review` or `building`.
    ///
    /// The comments land on the durable thread first, so the turn that travels
    /// is only ever an instruction to read them: a warm agent gets exactly that,
    /// and a cold one gets it wrapped in enough run context to act on. The
    /// worktree's agent is delivered to, never killed and replaced.
    pub(crate) fn run_request_changes(&mut self, params: &Value) -> Result<Value, String> {
        let run_id = require_str(params, "run_id")?;
        let messages = parse_thread_inputs(params, crate::thread::ArtifactKind::Diff, "comments")?;
        let project_id = self.project_of(&run_id)?;
        let plan_docs = {
            let active = self.runs.get(&run_id).ok_or("unknown run_id")?;
            self.owning_plan_stage_docs(active)
        };
        let run_agent_id = self.resolve_conversation_params(&run_id, params)?.agent_id;
        self.edit_agent_conversation(&run_id, &run_agent_id, |thread, _artifact| {
            append_user_thread_messages(thread, messages);
            Ok(())
        })?;
        let mut active = self.take_run(&run_id)?;
        let outcome = (|| -> Result<(), String> {
            let turn = self
                .orch_for(&project_id)?
                .run_request_changes(
                    &mut active,
                    &plan_docs,
                    NEW_THREAD_MESSAGES_PROMPT,
                    Some(&run_agent_id),
                )
                .map_err(err)?;
            self.delivery_queue.enqueue(PendingAgentTurn::for_run_agent(
                &run_id,
                &run_agent_id,
                &active,
                turn,
            ));
            self.qa_drive_run(&project_id, &mut active, &plan_docs)
        })();
        let (view, persisted) = self.answer_run_mutation(run_id, active, thread_detail(params));
        outcome?;
        persisted?;
        Ok(view)
    }

    pub(crate) fn run_stage_dispatch(&mut self, params: &Value) -> Result<Value, String> {
        let run_id = require_str(params, "run_id")?;
        let stage_id = require_str(params, "stage_id")?;
        let model_override = if has_agent_choice(params) {
            Some(model_choice_from(params, self.default_harness)?)
        } else {
            None
        };
        let project_id = self.project_of(&run_id)?;
        let plan_docs = {
            let active = self.runs.get(&run_id).ok_or("unknown run_id")?;
            self.owning_plan_stage_docs(active)
        };
        let mut active = self.take_run(&run_id)?;
        let outcome = (|| -> Result<(), String> {
            let turn = self
                .orch_for(&project_id)?
                .dispatch_run_stage(&mut active, &plan_docs, &stage_id, model_override)
                .map_err(err)?;
            self.delivery_queue
                .enqueue(PendingAgentTurn::for_run(&run_id, &mut active, turn));
            self.qa_drive_run(&project_id, &mut active, &plan_docs)
        })();
        let persisted = self.finish_run_mutation(run_id.clone(), active);
        outcome?;
        persisted?;
        self.record_issue_current_stage_started(&run_id, &plan_docs)?;
        self.auto_advance_run(&run_id);
        let active = self.runs.get(&run_id).ok_or("unknown run_id")?;
        Ok(self.run_view(&run_id, active, thread_detail(params), DigestScope::Detail))
    }

    /// Send a stage's open comments (persisted on the owning plan) to a fresh
    /// mid-run revision session in the RUN's worktree — the stage-gate
    /// analogue of `plan.stage_send_notes`, which is illegal once the plan is
    /// Approved. The run owns the session; the plan owns the docs; the
    /// revision's `done` ingests the rewritten docs back to the store.
    pub(crate) fn run_stage_send_notes(&mut self, params: &Value) -> Result<Value, String> {
        let run_id = require_str(params, "run_id")?;
        let stage_id = require_str(params, "stage_id")?;
        let project_id = self.project_of(&run_id)?;
        let plan_id = self
            .runs
            .get(&run_id)
            .ok_or("unknown run_id")?
            .run
            .plan_id
            .as_ref()
            .map(|p| p.0.clone())
            .ok_or("this run implements no plan — there are no plan docs to revise")?;
        let mut active = self.take_run(&run_id)?;
        let mut plan = self.plans.remove(&plan_id);
        let outcome = (|| -> Result<(), String> {
            let plan = plan.as_mut().ok_or("unknown plan_id")?;
            let turn = self
                .orch_for(&project_id)?
                .send_run_stage_notes(&mut active, plan, &stage_id)
                .map_err(err)?;
            self.delivery_queue
                .enqueue(PendingAgentTurn::for_run(&run_id, &mut active, turn));
            if self.qa_agent {
                self.qa_simulate_run_stage_revise(&project_id, &mut active, plan)?;
            }
            Ok(())
        })();
        // Both entities re-insert before any error propagates — the
        // take → finish_mutation invariant covers the plan here too.
        let plan_persisted = plan.map(|plan| self.finish_plan_mutation(plan_id, plan));
        let persisted = self.finish_run_mutation(run_id.clone(), active);
        outcome?;
        if let Some(persisted) = plan_persisted {
            persisted?;
        }
        persisted?;
        let active = self.runs.get(&run_id).ok_or("unknown run_id")?;
        Ok(self.run_view(&run_id, active, thread_detail(params), DigestScope::Detail))
    }

    /// "Run all": arm/disarm auto-advance, then (armed) run every dispatchable
    /// approved stage to its verdict.
    pub(crate) fn run_set_auto_advance(&mut self, params: &Value) -> Result<Value, String> {
        let run_id = require_str(params, "run_id")?;
        let enabled = params
            .get("enabled")
            .and_then(Value::as_bool)
            .ok_or("missing required param: enabled")?;
        let mut active = self.take_run(&run_id)?;
        if active.run.state.is_terminal() {
            self.runs.insert(run_id, active);
            return Err("cannot set auto_advance on a terminal run".to_string());
        }
        active.auto_advance = enabled;
        let persisted = self.finish_run_mutation(run_id.clone(), active);
        persisted?;
        if enabled {
            self.auto_advance_run(&run_id);
        }
        let active = self.runs.get(&run_id).ok_or("unknown run_id")?;
        Ok(self.run_view(&run_id, active, thread_detail(params), DigestScope::Detail))
    }

    /// If a run is parked at the stage gate with run-all armed and a next
    /// approved+dispatchable stage, dispatch it (looping through as many stages
    /// as run-all chains into). In production one hop leaves the run `Building`
    /// and the loop returns; in QA `qa_drive_run` drives it back to the gate,
    /// so the loop advances to the next stage.
    pub(in crate::app) fn auto_advance_run(&mut self, run_id: &str) {
        let max_hops = 64;
        for _ in 0..max_hops {
            let (project_id, plan_docs, next) = {
                let Some(active) = self.runs.get(run_id) else {
                    return;
                };
                if active.run.state != RunState::StageGate || !active.auto_advance {
                    return;
                }
                let plan_docs = self.owning_plan_stage_docs(active);
                let Some(next) = dispatchable_next_run_stage(active, &plan_docs) else {
                    return;
                };
                let Ok(project_id) = self.project_of(run_id) else {
                    return;
                };
                (project_id, plan_docs, next)
            };
            let Ok(mut active) = self.take_run(run_id) else {
                return;
            };
            let outcome = (|| -> Result<(), String> {
                let turn = self
                    .orch_for(&project_id)?
                    .dispatch_run_stage(&mut active, &plan_docs, &next, None)
                    .map_err(err)?;
                self.delivery_queue
                    .enqueue(PendingAgentTurn::for_run(run_id, &mut active, turn));
                self.qa_drive_run(&project_id, &mut active, &plan_docs)
            })();
            let persisted = self.finish_run_mutation(run_id.to_string(), active);
            if let Err(e) = outcome {
                eprintln!("auto-advance {run_id}: {e}");
                return;
            }
            if let Err(e) = persisted {
                eprintln!("auto-advance {run_id}: {e}");
                return;
            }
            if let Err(e) = self.record_issue_current_stage_started(run_id, &plan_docs) {
                eprintln!("auto-advance {run_id}: {e}");
                return;
            }
        }
        eprintln!("auto-advance {run_id}: did not converge");
    }

    /// Finish-the-worktree git actions from the diff review: `commit`/`push`
    /// keep the worktree; `merge`/`merge_push` merge into the base and end the
    /// run. Every action commits outstanding work first.
    #[allow(clippy::cognitive_complexity)] // ratchet: run_git_action is at 18, threshold 15 — bring it under, then remove
    pub(crate) fn run_git_action(&mut self, params: &Value) -> Result<Value, String> {
        let run_id = require_str(params, "run_id")?;
        let action = require_str(params, "action")?;
        let project_id = self.project_of(&run_id)?;
        let is_merge_action = matches!(action.as_str(), "merge" | "merge_push");
        if !is_merge_action && params.get("cleanup").is_some() {
            return Err("cleanup only applies to merge actions".to_string());
        }
        let bound = self.runs.get(&run_id).ok_or("unknown run_id")?;
        let adopted = bound.adopted;
        // A merge lands the run's branch on the base branch through the
        // project's repository. For a run standing in that repository the
        // target IS the checkout being merged — a no-op when it sits on the
        // base branch, and a merge into the wrong tree when it does not.
        if is_merge_action && self.stands_in_the_repository(&run_id, bound) {
            return Err(
                "run.git_action: a run standing in the project's repository cannot be merged — \
                 its branch is what a merge would target"
                    .to_string(),
            );
        }
        let cleanup = if is_merge_action {
            merge_cleanup_from(params, adopted)?
        } else {
            MergeCleanup::Prune
        };
        let mut active = self.take_run(&run_id)?;
        let issue_id = active.run.plan_id.as_ref().map(|id| id.0.clone());

        // Push and merge cross a process boundary: git may complete and the
        // daemon may die before the resulting state is saved. Commit first so
        // the exact candidate is known, then fsync a write-ahead intent before
        // invoking the externally visible side effect.
        if matches!(action.as_str(), "push" | "merge" | "merge_push") {
            let prepared = self
                .orch_for(&project_id)
                .and_then(|orch| orch.run_commit(&active).map_err(err))
                .and_then(|()| git_stdout(&active.worktree.path, &["rev-parse", "HEAD"]))
                .map(|candidate_sha| PublicationAttempt {
                    action: action.clone(),
                    candidate_sha: candidate_sha.trim().to_string(),
                    started_at: now_rfc3339(),
                });
            let attempt = match prepared {
                Ok(attempt) => attempt,
                Err(error) => {
                    self.runs.insert(run_id, active);
                    return Err(error);
                }
            };
            active.publication_attempt = Some(attempt);
            if let Err(error) = self.persist_run_record(&run_id, &active) {
                self.runs.insert(run_id, active);
                return Err(error);
            }
        }
        let result = {
            let orch = self.orch_for(&project_id)?;
            match action.as_str() {
                "commit" => orch.run_commit(&active).map_err(err),
                "push" => orch.run_push(&active).map_err(err),
                "merge" => orch.run_approve_merge(&mut active).map_err(err),
                "merge_push" => orch.run_merge_and_push(&mut active).map_err(err),
                other => Err(format!("unknown git action: {other}")),
            }
        };
        if let Err(message) = &result {
            if message.starts_with("merge_failed:") {
                active.last_error = Some(message.clone());
            }
        }
        let (event, summary) = match (&result, action.as_str()) {
            (Ok(()), "commit") => (
                crate::thread::ThreadEventKind::Committed,
                "Changes committed".to_string(),
            ),
            (Ok(()), "push") => (
                crate::thread::ThreadEventKind::Pushed,
                "Changes committed and pushed".to_string(),
            ),
            (Ok(()), "merge") => (
                crate::thread::ThreadEventKind::Merged,
                "Changes merged into the base branch".to_string(),
            ),
            (Ok(()), "merge_push") => (
                crate::thread::ThreadEventKind::Merged,
                "Changes merged and pushed".to_string(),
            ),
            (Err(error), _) => (
                crate::thread::ThreadEventKind::RunFailed,
                format!("Git action {action} failed: {error}"),
            ),
            _ => unreachable!("validated git action"),
        };
        if result.is_ok() {
            let publication = match action.as_str() {
                "push" => Some(StagePublication::Pushed),
                "merge" | "merge_push" => Some(StagePublication::Merged),
                _ => None,
            };
            if let Some(publication) = publication {
                for progress in &mut active.stages {
                    if progress.completion_sha.is_some() {
                        progress.publication = publication;
                        progress.invalidation_reason = None;
                    }
                }
                // Clearing this field and the lifecycle/thread update below
                // share the post-side-effect atomic record write. If that write
                // is interrupted, boot still sees the prior journal.
                active.publication_attempt = None;
            }
        }
        let links = issue_id
            .as_ref()
            .and_then(|issue_id| self.plans.get(issue_id).map(|issue| (issue_id, issue)))
            .map(|(issue_id, issue)| {
                let mut links = vec![crate::thread::ThreadLink::Implementation {
                    issue_id: issue_id.clone(),
                    implementation_id: run_id.clone(),
                }];
                links.extend(issue.stages.iter().map(|stage| {
                    crate::thread::ThreadLink::IssueStage {
                        issue_id: issue_id.clone(),
                        stage_id: stage.id.clone(),
                        path: stage.path.clone(),
                    }
                }));
                links
            })
            .unwrap_or_else(|| {
                vec![crate::thread::ThreadLink::Run {
                    run_id: run_id.clone(),
                }]
            });
        if let (None, Some(primary)) = (&issue_id, active.agents.primary_mut()) {
            primary.thread.push_event_with_links(
                event,
                Some(summary.clone()),
                None,
                None,
                links.clone(),
                now_rfc3339(),
            );
        }
        let merged_worktree = (result.is_ok() && active.run.state == RunState::Merged)
            .then(|| active.worktree.clone());
        let (view, persisted) =
            self.answer_run_mutation(run_id.clone(), active, thread_detail(params));
        let issue_persisted = if let Some(issue_id) = issue_id {
            let mut issue = self.take_plan(&issue_id)?;
            issue.agents.sole_thread_mut().push_event_with_links(
                event,
                Some(summary),
                None,
                None,
                links,
                now_rfc3339(),
            );
            Some(self.finish_plan_mutation(issue_id, issue))
        } else {
            None
        };
        result?;
        persisted?;
        if let Some(issue_persisted) = issue_persisted {
            issue_persisted?;
        }
        if let Some(worktree) = merged_worktree {
            self.apply_merge_cleanup(&run_id, &project_id, &worktree, cleanup);
        }
        Ok(view)
    }

    /// Prune a merged run's worktree once its `Merged` verdict is durable.
    pub(in crate::app) fn prune_merged_worktree(&self, project_id: &str, worktree: &Worktree) {
        if let Ok(orch) = self.orch_for(project_id) {
            orch.discard_checkout(worktree, /* keep_branch */ false);
        }
    }

    /// What happens to the worktree after a user-approved merge lands.
    pub(in crate::app) fn apply_merge_cleanup(
        &mut self,
        run_id: &str,
        project_id: &str,
        worktree: &Worktree,
        cleanup: MergeCleanup,
    ) {
        match cleanup {
            MergeCleanup::Prune => {
                self.retire_agents_of_pruned_worktree(&worktree.path);
                self.prune_merged_worktree(project_id, worktree);
            }
            MergeCleanup::Keep => {}
            MergeCleanup::Release => {
                if let Err(error) = self.preserve_entity_issue_identities(run_id) {
                    eprintln!("merge cleanup release {run_id}: {error}; keeping the run");
                    return;
                }
                if let Some(store) = &self.store {
                    if let Err(e) = store.delete_run(run_id) {
                        eprintln!("merge cleanup release {run_id}: run store: {e}");
                    }
                }
                self.runs.remove(run_id);
                self.projects.unbind_entity(run_id);
                self.board.attention_mut().remove_entity_clocks(run_id);
                self.board.diff_mut().remove_run_files_changed_at(run_id);
                self.invalidate_run_stat(run_id);
                self.rescan_external_worktrees(project_id);
            }
        }
    }
}

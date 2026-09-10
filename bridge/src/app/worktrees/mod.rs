use crate::app::{
    archived_worktree_json, named_agent_id, parse_worktree_finish_action, require_str,
    scan_may_yet_show_it, view_thread_detail, AppState, BranchFinishEpilogue, DigestScope,
    FinishKind, FinishRequirement, PlannedFinish, PlannedRunFinish, WorktreeFinishJob,
};
use crate::lifecycle::holders::ProjectCheckouts;
use crate::lifecycle::{CreateWorktree, PendingRow};
use crate::run::RunState;
use crate::store::{WorktreeFinishAction, WorktreeFinishStatus};
use serde_json::{json, Value};

pub(in crate::app) mod dispatch;
pub(in crate::app) mod finish;

impl AppState {
    /// Mint a bare worktree — no run, no agent, no session. It is the
    /// "somewhere to work" affordance beside issue creation: the human opens a
    /// terminal or an agent tab in it, and it stays unbound (the scan reports
    /// it like any hand-made worktree) until a mutating action adopts it.
    ///
    /// The two slots mean opposite things, and exactly one is given. `branch`
    /// names a branch that already exists — here or on a remote — and Build
    /// borrows it a directory, cutting nothing. `name` is words to cut a new
    /// branch after, and no branch of that spelling is consulted.
    pub(in crate::app) fn worktree_create(&mut self, params: &Value) -> Result<Value, String> {
        let project_id = require_str(params, "project_id")?;
        let (title, existing_branch, branch) = match (
            params.get("branch").and_then(Value::as_str),
            params.get("name").and_then(Value::as_str),
        ) {
            (Some(branch), None) => {
                if !crate::worktree::is_ref_name(branch) {
                    return Err(format!("{branch:?} is not a branch name"));
                }
                (branch.to_string(), Some(branch.to_string()), branch.to_string())
            }
            (None, Some(name)) => {
                if !name.chars().any(|c| c.is_ascii_alphanumeric()) {
                    return Err("a worktree name needs at least one letter or number".to_string());
                }
                (name.to_string(), None, crate::worktree::branch_name_for(&crate::worktree::slugify(name)))
            }
            _ => return Err("worktree.create takes exactly one of branch (a branch that already exists) and name (words to cut a new branch after)".to_string()),
        };
        let slug = crate::worktree::slugify(&title);
        let placeholder_id = if existing_branch.is_some() {
            format!("pending-worktree-{}", uuid::Uuid::new_v4())
        } else {
            self.planned_checkout_id(&project_id, &slug)?
        };
        let checkouts = self.project_checkouts(&project_id)?;
        let mutation = CreateWorktree {
            project: self.orch_for(&project_id)?.clone(),
            base_branch: self.base_for(&project_id)?,
            slug,
            existing_branch,
            checkouts: checkouts.clone(),
            resolved: self.resolved_isolation(&project_id),
        };
        let row = PendingRow::creating(placeholder_id.clone(), Some(project_id.clone()), title)
            .on_branch(branch)
            .isolated_as(mutation.resolved.isolation);
        self.defer_lifecycle(
            row,
            mutation,
            crate::app::runtime::lifecycle::CreateWorktreeSettlement {
                project_id,
                placeholder_id,
                checkouts,
            },
        )
    }

    /// Finish an external worktree selected only by server-resolved ids.
    ///
    /// Two halves. HERE, under the app mutex: resolve the project, settle the
    /// idempotent replays out of memory, and claim the checkout. Then
    /// [`WorktreeFinishJob::run`] with the mutex released: the forced rescan
    /// (which is both stale-id protection and the execution-time status
    /// recheck), the checkpoint, and the destructive git. Client paths are
    /// ignored and never become an authority in either half.
    pub(in crate::app) fn worktree_finish(&mut self, params: &Value) -> Result<Value, String> {
        match self.plan_worktree_finish(params)? {
            PlannedFinish::Settled(value) => Ok(value),
            PlannedFinish::Deferred(job) => {
                let epilogue = job.epilogue(FinishKind::Worktree);
                Ok(self.defer_finish(job, epilogue))
            }
        }
    }

    /// The lock-held half of every finish verb: what the app mutex decides
    /// before any disk is touched.
    pub(in crate::app) fn plan_worktree_finish(
        &mut self,
        params: &Value,
    ) -> Result<PlannedFinish, String> {
        let project_id = require_str(params, "project_id")?;
        let worktree_id = require_str(params, "worktree_id")?;
        let action = parse_worktree_finish_action(&require_str(params, "action")?)?;
        let (project_path, base_branch) = self
            .projects
            .iter()
            .find(|project| project.id == project_id)
            .map(|project| (project.repo_path.clone(), project.base_branch.clone()))
            .ok_or("unknown project_id")?;
        let canonical_project_path = project_path.display().to_string();

        self.require_store()?;
        // A completed record makes the mutation idempotent. A pending record is
        // the crash/failure-safe resume point and uses only the same server ids.
        let mut resume = None;
        if let Some(record) = self.board.archived(&worktree_id).cloned() {
            if record.project_path == canonical_project_path {
                if record.action != action {
                    return Err(format!(
                        "worktree.finish already started with action {:?}",
                        record.action
                    ));
                }
                if record.status == WorktreeFinishStatus::Archived {
                    return Ok(PlannedFinish::Settled(archived_worktree_json(&record)));
                }
                resume = Some(record);
            }
        }

        // The claim is the last thing taken and the first thing the epilogue
        // gives back: past this point the checkout belongs to this finish until
        // its git work returns.
        let store = self.require_store()?.clone();
        let excluded = self.bound_worktree_paths();
        if !self.finishing_worktrees.insert(worktree_id.clone()) {
            return Err(format!(
                "worktree {worktree_id} is already finishing — wait for that to complete"
            ));
        }
        Ok(PlannedFinish::Deferred(Box::new(WorktreeFinishJob {
            worktrees: self.orch_for(&project_id)?.worktrees().clone(),
            project_id,
            base_branch,
            worktree_id,
            action,
            excluded,
            resume,
            store,
            #[cfg(test)]
            gate: self.off_lock_gate.clone(),
        })))
    }

    /// `branch.get` — resolve `(project_id, branch)` to the work item behind
    /// it, with the full underlying run view (`run_view`) when a run owns the
    /// branch and `run: null` when the checkout is bare.
    pub(in crate::app) fn branch_get(&mut self, params: &Value) -> Result<Value, String> {
        let project_id = require_str(params, "project_id")?;
        let branch = require_str(params, "branch")?;
        if !self.projects.iter().any(|p| p.id == project_id) {
            return Err(format!("unknown project_id: {project_id}"));
        }
        let checkouts = self.external_worktrees_json();
        let primary_changes = self.primary_changes_json();
        let found = self
            .work_items(&checkouts.rows, &primary_changes)
            .into_iter()
            .find(|row| {
                row["kind"] == crate::branch::WorkItemKind::Branch.as_str()
                    && row["project_id"] == json!(project_id)
                    && row["branch"] == json!(branch)
            });
        let Some(mut row) = found else {
            // This project's own scan, not the rail's board-wide flag: what a
            // neighbour has or has not been scanned for says nothing about the
            // branch that was asked for here.
            let settled = self.scan_settled_at(&project_id).is_some();
            self.rescan_external_worktrees(&project_id);
            return Err(format!(
                "branch.get: no checkout of this project is on branch {branch} ({})",
                scan_may_yet_show_it(settled)
            ));
        };
        let run = match row["run_id"].as_str().map(str::to_string) {
            Some(run_id) => {
                let active = self.runs.get(&run_id).expect("the row named a live run");
                // The branch surface sits under the rail: it reads the
                // conversation of whichever agent's bubble is open. See
                // `run_get`.
                let detail_thread = self.detail_thread_value(&run_id, params)?;
                let mut view = self.run_view(
                    &run_id,
                    active,
                    view_thread_detail(&detail_thread, params),
                    DigestScope::Detail,
                );
                if let Some(thread) = detail_thread {
                    view["thread"] = thread;
                }
                view
            }
            // A checkout Build owns no run in has no agent to name.
            None => match named_agent_id(params)? {
                Some(agent_id) => return Err(format!("unknown agent_id: {agent_id}")),
                None => Value::Null,
            },
        };
        if let Some(run_agents) = run.get("agents") {
            row["agents"] = run_agents.clone();
        }
        row["run"] = run;
        Ok(row)
    }

    /// `branch.finish` — the inbox entry's Done, for a branch.
    ///
    /// Done on a branch DELETES it: the checkout goes through the same durable
    /// path as `worktree.finish`, the branch goes with it, and the run's
    /// records and conversation leave the inbox. It is never refused for the
    /// state of the work — an unpushed commit, an unmerged branch and an
    /// uncommitted edit are warnings the row carries (`finish.warnings`) and
    /// the user confirms through. `action` chooses how the checkout is retired
    /// and defaults to `delete`, which is what Done means.
    ///
    /// An issue the branch implements only ends with it when the work landed:
    /// a merge finishes the issue too, and any other ending hands the issue
    /// back to the inbox with an event naming the branch it lost.
    /// `unlink: true` leaves the issue alone either way.
    pub(in crate::app) fn branch_finish(&mut self, params: &Value) -> Result<Value, String> {
        let project_id = require_str(params, "project_id")?;
        let branch = require_str(params, "branch")?;
        let action_name = params
            .get("action")
            .and_then(Value::as_str)
            .filter(|action| !action.is_empty())
            .unwrap_or("delete")
            .to_string();
        parse_worktree_finish_action(&action_name)?;
        let unlink = params
            .get("unlink")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let Some(run_id) = self.run_on_branch(&project_id, &branch) else {
            // No run behind the branch: it is a bare checkout, and the durable
            // archive path is the same one `run.finish` delegates to.
            //
            // The last scan is what maps the branch to a checkout id. Resolving
            // it is not the authority for what gets deleted — the job rescans
            // and re-resolves the id itself — so a stale hit fails closed there
            // rather than costing every other frame a scan under this lock.
            let worktree = self.find_checkout(
                &project_id,
                &format!("branch.finish: no checkout of this project is on branch {branch}"),
                |checkout| checkout.branch.as_deref() == Some(branch.as_str()),
            )?;
            let planned = self.plan_worktree_finish(&json!({
                "project_id": project_id,
                "worktree_id": worktree.id,
                "action": action_name,
            }))?;
            let epilogue = BranchFinishEpilogue {
                branch,
                run: None,
                issue_id: None,
                orphaned_issue_id: None,
            };
            return Ok(match planned {
                PlannedFinish::Settled(archived) => {
                    self.apply_branch_finish(epilogue, Ok(archived))?
                }
                PlannedFinish::Deferred(job) => {
                    let epilogue = job.epilogue(FinishKind::Branch(epilogue));
                    self.defer_finish(job, epilogue)
                }
            });
        };
        let implemented_issue_id = self.runs[&run_id]
            .run
            .plan_id
            .as_ref()
            .map(|id| id.0.clone())
            // An issue already filed away has nothing left to hear about this.
            .filter(|issue_id| {
                self.plans
                    .get(issue_id)
                    .is_some_and(|issue| issue.plan.archived_at.is_none())
            });
        // Whether the work landed. That is the whole question an issue's fate
        // turns on: a merge publishes it into the base branch and the issue is
        // done with the branch; anything else deletes work the issue was
        // waiting for, so the issue comes back to the inbox and has to be told
        // what happened to the branch that was speaking for it.
        let merged = matches!(
            parse_worktree_finish_action(&action_name),
            Ok(WorktreeFinishAction::Merge)
        ) || self.runs[&run_id].run.state == RunState::Merged;
        let issue_id = implemented_issue_id.clone().filter(|_| !unlink && merged);
        let orphaned_issue_id = implemented_issue_id.filter(|_| !merged);
        let planned =
            self.plan_finish_run(&run_id, &action_name, FinishRequirement::Unconditional)?;
        let epilogue = |run| BranchFinishEpilogue {
            branch,
            run,
            issue_id,
            orphaned_issue_id,
        };
        match planned {
            // The checkout was already gone: there is no branch left to finish
            // and no issue news to file, only the run's own retirement.
            PlannedRunFinish::Settled(archived) => {
                self.apply_branch_finish(epilogue(None), Ok(archived))
            }
            PlannedRunFinish::Replay { archived, run } => {
                self.apply_branch_finish(epilogue(Some(run)), Ok(archived))
            }
            PlannedRunFinish::Deferred { job, run } => {
                let epilogue = job.epilogue(FinishKind::Branch(epilogue(Some(run))));
                Ok(self.defer_finish(job, epilogue))
            }
        }
    }

    /// Settle the issue behind a finished branch, once the branch is actually
    /// gone: a merge files the issue away with it, anything else hands the
    /// issue back to the inbox with an event naming the branch it lost.
    pub(in crate::app) fn apply_branch_finish(
        &mut self,
        epilogue: BranchFinishEpilogue,
        archived: Result<Value, String>,
    ) -> Result<Value, String> {
        let BranchFinishEpilogue {
            branch,
            run,
            issue_id,
            orphaned_issue_id,
        } = epilogue;
        let run_id = run.as_ref().map(|run| run.run_id.clone());
        let finished = match run {
            Some(run) => self.apply_run_finish(run, archived)?,
            None => archived?,
        };
        if let (Some(orphaned_issue_id), Some(run_id)) = (&orphaned_issue_id, &run_id) {
            self.note_implementation_abandoned(
                orphaned_issue_id,
                run_id,
                &branch,
                "finished off the board",
            );
        }
        let issue_archived = match &issue_id {
            Some(issue_id) => {
                self.plan_archive(&json!({ "plan_id": issue_id }))?;
                true
            }
            None => false,
        };
        Ok(json!({
            "branch": branch,
            "run_id": run_id,
            "issue_id": issue_id,
            "issue_archived": issue_archived,
            "issue_abandoned": orphaned_issue_id.is_some(),
            "worktree": finished,
        }))
    }

    /// The checkouts of a project the mutex can name without touching the
    /// disk: the external scan the board already holds, every live run's
    /// checkout, and the primary. Which branch each one holds is git's to
    /// answer, and [`ProjectCheckouts::holders`] asks it.
    ///
    /// Only owned inputs are captured here. Every caller asks `holders` in
    /// its off-lock phase, including one-shot creates and dispatches.
    pub(in crate::app) fn project_checkouts(
        &self,
        project_id: &str,
    ) -> Result<ProjectCheckouts, String> {
        Ok(ProjectCheckouts {
            project: self.orch_for(project_id)?.clone(),
            primary_repo_path: self.repo_path_for(project_id)?,
            base_branch: self.base_for(project_id)?,
            excluded: self.bound_worktree_paths(),
            run_checkouts: self
                .live_runs_of(project_id)
                .map(|(id, run)| (id.clone(), run.worktree.clone()))
                .collect(),
        })
    }

    /// Recheck the records the off-lock holder reading was based on.
    pub(in crate::app) fn validate_checkout_snapshot(
        &self,
        project_id: &str,
        snapshot: &ProjectCheckouts,
    ) -> Result<(), String> {
        let project = self.project_for(project_id)?;
        let runs = self.live_runs_of(project_id).collect::<Vec<_>>();
        let unchanged = project.repo_path == snapshot.primary_repo_path
            && project.base_branch == snapshot.base_branch
            && runs.len() == snapshot.run_checkouts.len()
            && snapshot.run_checkouts.iter().all(|(id, checkout)| {
                runs.iter()
                    .any(|(current_id, active)| *current_id == id && active.worktree == *checkout)
            });
        if unchanged {
            Ok(())
        } else {
            Err("the project's branch holders changed while Git ran; retry the action".to_string())
        }
    }

    /// The id the board carries for a checkout that does not exist yet: the id
    /// its path will hash to once `git worktree add` has made it.
    /// [`WorktreeManager::create`] suffixes a slug something is already using,
    /// which the decide phase cannot know, so this is what the epilogue settles
    /// under unless it had to.
    ///
    /// [`WorktreeManager::create`]: crate::worktree::WorktreeManager::create
    pub(in crate::app) fn planned_checkout_id(
        &self,
        project_id: &str,
        slug: &str,
    ) -> Result<String, String> {
        let planned = self.orch_for(project_id)?.planned_checkout_path(slug);
        Ok(crate::worktree::external_worktree_id(
            &crate::worktree::canonical_planned_path(&planned),
        ))
    }

    /// The primary checkout of the project an entity belongs to — where a
    /// project-wide artifact like the review rules lives, rather than in
    /// whichever worktree happened to notice it.
    pub(in crate::app) fn primary_checkout_of(
        &self,
        entity_id: &str,
    ) -> Result<std::path::PathBuf, String> {
        let project_id = self.project_of(entity_id)?;
        self.repo_path_for(&project_id)
    }
}

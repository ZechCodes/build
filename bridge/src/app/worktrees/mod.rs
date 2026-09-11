use crate::app::{
    named_agent_id, require_str, scan_may_yet_show_it, view_thread_detail, AppState, DigestScope,
};
use crate::lifecycle::holders::ProjectCheckouts;
use crate::lifecycle::{CreateWorktree, PendingRow};
use serde_json::{json, Value};

pub(in crate::app) mod dispatch;

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

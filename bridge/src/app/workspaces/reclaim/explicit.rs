//! `workspace.reclaim`: remove a workspace whose work is somewhere else and
//! whose issues are finished, logged on each of those issues.
//!
//! Three steps, because measuring Git is unbounded and the app mutex must not
//! wait on it. Under the mutex, the holds only the app state knows are read
//! and the workspace is reserved, so nothing starts in it. With the mutex
//! released, the Git state Done reads is measured. Under the mutex again, the
//! reservation ends, every hold is read once more, and the removal is handed
//! to the drain the way `workspace.delete` hands it: every agent and terminal
//! anywhere in the workspace stops first, and the files go only once they
//! have.

use crate::app::git::deferred::DeferredGitWork;
use crate::app::{AppState, DeferredGit, DeferredWork};
use crate::reclaim::LinkedIssue;
use crate::tracker::{Actor, IssueEventKind};
use crate::workspace::Workspace;
use serde_json::{json, Value};
use std::sync::Mutex;

/// The Git measurement, run with the mutex released, and the decision it
/// feeds, made under it.
struct MeasureBeforeReclaim {
    workspace: Workspace,
    actor: Actor,
    params: Value,
    /// What the measurement found, for the decision to read.
    measured: Mutex<Vec<&'static str>>,
}

impl DeferredGitWork for MeasureBeforeReclaim {
    fn run(&self, _: &Value) -> Result<Value, String> {
        let blockers = crate::workspace::workspace_directory_blockers(&self.workspace);
        *self
            .measured
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = blockers;
        // Always an answer: the decision is what ends the reservation.
        Ok(Value::Null)
    }

    fn invalidate(&self, _: &mut AppState) {}

    fn settle(&self, app: &mut AppState, _: Value) -> Result<Value, String> {
        let measured = self
            .measured
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        app.decide_reclaim(&self.workspace, &self.actor, &self.params, &measured)
    }
}

impl AppState {
    /// `workspace.reclaim`, called by the user or, through `reclaim_workspace`,
    /// the project agent. Refuses at once on what the app state already knows,
    /// then hands the Git measurement to the drain.
    pub(crate) fn workspace_reclaim(
        &mut self,
        params: &Value,
        actor: &Actor,
    ) -> Result<Value, String> {
        let workspace = self.workspace_to_remove(params)?;
        self.refuse_removing_what_is_not_builds(&workspace)?;
        let issues = self.issues_of_workspace(&workspace.id);
        let holds = self.live_holds(&workspace, &issues);
        if !holds.is_empty() {
            return Err(reclaim_refusal(&workspace.name, &holds));
        }
        self.reserve_workspace(&workspace.id);
        let answer = json!({ "workspace_id": workspace.id, "deleted": true });
        self.deferred_work = Some(DeferredWork::Git(Box::new(DeferredGit {
            call: Box::new(MeasureBeforeReclaim {
                workspace,
                actor: actor.clone(),
                params: params.clone(),
                measured: Mutex::new(Vec::new()),
            }),
            params: params.clone(),
            invalidates: false,
            #[cfg(test)]
            gate: None,
        })));
        Ok(answer)
    }

    /// The measurement is back: end the reservation, read every hold again,
    /// and either refuse or hand the removal to the drain.
    fn decide_reclaim(
        &mut self,
        measured_workspace: &Workspace,
        actor: &Actor,
        params: &Value,
        measured: &[&'static str],
    ) -> Result<Value, String> {
        self.release_reservation(&measured_workspace.id);
        let workspace = self
            .workspaces
            .get(&measured_workspace.id)
            .cloned()
            .ok_or_else(|| format!("unknown workspace_id: {}", measured_workspace.id))?;
        if self.deferred_work.is_some() || self.active_deferred_filesystem_jobs > 0 {
            return Err(crate::reclaim::BUSY.to_string());
        }
        self.refuse_removing_what_is_not_builds(&workspace)?;
        let issues = self.issues_of_workspace(&workspace.id);
        let mut holds = self.live_holds(&workspace, &issues);
        for blocker in measured {
            if !holds.contains(blocker) {
                holds.push(blocker);
            }
        }
        if !holds.is_empty() {
            return Err(reclaim_refusal(&workspace.name, &holds));
        }
        self.log_reclaim(&workspace, &issues.unwrap_or_default(), actor);
        self.remove_workspace(&workspace, params, None)?;
        self.workspace_lifecycle.remove(&workspace.id);
        self.persist_workspace_lifecycle();
        Ok(json!({ "workspace_id": workspace.id, "deleted": true }))
    }

    /// The issues linking one workspace, or why they could not be read.
    fn issues_of_workspace(&self, workspace_id: &str) -> Result<Vec<LinkedIssue>, String> {
        super::issues_of(&self.issues_linking_workspaces(), workspace_id)
    }

    /// Each linked issue records the reclaim before the removal starts, the
    /// way Done closes them first. Quietly: nobody watching the issue is
    /// woken for it.
    fn log_reclaim(&mut self, workspace: &Workspace, issues: &[LinkedIssue], actor: &Actor) {
        let payload = json!({
            "workspace_id": workspace.id,
            "workspace_name": workspace.name,
            "size_bytes": self
                .workspace_lifecycle
                .get(&workspace.id)
                .and_then(|record| record.size_bytes),
            "branches": workspace
                .directories
                .iter()
                .filter_map(|directory| directory.branch.clone())
                .collect::<Vec<_>>(),
        });
        for issue in issues {
            if let Err(error) = self.record_quiet_event(
                &issue.issue_id,
                actor,
                IssueEventKind::WorkspaceReclaimed,
                payload.clone(),
            ) {
                eprintln!("workspace reclaim: log on #{}: {error}", issue.number);
            }
        }
    }
}

/// The refusal, as a sentence the user can read.
fn reclaim_refusal(name: &str, holds: &[&str]) -> String {
    let mut holds = holds.to_vec();
    holds.sort_by_key(|hold| crate::reclaim::hold_order(hold));
    format!(
        "{}{name} yet: {}.",
        crate::reclaim::REFUSAL,
        holds
            .iter()
            .map(|hold| crate::reclaim::hold_sentence(hold))
            .collect::<Vec<_>>()
            .join("; ")
    )
}

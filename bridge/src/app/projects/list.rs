use super::Project;
use crate::app::{AppState, DeferredRead, DeferredWork, ProjectListRow, ReadSubject};
#[cfg(test)]
use crate::worktree::git_remote_origin;
use serde_json::{json, Value};

impl AppState {
    /// All registered projects, for the New-task picker and Settings.
    #[cfg(test)]
    pub(in crate::app) fn project_list(&self) -> Value {
        let projects: Vec<Value> = self
            .projects
            .iter()
            .map(|project| {
                let remote = project
                    .is_git
                    .then(|| git_remote_origin(&project.repo_path))
                    .flatten();
                self.project_json(project, remote)
            })
            .collect();
        json!({ "projects": projects })
    }

    /// Capture project identity and settings under the app mutex, then leave
    /// repository and volume probes to the deferred-read drain. The answer is
    /// a coherent snapshot: registration changes while the probes run affect
    /// the next list request, not this one.
    pub(crate) fn defer_project_list(&mut self) -> Value {
        let projects = self
            .projects
            .iter()
            .map(|project| ProjectListRow {
                project_id: project.id.clone(),
                name: project.name.clone(),
                repo_path: project.repo_path.clone(),
                worktrees_root: self.project_worktrees_root(&project.id),
                base_branch: project.base_branch.clone(),
                is_git: project.is_git,
                isolation: project.isolation,
                isolation_default: self.isolation,
            })
            .collect();
        self.deferred_work = Some(DeferredWork::Read(Box::new(DeferredRead {
            subject: ReadSubject::ProjectList { projects },
            issue_id: None,
            if_diff_key: None,
            #[cfg(test)]
            gate: self.off_lock_project_list_gate.clone(),
        })));
        Value::Null
    }

    /// The wire row for a project: what it is, and the whole isolation picture
    /// a control paints from — what this project chose (`null` while it
    /// inherits), what the account chose, what its next checkout will be, and
    /// what its volume can make.
    ///
    /// The remote is passed in rather than read here: a verb that just wrote it
    /// knows what it wrote, and every read of it is a git subprocess that has
    /// to be made somewhere the caller can see.
    pub(in crate::app) fn project_json(&self, p: &Project, remote: Option<String>) -> Value {
        let available = p.orch.worktrees().availability();
        let effective = self.decide_isolation(p, &available).isolation;
        json!({
            "project_id": p.id,
            "name": p.name,
            "path": p.repo_path.display().to_string(),
            "base_branch": p.base_branch,
            "is_git": p.is_git,
            "remote": remote,
            "isolation": p.isolation,
            "isolation_default": self.isolation,
            "isolation_effective": effective,
            "isolation_available": available,
        })
    }
}

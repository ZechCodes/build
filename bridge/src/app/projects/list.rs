use super::{Project, ProjectSource};
use crate::app::{AppState, DeferredRead, DeferredWork, ProjectListRow, ReadSubject};
use serde_json::{json, Value};

impl AppState {
    /// All registered projects, rendered here rather than deferred.
    ///
    /// `project.list` itself defers, and the board item that carries the
    /// whole list renders off the lock too ([`crate::app::BoardListFacts`]):
    /// the render reads each repository and its volume, and nothing may hold
    /// the mutex for that. The remaining callers are the synchronous tests.
    #[cfg(test)]
    pub(in crate::app) fn project_list(&self) -> Value {
        let projects: Vec<Value> = self
            .project_list_rows()
            .iter()
            .map(ProjectListRow::render)
            .collect();
        json!({ "projects": projects })
    }

    /// One project per registered project, as the list answers for it: identity
    /// and settings read under the app mutex, and the conversation owner it has
    /// right now — `None` for a project nobody has talked to yet, because a
    /// read may never mint one.
    pub(in crate::app) fn project_list_rows(&self) -> Vec<ProjectListRow> {
        self.projects
            .iter()
            .map(|project| {
                let conversation = self.project_conversation_run(&project.id);
                ProjectListRow {
                    project_id: project.id.clone(),
                    name: project.name.clone(),
                    repo_path: project.repo_path.clone(),
                    worktrees_root: self.project_worktrees_root(&project.id),
                    base_branch: project.base_branch.clone(),
                    is_git: project.is_git,
                    sources: project.sources.clone(),
                    isolation: project.isolation,
                    isolation_default: self.isolation,
                    conversations: conversation
                        .as_deref()
                        .map(|id| self.conversation_activity_rows(id))
                        .unwrap_or_default(),
                    session: self.session_summary(&project.id),
                    conversation,
                }
            })
            .collect()
    }

    /// Capture project identity and settings under the app mutex, then leave
    /// repository and volume probes to the deferred-read drain. The answer is
    /// a coherent snapshot: registration changes while the probes run affect
    /// the next list request, not this one.
    pub(crate) fn defer_project_list(&mut self) -> Value {
        let projects = self.project_list_rows();
        self.deferred_work = Some(DeferredWork::Read(Box::new(DeferredRead {
            subject: ReadSubject::ProjectList { projects },
            task_id: None,
            if_diff_key: None,
            with_patch: true,
            paths: None,
            range: None,
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
    /// A project has no remote of its own: each source's is its checkout's
    /// `origin` ([`ProjectSource::origin`]). The row's `remote` is its first
    /// source's, kept for clients that predate per-source remotes.
    pub(in crate::app) fn project_json(&self, p: &Project) -> Value {
        let available = p.orch.worktrees().availability();
        let effective = self.decide_isolation(p, &available).isolation;
        let sources = p
            .sources
            .iter()
            .map(ProjectSource::wire)
            .collect::<Vec<_>>();
        json!({
            "project_id": p.id,
            "name": p.name,
            "path": p.repo_path.display().to_string(),
            "base_branch": p.base_branch,
            "is_git": p.is_git,
            "remote": primary_remote(&sources),
            "sources": sources,
            "isolation": p.isolation,
            "isolation_default": self.isolation,
            "isolation_effective": effective,
            "isolation_available": available,
            "session_started_ms": self.session_summary(&p.id).session_started_ms,
            "last_activity_ms": self.session_summary(&p.id).last_activity_ms,
        })
    }
}

/// The remote a client that predates per-source remotes shows for the whole
/// project: its first source's.
pub(in crate::app) fn primary_remote(sources: &[Value]) -> Value {
    sources
        .first()
        .map(|source| source["remote"].clone())
        .unwrap_or(Value::Null)
}

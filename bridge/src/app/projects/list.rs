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
        #[cfg(test)]
        crate::app::snapshot_projection::work_counts::project_capture::capture();
        #[cfg(test)]
        let _phase = crate::timing::phase_samples::phase("publication.project_rows.capture");
        let workspace_counts = self.project_workspace_counts();
        self.projects
            .iter()
            .map(|project| {
                #[cfg(test)]
                crate::app::snapshot_projection::work_counts::project_capture::project(&project.id);
                let conversation = self.project_snapshot_conversation_run(&project.id);
                let scope = crate::compaction::CompactionScope::Project(project.id.clone());
                let (open_tasks, total_tasks) = self.project_task_counts(&project.id);
                ProjectListRow {
                    compaction_thresholds: self
                        .compaction_thresholds(&scope)
                        .unwrap_or_default()
                        .read_value(),
                    compaction: self.scope_compaction_observation(&scope),
                    project_id: project.id.clone(),
                    environment_id: project.environment_id.clone(),
                    environment_name: self
                        .projects
                        .environment(&project.environment_id)
                        .map(|e| e.name.clone())
                        .unwrap_or_default(),
                    project_key: project.project_key.clone(),
                    diverged: !project.added_sources.is_empty(),
                    device: self.device_name.clone(),
                    open_tasks,
                    total_tasks,
                    workspaces: workspace_counts
                        .get(project.id.as_str())
                        .copied()
                        .unwrap_or_default(),
                    name: project.name.clone(),
                    repo_path: project.home_path.clone(),
                    worktrees_root: self.project_worktrees_root(&project.id),
                    base_branch: project.base_branch.clone(),
                    is_git: project.is_git,
                    sources: project.sources.clone(),
                    added_sources: project.added_sources.clone(),
                    isolation: project.isolation,
                    isolation_default: self.environment_isolation(project),
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
        self.revive_parked_projects();
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
        let value = json!({
            "project_id": p.id,
            "environment_id": p.environment_id,
            "environment_name": self.projects.environment(&p.environment_id).map(|environment| &environment.name),
            "compaction_thresholds": self.compaction_thresholds(&crate::compaction::CompactionScope::Project(p.id.clone())).unwrap_or_default().read_value(),
            "compaction": self.scope_compaction_observation(&crate::compaction::CompactionScope::Project(p.id.clone())),
            "name": p.name,
            "path": p.home_path.display().to_string(),
            "base_branch": p.base_branch,
            "is_git": p.is_git,
            "remote": primary_remote(&sources),
            "sources": sources,
            "added_sources": p.added_sources.iter().map(ProjectSource::wire).collect::<Vec<_>>(),
            "diverged": !p.added_sources.is_empty(),
            "isolation": p.isolation,
            "isolation_default": self.environment_isolation(p),
            "isolation_effective": effective,
            "isolation_available": available,
            "session_started_ms": self.session_summary(&p.id).session_started_ms,
            "last_activity_ms": self.session_summary(&p.id).last_activity_ms,
        });
        self.record_project_snapshot_observation(&p.id, &value);
        value
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

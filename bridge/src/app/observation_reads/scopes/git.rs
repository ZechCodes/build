use super::*;
use crate::app::git::{optional_scope_id, require_changeset_paths};
use crate::app::projects::MOVED_GIT_SOURCE;

impl ScopeReadContext {
    pub(super) fn prepare_git(&self, method: &str, params: &Value) -> Result<ScopeReadJob, String> {
        let mut fence = ScopeReadFence::new(MOVED_GIT_SOURCE);
        let mut scope = self.git_scope(params, &mut fence)?;
        let method = method.to_owned();
        let params = params.clone();
        Ok(ScopeReadJob::new(fence, move || {
            // Original source resolution used to canonicalize while holding the
            // app. It remains a real filesystem check, now owned by this job.
            if let Some(source) = scope.source.as_mut() {
                let root = std::fs::canonicalize(&source.root)
                    .map_err(|error| format!("cannot resolve scope root: {error}"))?;
                source.root = root.clone();
                scope.repo_path = root;
            }
            scope.read(&method, &params)
        }))
    }

    pub(super) fn prepare_worktree_diff(&self, params: &Value) -> Result<ScopeReadJob, String> {
        let project = crate::app::require_str(params, "project_id")?;
        let id = crate::app::require_str(params, "worktree_id")?;
        let external = self.external(&project, &id)?;
        let mut fence = ScopeReadFence::new(MOVED_GIT_SOURCE);
        fence.bindings.push(self.project_binding(&project)?);
        fence
            .bindings
            .push(self.external_binding(&project, &external));
        let subject = ReadSubject::Worktree {
            external: Box::new(external),
            base_branch: self.project(&project)?.base_branch.clone(),
        };
        Ok(Self::diff_job(subject, None, params, fence))
    }

    pub(super) fn prepare_run_diff(&self, params: &Value) -> Result<ScopeReadJob, String> {
        let id = crate::app::require_str(params, "run_id")?;
        let run = self.run(&id)?;
        if run.project_id.is_none() {
            return Err("unknown entity id".into());
        }
        let mut fence = ScopeReadFence::new(MOVED_GIT_SOURCE);
        fence.bindings.push(self.run_binding(&id)?);
        Ok(Self::diff_job(run.subject(), None, params, fence))
    }

    pub(super) fn prepare_changeset(&self, params: &Value) -> Result<ScopeReadJob, String> {
        let paths = require_changeset_paths(params)?;
        let range = crate::body_page::BodyRange::from_params(params)?;
        if range.is_some() && paths.len() != 1 {
            return Err("git.changeset_diff takes one path with a range".into());
        }
        let mut fence = ScopeReadFence::new(MOVED_GIT_SOURCE);
        let subject = self.changeset_subject(params, &mut fence)?;
        let read = crate::app::runtime::deferred::DeferredRead {
            subject,
            task_id: None,
            if_diff_key: params
                .get("if_diff_key")
                .and_then(Value::as_str)
                .map(str::to_owned),
            with_patch: true,
            paths: Some(paths),
            range,
            #[cfg(test)]
            gate: None,
        };
        Ok(ScopeReadJob::new(fence, move || read.run()))
    }

    fn changeset_subject(
        &self,
        params: &Value,
        fence: &mut ScopeReadFence,
    ) -> Result<ReadSubject, String> {
        if let Some(scope) = self.source_git_scope(params, fence)? {
            return Ok(scope.source_changeset());
        }
        if let Some(id) = optional_scope_id(params, "run_id")? {
            let run = self.run(&id)?;
            if run.project_id.is_none() {
                return Err("unknown entity id".into());
            }
            fence.bindings.push(self.run_binding(&id)?);
            return Ok(run.subject());
        }
        let project = crate::app::require_str(params, "project_id")?;
        fence.bindings.push(self.project_binding(&project)?);
        if let Some(id) = optional_scope_id(params, "worktree_id")? {
            let external = self.external(&project, &id)?;
            fence
                .bindings
                .push(self.external_binding(&project, &external));
            return Ok(ReadSubject::Worktree {
                external: Box::new(external),
                base_branch: self.project(&project)?.base_branch.clone(),
            });
        }
        let record = self.project(&project)?;
        if !record.is_git {
            return Err("project is not a git repository; initialize Git first".into());
        }
        Ok(ReadSubject::Project {
            project_id: project,
            repo_path: record.path.clone(),
            key_namespace: None,
        })
    }
}

impl RunInput {
    fn subject(&self) -> ReadSubject {
        ReadSubject::Run {
            worktree_path: self.git_root.clone(),
            base_sha: self.base_sha.clone(),
            base_branch: self.base_branch.clone(),
        }
    }
}

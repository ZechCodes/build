#[cfg(test)]
use crate::app::OffLockGate;
use serde_json::Value;

use super::super::AppState;
use super::GitScope;

/// One `git.*` verb: the checkout the app mutex resolved for it, the git call
/// to make there with the mutex released, and whether its answer invalidates
/// the summaries the board reads.
///
/// A `git status` walks the whole worktree and a `git fetch` waits on a
/// network; the review surfaces poll both. Neither may hold the daemon still.
pub(in crate::app) struct DeferredGit {
    pub(in crate::app) call: Box<dyn DeferredGitWork>,
    pub(in crate::app) params: Value,
    /// Whether a successful call made the scope's cached summaries stale.
    pub(in crate::app) invalidates: bool,
    #[cfg(test)]
    pub(in crate::app) gate: Option<OffLockGate>,
}

impl GitCallScope for GitScope {
    fn mutation_path(&self) -> Option<&std::path::Path> {
        // Legacy project-only scope reaches the same original source.
        (self.source.is_some()
            || (self.project_id.is_some() && self.run.is_none() && self.worktree.is_none()))
        .then_some(self.repo_path.as_path())
    }

    fn validate(&self, app: &AppState) -> Result<(), String> {
        if let Some(source) = &self.source {
            app.validate_source_location(source)?;
        } else if let Some(project_id) = &self.project_id {
            if self.run.is_none()
                && self.worktree.is_none()
                && !app
                    .projects
                    .get(project_id)
                    .is_some_and(|project| project.repo_path == self.repo_path)
            {
                return Err(super::super::projects::MOVED_GIT_SOURCE.to_string());
            }
        }
        Ok(())
    }

    fn invalidate(&self, app: &mut AppState) {
        if app.git_scope_is_current(self) {
            app.invalidate_git_scope_caches(self);
        }
    }
}

impl<S: GitCallScope> DeferredGitWork for ScopedGitCall<S> {
    fn run(&self, params: &Value) -> Result<Value, String> {
        let _held = if self.serialized {
            self.scope
                .mutation_path()
                .map(|path| {
                    crate::source_sync::SyncLock::acquire(
                        path,
                        crate::source_sync::SERVICE_FETCH_DEADLINE,
                    )
                    .map(|(held, _)| held)
                    .ok_or_else(|| {
                        "Another source operation is still running; try again.".to_string()
                    })
                })
                .transpose()?
        } else {
            None
        };
        if let Some(state) = self.state.as_ref().and_then(std::sync::Weak::upgrade) {
            self.scope.validate(&state.lock().unwrap())?;
        }
        (self.work)(&self.scope, params)
    }

    fn invalidate(&self, app: &mut AppState) {
        self.scope.invalidate(app);
    }
}

impl DeferredGit {
    pub(in crate::app) fn run(&self) -> Result<Value, String> {
        self.call.run(&self.params)
    }
}

/// The git call a verb handed to the drain: what to run with the mutex
/// released, and which cached summaries to drop once it has changed the tree
/// underneath them.
pub(in crate::app) trait DeferredGitWork: Send {
    fn run(&self, params: &Value) -> Result<Value, String>;
    fn invalidate(&self, app: &mut AppState);

    fn settle(&self, _app: &mut AppState, result: Value) -> Result<Value, String> {
        Ok(result)
    }

    /// Provisioning can durably record partial progress before returning an
    /// error. Its in-memory view must still reconcile with that journal.
    fn invalidates_on_error(&self) -> bool {
        false
    }
}

/// A resolution the app mutex made for a git verb — which checkout, which
/// project, which cached summaries describe it — and what to drop from those
/// caches once a call against it has changed the tree.
pub(in crate::app) trait GitCallScope: Send {
    fn invalidate(&self, app: &mut AppState);
    fn mutation_path(&self) -> Option<&std::path::Path> {
        None
    }
    fn validate(&self, _app: &AppState) -> Result<(), String> {
        Ok(())
    }
}

/// One resolved scope and the call to make against it.
///
/// The scope and the function are one value because they are one decision —
/// the verb that defers the work picks both at once, and no other pairing can
/// be spelled. The call itself is a plain function of the scope and the
/// request, so it holds no state. Only the scope preflight briefly rechecks app records
/// after its source lock acquisition.
pub(in crate::app) struct ScopedGitCall<S: GitCallScope> {
    pub(in crate::app) scope: S,
    pub(in crate::app) work: fn(&S, &Value) -> Result<Value, String>,
    pub(in crate::app) serialized: bool,
    pub(in crate::app) state: Option<std::sync::Weak<std::sync::Mutex<AppState>>>,
}

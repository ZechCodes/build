#[cfg(test)]
use crate::app::OffLockGate;
use serde_json::Value;

use super::super::AppState;
use super::{BranchListingScope, BranchScope, GitScope};

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
    fn invalidate(&self, app: &mut AppState) {
        if app.git_scope_is_current(self) {
            app.invalidate_git_scope_caches(self);
        }
    }
}

impl GitCallScope for BranchScope {
    fn invalidate(&self, app: &mut AppState) {
        app.invalidate_branch_scope_caches(self);
    }
}

impl GitCallScope for BranchListingScope {
    fn invalidate(&self, app: &mut AppState) {
        self.checkout.invalidate(app);
    }
}

impl<S: GitCallScope> DeferredGitWork for ScopedGitCall<S> {
    fn run(&self, params: &Value) -> Result<Value, String> {
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
}

/// A resolution the app mutex made for a git verb — which checkout, which
/// project, which cached summaries describe it — and what to drop from those
/// caches once a call against it has changed the tree.
pub(in crate::app) trait GitCallScope: Send {
    fn invalidate(&self, app: &mut AppState);
}

/// One resolved scope and the call to make against it.
///
/// The scope and the function are one value because they are one decision —
/// the verb that defers the work picks both at once, and no other pairing can
/// be spelled. The call itself is a plain function of the scope and the
/// request, so it holds no state and cannot reach the daemon while it runs.
pub(in crate::app) struct ScopedGitCall<S: GitCallScope> {
    pub(in crate::app) scope: S,
    pub(in crate::app) work: fn(&S, &Value) -> Result<Value, String>,
}

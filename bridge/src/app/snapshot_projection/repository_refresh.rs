//! Repository jobs carry environment evidence through their off-lock work.
use super::*;

/// Shared with settlement so even a partially failed filesystem mutation can
/// publish the evidence it collected. Neither phase retains the app mutex.
#[derive(Clone)]
pub(in crate::app) struct RepositoryRefresh {
    state: Arc<Mutex<RefreshState>>,
    clock: super::super::read_view::ReadViewSlot,
    #[cfg(test)]
    gate: Option<crate::app::OffLockGate>,
    #[cfg(test)]
    publication_gate: Option<crate::app::OffLockGate>,
}

enum RefreshState {
    Pending(ProjectSnapshotRefresh, Vec<PathBuf>),
    Observed(RenderedProjectSnapshotRefresh, Vec<PathBuf>),
    Taken,
}

impl AppState {
    /// Capture identities only. Common Git directories are resolved by the job.
    pub(in crate::app) fn capture_repository_roots(&self) -> Vec<PathBuf> {
        self.worktree_roots()
            .into_values()
            .chain(
                self.projects
                    .registered_sources()
                    .into_iter()
                    .map(|s| s.path),
            )
            .chain(
                self.workspaces
                    .list(None)
                    .into_iter()
                    .flat_map(|workspace| workspace.directories.iter().map(|d| d.path.clone())),
            )
            .collect()
    }
    pub(in crate::app) fn capture_repository_refresh(&self) -> RepositoryRefresh {
        RepositoryRefresh {
            state: Arc::new(Mutex::new(RefreshState::Pending(
                self.capture_project_snapshot_refresh(),
                self.capture_repository_roots(),
            ))),
            clock: self.read_view.clone(),
            #[cfg(test)]
            gate: self.off_lock_environment_probe_gate.clone(),
            #[cfg(test)]
            publication_gate: self.off_lock_repository_publication_gate.clone(),
        }
    }
}

impl RepositoryRefresh {
    /// Refresh immutable inputs after the repository work. Configuration can
    /// gain or change references while a queued Git job waits for its lock.
    /// Inline lifecycle drains use the admission snapshot because they already
    /// own AppState; shared drains briefly lock only to copy these inputs.
    pub(in crate::app) fn recapture(&self, app: &AppState) {
        if let RefreshState::Pending(refresh, roots) = &mut *self.state.lock().unwrap() {
            *refresh = app.capture_project_snapshot_refresh();
            *roots = app.capture_repository_roots();
        }
    }

    /// Called after Git work, while its repository locks are still held. Both
    /// common-directory discovery and source/availability probes run off-lock.
    pub(in crate::app) fn observe(&self, roots: &[PathBuf]) {
        let RefreshState::Pending(mut refresh, candidates) =
            std::mem::replace(&mut *self.state.lock().unwrap(), RefreshState::Taken)
        else {
            return;
        };
        #[cfg(test)]
        if let Some(gate) = &self.publication_gate {
            gate.arrive();
        }
        let affected = affected_repository_roots(candidates, roots);
        // Order observations when their read starts, rather than when a delayed
        // result settles. A prior read cannot overwrite a newer producer.
        refresh.version = self.clock.next_version();
        #[cfg(test)]
        if let Some(gate) = &self.gate {
            gate.arrive();
        }
        refresh.select_environment_edit("", roots);
        let observed = refresh.render();
        *self.state.lock().unwrap() = RefreshState::Observed(observed, affected);
    }

    /// Installation checks the current source inputs and the observation version
    /// before replacing any row; publication reads only these owned values.
    pub(in crate::app) fn install(&self, app: &AppState) {
        if let RefreshState::Observed(refresh, roots) =
            std::mem::replace(&mut *self.state.lock().unwrap(), RefreshState::Taken)
        {
            app.install_project_snapshot_refresh(refresh);
            app.invalidate_observed_repository_roots(&roots);
        }
    }
}

/// Resolve all common-directory aliases off-lock, keeping removed owners' roots
/// as fences for observations that were already in flight against those paths.
pub(in crate::app) fn affected_repository_roots(
    candidates: Vec<PathBuf>,
    changed: &[PathBuf],
) -> Vec<PathBuf> {
    if changed.is_empty() {
        return vec![];
    }
    let identities = changed
        .iter()
        .map(|p| crate::source_sync::sync_lock_identity(p))
        .collect::<BTreeSet<_>>();
    let mut roots = candidates
        .into_iter()
        .chain(changed.iter().cloned())
        .filter(|p| identities.contains(&crate::source_sync::sync_lock_identity(p)))
        .chain(changed.iter().cloned())
        .collect::<Vec<_>>();
    roots.sort();
    roots.dedup();
    roots
}

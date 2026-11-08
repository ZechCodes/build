//! Repository jobs carry environment evidence through their off-lock work.
use super::*;

/// Shared with settlement so even a partially failed filesystem mutation can
/// publish the evidence it collected. Neither phase retains the app mutex.
#[derive(Clone)]
pub(in crate::app) struct EnvironmentRepositoryRefresh {
    state: Arc<Mutex<RefreshState>>,
    clock: super::super::read_view::ReadViewSlot,
    #[cfg(test)]
    gate: Option<crate::app::OffLockGate>,
}

enum RefreshState {
    Pending(ProjectSnapshotRefresh),
    Observed(RenderedProjectSnapshotRefresh),
    Taken,
}

impl AppState {
    pub(in crate::app) fn capture_environment_repository_refresh(
        &self,
    ) -> EnvironmentRepositoryRefresh {
        EnvironmentRepositoryRefresh {
            state: Arc::new(Mutex::new(RefreshState::Pending(ProjectSnapshotRefresh {
                version: 0,
                rows: vec![],
                environments: self.capture_environment_snapshot_rows(),
            }))),
            clock: self.read_view.clone(),
            #[cfg(test)]
            gate: self.off_lock_environment_probe_gate.clone(),
        }
    }
}

impl EnvironmentRepositoryRefresh {
    /// Called after Git work, while its repository locks are still held. Both
    /// common-directory discovery and source/availability probes run off-lock.
    pub(in crate::app) fn observe(&self, roots: &[PathBuf]) {
        let RefreshState::Pending(mut refresh) =
            std::mem::replace(&mut *self.state.lock().unwrap(), RefreshState::Taken)
        else {
            return;
        };
        // Order observations when their read starts, rather than when a delayed
        // result settles. A prior read cannot overwrite a newer producer.
        refresh.version = self.clock.next_version();
        #[cfg(test)]
        if let Some(gate) = &self.gate {
            gate.arrive();
        }
        refresh.select_environment_edit("", roots);
        *self.state.lock().unwrap() = RefreshState::Observed(refresh.render());
    }

    /// Installation checks the current source inputs and the observation version
    /// before replacing any row; publication reads only these owned values.
    pub(in crate::app) fn install(&self, app: &AppState) {
        if let RefreshState::Observed(refresh) =
            std::mem::replace(&mut *self.state.lock().unwrap(), RefreshState::Taken)
        {
            app.install_project_snapshot_refresh(refresh);
        }
    }
}

use crate::app::AppState;
use crate::store::StorageMaintenanceHandle;

impl AppState {
    /// Capture the writer before sharing application state. The worker never
    /// receives the app mutex and all SQLite work runs on the blocking pool.
    pub fn spawn_storage_maintenance(&self) -> Option<StorageMaintenanceHandle> {
        self.store
            .as_ref()
            .map(|store| store.writer().spawn_storage_maintenance())
    }
}

/// Make the restart roster durable and stop children before maintenance can
/// wait on a busy writer. The worker's final join has its own short budget.
pub async fn shut_down_with_storage_maintenance(
    live: &crate::resume::LiveRoster,
    maintenance: Option<StorageMaintenanceHandle>,
    teardown: impl FnOnce(),
) {
    crate::resume::shut_down(live, teardown);
    if let Some(maintenance) = maintenance {
        maintenance.stop().await;
    }
}

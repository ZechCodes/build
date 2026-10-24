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

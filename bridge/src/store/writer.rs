//! The sole writable SQLite connection. Legacy Store calls and journal work
//! share this boundary; journal callers must enter it outside app/domain locks.
use super::StoreError;
use rusqlite::{Connection, Transaction, TransactionBehavior};
use std::sync::{Arc, Mutex, MutexGuard};

#[derive(Clone)]
pub struct StoreWriter {
    pub(super) connection: Arc<Mutex<Connection>>,
    pub(super) wakeup: Arc<tokio::sync::Notify>,
    #[cfg(test)]
    pub(super) fail_next_write: Arc<std::sync::atomic::AtomicBool>,
}

impl StoreWriter {
    pub(super) fn new(connection: Connection) -> Self {
        Self {
            connection: Arc::new(Mutex::new(connection)),
            wakeup: Arc::new(tokio::sync::Notify::new()),
            #[cfg(test)]
            fail_next_write: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    pub(super) fn connection(&self) -> MutexGuard<'_, Connection> {
        self.connection
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }

    pub(super) fn transaction<T>(
        &self,
        work: impl FnOnce(&Transaction<'_>) -> Result<T, StoreError>,
    ) -> Result<T, StoreError> {
        #[cfg(test)]
        let _phase = crate::timing::phase_samples::phase("sql.write");
        #[cfg(test)]
        if self
            .fail_next_write
            .swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            return Err(StoreError::Io(std::io::Error::other(
                "injected store failure",
            )));
        }
        let mut connection = self.connection();
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let result = work(&transaction)?;
        transaction.commit()?;
        Ok(result)
    }
}

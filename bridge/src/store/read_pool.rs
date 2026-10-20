//! Bounded independent SQLite readers. Checkout waits asynchronously; SQL runs
//! on the dedicated read workers, and every request gets one fresh snapshot.
use super::StoreError;
use rusqlite::{Connection, OpenFlags};
use std::path::Path;
use std::sync::{Arc, Mutex};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

pub(crate) const READ_CONNECTIONS: usize = 8;

#[derive(Clone)]
pub struct ReadPool {
    inner: Arc<Pool>,
}

struct Pool {
    available: Mutex<Vec<Connection>>,
    permits: Arc<Semaphore>,
}

/// One exclusively borrowed connection; dropping it returns its capacity.
pub struct ReadConnection {
    connection: Option<Connection>,
    pool: Arc<Pool>,
    permit: Option<OwnedSemaphorePermit>,
    #[cfg(test)]
    observer: Option<TransactionObserver>,
}

/// Typed query methods only. The underlying connection is never exposed to
/// callers outside the store, so mutations cannot accidentally use this lane.
pub struct ReadTransaction<'a> {
    connection: &'a Connection,
}

impl ReadPool {
    pub(super) fn open(path: &Path) -> Result<Self, StoreError> {
        let mut available = Vec::with_capacity(READ_CONNECTIONS);
        for _ in 0..READ_CONNECTIONS {
            let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
            connection.pragma_update(None, "query_only", true)?;
            connection.busy_timeout(std::time::Duration::from_secs(5))?;
            available.push(connection);
        }
        Ok(Self {
            inner: Arc::new(Pool {
                available: Mutex::new(available),
                permits: Arc::new(Semaphore::new(READ_CONNECTIONS)),
            }),
        })
    }

    /// Tokio's semaphore queues borrowers fairly and removes cancelled waits.
    /// This pool is never closed: each handle owns the connections it serves.
    pub async fn acquire(&self) -> ReadConnection {
        let permit = self
            .inner
            .permits
            .clone()
            .acquire_owned()
            .await
            .expect("read pool stays open");
        let connection = self
            .inner
            .available
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .pop()
            .expect("permit owns a read connection");
        ReadConnection {
            connection: Some(connection),
            pool: self.inner.clone(),
            permit: Some(permit),
            #[cfg(test)]
            observer: None,
        }
    }
}

#[cfg(test)]
type TransactionObserver =
    Box<dyn Fn(std::time::Duration, std::time::Duration, std::time::Duration) + Send>;

impl ReadConnection {
    /// Test-only transaction boundaries; absent from the daemon and its stats.
    #[cfg(test)]
    pub(crate) fn observe_transactions(
        &mut self,
        observer: impl Fn(std::time::Duration, std::time::Duration, std::time::Duration)
            + Send
            + 'static,
    ) {
        self.observer = Some(Box::new(observer));
    }

    /// One snapshot covers all rows the request needs. Results cannot borrow
    /// from the transaction, which ends before the connection becomes reusable.
    pub fn read<T>(
        &mut self,
        work: impl FnOnce(&ReadTransaction<'_>) -> T,
    ) -> Result<T, StoreError> {
        let connection = self
            .connection
            .as_mut()
            .expect("borrowed connection exists");
        #[cfg(test)]
        let started = std::time::Instant::now();
        let transaction = connection.transaction()?;
        #[cfg(test)]
        let began = std::time::Instant::now();
        let result = work(&ReadTransaction::new(&transaction));
        #[cfg(test)]
        let queried = std::time::Instant::now();
        transaction.commit()?;
        #[cfg(test)]
        if let Some(observer) = &self.observer {
            observer(began - started, queried - began, queried.elapsed());
        }
        Ok(result)
    }
}

impl Drop for ReadConnection {
    fn drop(&mut self) {
        if let Some(connection) = self.connection.take() {
            self.pool
                .available
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .push(connection);
        }
        // Return the connection before releasing its permit to the next waiter.
        self.permit.take();
    }
}

impl<'a> ReadTransaction<'a> {
    pub(super) fn new(connection: &'a Connection) -> Self {
        Self { connection }
    }

    pub(super) fn connection(&self) -> &Connection {
        self.connection
    }
}

#[cfg(test)]
mod metadata_timing;

#[cfg(test)]
mod tests;

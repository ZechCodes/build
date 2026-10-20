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
    // Fields drop in declaration order: close every idle reader before the
    // final writable connection, so SQLite can checkpoint and remove its WAL.
    // This is ownership only; read paths never acquire the writer mutex.
    _writer_lifetime: Arc<Mutex<Connection>>,
    #[cfg(test)]
    initialization: ReadPoolInitialization,
}

/// Natural startup work, nested inside Store::new rather than any request.
#[cfg(test)]
#[derive(Clone, Copy, Default)]
pub(crate) struct ReadPoolInitialization {
    pub total: std::time::Duration,
    pub schema_total: std::time::Duration,
    pub schema_count: usize,
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

fn open_read_connection(path: &Path) -> Result<Connection, StoreError> {
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    connection.pragma_update(None, "query_only", true)?;
    connection.busy_timeout(std::time::Duration::from_secs(5))?;
    Ok(connection)
}

impl ReadPool {
    pub(super) fn open(path: &Path, writer: Arc<Mutex<Connection>>) -> Result<Self, StoreError> {
        #[cfg(test)]
        let started = std::time::Instant::now();
        #[cfg(test)]
        let mut initialization = ReadPoolInitialization::default();
        let mut available = Vec::with_capacity(READ_CONNECTIONS);
        for _ in 0..READ_CONNECTIONS {
            let connection = open_read_connection(path)?;
            #[cfg(test)]
            let schema_started = std::time::Instant::now();
            // Store startup precedes request acceptance. Load every reader's
            // schema here so the ready pool never defers this work to a read.
            // Preparing needs no application table, rows, or retained statement.
            drop(connection.prepare("SELECT 1 FROM main.sqlite_schema LIMIT 0")?);
            #[cfg(test)]
            {
                initialization.schema_total += schema_started.elapsed();
                initialization.schema_count += 1;
            }
            available.push(connection);
        }
        let pool = Self::from_connections(available, writer);
        #[cfg(test)]
        let pool = {
            let mut pool = pool;
            initialization.total = started.elapsed();
            Arc::get_mut(&mut pool.inner)
                .expect("startup pool has not been published")
                .initialization = initialization;
            pool
        };
        Ok(pool)
    }

    fn from_connections(available: Vec<Connection>, writer: Arc<Mutex<Connection>>) -> Self {
        Self {
            inner: Arc::new(Pool {
                available: Mutex::new(available),
                permits: Arc::new(Semaphore::new(READ_CONNECTIONS)),
                _writer_lifetime: writer,
                #[cfg(test)]
                initialization: ReadPoolInitialization::default(),
            }),
        }
    }

    #[cfg(test)]
    pub(crate) fn initialization_timing(&self) -> ReadPoolInitialization {
        self.inner.initialization
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
mod schema_tests;

#[cfg(test)]
mod tests;

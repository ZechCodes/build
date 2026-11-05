//! The sole writable SQLite connection. Legacy Store calls and journal work
//! share this boundary; journal callers must enter it outside app/domain locks.
use super::StoreError;
use rusqlite::{Connection, Transaction, TransactionBehavior};
use std::sync::{Arc, Mutex, MutexGuard};

#[derive(Clone)]
pub struct StoreWriter {
    pub(super) connection: Arc<Mutex<Connection>>,
    pub(super) wakeup: Arc<tokio::sync::Notify>,
    secret_change_observer:
        Arc<Mutex<Option<Arc<dyn Fn(&crate::secrets::SecretRecord) + Send + Sync>>>>,
    #[cfg(test)]
    pub(super) fail_next_write: Arc<std::sync::atomic::AtomicBool>,
}

impl StoreWriter {
    pub(super) fn new(connection: Connection) -> Self {
        Self {
            connection: Arc::new(Mutex::new(connection)),
            wakeup: Arc::new(tokio::sync::Notify::new()),
            secret_change_observer: Arc::new(Mutex::new(None)),
            #[cfg(test)]
            fail_next_write: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    pub(super) fn connection(&self) -> MutexGuard<'_, Connection> {
        self.connection
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }

    pub(crate) fn set_secret_change_observer(
        &self,
        observer: Arc<dyn Fn(&crate::secrets::SecretRecord) + Send + Sync>,
    ) {
        *self
            .secret_change_observer
            .lock()
            .unwrap_or_else(|poison| poison.into_inner()) = Some(observer);
    }

    pub(super) fn notify_secret_records(&self, records: &[crate::secrets::SecretRecord]) {
        let observer = self
            .secret_change_observer
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .clone();
        let Some(observer) = observer else {
            return;
        };
        for record in records {
            if let Ok(record) = self.secret_metadata_resolve(record) {
                observer(&record);
            }
        }
    }

    pub(super) fn transaction<T>(
        &self,
        work: impl FnOnce(&Transaction<'_>) -> Result<T, StoreError>,
    ) -> Result<T, StoreError> {
        self.transaction_with_publications(|tx, _| work(tx))
    }

    pub(super) fn transaction_with_publications<T>(
        &self,
        work: impl FnOnce(&Transaction<'_>, &mut super::sync::PublicationBatch) -> Result<T, StoreError>,
    ) -> Result<T, StoreError> {
        self.transaction_with_durability(false, work)
    }

    pub(super) fn durable_transaction<T>(
        &self,
        work: impl FnOnce(&Transaction<'_>) -> Result<T, StoreError>,
    ) -> Result<T, StoreError> {
        self.transaction_with_durability(true, |tx, _| work(tx))
    }

    fn transaction_with_durability<T>(
        &self,
        full: bool,
        work: impl FnOnce(&Transaction<'_>, &mut super::sync::PublicationBatch) -> Result<T, StoreError>,
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
        let restore = if full {
            let previous: i64 =
                connection.pragma_query_value(None, "synchronous", |row| row.get(0))?;
            if previous == 2 {
                None
            } else {
                connection.pragma_update(None, "synchronous", "FULL")?;
                Some(previous)
            }
        } else {
            None
        };
        let result: Result<T, StoreError> = (|| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let mut batch = super::sync::PublicationBatch::default();
            let result = work(&transaction, &mut batch)?;
            super::sync::collect_tasks(&transaction, &mut batch)?;
            super::sync::collect_device(&transaction, &mut batch)?;
            batch.publish(&transaction, super::sync::now_ms())?;
            transaction.commit()?;
            Ok(result)
        })();
        let restored = restore.map_or(Ok(()), |previous| {
            connection.pragma_update(None, "synchronous", previous)
        });
        match result {
            Ok(value) => {
                if let Err(error) = restored {
                    eprintln!("store: committed durable write but could not restore synchronous pragma: {error}");
                }
                Ok(value)
            }
            Err(error) => {
                if let Err(restore_error) = restored {
                    eprintln!("store: failed write and could not restore synchronous pragma: {restore_error}");
                }
                Err(error)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::{c_char, c_int, c_void, CStr};
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct PragmaAuthorizer {
        calls: AtomicUsize,
        deny_at: usize,
    }

    unsafe extern "C" fn authorize_pragma(
        state: *mut c_void,
        action: c_int,
        name: *const c_char,
        _value: *const c_char,
        _database: *const c_char,
        _trigger: *const c_char,
    ) -> c_int {
        if action == rusqlite::ffi::SQLITE_PRAGMA && !name.is_null() {
            // SAFETY: SQLite supplies a valid NUL-terminated pragma name and
            // the test keeps the boxed state alive until it unregisters.
            let name = unsafe { CStr::from_ptr(name) }.to_bytes();
            if name.eq_ignore_ascii_case(b"synchronous") {
                let state = unsafe { &*state.cast::<PragmaAuthorizer>() };
                let call = state.calls.fetch_add(1, Ordering::SeqCst) + 1;
                if call >= state.deny_at {
                    return rusqlite::ffi::SQLITE_DENY;
                }
            }
        }
        rusqlite::ffi::SQLITE_OK
    }

    fn set_authorizer(writer: &StoreWriter, state: &mut PragmaAuthorizer) {
        let connection = writer.connection();
        // SAFETY: the connection is exclusively held and `state` remains at a
        // stable address until `clear_authorizer` runs under the same mutex.
        let code = unsafe {
            rusqlite::ffi::sqlite3_set_authorizer(
                connection.handle(),
                Some(authorize_pragma),
                (state as *mut PragmaAuthorizer).cast(),
            )
        };
        assert_eq!(code, rusqlite::ffi::SQLITE_OK);
    }

    fn clear_authorizer(writer: &StoreWriter) {
        let connection = writer.connection();
        // SAFETY: the mutex excludes users while the live callback is removed.
        let code = unsafe {
            rusqlite::ffi::sqlite3_set_authorizer(connection.handle(), None, std::ptr::null_mut())
        };
        assert_eq!(code, rusqlite::ffi::SQLITE_OK);
    }

    #[test]
    fn ordinary_writes_do_not_read_or_change_the_durability_pragma() {
        let dir = tempfile::tempdir().unwrap();
        let store = crate::store::Store::new(dir.path()).unwrap();
        let writer = store.writer();
        let mut authorizer = PragmaAuthorizer {
            calls: AtomicUsize::new(0),
            deny_at: 1,
        };
        set_authorizer(&writer, &mut authorizer);
        let result = writer.transaction(|tx| {
            tx.execute(
                "INSERT INTO meta(key,value) VALUES ('writer-test','ordinary')",
                [],
            )?;
            Ok(())
        });
        clear_authorizer(&writer);
        assert!(result.is_ok(), "ordinary writes must not touch synchronous");
        assert_eq!(authorizer.calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn committed_durable_write_succeeds_when_pragma_restore_fails() {
        let dir = tempfile::tempdir().unwrap();
        let store = crate::store::Store::new(dir.path()).unwrap();
        let writer = store.writer();
        let mut authorizer = PragmaAuthorizer {
            calls: AtomicUsize::new(0),
            deny_at: 3,
        };
        set_authorizer(&writer, &mut authorizer);
        let result = writer.durable_transaction(|tx| {
            tx.execute(
                "INSERT INTO meta(key,value) VALUES ('writer-test','committed')",
                [],
            )?;
            Ok(())
        });
        clear_authorizer(&writer);
        assert!(result.is_ok(), "a committed write must report success");
        let value: String = writer
            .connection()
            .query_row(
                "SELECT value FROM meta WHERE key='writer-test'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(value, "committed");
        assert_eq!(authorizer.calls.load(Ordering::SeqCst), 3);
    }
}

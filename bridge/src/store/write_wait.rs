//! Test-only conservative SQLite service accounting after writer acquisition.
use crate::timing::write_wait::{self, IntervalCategory, WaitGuard, WaitKind};
use rusqlite::Connection;
use std::ops::{Deref, DerefMut};
use std::sync::{Mutex, MutexGuard};

pub(super) struct ConnectionGuard<'a> {
    connection: MutexGuard<'a, Connection>,
    service: Option<WaitGuard>,
}

pub(super) fn acquire(connection: &Mutex<Connection>) -> ConnectionGuard<'_> {
    let context = write_wait::current_context();
    let waiting = context
        .as_ref()
        .map(|context| context.interval(IntervalCategory::KnownWait(WaitKind::StoreWriter)));
    let connection = connection
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    drop(waiting);
    // This upper bound includes all guard-held SQL/host work. It does not
    // pretend to identify SQLite's busy callback or change its timeout.
    let service = context.map(|context| context.interval(IntervalCategory::SqlService));
    ConnectionGuard {
        connection,
        service,
    }
}

impl Deref for ConnectionGuard<'_> {
    type Target = Connection;

    fn deref(&self) -> &Connection {
        &self.connection
    }
}

impl DerefMut for ConnectionGuard<'_> {
    fn deref_mut(&mut self) -> &mut Connection {
        &mut self.connection
    }
}

impl Drop for ConnectionGuard<'_> {
    fn drop(&mut self) {
        // Close the opaque service interval before the field unlocks SQLite.
        drop(self.service.take());
    }
}

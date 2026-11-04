//! Attribute actual SQLite changes, including WITHOUT ROWID device tables.
use crate::store::Store;
use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;

thread_local! {
    static DATABASE: Cell<*mut rusqlite::ffi::sqlite3> = const {Cell::new(std::ptr::null_mut())};
    static CURRENT: RefCell<String> = const {RefCell::new(String::new())};
    static PREVIOUS: Cell<u64> = const {Cell::new(0)};
    static ROWS: RefCell<BTreeMap<String,u64>> = const {RefCell::new(BTreeMap::new())};
}

fn target(sql: &str) -> String {
    let mut words = sql.split_whitespace();
    let operation = words.next().unwrap_or("");
    let table = match operation {
        "INSERT" => {
            words.find(|word| *word == "INTO");
            words.next()
        }
        "DELETE" => {
            words.find(|word| *word == "FROM");
            words.next()
        }
        "UPDATE" => words.next(),
        _ => None,
    };
    table
        .unwrap_or(operation)
        .split('(')
        .next()
        .unwrap_or("")
        .trim_start_matches("temp.")
        .to_owned()
}

fn observe(event: rusqlite::trace::TraceEvent<'_>) {
    DATABASE.with(|database| {
        // SAFETY: WriteCost holds the Store alive and unregisters the
        // callback before clearing the handle. This scalar getter neither
        // executes SQL nor re-enters the connection mutex.
        let total = unsafe { rusqlite::ffi::sqlite3_total_changes64(database.get()) } as u64;
        let changed = PREVIOUS.with(|previous| total - previous.replace(total));
        if changed > 0 {
            CURRENT.with(|current| {
                ROWS.with(|rows| {
                    *rows
                        .borrow_mut()
                        .entry(target(&current.borrow()))
                        .or_default() += changed;
                })
            });
        }
    });
    // RETURNING can finish after another statement starts. Attribute changes
    // to the last started statement, rather than its delayed PROFILE event.
    if let rusqlite::trace::TraceEvent::Stmt(statement, _) = event {
        CURRENT.with(|current| *current.borrow_mut() = statement.sql().into_owned());
    }
}

pub(super) struct WriteCost<'a>(&'a Store);
impl<'a> WriteCost<'a> {
    pub(super) fn start(store: &'a Store) -> Self {
        let connection = store.connection();
        // SAFETY: the Store owns this connection for the observer's lifetime.
        DATABASE.with(|database| database.set(unsafe { connection.handle() }));
        PREVIOUS.with(|previous| previous.set(connection.total_changes()));
        ROWS.with(|rows| rows.borrow_mut().clear());
        connection.trace_v2(
            rusqlite::trace::TraceEventCodes::SQLITE_TRACE_PROFILE
                | rusqlite::trace::TraceEventCodes::SQLITE_TRACE_STMT,
            Some(observe),
        );
        Self(store)
    }
    pub(super) fn rows(&self) -> BTreeMap<String, u64> {
        ROWS.with(|rows| rows.borrow().clone())
    }
}
impl Drop for WriteCost<'_> {
    fn drop(&mut self) {
        self.0
            .connection()
            .trace_v2(rusqlite::trace::TraceEventCodes::empty(), None);
        DATABASE.with(|database| database.set(std::ptr::null_mut()));
    }
}

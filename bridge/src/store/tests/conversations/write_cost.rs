//! Attribute actual SQLite changes, including WITHOUT ROWID device tables.
use crate::store::Store;
use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;

thread_local! {
    static DATABASE: Cell<*mut rusqlite::ffi::sqlite3> = const {Cell::new(std::ptr::null_mut())};
    static CURRENT: RefCell<String> = const {RefCell::new(String::new())};
    static PREVIOUS: Cell<u64> = const {Cell::new(0)};
    static ROWS: RefCell<BTreeMap<String,u64>> = const {RefCell::new(BTreeMap::new())};
    static STATEMENTS: RefCell<BTreeMap<String,u64>> = const {RefCell::new(BTreeMap::new())};
}

/// Actual statement starts, including repeated executions of cached statements.
/// Keys retain unexpanded SQL: fixture values and retained history do not alter
/// the keys, while different SQL and repeated executions remain distinguishable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ExecutionCost {
    pub total: u64,
    /// Statements whose first SQL token is SELECT (not preparation count).
    pub selects: u64,
    pub statements: BTreeMap<String, u64>,
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
        let sql = statement.sql().into_owned();
        STATEMENTS.with(|statements| {
            *statements.borrow_mut().entry(sql.clone()).or_default() += 1;
        });
        CURRENT.with(|current| *current.borrow_mut() = sql);
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
        STATEMENTS.with(|statements| statements.borrow_mut().clear());
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
    pub(super) fn executions(&self) -> ExecutionCost {
        let statements = STATEMENTS.with(|statements| statements.borrow().clone());
        let total = statements.values().sum();
        let selects = statements
            .iter()
            .filter(|(sql, _)| {
                sql.split_whitespace()
                    .next()
                    .is_some_and(|word| word.eq_ignore_ascii_case("SELECT"))
            })
            .map(|(_, count)| count)
            .sum();
        ExecutionCost {
            total,
            selects,
            statements,
        }
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

#[test]
fn execution_cost_counts_each_cached_statement_execution_instead_of_preparations() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().join("tasks")).unwrap();
    let cost = WriteCost::start(&store);
    {
        let connection = store.connection();
        let mut statement = connection.prepare_cached("SELECT ?1").unwrap();
        for value in [1, 2, 3] {
            assert_eq!(
                statement
                    .query_row([value], |row| row.get::<_, i64>(0))
                    .unwrap(),
                value
            );
        }
    }
    assert_eq!(
        cost.executions(),
        ExecutionCost {
            total: 3,
            selects: 3,
            statements: BTreeMap::from([("SELECT ?1".to_owned(), 3)]),
        }
    );
    assert!(cost.rows().is_empty());
}

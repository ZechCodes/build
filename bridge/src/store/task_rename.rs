//! Schema 10 (#190): what Build called an issue is a task.
//!
//! Everything a v9 store holds under the old word moves to the new one, in one
//! transaction, before the schema batch runs: the batch would otherwise create
//! the new tables empty beside the old full ones.
//!
//! - The tables `issues` and `tracker_issues` become `tasks` and
//!   `tracker_tasks`, and every `issue_id` column becomes `task_id`. The
//!   indexes named for the old word are dropped; the schema batch builds them
//!   again under the new names.
//! - Ids: `issue-…` becomes `task-…`, a comment's `ic-…` becomes `tc-…` and an
//!   event's `ie-…` becomes `te-…`, in every id column and wherever a record
//!   holds one as a whole value or as a key.
//! - Record keys: every snake_case key with the word in it (`issue_id`,
//!   `parent_issue_id`, `from_issue`, `issue_notice`, `issues`, …) is spelled
//!   with the new one.
//! - The few values that are Build's own vocabulary rather than somebody's
//!   words — a `kind`, a `phase`, a `holds` entry — are renamed the same way.
//!   Nothing else a person or an agent wrote is touched: a label called
//!   "issues" stays "issues", and a message that quotes an old id keeps
//!   quoting it (the lookups read an old id as its new one).
//!
//! The whole database is copied beside itself first, once. The migration is
//! idempotent on its own as well as by the schema version: a table already
//! renamed is not renamed again, and a record already spelled the new way has
//! nothing left to rewrite.
//!
//! The plan documents of the retired plan flow stay where they are on disk
//! (`issues/<plan>/docs`): that directory is also the JSON tree the store was
//! imported from, which the store refuses to open over once it has changed.

use super::StoreError;
use rusqlite::{Connection, OptionalExtension};
use serde_json::Value;
use std::path::Path;

/// The schema version that renamed issues to tasks; `SCHEMA_VERSION` from it on.
pub(super) const TASK_RENAME_VERSION: i64 = 10;

/// The copy taken before anything is renamed, beside `build.db`.
pub(super) const BACKUP_FILE: &str = "build.db.before-tasks";

const RENAMED_TABLES: [(&str, &str); 2] =
    [("issues", "tasks"), ("tracker_issues", "tracker_tasks")];

const RENAMED_COLUMNS: [(&str, &str, &str); 3] = [
    ("implementations", "issue_id", "task_id"),
    ("tracker_comments", "issue_id", "task_id"),
    ("tracker_events", "issue_id", "task_id"),
];

const RETIRED_INDEXES: [&str; 8] = [
    "implementations_by_issue",
    "tracker_issues_number",
    "tracker_issues_by_project",
    "tracker_issues_by_state",
    "tracker_issues_by_status",
    "tracker_issues_by_state_status",
    "tracker_comments_by_issue",
    "tracker_events_by_issue",
];

/// How one column is rewritten.
#[derive(Clone, Copy)]
enum Column {
    /// Holds an id, or text that is not one.
    Id,
    /// Holds a JSON document, or text that is not one.
    Json,
}

/// Every column that can hold the old word, by table (named as the migration
/// leaves it). A table an older store never had is skipped.
const COLUMNS: &[(&str, &[(&str, Column)])] = &[
    ("meta", &[("value", Column::Json)]),
    ("tasks", &[("id", Column::Id), ("record", Column::Json)]),
    (
        "implementations",
        &[("task_id", Column::Id), ("record", Column::Json)],
    ),
    (
        "agents",
        &[("owner_id", Column::Id), ("record", Column::Json)],
    ),
    ("thread_items", &[("item", Column::Json)]),
    ("captures", &[("record", Column::Json)]),
    (
        "attention",
        &[("entity_id", Column::Id), ("record", Column::Json)],
    ),
    ("archived_worktrees", &[("record", Column::Json)]),
    (
        "operations",
        &[
            ("entity_id", Column::Id),
            ("conversation_id", Column::Id),
            ("delivery", Column::Json),
        ],
    ),
    ("agent_migration_backups", &[("record", Column::Json)]),
    (
        "tracker_tasks",
        &[("id", Column::Id), ("record", Column::Json)],
    ),
    (
        "tracker_comments",
        &[
            ("id", Column::Id),
            ("task_id", Column::Id),
            ("record", Column::Json),
        ],
    ),
    (
        "tracker_events",
        &[
            ("id", Column::Id),
            ("task_id", Column::Id),
            ("record", Column::Json),
        ],
    ),
];

/// The keys whose string values are Build's own vocabulary, not anybody's
/// words: only these values are renamed.
const VOCABULARY_KEYS: [&str; 6] = ["kind", "phase", "holds", "target", "type", "reason"];

const RENAMED_ID_PREFIXES: [(&str, &str); 3] =
    [("issue-", "task-"), ("ic-", "tc-"), ("ie-", "te-")];

/// The length of a ULID, which is what follows a minted id's prefix.
const ULID_LEN: usize = 26;

/// What one run of the migration changed, for the log and for the test that
/// reads it.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct TaskRenameReport {
    pub tables_renamed: usize,
    pub columns_renamed: usize,
    pub indexes_dropped: usize,
    pub rows_rewritten: usize,
    pub backup_taken: bool,
}

/// Take the backup, then rename everything in one transaction.
pub(super) fn migrate_to_task_names(
    conn: &mut Connection,
    dir: &Path,
) -> Result<TaskRenameReport, StoreError> {
    let mut report = TaskRenameReport {
        backup_taken: back_up(conn, &dir.join(BACKUP_FILE))?,
        ..TaskRenameReport::default()
    };
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    for index in RETIRED_INDEXES {
        if exists(&tx, "index", index)? {
            tx.execute_batch(&format!("DROP INDEX {index}"))?;
            report.indexes_dropped += 1;
        }
    }
    for (old, new) in RENAMED_TABLES {
        if exists(&tx, "table", old)? && !exists(&tx, "table", new)? {
            tx.execute_batch(&format!("ALTER TABLE {old} RENAME TO {new}"))?;
            report.tables_renamed += 1;
        }
    }
    for (table, old, new) in RENAMED_COLUMNS {
        if has_column(&tx, table, old)? {
            tx.execute_batch(&format!("ALTER TABLE {table} RENAME COLUMN {old} TO {new}"))?;
            report.columns_renamed += 1;
        }
    }
    for (table, columns) in COLUMNS {
        if exists(&tx, "table", table)? {
            report.rows_rewritten += rewrite_table(&tx, table, columns)?;
        }
    }
    tx.commit()?;
    Ok(report)
}

/// A whole copy of the database, taken once: a second attempt after a failed
/// first finds the first's copy, which is of the same unmigrated store because
/// the migration is one transaction.
///
/// Written beside its name and renamed into it, so the name only ever holds a
/// whole copy: a boot killed mid-`VACUUM` leaves the `.tmp`, which the next
/// one throws away and writes again.
fn back_up(conn: &Connection, path: &Path) -> Result<bool, StoreError> {
    if path.exists() {
        return Ok(false);
    }
    let mut partial = path.as_os_str().to_owned();
    partial.push(".tmp");
    let partial = std::path::PathBuf::from(partial);
    if partial.exists() {
        std::fs::remove_file(&partial)?;
    }
    conn.execute("VACUUM INTO ?1", [partial.to_string_lossy()])?;
    std::fs::rename(&partial, path)?;
    Ok(true)
}

fn exists(conn: &Connection, kind: &str, name: &str) -> Result<bool, StoreError> {
    Ok(conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type = ?1 AND name = ?2",
            [kind, name],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

fn has_column(conn: &Connection, table: &str, column: &str) -> Result<bool, StoreError> {
    if !exists(conn, "table", table)? {
        return Ok(false);
    }
    let mut statement = conn.prepare(&format!("SELECT name FROM pragma_table_info('{table}')"))?;
    let names = statement.query_map([], |row| row.get::<_, String>(0))?;
    for name in names {
        if name? == column {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Rewrite every row of `table` whose listed columns hold the old word, and
/// answer how many rows changed.
fn rewrite_table(
    tx: &rusqlite::Transaction,
    table: &str,
    columns: &[(&str, Column)],
) -> Result<usize, StoreError> {
    let names: Vec<&str> = columns.iter().map(|(name, _)| *name).collect();
    let rows: Vec<(i64, Vec<Option<String>>)> = {
        let mut statement =
            tx.prepare(&format!("SELECT rowid, {} FROM {table}", names.join(", ")))?;
        let rows = statement
            .query_map([], |row| {
                let values = (1..=names.len())
                    .map(|index| row.get::<_, Option<String>>(index))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok((row.get(0)?, values))
            })?
            .collect::<Result<_, _>>()?;
        rows
    };
    let assignments: Vec<String> = names
        .iter()
        .enumerate()
        .map(|(index, name)| format!("{name} = ?{}", index + 2))
        .collect();
    let update = format!(
        "UPDATE {table} SET {} WHERE rowid = ?1",
        assignments.join(", ")
    );
    let mut rewritten = 0;
    for (rowid, values) in rows {
        let renamed: Vec<Option<String>> = values
            .iter()
            .zip(columns)
            .map(|(value, (_, column))| value.as_deref().map(|text| renamed_column(text, *column)))
            .collect();
        if renamed == values {
            continue;
        }
        let mut parameters: Vec<&dyn rusqlite::ToSql> = vec![&rowid];
        parameters.extend(renamed.iter().map(|value| value as &dyn rusqlite::ToSql));
        tx.execute(&update, parameters.as_slice())?;
        rewritten += 1;
    }
    Ok(rewritten)
}

fn renamed_column(text: &str, column: Column) -> String {
    match column {
        Column::Id => renamed_id(text).unwrap_or_else(|| text.to_string()),
        Column::Json => renamed_json(text).unwrap_or_else(|| text.to_string()),
    }
}

/// A JSON document with the old word renamed, or `None` when it is not JSON or
/// holds nothing to rename.
fn renamed_json(text: &str) -> Option<String> {
    if !text.contains("issue") && !text.contains("\"ic-") && !text.contains("\"ie-") {
        return None;
    }
    let mut value: Value = serde_json::from_str(text).ok()?;
    rename_value(&mut value, None).then(|| value.to_string())
}

/// Rename in place, answering whether anything changed. `key` is the key the
/// value sits under (an array's elements sit under the array's key).
fn rename_value(value: &mut Value, key: Option<&str>) -> bool {
    match value {
        Value::Object(map) => {
            let mut changed = false;
            for (old_key, mut inner) in std::mem::take(map) {
                let renamed = renamed_key(&old_key);
                changed |= renamed.is_some();
                let new_key = renamed.unwrap_or(old_key);
                changed |= rename_value(&mut inner, Some(&new_key));
                map.insert(new_key, inner);
            }
            changed
        }
        Value::Array(items) => items
            .iter_mut()
            .fold(false, |changed, item| rename_value(item, key) | changed),
        Value::String(text) => match renamed_string(text, key) {
            Some(renamed) => {
                *text = renamed;
                true
            }
            None => false,
        },
        _ => false,
    }
}

fn renamed_key(key: &str) -> Option<String> {
    renamed_id(key).or_else(|| renamed_token(key))
}

fn renamed_string(text: &str, key: Option<&str>) -> Option<String> {
    renamed_id(text).or_else(|| {
        key.filter(|key| VOCABULARY_KEYS.contains(key))
            .and_then(|_| renamed_token(text))
    })
}

/// An id minted under an old prefix, under its new one.
pub fn renamed_id(text: &str) -> Option<String> {
    RENAMED_ID_PREFIXES.iter().find_map(|(old, new)| {
        let rest = text.strip_prefix(old)?;
        let minted = rest.len() == ULID_LEN
            && rest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || byte.is_ascii_uppercase());
        minted.then(|| format!("{new}{rest}"))
    })
}

/// A snake_case token with `issue` or `issues` as one of its words, with that
/// word renamed. Anything else — prose, a camelCase name, `issued` — is not.
fn renamed_token(text: &str) -> Option<String> {
    let snake = !text.is_empty()
        && text
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_');
    if !snake {
        return None;
    }
    let mut changed = false;
    let words: Vec<&str> = text
        .split('_')
        .map(|word| match word {
            "issue" => {
                changed = true;
                "task"
            }
            "issues" => {
                changed = true;
                "tasks"
            }
            other => other,
        })
        .collect();
    changed.then(|| words.join("_"))
}

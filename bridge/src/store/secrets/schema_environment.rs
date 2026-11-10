//! Schema 27 preserves ciphertext AAD and every old metadata column verbatim.
use super::SCHEMA;
use rusqlite::{Connection, OptionalExtension};

fn columns(connection: &Connection, table: &str) -> rusqlite::Result<Vec<String>> {
    connection
        .prepare(&format!("PRAGMA table_info({table})"))?
        .query_map([], |r| r.get(1))?
        .collect()
}

pub(super) fn upgrade(connection: &Connection) -> rusqlite::Result<()> {
    let old = columns(connection, "secret_records")?;
    if old.is_empty() {
        return Ok(());
    }
    // SQLite's documented create/copy/drop/rename rebuild requires FK checks
    // disabled outside the transaction; restore the original setting on error.
    let foreign_keys: bool = connection.pragma_query_value(None, "foreign_keys", |r| r.get(0))?;
    connection.pragma_update(None, "foreign_keys", false)?;
    let result = upgrade_in(connection, &old);
    let restored = connection.pragma_update(None, "foreign_keys", foreign_keys);
    result.and(restored)
}

fn upgrade_in(connection: &Connection, old: &[String]) -> rusqlite::Result<()> {
    let tx = connection.unchecked_transaction()?;
    if !old.iter().any(|column| column == "grant_environment_id") {
        let ddl = SCHEMA
            .split(';')
            .map(str::trim)
            .find(|s| s.starts_with("CREATE TABLE IF NOT EXISTS secret_records ("))
            .ok_or(rusqlite::Error::InvalidQuery)?;
        tx.execute_batch(&ddl.replacen(
            "CREATE TABLE IF NOT EXISTS secret_records (",
            "CREATE TABLE secret_records_next (",
            1,
        ))?;
        let names = old.join(",");
        tx.execute_batch(&format!("INSERT INTO secret_records_next ({names}) SELECT {names} FROM secret_records; DROP TABLE secret_records; ALTER TABLE secret_records_next RENAME TO secret_records;"))?;
    }
    for table in ["secret_final_bindings", "secret_audit"] {
        let names = columns(&tx, table)?;
        if !names.is_empty() && !names.iter().any(|c| c == "environment_id") {
            tx.execute_batch(&format!(
                "ALTER TABLE {table} ADD COLUMN environment_id TEXT"
            ))?;
        }
    }
    let violation:Option<String>=tx.query_row("SELECT \"table\" FROM pragma_foreign_key_check WHERE \"table\"='secret_final_bindings' LIMIT 1",[],|r|r.get(0)).optional()?;
    if violation.is_some() {
        return Err(rusqlite::Error::InvalidQuery);
    }
    tx.commit()
}

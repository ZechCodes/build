use rusqlite::OptionalExtension;

pub(super) const DENIAL_OUTCOMES: &[&str] =
    &["secret_unavailable", "secret_expired", "secret_revoked"];

pub(super) fn denial_outcomes_sql() -> String {
    format!(
        "({})",
        DENIAL_OUTCOMES
            .iter()
            .map(|outcome| format!("'{outcome}'"))
            .collect::<Vec<_>>()
            .join(",")
    )
}

const AUDIT_COLUMNS: &str = "sequence,secret_id,creator,project_id,owner_id,agent_id,conversation_id,generation,session_incarnation,request_id,occurred_at,attempt_count,last_occurred_at,outcome";

/// Durable audit outcomes shared by release and metadata lifecycle writers.
pub(crate) const AUDIT_OUTCOMES: &[&str] = &[
    "released",
    "secret_unavailable",
    "secret_expired",
    "secret_revoked",
    "created",
    "attached",
    "expired",
    "revoked",
];

const REVOCATION_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS secret_revocations (
    sequence INTEGER PRIMARY KEY AUTOINCREMENT,
    device_id TEXT NOT NULL,
    creator TEXT NOT NULL,
    operation_id TEXT NOT NULL,
    secret_id TEXT NOT NULL,
    accepted_at INTEGER NOT NULL CHECK (accepted_at >= 0),
    request TEXT NOT NULL,
    response TEXT NOT NULL,
    UNIQUE(device_id,creator,operation_id)
);
CREATE INDEX IF NOT EXISTS secret_revocations_retention
    ON secret_revocations(accepted_at,sequence);
"#;

pub(in crate::store) static SCHEMA: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
    SCHEMA_TEMPLATE.replace("{DENIAL_OUTCOMES}", &denial_outcomes_sql())
});

const SCHEMA_TEMPLATE: &str = r#"
CREATE TABLE IF NOT EXISTS secret_records (
    id TEXT PRIMARY KEY,
    device_id TEXT NOT NULL,
    creator TEXT NOT NULL,
    name TEXT NOT NULL,
    ttl_seconds INTEGER CHECK (ttl_seconds IS NULL OR ttl_seconds > 0),
    grant_scope TEXT NOT NULL CHECK (grant_scope IN ('agent','project')),
    grant_agent_id TEXT,
    grant_conversation_id TEXT,
    grant_generation TEXT,
    grant_project_id TEXT,
    stage_owner_id TEXT NOT NULL,
    stage_conversation_id TEXT NOT NULL,
    stage_generation TEXT NOT NULL,
    stage_draft_id TEXT,
    created_at INTEGER NOT NULL CHECK (created_at >= 0),
    attached_at INTEGER,
    expires_at INTEGER,
    revoked_at INTEGER,
    revocation_reason TEXT CHECK (revocation_reason IS NULL OR revocation_reason IN ('key_lost','revoked','expired')),
    revision INTEGER NOT NULL CHECK (revision >= 1),
    message_operation_id TEXT,
    upload_request_id TEXT NOT NULL,
    upload_digest TEXT NOT NULL,
    blob_present INTEGER NOT NULL CHECK (blob_present IN (0,1)),
    UNIQUE (device_id, creator, upload_request_id),
    CHECK ((grant_scope='agent' AND grant_agent_id IS NOT NULL AND grant_conversation_id IS NOT NULL AND grant_generation IS NOT NULL AND grant_project_id IS NULL)
        OR (grant_scope='project' AND grant_agent_id IS NULL AND grant_conversation_id IS NULL AND grant_generation IS NULL AND grant_project_id IS NOT NULL)),
    CHECK ((attached_at IS NULL AND expires_at IS NULL AND message_operation_id IS NULL)
        OR (attached_at IS NOT NULL AND message_operation_id IS NOT NULL))
);
CREATE INDEX IF NOT EXISTS secrets_staged_by_creator
    ON secret_records(device_id, creator) WHERE blob_present=1 AND attached_at IS NULL AND revoked_at IS NULL;
CREATE INDEX IF NOT EXISTS secrets_retained_by_device
    ON secret_records(device_id) WHERE blob_present=1;
CREATE INDEX IF NOT EXISTS secrets_agent_generation
    ON secret_records(grant_conversation_id, grant_generation) WHERE grant_scope='agent' AND revoked_at IS NULL;
CREATE INDEX IF NOT EXISTS secrets_agent
    ON secret_records(grant_agent_id) WHERE grant_scope='agent' AND revoked_at IS NULL;
CREATE INDEX IF NOT EXISTS secrets_project
    ON secret_records(grant_project_id) WHERE grant_scope='project' AND revoked_at IS NULL;
CREATE INDEX IF NOT EXISTS secrets_unretired
    ON secret_records(id) WHERE revoked_at IS NULL;
CREATE TABLE IF NOT EXISTS secret_vault_state (
    singleton INTEGER PRIMARY KEY CHECK (singleton=1),
    active_key_id TEXT,
    pending_key_id TEXT CHECK (pending_key_id IS NULL OR active_key_id IS NULL)
);
CREATE TABLE IF NOT EXISTS secret_clock (
    singleton INTEGER PRIMARY KEY CHECK (singleton=1),
    high_water INTEGER NOT NULL CHECK (high_water >= 0),
    revision INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS secret_bindings (
    operation_id TEXT PRIMARY KEY,
    secret_ids TEXT NOT NULL,
    attached_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS secret_final_bindings (
    secret_id TEXT PRIMARY KEY REFERENCES secret_records(id),
    operation_id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    owner_id TEXT NOT NULL,
    agent_id TEXT NOT NULL,
    conversation_id TEXT NOT NULL,
    generation TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS secret_final_agent ON secret_final_bindings(agent_id,conversation_id);
CREATE INDEX IF NOT EXISTS secrets_metadata_project
    ON secret_records(device_id,creator,grant_project_id,id) WHERE grant_scope='project';
CREATE INDEX IF NOT EXISTS secrets_metadata_stage
    ON secret_records(device_id,creator,stage_owner_id,stage_conversation_id,stage_generation,stage_draft_id,id);
CREATE INDEX IF NOT EXISTS secrets_metadata_final
    ON secret_final_bindings(project_id,owner_id,conversation_id,generation,secret_id);
CREATE TABLE IF NOT EXISTS secret_audit (
    sequence INTEGER PRIMARY KEY AUTOINCREMENT,
    secret_id TEXT NOT NULL,
    creator TEXT,
    project_id TEXT NOT NULL,
    owner_id TEXT NOT NULL,
    agent_id TEXT NOT NULL,
    conversation_id TEXT NOT NULL,
    generation TEXT NOT NULL,
    session_incarnation TEXT NOT NULL,
    request_id TEXT NOT NULL,
    occurred_at INTEGER NOT NULL,
    attempt_count INTEGER NOT NULL DEFAULT 1 CHECK (attempt_count > 0),
    last_occurred_at INTEGER NOT NULL DEFAULT 0,
    outcome TEXT NOT NULL CHECK (outcome IN ('released','secret_unavailable','secret_expired','secret_revoked','created','attached','expired','revoked'))
);
CREATE INDEX IF NOT EXISTS secret_audit_by_secret ON secret_audit(secret_id,sequence);
CREATE INDEX IF NOT EXISTS secret_audit_by_caller ON secret_audit(agent_id,sequence);
CREATE INDEX IF NOT EXISTS secret_audit_denial_order ON secret_audit(sequence) WHERE outcome IN {DENIAL_OUTCOMES};
CREATE INDEX IF NOT EXISTS secret_audit_denial_group ON secret_audit(agent_id,secret_id,outcome,occurred_at) WHERE outcome IN {DENIAL_OUTCOMES};
CREATE INDEX IF NOT EXISTS secret_audit_retention ON secret_audit(occurred_at,sequence);
CREATE TABLE IF NOT EXISTS secret_audit_summaries (
    secret_id TEXT PRIMARY KEY,
    release_count INTEGER NOT NULL,
    denial_count INTEGER NOT NULL,
    last_released_at INTEGER,
    last_outcome TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS secret_unauthenticated_summary (
    singleton INTEGER PRIMARY KEY CHECK (singleton=1),
    attempt_count INTEGER NOT NULL,
    first_at INTEGER NOT NULL,
    last_at INTEGER NOT NULL
);

"#;

/// Schema 24 was unadvertised before key-loss recovery was revised. Upgrade
/// those same-version databases too; an absent identity is never adopted while
/// unretired grants remain.
pub(in crate::store) fn initialize(connection: &rusqlite::Connection) -> rusqlite::Result<()> {
    connection.execute_batch(&SCHEMA)?;
    let columns = connection
        .prepare("PRAGMA table_info(secret_records)")?
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if !columns.iter().any(|column| column == "revocation_reason") {
        connection.execute_batch("ALTER TABLE secret_records ADD COLUMN revocation_reason TEXT CHECK (revocation_reason IS NULL OR revocation_reason IN ('key_lost','revoked','expired'))")?;
    }
    ensure_pending_key_column(connection)?;
    ensure_audit_upgrades(connection)?;
    ensure_lifecycle_retention(connection)?;
    Ok(())
}

fn ensure_lifecycle_retention(connection: &rusqlite::Connection) -> rusqlite::Result<()> {
    connection.execute_batch(
        "CREATE TRIGGER IF NOT EXISTS secret_records_prune_lifecycle_audit
         AFTER DELETE ON secret_records
         BEGIN
             DELETE FROM secret_audit
             WHERE secret_id=OLD.id
             AND outcome IN ('created','attached','expired','revoked');
         END;",
    )
}

fn ensure_pending_key_column(connection: &rusqlite::Connection) -> rusqlite::Result<()> {
    let columns = connection
        .prepare("PRAGMA table_info(secret_vault_state)")?
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if !columns.iter().any(|column| column == "pending_key_id") {
        connection.execute_batch("ALTER TABLE secret_vault_state ADD COLUMN pending_key_id TEXT CHECK (pending_key_id IS NULL OR active_key_id IS NULL)")?;
    }
    Ok(())
}

/// Receipts and the audit CHECK extension are one atomic schema-25 upgrade.
fn ensure_audit_upgrades(connection: &rusqlite::Connection) -> rusqlite::Result<()> {
    connection.execute_batch("SAVEPOINT secret_audit_upgrade")?;
    let result = (|| {
        ensure_audit_group_columns(connection)?;
        ensure_lifecycle_audit(connection)?;
        ensure_audit_indexes(connection)?;
        ensure_revocation_receipts(connection)?;
        connection.execute_batch("RELEASE secret_audit_upgrade")
    })();
    if result.is_err() {
        let _ = connection
            .execute_batch("ROLLBACK TO secret_audit_upgrade; RELEASE secret_audit_upgrade");
    }
    result
}

fn audit_column_names(connection: &rusqlite::Connection) -> rusqlite::Result<Vec<String>> {
    connection
        .prepare("PRAGMA table_info(secret_audit)")?
        .query_map([], |row| row.get(1))?
        .collect()
}

fn ensure_audit_group_columns(connection: &rusqlite::Connection) -> rusqlite::Result<()> {
    let columns = audit_column_names(connection)?;
    if !columns.iter().any(|column| column == "attempt_count") {
        connection.execute_batch("ALTER TABLE secret_audit ADD COLUMN attempt_count INTEGER NOT NULL DEFAULT 1 CHECK(attempt_count > 0)")?;
    }
    if !columns.iter().any(|column| column == "last_occurred_at") {
        connection.execute_batch("ALTER TABLE secret_audit ADD COLUMN last_occurred_at INTEGER NOT NULL DEFAULT 0; UPDATE secret_audit SET last_occurred_at=occurred_at;")?;
    }
    // The cap applies only to authenticated denial groups. Lifecycle rows and
    // release detail survive this same-version upgrade and index replacement.
    connection.execute(&format!("DELETE FROM secret_audit WHERE sequence IN (SELECT sequence FROM secret_audit WHERE outcome IN {} ORDER BY sequence DESC LIMIT -1 OFFSET 4096)", denial_outcomes_sql()), [])?;
    Ok(())
}

fn canonical_audit_indexes() -> impl Iterator<Item = &'static str> {
    SCHEMA
        .split(';')
        .map(str::trim)
        .filter(|statement| statement.starts_with("CREATE INDEX IF NOT EXISTS secret_audit_"))
}

fn normalized_index(sql: &str) -> String {
    sql.replace("IF NOT EXISTS ", "")
        .split_whitespace()
        .collect()
}

fn ensure_audit_indexes(connection: &rusqlite::Connection) -> rusqlite::Result<()> {
    for statement in canonical_audit_indexes() {
        let name = statement
            .split_whitespace()
            .nth(5)
            .ok_or(rusqlite::Error::InvalidQuery)?;
        let existing: Option<String> = connection
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type='index' AND name=?1",
                [name],
                |row| row.get(0),
            )
            .optional()?;
        let expected = normalized_index(statement);
        if existing.as_deref().map(normalized_index).as_deref() != Some(expected.as_str()) {
            connection.execute_batch(&format!("DROP INDEX IF EXISTS {name}; {statement}"))?;
        }
    }
    Ok(())
}

fn ensure_revocation_receipts(connection: &rusqlite::Connection) -> rusqlite::Result<()> {
    connection.execute_batch(REVOCATION_SCHEMA)
}

/// Preserve sequence numbers, rows and the AUTOINCREMENT high-water while
/// extending the CHECK from the pre-wire schema. Savepoint composes with an
/// existing migration transaction and makes every crash observe one table.
fn ensure_lifecycle_audit(connection: &rusqlite::Connection) -> rusqlite::Result<()> {
    let sql: String = connection.query_row(
        "SELECT sql FROM sqlite_master WHERE type='table' AND name='secret_audit'",
        [],
        |row| row.get(0),
    )?;
    if AUDIT_OUTCOMES
        .iter()
        .all(|outcome| sql.contains(&format!("'{outcome}'")))
        && audit_column_names(connection)?
            .iter()
            .map(String::as_str)
            .eq(AUDIT_COLUMNS.split(','))
    {
        return Ok(());
    }
    let revised = upgraded_audit_table()?;
    connection.execute_batch(&revised)?;
    connection.execute_batch(&format!(
        "INSERT INTO secret_audit_next ({AUDIT_COLUMNS}) SELECT {AUDIT_COLUMNS} FROM secret_audit"
    ))?;
    connection.execute_batch("INSERT INTO sqlite_sequence(name,seq) SELECT 'secret_audit_next',seq FROM sqlite_sequence WHERE name='secret_audit' AND NOT EXISTS(SELECT 1 FROM sqlite_sequence WHERE name='secret_audit_next'); UPDATE sqlite_sequence SET seq=MAX(seq,COALESCE((SELECT seq FROM sqlite_sequence WHERE name='secret_audit'),0)) WHERE name='secret_audit_next'; DROP TABLE secret_audit; ALTER TABLE secret_audit_next RENAME TO secret_audit;")?;
    Ok(())
}

fn upgraded_audit_table() -> rusqlite::Result<String> {
    let statement = SCHEMA
        .split(';')
        .map(str::trim)
        .find(|statement| statement.starts_with("CREATE TABLE IF NOT EXISTS secret_audit ("))
        .ok_or(rusqlite::Error::InvalidQuery)?;
    Ok(statement.replacen(
        "CREATE TABLE IF NOT EXISTS secret_audit (",
        "CREATE TABLE secret_audit_next (",
        1,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lifecycle_audit_upgrades_d_fresh_order_without_losing_group_values_or_indexes() {
        assert_d_upgrade(false, false);
    }

    #[test]
    fn lifecycle_audit_upgrades_d_alter_order_without_losing_group_values_or_indexes() {
        assert_d_upgrade(true, false);
    }

    #[test]
    fn lifecycle_audit_adds_group_columns_before_rebuilding_the_outcome_check() {
        assert_d_upgrade(false, true);
    }

    #[test]
    fn full_lifecycle_check_still_repairs_denial_indexes_and_preserves_metadata_during_cleanup() {
        let connection = rusqlite::Connection::open_in_memory().unwrap();
        connection.execute_batch(&SCHEMA).unwrap();
        connection.execute_batch("DROP INDEX secret_audit_denial_order; DROP INDEX secret_audit_denial_group; CREATE INDEX secret_audit_denial_order ON secret_audit(sequence) WHERE outcome!='released'; CREATE INDEX secret_audit_denial_group ON secret_audit(agent_id,secret_id,outcome,occurred_at) WHERE outcome!='released';").unwrap();
        connection.execute("INSERT INTO secret_audit(secret_id,project_id,owner_id,agent_id,conversation_id,generation,session_incarnation,request_id,occurred_at,last_occurred_at,outcome) VALUES ('sec_synthetic','project-a','owner-a','agent-a','conversation-a','generation-a','device_user','synthetic-request',1000,1000,'created')",[]).unwrap();
        connection.execute_batch("WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i+1 FROM n WHERE i<4100) INSERT INTO secret_audit(secret_id,project_id,owner_id,agent_id,conversation_id,generation,session_incarnation,request_id,occurred_at,outcome) SELECT 'unavailable','project-a','owner-a','agent-a','conversation-a','generation-a','session-a','request-a',0,'secret_unavailable' FROM n").unwrap();
        initialize(&connection).unwrap();
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM secret_audit WHERE outcome='created'",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            1
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM secret_audit WHERE outcome='secret_unavailable'",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            4096
        );
        let fresh = rusqlite::Connection::open_in_memory().unwrap();
        initialize(&fresh).unwrap();
        assert_eq!(audit_indexes(&connection), audit_indexes(&fresh));
        initialize(&connection).unwrap();
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM secret_audit", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            4097
        );
    }

    fn audit_indexes(connection: &rusqlite::Connection) -> Vec<(String, String)> {
        connection.prepare("SELECT name,sql FROM sqlite_master WHERE type='index' AND tbl_name='secret_audit' ORDER BY name").unwrap().query_map([], |row|Ok((row.get(0)?,row.get(1)?))).unwrap().collect::<rusqlite::Result<Vec<_>>>().unwrap()
    }

    fn audit_columns(connection: &rusqlite::Connection) -> Vec<String> {
        connection
            .prepare("PRAGMA table_info(secret_audit)")
            .unwrap()
            .query_map([], |row| row.get(1))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
    }

    fn assert_denial_indexes(connection: &rusqlite::Connection) {
        for (name, sql) in audit_indexes(connection) {
            if name.contains("denial") {
                assert!(
                    sql.contains("secret_unavailable")
                        && sql.contains("secret_expired")
                        && sql.contains("secret_revoked"),
                    "{sql}"
                );
                assert!(!sql.contains("!='released'"), "{sql}");
            }
        }
    }

    fn assert_d_upgrade(alter_order: bool, missing_groups: bool) {
        let connection = rusqlite::Connection::open_in_memory().unwrap();
        let mut old = SCHEMA.replace(
            "'secret_revoked','created','attached','expired','revoked'",
            "'secret_revoked'",
        );
        if alter_order || missing_groups {
            old = old.replace("    attempt_count INTEGER NOT NULL DEFAULT 1 CHECK (attempt_count > 0),\n    last_occurred_at INTEGER NOT NULL DEFAULT 0,\n", "");
        }
        connection.execute_batch(&old).unwrap();
        if alter_order {
            connection.execute_batch("ALTER TABLE secret_audit ADD COLUMN attempt_count INTEGER NOT NULL DEFAULT 1 CHECK(attempt_count > 0); ALTER TABLE secret_audit ADD COLUMN last_occurred_at INTEGER NOT NULL DEFAULT 0;").unwrap();
        }
        connection.execute("INSERT INTO secret_audit(sequence,secret_id,creator,project_id,owner_id,agent_id,conversation_id,generation,session_incarnation,request_id,occurred_at,outcome) VALUES (500,'sec_synthetic','device_user','project-a','owner-a','agent-a','conversation-a','generation-a','synthetic-session','synthetic-request',1000,'secret_revoked')",[]).unwrap();
        if !missing_groups {
            connection.execute("UPDATE secret_audit SET attempt_count=7,last_occurred_at=1012 WHERE sequence=500",[]).unwrap();
        }
        connection.execute("INSERT INTO secret_audit(sequence,secret_id,project_id,owner_id,agent_id,conversation_id,generation,session_incarnation,request_id,occurred_at,outcome) VALUES (900,'unavailable','project-a','owner-a','agent-a','conversation-a','generation-a','synthetic-session','synthetic-request',1000,'released')",[]).unwrap();
        connection
            .execute("DELETE FROM secret_audit WHERE sequence=900", [])
            .unwrap();
        initialize(&connection).unwrap();
        let value: (i64,i64,String) = connection.query_row("SELECT attempt_count,last_occurred_at,outcome FROM secret_audit WHERE sequence=500",[],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?))).unwrap();
        assert_eq!(
            value,
            if missing_groups {
                (1, 1000, "secret_revoked".into())
            } else {
                (7, 1012, "secret_revoked".into())
            }
        );
        let fresh = rusqlite::Connection::open_in_memory().unwrap();
        initialize(&fresh).unwrap();
        assert_eq!(audit_columns(&connection), audit_columns(&fresh));
        assert_eq!(audit_indexes(&connection), audit_indexes(&fresh));
        assert_eq!(audit_indexes(&connection).len(), 5);
        assert_denial_indexes(&connection);
        connection.execute("INSERT INTO secret_audit(secret_id,project_id,owner_id,agent_id,conversation_id,generation,session_incarnation,request_id,occurred_at,last_occurred_at,outcome) VALUES ('sec_synthetic','project-a','owner-a','agent-a','conversation-a','generation-a','device_user','synthetic-request',1020,1020,'created')",[]).unwrap();
        assert_eq!(connection.last_insert_rowid(), 901);
        initialize(&connection).unwrap();
        assert_eq!(audit_indexes(&connection), audit_indexes(&fresh));
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM secret_audit", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            2
        );
    }

    #[test]
    fn revocation_receipt_ensure_runs_for_full_audit_schema_and_reopens() {
        let connection = rusqlite::Connection::open_in_memory().unwrap();
        connection.execute_batch(&SCHEMA).unwrap();
        initialize(&connection).unwrap();
        let columns = connection
            .prepare("PRAGMA table_info(secret_revocations)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert!(columns.iter().any(|column| column == "accepted_at"));
        assert!(columns.iter().any(|column| column == "secret_id"));
        initialize(&connection).unwrap();
        let index: bool = connection.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='index' AND name='secret_revocations_retention')",[],|row|row.get(0)).unwrap();
        assert!(index);
    }

    #[test]
    fn receipt_ensure_failure_rolls_back_the_audit_upgrade() {
        let connection = rusqlite::Connection::open_in_memory().unwrap();
        let old = SCHEMA.replace(
            "'secret_revoked','created','attached','expired','revoked'",
            "'secret_revoked'",
        );
        connection.execute_batch(&old).unwrap();
        connection.execute_batch("DROP TABLE IF EXISTS secret_revocations; CREATE TABLE secret_revocations(sentinel TEXT)").unwrap();
        let error = initialize(&connection).unwrap_err();
        assert!(error.to_string().contains("accepted_at"), "{error}");
        let sql: String = connection
            .query_row(
                "SELECT sql FROM sqlite_master WHERE name='secret_audit'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(
            !sql.contains("'created'"),
            "receipt and audit upgrades must roll back together"
        );
        connection
            .execute_batch("DROP TABLE secret_revocations")
            .unwrap();
        initialize(&connection).unwrap();
    }

    #[test]
    fn lifecycle_audit_upgrade_keeps_rows_and_deleted_sequence_highwater() {
        upgrade_preserves_sequence("'secret_revoked'");
    }
    #[test]
    fn lifecycle_audit_partial_upgrade_rebuilds_the_complete_outcome_set() {
        upgrade_preserves_sequence("'secret_revoked','created'");
    }
    fn upgrade_preserves_sequence(old_outcomes: &str) {
        let connection = rusqlite::Connection::open_in_memory().unwrap();
        let old = SCHEMA.replace(
            "'secret_revoked','created','attached','expired','revoked'",
            old_outcomes,
        );
        connection.execute_batch(&old).unwrap();
        for sequence in [500, 900] {
            connection.execute("INSERT INTO secret_audit(sequence,secret_id,creator,project_id,owner_id,agent_id,conversation_id,generation,session_incarnation,request_id,occurred_at,outcome) VALUES (?1,'sec_synthetic','device_user','project-a','owner-a','agent-a','conversation-a','generation-a','synthetic-session','synthetic-request',1000,'released')",[sequence]).unwrap();
        }
        connection
            .execute("DELETE FROM secret_audit WHERE sequence=900", [])
            .unwrap();
        initialize(&connection).unwrap();
        let ddl: String = connection
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type='table' AND name='secret_audit'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        for outcome in AUDIT_OUTCOMES {
            assert!(
                ddl.contains(&format!("'{outcome}'")),
                "{outcome} missing from rebuilt CHECK"
            );
        }
        connection.execute("INSERT INTO secret_audit(secret_id,creator,project_id,owner_id,agent_id,conversation_id,generation,session_incarnation,request_id,occurred_at,outcome) VALUES ('sec_synthetic','device_user','project-a','owner-a','agent-a','conversation-a','generation-a','device_user','synthetic-request',1001,'created')",[]).unwrap();
        let rows = connection
            .prepare("SELECT sequence,outcome FROM secret_audit ORDER BY sequence")
            .unwrap()
            .query_map([], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })
            .unwrap()
            .collect::<std::result::Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(
            rows,
            vec![(500, "released".into()), (901, "created".into())]
        );
        initialize(&connection).unwrap();
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM secret_audit", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            2
        );
    }
}

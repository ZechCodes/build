pub(in crate::store) const SCHEMA: &str = r#"
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
    outcome TEXT NOT NULL CHECK (outcome IN ('released','secret_unavailable','secret_expired','secret_revoked','created','attached','expired','revoked'))
);
CREATE INDEX IF NOT EXISTS secret_audit_by_secret ON secret_audit(secret_id,sequence);
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
    connection.execute_batch(SCHEMA)?;
    let columns = connection
        .prepare("PRAGMA table_info(secret_records)")?
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if !columns.iter().any(|column| column == "revocation_reason") {
        connection.execute_batch("ALTER TABLE secret_records ADD COLUMN revocation_reason TEXT CHECK (revocation_reason IS NULL OR revocation_reason IN ('key_lost','revoked','expired'))")?;
    }
    ensure_pending_key_column(connection)?;
    ensure_lifecycle_audit(connection)?;
    Ok(())
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

/// Preserve sequence numbers, rows and the AUTOINCREMENT high-water while
/// extending the CHECK from the pre-wire schema. Savepoint composes with an
/// existing migration transaction and makes every crash observe one table.
fn ensure_lifecycle_audit(connection: &rusqlite::Connection) -> rusqlite::Result<()> {
    let sql: String = connection.query_row(
        "SELECT sql FROM sqlite_master WHERE type='table' AND name='secret_audit'",
        [],
        |row| row.get(0),
    )?;
    if sql.contains("'created'") {
        return Ok(());
    }
    let revised = sql.replace("secret_audit", "secret_audit_next").replace(
        "'secret_revoked'",
        "'secret_revoked','created','attached','expired','revoked'",
    );
    connection.execute_batch("SAVEPOINT secret_audit_upgrade")?;
    let result = (|| {
        connection.execute_batch(&revised)?;
        connection.execute_batch("INSERT INTO secret_audit_next SELECT * FROM secret_audit; INSERT INTO sqlite_sequence(name,seq) SELECT 'secret_audit_next',seq FROM sqlite_sequence WHERE name='secret_audit' AND NOT EXISTS(SELECT 1 FROM sqlite_sequence WHERE name='secret_audit_next'); UPDATE sqlite_sequence SET seq=MAX(seq,COALESCE((SELECT seq FROM sqlite_sequence WHERE name='secret_audit'),0)) WHERE name='secret_audit_next'; DROP TABLE secret_audit; ALTER TABLE secret_audit_next RENAME TO secret_audit; CREATE INDEX secret_audit_by_secret ON secret_audit(secret_id,sequence); CREATE INDEX secret_audit_retention ON secret_audit(occurred_at,sequence);")?;
        connection.execute_batch("RELEASE secret_audit_upgrade")
    })();
    if result.is_err() {
        let _ = connection
            .execute_batch("ROLLBACK TO secret_audit_upgrade; RELEASE secret_audit_upgrade");
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lifecycle_audit_upgrade_keeps_rows_and_deleted_sequence_highwater() {
        let connection = rusqlite::Connection::open_in_memory().unwrap();
        let old = SCHEMA.replace(
            "'secret_revoked','created','attached','expired','revoked'",
            "'secret_revoked'",
        );
        connection.execute_batch(&old).unwrap();
        for sequence in [500, 900] {
            connection.execute("INSERT INTO secret_audit(sequence,secret_id,creator,project_id,owner_id,agent_id,conversation_id,generation,session_incarnation,request_id,occurred_at,outcome) VALUES (?1,'sec_synthetic','device_user','project-a','owner-a','agent-a','conversation-a','generation-a','synthetic-session','synthetic-request',1000,'released')",[sequence]).unwrap();
        }
        connection
            .execute("DELETE FROM secret_audit WHERE sequence=900", [])
            .unwrap();
        initialize(&connection).unwrap();
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

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
    active_key_id TEXT
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
    Ok(())
}

-- Frozen schema-26 secret tables, before environment grants.

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
CREATE INDEX IF NOT EXISTS secret_audit_denial_order ON secret_audit(sequence) WHERE outcome IN ('secret_unavailable','secret_expired','secret_revoked');
CREATE INDEX IF NOT EXISTS secret_audit_denial_group ON secret_audit(agent_id,secret_id,outcome,occurred_at) WHERE outcome IN ('secret_unavailable','secret_expired','secret_revoked');
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


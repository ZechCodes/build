//! Durable command identities, reservations, owner order and confirmation outbox.
//! These APIs accept authenticated, scope-validated internal commands. They do
//! not grant authority from envelope fields or execute a public RPC method.
use super::{ReadTransaction, StoreError, StoreWriter};
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

mod execution;
mod reads;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct MutationOwner {
    pub kind: String,
    pub id: String,
    pub generation: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct MutationOwnerSequence {
    pub kind: String,
    pub id: String,
    pub generation: u64,
    pub sequence: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct MutationResource {
    pub kind: String,
    pub id: String,
    pub owner: MutationOwner,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MutationAdmission {
    pub principal: String,
    pub operation_id: String,
    pub method: String,
    pub params: Value,
    /// Immutable authority binding, including source/thread generations.
    pub scope: Value,
    pub owners: Vec<MutationOwner>,
    pub resources: Vec<MutationResource>,
    pub reservations: Vec<MutationResource>,
    pub after: Option<Vec<String>>,
    pub recovery_policy: String,
    pub accepted_epoch: String,
    pub acceptance: Value,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct MutationRecord {
    pub command_id: i64,
    pub principal: String,
    pub operation_id: String,
    pub method: String,
    pub params: Value,
    pub scope: Value,
    pub owners: Vec<MutationOwnerSequence>,
    pub resources: Vec<MutationResource>,
    pub reservations: Vec<MutationResource>,
    pub dependencies: Vec<String>,
    pub recovery_policy: String,
    pub accepted_epoch: String,
    pub accepted_at: String,
    pub revision: u64,
    pub state: String,
    pub acceptance: Value,
    pub outcome: Option<Value>,
    pub intent_hash: String,
    pub expired: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MutationOutboxEntry {
    pub cursor: u64,
    pub operation: MutationRecord,
}
#[derive(Default, Debug)]
pub struct MutationRecovery {
    pub accepted: Vec<MutationRecord>,
    pub committed: Vec<MutationRecord>,
    pub terminal: Vec<MutationRecord>,
}

const JOURNAL_SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS mutation_commands (
 command_id INTEGER PRIMARY KEY AUTOINCREMENT, principal TEXT NOT NULL,
 operation_id TEXT NOT NULL, state TEXT NOT NULL, accepted_at INTEGER NOT NULL,
 terminal_at INTEGER, intent TEXT NOT NULL, record TEXT NOT NULL,
 UNIQUE(principal, operation_id));
CREATE INDEX IF NOT EXISTS mutation_commands_state ON mutation_commands(state, command_id);
CREATE TABLE IF NOT EXISTS mutation_owner_order (
 kind TEXT NOT NULL, owner_id TEXT NOT NULL, generation INTEGER NOT NULL,
 sequence INTEGER NOT NULL, command_id INTEGER NOT NULL REFERENCES mutation_commands(command_id),
 active INTEGER NOT NULL DEFAULT 1,
 PRIMARY KEY(kind, owner_id, generation, sequence));
CREATE INDEX IF NOT EXISTS mutation_order_command ON mutation_owner_order(command_id);
CREATE INDEX IF NOT EXISTS mutation_active_order ON mutation_owner_order(kind, owner_id, generation, sequence) WHERE active=1;
CREATE TABLE IF NOT EXISTS mutation_dependencies (
 command_id INTEGER NOT NULL REFERENCES mutation_commands(command_id),
 predecessor INTEGER NOT NULL REFERENCES mutation_commands(command_id),
 PRIMARY KEY(command_id, predecessor));
CREATE TABLE IF NOT EXISTS mutation_reservations (
 kind TEXT NOT NULL, resource_id TEXT NOT NULL, command_id INTEGER NOT NULL REFERENCES mutation_commands(command_id),
 record TEXT NOT NULL, PRIMARY KEY(kind, resource_id));
CREATE TABLE IF NOT EXISTS mutation_outbox (
 cursor INTEGER PRIMARY KEY AUTOINCREMENT,
 principal TEXT NOT NULL,
 command_id INTEGER NOT NULL REFERENCES mutation_commands(command_id), revision INTEGER NOT NULL,
 UNIQUE(command_id, revision));
CREATE INDEX IF NOT EXISTS mutation_outbox_principal_cursor ON mutation_outbox(principal, cursor);
";

pub(super) fn initialize(connection: &Connection) -> Result<String, StoreError> {
    let transaction = connection.unchecked_transaction()?;
    transaction.execute_batch(JOURNAL_SCHEMA)?;
    transaction.execute(
        "INSERT OR IGNORE INTO meta(key,value) VALUES('journal_incarnation',?1)",
        [uuid::Uuid::new_v4().to_string()],
    )?;
    let incarnation = transaction.query_row(
        "SELECT value FROM meta WHERE key='journal_incarnation'",
        [],
        |row| row.get(0),
    )?;
    transaction.commit()?;
    Ok(incarnation)
}
fn invalid(message: &str) -> StoreError {
    StoreError::MutationInvalid(message.into())
}
fn decode(record: String) -> Result<MutationRecord, StoreError> {
    serde_json::from_str(&record).map_err(|error| invalid(&format!("corrupt command: {error}")))
}
fn load(
    connection: &Connection,
    principal: &str,
    id: &str,
) -> Result<Option<MutationRecord>, StoreError> {
    connection
        .query_row(
            "SELECT record FROM mutation_commands WHERE principal=?1 AND operation_id=?2",
            params![principal, id],
            |row| row.get(0),
        )
        .optional()?
        .map(decode)
        .transpose()
}
fn save(transaction: &Transaction<'_>, record: &MutationRecord) -> Result<(), StoreError> {
    transaction.execute(
        "UPDATE mutation_commands SET state=?1,record=?2 WHERE command_id=?3",
        params![record.state, json!(record).to_string(), record.command_id],
    )?;
    Ok(())
}
fn sorted_dependencies(values: &[String]) -> Vec<String> {
    let mut values = values.to_vec();
    values.sort();
    values.dedup();
    values
}
fn intent(admission: &MutationAdmission, dependencies: &[String]) -> String {
    let mut owners = admission.owners.clone();
    owners.sort();
    owners.dedup();
    let mut resources = admission.resources.clone();
    resources.sort();
    resources.dedup();
    let mut reservations = admission.reservations.clone();
    reservations.sort();
    reservations.dedup();
    json!({"method":admission.method,"params":admission.params,"scope":admission.scope,"owners":owners,"resources":resources,"reservations":reservations,"after":dependencies,"recovery_policy":admission.recovery_policy}).to_string()
}
fn validate(admission: &MutationAdmission) -> Result<(), StoreError> {
    if uuid::Uuid::parse_str(&admission.operation_id)
        .map(|id| id.to_string())
        .ok()
        .as_deref()
        != Some(admission.operation_id.as_str())
    {
        return Err(invalid("operation_id must be a canonical UUID"));
    }
    if !["sql_only", "external"].contains(&admission.recovery_policy.as_str()) {
        return Err(invalid("unknown recovery policy"));
    }
    if admission.principal.is_empty() || admission.method.is_empty() || admission.owners.is_empty()
    {
        return Err(invalid("principal, method and owners required"));
    }
    if admission.owners.len() > 16
        || admission.resources.len() > 32
        || admission.reservations.len() > 32
    {
        return Err(invalid("scope limit exceeded"));
    }
    if json!(admission).to_string().len() > 64 * 1024 {
        return Err(invalid("intent byte limit exceeded"));
    }
    if admission
        .owners
        .iter()
        .any(|owner| owner.generation > i64::MAX as u64)
    {
        return Err(invalid("generation exceeds SQLite range"));
    }
    for resource in admission.resources.iter().chain(&admission.reservations) {
        if !admission.owners.contains(&resource.owner) {
            return Err(invalid("resource owner missing"));
        }
    }
    Ok(())
}
fn check_quota(
    transaction: &Transaction<'_>,
    principal: &str,
    added_bytes: usize,
) -> Result<(), StoreError> {
    let (total, own, bytes, own_bytes):(i64,i64,i64,i64)=transaction.query_row("SELECT count(*),coalesce(sum(principal=?1),0),coalesce(sum(length(CAST(intent AS BLOB))),0),coalesce(sum(CASE WHEN principal=?1 THEN length(CAST(intent AS BLOB)) ELSE 0 END),0) FROM mutation_commands WHERE state IN ('accepted','executing','committed')",[principal],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?)))?;
    if total >= 4096
        || own >= 128
        || bytes + added_bytes as i64 > 128 * 1024 * 1024
        || own_bytes + added_bytes as i64 > 8 * 1024 * 1024
    {
        return Err(StoreError::MutationBusy);
    }
    Ok(())
}
fn required_creators(
    transaction: &Transaction<'_>,
    admission: &MutationAdmission,
) -> Result<Vec<String>, StoreError> {
    let mut required = Vec::new();
    for resource in &admission.resources {
        let creator:Option<(String,String,String,String)>=transaction.query_row("SELECT c.principal,c.operation_id,c.state,r.record FROM mutation_reservations r JOIN mutation_commands c USING(command_id) WHERE r.kind=?1 AND r.resource_id=?2",params![resource.kind,resource.id],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?))).optional()?;
        if let Some((principal, id, state, saved)) = creator {
            if state == "succeeded" {
                continue;
            }
            if principal != admission.principal {
                return Err(StoreError::MutationNotYetAvailable);
            }
            let saved: MutationResource = serde_json::from_str(&saved)
                .map_err(|error| invalid(&format!("corrupt reservation: {error}")))?;
            if &saved != resource {
                return Err(invalid("reservation scope mismatch"));
            }
            if state != "succeeded" {
                required.push(id);
            }
        }
    }
    Ok(sorted_dependencies(&required))
}
fn dependencies(
    transaction: &Transaction<'_>,
    admission: &MutationAdmission,
) -> Result<Vec<String>, StoreError> {
    let required = required_creators(transaction, admission)?;
    let selected = admission
        .after
        .as_ref()
        .map(|values| sorted_dependencies(values))
        .unwrap_or_else(|| required.clone());
    if selected.len() > 8 || required.iter().any(|id| !selected.contains(id)) {
        return Err(invalid("invalid dependency override or limit"));
    }
    for id in &selected {
        if id == &admission.operation_id {
            return Err(invalid("self dependency"));
        }
        let predecessor = load(transaction, &admission.principal, id)?
            .ok_or_else(|| invalid("unknown or unauthorized dependency"))?;
        if !predecessor.owners.iter().any(|owner| {
            admission.owners.iter().any(|current| {
                current.kind == owner.kind
                    && current.id == owner.id
                    && current.generation == owner.generation
            })
        }) || predecessor.state == "failed"
        {
            return Err(invalid("invalid scope or failed dependency"));
        }
    }
    Ok(selected)
}
fn retry(
    transaction: &Transaction<'_>,
    admission: &MutationAdmission,
    record: MutationRecord,
) -> Result<MutationRecord, StoreError> {
    let selected = admission
        .after
        .as_ref()
        .map(|ids| sorted_dependencies(ids))
        .unwrap_or_else(|| record.dependencies.clone());
    let original: String = transaction.query_row(
        "SELECT intent FROM mutation_commands WHERE command_id=?1",
        [record.command_id],
        |row| row.get(0),
    )?;
    let hash = format!(
        "{:x}",
        Sha256::digest(intent(admission, &selected).as_bytes())
    );
    if hash != record.intent_hash || (!record.expired && original != intent(admission, &selected)) {
        return Err(StoreError::OperationConflict {
            operation_id: admission.operation_id.clone(),
        });
    }
    Ok(record)
}
impl StoreWriter {
    /// The caller validates authority before this short SQL-only transaction.
    /// No compatibility reservation or application mutex may be held here.
    pub fn admit_mutation(
        &self,
        admission: &MutationAdmission,
    ) -> Result<MutationRecord, StoreError> {
        self.transaction(|transaction| {
            if let Some(record) = load(transaction, &admission.principal, &admission.operation_id)? {
                return retry(transaction, admission, record);
            }
            validate(admission)?;
            let dependencies = dependencies(transaction, admission)?;
            let normalized = intent(admission, &dependencies);
            check_quota(transaction, &admission.principal, normalized.len())?;
            transaction.execute(
                "INSERT INTO mutation_commands(principal,operation_id,state,accepted_at,intent,record)
                 VALUES(?1,?2,'accepted',unixepoch(),?3,'{}')",
                params![admission.principal, admission.operation_id, normalized],
            )?;
            let command_id = transaction.last_insert_rowid();
            let owners = reserve_owners(transaction, command_id, &admission.owners)?;
            let record = MutationRecord {
                command_id,
                principal: admission.principal.clone(),
                operation_id: admission.operation_id.clone(),
                method: admission.method.clone(),
                params: admission.params.clone(),
                scope: admission.scope.clone(),
                owners,
                resources: admission.resources.clone(),
                reservations: admission.reservations.clone(),
                dependencies,
                recovery_policy: admission.recovery_policy.clone(),
                accepted_epoch: admission.accepted_epoch.clone(),
                accepted_at: super::now_rfc3339(),
                revision: 1,
                state: "accepted".into(),
                acceptance: admission.acceptance.clone(),
                outcome: None,
                intent_hash: format!("{:x}", Sha256::digest(normalized.as_bytes())),
                expired: false,
            };
            persist_links(transaction, &record)?;
            save(transaction, &record)?;
            Ok(record)
        })
    }
}
fn reserve_owners(
    transaction: &Transaction<'_>,
    command_id: i64,
    owners: &[MutationOwner],
) -> Result<Vec<MutationOwnerSequence>, StoreError> {
    let mut ordered = owners.to_vec();
    ordered.sort();
    ordered.dedup();
    let mut result = Vec::new();
    for owner in ordered {
        let sequence:i64=transaction.query_row("SELECT coalesce(max(sequence),0)+1 FROM mutation_owner_order WHERE kind=?1 AND owner_id=?2 AND generation=?3",params![owner.kind,owner.id,owner.generation as i64],|row|row.get(0))?;
        transaction.execute("INSERT INTO mutation_owner_order(kind,owner_id,generation,sequence,command_id) VALUES(?1,?2,?3,?4,?5)",params![owner.kind,owner.id,owner.generation as i64,sequence,command_id])?;
        result.push(MutationOwnerSequence {
            kind: owner.kind,
            id: owner.id,
            generation: owner.generation,
            sequence: sequence as u64,
        });
    }
    Ok(result)
}
fn persist_links(transaction: &Transaction<'_>, record: &MutationRecord) -> Result<(), StoreError> {
    for id in &record.dependencies {
        transaction.execute("INSERT INTO mutation_dependencies(command_id,predecessor) SELECT ?1,command_id FROM mutation_commands WHERE principal=?2 AND operation_id=?3",params![record.command_id,record.principal,id])?;
    }
    for resource in &record.reservations {
        let inserted=transaction.execute("INSERT OR IGNORE INTO mutation_reservations(kind,resource_id,command_id,record) VALUES(?1,?2,?3,?4)",params![resource.kind,resource.id,record.command_id,json!(resource).to_string()])?;
        if inserted == 0 {
            return Err(invalid("resource already reserved"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;

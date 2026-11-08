//! Creation checkpoints retain external evidence without publishing an outcome.
use super::execution::{require, terminal};
use super::*;

pub(super) fn is_creation(method: &str) -> bool {
    matches!(method, "environment.create_project" | "workspace.create")
}
fn nonterminal(record: &MutationRecord) -> bool {
    matches!(
        record.state.as_str(),
        "accepted" | "executing" | "unknown" | "reconciling" | "committed"
    )
}
fn semantic_params(params: &Value) -> Value {
    let mut value = params.clone();
    if let Some(object) = value.as_object_mut() {
        object.remove("continues");
    }
    value
}
pub(super) fn check_conflicting_intent(
    tx: &Transaction<'_>,
    admission: &MutationAdmission,
) -> Result<(), StoreError> {
    if !is_creation(&admission.method) {
        return Ok(());
    }
    let mut statement = tx.prepare("SELECT record FROM mutation_commands WHERE principal=?1 AND state IN ('accepted','executing','unknown','reconciling') AND json_extract(record,'$.method')=?2")?;
    let records = statement.query_map(params![admission.principal, admission.method], |row| {
        row.get::<_, String>(0)
    })?;
    for raw in records {
        let record = decode(raw?)?;
        if semantic_params(&record.params) == semantic_params(&admission.params) {
            return Err(StoreError::OperationConflict {
                operation_id: record.operation_id,
            });
        }
    }
    Ok(())
}
/// A landed project's workspace retry has a different owner kind from the
/// environment attempt that created it. Keep that boundary inside the claim.
pub(super) fn project_blocked(
    tx: &Transaction<'_>,
    record: &MutationRecord,
) -> Result<bool, StoreError> {
    if !is_creation(&record.method) {
        return Ok(false);
    }
    let Some(project) = record.execution["project_id"].as_str() else {
        return Ok(false);
    };
    Ok(tx.query_row("SELECT EXISTS(SELECT 1 FROM mutation_commands WHERE command_id<>?1 AND state IN ('executing','unknown','reconciling') AND json_extract(record,'$.method') IN ('environment.create_project','workspace.create') AND json_extract(record,'$.execution.project_id')=?2)",params![record.command_id,project],|row|row.get(0))?)
}
impl StoreWriter {
    pub fn creation_record(
        &self,
        principal: &str,
        id: &str,
    ) -> Result<Option<MutationRecord>, StoreError> {
        load(&self.connection(), principal, id)
    }
    pub fn pending_creations(&self) -> Result<Vec<MutationRecord>, StoreError> {
        let connection = self.connection();
        let mut statement = connection.prepare("SELECT record FROM mutation_commands WHERE state IN ('accepted','executing','unknown','reconciling','committed') AND json_extract(record,'$.method') IN ('environment.create_project','workspace.create') ORDER BY command_id")?;
        let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
        rows.map(|row| decode(row?)).collect()
    }
    pub fn checkpoint_creation(
        &self,
        principal: &str,
        id: &str,
        evidence: Value,
    ) -> Result<MutationRecord, StoreError> {
        self.park_creation(principal, id, "executing", evidence)
    }
    pub fn park_creation(
        &self,
        principal: &str,
        id: &str,
        state: &str,
        evidence: Value,
    ) -> Result<MutationRecord, StoreError> {
        if !matches!(state, "executing" | "unknown" | "reconciling")
            || !evidence.is_object()
            || evidence.to_string().len() > 160 * 1024
        {
            return Err(invalid("invalid creation checkpoint"));
        }
        // Checkpoints precede external filesystem/config effects. Flush their
        // identity and ownership evidence before those effects can become durable.
        let record = self.durable_transaction(|tx| {
            let mut record = require(tx, principal, id)?;
            if !is_creation(&record.method) || !nonterminal(&record) {
                return Err(invalid("creation is already terminal"));
            }
            record.execution = evidence;
            record.state = state.into();
            record.revision += 1;
            save(tx, &record)?;
            Ok(record)
        })?;
        self.wakeup.notify_waiters();
        Ok(record)
    }
    pub fn finish_creation(
        &self,
        principal: &str,
        id: &str,
        mut outcome: Value,
    ) -> Result<MutationRecord, StoreError> {
        let state = outcome["state"].as_str().unwrap_or("").to_string();
        if state != "succeeded"
            && !(state == "failed"
                && matches!(outcome["effect"].as_str(), Some("unapplied" | "partial")))
        {
            return Err(invalid("creation outcome must be proven"));
        }
        let record = self.transaction(|tx| {
            let mut record = require(tx, principal, id)?;
            if !is_creation(&record.method) || !nonterminal(&record) {
                return Err(invalid("creation is already terminal"));
            }
            if record
                .execution
                .get("residue")
                .is_some_and(|value| !value.is_null())
            {
                return Err(invalid("creation cleanup residue is not settled"));
            }
            outcome["creation"] = record.execution.clone();
            terminal(tx, &mut record, &state, outcome.clone())?;
            tx.execute(
                "UPDATE mutation_owner_order SET active=0 WHERE command_id=?1",
                [record.command_id],
            )?;
            Ok(record)
        })?;
        self.wakeup.notify_waiters();
        Ok(record)
    }
}

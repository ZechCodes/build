//! Internal deletion records use schema-18 JSON storage, without entering the
//! public command executor. Old readers ignore these two SQL states.
use super::*;
use std::path::PathBuf;

const PRINCIPAL: &str = "internal/project-deletion";
const PENDING: &str = "deletion_pending";
const TERMINAL: &str = "deletion_complete";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProjectDeletionPlan {
    pub project_id: String,
    pub project_key: String,
    pub project_config: Value,
    pub environment_config: Value,
    pub delete_tasks: bool,
    pub roots: Vec<PathBuf>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lock_markers: Vec<PathBuf>,
    pub checkouts: Vec<ProjectDeletionCheckout>,
    pub rift_root: PathBuf,
    pub run_ids: Vec<String>,
    pub plan_ids: Vec<String>,
    pub capture_ids: Vec<String>,
    pub workspace_ids: Vec<String>,
    pub inventory: std::collections::BTreeMap<PathBuf, String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProjectDeletionCheckout {
    pub source: PathBuf,
    pub path: PathBuf,
    pub registered_name: String,
    pub registry_root: PathBuf,
    pub isolation: crate::isolation::Isolation,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProjectDeletion {
    pub command_id: i64,
    pub plan: ProjectDeletionPlan,
    pub history: String,
    pub phase: String,
    pub tombstone: Option<String>,
    #[serde(default)]
    pub pending_pins: Vec<crate::reviews::history_pins::HistoryPins>,
}

fn records(connection: &Connection) -> Result<Vec<ProjectDeletion>, StoreError> {
    let mut statement = connection
        .prepare("SELECT record FROM mutation_commands WHERE principal=?1 ORDER BY command_id")?;
    let rows = statement.query_map([PRINCIPAL], |row| row.get::<_, String>(0))?;
    rows.map(|raw| {
        serde_json::from_value(decode(raw?)?.execution)
            .map_err(|error| invalid(&format!("corrupt project deletion: {error}")))
    })
    .collect()
}

pub(in crate::store) fn checkpoint(
    tx: &Transaction<'_>,
    deletion: &ProjectDeletion,
) -> Result<(), StoreError> {
    let raw: String = tx.query_row(
        "SELECT record FROM mutation_commands WHERE command_id=?1 AND principal=?2",
        params![deletion.command_id, PRINCIPAL],
        |row| row.get(0),
    )?;
    let mut record = decode(raw)?;
    if record.execution["plan"] != json!(deletion.plan) {
        return Err(invalid("project deletion cleanup plan is immutable"));
    }
    let mut deletion = deletion.clone();
    if deletion.phase == "history_removed" {
        deletion.history = history(tx, &deletion.plan.project_key)?;
    }
    record.execution = json!(deletion);
    record.state = if deletion.tombstone.is_some() {
        TERMINAL
    } else {
        PENDING
    }
    .into();
    record.revision += 1;
    save(tx, &record)?;
    if deletion.tombstone.is_some() {
        tx.execute(
            "UPDATE mutation_commands SET terminal_at=unixepoch() WHERE command_id=?1",
            [deletion.command_id],
        )?;
    }
    Ok(())
}

/// Rows are hashed in primary-key order, including comments, reviews and
/// assignment intents. Recovery never guesses that changed history is ours.
fn history(connection: &Connection, key: &str) -> Result<String, StoreError> {
    let mut hash = Sha256::new();
    for (table, predicate) in [
        ("tracker_tasks", "project_key=?1"),
        ("tracker_task_dependencies", "task_id IN (SELECT id FROM tracker_tasks WHERE project_key=?1) OR waiting_on_task_id IN (SELECT id FROM tracker_tasks WHERE project_key=?1)"),
        ("tracker_delivery_notices", "project_key=?1"),
        ("review_workspace_claims", "project_key=?1"),
        ("review_branch_bindings", "task_id IN (SELECT id FROM tracker_tasks WHERE project_key=?1)"),
        ("review_sync_candidates", "task_id IN (SELECT id FROM tracker_tasks WHERE project_key=?1)"),
        ("review_sync_observations", "task_id IN (SELECT id FROM tracker_tasks WHERE project_key=?1)"),
        ("review_actions", "task_id IN (SELECT id FROM tracker_tasks WHERE project_key=?1)"),
        ("review_revision_dispatches", "task_id IN (SELECT id FROM tracker_tasks WHERE project_key=?1)"),
        ("review_verdict_deliveries", "task_id IN (SELECT id FROM tracker_tasks WHERE project_key=?1)"),
        ("review_openings", "project_key=?1"),
        ("review_merge_intents", "project_key=?1"),
        (
            "tracker_comments",
            "task_id IN (SELECT id FROM tracker_tasks WHERE project_key=?1)",
        ),
        (
            "tracker_events",
            "task_id IN (SELECT id FROM tracker_tasks WHERE project_key=?1)",
        ),
        (
            "tracker_delivery_intents",
            "task_id IN (SELECT id FROM tracker_tasks WHERE project_key=?1)",
        ),
        (
            "reviews",
            "task_id IN (SELECT id FROM tracker_tasks WHERE project_key=?1)",
        ),
        (
            "review_snapshots",
            "task_id IN (SELECT id FROM tracker_tasks WHERE project_key=?1)",
        ),
    ] {
        hash.update(table.as_bytes());
        let mut statement = connection.prepare(&format!(
            "SELECT * FROM {table} WHERE {predicate} ORDER BY 1"
        ))?;
        let columns = statement.column_count();
        let mut rows = statement.query([key])?;
        while let Some(row) = rows.next()? {
            for column in 0..columns {
                let value = row.get_ref(column)?;
                hash.update(format!("{value:?}\n").as_bytes());
            }
        }
    }
    Ok(format!("{:x}", hash.finalize()))
}

impl StoreWriter {
    pub fn project_deletions(&self) -> Result<Vec<ProjectDeletion>, StoreError> {
        records(&self.connection())
    }

    pub fn begin_project_deletion(
        &self,
        plan: ProjectDeletionPlan,
    ) -> Result<ProjectDeletion, StoreError> {
        self.durable_transaction(|tx| {
            if records(tx)?.iter().any(|record| record.tombstone.is_none()
                && (record.plan.project_id == plan.project_id || record.plan.project_key == plan.project_key)) {
                return Err(invalid("project deletion is already pending"));
            }
            let fingerprint = history(tx, &plan.project_key)?;
            let operation_id=uuid::Uuid::new_v4().to_string();
            tx.execute("INSERT INTO mutation_commands(principal,operation_id,state,accepted_at,intent,record) VALUES(?1,?2,?3,unixepoch(),?4,'{}')",
                params![PRINCIPAL, operation_id, PENDING, json!(plan).to_string()])?;
            let command_id = tx.last_insert_rowid();
            let deletion = ProjectDeletion { command_id, plan, history:fingerprint, phase:"prepared".into(), tombstone:None, pending_pins:vec![] };
            let record = MutationRecord {
                command_id, principal:PRINCIPAL.into(), operation_id,
                method:"project.delete".into(), params:json!({}), scope:json!({}), owners:vec![], resources:vec![],
                reservations:vec![], dependencies:vec![], recovery_policy:"external".into(),
                accepted_epoch:crate::bridge_epoch::current().into(), accepted_at:super::super::now_rfc3339(),
                revision:1, state:PENDING.into(), acceptance:Value::Null, outcome:None,
                intent_hash:String::new(), expired:false, execution:json!(deletion),
            };
            save(tx, &record)?;
            Ok(deletion)
        })
    }

    pub fn checkpoint_project_deletion(
        &self,
        deletion: &ProjectDeletion,
    ) -> Result<(), StoreError> {
        self.durable_transaction(|tx| checkpoint(tx, deletion))
    }

    pub fn project_deletion_history_unchanged(
        &self,
        deletion: &ProjectDeletion,
    ) -> Result<bool, StoreError> {
        Ok(history(&self.connection(), &deletion.plan.project_key)? == deletion.history)
    }

    /// Eagerly bound internal tombstones to 256 and thirty days. Pending plans
    /// are never collected; a collected identity cannot authorize a retry.
    pub fn collect_project_deletion_tombstones(&self) -> Result<(), StoreError> {
        self.durable_transaction(|tx| {
            tx.execute("DELETE FROM mutation_commands WHERE principal=?1 AND state=?2 AND (terminal_at<unixepoch()-30*86400 OR command_id NOT IN (SELECT command_id FROM mutation_commands WHERE principal=?1 AND state=?2 ORDER BY command_id DESC LIMIT 256))",params![PRINCIPAL,TERMINAL])?;
            Ok(())
        })
    }
}

impl super::super::Store {
    pub fn project_deletion_pending(&self, identity: &str) -> Result<bool, StoreError> {
        ReadTransaction::new(&self.connection()).project_deletion_pending(identity)
    }
}
impl ReadTransaction<'_> {
    pub fn project_deletion_pending(&self, identity: &str) -> Result<bool, StoreError> {
        Ok(self.connection().query_row("SELECT EXISTS(SELECT 1 FROM mutation_commands WHERE principal=?1 AND state=?2 AND (json_extract(record,'$.execution.plan.project_id')=?3 OR json_extract(record,'$.execution.plan.project_key')=?3))",
            params![PRINCIPAL,PENDING,identity],|row|row.get(0))?)
    }
}

pub(in crate::store) fn verify_history(
    tx: &Transaction<'_>,
    deletion: &ProjectDeletion,
) -> Result<(), StoreError> {
    if history(tx, &deletion.plan.project_key)? != deletion.history {
        return Err(invalid(
            "project deletion-pending history changed; restart to abandon the stale plan",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;
    fn plan() -> ProjectDeletionPlan {
        ProjectDeletionPlan {
            project_id: "proj-1".into(),
            project_key: "project:one".into(),
            project_config: json!({"id":"proj-1"}),
            environment_config: json!({"id":"env-1"}),
            delete_tasks: true,
            roots: vec![],
            lock_markers: vec![],
            checkouts: vec![],
            rift_root: "/unused".into(),
            run_ids: vec![],
            plan_ids: vec![],
            capture_ids: vec![],
            workspace_ids: vec![],
            inventory: Default::default(),
        }
    }
    #[test]
    fn deletion_record_is_compatible_with_previous_journal_reader() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::new(root.path()).unwrap();
        let before: String = store
            .connection()
            .query_row(
                "SELECT value FROM meta WHERE key='schema_version'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let deletion = store.writer().begin_project_deletion(plan()).unwrap();
        let raw: String = store
            .connection()
            .query_row(
                "SELECT record FROM mutation_commands WHERE command_id=?1",
                [deletion.command_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(decode(raw.clone()).unwrap().method, "project.delete");
        let recovery = store.writer().recover_mutations().unwrap();
        assert!(
            recovery.accepted.is_empty()
                && recovery.committed.is_empty()
                && recovery.terminal.is_empty()
        );
        let after: String = store
            .connection()
            .query_row(
                "SELECT record FROM mutation_commands WHERE command_id=?1",
                [deletion.command_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(raw, after);
        let version: String = store
            .connection()
            .query_row(
                "SELECT value FROM meta WHERE key='schema_version'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(before, version);
        assert!(store.project_deletion_pending("project:one").unwrap());
        assert!(store.project_deletion_pending("proj-1").unwrap());
        drop(store);
        let store = Store::new(root.path()).unwrap();
        assert_eq!(store.writer().project_deletions().unwrap().len(), 1);
    }
    #[test]
    fn deletion_tombstones_are_bounded_and_pending_records_survive_collection() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::new(root.path()).unwrap();
        let mut deletion = store.writer().begin_project_deletion(plan()).unwrap();
        deletion.tombstone = Some("deleted".into());
        deletion.phase = "complete".into();
        store
            .writer()
            .checkpoint_project_deletion(&deletion)
            .unwrap();
        store.writer().transaction(|tx| {
            for n in 0..300 {
                tx.execute("INSERT INTO mutation_commands(principal,operation_id,state,accepted_at,terminal_at,intent,record) SELECT principal,?1,state,accepted_at,terminal_at,intent,record FROM mutation_commands WHERE command_id=?2",
                    params![format!("synthetic-{n}"),deletion.command_id])?;
            }
            Ok(())
        }).unwrap();
        let mut pending = plan();
        pending.project_id = "proj-2".into();
        pending.project_key = "project:two".into();
        store.writer().begin_project_deletion(pending).unwrap();
        store
            .writer()
            .collect_project_deletion_tombstones()
            .unwrap();
        assert_eq!(store.writer().project_deletions().unwrap().len(), 257);
        store
            .writer()
            .transaction(|tx| {
                tx.execute(
                    "UPDATE mutation_commands SET terminal_at=0 WHERE state=?1",
                    [TERMINAL],
                )?;
                Ok(())
            })
            .unwrap();
        store
            .writer()
            .collect_project_deletion_tombstones()
            .unwrap();
        let records = store.writer().project_deletions().unwrap();
        assert_eq!(records.len(), 1);
        assert!(records[0].tombstone.is_none());
    }
}

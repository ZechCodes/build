use super::*;
use crate::reviews::records::{Review, ReviewState};
use crate::store::now_rfc3339;
use crate::store::reviews::{load_review, next_snapshot_number, snapshot_exists, write_header};
use crate::store::tracker::{append_activity, write_tracker_task};
use crate::tracker::{Actor, TaskEvent, TaskEventKind, IN_REVIEW_STATUS};
use std::collections::{BTreeMap, BTreeSet};

impl Store {
    /// Keyset pagination keeps each background sync pass bounded in SQLite.
    pub fn list_active_review_sync_tasks(
        &self,
        after_task_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<String>, StoreError> {
        let conn = self.connection();
        let mut statement = conn.prepare(
            "SELECT task_id FROM reviews WHERE json_extract(record, '$.mode') = 'pull_request'
             AND json_extract(record, '$.pull_request.status') IN ('open', 'changes_requested', 'approved')
             AND (?1 IS NULL OR task_id > ?1) ORDER BY task_id LIMIT ?2",
        )?;
        let rows = statement.query_map(
            params![after_task_id, limit.min(i64::MAX as usize) as i64],
            |row| row.get(0),
        )?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }

    /// Append the receiver's pinned snapshot and advance all bindings in one
    /// versioned transaction. Prior snapshots and their review facts survive.
    pub fn save_review_received_snapshot(
        &self,
        task_id: &str,
        expected_version: u64,
        mut snapshot: ReviewSnapshot,
        bindings: &[ReviewBranchBinding],
    ) -> Result<Review, StoreError> {
        self.in_transaction(|tx| {
            let mut header = require_pull_request(tx, task_id)?;
            check_version(&header, expected_version)?;
            let metadata = header.pull_request.as_ref().expect("PR metadata checked");
            if !metadata.status.is_active() {
                return Err(invalid("received snapshots require an active PR"));
            }
            validate_received_snapshot(metadata, &load_bindings(tx, task_id)?, bindings, &snapshot)?;
            if snapshot.id.is_empty() || snapshot_exists(tx, &snapshot.id)? {
                return Err(StoreError::ReviewSnapshotExists { snapshot_id: snapshot.id.clone() });
            }
            snapshot.number = next_snapshot_number(tx, task_id)?;
            header.version += 1;
            header.state = ReviewState::Open;
            header.completion = None;
            let metadata = header.pull_request.as_mut().expect("PR metadata checked");
            metadata.status = PullRequestStatus::Open;
            metadata.latest_published_snapshot_id = Some(snapshot.id.clone());
            write_header(tx, &header)?;
            tx.execute("INSERT INTO review_snapshots (id, task_id, number, record) VALUES (?1, ?2, ?3, ?4)", params![snapshot.id, task_id, snapshot.number as i64, serde_json::to_string(&snapshot).expect("snapshot serializes")])?;
            for binding in bindings {
                tx.execute("UPDATE review_branch_bindings SET record = ?3 WHERE task_id = ?1 AND directory_id = ?2", params![task_id, binding.directory_id, serde_json::to_string(binding).expect("binding serializes")])?;
            }
            move_received_task(tx, task_id)?;
            Ok(load_review(tx, task_id)?.expect("received snapshot was written"))
        })
    }
}

fn validate_received_snapshot(
    metadata: &PullRequestMetadata,
    previous: &[ReviewBranchBinding],
    bindings: &[ReviewBranchBinding],
    snapshot: &ReviewSnapshot,
) -> Result<(), StoreError> {
    let old: BTreeMap<_, _> = previous
        .iter()
        .map(|binding| (&binding.directory_id, binding_identity(binding)))
        .collect();
    let new: BTreeMap<_, _> = bindings
        .iter()
        .map(|binding| (&binding.directory_id, binding_identity(binding)))
        .collect();
    if old != new || new.len() != bindings.len() {
        return Err(invalid(
            "received bindings must preserve fixed repository and branch identities",
        ));
    }
    let expected: BTreeSet<_> = metadata
        .directories
        .iter()
        .map(|member| (&member.directory_id, &member.source_id))
        .collect();
    let found: BTreeSet<_> = snapshot
        .directories
        .iter()
        .map(|directory| (&directory.id, &directory.source_id))
        .collect();
    if expected != found || found.len() != snapshot.directories.len() {
        return Err(invalid(
            "received snapshot must preserve complete fixed membership",
        ));
    }
    for binding in bindings {
        let directory = snapshot
            .directories
            .iter()
            .find(|directory| directory.id == binding.directory_id)
            .ok_or_else(|| invalid("binding must name fixed snapshot membership"))?;
        validate_received_directory(binding, directory)?;
    }
    Ok(())
}

fn validate_received_directory(
    binding: &ReviewBranchBinding,
    directory: &ReviewDirectory,
) -> Result<(), StoreError> {
    if binding.preparation != ReviewPreparationState::Ready
        || binding.publication != ReviewPublicationState::Published
        || binding.recovery.is_some()
        || binding
            .last_received_head
            .as_ref()
            .is_none_or(|head| head.is_empty())
    {
        return Err(invalid(
            "received binding must be ready and published without recovery",
        ));
    }
    if directory.status != ReviewDirectoryStatus::Git
        || !directory.is_git
        || directory.head != binding.last_received_head
        || directory.common_git_dir.as_ref() != Some(&binding.receiving_repository)
        || directory.path != binding.working_repository
        || directory.source_path != binding.source_repository
        || directory.base.is_none()
    {
        return Err(invalid(
            "received snapshot must capture the bound receiver head and base",
        ));
    }
    Ok(())
}

fn move_received_task(tx: &Transaction, task_id: &str) -> Result<(), StoreError> {
    let mut task = require_task(tx, task_id)?;
    if task.status == IN_REVIEW_STATUS {
        return Ok(());
    }
    let now = now_rfc3339();
    let event = TaskEvent::new(
        task_id,
        Actor::Build,
        TaskEventKind::Moved,
        serde_json::json!({"from": task.status, "to": IN_REVIEW_STATUS}),
        &now,
    );
    task.status = IN_REVIEW_STATUS.into();
    task.done_at = None;
    task.updated_at = now;
    write_tracker_task(tx, &task)?;
    append_activity(tx, &[], &[event])
}

impl Store {
    /// Observations have their own revision. A sync error or pending commit
    /// cannot change review lifecycle, version, opinions or snapshot history.
    pub fn save_review_sync_observation(
        &self,
        observation: &ReviewSyncObservation,
        expected_revision: u64,
    ) -> Result<ReviewSyncObservation, StoreError> {
        self.in_transaction(|tx| {
            require_pull_request(tx, &observation.task_id)?;
            let exists: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM review_branch_bindings WHERE task_id = ?1 AND directory_id = ?2)", params![observation.task_id, observation.directory_id], |row| row.get(0))?;
            if !exists {
                return Err(invalid("sync observation must name a bound Git directory"));
            }
            let found: Option<i64> = tx.query_row("SELECT revision FROM review_sync_observations WHERE task_id = ?1 AND directory_id = ?2", params![observation.task_id, observation.directory_id], |row| row.get(0)).optional()?;
            check_operation_version(&format!("{}/{}", observation.task_id, observation.directory_id), expected_revision, found.unwrap_or(0) as u64)?;
            let mut saved = observation.clone();
            saved.revision = found.unwrap_or(0) as u64 + 1;
            tx.execute("INSERT INTO review_sync_observations (task_id, directory_id, revision, record) VALUES (?1, ?2, ?3, ?4) ON CONFLICT(task_id, directory_id) DO UPDATE SET revision = ?3, record = ?4", params![saved.task_id, saved.directory_id, saved.revision as i64, serde_json::to_string(&saved).expect("sync observation serializes")])?;
            Ok(saved)
        })
    }

    pub fn load_review_sync_observations(
        &self,
        task_id: &str,
    ) -> Result<Vec<ReviewSyncObservation>, StoreError> {
        let conn = self.connection();
        let mut statement = conn.prepare("SELECT directory_id, record FROM review_sync_observations WHERE task_id = ?1 ORDER BY directory_id")?;
        let rows = statement.query_map([task_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        rows.map(|row| {
            let (directory, raw) = row?;
            decode(
                &raw,
                "review_sync_observations",
                &format!("{task_id}/{directory}"),
            )
        })
        .collect()
    }
}

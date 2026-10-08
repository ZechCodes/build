use super::*;
use crate::reviews::records::{Review, ReviewState};
use crate::store::now_rfc3339;
use crate::store::reviews::{next_snapshot_number, snapshot_exists, write_header};
use crate::store::tracker::{append_activity, write_tracker_task};
use crate::tracker::{Actor, TaskEvent, TaskEventKind, IN_REVIEW_STATUS};
use std::collections::{BTreeMap, BTreeSet};

#[cfg(test)]
thread_local! { static SYNC_SCAN_STEPS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) }; }

impl Store {
    /// Globally unique snapshot IDs protect published pins even if a candidate
    /// journal is altered to name a snapshot belonging to another task.
    pub fn review_snapshot_is_published(&self, snapshot_id: &str) -> Result<bool, StoreError> {
        self.connection()
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM review_snapshots WHERE id = ?1)",
                [snapshot_id],
                |row| row.get(0),
            )
            .map_err(StoreError::from)
    }

    pub fn load_review_sync_candidate(&self, task_id: &str) -> Result<Option<String>, StoreError> {
        self.connection()
            .query_row(
                "SELECT snapshot_id FROM review_sync_candidates WHERE task_id = ?1",
                [task_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(StoreError::from)
    }

    /// Register before writing any receiver journal or candidate pin. A task
    /// has at most one outstanding capture, independently of its active status.
    pub fn register_review_sync_candidate(
        &self,
        task_id: &str,
        snapshot_id: &str,
    ) -> Result<(), StoreError> {
        let id = uuid::Uuid::parse_str(snapshot_id)
            .map_err(|_| invalid("sync candidate needs a canonical UUID"))?;
        if id.to_string() != snapshot_id {
            return Err(invalid("sync candidate needs a canonical UUID"));
        }
        self.in_transaction(|tx| {
            require_task(tx, task_id)?;
            require_pull_request(tx, task_id)?;
            let found: Option<String> = tx
                .query_row(
                    "SELECT snapshot_id FROM review_sync_candidates WHERE task_id = ?1",
                    [task_id],
                    |row| row.get(0),
                )
                .optional()?;
            if let Some(found) = found {
                return if found == snapshot_id {
                    Ok(())
                } else {
                    Err(invalid(
                        "sync candidate identity cannot change before recovery",
                    ))
                };
            }
            tx.execute(
                "INSERT INTO review_sync_candidates (task_id, snapshot_id) VALUES (?1, ?2)",
                params![task_id, snapshot_id],
            )?;
            Ok(())
        })
    }

    /// Remove only the registry identity whose owned journal/pins were handled.
    pub fn clear_review_sync_candidate(
        &self,
        task_id: &str,
        snapshot_id: &str,
    ) -> Result<(), StoreError> {
        self.in_transaction(|tx| {
            let found: Option<String> = tx
                .query_row(
                    "SELECT snapshot_id FROM review_sync_candidates WHERE task_id = ?1",
                    [task_id],
                    |row| row.get(0),
                )
                .optional()?;
            if found.as_deref().is_some_and(|found| found != snapshot_id) {
                return Err(invalid("sync candidate registry changed before cleanup"));
            }
            tx.execute(
                "DELETE FROM review_sync_candidates WHERE task_id = ?1 AND snapshot_id = ?2",
                params![task_id, snapshot_id],
            )?;
            Ok(())
        })
    }

    /// Sync reads only the pointed-to published snapshot and fixed bindings.
    /// Historical snapshots, actions and destinations are omitted; legacy
    /// snapshot-mode reviews have no sync state.
    pub fn load_review_sync_state(&self, task_id: &str) -> Result<Option<Review>, StoreError> {
        load_sync_state(&self.connection(), task_id)
    }

    #[cfg(test)]
    pub(crate) fn review_sync_scan_steps(&self) -> u64 {
        SYNC_SCAN_STEPS.with(std::cell::Cell::get)
    }

    /// Keyset pagination includes active PRs and any outstanding sync capture,
    /// so a concurrent close cannot hide a crashed candidate from recovery.
    pub fn list_active_review_sync_tasks(
        &self,
        after_task_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<String>, StoreError> {
        #[cfg(test)]
        SYNC_SCAN_STEPS.with(|steps| steps.set(0));
        let conn = self.connection();
        let after = after_task_id.unwrap_or("");
        let bound = limit.min(i64::MAX as usize) as i64;
        let active = sync_task_page(&conn,
            "SELECT task_id FROM reviews WHERE json_extract(record, '$.mode') = 'pull_request'
             AND json_extract(record, '$.pull_request.status') IN ('open', 'changes_requested', 'approved')
             AND task_id > ?1 ORDER BY task_id LIMIT ?2", after, bound)?;
        let pending = sync_task_page(&conn,
            "SELECT task_id FROM review_sync_candidates WHERE task_id > ?1 ORDER BY task_id LIMIT ?2",
            after, bound)?;
        Ok(active
            .into_iter()
            .chain(pending)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .take(limit)
            .collect())
    }

    /// Append the receiver's pinned snapshot and advance all bindings in one
    /// versioned transaction. Prior snapshots and their review facts survive.
    /// Returns bounded sync state containing only the latest snapshot.
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
            Ok(load_sync_state(tx, task_id)?.expect("received snapshot was written"))
        })
    }
}

fn sync_task_page(
    conn: &Connection,
    sql: &str,
    after: &str,
    limit: i64,
) -> Result<Vec<String>, StoreError> {
    let mut statement = conn.prepare(sql)?;
    let rows = statement.query_map(params![after, limit], |row| row.get(0))?;
    let tasks = rows.collect::<Result<Vec<_>, _>>()?;
    #[cfg(test)]
    SYNC_SCAN_STEPS.with(|steps| {
        steps.set(steps.get() + statement.get_status(rusqlite::StatementStatus::VmStep) as u64)
    });
    Ok(tasks)
}

fn load_sync_state(conn: &Connection, task_id: &str) -> Result<Option<Review>, StoreError> {
    let Some(header) = load_header(conn, task_id)? else {
        return Ok(None);
    };
    if header.mode != ReviewMode::PullRequest {
        return Ok(None);
    }
    let snapshot_id = header
        .pull_request
        .as_ref()
        .and_then(|metadata| metadata.latest_published_snapshot_id.as_deref())
        .ok_or_else(|| invalid("PR sync state requires a published snapshot"))?;
    let raw: Option<String> = conn
        .query_row(
            "SELECT record FROM review_snapshots WHERE id = ?1 AND task_id = ?2",
            params![snapshot_id, task_id],
            |row| row.get(0),
        )
        .optional()?;
    let snapshot: ReviewSnapshot = decode(
        &raw.ok_or_else(|| invalid("PR published snapshot is missing from its task"))?,
        "review_snapshots",
        snapshot_id,
    )?;
    if snapshot.id != snapshot_id {
        return Err(invalid(
            "PR snapshot record identity must match its pointer",
        ));
    }
    Ok(Some(Review {
        task_id: header.task_id,
        workspace_id: header.workspace_id,
        version: header.version,
        state: header.state,
        completion: header.completion,
        mode: header.mode,
        pull_request: header.pull_request,
        snapshots: vec![snapshot],
        bindings: load_bindings(conn, task_id)?,
        actions: Vec::new(),
        destinations: Vec::new(),
    }))
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

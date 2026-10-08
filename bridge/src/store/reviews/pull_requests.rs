//! Dormant PR persistence. Git preparation/recovery belongs to the services;
//! these transactions retain identity, ownership and compare-and-swap journals.

use super::{check_version, decode, load_header, require_task, ReviewHeader};
use crate::reviews::model::*;
use crate::store::{Store, StoreError};
use rusqlite::{params, Connection, OptionalExtension, Transaction};

mod merge;
mod opening;
mod sync;
pub(super) use sync::validate_received_snapshot;

impl Store {
    /// Branch allocation consults durable claims as well as refs on disk: an
    /// interrupted opening may have reserved its name before creating a ref.
    pub fn review_branch_owner(
        &self,
        repository_id: &str,
        branch_ref: &str,
    ) -> Result<Option<String>, StoreError> {
        self.connection().query_row(
            "SELECT task_id FROM review_branch_bindings WHERE repository_id = ?1 AND dedicated_ref = ?2",
            params![repository_id, branch_ref],
            |row| row.get(0),
        ).optional().map_err(StoreError::from)
    }

    /// Lightweight projection for task rows, without reading snapshots/actions.
    pub fn load_review_summary(&self, task_id: &str) -> Result<Option<ReviewSummary>, StoreError> {
        Ok(load_header(&self.connection(), task_id)?.and_then(header_summary))
    }

    /// Prefer the workspace's active owner; otherwise retain its latest PR link.
    /// A preparing opening suppresses an older terminal link until it publishes.
    pub fn load_workspace_review_summary(
        &self,
        workspace_id: &str,
    ) -> Result<Option<ReviewSummary>, StoreError> {
        let conn = self.connection();
        let task_id: Option<String> = conn.query_row(
            "SELECT COALESCE(
                (SELECT task_id FROM review_workspace_claims WHERE workspace_id = ?1),
                (SELECT reviews.task_id FROM reviews
                 JOIN tracker_tasks ON tracker_tasks.id = reviews.task_id
                 WHERE reviews.workspace_id = ?1 AND json_extract(reviews.record, '$.mode') = 'pull_request'
                 ORDER BY tracker_tasks.created_at DESC, tracker_tasks.number DESC LIMIT 1))",
            [workspace_id], |row| row.get(0),
        )?;
        task_id
            .map(|id| load_header(&conn, &id))
            .transpose()
            .map(|header| header.flatten().and_then(header_summary))
    }
}

fn header_summary(header: ReviewHeader) -> Option<ReviewSummary> {
    if header.mode != ReviewMode::PullRequest {
        return None;
    }
    header.pull_request.map(|metadata| ReviewSummary {
        task_id: header.task_id,
        workspace_id: metadata.originating_workspace_id,
        version: header.version,
        status: metadata.status,
        latest_published_snapshot_id: metadata.latest_published_snapshot_id,
    })
}

pub(super) fn load_bindings(
    conn: &Connection,
    task_id: &str,
) -> Result<Vec<ReviewBranchBinding>, StoreError> {
    let mut statement = conn.prepare(
        "SELECT directory_id, record FROM review_branch_bindings WHERE task_id = ?1 ORDER BY directory_id",
    )?;
    let rows = statement.query_map([task_id], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    rows.map(|row| {
        let (id, raw) = row?;
        decode(&raw, "review_branch_bindings", &format!("{task_id}/{id}"))
    })
    .collect()
}

pub(super) fn sync_workspace_claim(
    tx: &Transaction,
    header: &ReviewHeader,
) -> Result<(), StoreError> {
    if header.mode == ReviewMode::Snapshot {
        return Ok(());
    }
    let metadata = header
        .pull_request
        .as_ref()
        .ok_or_else(|| invalid("PR header is missing metadata"))?;
    if metadata.originating_workspace_id != header.workspace_id {
        return Err(invalid("PR workspace identity cannot change"));
    }
    if metadata.status.is_active() {
        let task = require_task(tx, &header.task_id)?;
        claim_workspace(
            tx,
            &task.project_path,
            &header.workspace_id,
            &header.task_id,
        )
    } else {
        release_workspace(tx, &header.task_id)
    }
}

fn claim_workspace(
    tx: &Transaction,
    project: &str,
    workspace: &str,
    task: &str,
) -> Result<(), StoreError> {
    let owner: Option<String> = tx
        .query_row(
            "SELECT task_id FROM review_workspace_claims WHERE workspace_id = ?1",
            [workspace],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(owner) = owner {
        if owner != task {
            return Err(StoreError::ReviewWorkspaceBusy {
                workspace_id: workspace.into(),
                task_id: owner,
            });
        }
        return Ok(());
    }
    tx.execute("INSERT INTO review_workspace_claims (workspace_id, project_key, task_id) VALUES (?1, ?2, ?3)", params![workspace, project, task])?;
    Ok(())
}

fn release_workspace(tx: &Transaction, task: &str) -> Result<(), StoreError> {
    tx.execute(
        "DELETE FROM review_workspace_claims WHERE task_id = ?1",
        [task],
    )?;
    Ok(())
}

fn invalid(message: &str) -> StoreError {
    StoreError::ReviewPullRequestInvalid(message.into())
}

fn check_operation_version(request: &str, expected: u64, found: u64) -> Result<(), StoreError> {
    if expected != found {
        return Err(StoreError::ReviewOperationVersionConflict {
            request_id: request.into(),
            expected,
            found,
        });
    }
    Ok(())
}

fn binding_identity(binding: &ReviewBranchBinding) -> ReviewBranchBinding {
    let mut identity = binding.clone();
    identity.last_received_head = None;
    identity.preparation = ReviewPreparationState::Planned;
    identity.publication = ReviewPublicationState::Pending;
    identity.recovery = None;
    identity
}

pub(super) fn require_pull_request(
    conn: &Connection,
    task_id: &str,
) -> Result<ReviewHeader, StoreError> {
    let header = load_header(conn, task_id)?.ok_or_else(|| StoreError::ReviewNotFound {
        task_id: task_id.into(),
    })?;
    if header.mode != ReviewMode::PullRequest || header.pull_request.is_none() {
        return Err(invalid("operation requires a PR-mode review"));
    }
    Ok(header)
}

use super::*;
use crate::store::now_rfc3339;
use std::collections::BTreeSet;

impl Store {
    /// Admit one immutable merge vector. Retry identity is checked before the
    /// current review version, so a lost response never admits another merge.
    pub fn reserve_review_merge(
        &self,
        project_path: &str,
        request_id: &str,
        request: ReviewMergeRequest,
    ) -> Result<ReviewMergeIntent, StoreError> {
        self.in_transaction(|tx| {
            if let Some(previous) = load_intent(tx, project_path, request_id)? {
                if previous.request != request {
                    return Err(StoreError::ReviewRequestConflict {
                        request_id: request_id.into(),
                    });
                }
                return Ok(previous);
            }
            validate_merge(tx, project_path, request_id, &request)?;
            let now = now_rfc3339();
            let intent = ReviewMergeIntent {
                project_path: project_path.into(),
                request_id: request_id.into(),
                version: 1,
                request,
                state: ReviewMergeState::Running,
                execution_version: None,
                action_ids: Vec::new(),
                created_at: now.clone(),
                updated_at: now,
                error: None,
            };
            write_intent(tx, &intent)?;
            Ok(intent)
        })
    }

    pub fn load_review_merge_intent(
        &self,
        project_path: &str,
        request_id: &str,
    ) -> Result<Option<ReviewMergeIntent>, StoreError> {
        load_intent(&self.connection(), project_path, request_id)
    }

    /// Store references to durable per-source action results. Saving success
    /// here alone never asserts that the PR lifecycle is Merged.
    pub fn save_review_merge_intent(
        &self,
        intent: &ReviewMergeIntent,
        expected_version: u64,
    ) -> Result<ReviewMergeIntent, StoreError> {
        self.in_transaction(|tx| {
            let previous = load_intent(tx, &intent.project_path, &intent.request_id)?
                .ok_or_else(|| invalid("unknown merge request"))?;
            check_operation_version(&intent.request_id, expected_version, previous.version)?;
            if previous.request != intent.request || previous.created_at != intent.created_at {
                return Err(invalid("merge intent identity cannot change"));
            }
            if previous.state == ReviewMergeState::Succeeded {
                return Err(invalid("successful merge intent cannot be changed"));
            }
            if !intent.action_ids.starts_with(&previous.action_ids) {
                return Err(invalid("merge result references must be retained in order"));
            }
            let mut saved = intent.clone();
            saved.version = previous.version + 1;
            saved.updated_at = now_rfc3339();
            write_intent(tx, &saved)?;
            Ok(saved)
        })
    }

    /// Startup recovery enumerates uncertain admitted operations; opening the
    /// store itself never replays Git or changes their state.
    pub fn load_running_review_merge_intents(&self) -> Result<Vec<ReviewMergeIntent>, StoreError> {
        let conn = self.connection();
        let mut statement = conn.prepare("SELECT project_key, request_id, record FROM review_merge_intents WHERE state = 'running' ORDER BY project_key, request_id")?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        rows.map(|row| {
            let (project, request, raw) = row?;
            decode(
                &raw,
                "review_merge_intents",
                &format!("{project}/{request}"),
            )
        })
        .collect()
    }
}

fn validate_merge(
    tx: &Transaction,
    project: &str,
    request_id: &str,
    request: &ReviewMergeRequest,
) -> Result<(), StoreError> {
    if request_id.is_empty() || require_task(tx, &request.task_id)?.project_path != project {
        return Err(invalid("merge request must belong to the task's project"));
    }
    let header = require_pull_request(tx, &request.task_id)?;
    check_version(&header, request.expected_version)?;
    let metadata = header.pull_request.expect("PR metadata checked");
    if !metadata.status.is_active()
        || metadata.latest_published_snapshot_id.as_deref() != Some(&request.snapshot_id)
    {
        return Err(invalid(
            "merge requires the latest active published PR snapshot",
        ));
    }
    let running: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM review_merge_intents WHERE task_id = ?1 AND state = 'running')", [&request.task_id], |row| row.get(0))?;
    if running {
        return Err(invalid("a merge is already running for this PR"));
    }
    validate_sources(tx, request)
}

fn validate_sources(conn: &Connection, request: &ReviewMergeRequest) -> Result<(), StoreError> {
    let bindings = load_bindings(conn, &request.task_id)?;
    if request.sources.len() != bindings.len() || bindings.is_empty() {
        return Err(invalid("merge intent must name every included Git binding"));
    }
    let raw: String = conn.query_row(
        "SELECT record FROM review_snapshots WHERE task_id = ?1 AND id = ?2",
        params![request.task_id, request.snapshot_id],
        |row| row.get(0),
    )?;
    let snapshot: ReviewSnapshot = decode(&raw, "review_snapshots", &request.snapshot_id)?;
    let mut ids = BTreeSet::new();
    for source in &request.sources {
        if !ids.insert(&source.directory_id) {
            return Err(invalid("duplicate merge directory"));
        }
        validate_source(source, &bindings, &snapshot)?;
    }
    Ok(())
}

fn validate_source(
    source: &ReviewMergeSource,
    bindings: &[ReviewBranchBinding],
    snapshot: &ReviewSnapshot,
) -> Result<(), StoreError> {
    let binding = bindings
        .iter()
        .find(|binding| binding.directory_id == source.directory_id)
        .ok_or_else(|| invalid("unknown merge directory"))?;
    let directory = snapshot
        .directories
        .iter()
        .find(|directory| directory.id == source.directory_id)
        .ok_or_else(|| invalid("merge directory is missing from the snapshot"))?;
    if binding.repository_id != source.repository_id
        || binding.base_branch_ref != source.base_branch_ref
        || binding.last_received_head.as_deref() != Some(&source.head)
        || directory.head.as_deref() != Some(&source.head)
        || source.expected_base_head.is_empty()
    {
        return Err(invalid(
            "merge vector differs from the published repository/head/base bindings",
        ));
    }
    Ok(())
}

pub(super) fn load_intent(
    conn: &Connection,
    project: &str,
    request: &str,
) -> Result<Option<ReviewMergeIntent>, StoreError> {
    let raw: Option<String> = conn
        .query_row(
            "SELECT record FROM review_merge_intents WHERE project_key = ?1 AND request_id = ?2",
            params![project, request],
            |row| row.get(0),
        )
        .optional()?;
    raw.map(|raw| {
        decode(
            &raw,
            "review_merge_intents",
            &format!("{project}/{request}"),
        )
    })
    .transpose()
}

pub(super) fn write_intent(tx: &Transaction, intent: &ReviewMergeIntent) -> Result<(), StoreError> {
    let state = match intent.state {
        ReviewMergeState::Running => "running",
        ReviewMergeState::Succeeded => "succeeded",
        ReviewMergeState::Failed => "failed",
        ReviewMergeState::Interrupted => "interrupted",
    };
    tx.execute("INSERT INTO review_merge_intents (project_key, request_id, task_id, version, state, record) VALUES (?1, ?2, ?3, ?4, ?5, ?6) ON CONFLICT(project_key, request_id) DO UPDATE SET version = ?4, state = ?5, record = ?6", params![intent.project_path, intent.request_id, intent.request.task_id, intent.version as i64, state, serde_json::to_string(intent).expect("merge intent serializes")])?;
    Ok(())
}

mod execution;

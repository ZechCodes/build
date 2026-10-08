//! Shared lifecycle writes: review facts, task column and activity are atomic.

use super::*;
use crate::reviews::model::ReviewBranchBinding;
use crate::tracker::TaskState;

impl Store {
    /// Reopening and a fresh published snapshot are one compare-and-swap. The
    /// service proves receiver refs and recoverable placement before entry.
    pub(crate) fn reopen_review_received_snapshot(
        &self,
        task_id: &str,
        expected_version: u64,
        mut snapshot: ReviewSnapshot,
        bindings: &[ReviewBranchBinding],
        actor: &Actor,
    ) -> Result<Review, StoreError> {
        self.in_transaction(|tx| {
            let mut header = pull_requests::require_pull_request(tx, task_id)?;
            check_version(&header, expected_version)?;
            let metadata = header.pull_request.as_ref().expect("PR metadata checked");
            if metadata.status != PullRequestStatus::Closed {
                return Err(invalid("only Closed unmerged PRs can reopen"));
            }
            pull_requests::validate_received_snapshot(metadata, &pull_requests::load_bindings(tx, task_id)?, bindings, &snapshot)?;
            if snapshot.id.is_empty() || snapshot_exists(tx, &snapshot.id)? {
                return Err(StoreError::ReviewSnapshotExists { snapshot_id: snapshot.id });
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
            reopen_task(tx, task_id, actor, &snapshot.id)?;
            Ok(load_review(tx, task_id)?.expect("PR was reopened"))
        })
    }

    /// Tracker surfaces capture a review version while validating an opinion,
    /// then fence that observation in the transaction that writes its comment.
    pub fn save_review_task_activity(
        &self,
        task: &Task,
        comments: &[TaskComment],
        events: &[TaskEvent],
        actor: &Actor,
        expected_review_version: Option<u64>,
        now: &str,
    ) -> Result<(Task, Vec<TaskEvent>), StoreError> {
        self.in_transaction(|tx| {
            save_activity_in_tx(
                tx,
                task,
                comments,
                events,
                actor,
                expected_review_version,
                now,
            )
        })
    }

    /// An explicit current-snapshot opinion has both snapshot and version
    /// preconditions; older-snapshot task comments remain historical context.
    pub fn record_review_opinion(
        &self,
        task_id: &str,
        expected_version: u64,
        comment: &TaskComment,
    ) -> Result<Review, StoreError> {
        self.in_transaction(|tx| {
            let task = require_task(tx, task_id)?;
            let header = pull_requests::require_pull_request(tx, task_id)?;
            check_version(&header, expected_version)?;
            let opinion = comment
                .opinion
                .as_ref()
                .ok_or_else(|| invalid("opinion comment is missing its verdict"))?;
            let metadata = header.pull_request.as_ref().expect("PR metadata checked");
            if comment.task_id != task_id
                || metadata.latest_published_snapshot_id.as_deref() != Some(&opinion.snapshot_id)
            {
                return Err(invalid("opinion requires the latest published PR snapshot"));
            }
            save_activity_in_tx(
                tx,
                &task,
                std::slice::from_ref(comment),
                &[],
                &comment.author,
                Some(expected_version),
                &comment.created_at,
            )?;
            Ok(load_review(tx, task_id)?.expect("review was written"))
        })
    }
}

fn reopen_task(
    tx: &Transaction,
    task_id: &str,
    actor: &Actor,
    snapshot_id: &str,
) -> Result<(), StoreError> {
    let mut task = require_task(tx, task_id)?;
    let now = now_rfc3339();
    let event = TaskEvent::new(
        task_id,
        actor.clone(),
        TaskEventKind::Reopened,
        json!({"by":"review", "snapshot_id":snapshot_id}),
        &now,
    );
    let moved = move_column(&mut task, PullRequestStatus::Open, actor, &now);
    task.state = TaskState::Open;
    task.closed_at = None;
    task.done_at = None;
    task.updated_at = now;
    write_tracker_task(tx, &task)?;
    let mut events = vec![event];
    events.extend(moved);
    append_activity(tx, &[], &events)
}

pub(in crate::store) fn save_activity_in_tx(
    tx: &Transaction,
    task: &Task,
    comments: &[TaskComment],
    events: &[TaskEvent],
    actor: &Actor,
    expected_review_version: Option<u64>,
    now: &str,
) -> Result<(Task, Vec<TaskEvent>), StoreError> {
    let task = preserve_unmoved_lifecycle(tx, task, events)?;
    let header = load_header(tx, &task.id)?;
    if let Some(header) = &header {
        if let Some(expected) = expected_review_version {
            check_version(header, expected)?;
        }
        if header.mode == ReviewMode::PullRequest
            && events
                .iter()
                .any(|event| event.kind == TaskEventKind::Reopened)
        {
            return Err(invalid("reopen a closed PR through the review lifecycle service; its original workspace and bindings must be recoverable"));
        }
        if should_complete(header, events) {
            let description = if events
                .iter()
                .any(|event| event.kind == TaskEventKind::Closed)
            {
                "Task closed"
            } else {
                "Marked done"
            };
            let (_, additional) = complete_review_in_tx(
                tx,
                CompletionWrite {
                    task: &task,
                    comments,
                    events,
                    actor,
                    expected_version: expected_review_version,
                    description,
                    now,
                    terminal_status: None,
                },
            )?;
            return Ok((require_task(tx, &task.id)?, additional));
        }
    }
    let mut saved = task.clone();
    let additional = apply_opinions(tx, &mut saved, comments, header, now)?;
    write_tracker_task(tx, &saved)?;
    append_activity(tx, comments, events)?;
    append_activity(tx, &[], &additional)?;
    Ok((saved, additional))
}

fn preserve_unmoved_lifecycle(
    tx: &Transaction,
    task: &Task,
    events: &[TaskEvent],
) -> Result<Task, StoreError> {
    let current = require_task(tx, &task.id)?;
    let mut saved = task.clone();
    if !events
        .iter()
        .any(|event| event.kind == TaskEventKind::Moved)
    {
        saved.status = current.status;
        saved.done_at = current.done_at;
    }
    if !events
        .iter()
        .any(|event| matches!(event.kind, TaskEventKind::Closed | TaskEventKind::Reopened))
    {
        saved.state = current.state;
        saved.closed_at = current.closed_at;
    }
    Ok(saved)
}

fn should_complete(header: &ReviewHeader, events: &[TaskEvent]) -> bool {
    if header.state != ReviewState::Open {
        return false;
    }
    events.iter().any(|event| {
        (event.kind == TaskEventKind::Closed && header.mode == ReviewMode::PullRequest)
            || (event.kind == TaskEventKind::Moved
                && event.payload.get("to").and_then(serde_json::Value::as_str) == Some(DONE_STATUS))
    })
}

fn apply_opinions(
    tx: &Transaction,
    task: &mut Task,
    comments: &[TaskComment],
    header: Option<ReviewHeader>,
    now: &str,
) -> Result<Vec<TaskEvent>, StoreError> {
    let Some(mut header) = header.filter(|header| header.mode == ReviewMode::PullRequest) else {
        return Ok(vec![]);
    };
    let metadata = header.pull_request.as_ref().expect("PR metadata checked");
    let snapshot_id = metadata
        .latest_published_snapshot_id
        .as_deref()
        .ok_or_else(|| invalid("PR has no published snapshot"))?;
    let current = new_current_opinions(tx, &task.id, snapshot_id, comments)?;
    if !current {
        return Ok(vec![]);
    }
    if !metadata.status.is_active() {
        return Err(invalid("current opinions require an active PR"));
    }
    append_activity(tx, comments, &[])?;
    let status = crate::reviews::lifecycle::opinion_status(
        snapshot_id,
        &load_opinions(tx, &task.id, snapshot_id)?,
    );
    header.version += 1;
    header
        .pull_request
        .as_mut()
        .expect("PR metadata checked")
        .status = status;
    write_header(tx, &header)?;
    Ok(move_column(
        task,
        status,
        &comments.last().expect("current opinion exists").author,
        now,
    )
    .into_iter()
    .collect())
}

fn new_current_opinions(
    tx: &Transaction,
    task_id: &str,
    current: &str,
    comments: &[TaskComment],
) -> Result<bool, StoreError> {
    let mut found = false;
    for comment in comments {
        let Some(opinion) = &comment.opinion else {
            continue;
        };
        let snapshot: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM review_snapshots WHERE task_id = ?1 AND id = ?2)",
            params![task_id, opinion.snapshot_id],
            |row| row.get(0),
        )?;
        if comment.task_id != task_id || !snapshot {
            return Err(invalid("opinion must name a snapshot of its task"));
        }
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM tracker_comments WHERE id = ?1)",
            [&comment.id],
            |row| row.get(0),
        )?;
        found |= opinion.snapshot_id == current && !exists;
    }
    Ok(found)
}

fn load_opinions(
    tx: &Transaction,
    task_id: &str,
    snapshot_id: &str,
) -> Result<Vec<TaskComment>, StoreError> {
    let mut statement = tx.prepare("SELECT id, record FROM tracker_comments WHERE task_id = ?1 AND json_extract(record, '$.opinion.snapshot_id') = ?2 ORDER BY created_at, id")?;
    let rows = statement.query_map(params![task_id, snapshot_id], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    rows.map(|row| {
        let (id, raw) = row?;
        decode(&raw, "tracker_comments", &id)
    })
    .collect()
}

fn move_column(
    task: &mut Task,
    status: PullRequestStatus,
    actor: &Actor,
    now: &str,
) -> Option<TaskEvent> {
    let column = crate::reviews::lifecycle::column(status);
    if task.status == column {
        return None;
    }
    let from = std::mem::replace(&mut task.status, column.into());
    task.done_at = (column == DONE_STATUS).then(|| now.into());
    task.updated_at = now.into();
    Some(TaskEvent::new(
        &task.id,
        actor.clone(),
        TaskEventKind::Moved,
        json!({"from": from, "to": column, "by": "review"}),
        now,
    ))
}

/// The merge writer validates its intent, recorded results and version in the
/// same transaction before entering this terminal writer under receiver locks.
pub(crate) fn transition_in_tx(
    tx: &Transaction,
    task_id: &str,
    status: PullRequestStatus,
    actor: &Actor,
    description: &str,
) -> Result<(Review, Vec<TaskEvent>), StoreError> {
    if !matches!(
        status,
        PullRequestStatus::Merged | PullRequestStatus::Closed
    ) {
        return Err(invalid("terminal transition requires Merged or Closed"));
    }
    pull_requests::require_pull_request(tx, task_id)?;
    let task = require_task(tx, task_id)?;
    let now = now_rfc3339();
    complete_review_in_tx(
        tx,
        CompletionWrite {
            task: &task,
            comments: &[],
            events: &[],
            actor,
            expected_version: None,
            description: validate_description(description)?,
            now: &now,
            terminal_status: Some(status),
        },
    )
}

fn invalid(message: &str) -> StoreError {
    StoreError::ReviewPullRequestInvalid(message.into())
}

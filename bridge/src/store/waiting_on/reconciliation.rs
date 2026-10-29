use super::{changed, invalid, reads, require_task, WaitingOnPage};
use crate::store::{tracker::append_activity, StoreError};
use crate::tracker::{Actor, Task, TaskEvent, TaskEventKind, DONE_STATUS};
use rusqlite::{params, OptionalExtension, Transaction};

/// Called by the common task writer, including review and workspace paths.
/// A marker, record and cause timeline share the caller's IMMEDIATE transaction.
pub(in crate::store) fn note_task_write(tx: &Transaction, task: &Task) -> Result<(), StoreError> {
    let before: Option<(String, i64, String, String, String)> = tx.query_row(
        "SELECT project_key, number, state, status, json_extract(record, '$.title') FROM tracker_tasks WHERE id = ?1",
        [&task.id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
    ).optional()?;
    let Some((project, number, state, status, title)) = before else {
        return Ok(());
    };
    if project != task.project_path || number != task.number as i64 {
        return Err(invalid("task project and number are immutable"));
    }
    let forward = reads::forward(tx, &task.id)?;
    let unresolved = forward.iter().any(|d| !d.resolved);
    if status != task.status
        && unresolved
        && !task.is_finished()
        && matches!(task.status.as_str(), "in_progress" | "in_review")
    {
        return Err(StoreError::WaitingOnConflict);
    }
    if state == task.state.as_str() && status == task.status && title == task.title {
        return Ok(());
    }
    invalidate_reverse_blocks(tx, &forward)?;
    let affected: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM tracker_task_dependencies WHERE task_id = ?1)
        OR EXISTS(SELECT 1 FROM tracker_task_dependencies WHERE waiting_on_task_id = ?1)",
        [&task.id],
        |r| r.get(0),
    )?;
    if affected {
        let resolution_changed = (state == "closed" || status == DONE_STATUS) != task.is_finished();
        tx.execute(
            "INSERT INTO tracker_dependency_reconcile(source_task_id, at, resolution_changed) VALUES (?1, ?2, ?3)",
            params![task.id, task.updated_at, resolution_changed],
        )?;
    }
    Ok(())
}

/// A dependent's digest appears in each prerequisite's reverse Blocks list.
/// Forward edges are bounded at twenty and share this task write's transaction.
fn invalidate_reverse_blocks(
    tx: &Transaction,
    forward: &[super::DependencyDigest],
) -> Result<(), StoreError> {
    for edge in forward {
        changed(tx, &edge.task_id)?;
    }
    Ok(())
}

/// Record causes only for a write that changed prerequisite resolution.
/// Later moves/closes that leave it resolved cannot replace its release.
/// Additional review events append to the same recorded transition.
pub(in crate::store) fn note_activity(
    tx: &Transaction,
    events: &[TaskEvent],
) -> Result<(), StoreError> {
    for event in events.iter().filter(|e| {
        matches!(
            e.kind,
            TaskEventKind::Moved
                | TaskEventKind::Closed
                | TaskEventKind::Reopened
                | TaskEventKind::ReviewCompleted
        )
    }) {
        let marker: Option<(i64, String, bool)> = tx.query_row(
            "SELECT id, cause_event_ids, resolution_changed FROM tracker_dependency_reconcile WHERE source_task_id = ?1 AND at = ?2 AND self_done = 0 ORDER BY id DESC LIMIT 1",
            params![event.task_id, event.at], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        ).optional()?;
        if let Some((id, raw, true)) = marker {
            let mut ids: Vec<String> = super::super::tracker::decode(
                &raw,
                "tracker_dependency_reconcile",
                &id.to_string(),
            )?;
            if !ids.contains(&event.id) {
                ids.push(event.id.clone());
            }
            tx.execute(
                "UPDATE tracker_dependency_reconcile SET cause_event_ids = ?1 WHERE id = ?2",
                params![serde_json::to_string(&ids).expect("ids serialize"), id],
            )?;
        }
    }
    Ok(())
}

pub(super) fn remember_removed(
    tx: &Transaction,
    id: &str,
    removed: &[String],
) -> Result<(), StoreError> {
    let (blocked, mut previous) = readiness(tx, id)?;
    if !blocked {
        return Ok(());
    }
    for id in removed {
        if !previous.contains(id) {
            previous.push(id.clone());
        }
    }
    save_readiness(tx, id, blocked, &previous)
}

fn readiness(tx: &Transaction, id: &str) -> Result<(bool, Vec<String>), StoreError> {
    let row: Option<(bool, String)> = tx
        .query_row(
            "SELECT blocked, removed_task_ids FROM tracker_dependency_readiness WHERE task_id = ?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let Some((blocked, raw)) = row else {
        return Ok((false, Vec::new()));
    };
    Ok((
        blocked,
        super::super::tracker::decode(&raw, "tracker_dependency_readiness", id)?,
    ))
}

fn save_readiness(
    tx: &Transaction,
    id: &str,
    blocked: bool,
    removed: &[String],
) -> Result<(), StoreError> {
    tx.execute("INSERT INTO tracker_dependency_readiness(task_id, blocked, removed_task_ids) VALUES (?1, ?2, ?3)
        ON CONFLICT(task_id) DO UPDATE SET blocked = ?2, removed_task_ids = ?3", params![id, blocked, serde_json::to_string(removed).expect("ids serialize")])?;
    Ok(())
}

pub(super) fn reconcile_task(
    tx: &Transaction,
    task: &Task,
    causes: &[String],
    now: &str,
) -> Result<Option<TaskEvent>, StoreError> {
    let forward = reads::forward(tx, &task.id)?;
    let blocked = !task.is_finished() && forward.iter().any(|d| !d.resolved);
    let (before, removed) = readiness(tx, &task.id)?;
    save_readiness(tx, &task.id, blocked, if blocked { &removed } else { &[] })?;
    if before == blocked || task.is_finished() {
        super::super::delivery_intents::reconcile_task(tx, task, blocked, None)?;
        return Ok(None);
    }
    let causes = current_causes(tx, &forward, causes)?;
    let event = transition(task, blocked, &forward, &removed, &causes, now);
    append_activity(tx, &[], std::slice::from_ref(&event))?;
    tx.execute(
        "INSERT INTO tracker_dependency_outbox(event_id, task_id) VALUES (?1, ?2)",
        params![event.id, task.id],
    )?;
    tx.execute("UPDATE tracker_tasks SET updated_at = ?1, record = json_set(record, '$.updated_at', ?1) WHERE id = ?2", params![now, task.id])?;
    crate::store::task_summaries::task_refresh(tx, &task.id)?;
    changed(tx, &task.id)?;
    super::super::delivery_intents::reconcile_task(tx, task, blocked, Some(&event))?;
    Ok(Some(event))
}

fn transition(
    task: &Task,
    blocked: bool,
    forward: &[super::DependencyDigest],
    removed: &[String],
    causes: &[String],
    now: &str,
) -> TaskEvent {
    let (kind, payload) = if blocked {
        let unresolved: Vec<_> = forward
            .iter()
            .filter(|d| !d.resolved)
            .map(|d| &d.task_id)
            .collect();
        (
            TaskEventKind::Blocked,
            serde_json::json!({"unresolved_task_ids":unresolved,"cause_event_ids":causes}),
        )
    } else {
        let removed: Vec<_> = removed
            .iter()
            .filter(|id| !forward.iter().any(|d| &d.task_id == *id))
            .collect();
        let resolved: Vec<_> = forward
            .iter()
            .filter(|d| d.resolved)
            .map(|d| &d.task_id)
            .collect();
        let closed: Vec<_> = forward
            .iter()
            .filter(|d| d.resolved_by.as_deref() == Some("close"))
            .map(|d| &d.task_id)
            .collect();
        (
            TaskEventKind::Unblocked,
            serde_json::json!({"resolved_task_ids":resolved,"closed_without_done_task_ids":closed,"removed_task_ids":removed,"cause_event_ids":causes}),
        )
    };
    TaskEvent::new(&task.id, Actor::Build, kind, payload, now)
}

/// A live readiness check may see a later prerequisite write before its
/// queued page runs. Preserve that write's cause too, rather than attributing
/// a fan-in release solely to the first marker the service happened to drain.
/// Each of the at most twenty edges seeks its recorded resolution transition
/// through a partial index; later writes that leave it resolved cannot hide it.
fn current_causes(
    tx: &Transaction,
    forward: &[super::DependencyDigest],
    causes: &[String],
) -> Result<Vec<String>, StoreError> {
    let mut result = causes.to_vec();
    for edge in forward {
        let raw: Option<String> = tx.query_row(
            "SELECT cause_event_ids FROM tracker_dependency_reconcile WHERE source_task_id = ?1 AND fanout = 1 AND resolution_changed = 1 ORDER BY id DESC LIMIT 1",
            [&edge.task_id], |r| r.get(0),
        ).optional()?;
        if let Some(raw) = raw {
            let pending: Vec<String> =
                super::super::tracker::decode(&raw, "tracker_dependency_reconcile", &edge.task_id)?;
            for id in pending {
                if !result.contains(&id) {
                    result.push(id);
                }
            }
        }
    }
    Ok(result)
}

struct Marker {
    id: i64,
    source: String,
    after: u64,
    self_done: bool,
    fanout: bool,
    causes: Vec<String>,
}

fn first_marker(tx: &Transaction) -> Result<Option<Marker>, StoreError> {
    let row = tx.query_row("SELECT id, source_task_id, after_number, self_done, cause_event_ids, fanout FROM tracker_dependency_reconcile ORDER BY id LIMIT 1", [], |r| {
        Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)? as u64, r.get::<_, bool>(3)?, r.get::<_, String>(4)?, r.get::<_, bool>(5)?))
    }).optional()?;
    row.map(|(id, source, after, self_done, raw, fanout)| {
        Ok(Marker {
            id,
            source,
            after,
            self_done,
            fanout,
            causes: super::super::tracker::decode(
                &raw,
                "tracker_dependency_reconcile",
                &id.to_string(),
            )?,
        })
    })
    .transpose()
}

pub(super) fn page(tx: &Transaction, limit: usize) -> Result<WaitingOnPage, StoreError> {
    let Some(marker) = first_marker(tx)? else {
        return Ok(WaitingOnPage::default());
    };
    let project: String = tx.query_row(
        "SELECT project_key FROM tracker_tasks WHERE id = ?1",
        [&marker.source],
        |r| r.get(0),
    )?;
    let mut page = WaitingOnPage::default();
    let now = crate::store::now_rfc3339();
    if !marker.self_done {
        let task = require_task(tx, &project, &marker.source)?;
        page.transitions
            .extend(reconcile_task(tx, &task, &marker.causes, &now)?);
        changed(tx, &task.id)?;
        tx.execute(
            "UPDATE tracker_dependency_reconcile SET self_done = 1 WHERE id = ?1",
            [marker.id],
        )?;
    }
    let tasks = if marker.fanout {
        dependents(tx, &project, &marker, limit)?
    } else {
        Vec::new()
    };
    for task in tasks.iter().take(limit) {
        page.transitions
            .extend(reconcile_task(tx, task, &marker.causes, &now)?);
        // Title/state/status edits invalidate digests even without a transition.
        changed(tx, &task.id)?;
    }
    if tasks.len() > limit {
        tx.execute(
            "UPDATE tracker_dependency_reconcile SET after_number = ?1 WHERE id = ?2",
            params![tasks[limit - 1].number as i64, marker.id],
        )?;
    } else {
        tx.execute(
            "DELETE FROM tracker_dependency_reconcile WHERE id = ?1",
            [marker.id],
        )?;
    }
    page.pending = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM tracker_dependency_reconcile)",
        [],
        |r| r.get(0),
    )?;
    Ok(page)
}

fn dependents(
    tx: &Transaction,
    project: &str,
    marker: &Marker,
    limit: usize,
) -> Result<Vec<Task>, StoreError> {
    let mut statement = tx.prepare("SELECT t.id, t.record FROM tracker_task_dependencies d JOIN tracker_tasks t ON t.id = d.task_id
        WHERE d.waiting_on_task_id = ?1 AND d.task_number > ?2 AND t.project_key = ?3 ORDER BY d.task_number LIMIT ?4")?;
    let rows = statement.query_map(
        params![
            marker.source,
            marker.after as i64,
            project,
            (limit + 1) as i64
        ],
        |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
    )?;
    rows.map(|r| {
        let (id, raw) = r?;
        super::super::tracker::decode(&raw, "tracker_tasks", &id)
    })
    .collect()
}

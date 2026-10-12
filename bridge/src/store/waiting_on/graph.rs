use super::{changed, invalid, reconciliation, require_task};
use crate::store::{tracker::append_activity, StoreError};
use crate::tracker::{Actor, Task, TaskEvent, TaskEventKind, MAX_LINKS_PER_KIND};
use rusqlite::{params, Transaction};

pub(super) fn edit(
    tx: &Transaction,
    task: &Task,
    add: &[String],
    remove: &[String],
    actor: &Actor,
    now: &str,
) -> Result<(), StoreError> {
    validate_endpoints(tx, task, add, remove)?;
    let mut events = Vec::new();
    let mut removed = Vec::new();
    for id in remove {
        if tx.execute(
            "DELETE FROM tracker_task_dependencies WHERE task_id = ?1 AND waiting_on_task_id = ?2",
            params![task.id, id],
        )? != 0
        {
            removed.push(id.clone());
            events.push(edge_event(task, id, TaskEventKind::Unlinked, actor, now));
            changed(tx, id)?;
        }
    }
    add_edges(tx, task, add, actor, now, &mut events)?;
    if events.is_empty() {
        return Ok(());
    }
    append_activity(tx, &[], &events)?;
    reconciliation::remember_removed(tx, &task.id, &removed)?;
    let causes = events.iter().map(|e| e.id.clone()).collect::<Vec<_>>();
    reconciliation::reconcile_task(tx, task, &causes, now)?;
    tx.execute("INSERT INTO tracker_dependency_reconcile(source_task_id, self_done, fanout, at, cause_event_ids) VALUES (?1, 1, 0, ?2, ?3)",
        params![task.id, now, serde_json::to_string(&causes).expect("ids serialize")])?;
    changed(tx, &task.id)?;
    // Reverse counts and summaries of every edited blocker invalidate too.
    tx.execute("UPDATE tracker_tasks SET updated_at = ?1, record = json_set(record, '$.updated_at', ?1) WHERE id = ?2", params![now, task.id])?;
    Ok(())
}

fn validate_endpoints(
    tx: &Transaction,
    task: &Task,
    add: &[String],
    remove: &[String],
) -> Result<(), StoreError> {
    if add.len() > 100 || remove.len() > 100 {
        return Err(invalid("dependency batch exceeds 100 entries"));
    }
    for id in add.iter().chain(remove) {
        if *id == task.id {
            return Err(invalid("a task cannot wait on itself"));
        }
        require_task(tx, &task.project_path, id)?;
    }
    if add.iter().any(|id| remove.contains(id)) {
        return Err(invalid("an edge cannot be added and removed in one edit"));
    }
    Ok(())
}

fn add_edges(
    tx: &Transaction,
    task: &Task,
    ids: &[String],
    actor: &Actor,
    now: &str,
    events: &mut Vec<TaskEvent>,
) -> Result<(), StoreError> {
    for id in ids {
        let exists: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM tracker_task_dependencies WHERE task_id = ?1 AND waiting_on_task_id = ?2)", params![task.id, id], |r| r.get(0))?;
        if exists {
            continue;
        }
        check_new_edge(tx, task, id)?;
        tx.execute("INSERT INTO tracker_task_dependencies(task_id, waiting_on_task_id, position, task_number)
            SELECT ?1, ?2, COALESCE(MAX(position), 0) + 1, ?3 FROM tracker_task_dependencies WHERE task_id = ?1", params![task.id, id, task.number as i64])?;
        events.push(edge_event(task, id, TaskEventKind::Linked, actor, now));
        changed(tx, id)?;
    }
    Ok(())
}

fn check_new_edge(tx: &Transaction, task: &Task, id: &str) -> Result<(), StoreError> {
    let count: i64 = tx.query_row(
        "SELECT COUNT(*) FROM tracker_task_dependencies WHERE task_id = ?1",
        [&task.id],
        |r| r.get(0),
    )?;
    if count >= MAX_LINKS_PER_KIND as i64 {
        return Err(invalid("waiting_on exceeds 20 distinct tasks"));
    }
    let blocker = require_task(tx, &task.project_path, id)?;
    if !blocker.is_finished()
        && (task.is_finished() || matches!(task.status.as_str(), "in_progress" | "in_review"))
    {
        return Err(StoreError::WaitingOnConflict);
    }
    let cycle: bool = tx.query_row(
        "WITH RECURSIVE reachable(id) AS (
            SELECT ?1 UNION SELECT d.waiting_on_task_id FROM tracker_task_dependencies d JOIN reachable r ON d.task_id = r.id
         ) SELECT EXISTS(SELECT 1 FROM reachable WHERE id = ?2)",
        params![id, task.id], |r| r.get(0),
    )?;
    if cycle {
        return Err(invalid("waiting_on would create a cycle"));
    }
    // Use all retained edges, even those currently resolved.
    Ok(())
}

fn edge_event(task: &Task, id: &str, kind: TaskEventKind, actor: &Actor, now: &str) -> TaskEvent {
    TaskEvent::new(
        &task.id,
        actor.clone(),
        kind,
        serde_json::json!({"waiting_on":id}),
        now,
    )
}

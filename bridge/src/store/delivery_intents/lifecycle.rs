use super::*;
use crate::tracker::{TaskEvent, TaskEventKind};
use rusqlite::Transaction;

pub(in crate::store) fn cancel_task(tx: &Transaction, task_id: &str) -> Result<(), StoreError> {
    if let Some(mut intent) = current(tx, task_id)? {
        intent.state = "cancelled".into();
        write(tx, &intent)?;
    }
    Ok(())
}

pub(in crate::store) fn note_task_write(tx: &Transaction, after: &Task) -> Result<(), StoreError> {
    let Some(intent) = current(tx, &after.id)? else {
        return Ok(());
    };
    let Some(before) = task(tx, &intent)? else {
        return Ok(());
    };
    let returned_to_ready = after.status == "ready" && matches!(before.status.as_str(), "in_progress" | "in_review");
    if after.is_finished() || after.assignee != before.assignee || returned_to_ready {
        cancel_task(tx, &after.id)?;
    }
    Ok(())
}

/// Assignment events retire authority but can never create it. The explicit
/// assignment API appends its events before inserting the replacement intent.
pub(in crate::store) fn note_activity(
    tx: &Transaction,
    events: &[TaskEvent],
) -> Result<(), StoreError> {
    for event in events
        .iter()
        .filter(|e| matches!(e.kind, TaskEventKind::Assigned | TaskEventKind::Unassigned))
    {
        cancel_task(tx, &event.task_id)?;
    }
    Ok(())
}

pub(in crate::store) fn reconcile_task(
    tx: &Transaction,
    task: &Task,
    blocked: bool,
    transition: Option<&TaskEvent>,
) -> Result<(), StoreError> {
    let current = current(tx, &task.id)?;
    if let Some(mut intent) = current.as_ref().cloned() {
        if task.is_finished() {
            cancel_task(tx, &task.id)?;
        } else if matches!(intent.state.as_str(), "waiting" | "queued" | "provisioning") {
            // Once admitted, only receipt recovery may consume this generation.
            intent.state = if blocked && !has_receipt(tx, &intent)? {
                "waiting"
            } else if intent.state == "provisioning" {
                "provisioning"
            } else {
                "queued"
            }
            .into();
            write(tx, &intent)?;
        }
    }
    if let Some(event) = transition {
        let unassigned_release =
            event.kind == TaskEventKind::Unblocked && task.assignee.is_none() && current.is_none();
        let consumed_holder = current
            .as_ref()
            .is_some_and(|intent| intent.state == "delivered");
        let blocked_notice =
            event.kind == TaskEventKind::Blocked && (consumed_holder || !task.trackers.is_empty());
        if blocked_notice || unassigned_release {
            notices::enqueue(tx, &task.project_path, event)?;
        }
    }
    Ok(())
}

/// Retirement and project history deletion share one transaction, so neither
/// startup nor an off-lock completion can recover an orphaned assignment.
pub(in crate::store) fn cancel_project(tx: &Transaction, project: &str) -> Result<(), StoreError> {
    tx.execute("UPDATE tracker_delivery_intents SET state='cancelled', record=json_set(record, '$.state', 'cancelled') WHERE project_key=?1 AND state <> 'cancelled'", [project])?;
    tx.execute("UPDATE tracker_delivery_notices SET state='cancelled', record=json_set(record, '$.state', 'cancelled') WHERE project_key=?1 AND state='queued'", [project])?;
    Ok(())
}

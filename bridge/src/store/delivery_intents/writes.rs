use super::*;
use crate::tracker::{TaskEvent, TaskEventKind};
use rusqlite::Transaction;
use serde_json::json;

impl Store {
    /// Assignment and its authority land together; live graph determines state.
    pub fn accept_delivery_intent(
        &self,
        task: &Task,
        events: &[TaskEvent],
        mut intent: DeliveryIntent,
    ) -> Result<DeliveryIntent, StoreError> {
        self.in_transaction(|tx| {
            if task.is_finished() || intent.task_id != task.id || intent.project_path != task.project_path {
                return Err(invalid("delivery requires its open unfinished project task"));
            }
            if matches!(intent.target.get("kind").and_then(Value::as_str), None | Some("user")) {
                return Err(invalid("delivery requires an explicit agent target"));
            }
            let generation: i64 = tx.query_row("SELECT COALESCE(MAX(generation),0)+1 FROM tracker_delivery_intents WHERE task_id=?1", [&task.id], |r| r.get(0))?;
            if !intent.id.is_empty() && load(tx, &intent.id)?.is_some() {
                return Err(invalid("a new assignment requires a new delivery identity"));
            }
            lifecycle::cancel_task(tx, &task.id)?;
            tracker::write_tracker_task(tx, task)?;
            tracker::append_activity(tx, &[], events)?;
            intent.generation = generation as u64;
            if intent.id.is_empty() { intent.id = format!("td-{}", uuid::Uuid::new_v4().simple()); }
            if intent.operation_id.is_empty() { intent.operation_id = format!("task-delivery-{}", intent.id); }
            intent.state = if is_ready(tx, &intent)? { "queued" } else { "waiting" }.into();
            intent.error = None;
            intent.reservation = Value::Null;
            intent.admitted = None;
            write(tx, &intent)?;
            if intent.state == "waiting" {
                let event = TaskEvent::new(&task.id, intent.actor.clone(), TaskEventKind::DispatchDeferred, json!({"delivery_id":intent.id,"generation":intent.generation,"target":intent.target}), &task.updated_at);
                tracker::append_activity(tx, &[], &[event])?;
            }
            Ok(intent)
        })
    }

    pub fn claim_delivery_intent(
        &self,
        id: &str,
        generation: u64,
    ) -> Result<Option<DeliveryIntent>, StoreError> {
        self.in_transaction(|tx| {
            let Some(mut intent) = load(tx, id)? else {
                return Ok(None);
            };
            if intent.generation != generation
                || !authority(tx, &intent)?
                || !matches!(intent.state.as_str(), "queued" | "provisioning")
            {
                return Ok(None);
            }
            if !is_ready(tx, &intent)? && !has_receipt(tx, &intent)? {
                intent.state = "waiting".into();
                write(tx, &intent)?;
                return Ok(None);
            }
            intent.state = "provisioning".into();
            write(tx, &intent)?;
            Ok(Some(intent))
        })
    }

    /// Stable reservation identities may acquire additional recovery snapshots.
    /// An admitted request is completely immutable for the operation's lifetime.
    pub fn checkpoint_delivery_intent(
        &self,
        id: &str,
        generation: u64,
        reservation: Value,
        admitted: Option<Value>,
    ) -> Result<DeliveryIntent, StoreError> {
        self.in_transaction(|tx| {
            let mut intent = require(tx, id, generation)?;
            if intent.state == "delivered" {
                return Err(invalid("delivery already consumed"));
            }
            if !extends_reservation(&intent.reservation, &reservation)
                || (intent.admitted.is_some() && admitted != intent.admitted)
            {
                return Err(StoreError::OperationConflict {
                    operation_id: intent.operation_id,
                });
            }
            if admitted.is_some() && intent.admitted.is_none() && !is_ready(tx, &intent)? {
                return Err(StoreError::WaitingOnConflict);
            }
            intent.reservation = reservation;
            intent.admitted = admitted;
            write(tx, &intent)?;
            Ok(intent)
        })
    }

    pub fn fail_delivery_intent(
        &self,
        id: &str,
        generation: u64,
        error: &str,
    ) -> Result<(), StoreError> {
        self.in_transaction(|tx| {
            let mut intent = require(tx, id, generation)?;
            if matches!(intent.state.as_str(), "failed" | "delivered") {
                return Ok(());
            }
            intent.state = "failed".into();
            intent.error = Some(error.into());
            write(tx, &intent)?;
            let event = TaskEvent::new(
                &intent.task_id,
                Actor::Build,
                TaskEventKind::DispatchFailed,
                json!({"delivery_id":id,"generation":generation,"error":error}),
                &super::super::now_rfc3339(),
            );
            tracker::append_activity(tx, &[], std::slice::from_ref(&event))?;
            notices::enqueue(tx, &intent.project_path, &event)?;
            Ok(())
        })
    }

    pub fn retry_delivery_intent(&self, id: &str) -> Result<DeliveryIntent, StoreError> {
        self.in_transaction(|tx| {
            let original = load(tx, id)?.ok_or_else(|| invalid("delivery intent not found"))?;
            let mut intent = require(tx, id, original.generation)?;
            if intent.state != "failed" {
                return Err(invalid("only the current failed delivery can be retried"));
            }
            intent.state = if is_ready(tx, &intent)? {
                "queued"
            } else {
                "waiting"
            }
            .into();
            intent.error = None;
            write(tx, &intent)?;
            Ok(intent)
        })
    }

    pub fn settle_delivery_intent(
        &self,
        id: &str,
        generation: u64,
        task: &Task,
        events: &[TaskEvent],
    ) -> Result<bool, StoreError> {
        self.in_transaction(|tx| {
            let Some(mut intent) = load(tx, id)? else {
                return Ok(false);
            };
            if intent.generation != generation || !authority(tx, &intent)? {
                return Ok(false);
            }
            if intent.state == "delivered" {
                return Ok(false);
            }
            if task.id != intent.task_id
                || task.project_path != intent.project_path
                || intent.admitted.is_none()
            {
                return Err(invalid("settlement requires its admitted task delivery"));
            }
            // Already admitted work may acquire a new blocker before receipt
            // recovery. Preserve the task column in that case; never repost.
            let mut saved = task.clone();
            if !is_ready(tx, &intent)? {
                saved.status = super::task(tx, &intent)?.expect("authority checked").status;
            }
            tracker::write_tracker_task(tx, &saved)?;
            tracker::append_activity(tx, &[], events)?;
            intent.state = "delivered".into();
            write(tx, &intent)?;
            Ok(true)
        })
    }
}

fn extends_reservation(before: &Value, after: &Value) -> bool {
    match (before, after) {
        (Value::Null, _) => true,
        (Value::Object(before), Value::Object(after)) => before.iter().all(|(key, value)| {
            after
                .get(key)
                .is_some_and(|next| extends_reservation(value, next))
        }),
        _ => before == after,
    }
}

pub(in crate::store) fn check_operation_admission(
    tx: &Transaction,
    operation_id: &str,
) -> Result<(), StoreError> {
    let id: Option<String> = tx
        .query_row(
            "SELECT id FROM tracker_delivery_intents WHERE operation_id=?1",
            [operation_id],
            |r| r.get(0),
        )
        .optional()?;
    let Some(id) = id else {
        return Ok(());
    };
    let intent = load(tx, &id)?.expect("delivery selected");
    if !matches!(intent.state.as_str(), "queued" | "provisioning")
        || intent.admitted.is_none()
        || !authority(tx, &intent)?
        || !is_ready(tx, &intent)?
    {
        return Err(invalid("delivery no longer authorized for admission"));
    }
    Ok(())
}

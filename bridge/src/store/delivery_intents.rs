//! Durable explicit assignment authority. Task history and reads never arm work.
mod lifecycle;
mod notices;
mod writes;

use super::{tracker, Store, StoreError};
use crate::tracker::{Actor, Task};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub(super) use lifecycle::{cancel_project, note_activity, note_task_write, reconcile_task};
pub use notices::DeliveryNotice;
pub(super) use writes::check_operation_admission;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeliveryIntent {
    pub id: String,
    pub task_id: String,
    pub project_path: String,
    pub generation: u64,
    pub target: Value,
    pub actor: Actor,
    pub sender: Option<Value>,
    pub note: Option<String>,
    pub operation_id: String,
    pub state: String,
    pub reservation: Value,
    pub admitted: Option<Value>,
    pub error: Option<String>,
}

impl Store {
    pub(crate) fn retire_task_deliveries_of_project(&self, project: &str) -> Result<(), StoreError> {
        self.in_transaction(|tx| cancel_project(tx, project))
    }

    pub fn load_delivery_intent(&self, id: &str) -> Result<Option<DeliveryIntent>, StoreError> {
        load(&self.connection(), id)
    }

    pub fn delivery_intent_for_operation(
        &self,
        operation_id: &str,
    ) -> Result<Option<DeliveryIntent>, StoreError> {
        let conn = self.connection();
        let id: Option<String> = conn
            .query_row(
                "SELECT id FROM tracker_delivery_intents WHERE operation_id=?1",
                [operation_id],
                |r| r.get(0),
            )
            .optional()?;
        id.map(|id| load(&conn, &id))
            .transpose()
            .map(Option::flatten)
    }

    pub fn current_delivery_intent(
        &self,
        task_id: &str,
    ) -> Result<Option<DeliveryIntent>, StoreError> {
        current(&self.connection(), task_id)
    }

    /// Saved intents only. Admitted work remains discoverable when reblocked,
    /// so startup can reconcile its receipt without admitting another message.
    pub fn delivery_intents_ready(&self, limit: usize) -> Result<Vec<DeliveryIntent>, StoreError> {
        self.in_transaction(|tx| {
            let mut statement = tx.prepare("SELECT id, record FROM tracker_delivery_intents WHERE state IN ('queued', 'provisioning') ORDER BY rowid LIMIT ?1")?;
            let intents = statement.query_map([limit.clamp(1, 100) as i64], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
                .map(|r| { let (id, raw) = r?; tracker::decode::<DeliveryIntent>(&raw, "tracker_delivery_intents", &id) }).collect::<Result<Vec<_>, StoreError>>()?;
            drop(statement);
            let mut ready = Vec::new();
            for mut intent in intents {
                if authority(tx, &intent)? && (is_ready(tx, &intent)? || has_receipt(tx, &intent)?) {
                    ready.push(intent);
                } else if authority(tx, &intent)? {
                    intent.state = "waiting".into();
                    write(tx, &intent)?;
                }
            }
            Ok(ready)
        })
    }

    pub fn delivery_authority_current(
        &self,
        id: &str,
        generation: u64,
    ) -> Result<bool, StoreError> {
        let conn = self.connection();
        let Some(intent) = load(&conn, id)? else {
            return Ok(false);
        };
        Ok(intent.generation == generation && authority(&conn, &intent)?)
    }

    pub fn delivery_intent_is_current(
        &self,
        id: &str,
        generation: u64,
    ) -> Result<bool, StoreError> {
        let conn = self.connection();
        let Some(intent) = load(&conn, id)? else {
            return Ok(false);
        };
        Ok(intent.generation == generation
            && matches!(intent.state.as_str(), "queued" | "provisioning")
            && authority(&conn, &intent)?
            && is_ready(&conn, &intent)?)
    }
}

fn invalid(message: &str) -> StoreError {
    StoreError::WaitingOnInvalid(message.into())
}

fn load(conn: &Connection, id: &str) -> Result<Option<DeliveryIntent>, StoreError> {
    let raw: Option<String> = conn
        .query_row(
            "SELECT record FROM tracker_delivery_intents WHERE id = ?1",
            [id],
            |r| r.get(0),
        )
        .optional()?;
    raw.map(|raw| tracker::decode(&raw, "tracker_delivery_intents", id))
        .transpose()
}

fn current(conn: &Connection, task_id: &str) -> Result<Option<DeliveryIntent>, StoreError> {
    let raw: Option<(String, String)> = conn.query_row("SELECT id, record FROM tracker_delivery_intents WHERE task_id = ?1 AND state <> 'cancelled' ORDER BY generation DESC LIMIT 1", [task_id], |r| Ok((r.get(0)?, r.get(1)?))).optional()?;
    raw.map(|(id, raw)| tracker::decode(&raw, "tracker_delivery_intents", &id))
        .transpose()
}

fn require(conn: &Connection, id: &str, generation: u64) -> Result<DeliveryIntent, StoreError> {
    let intent = load(conn, id)?.ok_or_else(|| invalid("delivery intent not found"))?;
    if intent.generation != generation || !authority(conn, &intent)? {
        return Err(invalid("stale delivery intent"));
    }
    Ok(intent)
}

fn task(conn: &Connection, intent: &DeliveryIntent) -> Result<Option<Task>, StoreError> {
    let raw: Option<String> = conn
        .query_row(
            "SELECT record FROM tracker_tasks WHERE id = ?1 AND project_key = ?2",
            params![intent.task_id, intent.project_path],
            |r| r.get(0),
        )
        .optional()?;
    raw.map(|raw| tracker::decode(&raw, "tracker_tasks", &intent.task_id))
        .transpose()
}

fn authority(conn: &Connection, intent: &DeliveryIntent) -> Result<bool, StoreError> {
    if intent.state == "cancelled" {
        return Ok(false);
    }
    let Some(task) = task(conn, intent)? else {
        return Ok(false);
    };
    if task.is_finished() {
        return Ok(false);
    }
    let newer: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM tracker_delivery_intents WHERE task_id = ?1 AND generation > ?2)", params![intent.task_id, intent.generation as i64], |r| r.get(0))?;
    Ok(!newer)
}

fn has_receipt(conn: &Connection, intent: &DeliveryIntent) -> Result<bool, StoreError> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM operations WHERE operation_id=?1)",
        [&intent.operation_id],
        |r| r.get(0),
    )
    .map_err(Into::into)
}

fn is_ready(conn: &Connection, intent: &DeliveryIntent) -> Result<bool, StoreError> {
    let blocked: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM tracker_task_dependencies d JOIN tracker_tasks p ON p.id = d.waiting_on_task_id WHERE d.task_id = ?1 AND p.project_key = ?2 AND p.state = 'open' AND p.status <> 'done')", params![intent.task_id, intent.project_path], |r| r.get(0))?;
    Ok(!blocked)
}

fn write(conn: &Connection, intent: &DeliveryIntent) -> Result<(), StoreError> {
    conn.execute("INSERT INTO tracker_delivery_intents(id,task_id,project_key,generation,operation_id,state,record) VALUES (?1,?2,?3,?4,?5,?6,?7) ON CONFLICT(id) DO UPDATE SET state = ?6, record = ?7", params![intent.id,intent.task_id,intent.project_path,intent.generation as i64,intent.operation_id,intent.state,serde_json::to_string(intent).expect("delivery serializes")])?;
    Ok(())
}

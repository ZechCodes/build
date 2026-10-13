use super::*;
use crate::tracker::TaskEvent;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeliveryNotice {
    pub id: String,
    pub task_id: String,
    pub project_path: String,
    pub operation_id: String,
    pub payload: Value,
    pub admitted: Option<Value>,
    pub state: String,
}

pub(super) fn enqueue(
    conn: &Connection,
    project: &str,
    event: &TaskEvent,
) -> Result<(), StoreError> {
    let notice = DeliveryNotice {
        id: event.id.clone(),
        task_id: event.task_id.clone(),
        project_path: project.into(),
        operation_id: format!("task-dependency-notice-{}", event.id),
        payload: serde_json::to_value(event).expect("event serializes"),
        admitted: None,
        state: "queued".into(),
    };
    conn.execute("INSERT OR IGNORE INTO tracker_delivery_notices(id,task_id,project_key,state,record) VALUES (?1,?2,?3,?4,?5)", params![notice.id,notice.task_id,project,notice.state,serde_json::to_string(&notice).expect("notice serializes")])?;
    Ok(())
}

fn load_notice(conn: &Connection, id: &str) -> Result<Option<DeliveryNotice>, StoreError> {
    let raw: Option<String> = conn
        .query_row(
            "SELECT record FROM tracker_delivery_notices WHERE id=?1",
            [id],
            |r| r.get(0),
        )
        .optional()?;
    raw.map(|raw| tracker::decode(&raw, "tracker_delivery_notices", id))
        .transpose()
}

fn write_notice(conn: &Connection, notice: &DeliveryNotice) -> Result<(), StoreError> {
    conn.execute(
        "UPDATE tracker_delivery_notices SET state=?2,record=?3 WHERE id=?1",
        params![
            notice.id,
            notice.state,
            serde_json::to_string(notice).expect("notice serializes")
        ],
    )?;
    Ok(())
}

impl Store {
    pub fn delivery_notices_ready(&self, limit: usize) -> Result<Vec<DeliveryNotice>, StoreError> {
        let conn = self.connection();
        let mut statement = conn.prepare("SELECT id,record FROM tracker_delivery_notices WHERE state='queued' ORDER BY rowid LIMIT ?1")?;
        let notices = statement
            .query_map([limit.clamp(1, 100) as i64], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })?
            .map(|r| {
                let (id, raw) = r?;
                tracker::decode(&raw, "tracker_delivery_notices", &id)
            })
            .collect();
        notices
    }

    pub fn checkpoint_delivery_notice(
        &self,
        id: &str,
        admitted: Value,
    ) -> Result<DeliveryNotice, StoreError> {
        self.in_waiting_on_transaction(|tx| {
            let mut notice =
                load_notice(tx, id)?.ok_or_else(|| invalid("delivery notice not found"))?;
            if notice
                .admitted
                .as_ref()
                .is_some_and(|previous| previous != &admitted)
            {
                return Err(StoreError::OperationConflict {
                    operation_id: notice.operation_id,
                });
            }
            notice.admitted = Some(admitted);
            write_notice(tx, &notice)?;
            Ok(notice)
        })
    }

    pub fn settle_delivery_notice(&self, id: &str) -> Result<(), StoreError> {
        self.in_waiting_on_transaction(|tx| {
            let mut notice =
                load_notice(tx, id)?.ok_or_else(|| invalid("delivery notice not found"))?;
            notice.state = "delivered".into();
            write_notice(tx, &notice)
        })
    }
}

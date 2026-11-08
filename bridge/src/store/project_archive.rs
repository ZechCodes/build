//! Archive admission reads all durable blockers in one coherent SQL statement.
use super::{Store, StoreError};

impl Store {
    pub(crate) fn project_archive_task_blockers(
        &self,
        project: &str,
    ) -> Result<Vec<String>, StoreError> {
        let conn = self.connection();
        let mut statement = conn.prepare(r#"
            WITH tasks AS (SELECT id, number, state, status FROM tracker_tasks WHERE project_key=?1),
            blockers AS (
                SELECT t.number, 'answer #' || t.number AS action FROM tasks t
                JOIN tracker_task_query_facts q ON q.task_id=t.id WHERE q.attention_asked=1
                UNION ALL
                SELECT t.number, 'settle pending delivery for #' || t.number FROM tasks t
                JOIN tracker_delivery_intents i ON i.task_id=t.id
                WHERE i.state IN ('waiting','queued','provisioning','failed') AND t.state='open' AND t.status<>'done'
                UNION ALL
                SELECT t.number, 'resolve waiting-on for #' || t.number FROM tasks t
                JOIN tracker_task_query_facts q ON q.task_id=t.id WHERE q.blocked=1 AND q.unfinished=1
                UNION ALL
                SELECT t.number, 'give a verdict on #' || t.number FROM tasks t
                JOIN reviews r ON r.task_id=t.id WHERE json_extract(r.record,'$.pull_request.status')='open'
                    OR (COALESCE(json_extract(r.record,'$.mode'),'snapshot')='snapshot' AND json_extract(r.record,'$.state')='open')
            ) SELECT action FROM blockers ORDER BY number, action
        "#)?;
        let rows = statement.query_map([project], |row| row.get(0))?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }
}

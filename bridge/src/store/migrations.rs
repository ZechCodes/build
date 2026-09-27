use super::{Store, StoreError};
use rusqlite::Connection;

impl Store {
    /// Add one of the [`HOISTED_ITEM_COLUMNS`] to a table written before it
    /// existed.
    ///
    /// `CREATE TABLE IF NOT EXISTS` does not alter a table that already exists,
    /// so a column an older bridge never wrote has to be added by hand — and
    /// added before the schema batch, whose partial indexes name it.
    pub(super) fn add_hoisted_column(conn: &Connection, column: &str) -> Result<(), StoreError> {
        if conn
            .prepare(&format!("SELECT {column} FROM thread_items LIMIT 1"))
            .is_ok()
        {
            return Ok(());
        }
        conn.execute(
            &format!("ALTER TABLE thread_items ADD COLUMN {column} INTEGER NOT NULL DEFAULT 0"),
            [],
        )?;
        Ok(())
    }
    /// Test-only: stamp a schema version, so the refusal path can be exercised
    /// without a second build of the bridge.
    #[cfg(test)]
    pub fn set_schema_version(&self, version: i64) {
        self.connection()
            .execute(
                "INSERT INTO meta (key, value) VALUES ('schema_version', ?1)
                 ON CONFLICT(key) DO UPDATE SET value = ?1",
                [version.to_string()],
            )
            .expect("the schema version is stamped");
    }
    /// Test-only: strip the hoisted columns and stamp the version back, so the
    /// upgrade path can be exercised against a database this build wrote.
    #[cfg(test)]
    pub fn pretend_to_be_v1(&self) {
        let conn = self.connection();
        conn.execute_batch(
            "DROP INDEX IF EXISTS thread_items_attention;
             DROP INDEX IF EXISTS thread_items_conversation;
             DROP INDEX IF EXISTS thread_items_messages;
             DROP INDEX IF EXISTS thread_items_own_messages;
             DROP INDEX IF EXISTS thread_items_tool_calls;
             DROP INDEX IF EXISTS thread_items_activity;
             ALTER TABLE thread_items DROP COLUMN attention;
             ALTER TABLE thread_items DROP COLUMN message;
             ALTER TABLE thread_items DROP COLUMN tool_call;
             ALTER TABLE thread_items DROP COLUMN activity;
             ALTER TABLE thread_items DROP COLUMN handoff;
             UPDATE meta SET value = '1' WHERE key = 'schema_version';",
        )
        .expect("the v1 shape is staged");
    }
    /// Test-only: the same for the v2 shape — attention hoisted, message not.
    #[cfg(test)]
    pub fn pretend_to_be_v2(&self) {
        let conn = self.connection();
        conn.execute_batch(
            "DROP INDEX IF EXISTS thread_items_conversation;
             DROP INDEX IF EXISTS thread_items_messages;
             DROP INDEX IF EXISTS thread_items_own_messages;
             DROP INDEX IF EXISTS thread_items_tool_calls;
             DROP INDEX IF EXISTS thread_items_activity;
             ALTER TABLE thread_items DROP COLUMN message;
             ALTER TABLE thread_items DROP COLUMN tool_call;
             ALTER TABLE thread_items DROP COLUMN activity;
             ALTER TABLE thread_items DROP COLUMN handoff;
             UPDATE meta SET value = '2' WHERE key = 'schema_version';",
        )
        .expect("the v2 shape is staged");
    }
    /// Test-only: the v3 shape — attention and message hoisted, the rest not.
    #[cfg(test)]
    pub fn pretend_to_be_v3(&self) {
        let conn = self.connection();
        conn.execute_batch(
            "DROP INDEX IF EXISTS thread_items_own_messages;
             DROP INDEX IF EXISTS thread_items_tool_calls;
             DROP INDEX IF EXISTS thread_items_activity;
             ALTER TABLE thread_items DROP COLUMN tool_call;
             ALTER TABLE thread_items DROP COLUMN activity;
             ALTER TABLE thread_items DROP COLUMN handoff;
             UPDATE meta SET value = '3' WHERE key = 'schema_version';",
        )
        .expect("the v3 shape is staged");
    }
    /// Test-only: the v4 shape — everything hoisted but the runs.
    #[cfg(test)]
    pub fn pretend_to_be_v4(&self) {
        let conn = self.connection();
        conn.execute_batch(
            "DROP INDEX IF EXISTS thread_items_own_messages;
             DROP INDEX IF EXISTS thread_items_activity;
             ALTER TABLE thread_items DROP COLUMN activity;
             ALTER TABLE thread_items DROP COLUMN handoff;
             UPDATE meta SET value = '4' WHERE key = 'schema_version';",
        )
        .expect("the v4 shape is staged");
    }
    /// Test-only: the v6 shape — every column but the hand-off flag.
    #[cfg(test)]
    pub fn pretend_to_be_v6(&self) {
        self.connection()
            .execute_batch(
                "DROP INDEX IF EXISTS thread_items_own_messages;
                 ALTER TABLE thread_items DROP COLUMN handoff;
                 UPDATE meta SET value = '6' WHERE key = 'schema_version';",
            )
            .expect("the v6 shape is staged");
    }
    /// Test-only: the v7 shape — everything but the tracker's three tables.
    #[cfg(test)]
    pub fn pretend_to_be_v7(&self) {
        self.connection()
            .execute_batch(
                "DROP INDEX IF EXISTS tracker_tasks_number;
                 DROP INDEX IF EXISTS tracker_tasks_by_project;
                 DROP INDEX IF EXISTS tracker_comments_by_task;
                 DROP INDEX IF EXISTS tracker_events_by_task;
                 DROP TABLE IF EXISTS tracker_comments;
                 DROP TABLE IF EXISTS tracker_events;
                 DROP TABLE IF EXISTS tracker_tasks;
                 UPDATE meta SET value = '7' WHERE key = 'schema_version';",
            )
            .expect("the v7 shape is staged");
    }

    /// Test-only: the v5 shape, before durable operation receipts.
    #[cfg(test)]
    pub fn pretend_to_be_v5(&self) {
        self.connection()
            .execute_batch(
                "DROP INDEX IF EXISTS thread_items_own_messages;
                 DROP INDEX IF EXISTS operations_by_status;
                 DROP TABLE IF EXISTS operations;
                 DROP TABLE IF EXISTS agent_migration_backups;
                 ALTER TABLE thread_items DROP COLUMN handoff;
                 UPDATE meta SET value = '5' WHERE key = 'schema_version';",
            )
            .expect("the v5 shape is staged");
    }
}

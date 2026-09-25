//! What the workspace reclaim service last concluded about each workspace
//! (#135). It is one row in `meta`, rewritten whole after each sweep: a
//! device holds tens of workspaces, and every record is read and written
//! together.

use super::{Store, StoreError};
use crate::reclaim::LifecycleRecord;
use rusqlite::OptionalExtension;
use std::collections::HashMap;
use std::path::PathBuf;

const WORKSPACE_LIFECYCLE_KEY: &str = "workspace_lifecycle";

impl Store {
    pub fn load_workspace_lifecycle(&self) -> Result<HashMap<String, LifecycleRecord>, StoreError> {
        let raw: Option<String> = self
            .connection()
            .query_row(
                "SELECT value FROM meta WHERE key = ?1",
                [WORKSPACE_LIFECYCLE_KEY],
                |row| row.get(0),
            )
            .optional()?;
        let Some(raw) = raw else {
            return Ok(HashMap::new());
        };
        serde_json::from_str(&raw).map_err(|source| StoreError::Corrupt {
            path: PathBuf::from(format!("meta/{WORKSPACE_LIFECYCLE_KEY}")),
            source,
        })
    }

    pub fn save_workspace_lifecycle(
        &self,
        records: &HashMap<String, LifecycleRecord>,
    ) -> Result<(), StoreError> {
        let value = serde_json::to_string(records).expect("lifecycle records serialize");
        self.connection().execute(
            "INSERT INTO meta (key, value) VALUES (?1, ?2) \
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            rusqlite::params![WORKSPACE_LIFECYCLE_KEY, value],
        )?;
        Ok(())
    }
}

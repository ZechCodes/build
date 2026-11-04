//! What the workspace reclaim service last concluded about each workspace
//! (#135). It is one row in `meta`, rewritten whole after each sweep: a
//! device holds tens of workspaces, and every record is read and written
//! together.

use super::{Store, StoreError};
use crate::reclaim::LifecycleRecord;
use rusqlite::OptionalExtension;
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

const WORKSPACE_LIFECYCLE_KEY: &str = "workspace_lifecycle";
const IDLE_DIGEST_SENT_KEY: &str = "idle_digest_sent";

/// The local day, `YYYY-MM-DD`, each project's idle-workspace digest last
/// went out for (#958), by project id.
pub type IdleDigestsSent = BTreeMap<String, String>;

impl Store {
    pub fn load_workspace_lifecycle(&self) -> Result<HashMap<String, LifecycleRecord>, StoreError> {
        self.load_meta_json(WORKSPACE_LIFECYCLE_KEY)
    }

    pub fn save_workspace_lifecycle(
        &self,
        records: &HashMap<String, LifecycleRecord>,
    ) -> Result<(), StoreError> {
        let value = serde_json::to_string(records).expect("lifecycle records serialize");
        self.writer().sync_transaction(|tx, batch| {
            tx.execute(
                "INSERT INTO meta(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=?2",
                rusqlite::params![WORKSPACE_LIFECYCLE_KEY, value],
            )?;
            super::sync::stage_device_lifecycle(tx, batch, records)
        })
    }

    pub fn load_idle_digests_sent(&self) -> Result<IdleDigestsSent, StoreError> {
        self.load_meta_json(IDLE_DIGEST_SENT_KEY)
    }

    pub fn save_idle_digests_sent(&self, sent: &IdleDigestsSent) -> Result<(), StoreError> {
        let value = serde_json::to_string(sent).expect("digest days serialize");
        self.save_meta_json(IDLE_DIGEST_SENT_KEY, &value)
    }

    /// The JSON one `meta` row holds, or the empty value when there is none.
    fn load_meta_json<T: serde::de::DeserializeOwned + Default>(
        &self,
        key: &str,
    ) -> Result<T, StoreError> {
        let raw: Option<String> = self
            .connection()
            .query_row("SELECT value FROM meta WHERE key = ?1", [key], |row| {
                row.get(0)
            })
            .optional()?;
        let Some(raw) = raw else {
            return Ok(T::default());
        };
        serde_json::from_str(&raw).map_err(|source| StoreError::Corrupt {
            path: PathBuf::from(format!("meta/{key}")),
            source,
        })
    }

    fn save_meta_json(&self, key: &str, value: &str) -> Result<(), StoreError> {
        self.connection().execute(
            "INSERT INTO meta (key, value) VALUES (?1, ?2) \
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            rusqlite::params![key, value],
        )?;
        Ok(())
    }
}

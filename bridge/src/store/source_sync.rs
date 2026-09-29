//! What the last sync of each project source's base branch concluded (#267).
//! One row in `meta`, rewritten whole after each pass: a device holds a
//! handful of sources, and every status is read and written together.

use super::{Store, StoreError};
use rusqlite::OptionalExtension;
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::collections::HashMap;
use std::path::PathBuf;

const SOURCE_SYNC_KEY: &str = "source_sync";

impl Store {
    pub fn load_source_syncs<T: DeserializeOwned>(&self) -> Result<HashMap<String, T>, StoreError> {
        let raw: Option<String> = self
            .connection()
            .query_row(
                "SELECT value FROM meta WHERE key = ?1",
                [SOURCE_SYNC_KEY],
                |row| row.get(0),
            )
            .optional()?;
        let Some(raw) = raw else {
            return Ok(HashMap::new());
        };
        serde_json::from_str(&raw).map_err(|source| StoreError::Corrupt {
            path: PathBuf::from(format!("meta/{SOURCE_SYNC_KEY}")),
            source,
        })
    }

    pub fn save_source_syncs<T: Serialize>(
        &self,
        statuses: &HashMap<String, T>,
    ) -> Result<(), StoreError> {
        let value = serde_json::to_string(statuses).expect("sync statuses serialize");
        self.connection().execute(
            "INSERT INTO meta (key, value) VALUES (?1, ?2) \
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            rusqlite::params![SOURCE_SYNC_KEY, value],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sync_statuses_come_back_after_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path()).unwrap();
        assert!(store.load_source_syncs::<u64>().unwrap().is_empty());

        store
            .save_source_syncs(&HashMap::from([("proj-1/source-1".to_string(), 7_u64)]))
            .unwrap();
        drop(store);

        let reopened = Store::new(dir.path()).unwrap();
        assert_eq!(
            reopened.load_source_syncs::<u64>().unwrap()["proj-1/source-1"],
            7
        );
    }
}

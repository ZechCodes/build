use super::*;

impl Store {
    /// Observations have their own revision. A sync error or pending commit
    /// cannot change review lifecycle, version, opinions or snapshot history.
    pub fn save_review_sync_observation(
        &self,
        observation: &ReviewSyncObservation,
        expected_revision: u64,
    ) -> Result<ReviewSyncObservation, StoreError> {
        self.in_transaction(|tx| {
            require_pull_request(tx, &observation.task_id)?;
            let exists: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM review_branch_bindings WHERE task_id = ?1 AND directory_id = ?2)", params![observation.task_id, observation.directory_id], |row| row.get(0))?;
            if !exists {
                return Err(invalid("sync observation must name a bound Git directory"));
            }
            let found: Option<i64> = tx.query_row("SELECT revision FROM review_sync_observations WHERE task_id = ?1 AND directory_id = ?2", params![observation.task_id, observation.directory_id], |row| row.get(0)).optional()?;
            check_operation_version(&format!("{}/{}", observation.task_id, observation.directory_id), expected_revision, found.unwrap_or(0) as u64)?;
            let mut saved = observation.clone();
            saved.revision = found.unwrap_or(0) as u64 + 1;
            tx.execute("INSERT INTO review_sync_observations (task_id, directory_id, revision, record) VALUES (?1, ?2, ?3, ?4) ON CONFLICT(task_id, directory_id) DO UPDATE SET revision = ?3, record = ?4", params![saved.task_id, saved.directory_id, saved.revision as i64, serde_json::to_string(&saved).expect("sync observation serializes")])?;
            Ok(saved)
        })
    }

    pub fn load_review_sync_observations(
        &self,
        task_id: &str,
    ) -> Result<Vec<ReviewSyncObservation>, StoreError> {
        let conn = self.connection();
        let mut statement = conn.prepare("SELECT directory_id, record FROM review_sync_observations WHERE task_id = ?1 ORDER BY directory_id")?;
        let rows = statement.query_map([task_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        rows.map(|row| {
            let (directory, raw) = row?;
            decode(
                &raw,
                "review_sync_observations",
                &format!("{task_id}/{directory}"),
            )
        })
        .collect()
    }
}

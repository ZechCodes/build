//! A dedicated read-only lane: maintenance never takes the writer mutex or
//! occupies a connection from the bounded interactive read pool.
use super::*;
use crate::reviews::model::ReviewOpening;

pub(crate) struct ReceiverMaintenanceReader(Connection);

impl Store {
    pub(crate) fn receiver_maintenance_reader(
        &self,
    ) -> Result<ReceiverMaintenanceReader, StoreError> {
        let connection = Connection::open_with_flags(
            self.dir.join(super::super::DB_FILE),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )?;
        connection.busy_timeout(std::time::Duration::ZERO)?;
        Ok(ReceiverMaintenanceReader(connection))
    }
}

impl ReceiverMaintenanceReader {
    pub(crate) fn reviews_at(&self, receiver: &std::path::Path) -> Result<Vec<Review>, StoreError> {
        let mut statement = self.0.prepare("SELECT DISTINCT r.record FROM reviews r JOIN review_branch_bindings b ON r.task_id = b.task_id WHERE b.receiving_repository = ?1 ORDER BY r.task_id")?;
        let rows =
            statement.query_map([receiver.to_string_lossy()], |row| row.get::<_, String>(0))?;
        rows.map(|row| {
            let header: ReviewHeader = decode(&row?, "reviews", "receiver-maintenance")?;
            let bindings = pull_requests::load_bindings(&self.0, &header.task_id)?;
            Ok(Review {
                task_id: header.task_id,
                workspace_id: header.workspace_id,
                version: header.version,
                state: header.state,
                completion: header.completion,
                mode: header.mode,
                pull_request: header.pull_request,
                bindings,
                snapshots: Vec::new(),
                actions: Vec::new(),
                destinations: Vec::new(),
            })
        })
        .collect()
    }

    pub(crate) fn has_pending_opening(
        &self,
        receiver: &std::path::Path,
    ) -> Result<bool, StoreError> {
        // Before bindings reach their indexed table they still live in the
        // unfinished opening journal, which must also protect its receiver.
        let mut statement = self.0.prepare("SELECT record FROM review_openings WHERE state IN ('preparing', 'failed', 'interrupted')")?;
        for row in statement.query_map([], |row| row.get::<_, String>(0))? {
            let opening: ReviewOpening = decode(&row?, "review_openings", "maintenance")?;
            if opening
                .bindings
                .iter()
                .any(|binding| binding.receiving_repository == receiver)
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub(crate) fn merge_unsettled(&self, review: &Review) -> Result<bool, StoreError> {
        if pull_requests::publication_pending(&self.0, None, Some(&review.task_id))? {
            return Ok(true);
        }
        let mut statement = self
            .0
            .prepare("SELECT record FROM review_merge_intents WHERE task_id = ?1")?;
        let records = statement
            .query_map([&review.task_id], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        if records.is_empty() {
            return Ok(false);
        }
        // Only terminal reviews reach this method. Inspect their recorded
        // uncertain Git outcomes with the merge service's existing predicate.
        let mut review = review.clone();
        review.actions = actions::load_actions(&self.0, &review.task_id)?;
        for record in records {
            let intent = decode(&record, "review_merge_intents", &review.task_id)?;
            if crate::reviews::merge::progress::unresolved_actions(&review, &intent) {
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub(crate) fn has_candidate(&self, task: &str) -> Result<bool, StoreError> {
        self.0
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM review_sync_candidates WHERE task_id = ?1)",
                [task],
                |row| row.get(0),
            )
            .map_err(StoreError::from)
    }
}

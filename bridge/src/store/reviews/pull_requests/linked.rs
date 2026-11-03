//! Detail-only reverse discovery of child review tasks, without task bodies.
use super::{decode, header_summary, invalid, ReviewHeader};
use crate::reviews::records::ReviewSummary;
use crate::store::{ReadTransaction, Store, StoreError};
use rusqlite::params;

const MAX_LINKED_REVIEWS: usize = 20;

impl Store {
    pub fn load_linked_review_summaries(
        &self,
        project: &str,
        parent: &str,
    ) -> Result<(Vec<ReviewSummary>, bool), StoreError> {
        ReadTransaction::new(&self.connection()).load_linked_review_summaries(project, parent)
    }
}

impl ReadTransaction<'_> {
    /// Bounded newest-first reverse links. One extra row detects truncation.
    pub fn load_linked_review_summaries(
        &self,
        project: &str,
        parent: &str,
    ) -> Result<(Vec<ReviewSummary>, bool), StoreError> {
        let mut statement = self.connection().prepare(
            "SELECT t.id, t.number, r.record FROM tracker_tasks t
             JOIN reviews r ON r.task_id = t.id
             WHERE t.project_key = ?1
               AND CASE WHEN json_valid(t.record) THEN json_extract(t.record, '$.links.parent_task_id') END = ?2
               AND json_extract(r.record, '$.mode') = 'pull_request'
             ORDER BY t.number DESC LIMIT ?3",
        )?;
        let rows = statement.query_map(params![project, parent, 21_i64], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        let mut summaries = Vec::new();
        for row in rows {
            let (id, number, raw) = row?;
            let header: ReviewHeader = decode(&raw, "reviews", &id)?;
            if let Some(mut summary) = header_summary(header, Some(parent.to_owned())) {
                summary.number = Some(
                    u64::try_from(number)
                        .map_err(|_| invalid("linked review has a negative task number"))?,
                );
                summaries.push(summary);
            }
        }
        let truncated = summaries.len() > MAX_LINKED_REVIEWS;
        summaries.truncate(MAX_LINKED_REVIEWS);
        Ok((summaries, truncated))
    }
}

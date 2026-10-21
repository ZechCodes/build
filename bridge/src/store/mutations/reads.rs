use super::*;
#[cfg(test)]
thread_local! {
    pub(super) static OUTBOX_QUERY_STEPS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}
#[cfg(test)]
fn record_steps(statement: &rusqlite::Statement<'_>) {
    OUTBOX_QUERY_STEPS.with(|steps| {
        steps.set(steps.get() + statement.get_status(rusqlite::StatementStatus::VmStep) as u64)
    });
}
impl ReadTransaction<'_> {
    pub fn journal_incarnation(&self) -> Result<String, StoreError> {
        Ok(self.connection().query_row(
            "SELECT value FROM meta WHERE key='journal_incarnation'",
            [],
            |row| row.get(0),
        )?)
    }
    pub fn mutation_status(
        &self,
        principal: &str,
        ids: &[String],
    ) -> Result<Vec<Option<MutationRecord>>, StoreError> {
        if ids.len() > 128 {
            return Err(invalid("status limit exceeded"));
        }
        let mut bytes = 0;
        let mut records = Vec::new();
        for id in ids {
            let record = load(self.connection(), principal, id)?;
            bytes += record
                .as_ref()
                .map(|record| json!(record).to_string().len())
                .unwrap_or(0);
            if bytes > 512 * 1024 {
                return Err(invalid("status byte limit exceeded; page by operation ID"));
            }
            records.push(record);
        }
        Ok(records)
    }
    pub fn mutation_cursor(&self, principal: &str) -> Result<u64, StoreError> {
        let mut statement = self
            .connection()
            .prepare("SELECT coalesce(max(cursor),0) FROM mutation_outbox WHERE principal=?1")?;
        let cursor = statement.query_row([principal], |row| row.get::<_, i64>(0))? as u64;
        #[cfg(test)]
        record_steps(&statement);
        Ok(cursor)
    }
    pub fn mutation_outbox(
        &self,
        principal: &str,
        after: u64,
        limit: usize,
    ) -> Result<Vec<MutationOutboxEntry>, StoreError> {
        if limit > 256 {
            return Err(invalid("outbox limit exceeded"));
        }
        let mut statement=self.connection().prepare("SELECT o.cursor,c.record FROM mutation_outbox o JOIN mutation_commands c USING(command_id) WHERE o.principal=?1 AND o.cursor>?2 ORDER BY o.cursor LIMIT ?3")?;
        let rows = statement.query_map(
            params![
                principal,
                i64::try_from(after).map_err(|_| invalid("cursor exceeds SQLite range"))?,
                limit as i64
            ],
            |row| Ok((row.get::<_, i64>(0)? as u64, row.get::<_, String>(1)?)),
        )?;
        let mut result = Vec::new();
        let mut bytes = 0;
        for row in rows {
            let (cursor, record) = row?;
            bytes += record.len();
            if bytes > 512 * 1024 {
                break;
            }
            result.push(MutationOutboxEntry {
                cursor,
                operation: decode(record)?,
            });
        }
        #[cfg(test)]
        record_steps(&statement);
        Ok(result)
    }
    pub fn mutation_reservation(
        &self,
        principal: &str,
        kind: &str,
        id: &str,
    ) -> Result<Option<MutationRecord>, StoreError> {
        self.connection().query_row("SELECT c.record FROM mutation_reservations r JOIN mutation_commands c USING(command_id) WHERE c.principal=?1 AND r.kind=?2 AND r.resource_id=?3 AND c.state<>'succeeded'",params![principal,kind,id],|row|row.get(0)).optional()?.map(decode).transpose()
    }
}

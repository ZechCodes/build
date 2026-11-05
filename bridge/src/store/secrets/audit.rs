use super::{records, writes};
use crate::secrets::{
    Result, SecretAuditRecord, SecretAuditSummary, SecretCaller, SecretError, SecretRecord,
};
use crate::store::StoreWriter;
use rusqlite::{params, OptionalExtension, Transaction};

const DETAIL_RETENTION_SECONDS: i64 = 90 * 24 * 60 * 60;

pub(super) fn append_in(
    tx: &Transaction<'_>,
    caller: &SecretCaller,
    record: Option<&SecretRecord>,
    outcome: &str,
) -> Result<()> {
    let now = records::highwater(tx)?;
    // Only resolved, authorized metadata reaches the durable sink. Never
    // persist an endpoint-supplied selector or correlation ID: either can be
    // chosen to contain a capability or a UUID-shaped secret value.
    let id = record.map_or("unavailable", |record| record.context.id.as_str());
    if outcome != "released" && coalesce_denial_in(tx, caller, id, outcome, now)? {
        if record.is_some() {
            summary_in(tx, id, now, outcome)?;
        }
        return Ok(());
    }
    let request_id = uuid::Uuid::new_v4().to_string();
    let creator = record.map(|record| record.context.creator.as_str());
    let changed=tx.execute("INSERT INTO secret_audit (secret_id,creator,project_id,owner_id,agent_id,conversation_id,generation,session_incarnation,request_id,occurred_at,last_occurred_at,outcome) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?10,?11)",params![id,creator,caller.project_id,caller.owner_id,caller.agent_id,caller.conversation_id,caller.generation,caller.session_incarnation,request_id,now,outcome])?;
    if changed != 1 {
        return Err(SecretError::Unavailable);
    }
    if outcome != "released" {
        // At most one new denial group entered this transaction. The startup
        // migration brings older stores under this cap before admission.
        tx.execute("DELETE FROM secret_audit WHERE sequence=(SELECT sequence FROM secret_audit WHERE outcome!='released' ORDER BY sequence DESC LIMIT 1 OFFSET 4096)", [])?;
    }
    // Unknown/wrong-scope targets never grow per-secret summaries.
    if record.is_some() {
        summary_in(tx, id, now, outcome)?;
    }
    Ok(())
}

fn coalesce_denial_in(
    tx: &Transaction<'_>,
    caller: &SecretCaller,
    id: &str,
    outcome: &str,
    now: i64,
) -> Result<bool> {
    let changed = tx.execute("UPDATE secret_audit SET attempt_count=MIN(attempt_count,9223372036854775806)+1,last_occurred_at=?1 WHERE sequence=(SELECT sequence FROM secret_audit WHERE agent_id=?2 AND secret_id=?3 AND outcome=?4 AND outcome!='released' AND occurred_at>=?5 AND project_id=?6 AND owner_id=?7 AND conversation_id=?8 AND generation=?9 AND session_incarnation=?10 ORDER BY occurred_at DESC LIMIT 1)", params![now,caller.agent_id,id,outcome,now-now%60,caller.project_id,caller.owner_id,caller.conversation_id,caller.generation,caller.session_incarnation])?;
    Ok(changed == 1)
}

fn summary_in(tx: &Transaction<'_>, id: &str, now: i64, outcome: &str) -> Result<()> {
    let released = outcome == "released";
    let changed=tx.execute("INSERT INTO secret_audit_summaries(secret_id,release_count,denial_count,last_released_at,last_outcome) VALUES (?1,?2,?3,?4,?5) ON CONFLICT(secret_id) DO UPDATE SET release_count=release_count+excluded.release_count,denial_count=denial_count+excluded.denial_count,last_released_at=COALESCE(excluded.last_released_at,last_released_at),last_outcome=excluded.last_outcome",params![id,i64::from(released),i64::from(!released),released.then_some(now),outcome])?;
    if changed != 1 {
        return Err(SecretError::Unavailable);
    }
    Ok(())
}

impl StoreWriter {
    /// Metadata-only bounded keyset page. User-facing admission belongs to the
    /// authenticated metadata API; this internal reader grants no authority.
    pub fn secret_audit_page(
        &self,
        id: &str,
        after: i64,
        limit: usize,
    ) -> Result<Vec<SecretAuditRecord>> {
        crate::secrets::validate_id(id)?;
        if after < 0 || !(1..=100).contains(&limit) {
            return Err(SecretError::Invalid);
        }
        let connection = self.connection();
        let mut query=connection.prepare("SELECT sequence,secret_id,creator,project_id,owner_id,agent_id,conversation_id,generation,session_incarnation,request_id,occurred_at,outcome,attempt_count,last_occurred_at FROM secret_audit WHERE secret_id=?1 AND sequence>?2 ORDER BY sequence LIMIT ?3")?;
        let records = query
            .query_map(params![id, after, limit as i64], audit_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(records)
    }

    /// Includes fixed `unavailable` targets without accepting caller-supplied
    /// secret selectors. Internal metadata only; this reader grants no authority.
    pub fn secret_audit_by_caller_page(
        &self,
        agent_id: &str,
        after: i64,
        limit: usize,
    ) -> Result<Vec<SecretAuditRecord>> {
        if agent_id.is_empty()
            || agent_id.len() > 128
            || agent_id.contains('\0')
            || after < 0
            || !(1..=100).contains(&limit)
        {
            return Err(SecretError::Invalid);
        }
        let connection = self.connection();
        let mut query = connection.prepare("SELECT sequence,secret_id,creator,project_id,owner_id,agent_id,conversation_id,generation,session_incarnation,request_id,occurred_at,outcome,attempt_count,last_occurred_at FROM secret_audit WHERE agent_id=?1 AND sequence>?2 ORDER BY sequence LIMIT ?3")?;
        let records = query
            .query_map(params![agent_id, after, limit as i64], audit_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(records)
    }

    pub fn secret_audit_summary(&self, id: &str) -> Result<Option<SecretAuditSummary>> {
        crate::secrets::validate_id(id)?;
        Ok(self.connection().query_row("SELECT release_count,denial_count,last_released_at,last_outcome FROM secret_audit_summaries WHERE secret_id=?1",[id],|row|Ok(SecretAuditSummary {release_count:row.get(0)?,denial_count:row.get(1)?,last_released_at:row.get(2)?,last_outcome:row.get(3)?})).optional()?)
    }

    /// At most eight 1,000-row transactions per sweep, releasing the writer
    /// between batches. This exceeds the local endpoint's 64/s admission rate
    /// over its 60-second maintenance interval. Summaries remain indefinitely.
    pub fn secrets_audit_maintain(&self, now: i64) -> Result<usize> {
        if now < 0 {
            return Err(SecretError::Invalid);
        }
        let mut total = 0;
        for _ in 0..8 {
            let removed = writes::durable(self, |tx| {
                Ok(tx.execute("DELETE FROM secret_audit WHERE sequence IN (SELECT sequence FROM secret_audit WHERE occurred_at<?1 ORDER BY occurred_at,sequence LIMIT 1000)",[now.saturating_sub(DETAIL_RETENTION_SECONDS)])?)
            })?;
            total += removed;
            if removed < 1000 {
                break;
            }
        }
        Ok(total)
    }

    pub(crate) fn secrets_audit_unauthenticated(&self, count: i64, now: i64) -> Result<()> {
        if count <= 0 || now < 0 {
            return Err(SecretError::Invalid);
        }
        writes::durable(self, |tx| {
            let changed=tx.execute("INSERT INTO secret_unauthenticated_summary(singleton,attempt_count,first_at,last_at) VALUES (1,?1,?2,?2) ON CONFLICT(singleton) DO UPDATE SET attempt_count=MIN(9223372036854775807-attempt_count,excluded.attempt_count)+attempt_count,last_at=MAX(last_at,excluded.last_at)",params![count,now])?;
            if changed != 1 {
                return Err(SecretError::Unavailable);
            }
            Ok(())
        })
    }
}

fn audit_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<SecretAuditRecord> {
    Ok(SecretAuditRecord {
        sequence: row.get(0)?,
        secret_id: row.get(1)?,
        creator: row.get(2)?,
        project_id: row.get(3)?,
        owner_id: row.get(4)?,
        agent_id: row.get(5)?,
        conversation_id: row.get(6)?,
        generation: row.get(7)?,
        session_incarnation: row.get(8)?,
        request_id: row.get(9)?,
        occurred_at: row.get(10)?,
        outcome: row.get(11)?,
        attempt_count: row.get(12)?,
        last_occurred_at: row.get(13)?,
    })
}

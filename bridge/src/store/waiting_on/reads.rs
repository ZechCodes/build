use super::{invalid, require_task, DependencyDigest, DependencySummary, DependentsPage};
use crate::store::ReadTransaction;
use crate::store::{Store, StoreError};
use crate::tracker::{TaskState, DONE_STATUS};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use rusqlite::{params, Connection};
use sha2::{Digest, Sha256};
use std::collections::HashMap;

const DIGEST_COLUMNS: &str = "t.id, t.number, json_extract(t.record, '$.title'), t.state, t.status";

fn digest(row: &rusqlite::Row<'_>) -> rusqlite::Result<DependencyDigest> {
    let state: String = row.get(3)?;
    let status: String = row.get(4)?;
    let resolved_by = match (status.as_str(), state.as_str()) {
        (DONE_STATUS, _) => Some("done".to_string()),
        (_, "closed") => Some("close".to_string()),
        _ => None,
    };
    Ok(DependencyDigest {
        task_id: row.get(0)?,
        number: row.get::<_, i64>(1)? as u64,
        title: row.get(2)?,
        state: TaskState::parse(&state).unwrap_or_default(),
        status,
        resolved: resolved_by.is_some(),
        resolved_by,
    })
}

pub(super) fn forward(conn: &Connection, id: &str) -> Result<Vec<DependencyDigest>, StoreError> {
    let mut statement = conn.prepare(&format!("SELECT {DIGEST_COLUMNS} FROM tracker_task_dependencies d JOIN tracker_tasks t ON t.id = d.waiting_on_task_id WHERE d.task_id = ?1 ORDER BY d.position"))?;
    let rows = statement
        .query_map([id], digest)?
        .collect::<Result<_, _>>()?;
    Ok(rows)
}

impl Store {
    pub fn waiting_on_summary(
        &self,
        project: &str,
        id: &str,
    ) -> Result<DependencySummary, StoreError> {
        ReadTransaction::new(&self.connection()).waiting_on_summary(project, id)
    }

    /// Batch forward facts and reverse counts, without timeline reads or
    /// repeating bodies, and bounded bind counts for long task lists.
    pub fn waiting_on_summaries(
        &self,
        project: &str,
        ids: &[String],
    ) -> Result<HashMap<String, DependencySummary>, StoreError> {
        ReadTransaction::new(&self.connection()).waiting_on_summaries(project, ids)
    }

    pub fn waiting_on_blocks(
        &self,
        project: &str,
        id: &str,
        limit: usize,
        cursor: Option<&str>,
    ) -> Result<DependentsPage, StoreError> {
        ReadTransaction::new(&self.connection()).waiting_on_blocks(project, id, limit, cursor)
    }
}

fn parse_cursor(cursor: Option<&str>, scope: &str) -> Result<u64, StoreError> {
    let Some(cursor) = cursor else { return Ok(0) };
    if cursor.len() > 256 {
        return Err(invalid("invalid reverse cursor"));
    }
    let bytes = URL_SAFE_NO_PAD
        .decode(cursor)
        .map_err(|_| invalid("invalid reverse cursor"))?;
    let (made_in, after): (String, u64) =
        serde_json::from_slice(&bytes).map_err(|_| invalid("invalid reverse cursor"))?;
    if made_in != scope || after > i64::MAX as u64 {
        return Err(invalid("reverse cursor belongs to another list"));
    }
    Ok(after)
}

fn read_chunk(
    conn: &Connection,
    project: &str,
    ids: &[String],
    summaries: &mut HashMap<String, DependencySummary>,
) -> Result<(), StoreError> {
    let marks = vec!["?"; ids.len()].join(",");
    let binds = std::iter::once(project)
        .chain(ids.iter().map(String::as_str))
        .collect::<Vec<_>>();
    let mut statement = conn.prepare(&format!(
        "SELECT t.id, t.state, t.status,
        (SELECT COUNT(*) FROM tracker_task_dependencies d WHERE d.waiting_on_task_id = t.id)
        FROM tracker_tasks t WHERE t.project_key = ? AND t.id IN ({marks})"
    ))?;
    let rows = statement.query_map(rusqlite::params_from_iter(&binds), |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, i64>(3)? as u64,
        ))
    })?;
    let mut unfinished = std::collections::HashSet::new();
    for row in rows {
        let (id, state, status, count) = row?;
        if state == "open" && status != DONE_STATUS {
            unfinished.insert(id.clone());
        }
        summaries.insert(
            id,
            DependencySummary {
                blocks_count: count,
                ..Default::default()
            },
        );
    }
    read_forward_chunk(conn, project, ids, summaries)?;
    for id in unfinished {
        if let Some(summary) = summaries.get_mut(&id) {
            summary.blocked = summary.waiting_on.iter().any(|d| !d.resolved);
        }
    }
    Ok(())
}

fn read_forward_chunk(
    conn: &Connection,
    project: &str,
    ids: &[String],
    summaries: &mut HashMap<String, DependencySummary>,
) -> Result<(), StoreError> {
    let marks = vec!["?"; ids.len()].join(",");
    let mut statement = conn.prepare(&format!("SELECT {DIGEST_COLUMNS}, d.task_id FROM tracker_task_dependencies d
        JOIN tracker_tasks t ON t.id = d.waiting_on_task_id WHERE t.project_key = ? AND d.task_id IN ({marks}) ORDER BY d.position"))?;
    let binds = std::iter::once(project).chain(ids.iter().map(String::as_str));
    let rows = statement.query_map(rusqlite::params_from_iter(binds), |r| {
        Ok((r.get::<_, String>(5)?, digest(r)?))
    })?;
    for row in rows {
        let (id, digest) = row?;
        if let Some(summary) = summaries.get_mut(&id) {
            summary.waiting_on.push(digest);
        }
    }
    Ok(())
}

impl ReadTransaction<'_> {
    pub fn waiting_on_summary(
        &self,
        project: &str,
        id: &str,
    ) -> Result<DependencySummary, StoreError> {
        self.waiting_on_summaries(project, &[id.to_string()])?
            .remove(id)
            .ok_or_else(|| invalid("task not found in project"))
    }
    pub fn waiting_on_summaries(
        &self,
        project: &str,
        ids: &[String],
    ) -> Result<HashMap<String, DependencySummary>, StoreError> {
        let conn = self.connection();
        let mut summaries = HashMap::new();
        for chunk in ids.chunks(500) {
            read_chunk(conn, project, chunk, &mut summaries)?;
        }
        Ok(summaries)
    }
    pub fn waiting_on_blocks(
        &self,
        project: &str,
        id: &str,
        limit: usize,
        cursor: Option<&str>,
    ) -> Result<DependentsPage, StoreError> {
        let key = self.tracker_list_key()?;
        let scope = format!(
            "{:x}",
            Sha256::digest(serde_json::json!([key, project, id]).to_string().as_bytes())
        );
        let after = parse_cursor(cursor, &scope)?;
        let conn = self.connection();
        require_task(conn, project, id)?;
        let take = limit.clamp(1, 100);
        let mut statement = conn.prepare(&format!("SELECT {DIGEST_COLUMNS} FROM tracker_task_dependencies d JOIN tracker_tasks t ON t.id = d.task_id
            WHERE d.waiting_on_task_id = ?1 AND d.task_number > ?2 AND t.project_key = ?3 ORDER BY d.task_number LIMIT ?4"))?;
        let mut blocks: Vec<_> = statement
            .query_map(
                params![id, after as i64, project, (take + 1) as i64],
                digest,
            )?
            .collect::<Result<_, _>>()?;
        let next_cursor = if blocks.len() > take {
            blocks.truncate(take);
            Some(
                URL_SAFE_NO_PAD.encode(
                    serde_json::json!([scope, blocks.last().expect("nonempty page").number])
                        .to_string(),
                ),
            )
        } else {
            None
        };
        Ok(DependentsPage {
            blocks,
            next_cursor,
        })
    }
}

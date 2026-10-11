//! Lean conversation reads: select one visible item per run, and build the
//! digest from indexed metadata rather than decoding discarded item bodies.

use super::{decode_thread_item_text, read_sequenced_rows};
use crate::store::{Store, StoreError};
use crate::thread::{
    page_activity_budget, run_items_shipped_with_activity, ActivityDigest, LastToolCall,
    PageActivity, PageCut, ThreadItem,
};
use rusqlite::{params, Connection, OptionalExtension, Statement};
use std::path::PathBuf;

impl Store {
    /// A message-counted page with exactly the newest visible row of every
    /// activity run it touches. Digests describe the whole run even when a
    /// cursor clips it. Only shipped items are read and decoded in full.
    pub fn thread_conversation_page_newest_activity(
        &self,
        agent_id: &str,
        after_sequence: Option<u64>,
        before_sequence: Option<u64>,
        limit: usize,
        newest: bool,
    ) -> Result<(PageCut<ThreadItem>, bool), StoreError> {
        let after = after_sequence.map(sql_sequence).unwrap_or(0);
        let before = before_sequence.map(sql_sequence).unwrap_or(i64::MAX);
        let connection = self.connection();
        let (floor, ceiling) = page_bounds(
            &connection,
            agent_id,
            after,
            before,
            limit.max(1),
            after_sequence.is_none() || newest,
        )?;
        let RawPage {
            items: raw,
            digests,
        } = NewestRunReader::new(&connection, agent_id, limit)?.span(floor, ceiling)?;
        let has_more = page_has_more(&connection, agent_id, &raw, after_sequence, newest)?;
        drop(connection);
        let mut items =
            decode_thread_item_text(agent_id, raw.into_iter().map(|(_, item)| item).collect())?;
        items.reverse();
        let mut digests = digests
            .into_iter()
            .map(|digest| digest.decode(agent_id))
            .collect::<Result<Vec<_>, _>>()?;
        digests.reverse();
        Ok((PageCut { items, digests }, has_more))
    }
}

fn sql_sequence(sequence: u64) -> i64 {
    i64::try_from(sequence).unwrap_or(i64::MAX)
}

/// Bounds are exclusive at the ceiling and inclusive at the floor. A forward
/// page includes the activity after its last message until the next message.
fn page_bounds(
    connection: &Connection,
    agent_id: &str,
    after: i64,
    before: i64,
    limit: usize,
    newest: bool,
) -> Result<(i64, i64), StoreError> {
    let floor = after.saturating_add(1);
    if newest {
        let message_floor: Option<i64> = connection
            .query_row(
                "SELECT sequence FROM thread_items WHERE agent_id = ?1 AND message = 1
             AND sequence > ?2 AND sequence < ?3 ORDER BY sequence DESC LIMIT 1 OFFSET ?4",
                params![agent_id, after, before, (limit - 1) as i64],
                |row| row.get(0),
            )
            .optional()?;
        return Ok((message_floor.unwrap_or(floor), before));
    }
    let next_message: Option<i64> = connection
        .query_row(
            "SELECT sequence FROM thread_items WHERE agent_id = ?1 AND message = 1
         AND sequence > ?2 AND sequence < ?3 ORDER BY sequence ASC LIMIT 1 OFFSET ?4",
            params![agent_id, after, before, limit as i64],
            |row| row.get(0),
        )
        .optional()?;
    Ok((floor, next_message.unwrap_or(before)))
}

/// Keep the existing directional meaning: backward has history below the
/// oldest shipped item, forward has news above the newest, and newest-forward
/// reports rows omitted between the cursor and the oldest shipped item.
fn page_has_more(
    connection: &Connection,
    agent_id: &str,
    raw_newest_first: &[(i64, String)],
    after: Option<u64>,
    newest: bool,
) -> Result<bool, StoreError> {
    let Some(&(latest, _)) = raw_newest_first.first() else {
        return Ok(false);
    };
    let oldest = raw_newest_first.last().unwrap().0;
    let sql = match (after, newest) {
        (Some(_), true) => "SELECT 1 FROM thread_items WHERE agent_id = ?1 AND sequence < ?2 AND sequence > ?3 LIMIT 1",
        (Some(_), false) => "SELECT 1 FROM thread_items WHERE agent_id = ?1 AND sequence > ?2 AND sequence > ?3 LIMIT 1",
        (None, _) => "SELECT 1 FROM thread_items WHERE agent_id = ?1 AND sequence < ?2 AND sequence > ?3 LIMIT 1",
    };
    let edge = if after.is_some() && !newest {
        latest
    } else {
        oldest
    };
    Ok(connection
        .query_row(
            sql,
            params![agent_id, edge, after.map(sql_sequence).unwrap_or(0)],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

struct RawPage {
    items: Vec<(i64, String)>,
    digests: Vec<RawActivityDigest>,
}

struct RawActivityDigest {
    from_sequence: u64,
    through_sequence: u64,
    rows: u64,
    tool_calls: u64,
    last_tool_call: Option<String>,
}

impl RawActivityDigest {
    fn decode(self, agent_id: &str) -> Result<ActivityDigest, StoreError> {
        let last_tool_call: Option<LastToolCall> = self
            .last_tool_call
            .map(|raw| serde_json::from_str(&raw))
            .transpose()
            .map_err(|source| StoreError::Corrupt {
                path: PathBuf::from(format!("thread_items/{agent_id}/last_tool_call")),
                source,
            })?;
        Ok(ActivityDigest {
            from_sequence: self.from_sequence,
            through_sequence: self.through_sequence,
            rows: self.rows,
            tool_calls: self.tool_calls,
            last_tool_call,
        })
    }
}

struct NewestRunReader<'a> {
    agent_id: &'a str,
    structure: Statement<'a>,
    newest: Statement<'a>,
    boundaries: Statement<'a>,
    census: Statement<'a>,
    tool_count: Statement<'a>,
    last_call: Statement<'a>,
    activity_left: usize,
}

impl<'a> NewestRunReader<'a> {
    fn new(
        connection: &'a Connection,
        agent_id: &'a str,
        limit: usize,
    ) -> Result<Self, StoreError> {
        Ok(Self {
            agent_id,
            structure: connection.prepare(super::THREAD_CONVERSATION_STRUCTURE_SQL)?,
            newest: connection.prepare(super::THREAD_ACTIVITY_RANGE_SQL)?,
            boundaries: connection.prepare(crate::store::schema::THREAD_RUN_BOUNDARIES_SQL)?,
            census: connection.prepare(
                "SELECT MIN(sequence), MAX(sequence), COUNT(*) FROM thread_items
                 WHERE agent_id = ?1 AND activity = 1 AND sequence > ?2 AND sequence < ?3",
            )?,
            tool_count: connection.prepare(
                "SELECT COUNT(*) FROM thread_items WHERE agent_id = ?1 AND tool_call = 1
                 AND sequence > ?2 AND sequence < ?3",
            )?,
            last_call: connection.prepare(
                "SELECT json_object('sequence', sequence,
                 'created_at', json_extract(item, '$.data.created_at'),
                 'summary', json_extract(item, '$.data.summary'),
                 'outcome', json_extract(item, '$.data.outcome'))
                 FROM thread_items WHERE agent_id = ?1 AND tool_call = 1
                 AND sequence > ?2 AND sequence < ?3 ORDER BY sequence DESC LIMIT 1",
            )?,
            activity_left: page_activity_budget(limit),
        })
    }

    fn span(mut self, floor: i64, before: i64) -> Result<RawPage, StoreError> {
        let structure =
            read_sequenced_rows(&mut self.structure, params![self.agent_id, before, floor])?;
        let mut raw = Vec::with_capacity(structure.len());
        let mut digests = Vec::new();
        let mut above = before;
        for (sequence, item) in structure {
            self.append_run(sequence, above, &mut raw, &mut digests)?;
            raw.push((sequence, item));
            above = sequence;
        }
        self.append_run(floor.saturating_sub(1), above, &mut raw, &mut digests)?;
        Ok(RawPage {
            items: raw,
            digests,
        })
    }

    fn append_run(
        &mut self,
        after: i64,
        before: i64,
        raw: &mut Vec<(i64, String)>,
        digests: &mut Vec<RawActivityDigest>,
    ) -> Result<(), StoreError> {
        let ships =
            run_items_shipped_with_activity(usize::MAX, self.activity_left, PageActivity::Newest);
        let visible = self
            .newest
            .query_row(params![self.agent_id, after, before, ships as i64], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })
            .optional()?;
        let Some((sequence, item)) = visible else {
            return Ok(());
        };
        let (from, through): (i64, i64) = self
            .boundaries
            .query_row(params![self.agent_id, sequence], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })?;
        let bounds = params![self.agent_id, from, through];
        let (first, last, rows): (i64, i64, i64) = self
            .census
            .query_row(bounds, |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
        let tool_calls: i64 = self.tool_count.query_row(bounds, |row| row.get(0))?;
        let last_tool_call = self
            .last_call
            .query_row(bounds, |row| row.get(0))
            .optional()?;
        self.activity_left = self.activity_left.saturating_sub(ships);
        raw.push((sequence, item));
        digests.push(RawActivityDigest {
            from_sequence: first as u64,
            through_sequence: last as u64,
            rows: rows as u64,
            tool_calls: tool_calls as u64,
            last_tool_call,
        });
        Ok(())
    }
}

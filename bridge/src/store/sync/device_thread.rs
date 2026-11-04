//! Bounded transcript deltas and generation-fenced summary metadata.
use crate::store::StoreError;
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde_json::Value;
use std::collections::BTreeMap;

pub(super) const ITEM_FIELDS:&str="attention,message,handoff,json_object('data',json_object('created_at',json_extract(item,'$.data.created_at'),'body',substr(json_extract(item,'$.data.body'),1,256),'summary',substr(json_extract(item,'$.data.summary'),1,256),'still_working',json_extract(item,'$.data.still_working'),'from_build',json_extract(item,'$.data.from_build'),'event',json_extract(item,'$.data.event'),'outcome',json_extract(item,'$.data.outcome'),'done',json_extract(item,'$.data.done')))";
pub(super) type ItemMeta = (i64, bool, bool, String);

pub(super) fn install(conn: &Connection) -> Result<(), StoreError> {
    conn.execute_batch("CREATE TABLE IF NOT EXISTS device_conversation_generations(conversation_id TEXT PRIMARY KEY,generation INTEGER NOT NULL) WITHOUT ROWID;
        CREATE TABLE IF NOT EXISTS device_attention_counts(conversation_id TEXT NOT NULL,generation INTEGER NOT NULL DEFAULT 0,node INTEGER NOT NULL,total INTEGER NOT NULL,PRIMARY KEY(conversation_id,generation,node)) WITHOUT ROWID;
        CREATE TABLE IF NOT EXISTS device_summary_items(conversation_id TEXT NOT NULL,generation INTEGER NOT NULL DEFAULT 0,sequence INTEGER NOT NULL,own INTEGER NOT NULL,attention INTEGER NOT NULL,summary TEXT,reason TEXT,PRIMARY KEY(conversation_id,generation,sequence)) WITHOUT ROWID;
        CREATE INDEX IF NOT EXISTS device_summary_own ON device_summary_items(conversation_id,generation,sequence) WHERE own=1;
        CREATE INDEX IF NOT EXISTS device_summary_attention ON device_summary_items(conversation_id,generation,sequence) WHERE attention=1;
        CREATE TABLE IF NOT EXISTS device_attention_gc(conversation_id TEXT NOT NULL,generation INTEGER NOT NULL,PRIMARY KEY(conversation_id,generation)) WITHOUT ROWID;
        CREATE TABLE IF NOT EXISTS device_conversation_sessions(conversation_id TEXT PRIMARY KEY,session_started_ms INTEGER,last_activity_ms INTEGER) WITHOUT ROWID;
        CREATE TEMP TABLE device_item_touches(conversation_id TEXT NOT NULL,sequence INTEGER NOT NULL,old_attention INTEGER NOT NULL,PRIMARY KEY(conversation_id,sequence)) WITHOUT ROWID;")?;
    Ok(())
}
fn installed(conn: &Connection) -> Result<bool, StoreError> {
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_temp_schema WHERE name='device_item_touches')",
        [],
        |row| row.get(0),
    )?)
}
pub(in crate::store) fn before_item(
    conn: &Connection,
    id: &str,
    seq: i64,
) -> Result<(), StoreError> {
    if installed(conn)? {
        conn.prepare_cached("INSERT OR IGNORE INTO temp.device_item_touches VALUES(?1,?2,COALESCE((SELECT attention FROM thread_items WHERE agent_id=?1 AND sequence=?2),0))")?.execute(params![id,seq])?;
    }
    Ok(())
}
pub(super) fn generation(conn: &Connection, id: &str) -> Result<i64, StoreError> {
    Ok(conn
        .prepare_cached(
            "SELECT generation FROM device_conversation_generations WHERE conversation_id=?1",
        )?
        .query_row([id], |row| row.get(0))
        .optional()?
        .unwrap_or(0))
}
pub(super) fn flush(tx: &Transaction<'_>) -> Result<(), StoreError> {
    let rows=tx.prepare_cached("SELECT conversation_id,sequence,old_attention FROM temp.device_item_touches ORDER BY conversation_id,sequence")?
        .query_map([],|row|Ok((row.get::<_,String>(0)?,row.get::<_,i64>(1)?,row.get::<_,i64>(2)?)))?.collect::<Result<Vec<_>,_>>()?;
    let mut deltas = BTreeMap::new();
    for (id, seq, old) in rows {
        let current = tx
            .prepare_cached(&format!(
                "SELECT {ITEM_FIELDS} FROM thread_items WHERE agent_id=?1 AND sequence=?2"
            ))?
            .query_row(params![id, seq], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, bool>(1)?,
                    row.get::<_, bool>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })
            .optional()?;
        add_nodes(
            &mut deltas,
            &id,
            seq,
            current.as_ref().map_or(0, |row| row.0) - old,
        );
        observe(tx, &id, seq, current.as_ref())?;
        super::device_producers::touch_conversation(tx, &id)?;
    }
    apply_nodes(tx, deltas)?;
    tx.execute("DELETE FROM temp.device_item_touches", [])?;
    Ok(())
}
pub(super) fn backfill_item(
    tx: &Transaction<'_>,
    id: &str,
    seq: i64,
    meta: &ItemMeta,
) -> Result<(), StoreError> {
    let mut nodes = BTreeMap::new();
    add_nodes(&mut nodes, id, seq, meta.0);
    apply_nodes(tx, nodes)?;
    observe(tx, id, seq, Some(meta))
}
fn add_nodes(nodes: &mut BTreeMap<(String, i64), i64>, id: &str, mut node: i64, delta: i64) {
    if delta == 0 {
        return;
    }
    while node > 0 {
        *nodes.entry((id.to_owned(), node)).or_default() += delta;
        let Some(next) = node.checked_add(node.isolate_lowest_one()) else {
            break;
        };
        node = next;
    }
}
fn apply_nodes(
    tx: &Transaction<'_>,
    nodes: BTreeMap<(String, i64), i64>,
) -> Result<(), StoreError> {
    let mut generations = BTreeMap::new();
    for ((id, node), delta) in nodes {
        if delta == 0 {
            continue;
        }
        let current = match generations.entry(id.clone()) {
            std::collections::btree_map::Entry::Occupied(entry) => *entry.get(),
            std::collections::btree_map::Entry::Vacant(entry) => {
                *entry.insert(generation(tx, &id)?)
            }
        };
        tx.prepare_cached("INSERT INTO device_attention_counts VALUES(?1,?2,?3,?4) ON CONFLICT(conversation_id,generation,node) DO UPDATE SET total=total+?4")?.execute(params![id,current,node,delta])?;
    }
    Ok(())
}
fn prefix(conn: &Connection, id: &str, current: i64, mut node: i64) -> Result<i64, StoreError> {
    let mut count = 0;
    while node > 0 {
        count+=conn.prepare_cached("SELECT total FROM device_attention_counts WHERE conversation_id=?1 AND generation=?2 AND node=?3")?.query_row(params![id,current,node],|row|row.get::<_,i64>(0)).optional()?.unwrap_or(0);
        node -= node.isolate_lowest_one();
    }
    Ok(count)
}
pub(super) fn unread(conn: &Connection, id: &str, read: u64) -> Result<i64, StoreError> {
    let current = generation(conn, id)?;
    Ok((prefix(conn, id, current, i64::MAX)?
        - prefix(conn, id, current, i64::try_from(read).unwrap_or(i64::MAX))?)
    .max(0))
}
fn flag(value: &Value, key: &str) -> bool {
    value[key].as_bool() == Some(true) || value[key].as_i64() == Some(1)
}
fn observe(
    tx: &Transaction<'_>,
    id: &str,
    seq: i64,
    meta: Option<&ItemMeta>,
) -> Result<(), StoreError> {
    let current = generation(tx, id)?;
    let Some((attention, message, handoff, raw)) = meta else {
        return remove_summary(tx, id, current, seq);
    };
    let value: Value = serde_json::from_str(raw).map_err(|source| StoreError::Corrupt {
        path: format!("thread_items/{id}").into(),
        source,
    })?;
    let data = &value["data"];
    if flag(data, "still_working") || (*attention == 0 && !*message) {
        return remove_summary(tx, id, current, seq);
    }
    let summary = data["body"]
        .as_str()
        .or_else(|| data["summary"].as_str())
        .map(|text| text.chars().take(256).collect::<String>());
    let reason = (*attention != 0).then(|| attention_reason(data, *message));
    tx.prepare_cached("INSERT INTO device_summary_items VALUES(?1,?2,?3,?4,?5,?6,?7) ON CONFLICT(conversation_id,generation,sequence) DO UPDATE SET own=?4,attention=?5,summary=?6,reason=?7")?
        .execute(params![id,current,seq,*message && !*handoff,*attention,summary,reason])?;
    if *message && !flag(data, "from_build") {
        update_session(tx, id, data)?;
    }
    Ok(())
}
fn remove_summary(
    tx: &Transaction<'_>,
    id: &str,
    current: i64,
    seq: i64,
) -> Result<(), StoreError> {
    tx.prepare_cached("DELETE FROM device_summary_items WHERE conversation_id=?1 AND generation=?2 AND sequence=?3")?.execute(params![id,current,seq])?;
    Ok(())
}
fn attention_reason(data: &Value, message: bool) -> String {
    if !message {
        return data["event"].as_str().unwrap_or("attention").into();
    }
    match data["outcome"].as_str() {
        Some("completed") => "done",
        Some("blocked") => "blocked",
        Some("failed") => "run_failed",
        _ if flag(data, "done") => "done",
        _ => "agent_message",
    }
    .into()
}
fn update_session(tx: &Transaction<'_>, id: &str, data: &Value) -> Result<(), StoreError> {
    let Some(at) = data["created_at"]
        .as_str()
        .and_then(crate::session_summary::message_millis)
    else {
        return Ok(());
    };
    let current=tx.prepare_cached("SELECT session_started_ms,last_activity_ms FROM device_conversation_sessions WHERE conversation_id=?1")?.query_row([id],|row|Ok(crate::session_summary::SessionSummary{session_started_ms:row.get(0)?,last_activity_ms:row.get(1)?})).optional()?.unwrap_or_default();
    let next = current.updated(at);
    if current != next {
        tx.prepare_cached("INSERT INTO device_conversation_sessions VALUES(?1,?2,?3) ON CONFLICT(conversation_id) DO UPDATE SET session_started_ms=?2,last_activity_ms=?3")?.execute(params![id,next.session_started_ms,next.last_activity_ms])?;
    }
    Ok(())
}
pub(in crate::store) fn reset(conn: &Connection, id: &str) -> Result<(), StoreError> {
    if !installed(conn)? {
        return Ok(());
    }
    let old = generation(conn, id)?;
    let next = old
        .checked_add(1)
        .ok_or_else(|| super::invalid("device counter generation exhausted"))?;
    conn.prepare_cached("INSERT INTO device_conversation_generations VALUES(?1,?2) ON CONFLICT(conversation_id) DO UPDATE SET generation=?2")?.execute(params![id,next])?;
    conn.prepare_cached("INSERT OR IGNORE INTO device_attention_gc VALUES(?1,?2)")?
        .execute(params![id, old])?;
    conn.prepare_cached("DELETE FROM device_conversation_sessions WHERE conversation_id=?1")?
        .execute([id])?;
    super::device_producers::touch_conversation(conn, id)
}
pub(super) fn gc(tx: &Transaction<'_>, limit: usize) -> Result<usize, StoreError> {
    let before = tx.total_changes();
    while (tx.total_changes() - before) as usize + 2 < limit {
        let next:Option<(String,i64)>=tx.query_row("SELECT conversation_id,generation FROM device_attention_gc ORDER BY conversation_id,generation LIMIT 1",[],|row|Ok((row.get(0)?,row.get(1)?))).optional()?;
        let Some((id, old)) = next else {
            break;
        };
        let budget = limit - (tx.total_changes() - before) as usize - 2;
        tx.execute("DELETE FROM device_attention_counts WHERE conversation_id=?1 AND generation=?2 AND node IN (SELECT node FROM device_attention_counts WHERE conversation_id=?1 AND generation=?2 ORDER BY node LIMIT ?3)",params![id,old,budget as i64])?;
        let budget = limit - (tx.total_changes() - before) as usize - 2;
        tx.execute("DELETE FROM device_summary_items WHERE conversation_id=?1 AND generation=?2 AND sequence IN (SELECT sequence FROM device_summary_items WHERE conversation_id=?1 AND generation=?2 ORDER BY sequence LIMIT ?3)",params![id,old,budget as i64])?;
        let pending:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM device_attention_counts WHERE conversation_id=?1 AND generation=?2) OR EXISTS(SELECT 1 FROM device_summary_items WHERE conversation_id=?1 AND generation=?2)",params![id,old],|row|row.get(0))?;
        if pending {
            break;
        }
        tx.execute(
            "DELETE FROM device_attention_gc WHERE conversation_id=?1 AND generation=?2",
            params![id, old],
        )?;
        tx.execute("DELETE FROM device_conversation_generations WHERE conversation_id=?1 AND NOT EXISTS(SELECT 1 FROM agents WHERE id=?1) AND NOT EXISTS(SELECT 1 FROM thread_items WHERE agent_id=?1) AND NOT EXISTS(SELECT 1 FROM device_attention_gc WHERE conversation_id=?1) AND NOT EXISTS(SELECT 1 FROM device_attention_counts WHERE conversation_id=?1) AND NOT EXISTS(SELECT 1 FROM device_summary_items WHERE conversation_id=?1)",[id])?;
    }
    Ok((tx.total_changes() - before) as usize)
}

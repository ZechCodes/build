//! Explicit, bounded transcript deltas; never recount history in a writer.
use crate::store::StoreError;
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde_json::Value;
use std::collections::BTreeMap;

pub(super) const SUMMARY_PREDICATE:&str="attention=1 OR (message=1 AND COALESCE(json_extract(item,'$.data.still_working'),0)=0)";

pub(super) fn install(conn:&Connection)->Result<(),StoreError> {
    conn.execute_batch("CREATE TABLE IF NOT EXISTS device_attention_counts(
        conversation_id TEXT NOT NULL,node INTEGER NOT NULL,total INTEGER NOT NULL,PRIMARY KEY(conversation_id,node)) WITHOUT ROWID;
        CREATE TABLE IF NOT EXISTS device_conversation_sessions(
        conversation_id TEXT PRIMARY KEY,session_started_ms INTEGER,last_activity_ms INTEGER) WITHOUT ROWID;
        CREATE TEMP TABLE device_item_touches(conversation_id TEXT NOT NULL,sequence INTEGER NOT NULL,old_attention INTEGER NOT NULL,PRIMARY KEY(conversation_id,sequence)) WITHOUT ROWID;
        CREATE INDEX IF NOT EXISTS device_thread_summary ON thread_items(agent_id,sequence)
        WHERE attention=1 OR (message=1 AND COALESCE(json_extract(item,'$.data.still_working'),0)=0);")?;
    Ok(())
}

pub(in crate::store) fn before_item(conn:&Connection,id:&str,seq:i64)->Result<(),StoreError> {
    let installed:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_temp_schema WHERE name='device_item_touches')",[],|row|row.get(0))?;
    if installed {
        conn.execute("INSERT OR IGNORE INTO temp.device_item_touches VALUES(?1,?2,COALESCE((SELECT attention FROM thread_items WHERE agent_id=?1 AND sequence=?2),0))",params![id,seq])?;
    }
    Ok(())
}

pub(super) fn flush(tx:&Transaction<'_>)->Result<(),StoreError> {
    let rows=tx.prepare("SELECT conversation_id,sequence,old_attention FROM temp.device_item_touches ORDER BY conversation_id,sequence")?
        .query_map([],|row|Ok((row.get::<_,String>(0)?,row.get::<_,i64>(1)?,row.get::<_,i64>(2)?)))?.collect::<Result<Vec<_>,_>>()?;
    let mut deltas=BTreeMap::new();
    for (id,seq,old) in rows {
        let current:Option<(i64,bool,String)>=tx.query_row("SELECT attention,message,item FROM thread_items WHERE agent_id=?1 AND sequence=?2",params![id,seq],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?))).optional()?;
        add_nodes(&mut deltas,&id,seq,current.as_ref().map_or(0,|row|row.0)-old);
        if let Some((_,true,raw))=current { update_session(tx,&id,&raw)?; }
        super::device_producers::touch_conversation(tx,&id)?;
    }
    apply_nodes(tx,deltas)?;
    tx.execute("DELETE FROM temp.device_item_touches",[])?;
    Ok(())
}

pub(super) fn backfill_item(tx:&Transaction<'_>,id:&str,seq:i64,attention:i64,message:bool,raw:&str)->Result<(),StoreError> {
    let mut nodes=BTreeMap::new();
    add_nodes(&mut nodes,id,seq,attention);
    apply_nodes(tx,nodes)?;
    if message {update_session(tx,id,raw)?;}
    Ok(())
}

fn add_nodes(nodes:&mut BTreeMap<(String,i64),i64>,id:&str,mut node:i64,delta:i64) {
    if delta==0 {return;}
    while node>0 {
        *nodes.entry((id.to_owned(),node)).or_default()+=delta;
        let Some(next)=node.checked_add(node & -node) else {break;};
        node=next;
    }
}
fn apply_nodes(tx:&Transaction<'_>,nodes:BTreeMap<(String,i64),i64>)->Result<(),StoreError> {
    for ((id,node),delta) in nodes {
        if delta!=0 {tx.execute("INSERT INTO device_attention_counts VALUES(?1,?2,?3) ON CONFLICT(conversation_id,node) DO UPDATE SET total=total+?3",params![id,node,delta])?;}
    }
    Ok(())
}
fn prefix(conn:&Connection,id:&str,mut node:i64)->Result<i64,StoreError> {
    let mut count=0;
    while node>0 {
        count+=conn.query_row("SELECT total FROM device_attention_counts WHERE conversation_id=?1 AND node=?2",params![id,node],|row|row.get::<_,i64>(0)).optional()?.unwrap_or(0);
        node-=node & -node;
    }
    Ok(count)
}
pub(super) fn unread(conn:&Connection,id:&str,read:u64)->Result<i64,StoreError> {
    Ok((prefix(conn,id,i64::MAX)?-prefix(conn,id,i64::try_from(read).unwrap_or(i64::MAX))?).max(0))
}

fn update_session(tx:&Transaction<'_>,id:&str,raw:&str)->Result<(),StoreError> {
    let value:Value=serde_json::from_str(raw).map_err(|source|StoreError::Corrupt{path:format!("thread_items/{id}").into(),source})?;
    if value["data"]["still_working"].as_bool()==Some(true) || value["data"]["still_working"].as_i64()==Some(1) {return Ok(());}
    let Some(at)=value["data"]["created_at"].as_str().and_then(crate::session_summary::message_millis) else {return Ok(());};
    let current=tx.query_row("SELECT session_started_ms,last_activity_ms FROM device_conversation_sessions WHERE conversation_id=?1",[id],|row|Ok(crate::session_summary::SessionSummary{session_started_ms:row.get(0)?,last_activity_ms:row.get(1)?})).optional()?.unwrap_or_default();
    let next=current.updated(at);
    if current!=next {
        tx.execute("INSERT INTO device_conversation_sessions VALUES(?1,?2,?3) ON CONFLICT(conversation_id) DO UPDATE SET session_started_ms=?2,last_activity_ms=?3",params![id,next.session_started_ms,next.last_activity_ms])?;
    }
    Ok(())
}

pub(in crate::store) fn reset(conn:&Connection,id:&str)->Result<(),StoreError> {
    let installed:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name='device_attention_counts')",[],|row|row.get(0))?;
    if !installed {return Ok(());}
    conn.execute("DELETE FROM device_attention_counts WHERE conversation_id=?1",[id])?;
    conn.execute("DELETE FROM device_conversation_sessions WHERE conversation_id=?1",[id])?;
    super::device_producers::touch_conversation(conn,id)
}

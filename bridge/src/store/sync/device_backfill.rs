//! No lease is admitted until bounded, restart-safe SQL backfill has finished.
use super::{Entity, PublicationBatch};
use crate::store::StoreError;
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

const CHUNK:i64=128;
#[derive(Default,Deserialize,Serialize)]
struct Position {phase:u8,key:String,seq:i64}

pub(super) fn install(conn:&mut Connection)->Result<(),StoreError> {
    let mut lifecycle=None;
    loop {
        let raw:Option<String>=conn.query_row("SELECT value FROM meta WHERE key='device_backfill_position'",[],|row|row.get(0)).optional()?;
        let phase=raw.as_deref().map(serde_json::from_str::<Position>).transpose().map_err(|source|StoreError::Corrupt{path:"device_backfill_position".into(),source})?.unwrap_or_default().phase;
        if phase==9 && lifecycle.is_none() {
            let raw:Option<String>=conn.query_row("SELECT value FROM meta WHERE key='workspace_lifecycle'",[],|row|row.get(0)).optional()?;
            lifecycle=Some(raw.map(|raw|serde_json::from_str::<BTreeMap<String,serde_json::Value>>(&raw).map_err(|source|StoreError::Corrupt{path:"workspace_lifecycle".into(),source})).transpose()?.unwrap_or_default());
        }
        if chunk(conn,lifecycle.as_ref())? {break;}
    }
    Ok(())
}
fn chunk(conn:&mut Connection,lifecycle:Option<&BTreeMap<String,serde_json::Value>>)->Result<bool,StoreError> {
    let tx=conn.transaction()?;
    let raw:Option<String>=tx.query_row("SELECT value FROM meta WHERE key='device_backfill_position'",[],|row|row.get(0)).optional()?;
    let mut position:Position=raw.map(|raw|serde_json::from_str(&raw).map_err(|source|StoreError::Corrupt{path:"device_backfill_position".into(),source})).transpose()?.unwrap_or_default();
    let mut batch=PublicationBatch::default();
    let done=step(&tx,&mut position,&mut batch,lifecycle)?;
    super::device_producers::collect(&tx,&mut batch)?;
    batch.publish(&tx,super::now_ms())?;
    tx.execute("INSERT INTO meta(key,value) VALUES('device_backfill_position',?1) ON CONFLICT(key) DO UPDATE SET value=?1",[serde_json::to_string(&position).expect("backfill position")])?;
    tx.commit()?;
    Ok(done)
}
fn advance(position:&mut Position) {position.phase+=1;position.key.clear();position.seq=0;}
fn step(tx:&Transaction<'_>,position:&mut Position,batch:&mut PublicationBatch,lifecycle:Option<&BTreeMap<String,serde_json::Value>>)->Result<bool,StoreError> {
    match position.phase {
        0=>{
            let removed=tx.execute("DELETE FROM device_attention_counts WHERE (conversation_id,generation,node) IN (SELECT conversation_id,generation,node FROM device_attention_counts LIMIT 128)",[])?;
            let summaries=tx.execute("DELETE FROM device_summary_items WHERE (conversation_id,generation,sequence) IN (SELECT conversation_id,generation,sequence FROM device_summary_items LIMIT ?1)",[(128-removed) as i64])?;
            if removed+summaries==0 {advance(position);}
        },
        1=>{if tx.execute("DELETE FROM device_conversation_sessions WHERE conversation_id IN (SELECT conversation_id FROM device_conversation_sessions LIMIT 128)",[])?==0 {advance(position);}},
        2=>backfill_threads(tx,position)?,
        3=>backfill_facts(tx,position)?,
        4=>backfill_agents(tx,position,batch)?,
        5=>backfill_captures(tx,position,batch)?,
        6=>backfill_tasks(tx,position,batch)?,
        7=>{backfill_session(tx,batch)?;advance(position);},
        8=>backfill_archives(tx,position,batch)?,
        9=>backfill_lifecycle(tx,position,batch,lifecycle)?,
        10=>backfill_retained(tx,position,false)?,
        11=>backfill_retained(tx,position,true)?,
        12=>return Ok(true),
        _=>return Err(super::invalid("invalid device backfill phase")),
    }
    Ok(false)
}
fn backfill_facts(tx:&Transaction<'_>,position:&mut Position)->Result<(),StoreError> {
    let ids=ids(tx,"agents","id",&position.key)?;
    for id in &ids {super::device_facts::backfill(tx,id)?;}
    if let Some(id)=ids.last() {position.key=id.clone();} else {advance(position);}
    Ok(())
}

fn backfill_threads(tx:&Transaction<'_>,position:&mut Position)->Result<(),StoreError> {
    let rows=tx.prepare(&format!("SELECT agent_id,sequence,{} FROM thread_items WHERE (agent_id,sequence)>(?1,?2) ORDER BY agent_id,sequence LIMIT ?3",super::device_thread::ITEM_FIELDS))?
        .query_map(params![position.key,position.seq,CHUNK],|row|Ok((row.get::<_,String>(0)?,row.get::<_,i64>(1)?,(row.get::<_,i64>(2)?,row.get::<_,bool>(3)?,row.get::<_,bool>(4)?,row.get::<_,String>(5)?))))?.collect::<Result<Vec<_>,_>>()?;
    for (id,seq,meta) in &rows {super::device_thread::backfill_item(tx,id,*seq,meta)?;}
    if let Some((id,seq,_))=rows.last() {position.key=id.clone();position.seq=*seq;} else {advance(position);}
    Ok(())
}
fn ids(tx:&Transaction<'_>,table:&str,column:&str,after:&str)->Result<Vec<String>,StoreError> {
    Ok(tx.prepare(&format!("SELECT {column} FROM {table} WHERE {column}>?1 ORDER BY {column} LIMIT ?2"))?
        .query_map(params![after,CHUNK],|row|row.get(0))?.collect::<Result<Vec<_>,_>>()?)
}
fn repair_missing(tx:&Transaction<'_>,batch:&mut PublicationBatch,kind:&str,id:&str)->Result<(),StoreError> {
    let exists:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM sync_projection WHERE kind=?1 AND key=?2)",params![kind,id],|row|row.get(0))?;
    if !exists {batch.force(&Entity::new(kind,id));}
    Ok(())
}
fn backfill_agents(tx:&Transaction<'_>,position:&mut Position,batch:&mut PublicationBatch)->Result<(),StoreError> {
    let ids=ids(tx,"agents","id",&position.key)?;
    for id in &ids {super::device_producers::touch_agent(tx,id)?;}
    super::device_producers::collect(tx,batch)?;
    for id in &ids {for kind in ["agent","identity","inbox_conversation"] {repair_missing(tx,batch,kind,id)?;}}
    if let Some(id)=ids.last() {position.key=id.clone();} else {advance(position);}
    Ok(())
}
fn backfill_captures(tx:&Transaction<'_>,position:&mut Position,batch:&mut PublicationBatch)->Result<(),StoreError> {
    let ids=ids(tx,"captures","id",&position.key)?;
    for id in &ids {
        let raw:String=tx.query_row("SELECT record FROM captures WHERE id=?1",[id],|row|row.get(0))?;
        let capture=serde_json::from_str(&raw).map_err(|source|StoreError::Corrupt{path:format!("captures/{id}").into(),source})?;
        super::stage_device(tx,batch,Entity::new("inbox_capture",id),Some(crate::store::entities::device_capture(&capture)))?;
        repair_missing(tx,batch,"inbox_capture",id)?;
    }
    if let Some(id)=ids.last() {position.key=id.clone();} else {advance(position);}
    Ok(())
}
fn backfill_tasks(tx:&Transaction<'_>,position:&mut Position,batch:&mut PublicationBatch)->Result<(),StoreError> {
    let ids=ids(tx,"tracker_task_summaries","task_id",&position.key)?;
    for id in &ids {
        super::stage_device(tx,batch,Entity::new("inbox_task",id),super::task_publication::inbox_projection(tx,id)?)?;
        repair_missing(tx,batch,"inbox_task",id)?;
    }
    if let Some(id)=ids.last() {position.key=id.clone();} else {advance(position);}
    Ok(())
}
fn backfill_session(tx:&Transaction<'_>,batch:&mut PublicationBatch)->Result<(),StoreError> {
    let raw:Option<String>=tx.query_row("SELECT value FROM meta WHERE key='user_session'",[],|row|row.get(0)).optional()?;
    if let Some(raw)=raw {
        let value=serde_json::from_str(&raw).map_err(|source|StoreError::Corrupt{path:"meta/user_session".into(),source})?;
        super::stage_device(tx,batch,Entity::new("user_session","current"),Some(value))?;
        repair_missing(tx,batch,"user_session","current")?;
    }
    Ok(())
}

fn backfill_archives(tx:&Transaction<'_>,position:&mut Position,batch:&mut PublicationBatch)->Result<(),StoreError> {
    let ids=ids(tx,"archived_worktrees","id",&position.key)?;
    for id in &ids {super::device_families::stage_archive(tx,batch,id)?;repair_missing(tx,batch,"archived_worktree",id)?;}
    if let Some(id)=ids.last() {position.key=id.clone();} else {advance(position);}
    Ok(())
}
fn backfill_lifecycle(tx:&Transaction<'_>,position:&mut Position,batch:&mut PublicationBatch,records:Option<&BTreeMap<String,serde_json::Value>>)->Result<(),StoreError> {
    let records=records.ok_or_else(||super::invalid("missing lifecycle backfill source"))?;
    let rows=records.range((std::ops::Bound::Excluded(position.key.clone()),std::ops::Bound::Unbounded)).take(CHUNK as usize).collect::<Vec<_>>();
    for (id,value) in &rows {super::device_families::stage_lifecycle(tx,batch,id,value)?;repair_missing(tx,batch,"workspace_lifecycle",id)?;}
    if let Some((id,_))=rows.last() {position.key=(*id).clone();} else {advance(position);}
    Ok(())
}
fn backfill_retained(tx:&Transaction<'_>,position:&mut Position,project:bool)->Result<(),StoreError> {
    let (table,column)=if project {("inbox_retained_messages","project_id")} else {("inbox_retained_run_messages","run_id")};
    for _ in 0..CHUNK {
        let next:Option<String>=tx.query_row(&format!("SELECT {column} FROM {table} WHERE {column}>?1 ORDER BY {column} LIMIT 1"),[&position.key],|row|row.get(0)).optional()?;
        let Some(id)=next else {advance(position);break;};
        let last:i64=tx.query_row(&format!("SELECT ts_ms FROM {table} WHERE {column}=?1 ORDER BY ts_ms DESC LIMIT 1"),[&id],|row|row.get(0))?;
        super::device_families::retain_time(tx,if project {"project"} else {"owner"},&id,last)?;
        position.key=id;
    }
    Ok(())
}

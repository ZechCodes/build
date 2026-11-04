//! Restart-safe, bounded bootstrap of pooled session authority. Timestamp
//! parsing precedes ordered replay so RFC3339 offsets cannot reorder activity.
use crate::store::StoreError;
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
const CHUNK: i64 = 128;
#[derive(Deserialize, Serialize)]
struct Position {
    phase: u8,
    key: String,
    seq: i64,
    at: i64,
    source: i64,
}
impl Default for Position {
    fn default() -> Self {
        Self {
            phase: 0,
            key: String::new(),
            seq: 0,
            at: i64::MIN,
            source: -1,
        }
    }
}
#[derive(Debug)]
struct Point {
    source: i64,
    key: String,
    seq: i64,
    at: i64,
    owner: Option<String>,
    project: Option<String>,
}

pub(super) fn install(conn: &Connection) -> Result<(), StoreError> {
    conn.execute_batch("CREATE TABLE IF NOT EXISTS device_session_replay(source INTEGER NOT NULL,source_key TEXT NOT NULL,sequence INTEGER NOT NULL,at_ms INTEGER NOT NULL,owner_id TEXT,project_key TEXT,PRIMARY KEY(source,source_key,sequence)) WITHOUT ROWID;
        CREATE INDEX IF NOT EXISTS device_session_replay_order ON device_session_replay(at_ms,source,source_key,sequence);")?;
    Ok(())
}
fn advance(position: &mut Position) {
    position.phase += 1;
    position.key.clear();
    position.seq = 0;
    position.at = i64::MIN;
    position.source = -1;
}
fn save_point(tx: &Transaction<'_>, point: Point) -> Result<(), StoreError> {
    tx.prepare_cached("INSERT OR IGNORE INTO device_session_replay VALUES(?1,?2,?3,?4,?5,?6)")?
        .execute(params![
            point.source,
            point.key,
            point.seq,
            point.at,
            point.owner,
            point.project
        ])?;
    Ok(())
}
fn threads(tx: &Transaction<'_>, position: &mut Position) -> Result<(), StoreError> {
    // Limit candidates before JSON predicates and owner joins. A long prefix
    // of Build-authored or orphaned messages must still checkpoint every128.
    let rows=tx.prepare_cached("WITH candidates AS MATERIALIZED (SELECT agent_id,sequence,json_extract(item,'$.data.created_at') AS at,COALESCE(json_extract(item,'$.data.from_build'),0) AS build FROM thread_items INDEXED BY thread_items_messages WHERE (agent_id,sequence)>(?1,?2) AND message=1 ORDER BY agent_id,sequence LIMIT ?3) SELECT t.agent_id,t.sequence,t.at,a.owner_id,o.project_path,t.build FROM candidates t LEFT JOIN agents a ON a.id=t.agent_id LEFT JOIN device_owner_projects o ON o.owner_id=a.owner_id ORDER BY t.agent_id,t.sequence")?
        .query_map(params![position.key,position.seq,CHUNK],|row| Ok((row.get::<_,String>(0)?,row.get::<_,i64>(1)?,row.get::<_,Option<String>>(2)?,row.get::<_,Option<String>>(3)?,row.get::<_,Option<String>>(4)?,row.get::<_,bool>(5)?)))?
        .collect::<Result<Vec<_>,_>>()?;
    for (key, seq, raw, owner, project, build) in &rows {
        if *build {
            continue;
        }
        let Some(owner) = owner else {
            continue;
        };
        if let Some(at) = raw
            .as_deref()
            .and_then(crate::session_summary::message_millis)
        {
            save_point(
                tx,
                Point {
                    source: 0,
                    key: key.clone(),
                    seq: *seq,
                    at,
                    owner: Some(owner.clone()),
                    project: project.clone(),
                },
            )?;
        }
    }
    if let Some((key, seq, _, _, _, _)) = rows.last() {
        position.key = key.clone();
        position.seq = *seq;
    } else {
        advance(position);
    }
    Ok(())
}

fn retained(
    tx: &Transaction<'_>,
    position: &mut Position,
    project: bool,
) -> Result<(), StoreError> {
    let (table, column) = if project {
        ("inbox_retained_messages", "project_id")
    } else {
        ("inbox_retained_run_messages", "run_id")
    };
    let rows = tx
        .prepare_cached(&format!(
            "SELECT rowid,{column},ts_ms FROM {table} WHERE rowid>?1 ORDER BY rowid LIMIT ?2"
        ))?
        .query_map(params![position.seq, CHUNK], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    for (seq, key, at) in &rows {
        let (owner, path) = if project {
            (None, Some(super::device_sessions::project_key(tx, key)?))
        } else {
            (
                Some(key.clone()),
                tx.prepare_cached(
                    "SELECT project_path FROM device_owner_projects WHERE owner_id=?1",
                )?
                .query_row([key], |row| row.get(0))
                .optional()?,
            )
        };
        save_point(
            tx,
            Point {
                source: if project { 2 } else { 1 },
                key: key.clone(),
                seq: *seq,
                at: *at,
                owner,
                project: path,
            },
        )?;
    }
    if let Some((seq, _, _)) = rows.last() {
        position.seq = *seq;
    } else {
        advance(position);
    }
    Ok(())
}
fn replay(tx: &Transaction<'_>, position: &mut Position) -> Result<(), StoreError> {
    let rows=tx.prepare_cached("SELECT source,source_key,sequence,at_ms,owner_id,project_key FROM device_session_replay WHERE (at_ms,source,source_key,sequence)>(?1,?2,?3,?4) ORDER BY at_ms,source,source_key,sequence LIMIT ?5")?
        .query_map(params![position.at,position.source,position.key,position.seq,CHUNK],|row| Ok(Point { source:row.get(0)?,key:row.get(1)?,seq:row.get(2)?,at:row.get(3)?,owner:row.get(4)?,project:row.get(5)? }))?.collect::<Result<Vec<_>,_>>()?;
    for point in &rows {
        if let Some(owner) = &point.owner {
            super::device_sessions::update(tx, "owner", owner, point.at)?;
        }
        if let Some(project) = &point.project {
            super::device_sessions::update(tx, "project", project, point.at)?;
        }
    }
    if let Some(last) = rows.last() {
        position.at = last.at;
        position.source = last.source;
        position.key = last.key.clone();
        position.seq = last.seq;
    } else {
        advance(position);
    }
    Ok(())
}
fn mapping(tx: &Transaction<'_>, position: &mut Position) -> Result<(), StoreError> {
    let paths = tx
        .prepare_cached("SELECT path FROM device_project_ids WHERE path>?1 ORDER BY path LIMIT ?2")?
        .query_map(params![position.key, CHUNK], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    for path in &paths {
        super::device_sessions::project_mapping_changed(tx, path)?;
    }
    if let Some(last) = paths.last() {
        position.key = last.clone();
    } else {
        advance(position);
    }
    Ok(())
}
fn step(tx: &Transaction<'_>, position: &mut Position) -> Result<(), StoreError> {
    match position.phase {
        0=>threads(tx,position)?,
        1=>retained(tx,position,false)?,
        2=>retained(tx,position,true)?,
        3=>replay(tx,position)?,
        4=>{ if tx.execute("DELETE FROM device_session_replay WHERE (source,source_key,sequence) IN (SELECT source,source_key,sequence FROM device_session_replay LIMIT 128)",[])?==0 { advance(position); } },
        5=>mapping(tx,position)?,
        _=>return Err(super::invalid("invalid device session bootstrap phase")),
    }
    Ok(())
}
pub(super) fn upgrade(conn: &mut Connection) -> Result<(), StoreError> {
    loop {
        let raw: Option<String> = conn
            .query_row(
                "SELECT value FROM meta WHERE key='device_sessions_334_position'",
                [],
                |row| row.get(0),
            )
            .optional()?;
        let mut position = raw
            .map(|raw| {
                serde_json::from_str::<Position>(&raw).map_err(|source| StoreError::Corrupt {
                    path: "device_sessions_334_position".into(),
                    source,
                })
            })
            .transpose()?
            .unwrap_or_default();
        if position.phase == 6 {
            return Ok(());
        }
        let tx = conn.transaction()?;
        step(&tx, &mut position)?;
        let mut batch = super::PublicationBatch::default();
        super::device_producers::collect(&tx, &mut batch)?;
        batch.publish(&tx, super::now_ms())?;
        tx.execute("INSERT INTO meta(key,value) VALUES('device_sessions_334_position',?1) ON CONFLICT(key) DO UPDATE SET value=?1",[serde_json::to_string(&position).expect("session bootstrap position")])?;
        tx.commit()?;
    }
}

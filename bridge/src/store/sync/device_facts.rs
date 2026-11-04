//! Compact directory facts, captured from owned records without their history.
use crate::{agent::Agent, models::ModelChoice, store::StoreError};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize)]
pub(super) struct AgentFacts {
    pub id: String,
    pub owner_id: String,
    pub conversation_id: String,
    pub choice: ModelChoice,
    pub choice_revision: u64,
    pub ordinal: u32,
    pub created_at: String,
    pub topic: Option<String>,
    pub name: Option<String>,
    pub created_by: Option<String>,
    pub watched: bool,
    pub watch_started_after: u64,
    pub thread_id: String,
    pub thread_generation_revision: String,
}
impl AgentFacts {
    pub fn conversation_id(&self) -> &str {
        &self.conversation_id
    }
}
fn display(text: &Option<String>) -> Option<String> {
    text.as_ref().map(|text| text.chars().take(256).collect())
}
pub(super) fn install(conn: &Connection) -> Result<(), StoreError> {
    conn.execute_batch("CREATE TABLE IF NOT EXISTS device_agent_facts(agent_id TEXT PRIMARY KEY,record TEXT NOT NULL) WITHOUT ROWID;
        CREATE TABLE IF NOT EXISTS device_owner_projects(owner_id TEXT PRIMARY KEY,project_path TEXT NOT NULL,workspace_path TEXT) WITHOUT ROWID;
        CREATE INDEX IF NOT EXISTS device_owners_project ON device_owner_projects(project_path,owner_id);
        CREATE INDEX IF NOT EXISTS device_owners_workspace ON device_owner_projects(workspace_path,owner_id);
        CREATE TABLE IF NOT EXISTS device_workspace_roots(root TEXT PRIMARY KEY,workspace_id TEXT NOT NULL) WITHOUT ROWID;")?;
    Ok(())
}
fn installed(conn: &Connection) -> Result<bool, StoreError> {
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name='device_agent_facts')",
        [],
        |row| row.get(0),
    )?)
}
pub(in crate::store) fn seed_agent(
    conn: &Connection,
    owner: &str,
    agent: &Agent,
) -> Result<(), StoreError> {
    if !installed(conn)? {
        return Ok(());
    }
    let facts = AgentFacts {
        id: agent.id.clone(),
        owner_id: owner.to_owned(),
        conversation_id: agent.conversation_id().to_owned(),
        choice: agent.choice.clone(),
        choice_revision: agent.choice_revision,
        ordinal: agent.ordinal,
        created_at: agent.created_at.clone(),
        topic: display(&agent.topic),
        name: display(&agent.name),
        created_by: agent.created_by.clone(),
        watched: agent.watched,
        watch_started_after: agent.watch_started_after,
        thread_id: agent.thread.id.clone(),
        thread_generation_revision: agent.thread.generation_revision.to_string(),
    };
    conn.prepare_cached("INSERT INTO device_agent_facts VALUES(?1,?2) ON CONFLICT(agent_id) DO UPDATE SET record=?2")?.execute(params![agent.id,serde_json::to_string(&facts).expect("device facts")])?;
    Ok(())
}
pub(in crate::store) fn seed_owner(
    conn: &Connection,
    id: &str,
    project: &str,
    workspace: Option<&str>,
) -> Result<(), StoreError> {
    if installed(conn)? {
        conn.prepare_cached("INSERT INTO device_owner_projects VALUES(?1,?2,?3) ON CONFLICT(owner_id) DO UPDATE SET project_path=?2,workspace_path=?3")?.execute(params![id,project,workspace])?;
    }
    Ok(())
}
pub(super) fn backfill(conn: &Connection, id: &str) -> Result<(), StoreError> {
    let (owner,raw):(String,String)=conn.query_row("SELECT owner_id,json_object(
        'id',id,'owner_id',owner_id,'conversation_id',COALESCE(json_extract(record,'$.conversation_id'),id),
        'choice',json(COALESCE(json_extract(record,'$.choice'),'{}')),'choice_revision',COALESCE(json_extract(record,'$.choice_revision'),0),
        'ordinal',ordinal,'created_at',json_extract(record,'$.created_at'),'topic',json_extract(record,'$.topic'),'name',json_extract(record,'$.name'),
        'created_by',json_extract(record,'$.created_by'),'watched',json(CASE COALESCE(json_extract(record,'$.watched'),1) WHEN 0 THEN 'false' ELSE 'true' END),
        'watch_started_after',COALESCE(json_extract(record,'$.watch_started_after'),0),'thread_id',COALESCE(json_extract(record,'$.thread.id'),'thread:'||id),
        'thread_generation_revision',''||COALESCE(record -> '$.thread.generation_revision','0')) FROM agents WHERE id=?1",[id],|row|Ok((row.get(0)?,row.get(1)?)))?;
    let mut facts: AgentFacts =
        serde_json::from_str(&raw).map_err(|source| StoreError::Corrupt {
            path: format!("agents/{id}").into(),
            source,
        })?;
    facts.topic = display(&facts.topic);
    facts.name = display(&facts.name);
    conn.execute("INSERT INTO device_agent_facts VALUES(?1,?2) ON CONFLICT(agent_id) DO UPDATE SET record=?2",params![id,serde_json::to_string(&facts).expect("device facts")])?;
    let exists: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM device_owner_projects WHERE owner_id=?1)",
        [&owner],
        |row| row.get(0),
    )?;
    if !exists {
        let links:Option<(String,Option<String>)>=conn.query_row("SELECT json_extract(record,'$.project_path'),json_extract(record,'$.worktree_path') FROM implementations WHERE id=?1 UNION ALL SELECT json_extract(record,'$.project_path'),NULL FROM tasks WHERE id=?1 LIMIT 1",[&owner],|row|Ok((row.get(0)?,row.get(1)?))).optional()?;
        if let Some((project, workspace)) = links {
            seed_owner(conn, &owner, &project, workspace.as_deref())?;
        }
    }
    Ok(())
}
pub(in crate::store) fn forget_agent(conn: &Connection, id: &str) -> Result<(), StoreError> {
    if installed(conn)? {
        conn.prepare_cached("DELETE FROM device_agent_facts WHERE agent_id=?1")?
            .execute([id])?;
    }
    Ok(())
}
pub(in crate::store) fn forget_owner(conn: &Connection, id: &str) -> Result<(), StoreError> {
    if installed(conn)? {
        conn.prepare_cached("DELETE FROM device_owner_projects WHERE owner_id=?1")?
            .execute([id])?;
    }
    Ok(())
}

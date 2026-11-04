//! Authoritative registry rows converge under the same dirty-generation fence.
use super::{Entity, SyncError};
use crate::store::StoreWriter;
use rusqlite::{params, OptionalExtension};
use serde_json::Value;
use std::collections::BTreeMap;

const FAMILIES:[&str;5]=["project","project_source","workspace","workspace_directory","badge"];
impl StoreWriter {
    pub fn device_registry_generation(&self)->Result<Option<i64>,SyncError> {
        Ok(self.connection().query_row("SELECT generation FROM sync_dirty WHERE kind='registry' AND key='lifecycle'",[],|row|row.get(0)).optional()?)
    }
    pub fn device_registry_markers(&self)->Result<Vec<(String,String,i64)>,SyncError> {
        Ok(self.connection().prepare("SELECT kind,key,generation FROM sync_dirty WHERE kind IN ('registry','project','project_source','workspace','workspace_directory') ORDER BY kind,key")?
            .query_map([],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?)))?.collect::<Result<Vec<_>,_>>()?)
    }
    pub fn reconcile_device_registry(&self,generation:i64,rows:&BTreeMap<Entity,Value>,projects:&BTreeMap<String,String>,_markers:&[(String,String,i64)])->Result<(),SyncError> {
        self.sync_transaction(|tx,batch| {
            let held:Option<i64>=tx.query_row("SELECT generation FROM sync_dirty WHERE kind='registry' AND key='lifecycle'",[],|row|row.get(0)).optional()?;
            if held!=Some(generation) {return Err(super::invalid("stale device registry generation"));}
            for family in FAMILIES {
                let previous=tx.prepare("SELECT key FROM sync_projection WHERE kind=?1 AND deleted=0")?
                    .query_map([family],|row|row.get::<_,String>(0))?.collect::<Result<Vec<_>,_>>()?;
                for key in previous {
                    let entity=Entity::new(family,&key);
                    if !rows.contains_key(&entity) {super::stage_device(tx,batch,entity,None)?;}
                }
            }
            for (entity,row) in rows {super::stage_device(tx,batch,entity.clone(),Some(row.clone()))?;}
            reconcile_project_ids(tx,projects)?;
            tx.execute("DELETE FROM sync_dirty WHERE kind='registry' AND key='lifecycle' AND generation=?1",[generation])?;
            // Startup owns these durable registry families, including markers
            // left by an older process immediately before a crash.
            tx.execute("DELETE FROM sync_dirty WHERE kind IN ('project','project_source','workspace','workspace_directory')",[])?;
            Ok(())
        })?;
        Ok(())
    }
}
fn reconcile_project_ids(tx:&rusqlite::Transaction<'_>,projects:&BTreeMap<String,String>)->Result<(),crate::store::StoreError> {
    let previous=tx.prepare("SELECT path,project_id FROM device_project_ids ORDER BY path")?
        .query_map([],|row|Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?)))?.collect::<Result<BTreeMap<_,_>,_>>()?;
    for (path,id) in projects {
        if previous.get(path)!=Some(id) {
            tx.execute("INSERT INTO device_project_ids VALUES(?1,?2) ON CONFLICT(path) DO UPDATE SET project_id=?2",params![path,id])?;
            super::touch_device_project(tx,path)?;
        }
    }
    for path in previous.keys().filter(|path|!projects.contains_key(*path)) {
        tx.execute("DELETE FROM device_project_ids WHERE path=?1",[path])?;
        super::touch_device_project(tx,path)?;
    }
    Ok(())
}

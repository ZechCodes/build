//! Config is authoritative. SQL is a disposable projection behind the durable
//! membership dirty fence, shared by writer authorization and metadata snapshots.
use crate::secrets::{GrantScope, Result, SecretError, SecretRecord};
use crate::store::StoreWriter;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;

impl StoreWriter {
    pub(crate) fn secret_membership_clean(&self) -> Result<bool> {
        clean(&self.connection())
    }
    pub(crate) fn secrets_reconcile_config(&self, config: &Value) -> Result<()> {
        super::writes::durable(self, |tx| {
            reconcile_config_in(tx, config)?;
            clear_fence_in(tx)
        })
    }
    pub(crate) fn secrets_cancel_membership_write(&self) -> Result<()> {
        super::writes::durable(self, clear_fence_in)
    }
}

fn clear_fence_in(tx: &rusqlite::Transaction<'_>) -> Result<()> {
    tx.execute(
        "DELETE FROM sync_dirty WHERE kind='secrets' AND key='membership'",
        [],
    )?;
    Ok(())
}

pub(super) fn reconcile_config_in(tx: &rusqlite::Transaction<'_>, config: &Value) -> Result<()> {
    tx.execute("DELETE FROM secret_project_membership", [])?;
    tx.execute("DELETE FROM secret_environments", [])?;
    for environment in config["environments"].as_array().into_iter().flatten() {
        let id = environment["id"].as_str().ok_or(SecretError::Invalid)?;
        tx.execute("INSERT INTO secret_environments VALUES (?1)", [id])?;
    }
    for project in config["projects"].as_array().into_iter().flatten() {
        let id = project["id"].as_str().ok_or(SecretError::Invalid)?;
        let environment = project["environment_id"]
            .as_str()
            .ok_or(SecretError::Invalid)?;
        // Parked entries referencing a missing environment remain in
        // config for recovery but cannot acquire environment authority.
        tx.execute("INSERT INTO secret_project_membership SELECT ?1,?2,?3 WHERE EXISTS(SELECT 1 FROM secret_environments WHERE environment_id=?2)",params![id,environment,!project["archived_at"].is_null()])?;
    }
    Ok(())
}

pub(super) fn clean(connection: &Connection) -> Result<bool> {
    Ok(!connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM sync_dirty WHERE kind='secrets' AND key='membership')",
        [],
        |r| r.get::<_, bool>(0),
    )?)
}

pub(super) fn current_project(connection: &Connection, project: &str) -> Result<bool> {
    let archived: Option<bool> = connection
        .query_row(
            "SELECT archived FROM secret_project_membership WHERE project_id=?1",
            [project],
            |r| r.get(0),
        )
        .optional()?;
    Ok(archived == Some(false))
}

pub(super) fn environment_allows(
    connection: &Connection,
    project: &str,
    environment: &str,
) -> Result<bool> {
    if !clean(connection)? || !current_project(connection, project)? {
        return Ok(false);
    }
    Ok(connection.query_row("SELECT EXISTS(SELECT 1 FROM secret_project_membership m JOIN secret_environments e USING(environment_id) WHERE m.project_id=?1 AND m.environment_id=?2 AND m.archived=0)",params![project,environment],|r|r.get(0))?)
}

pub(super) fn authorize_active_project(connection: &Connection, project: &str) -> Result<()> {
    let archived: Option<bool> = connection
        .query_row(
            "SELECT archived FROM secret_project_membership WHERE project_id=?1",
            [project],
            |r| r.get(0),
        )
        .optional()?;
    if archived == Some(true) {
        return Err(SecretError::ProjectArchived);
    }
    Ok(())
}

pub(super) fn authorize_grant(
    connection: &Connection,
    project: &str,
    grant: &GrantScope,
) -> Result<()> {
    if let GrantScope::Environment { environment_id } = grant {
        if !environment_allows(connection, project, environment_id)? {
            return Err(SecretError::Unavailable);
        }
    }
    Ok(())
}

pub(super) fn grant_environment(grant: &GrantScope) -> Option<&str> {
    match grant {
        GrantScope::Environment { environment_id } => Some(environment_id),
        _ => None,
    }
}

pub(super) fn overridden(
    connection: &Connection,
    project: &str,
    record: &SecretRecord,
) -> Result<bool> {
    let now = super::records::highwater(connection)?;
    Ok(connection.query_row("SELECT EXISTS(SELECT 1 FROM secret_records WHERE device_id=?1 AND name=?2 AND grant_scope='project' AND grant_project_id=?3 AND attached_at IS NOT NULL AND revoked_at IS NULL AND blob_present=1 AND (expires_at IS NULL OR expires_at>?4))",params![record.context.device_id,record.context.name,project,now],|r|r.get(0))?)
}

#[cfg(test)]
pub(crate) fn seed_current_project_for_test(store: &crate::store::Store, project: &str) {
    let c = store.writer_for_test();
    c.execute(
        "INSERT OR IGNORE INTO secret_environments VALUES ('synthetic-environment')",
        [],
    )
    .unwrap();
    c.execute(
        "INSERT OR IGNORE INTO secret_project_membership VALUES (?1,'synthetic-environment',0)",
        [project],
    )
    .unwrap();
}

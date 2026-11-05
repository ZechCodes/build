use super::{audit, records, writes};
use crate::secrets::{GrantScope, Result, SecretCaller, SecretError, SecretRecord};
use crate::store::StoreWriter;
use rusqlite::{Connection, OptionalExtension, Transaction};

impl StoreWriter {
    /// The writer/IMMEDIATE transaction is the shared linearization boundary
    /// with every revoke, reset, roster removal and project retirement.
    pub(crate) fn secrets_release<T>(
        &self,
        caller: &SecretCaller,
        id: &str,
        client_request_id: &str,
        observed_at: i64,
        is_current: impl FnOnce() -> bool,
        read: impl FnOnce(&Transaction<'_>, &SecretRecord) -> Result<T>,
    ) -> Result<T> {
        validate_request(caller, client_request_id)?;
        let result = writes::durable(self, |tx| {
            if !is_current() || !committed_member(tx, caller)? {
                return Err(SecretError::Unauthorized);
            }
            clock_observe_in(tx, observed_at)?;
            // A malformed identifier is indistinguishable from an absent one.
            let record = if crate::secrets::validate_id(id).is_ok() {
                records::load(tx, id)?
            } else {
                None
            };
            let authorized = match record.as_ref() {
                Some(record) if super::post::final_scope_allows(tx, caller, record)? => {
                    Some(record)
                }
                _ => None,
            };
            let result = match authorized {
                Some(record) => read(tx, record),
                None => Err(SecretError::Unavailable),
            };
            let outcome = outcome(&result);
            audit::append_in(tx, caller, authorized, outcome)?;
            Ok(result)
        })?;
        result
    }
}

fn validate_request(caller: &SecretCaller, request: &str) -> Result<()> {
    let fields = [
        &caller.device_id,
        &caller.channel,
        &caller.project_id,
        &caller.owner_id,
        &caller.canonical_owner_id,
        &caller.agent_id,
        &caller.conversation_id,
        &caller.thread_id,
        &caller.generation,
        &caller.session_incarnation,
        request,
    ];
    if fields
        .iter()
        .any(|field| field.is_empty() || field.len() > 128 || field.contains('\0'))
    {
        return Err(SecretError::Unauthorized);
    }
    Ok(())
}

fn committed_member(connection: &Connection, caller: &SecretCaller) -> Result<bool> {
    for owner in [&caller.owner_id, &caller.canonical_owner_id] {
        let project: Option<String> = connection.query_row(
            "SELECT p.project_id FROM device_owner_projects o JOIN device_project_ids p ON p.path=o.project_path WHERE o.owner_id=?1",
            [owner], |row| row.get(0)).optional()?;
        if project.as_deref() != Some(caller.project_id.as_str()) {
            return Ok(false);
        }
    }
    let agent = saved_agent(connection, &caller.agent_id)?;
    let canonical = saved_agent(connection, &caller.conversation_id)?;
    Ok(match (agent, canonical) {
        (Some((owner, agent)), Some((canonical_owner, canonical))) => {
            owner == caller.owner_id
                && agent.id == caller.agent_id
                && agent.owner_id == owner
                && agent.state != crate::agent::AgentLifecycle::Ended
                && agent.conversation_id() == caller.conversation_id
                && canonical_owner == caller.canonical_owner_id
                && canonical.id == caller.conversation_id
                && canonical.owner_id == canonical_owner
                && canonical.state != crate::agent::AgentLifecycle::Ended
                && canonical.conversation_id() == caller.conversation_id
                && canonical.thread.id == caller.thread_id
                && canonical.thread.generation_revision == caller.generation_revision
                && caller.generation == caller.thread_id
        }
        _ => false,
    })
}

fn saved_agent(connection: &Connection, id: &str) -> Result<Option<(String, crate::agent::Agent)>> {
    let row: Option<(String, String)> = connection
        .query_row(
            "SELECT owner_id,record FROM agents WHERE id=?1",
            [id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    row.map(|(owner, json)| {
        serde_json::from_str(&json)
            .map(|agent| (owner, agent))
            .map_err(|_| SecretError::Unavailable)
    })
    .transpose()
}

pub(crate) fn scope_allows(caller: &SecretCaller, record: &SecretRecord) -> bool {
    if caller.device_id != record.context.device_id {
        return false;
    }
    match &record.context.grant {
        GrantScope::Agent {
            agent_id,
            conversation_id,
            generation,
        } => {
            agent_id == &caller.agent_id
                && conversation_id == &caller.conversation_id
                && generation == &caller.generation
        }
        GrantScope::Project { project_id } => project_id == &caller.project_id,
    }
}

fn outcome<T>(result: &Result<T>) -> &'static str {
    match result {
        Ok(_) => "released",
        Err(SecretError::Expired) => "secret_expired",
        Err(SecretError::Revoked) => "secret_revoked",
        Err(_) => "secret_unavailable",
    }
}

pub(crate) fn clock_observe_in(tx: &Transaction<'_>, now: i64) -> Result<(bool, i64)> {
    if now < 0 {
        return Err(SecretError::Unavailable);
    }
    let highwater = records::highwater(tx)?;
    writes::observe_in(tx, now)?;
    Ok((now >= highwater, now.max(highwater)))
}

pub(crate) fn expire_in(tx: &Transaction<'_>, id: &str, now: i64) -> Result<()> {
    let changed=tx.execute("UPDATE secret_records SET revoked_at=?2,revocation_reason='expired',revision=revision+1 WHERE id=?1 AND revoked_at IS NULL",rusqlite::params![id,now])?;
    if changed == 1 {
        let record = records::required(tx, id)?;
        super::append_metadata_in(tx, &record, "expired", now)?;
    }
    Ok(())
}

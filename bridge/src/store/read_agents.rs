//! Conversation identity and metadata from the same request snapshot as pages.
use super::entities::primary_agent_id;
use super::{
    PersistedPlan, PersistedRun, ReadTransaction, Store, StoreError, THREAD_LAST_SEQUENCE_SQL,
};
use crate::agent::Agent;
use crate::models::ModelChoice;
use rusqlite::OptionalExtension;

impl ReadTransaction<'_> {
    /// Legacy-shaped rows derive identity in memory exactly as startup does.
    /// No identity materialization is written while serving a read.
    pub fn agents(&self, owner_id: &str) -> Result<Vec<Agent>, StoreError> {
        let (choice, shared) = self.owner_choice(owner_id)?;
        Store::read_agent_records(self.connection(), owner_id, &choice, shared.as_deref())
    }

    /// The durable identity, binding and generation, without reading history.
    /// A caller asks separately for the counters its response actually uses.
    pub fn load_agent_record(&self, agent_id: &str) -> Result<Option<(String, Agent)>, StoreError> {
        let owner: Option<String> = self
            .connection()
            .query_row(
                "SELECT owner_id FROM agents WHERE id = ?1",
                [agent_id],
                |row| row.get(0),
            )
            .optional()?;
        let Some(owner) = owner else { return Ok(None) };
        let Some(mut agent) = self
            .agents(&owner)?
            .into_iter()
            .find(|agent| agent.id == agent_id)
        else {
            return Ok(None);
        };
        agent.thread.rekey_to_agent(&agent.id);
        Ok(Some((owner, agent)))
    }

    /// The indexed high-water mark includes mutations outside a resident tail.
    pub fn thread_last_sequence(&self, agent_id: &str) -> Result<u64, StoreError> {
        let sequence: i64 =
            self.connection()
                .query_row(THREAD_LAST_SEQUENCE_SQL, [agent_id], |row| row.get(0))?;
        Ok(sequence as u64)
    }

    fn owner_choice(&self, owner: &str) -> Result<(ModelChoice, Option<String>), StoreError> {
        let run: Option<String> = self
            .connection()
            .query_row(
                "SELECT record FROM implementations WHERE id = ?1",
                [owner],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(raw) = run {
            let run: PersistedRun = super::tracker::decode(&raw, "implementations", owner)?;
            let shared = run
                .plan_id
                .as_deref()
                .map(|plan| primary_agent_id(self.connection(), plan))
                .transpose()?
                .flatten();
            return Ok((run.model_choice(), shared));
        }
        let plan: Option<String> = self
            .connection()
            .query_row("SELECT record FROM tasks WHERE id = ?1", [owner], |row| {
                row.get(0)
            })
            .optional()?;
        let choice = plan
            .map(|raw| {
                super::tracker::decode::<PersistedPlan>(&raw, "tasks", owner)
                    .map(|plan| plan.model_choice())
            })
            .transpose()?
            .unwrap_or_default();
        Ok((choice, None))
    }
}

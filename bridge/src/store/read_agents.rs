//! Conversation identity and metadata from the same request snapshot as pages.
use super::entities::primary_agent_id;
use super::{
    stored_conversation_summary, PersistedPlan, PersistedRun, ReadTransaction, Store, StoreError,
    THREAD_FIRST_ATTENTION_AFTER_SQL, THREAD_ITEM_COUNT_SQL, THREAD_LAST_ATTENTION_SQL,
    THREAD_LAST_MESSAGE_SQL, THREAD_LAST_OWN_MESSAGE_SQL, THREAD_LAST_SEQUENCE_SQL,
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

    /// The durable skeleton plus count/sequence fields omitted by serialization.
    /// History stays in SQLite; only indexed summary rows are decoded before
    /// a bounded page explicitly asks for items.
    pub fn load_agent_skeleton(
        &self,
        agent_id: &str,
    ) -> Result<Option<(String, Agent)>, StoreError> {
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
        self.restore_thread_metadata(&mut agent)?;
        Ok(Some((owner, agent)))
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

    fn restore_thread_metadata(&self, agent: &mut Agent) -> Result<(), StoreError> {
        let conn = self.connection();
        let count = |sql| conn.query_row(sql, [&agent.id], |row| row.get::<_, i64>(0));
        let held = count(THREAD_ITEM_COUNT_SQL)? as u64;
        let last_sequence = count(THREAD_LAST_SEQUENCE_SQL)? as u64;
        let last_own = count(THREAD_LAST_OWN_MESSAGE_SQL)? as u64;
        let last_attention = count(THREAD_LAST_ATTENTION_SQL)? as u64;
        let summary = stored_conversation_summary(
            &mut conn.prepare(THREAD_LAST_MESSAGE_SQL)?,
            &mut conn.prepare(THREAD_FIRST_ATTENTION_AFTER_SQL)?,
            &agent.id,
        )?;
        agent
            .thread
            .adopt_stored_tail(Vec::new(), held, last_sequence);
        agent.thread.adopt_conversation_summary(
            summary.last_message_sequence,
            last_own,
            last_attention,
            summary.activity_at,
            summary.working,
        );
        Ok(())
    }
}

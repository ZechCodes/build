use super::{
    load_legacy_owner_context, read_thread_page, remove_dir_if_present,
    stored_conversation_summary, Store, StoreError, RESIDENT_CONVERSATION_TAIL,
    THREAD_ACTIVITY_COUNT_SQL, THREAD_FIRST_ATTENTION_AFTER_SQL, THREAD_ITEM_COUNT_SQL,
    THREAD_LAST_ATTENTION_SQL, THREAD_LAST_MESSAGE_SQL, THREAD_LAST_OWN_MESSAGE_SQL,
    THREAD_LAST_SEQUENCE_SQL, THREAD_PAGE_SQL, THREAD_TOOL_CALL_COUNT_SQL,
};
use crate::agent::Agent;
use crate::agent::AgentRoster;
use crate::agent::CURRENT_SETTINGS_VERSION;
use crate::attention::Attention;
use crate::models::AgentProvider;
use crate::models::ModelChoice;
use crate::plan::is_worktree_contained_path;
use crate::plan::PlanState;
use crate::plan::StageDoc;
use crate::run::RunState;
use crate::run::StageProgress;
use crate::thread::RunCensus;
use crate::thread::Thread;
use crate::thread::ThreadItem;
use rusqlite::Connection;
use rusqlite::OptionalExtension;
use serde::Deserialize;
use serde::Serialize;
use std::collections::HashMap;
use std::collections::HashSet;
use std::path::PathBuf;

#[cfg(test)]
thread_local! {
    static AGENT_READ_COUNTS: std::cell::Cell<(usize, usize)> = const { std::cell::Cell::new((0, 0)) };
}

/// The durable core of one plan — the project-scoped half of the split. Its
/// canonical docs live beside the record under `plans/<plan_id>/docs/`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersistedPlan {
    pub id: String,
    pub goal: String,
    /// Canonical path of the project repo the plan belongs to. Stored as a
    /// path (not the in-memory project id) because a `proj-N` id is not
    /// durable: a boot mints ids from the config that restored them, and the
    /// same repository can come back wearing another one.
    pub project_path: String,
    pub base_branch: String,
    pub state: PlanState,
    /// Durable archival metadata. This is deliberately not a `PlanState` arm:
    /// lifecycle and filing are independent, and canonical docs remain live.
    #[serde(default)]
    pub archived_at: Option<String>,
    #[serde(default)]
    pub implementation_intent: crate::plan::ImplementationIntent,
    #[serde(default)]
    pub implementation_activity: crate::plan::ImplementationActivity,
    /// Docs-dir-relative path of the single plan doc (`.build/plan.md`).
    pub plan_path: String,
    /// Stage docs: manifest metadata + plan-side review sub-state. Empty for
    /// single-doc plans.
    #[serde(default)]
    pub stages: Vec<StageDoc>,
    #[serde(default)]
    pub provider: AgentProvider,
    /// Model/effort the plan's agents run on (None = harness default).
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub effort: Option<String>,
    /// This entity's agents, each owning its own conversation. Empty only on a
    /// record written before agents existed. `AgentRoster::restore` fills it
    /// from `legacy_thread` on reattach.
    #[serde(default)]
    pub agents: Vec<Agent>,
    /// The pre-agent, entity-keyed conversation. Retained so the JSON records
    /// the one-off import reads still deserialize; empty on everything written
    /// since, and never written by this store.
    #[serde(rename = "thread", default, skip_serializing_if = "Thread::is_empty")]
    pub legacy_thread: Thread,
    pub last_summary: Option<String>,
    /// The most recent thing that went wrong for this plan, shown on the card
    /// until the plan advances again.
    #[serde(default)]
    pub last_error: Option<String>,
    /// RFC 3339 UTC timestamps.
    pub created_at: String,
    pub updated_at: String,
    /// When the plan last changed *state* (vs `updated_at`, any mutation).
    /// `None` on records from before the field existed — restore falls back
    /// to `updated_at`.
    #[serde(default)]
    pub state_changed_at: Option<String>,
}

/// The durable core of one run — the worktree-scoped half of the split. A run
/// stores `plan_id` instead of plan docs; only an adopted (or v1-migrated) run
/// carries `plan_id: None`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersistedRun {
    pub id: String,
    /// The plan this run implements. `None` = an adopted (or v1-migrated) run:
    /// no plan behind it, so no plan gate.
    #[serde(default)]
    pub plan_id: Option<String>,
    pub goal: String,
    /// Canonical path of the project repo (same rationale as on the plan).
    pub project_path: String,
    pub base_branch: String,
    pub state: RunState,
    pub branch: String,
    pub worktree_name: String,
    pub worktree_path: String,
    /// The "plan: <goal>" materialization commit recorded at dispatch — the
    /// baseline of the run's review diff, keeping materialized docs out of
    /// review noise. `None` for adopted and migrated runs (the diff falls back
    /// to the merge-base).
    #[serde(default)]
    pub base_sha: Option<String>,
    /// Run-side per-stage execution progress, keyed by the plan's stage ids.
    /// A progress record exists only once its stage has been dispatched.
    #[serde(default)]
    pub stages: Vec<StageProgress>,
    /// The stage whose build/fix/validate session is (or was last) in flight.
    #[serde(default)]
    pub current_stage_id: Option<String>,
    /// The stage a mid-run revision session is running for (disambiguates
    /// store write-back from post-review changes on `done(revise)`).
    #[serde(default)]
    pub revising_stage_id: Option<String>,
    /// "Run all": auto-dispatch the next approved stage when one completes.
    #[serde(default)]
    pub auto_advance: bool,
    /// True for a run minted around a pre-existing (user-created) worktree.
    #[serde(default)]
    pub adopted: bool,
    /// Write-ahead journal for push/merge. It is cleared only after refs prove
    /// the candidate commit's publication (including after daemon restart).
    #[serde(default)]
    pub publication_attempt: Option<crate::run::PublicationAttempt>,
    #[serde(default)]
    pub provider: AgentProvider,
    /// Model/effort the run's agents run on (None = harness default).
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub effort: Option<String>,
    /// This entity's agents, each owning its own conversation. A branch carries
    /// as many as the human adds; empty only on a pre-agent record.
    #[serde(default)]
    pub agents: Vec<Agent>,
    /// The pre-agent, entity-keyed conversation — see [`PersistedPlan::legacy_thread`].
    #[serde(rename = "thread", default, skip_serializing_if = "Thread::is_empty")]
    pub legacy_thread: Thread,
    pub last_summary: Option<String>,
    #[serde(default)]
    pub last_error: Option<String>,
    /// RFC 3339 UTC timestamps.
    pub created_at: String,
    pub updated_at: String,
    /// When the run last changed *state* (vs `updated_at`, any mutation).
    /// `None` on records from before the field existed — restore falls back
    /// to `updated_at`.
    #[serde(default)]
    pub state_changed_at: Option<String>,
}

impl PersistedPlan {
    pub fn model_choice(&self) -> ModelChoice {
        ModelChoice {
            provider: self.provider,
            model: self.model.clone(),
            effort: self.effort.clone(),
        }
    }

    /// This record's agents, with a pre-agent conversation folded onto the
    /// first one. The reattach path and the boot migration share it, so a
    /// record loaded before the migration ran reads the same either way.
    pub fn roster(&self) -> AgentRoster {
        AgentRoster::restore(
            &self.id,
            self.agents.clone(),
            self.legacy_thread.clone(),
            self.model_choice(),
            &self.created_at,
        )
    }

    /// Who [`roster`](Self::roster) would restore, without their conversations.
    pub fn members(&self) -> Vec<crate::agent::RosterMember> {
        AgentRoster::restored_members(
            &self.id,
            &self.agents,
            &self.legacy_thread,
            &self.model_choice(),
        )
    }
}

impl PersistedRun {
    pub fn model_choice(&self) -> ModelChoice {
        ModelChoice {
            provider: self.provider,
            model: self.model.clone(),
            effort: self.effort.clone(),
        }
    }

    /// See [`PersistedPlan::roster`].
    pub fn roster(&self) -> AgentRoster {
        AgentRoster::restore(
            &self.id,
            self.agents.clone(),
            self.legacy_thread.clone(),
            self.model_choice(),
            &self.created_at,
        )
    }

    /// Who [`roster`](Self::roster) would restore, without their conversations.
    pub fn members(&self) -> Vec<crate::agent::RosterMember> {
        AgentRoster::restored_members(
            &self.id,
            &self.agents,
            &self.legacy_thread,
            &self.model_choice(),
        )
    }
}

/// Canonical durable Task aggregate. Planning state, stage-plan review,
/// implementation lineage, threads, and publication journals cross the crash
/// boundary as one record instead of being reconstructed from mutable halves.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersistedTask {
    pub task: PersistedPlan,
    #[serde(default)]
    pub implementations: Vec<PersistedRun>,
}

/// The user-selected way an external worktree was finished.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorktreeFinishAction {
    Cleanup,
    Push,
    Merge,
    Delete,
}

/// Whether a durable finish record is protecting an in-progress destructive
/// operation or is ready to appear in Project Archive.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorktreeFinishStatus {
    Pending,
    #[default]
    Archived,
}

/// Durable intent and eventual history for an external worktree finished
/// through `worktree.finish`. Project ids are intentionally absent because a
/// `proj-N` id is not durable across boots; the canonical project path is the
/// stable identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersistedArchivedWorktree {
    #[serde(default)]
    pub status: WorktreeFinishStatus,
    pub project_path: String,
    pub worktree_id: String,
    pub worktree_name: String,
    pub worktree_path: String,
    #[serde(default)]
    pub branch: Option<String>,
    pub head_sha: String,
    #[serde(default)]
    pub upstream: Option<String>,
    #[serde(default)]
    pub unpushed: Option<u64>,
    pub dirty_files: usize,
    pub uncommitted_files: usize,
    pub uncommitted_insertions: usize,
    pub uncommitted_deletions: usize,
    pub action: WorktreeFinishAction,
    #[serde(default)]
    pub archived_at: Option<String>,
}

fn retain_agent_message_times(
    tx: &rusqlite::Transaction,
    run_id: &str,
    agent_id: &str,
) -> Result<(), StoreError> {
    let mut query = tx.prepare(
        "SELECT json_extract(item, '$.data.created_at') FROM thread_items \
         WHERE agent_id = ?1 AND message = 1 \
         AND COALESCE(json_extract(item, '$.data.from_build'), 0) = 0",
    )?;
    let times = query
        .query_map([agent_id], |row| row.get::<_, Option<String>>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    drop(query);
    for at in times.into_iter().flatten() {
        if let Some(ts) = crate::session_summary::message_millis(&at) {
            tx.execute(
                "INSERT INTO inbox_retained_run_messages (run_id, ts_ms) VALUES (?1, ?2)",
                rusqlite::params![run_id, ts],
            )?;
        }
    }
    Ok(())
}

/// How a read fills in a record's agents: with their conversations' tails
/// ([`Store::read_agents`]) or without ([`Store::read_agent_records`]).
type ReadAgents =
    fn(&Connection, &str, &ModelChoice, Option<&str>) -> Result<Vec<Agent>, StoreError>;

impl Store {
    /// Calls to the record-only and conversation-loading agent readers on this
    /// test thread. A snapshot around an app call measures its store work
    /// without depending on host scheduling.
    #[cfg(test)]
    pub(crate) fn agent_read_counts() -> (usize, usize) {
        AGENT_READ_COUNTS.with(std::cell::Cell::get)
    }

    /// Classify every stored item for the freshly added columns.
    ///
    /// The one place Build reads whole conversations on purpose: it runs once,
    /// on the upgrade, because a column added with a default says nothing about
    /// the items already under it. Every column is written on every upgrade
    /// path — a v1 database gains them together, and rewriting one with the
    /// value it already holds is what makes a single classifier serve them
    /// all.
    pub(super) fn classify_stored_items(conn: &Connection) -> Result<(), StoreError> {
        let rows: Vec<(String, i64, String)> = {
            let mut statement =
                conn.prepare("SELECT agent_id, sequence, item FROM thread_items")?;
            let read = statement.query_map([], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get::<_, String>(2)?))
            })?;
            read.collect::<Result<_, _>>()?
        };
        let mut set = conn.prepare(
            "UPDATE thread_items SET attention = ?3, message = ?4, tool_call = ?5, \
                                     activity = ?6, handoff = ?7 \
             WHERE agent_id = ?1 AND sequence = ?2",
        )?;
        for (agent_id, sequence, raw) in rows {
            let Ok(item) = serde_json::from_str::<ThreadItem>(&raw) else {
                continue;
            };
            set.execute(rusqlite::params![
                agent_id,
                sequence,
                i64::from(item.attention_reason().is_some()),
                i64::from(item.counts_toward_page()),
                i64::from(item.is_tool_call()),
                i64::from(item.is_activity()),
                i64::from(item.is_handoff())
            ])?;
        }
        Ok(())
    }
    /// Test-only: how many rows this connection has written since it opened.
    /// The only way to hold the store to its central promise — that appending
    /// one message writes one row rather than rewriting the conversation.
    #[cfg(test)]
    pub fn total_changes(&self) -> u64 {
        self.connection().total_changes()
    }
    /// Test-only: make the next write fail once, then behave normally.
    #[cfg(test)]
    pub fn fail_next_write(&self) {
        self.fail_next_write
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }
    /// Test-only: put a row into `captures` that will not parse back, so the
    /// loader's fail-fast contract can be exercised against real corruption.
    #[cfg(test)]
    pub fn corrupt_capture_row(&self, capture_id: &str) {
        self.connection()
            .execute(
                "INSERT INTO captures (id, record) VALUES (?1, ?2)
                 ON CONFLICT(id) DO UPDATE SET record = ?2",
                rusqlite::params![capture_id, "{ not json"],
            )
            .expect("the corrupt row is written");
    }
    /// Test-only: read an archived worktree's stored JSON, so a test can put
    /// the record back in a half-finished state the way an interrupted daemon
    /// would have left it.
    #[cfg(test)]
    pub fn archived_worktree_json(&self, worktree_id: &str) -> Option<String> {
        self.connection()
            .query_row(
                "SELECT record FROM archived_worktrees WHERE id = ?1",
                [worktree_id],
                |row| row.get(0),
            )
            .optional()
            .ok()
            .flatten()
    }
    /// Test-only counterpart of [`archived_worktree_json`](Self::archived_worktree_json).
    #[cfg(test)]
    pub fn set_archived_worktree_json(&self, worktree_id: &str, record: &str) {
        self.connection()
            .execute(
                "UPDATE archived_worktrees SET record = ?2 WHERE id = ?1",
                rusqlite::params![worktree_id, record],
            )
            .expect("the archived worktree row is updated");
    }
    /// The attention map. A row that will not parse is skipped rather than
    /// fatal: this is ordering and colour, never correctness, and losing one
    /// costs a badly sorted rail rather than a task.
    pub fn load_attention(&self) -> HashMap<String, Attention> {
        let conn = self.connection();
        let Ok(mut statement) = conn.prepare("SELECT entity_id, record FROM attention") else {
            return HashMap::new();
        };
        let Ok(rows) = statement.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        }) else {
            return HashMap::new();
        };
        rows.flatten()
            .filter_map(|(id, raw)| serde_json::from_str(&raw).ok().map(|value| (id, value)))
            .collect()
    }
    /// Persist the attention map, pruned to `live` — ids that no longer exist
    /// (a deleted run, a removed worktree) drop out, so the table tracks the
    /// world rather than growing forever.
    pub fn save_attention(
        &self,
        attention: &HashMap<String, Attention>,
        live: &HashSet<String>,
    ) -> Result<(), StoreError> {
        self.in_transaction(|tx| {
            tx.execute("DELETE FROM attention", [])?;
            let mut insert =
                tx.prepare("INSERT INTO attention (entity_id, record) VALUES (?1, ?2)")?;
            for (id, record) in attention.iter().filter(|(id, _)| live.contains(*id)) {
                insert.execute(rusqlite::params![
                    id,
                    serde_json::to_string(record).expect("attention always serializes")
                ])?;
            }
            Ok(())
        })
    }
    /// Write one owner's agents and their conversations.
    ///
    /// The roster is replaced wholesale (an agent can be removed), but the
    /// conversation is NOT rewritten. The stored `(sequence, updated_sequence)`
    /// pairs are read first — integers off a covering index, never the item
    /// bodies — and only items that are new or have changed since are written.
    /// So appending one message writes one row, whatever the conversation
    /// already holds. That is the entire reason this store replaced a JSON
    /// aggregate that rewrote every conversation on the Task for every append;
    /// writing all N items here would have moved the amplification rather than
    /// removed it.
    pub(super) fn write_agents(
        tx: &rusqlite::Transaction,
        owner_id: &str,
        agents: &[Agent],
    ) -> Result<(), StoreError> {
        let keep: HashSet<&str> = agents.iter().map(|agent| agent.id.as_str()).collect();
        let mut roster = tx.prepare("SELECT id FROM agents WHERE owner_id = ?1")?;
        let existing: Vec<String> = roster
            .query_map([owner_id], |row| row.get::<_, String>(0))?
            .collect::<Result<_, _>>()?;
        drop(roster);
        for gone in existing.iter().filter(|id| !keep.contains(id.as_str())) {
            // Removing an agent removes its conversation, but cannot erase the
            // time it contributed to the still-live workspace/project session.
            // Copy and delete in this transaction so replay sees either both
            // the old conversation or its retained message times.
            retain_agent_message_times(tx, owner_id, gone)?;
            tx.execute("DELETE FROM thread_items WHERE agent_id = ?1", [gone])?;
            tx.execute("DELETE FROM agents WHERE id = ?1", [gone])?;
        }

        let mut upsert_agent = tx.prepare(
            "INSERT INTO agents (id, owner_id, ordinal, record) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(id) DO UPDATE SET owner_id = ?2, ordinal = ?3, record = ?4",
        )?;
        let mut upsert_item = tx.prepare(
            "INSERT INTO thread_items \
             (agent_id, sequence, updated_sequence, attention, message, tool_call, \
              activity, handoff, item)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(agent_id, sequence)
             DO UPDATE SET updated_sequence = ?3, attention = ?4, message = ?5, \
                           tool_call = ?6, activity = ?7, handoff = ?8, item = ?9",
        )?;
        let mut delete_item =
            tx.prepare("DELETE FROM thread_items WHERE agent_id = ?1 AND sequence = ?2")?;
        // The cursor columns alone, which `thread_items_cursor` covers: this is
        // what keeps the read cheap enough that skipping the writes is a win.
        // Bounded below by the tail the agent holds: a conversation loaded as
        // its tail knows nothing about the history under it, and an orphan
        // sweep that read that history would take it for items withdrawn from
        // a conversation that never had them.
        let mut stored_cursors = tx.prepare(
            "SELECT sequence, updated_sequence FROM thread_items \
             WHERE agent_id = ?1 AND sequence >= ?2",
        )?;

        for agent in agents {
            // The agent row carries everything about the conversation EXCEPT
            // its items — sessions, revisions, the last completion report —
            // because those are small, bounded and always read whole.
            let mut skeleton = agent.clone();
            let items = std::mem::take(&mut skeleton.thread.items);
            upsert_agent.execute(rusqlite::params![
                agent.id,
                owner_id,
                agent.ordinal,
                serde_json::to_string(&skeleton).expect("an agent always serializes")
            ])?;

            let mut stored: HashMap<i64, i64> = stored_cursors
                .query_map(
                    rusqlite::params![&agent.id, agent.thread.resident_from_sequence() as i64],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )?
                .collect::<Result<_, _>>()?;
            for item in &items {
                let sequence = item.sequence() as i64;
                let updated = item.latest_sequence() as i64;
                // `remove` both answers "was it stored?" and leaves the map
                // holding exactly the sequences the roster no longer has.
                if stored.remove(&sequence) == Some(updated) {
                    continue;
                }
                upsert_item.execute(rusqlite::params![
                    agent.id,
                    sequence,
                    updated,
                    i64::from(item.attention_reason().is_some()),
                    i64::from(item.counts_toward_page()),
                    i64::from(item.is_tool_call()),
                    i64::from(item.is_activity()),
                    i64::from(item.is_handoff()),
                    serde_json::to_string(item).expect("a thread item always serializes")
                ])?;
            }
            // An item can be taken off a conversation (a withdrawn draft), so
            // what is left over is deleted rather than left behind to reappear
            // on the next load.
            for orphan in stored.keys() {
                delete_item.execute(rusqlite::params![agent.id, orphan])?;
            }
        }
        Ok(())
    }
    /// Read one owner's agents back, conversations included, in rail order.
    /// A record's agents as their rows hold them — settings, names, rail
    /// positions — with every conversation left in the database.
    pub(super) fn read_agent_records(
        conn: &Connection,
        owner_id: &str,
        entity_choice: &ModelChoice,
        shared_primary_conversation: Option<&str>,
    ) -> Result<Vec<Agent>, StoreError> {
        #[cfg(test)]
        AGENT_READ_COUNTS.with(|counts| {
            let (records, full) = counts.get();
            counts.set((records + 1, full));
        });
        let mut statement =
            conn.prepare("SELECT id, record FROM agents WHERE owner_id = ?1 ORDER BY ordinal")?;
        let rows: Vec<(String, String)> = statement
            .query_map([owner_id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<Result<_, _>>()?;
        let mut agents = Vec::with_capacity(rows.len());
        for (index, (id, raw)) in rows.into_iter().enumerate() {
            let mut agent: Agent =
                serde_json::from_str(&raw).map_err(|source| StoreError::Corrupt {
                    path: PathBuf::from(format!("agents/{id}")),
                    source,
                })?;
            let primary_conversation = (index == 0)
                .then_some(shared_primary_conversation)
                .flatten();
            // A record inserted directly by a current-version test or recovery
            // tool can still be legacy-shaped. Materialize it in memory; only
            // the versioned boot migration above overwrites raw stored data.
            materialize_agent_identity(&mut agent, entity_choice, primary_conversation);
            agents.push(agent);
        }
        Ok(agents)
    }

    /// A record's agents, each with the tail of its conversation.
    pub(super) fn read_agents(
        conn: &Connection,
        owner_id: &str,
        entity_choice: &ModelChoice,
        shared_primary_conversation: Option<&str>,
    ) -> Result<Vec<Agent>, StoreError> {
        #[cfg(test)]
        AGENT_READ_COUNTS.with(|counts| {
            let (records, full) = counts.get();
            counts.set((records, full + 1));
        });
        let mut agents =
            Store::read_agent_records(conn, owner_id, entity_choice, shared_primary_conversation)?;
        // The tail of each conversation, not the whole of it: a boot that read
        // every item of every conversation would spend, in one go and for the
        // life of the process, exactly what the paged reads exist to save.
        let mut tail = conn.prepare(THREAD_PAGE_SQL)?;
        let mut count = conn.prepare(THREAD_ITEM_COUNT_SQL)?;
        let mut last_sequence = conn.prepare(THREAD_LAST_SEQUENCE_SQL)?;
        let mut last_message = conn.prepare(THREAD_LAST_MESSAGE_SQL)?;
        let mut last_own_message = conn.prepare(THREAD_LAST_OWN_MESSAGE_SQL)?;
        let mut first_attention_after = conn.prepare(THREAD_FIRST_ATTENTION_AFTER_SQL)?;
        let mut last_attention = conn.prepare(THREAD_LAST_ATTENTION_SQL)?;
        for agent in &mut agents {
            let id = agent.id.clone();
            let held = count.query_row([&id], |row| row.get::<_, i64>(0))? as u64;
            let stored_last = last_sequence.query_row([&id], |row| row.get::<_, i64>(0))? as u64;
            let items = read_thread_page(&mut tail, &id, i64::MAX, RESIDENT_CONVERSATION_TAIL)?;
            agent.thread.adopt_stored_tail(
                items,
                held.saturating_sub(RESIDENT_CONVERSATION_TAIL as u64),
                stored_last,
            );
            let summary =
                stored_conversation_summary(&mut last_message, &mut first_attention_after, &id)?;
            let last_attention_sequence =
                last_attention.query_row([&id], |row| row.get::<_, i64>(0))? as u64;
            let last_own_message_sequence =
                last_own_message.query_row([&id], |row| row.get::<_, i64>(0))? as u64;
            agent.thread.adopt_conversation_summary(
                summary.last_message_sequence,
                last_own_message_sequence,
                last_attention_sequence,
                summary.activity_at,
                summary.working,
            );
        }
        Ok(agents)
    }
    /// How many rows of work a span of a conversation holds, and how many of
    /// them are tool calls — the store's census, inclusive at both ends, and
    /// the exact sibling of the memory one on `Thread`.
    ///
    /// Two seeks down two partial indexes, and never a row read: the counts
    /// are what a digest says about a run whose items the cap left off the
    /// wire.
    pub fn run_census(
        &self,
        agent_id: &str,
        from_sequence: u64,
        through_sequence: u64,
    ) -> Result<RunCensus, StoreError> {
        let connection = self.connection();
        let count = |statement: &str| -> Result<u64, StoreError> {
            let count: i64 = connection.query_row(
                statement,
                rusqlite::params![agent_id, from_sequence as i64, through_sequence as i64],
                |row| row.get(0),
            )?;
            Ok(count as u64)
        };
        Ok(RunCensus {
            tool_calls: count(THREAD_TOOL_CALL_COUNT_SQL)?,
            rows: count(THREAD_ACTIVITY_COUNT_SQL)?,
        })
    }
    /// How many items a conversation holds. A `COUNT(*)`, never a load — the
    /// client needs the total to know whether its cache is whole, and reading
    /// the items to count them would spend exactly what paging saves.
    /// How many attention-class items sit strictly between `cursor` and
    /// `floor` — the unread the human is owed from UNDER the resident tail.
    ///
    /// A conversation is loaded as its newest items, so a badge counted off
    /// what this process holds under-reports the moment the human has not read
    /// in a while. The count has to come from the database, and it is a count
    /// rather than a read: deserializing the history to size a badge would
    /// undo the tail it exists to keep.
    pub fn unread_attention_between(
        &self,
        agent_id: &str,
        cursor: u64,
        floor: u64,
    ) -> Result<u64, StoreError> {
        if floor == 0 || floor <= cursor {
            return Ok(0);
        }
        let count: i64 = self.connection().query_row(
            "SELECT COUNT(*) FROM thread_items
             WHERE agent_id = ?1 AND attention = 1 AND sequence > ?2 AND sequence < ?3",
            rusqlite::params![agent_id, cursor as i64, floor as i64],
            |row| row.get(0),
        )?;
        Ok(count as u64)
    }
    /// Whether a Task exists. The question `task_record_path(..).is_file()`
    /// used to answer.
    pub fn task_exists(&self, task_id: &str) -> bool {
        self.connection()
            .query_row("SELECT 1 FROM tasks WHERE id = ?1", [task_id], |_| Ok(()))
            .optional()
            .map(|found| found.is_some())
            .unwrap_or(false)
    }
    /// Write a Task's own record and agents. Its implementations are separate
    /// rows and are not touched here — which is the point: saving a Task no
    /// longer rewrites every implementation inside it.
    pub fn save_task_plan(&self, record: &PersistedPlan) -> Result<(), StoreError> {
        self.in_transaction(|tx| {
            write_task(tx, record)?;
            Store::write_agents(tx, &record.id, &record.agents)
        })
    }
    /// Every Task with its implementations, oldest first.
    pub fn load_all_tasks(&self) -> Result<Vec<PersistedTask>, StoreError> {
        let conn = self.connection();
        let mut tasks = Vec::new();
        for task in Store::read_task_records(&conn, Store::read_agents)? {
            let implementations = Store::read_runs(&conn, Some(&task.id))?;
            tasks.push(PersistedTask {
                task,
                implementations,
            });
        }
        Ok(tasks)
    }
    /// Every Task's own record, oldest first, its agents read by `read_agents`.
    fn read_task_records(
        conn: &Connection,
        read_agents: ReadAgents,
    ) -> Result<Vec<PersistedPlan>, StoreError> {
        let mut statement = conn.prepare("SELECT id, record FROM tasks ORDER BY created_at, id")?;
        let rows: Vec<(String, String)> = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<Result<_, _>>()?;
        drop(statement);
        let mut tasks = Vec::with_capacity(rows.len());
        for (id, raw) in rows {
            let mut task: PersistedPlan =
                serde_json::from_str(&raw).map_err(|source| StoreError::Corrupt {
                    path: PathBuf::from(format!("{}/{id}", Store::PLANS_DIR)),
                    source,
                })?;
            let task_choice = ModelChoice {
                provider: task.provider,
                model: task.model.clone(),
                effort: task.effort.clone(),
            };
            task.agents = read_agents(conn, &id, &task_choice, None)?;
            tasks.push(task);
        }
        Ok(tasks)
    }
    /// Delete a Task, its implementations, every conversation on them, and
    /// its canonical plan docs.
    ///
    /// The docs are the filesystem half of the delete: they deliberately live
    /// outside the database, so removing only rows would leave the Task's
    /// plan on disk forever with nothing referring to it.
    pub fn delete_plan(&self, plan_id: &str) -> Result<(), StoreError> {
        remove_dir_if_present(&self.task_dir(plan_id))?;
        self.in_transaction(|tx| {
            let mut owned = tx.prepare("SELECT id FROM implementations WHERE task_id = ?1")?;
            let runs: Vec<String> = owned
                .query_map([plan_id], |row| row.get::<_, String>(0))?
                .flatten()
                .collect();
            drop(owned);
            for owner in std::iter::once(plan_id.to_string()).chain(runs) {
                tx.execute(
                    "DELETE FROM inbox_retained_run_messages WHERE run_id = ?1",
                    [&owner],
                )?;
                tx.execute(
                    "DELETE FROM thread_items WHERE agent_id IN
                     (SELECT id FROM agents WHERE owner_id = ?1)",
                    [&owner],
                )?;
                tx.execute("DELETE FROM agents WHERE owner_id = ?1", [&owner])?;
            }
            tx.execute("DELETE FROM implementations WHERE task_id = ?1", [plan_id])?;
            tx.execute("DELETE FROM tasks WHERE id = ?1", [plan_id])?;
            Ok(())
        })
    }
    /// Write one run and its agents, whether or not it belongs to a Task.
    pub fn save_run(&self, record: &PersistedRun) -> Result<(), StoreError> {
        self.in_transaction(|tx| {
            write_run(tx, record)?;
            Store::write_agents(tx, &record.id, &record.agents)
        })
    }
    /// Runs belonging to `task_id`, or every run when it is `None`.
    pub(super) fn read_runs(
        conn: &Connection,
        task_id: Option<&str>,
    ) -> Result<Vec<PersistedRun>, StoreError> {
        Store::read_runs_with(conn, task_id, Store::read_agents)
    }

    fn read_runs_with(
        conn: &Connection,
        task_id: Option<&str>,
        read_agents: ReadAgents,
    ) -> Result<Vec<PersistedRun>, StoreError> {
        let (sql, bind): (&str, Vec<&str>) = match task_id {
            Some(id) => (
                "SELECT id, record FROM implementations WHERE task_id = ?1
                 ORDER BY created_at, id",
                vec![id],
            ),
            None => (
                "SELECT id, record FROM implementations ORDER BY created_at, id",
                Vec::new(),
            ),
        };
        let mut statement = conn.prepare(sql)?;
        let rows: Vec<(String, String)> = statement
            .query_map(rusqlite::params_from_iter(bind), |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<Result<_, _>>()?;
        drop(statement);
        let mut runs = Vec::with_capacity(rows.len());
        for (id, raw) in rows {
            let mut run: PersistedRun =
                serde_json::from_str(&raw).map_err(|source| StoreError::Corrupt {
                    path: PathBuf::from(format!("implementations/{id}")),
                    source,
                })?;
            let run_choice = ModelChoice {
                provider: run.provider,
                model: run.model.clone(),
                effort: run.effort.clone(),
            };
            let shared_primary = run
                .plan_id
                .as_deref()
                .and_then(|task_id| primary_agent_id(conn, task_id).ok().flatten());
            run.agents = read_agents(conn, &id, &run_choice, shared_primary.as_deref())?;
            runs.push(run);
        }
        Ok(runs)
    }
    /// Every run, oldest first — a Task's implementations and the planless
    /// adopted ones alike. Boot reattaches from this one list.
    pub fn load_all_runs(&self) -> Result<Vec<PersistedRun>, StoreError> {
        let conn = self.connection();
        Store::read_runs(&conn, None)
    }
    /// Every run as [`load_all_runs`](Self::load_all_runs) reads it, less its
    /// agents' conversations: who ran where, for a caller that only names
    /// agents — whose cost is then the agent rows, not their conversations.
    pub fn load_all_run_rosters(&self) -> Result<Vec<PersistedRun>, StoreError> {
        let conn = self.connection();
        Store::read_runs_with(&conn, None, Store::read_agent_records)
    }
    /// Every Task's own record with its agents but not their conversations,
    /// and without its implementations — the
    /// [`load_all_run_rosters`](Self::load_all_run_rosters) of Tasks.
    pub fn load_all_plan_rosters(&self) -> Result<Vec<PersistedPlan>, StoreError> {
        let conn = self.connection();
        Store::read_task_records(&conn, Store::read_agent_records)
    }
    /// Every Task's own record, oldest first, without its implementations.
    pub fn load_all_plans(&self) -> Result<Vec<PersistedPlan>, StoreError> {
        Ok(self
            .load_all_tasks()?
            .into_iter()
            .map(|task| task.task)
            .collect())
    }
    pub fn delete_run(&self, run_id: &str) -> Result<(), StoreError> {
        self.in_transaction(|tx| {
            tx.execute(
                "DELETE FROM inbox_retained_run_messages WHERE run_id = ?1",
                [run_id],
            )?;
            tx.execute(
                "DELETE FROM thread_items WHERE agent_id IN
                 (SELECT id FROM agents WHERE owner_id = ?1)",
                [run_id],
            )?;
            tx.execute("DELETE FROM agents WHERE owner_id = ?1", [run_id])?;
            tx.execute("DELETE FROM implementations WHERE id = ?1", [run_id])?;
            Ok(())
        })
    }
    /// Done deletes the workspace conversation but keeps its message times in
    /// the project's session history. Copy and delete are one transaction so
    /// a failed deletion cannot lose the only remaining timestamps.
    pub fn delete_run_retaining_inbox_messages(
        &self,
        run_id: &str,
        project_id: &str,
    ) -> Result<(), StoreError> {
        self.in_transaction(|tx| {
            tx.execute(
                "INSERT INTO inbox_retained_messages (project_id, ts_ms) \
                 SELECT ?2, ts_ms FROM inbox_retained_run_messages WHERE run_id = ?1",
                rusqlite::params![run_id, project_id],
            )?;
            tx.execute(
                "DELETE FROM inbox_retained_run_messages WHERE run_id = ?1",
                [run_id],
            )?;
            let mut query = tx.prepare(
                "SELECT json_extract(t.item, '$.data.created_at') \
                 FROM thread_items t JOIN agents a ON a.id = t.agent_id \
                 WHERE a.owner_id = ?1 AND t.message = 1 \
                 AND COALESCE(json_extract(t.item, '$.data.from_build'), 0) = 0",
            )?;
            let times = query
                .query_map([run_id], |row| row.get::<_, Option<String>>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            drop(query);
            for at in times.into_iter().flatten() {
                if let Some(ts) = crate::session_summary::message_millis(&at) {
                    tx.execute(
                        "INSERT INTO inbox_retained_messages (project_id, ts_ms) VALUES (?1, ?2)",
                        rusqlite::params![project_id, ts],
                    )?;
                }
            }
            tx.execute(
                "DELETE FROM thread_items WHERE agent_id IN
                 (SELECT id FROM agents WHERE owner_id = ?1)",
                [run_id],
            )?;
            tx.execute("DELETE FROM agents WHERE owner_id = ?1", [run_id])?;
            tx.execute("DELETE FROM implementations WHERE id = ?1", [run_id])?;
            Ok(())
        })
    }
    /// A deleted project no longer has an inbox session. Clear all message
    /// times retained for it only after its configuration has been removed;
    /// until then a failed multi-step deletion can leave the project alive.
    pub fn clear_retained_project_messages(&self, project_id: &str) -> Result<(), StoreError> {
        self.in_transaction(|tx| {
            tx.execute(
                "DELETE FROM inbox_retained_messages WHERE project_id = ?1",
                [project_id],
            )?;
            Ok(())
        })
    }
    pub fn save_capture(&self, record: &crate::capture::Capture) -> Result<(), StoreError> {
        self.in_transaction(|tx| {
            tx.execute(
                "INSERT INTO captures (id, record) VALUES (?1, ?2)
                 ON CONFLICT(id) DO UPDATE SET record = ?2",
                rusqlite::params![
                    record.id,
                    serde_json::to_string(record).expect("a capture always serializes")
                ],
            )?;
            Ok(())
        })
    }
    pub fn delete_capture(&self, capture_id: &str) -> Result<(), StoreError> {
        self.in_transaction(|tx| {
            tx.execute("DELETE FROM captures WHERE id = ?1", [capture_id])?;
            Ok(())
        })
    }
    pub fn load_all_captures(&self) -> Result<Vec<crate::capture::Capture>, StoreError> {
        let conn = self.connection();
        let mut statement = conn.prepare("SELECT id, record FROM captures")?;
        let rows: Vec<(String, String)> = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<Result<_, _>>()?;
        let mut captures = Vec::with_capacity(rows.len());
        for (id, raw) in rows {
            captures.push(
                serde_json::from_str(&raw).map_err(|source| StoreError::Corrupt {
                    path: PathBuf::from(format!("captures/{id}")),
                    source,
                })?,
            );
        }
        captures.sort_by(|a: &crate::capture::Capture, b| a.created_at.cmp(&b.created_at));
        Ok(captures)
    }
    pub fn save_archived_worktree(
        &self,
        record: &PersistedArchivedWorktree,
    ) -> Result<(), StoreError> {
        self.in_transaction(|tx| {
            tx.execute(
                "INSERT INTO archived_worktrees (id, record) VALUES (?1, ?2)
                 ON CONFLICT(id) DO UPDATE SET record = ?2",
                rusqlite::params![
                    record.worktree_id,
                    serde_json::to_string(record).expect("an archived worktree always serializes")
                ],
            )?;
            Ok(())
        })
    }
    pub fn load_all_archived_worktrees(
        &self,
    ) -> Result<Vec<PersistedArchivedWorktree>, StoreError> {
        let conn = self.connection();
        let mut statement = conn.prepare("SELECT id, record FROM archived_worktrees")?;
        let rows: Vec<(String, String)> = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<Result<_, _>>()?;
        let mut archived = Vec::with_capacity(rows.len());
        for (id, raw) in rows {
            archived.push(
                serde_json::from_str(&raw).map_err(|source| StoreError::Corrupt {
                    path: PathBuf::from(format!("archived_worktrees/{id}")),
                    source,
                })?,
            );
        }
        archived.sort_by(|a: &PersistedArchivedWorktree, b| {
            a.archived_at
                .cmp(&b.archived_at)
                .then(a.worktree_id.cmp(&b.worktree_id))
        });
        Ok(archived)
    }
    /// Read one canonical plan doc by its worktree-relative path. `None` when
    /// the doc does not exist (or the path tries to escape the docs dir).
    pub fn read_plan_doc(&self, plan_id: &str, rel_path: &str) -> Option<String> {
        if !is_worktree_contained_path(rel_path) {
            return None;
        }
        std::fs::read_to_string(self.plan_docs_dir(plan_id).join(rel_path)).ok()
    }
}

pub(super) fn primary_agent_id(
    conn: &Connection,
    owner_id: &str,
) -> Result<Option<String>, StoreError> {
    conn.query_row(
        "SELECT id FROM agents WHERE owner_id = ?1 ORDER BY ordinal LIMIT 1",
        [owner_id],
        |row| row.get(0),
    )
    .optional()
    .map_err(StoreError::from)
}

/// Schema-v6 migration: freeze the model choice each legacy agent effectively
/// used and turn the old first-agent Task/run alias into an explicit
/// conversation id. The original skeleton is retained once before overwrite;
/// conversation rows stay exactly where they are.
pub(super) fn migrate_agents_to_v6(conn: &mut Connection) -> Result<(), StoreError> {
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let owners = load_legacy_owner_context(&tx)?;
    let primary_agents = load_primary_agent_ids(&tx)?;
    let rows: Vec<(String, String, String)> = {
        let mut statement = tx.prepare("SELECT id, owner_id, record FROM agents")?;
        let rows = statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
            .collect::<Result<_, _>>()?;
        rows
    };
    for (agent_id, owner_id, raw) in rows {
        let mut agent: Agent =
            serde_json::from_str(&raw).map_err(|source| StoreError::Corrupt {
                path: PathBuf::from(format!("agents/{agent_id}")),
                source,
            })?;
        let Some(context) = owners.get(&owner_id) else {
            continue;
        };
        let shared = context
            .task_id
            .as_deref()
            .filter(|_| primary_agents.get(&owner_id) == Some(&agent_id))
            .and_then(|task_id| primary_agents.get(task_id))
            .map(String::as_str);
        if !materialize_agent_identity(&mut agent, &context.choice, shared) {
            continue;
        }
        tx.execute(
            "INSERT OR IGNORE INTO agent_migration_backups \
             (agent_id, migration_version, record) VALUES (?1, 6, ?2)",
            rusqlite::params![agent_id, raw],
        )?;
        tx.execute(
            "UPDATE agents SET record = ?2 WHERE id = ?1",
            rusqlite::params![
                agent_id,
                serde_json::to_string(&agent).expect("an agent always serializes")
            ],
        )?;
    }
    tx.commit()?;
    Ok(())
}

pub(super) fn load_primary_agent_ids(
    conn: &Connection,
) -> Result<HashMap<String, String>, StoreError> {
    let mut statement = conn.prepare(
        "SELECT owner_id, id FROM agents \
         WHERE ordinal = (SELECT MIN(first.ordinal) FROM agents first \
                          WHERE first.owner_id = agents.owner_id)",
    )?;
    let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
    rows.collect::<Result<_, _>>().map_err(StoreError::from)
}

pub(super) fn write_task(
    tx: &rusqlite::Transaction,
    record: &PersistedPlan,
) -> Result<(), StoreError> {
    let mut skeleton = record.clone();
    skeleton.agents.clear();
    tx.execute(
        "INSERT INTO tasks (id, created_at, updated_at, record) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(id) DO UPDATE SET created_at = ?2, updated_at = ?3, record = ?4",
        rusqlite::params![
            record.id,
            record.created_at,
            record.updated_at,
            serde_json::to_string(&skeleton).expect("a Task always serializes")
        ],
    )?;
    Ok(())
}

pub(super) fn write_run(
    tx: &rusqlite::Transaction,
    record: &PersistedRun,
) -> Result<(), StoreError> {
    let mut skeleton = record.clone();
    skeleton.agents.clear();
    tx.execute(
        "INSERT INTO implementations (id, task_id, created_at, updated_at, record)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(id) DO UPDATE SET
             task_id = ?2, created_at = ?3, updated_at = ?4, record = ?5",
        rusqlite::params![
            record.id,
            record.plan_id,
            record.created_at,
            record.updated_at,
            serde_json::to_string(&skeleton).expect("a run always serializes")
        ],
    )?;
    Ok(())
}

/// One-way read migration from entity-owned choices and implicit transcript
/// ownership. It changes only the small agent skeleton; transcript rows remain
/// under the same existing storage id.
pub(super) fn materialize_agent_identity(
    agent: &mut Agent,
    entity_choice: &ModelChoice,
    shared_primary_conversation: Option<&str>,
) -> bool {
    let mut changed = false;
    if agent.settings_version < CURRENT_SETTINGS_VERSION {
        if agent.choice.provider == entity_choice.provider {
            agent.choice = entity_choice.clone();
        }
        agent.settings_version = CURRENT_SETTINGS_VERSION;
        changed = true;
    }
    if agent.conversation_id.is_none() {
        agent.conversation_id = Some(shared_primary_conversation.unwrap_or(&agent.id).to_string());
        changed = true;
    }
    changed
}

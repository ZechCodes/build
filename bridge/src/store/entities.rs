use super::{
    load_legacy_owner_context, read_thread_page, remove_dir_if_present,
    stored_conversation_summary, Store, StoreError, RESIDENT_CONVERSATION_TAIL,
    THREAD_ACTIVITY_COUNT_SQL, THREAD_FIRST_ATTENTION_AFTER_SQL, THREAD_ITEM_COUNT_SQL,
    THREAD_LAST_ATTENTION_SQL, THREAD_LAST_MESSAGE_SQL, THREAD_LAST_SEQUENCE_SQL, THREAD_PAGE_SQL,
    THREAD_TOOL_CALL_COUNT_SQL,
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

/// The durable core of one plan — the project-scoped half of the split. Its
/// canonical docs live beside the record under `plans/<plan_id>/docs/`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersistedPlan {
    pub id: String,
    pub goal: String,
    /// Canonical path of the project repo the plan belongs to. Stored as a
    /// path (not the in-memory project id) because project ids are re-minted
    /// on every boot.
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
    /// "Run all": auto-dispatch the next approved stage when validation passes.
    #[serde(default)]
    pub auto_advance: bool,
    /// True for a run minted around a pre-existing (user-created) worktree.
    #[serde(default)]
    pub adopted: bool,
    /// The last triage pass over this run's diff. Presentational, so it is
    /// persisted purely so a restart does not throw away an ordering the
    /// reviewer was reading; nothing waits on it and nothing reads it back into
    /// the lifecycle.
    #[serde(default)]
    pub triage: Option<crate::run::TriageReport>,
    /// Durable nonce-bound recovery journal. A started attempt survives daemon
    /// restart and can never be mistaken for a verified success.
    #[serde(default)]
    pub recovery: Option<crate::run::RecoveryAttempt>,
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
}

/// Canonical durable Issue aggregate. Planning state, stage-plan review,
/// implementation lineage, threads, and publication journals cross the crash
/// boundary as one record instead of being reconstructed from mutable halves.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersistedIssue {
    pub issue: PersistedPlan,
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
/// through `worktree.finish`. Project ids are intentionally absent because they
/// are re-minted at boot; the canonical project path is the stable identity.
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

impl Store {
    /// Classify every stored item for the freshly added columns.
    ///
    /// The one place Build reads whole conversations on purpose: it runs once,
    /// on the upgrade, because a column added with a default says nothing about
    /// the items already under it. Every column is written on every upgrade
    /// path — a v1 database gains them together, and rewriting one with the
    /// value it already holds is what makes a single classifier serve all
    /// three.
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
                                     activity = ?6 \
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
                i64::from(item.is_activity())
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
    /// aggregate that rewrote every conversation on the Issue for every append;
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
              activity, item)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(agent_id, sequence)
             DO UPDATE SET updated_sequence = ?3, attention = ?4, message = ?5, \
                           tool_call = ?6, activity = ?7, item = ?8",
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
    pub(super) fn read_agents(
        conn: &Connection,
        owner_id: &str,
        entity_choice: &ModelChoice,
        shared_primary_conversation: Option<&str>,
    ) -> Result<Vec<Agent>, StoreError> {
        let mut statement =
            conn.prepare("SELECT id, record FROM agents WHERE owner_id = ?1 ORDER BY ordinal")?;
        let rows: Vec<(String, String)> = statement
            .query_map([owner_id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<Result<_, _>>()?;
        // The tail of each conversation, not the whole of it: a boot that read
        // every item of every conversation would spend, in one go and for the
        // life of the process, exactly what the paged reads exist to save.
        let mut tail = conn.prepare(THREAD_PAGE_SQL)?;
        let mut count = conn.prepare(THREAD_ITEM_COUNT_SQL)?;
        let mut last_sequence = conn.prepare(THREAD_LAST_SEQUENCE_SQL)?;
        let mut last_message = conn.prepare(THREAD_LAST_MESSAGE_SQL)?;
        let mut first_attention_after = conn.prepare(THREAD_FIRST_ATTENTION_AFTER_SQL)?;
        let mut last_attention = conn.prepare(THREAD_LAST_ATTENTION_SQL)?;
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
            agent.thread.adopt_conversation_summary(
                summary.last_message_sequence,
                last_attention_sequence,
                summary.activity_at,
                summary.working,
            );
            agents.push(agent);
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
    /// Whether an Issue exists. The question `issue_record_path(..).is_file()`
    /// used to answer.
    pub fn issue_exists(&self, issue_id: &str) -> bool {
        self.connection()
            .query_row("SELECT 1 FROM issues WHERE id = ?1", [issue_id], |_| Ok(()))
            .optional()
            .map(|found| found.is_some())
            .unwrap_or(false)
    }
    /// Write an Issue's own record and agents. Its implementations are separate
    /// rows and are not touched here — which is the point: saving an Issue no
    /// longer rewrites every implementation inside it.
    pub fn save_issue_plan(&self, record: &PersistedPlan) -> Result<(), StoreError> {
        self.in_transaction(|tx| {
            write_issue(tx, record)?;
            Store::write_agents(tx, &record.id, &record.agents)
        })
    }
    /// Every Issue with its implementations, oldest first.
    pub fn load_all_issues(&self) -> Result<Vec<PersistedIssue>, StoreError> {
        let conn = self.connection();
        let mut statement =
            conn.prepare("SELECT id, record FROM issues ORDER BY created_at, id")?;
        let rows: Vec<(String, String)> = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<Result<_, _>>()?;
        drop(statement);
        let mut issues = Vec::with_capacity(rows.len());
        for (id, raw) in rows {
            let mut issue: PersistedPlan =
                serde_json::from_str(&raw).map_err(|source| StoreError::Corrupt {
                    path: PathBuf::from(format!("issues/{id}")),
                    source,
                })?;
            let issue_choice = ModelChoice {
                provider: issue.provider,
                model: issue.model.clone(),
                effort: issue.effort.clone(),
            };
            issue.agents = Store::read_agents(&conn, &id, &issue_choice, None)?;
            let implementations = Store::read_runs(&conn, Some(&id))?;
            issues.push(PersistedIssue {
                issue,
                implementations,
            });
        }
        Ok(issues)
    }
    /// Delete an Issue, its implementations, every conversation on them, and
    /// its canonical plan docs.
    ///
    /// The docs are the filesystem half of the delete: they deliberately live
    /// outside the database, so removing only rows would leave the Issue's
    /// plan on disk forever with nothing referring to it.
    pub fn delete_plan(&self, plan_id: &str) -> Result<(), StoreError> {
        remove_dir_if_present(&self.issue_dir(plan_id))?;
        self.in_transaction(|tx| {
            let mut owned = tx.prepare("SELECT id FROM implementations WHERE issue_id = ?1")?;
            let runs: Vec<String> = owned
                .query_map([plan_id], |row| row.get::<_, String>(0))?
                .flatten()
                .collect();
            drop(owned);
            for owner in std::iter::once(plan_id.to_string()).chain(runs) {
                tx.execute(
                    "DELETE FROM thread_items WHERE agent_id IN
                     (SELECT id FROM agents WHERE owner_id = ?1)",
                    [&owner],
                )?;
                tx.execute("DELETE FROM agents WHERE owner_id = ?1", [&owner])?;
            }
            tx.execute("DELETE FROM implementations WHERE issue_id = ?1", [plan_id])?;
            tx.execute("DELETE FROM issues WHERE id = ?1", [plan_id])?;
            Ok(())
        })
    }
    /// Write one run and its agents, whether or not it belongs to an Issue.
    pub fn save_run(&self, record: &PersistedRun) -> Result<(), StoreError> {
        self.in_transaction(|tx| {
            write_run(tx, record)?;
            Store::write_agents(tx, &record.id, &record.agents)
        })
    }
    /// Runs belonging to `issue_id`, or every run when it is `None`.
    pub(super) fn read_runs(
        conn: &Connection,
        issue_id: Option<&str>,
    ) -> Result<Vec<PersistedRun>, StoreError> {
        let (sql, bind): (&str, Vec<&str>) = match issue_id {
            Some(id) => (
                "SELECT id, record FROM implementations WHERE issue_id = ?1
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
                .and_then(|issue_id| primary_agent_id(conn, issue_id).ok().flatten());
            run.agents = Store::read_agents(conn, &id, &run_choice, shared_primary.as_deref())?;
            runs.push(run);
        }
        Ok(runs)
    }
    /// Every run, oldest first — an Issue's implementations and the planless
    /// adopted ones alike. Boot reattaches from this one list.
    pub fn load_all_runs(&self) -> Result<Vec<PersistedRun>, StoreError> {
        let conn = self.connection();
        Store::read_runs(&conn, None)
    }
    /// Every Issue's own record, oldest first, without its implementations.
    pub fn load_all_plans(&self) -> Result<Vec<PersistedPlan>, StoreError> {
        Ok(self
            .load_all_issues()?
            .into_iter()
            .map(|issue| issue.issue)
            .collect())
    }
    pub fn delete_run(&self, run_id: &str) -> Result<(), StoreError> {
        self.in_transaction(|tx| {
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
/// used and turn the old first-agent Issue/run alias into an explicit
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
            .issue_id
            .as_deref()
            .filter(|_| primary_agents.get(&owner_id) == Some(&agent_id))
            .and_then(|issue_id| primary_agents.get(issue_id))
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

pub(super) fn write_issue(
    tx: &rusqlite::Transaction,
    record: &PersistedPlan,
) -> Result<(), StoreError> {
    let mut skeleton = record.clone();
    skeleton.agents.clear();
    tx.execute(
        "INSERT INTO issues (id, created_at, updated_at, record) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(id) DO UPDATE SET created_at = ?2, updated_at = ?3, record = ?4",
        rusqlite::params![
            record.id,
            record.created_at,
            record.updated_at,
            serde_json::to_string(&skeleton).expect("an Issue always serializes")
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
        "INSERT INTO implementations (id, issue_id, created_at, updated_at, record)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(id) DO UPDATE SET
             issue_id = ?2, created_at = ?3, updated_at = ?4, record = ?5",
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

-- Frozen schema 16 DDL from main 0f164986, before assignment delivery authority.
-- Kept independent of the current schema so upgrades exercise a real old database.
CREATE TABLE IF NOT EXISTS meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS tasks (
    id         TEXT PRIMARY KEY,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    record     TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS implementations (
    id         TEXT PRIMARY KEY,
    task_id   TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    record     TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS implementations_by_task
    ON implementations(task_id, created_at);

CREATE TABLE IF NOT EXISTS agents (
    id       TEXT PRIMARY KEY,
    owner_id TEXT NOT NULL,
    ordinal  INTEGER NOT NULL,
    record   TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS agents_by_owner ON agents(owner_id, ordinal);

-- Draft uploads belong to the generation that accepted them, even before
-- a message references them. Content-addressed bytes may be shared.
CREATE TABLE IF NOT EXISTS conversation_attachment_uploads (
    conversation_id TEXT NOT NULL,
    thread_id TEXT NOT NULL,
    path TEXT NOT NULL,
    PRIMARY KEY (conversation_id, thread_id, path)
);

-- User/agent message times from a workspace removed by Done. Its run and
-- conversation are intentionally deleted, but the project's inbox session
-- must still include those messages after a bridge restart.
CREATE TABLE IF NOT EXISTS inbox_retained_messages (
    project_id TEXT NOT NULL,
    ts_ms      INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS inbox_retained_by_project
    ON inbox_retained_messages(project_id, ts_ms);

-- Message times from removed agents on a still-live run. The conversation
-- goes away with the agent, but its activity still belongs to the workspace
-- and project session until the run itself is removed.
CREATE TABLE IF NOT EXISTS inbox_retained_run_messages (
    run_id TEXT NOT NULL,
    ts_ms  INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS inbox_retained_by_run
    ON inbox_retained_run_messages(run_id, ts_ms);

-- One conversation item. `sequence` orders the conversation and
-- `updated_sequence` carries an in-place mutation (seen, resolved), which is
-- what the client cursor compares against — so both are columns rather than
-- fields buried in the item JSON.
CREATE TABLE IF NOT EXISTS thread_items (
    agent_id         TEXT NOT NULL,
    sequence         INTEGER NOT NULL,
    updated_sequence INTEGER NOT NULL,
    -- 1 when this item calls the human: what an unread badge counts. Hoisted
    -- out of the item JSON because a conversation is loaded as its tail, so the
    -- items under it can only be counted by the database — and counting them
    -- by deserializing every one would undo the tail.
    attention        INTEGER NOT NULL DEFAULT 0,
    -- 1 when this item is a message, either role. Hoisted for the same reason
    -- `attention` is: what a page's limit buys and what a catch-up packet
    -- carries is conversation, and a conversation buried in activity can only
    -- be found under the tail by the database.
    message          INTEGER NOT NULL DEFAULT 0,
    -- 1 when this item is a tool call. Hoisted for the same reason the other
    -- two are: a page folds a run of activity into one row that says how many
    -- calls it made, and the calls a page did not ship can only be counted by
    -- the database.
    tool_call        INTEGER NOT NULL DEFAULT 0,
    -- 1 when this message is one agent's words in another agent's
    -- conversation. Hoisted for the same reason the rest are: the line a
    -- dismissal is judged against is the newest message that is NOT one of
    -- these, and a conversation loaded as its tail cannot find that message
    -- under a long run of hand-offs without the database.
    handoff          INTEGER NOT NULL DEFAULT 0,
    -- 1 when this item is the agent working rather than something said or
    -- decided. Hoisted because a page reads the conversation and the newest of
    -- each run between it: without the column the read has to fetch a run to
    -- find out where it ends, which is the cost the bounded read exists to
    -- skip. `message = 1 OR attention = 1` is not this rule read backwards --
    -- a lifecycle marker is neither conversation nor activity.
    activity         INTEGER NOT NULL DEFAULT 0,
    item             TEXT NOT NULL,
    PRIMARY KEY (agent_id, sequence)
);
CREATE INDEX IF NOT EXISTS thread_items_attention
    ON thread_items(agent_id, attention, sequence);
CREATE INDEX IF NOT EXISTS thread_items_cursor
    ON thread_items(agent_id, updated_sequence);
-- The counted rule, indexed: an item the human reads as conversation. Partial,
-- so the index holds the conversation and not the activity between it, and
-- every statement that seeks down it repeats the predicate verbatim.
CREATE INDEX IF NOT EXISTS thread_items_conversation
    ON thread_items(agent_id, sequence) WHERE message = 1 OR attention = 1;
-- The page measure, indexed: a page's limit buys messages, so the seek that
-- finds where a page reaches back to walks the words and never the work
-- between them.
CREATE INDEX IF NOT EXISTS thread_items_messages
    ON thread_items(agent_id, sequence) WHERE message = 1;
-- The dismissal line, indexed: what a row was cleared against is the newest
-- message the conversation's own two parties spoke, so the seek that finds it
-- walks past the hand-offs between them without reading one.
CREATE INDEX IF NOT EXISTS thread_items_own_messages
    ON thread_items(agent_id, sequence) WHERE message = 1 AND handoff = 0;
-- The census, indexed: a page's digest counts the tool calls of a run exactly,
-- including the ones the per-run cap left off the wire. Partial and covering,
-- so the count is a seek down the calls themselves and never reads a row.
CREATE INDEX IF NOT EXISTS thread_items_tool_calls
    ON thread_items(agent_id, sequence) WHERE tool_call = 1;
-- The runs, indexed: a page reads the newest of each run between two things
-- somebody said, and the span above a page's floor holds far more work than
-- conversation. Partial, so the seek that bounds a run walks the work itself.
CREATE INDEX IF NOT EXISTS thread_items_activity
    ON thread_items(agent_id, sequence) WHERE activity = 1;
-- Whole-run edges seek through conversation structure, skipping activity
-- without visiting its large item bodies. Applied on every store open so
-- existing databases gain the index without rewriting stored items.
CREATE INDEX IF NOT EXISTS thread_items_structure
    ON thread_items(agent_id, sequence) WHERE activity = 0;

CREATE TABLE IF NOT EXISTS captures (
    id     TEXT PRIMARY KEY,
    record TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS attention (
    entity_id TEXT PRIMARY KEY,
    record    TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS archived_worktrees (
    id     TEXT PRIMARY KEY,
    record TEXT NOT NULL
);

-- An accepted mutation and the provider delivery it requires. The transcript
-- row is written in the same SQLite transaction as this receipt. `claimed` is
-- persisted before handoff so a restart can distinguish safe queued work from
-- an ambiguous provider write.
CREATE TABLE IF NOT EXISTS operations (
    operation_id    TEXT PRIMARY KEY,
    method          TEXT NOT NULL,
    entity_id       TEXT NOT NULL,
    agent_id        TEXT NOT NULL,
    conversation_id TEXT NOT NULL,
    choice_revision INTEGER NOT NULL,
    request_hash    TEXT NOT NULL,
    posted_sequence INTEGER NOT NULL,
    message_start_sequence INTEGER NOT NULL,
    status          TEXT NOT NULL,
    execution_error TEXT,
    delivery        TEXT,
    -- The agent that asked for this operation, when one did. Null is the human.
    requested_by    TEXT,
    created_at      TEXT NOT NULL,
    updated_at      TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS operations_by_status ON operations(status, created_at);

-- The per-project task tracker (spec: Tasks). NOT the `tasks` table above:
-- that one is the retired plan-and-stages flow, which shares the English word
-- and nothing else, so these are namespaced apart and neither reads the other.
--
-- A record is a record: a task, a comment and an event are each small,
-- bounded and read and written whole, so each keeps its serde shape in
-- `record`. Only what is QUERIED is hoisted into a column — the project key
-- and number (the list read and the number mint), the state and status (the
-- two filters answered in SQL), and the timestamps (the ordering). An
-- assignee and a label filter are read out of the record: hoisting a label
-- list would mean a join table, which phase 1 does not need.
--
-- `project_key` is the project's canonical repository PATH, not its `proj-N`
-- id: an id is minted per boot from the config that restored it, so an
-- id-keyed row would strand its tasks when the same repository comes back
-- wearing another one. `PersistedPlan` and `PersistedRun` carry a path for
-- exactly this reason.
CREATE TABLE IF NOT EXISTS tracker_tasks (
    id          TEXT PRIMARY KEY,
    project_key TEXT NOT NULL,
    number      INTEGER NOT NULL,
    state       TEXT NOT NULL,
    status      TEXT NOT NULL,
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL,
    record      TEXT NOT NULL
);
-- The number mint's backstop. Numbers are per project and never reused, so a
-- second writer that read the same maximum fails here rather than handing two
-- tasks one number.
CREATE UNIQUE INDEX IF NOT EXISTS tracker_tasks_number
    ON tracker_tasks(project_key, number);
-- The list read: one project's tasks, newest first, as a seek down the index
-- rather than a scan and a sort.
CREATE INDEX IF NOT EXISTS tracker_tasks_by_project
    ON tracker_tasks(project_key, number DESC);
-- The same seek narrowed by a column filter, so a page of the open tasks,
-- or of a column nobody is in, reads only the rows it answers (#85). Both
-- filters together have an index of their own: through either one alone, a
-- page of the closed tasks in a column full of open ones would step over
-- every open one to find nothing.
CREATE INDEX IF NOT EXISTS tracker_tasks_by_state
    ON tracker_tasks(project_key, state, number DESC);
CREATE INDEX IF NOT EXISTS tracker_tasks_by_status
    ON tracker_tasks(project_key, status, number DESC);
CREATE INDEX IF NOT EXISTS tracker_tasks_by_state_status
    ON tracker_tasks(project_key, state, status, number DESC);

CREATE TABLE IF NOT EXISTS tracker_comments (
    id         TEXT PRIMARY KEY,
    task_id   TEXT NOT NULL,
    created_at TEXT NOT NULL,
    record     TEXT NOT NULL
);
-- Half of a timeline read, ordered as the timeline is: when it happened, then
-- the id, which is time-ordered itself.
CREATE INDEX IF NOT EXISTS tracker_comments_by_task
    ON tracker_comments(task_id, created_at, id);

CREATE TABLE IF NOT EXISTS tracker_events (
    id       TEXT PRIMARY KEY,
    task_id TEXT NOT NULL,
    at       TEXT NOT NULL,
    record   TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS tracker_events_by_task
    ON tracker_events(task_id, at, id);

-- W1: the relation is the sole writable dependency authority. Old tasks
-- acquire neither edges nor dispatch authority when these empty tables land.
CREATE TABLE IF NOT EXISTS tracker_task_dependencies (
    task_id TEXT NOT NULL REFERENCES tracker_tasks(id) ON DELETE CASCADE,
    waiting_on_task_id TEXT NOT NULL REFERENCES tracker_tasks(id) ON DELETE CASCADE,
    position INTEGER NOT NULL,
    task_number INTEGER NOT NULL,
    PRIMARY KEY(task_id, waiting_on_task_id),
    CHECK(task_id <> waiting_on_task_id)
);
CREATE INDEX IF NOT EXISTS tracker_dependencies_reverse
    ON tracker_task_dependencies(waiting_on_task_id, task_number);
CREATE TABLE IF NOT EXISTS tracker_dependency_readiness (
    task_id TEXT PRIMARY KEY REFERENCES tracker_tasks(id) ON DELETE CASCADE,
    blocked INTEGER NOT NULL DEFAULT 0,
    removed_task_ids TEXT NOT NULL DEFAULT '[]'
);
CREATE TABLE IF NOT EXISTS tracker_dependency_reconcile (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    source_task_id TEXT NOT NULL REFERENCES tracker_tasks(id) ON DELETE CASCADE,
    after_number INTEGER NOT NULL DEFAULT 0,
    self_done INTEGER NOT NULL DEFAULT 0,
    fanout INTEGER NOT NULL DEFAULT 1,
    resolution_changed INTEGER NOT NULL DEFAULT 0,
    at TEXT NOT NULL,
    cause_event_ids TEXT NOT NULL DEFAULT '[]'
);
CREATE INDEX IF NOT EXISTS tracker_reconcile_by_source
    ON tracker_dependency_reconcile(source_task_id, id DESC);
DROP INDEX IF EXISTS tracker_reconcile_lifecycle_causes;
CREATE INDEX IF NOT EXISTS tracker_reconcile_resolution_causes
    ON tracker_dependency_reconcile(source_task_id, id DESC)
    WHERE fanout = 1 AND resolution_changed = 1;
-- Transition receipts are retained for W2 to admit notices/delivery. A read
-- or an old assignment can never create one.
CREATE TABLE IF NOT EXISTS tracker_dependency_outbox (
    event_id TEXT PRIMARY KEY REFERENCES tracker_events(id) ON DELETE CASCADE,
    task_id TEXT NOT NULL REFERENCES tracker_tasks(id) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS tracker_dependency_changes (
    task_id TEXT PRIMARY KEY REFERENCES tracker_tasks(id) ON DELETE CASCADE
);

-- One task review names one workspace. Snapshots are separate rows so adding
-- one never rewrites older metadata. Git bodies and patches stay in Git.
CREATE TABLE IF NOT EXISTS reviews (
    task_id      TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL,
    version      INTEGER NOT NULL,
    record       TEXT NOT NULL
);
-- Match discovery's exact predicate so inactive history is never scanned.
CREATE INDEX IF NOT EXISTS reviews_active_sync ON reviews(task_id)
    WHERE json_extract(record, '$.mode') = 'pull_request'
    AND json_extract(record, '$.pull_request.status') IN ('open', 'changes_requested', 'approved');
CREATE TABLE IF NOT EXISTS review_snapshots (
    id       TEXT PRIMARY KEY,
    task_id  TEXT NOT NULL,
    number   INTEGER NOT NULL,
    record   TEXT NOT NULL,
    UNIQUE(task_id, number)
);
CREATE INDEX IF NOT EXISTS review_snapshots_by_task
    ON review_snapshots(task_id, number);

CREATE TABLE IF NOT EXISTS review_actions (
    id       TEXT PRIMARY KEY,
    task_id  TEXT NOT NULL,
    status   TEXT NOT NULL,
    record   TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS review_actions_by_task ON review_actions(task_id);
CREATE INDEX IF NOT EXISTS review_actions_by_source ON review_actions(
    CASE WHEN json_valid(record) THEN json_extract(record, '$.source_path') END
);

-- Claims span opening preparation and active published PRs. Failed/interrupted
-- preparation retains ownership until its owned Git state is safely unwound.
CREATE TABLE IF NOT EXISTS review_workspace_claims (
    workspace_id TEXT PRIMARY KEY,
    project_key TEXT NOT NULL,
    task_id TEXT NOT NULL UNIQUE
);
CREATE TABLE IF NOT EXISTS review_openings (
    project_key TEXT NOT NULL,
    request_id TEXT NOT NULL,
    task_id TEXT NOT NULL UNIQUE,
    number INTEGER NOT NULL,
    version INTEGER NOT NULL,
    state TEXT NOT NULL,
    record TEXT NOT NULL,
    PRIMARY KEY (project_key, request_id),
    UNIQUE (project_key, number)
);
CREATE TABLE IF NOT EXISTS review_branch_bindings (
    task_id TEXT NOT NULL,
    directory_id TEXT NOT NULL,
    repository_id TEXT NOT NULL,
    dedicated_ref TEXT NOT NULL,
    receiving_repository TEXT NOT NULL,
    receiving_ref TEXT NOT NULL,
    record TEXT NOT NULL,
    PRIMARY KEY (task_id, directory_id),
    UNIQUE (repository_id, dedicated_ref),
    UNIQUE (receiving_repository, receiving_ref)
);
-- One outstanding sync capture; retained after task closure for bounded recovery.
CREATE TABLE IF NOT EXISTS review_sync_candidates (
    task_id TEXT PRIMARY KEY,
    snapshot_id TEXT NOT NULL UNIQUE
);
CREATE TABLE IF NOT EXISTS review_sync_observations (
    task_id TEXT NOT NULL,
    directory_id TEXT NOT NULL,
    revision INTEGER NOT NULL,
    record TEXT NOT NULL,
    PRIMARY KEY (task_id, directory_id)
);
CREATE TABLE IF NOT EXISTS review_merge_intents (
    project_key TEXT NOT NULL,
    request_id TEXT NOT NULL,
    task_id TEXT NOT NULL,
    version INTEGER NOT NULL,
    state TEXT NOT NULL,
    record TEXT NOT NULL,
    PRIMARY KEY (project_key, request_id)
);
CREATE INDEX IF NOT EXISTS review_merge_intents_by_task ON review_merge_intents(task_id);
CREATE UNIQUE INDEX IF NOT EXISTS review_merge_running_by_task
    ON review_merge_intents(task_id) WHERE state = 'running';

-- One bounded copy of each pre-v6 agent skeleton. The migration materializes
-- formerly inherited settings and implicit conversation aliases; retaining the
-- source makes that one-way interpretation auditable without copying any
-- transcript rows.
CREATE TABLE IF NOT EXISTS agent_migration_backups (
    agent_id          TEXT NOT NULL,
    migration_version INTEGER NOT NULL,
    record            TEXT NOT NULL,
    PRIMARY KEY (agent_id, migration_version)
);

-- The notification keys push content is sealed to (#200): one per push
-- subscription, keyed by `b64u(SHA-256(endpoint))`, at most 32 kept
-- (`store/push_keys.rs` evicts the oldest `registered_at`, unix millis).
CREATE TABLE IF NOT EXISTS push_keys (
    subscription_id TEXT PRIMARY KEY,
    public_key      TEXT NOT NULL,
    registered_at   INTEGER NOT NULL
);

-- The schema a v9 store was written with, before issues were renamed tasks
-- (#190): what the rename migration upgrades from.
CREATE TABLE IF NOT EXISTS meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS issues (
    id         TEXT PRIMARY KEY,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    record     TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS implementations (
    id         TEXT PRIMARY KEY,
    issue_id   TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    record     TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS implementations_by_issue
    ON implementations(issue_id, created_at);

CREATE TABLE IF NOT EXISTS agents (
    id       TEXT PRIMARY KEY,
    owner_id TEXT NOT NULL,
    ordinal  INTEGER NOT NULL,
    record   TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS agents_by_owner ON agents(owner_id, ordinal);

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

-- The per-project issue tracker (spec: Issues). NOT the `issues` table above:
-- that one is the retired plan-and-stages flow, which shares the English word
-- and nothing else, so these are namespaced apart and neither reads the other.
--
-- A record is a record: an issue, a comment and an event are each small,
-- bounded and read and written whole, so each keeps its serde shape in
-- `record`. Only what is QUERIED is hoisted into a column — the project key
-- and number (the list read and the number mint), the state and status (the
-- two filters answered in SQL), and the timestamps (the ordering). An
-- assignee and a label filter are read out of the record: hoisting a label
-- list would mean a join table, which phase 1 does not need.
--
-- `project_key` is the project's canonical repository PATH, not its `proj-N`
-- id: an id is minted per boot from the config that restored it, so an
-- id-keyed row would strand its issues when the same repository comes back
-- wearing another one. `PersistedPlan` and `PersistedRun` carry a path for
-- exactly this reason.
CREATE TABLE IF NOT EXISTS tracker_issues (
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
-- issues one number.
CREATE UNIQUE INDEX IF NOT EXISTS tracker_issues_number
    ON tracker_issues(project_key, number);
-- The list read: one project's issues, newest first, as a seek down the index
-- rather than a scan and a sort.
CREATE INDEX IF NOT EXISTS tracker_issues_by_project
    ON tracker_issues(project_key, number DESC);
-- The same seek narrowed by a column filter, so a page of the open issues,
-- or of a column nobody is in, reads only the rows it answers (#85). Both
-- filters together have an index of their own: through either one alone, a
-- page of the closed issues in a column full of open ones would step over
-- every open one to find nothing.
CREATE INDEX IF NOT EXISTS tracker_issues_by_state
    ON tracker_issues(project_key, state, number DESC);
CREATE INDEX IF NOT EXISTS tracker_issues_by_status
    ON tracker_issues(project_key, status, number DESC);
CREATE INDEX IF NOT EXISTS tracker_issues_by_state_status
    ON tracker_issues(project_key, state, status, number DESC);

CREATE TABLE IF NOT EXISTS tracker_comments (
    id         TEXT PRIMARY KEY,
    issue_id   TEXT NOT NULL,
    created_at TEXT NOT NULL,
    record     TEXT NOT NULL
);
-- Half of a timeline read, ordered as the timeline is: when it happened, then
-- the id, which is time-ordered itself.
CREATE INDEX IF NOT EXISTS tracker_comments_by_issue
    ON tracker_comments(issue_id, created_at, id);

CREATE TABLE IF NOT EXISTS tracker_events (
    id       TEXT PRIMARY KEY,
    issue_id TEXT NOT NULL,
    at       TEXT NOT NULL,
    record   TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS tracker_events_by_issue
    ON tracker_events(issue_id, at, id);

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

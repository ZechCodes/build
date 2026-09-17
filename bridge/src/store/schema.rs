/// The database schema. Applied on open; `schema_version` in `meta` is what a
/// future change reads to decide whether it has work to do.
///
/// One table carries the design: `thread_items`. Everything else is a small,
/// bounded record that is read and written whole, so those rows keep their
/// serde shape in a `record` column — normalizing them would buy nothing and
/// multiply the diff. A conversation is the opposite: it grows without bound
/// and is appended to constantly, so an item is a row, an append is one
/// `INSERT`, and a page is a `LIMIT`.
pub(super) const SCHEMA: &str = r#"
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
"#;

/// The two conversation reads that must never walk a whole conversation, held
/// here rather than inline so the test that checks their query plans checks
/// the statements that actually run.
///
/// `sequence DESC` with a `LIMIT` walks the primary key backward from the seek
/// point and stops, so the cost of a page is the page.
pub(super) const THREAD_PAGE_SQL: &str = "SELECT item FROM thread_items \
     WHERE agent_id = ?1 AND sequence < ?2 \
     ORDER BY sequence DESC LIMIT ?3";

/// The messages of a conversation, newest-first from the end — what a resumed
/// agent's catch-up packet is built from when the tail it booted onto holds
/// only activity.
///
/// `message = 1` is the message index's own predicate, so the planner's
/// implication check is trivial and the seek runs down the words themselves.
pub(super) const THREAD_MESSAGE_PAGE_SQL: &str = "SELECT item FROM thread_items \
     WHERE agent_id = ?1 AND message = 1 \
     ORDER BY sequence DESC LIMIT ?2";

/// How many tool calls a span of a conversation holds, counted off the hoisted
/// column: the census a page's activity digests are exact by.
///
/// Inclusive at both ends, because a digest's span is the run's own first and
/// last item — and exact over the whole run, including the items the page's
/// per-run cap left off the wire.
pub(super) const THREAD_TOOL_CALL_COUNT_SQL: &str = "SELECT COUNT(*) FROM thread_items \
     WHERE agent_id = ?1 AND tool_call = 1 AND sequence >= ?2 AND sequence <= ?3";

/// The other half of a run's census: every row of work in the span, the calls
/// among them. The same seek down the partial `activity` index the page's own
/// run walk uses.
pub(super) const THREAD_ACTIVITY_COUNT_SQL: &str = "SELECT COUNT(*) FROM thread_items \
     WHERE agent_id = ?1 AND activity = 1 AND sequence >= ?2 AND sequence <= ?3";

/// Where a page reaches back to: the sequence of the `limit`-th MESSAGE below
/// the seek, found by one seek down the partial index. Nothing found means the
/// conversation runs out above the page, and the floor is the bottom.
///
/// The page measure, and the reason the floor is always a message: whatever
/// the span above it holds, its oldest item is something somebody said.
pub(super) const THREAD_CONVERSATION_FLOOR_SQL: &str = "SELECT sequence FROM thread_items \
     WHERE agent_id = ?1 AND message = 1 AND sequence < ?2 \
     ORDER BY sequence DESC LIMIT 1 OFFSET ?3";

/// What a page is made of, newest-first: everything between the floor and the
/// seek that is not the agent working — the conversation, and the quiet
/// lifecycle markers beside it.
///
/// The runs of activity between these items are read one at a time, bounded,
/// through [`THREAD_ACTIVITY_RANGE_SQL`]. So a page's cost is its own size
/// rather than the size of the work it reaches over: a turn of five thousand
/// tool calls contributes the hundred rows the page ships, not five thousand
/// rows read, deserialized and dropped.
pub(super) const THREAD_CONVERSATION_STRUCTURE_SQL: &str =
    "SELECT sequence, item FROM thread_items \
     WHERE agent_id = ?1 AND activity = 0 AND sequence < ?2 AND sequence >= ?3 \
     ORDER BY sequence DESC";

/// The newest activity between two sequences, exclusive at both ends and
/// bounded by a limit — the run read, and the whole of what `thread.activity`
/// answers with.
///
/// Exclusive on both ends because the run read's bounds ARE the items that end
/// the run on either side, and a verb whose span is inclusive says so at its
/// own call site rather than here. One statement, two readers, so a page and a
/// client scrolling one open run read the conversation the same way.
pub(super) const THREAD_ACTIVITY_RANGE_SQL: &str = "SELECT sequence, item FROM thread_items \
     WHERE agent_id = ?1 AND activity = 1 AND sequence > ?2 AND sequence < ?3 \
     ORDER BY sequence DESC LIMIT ?4";

/// Where a run of activity starts — the row a folded run's digest names as its
/// `from_sequence`.
///
/// Read on its own because the page ships a run's NEWEST items: without it a
/// bounded read would name the oldest row it happened to fetch, and a client
/// would cache the run under a span it never had.
pub(super) const THREAD_RUN_OLDEST_SQL: &str = "SELECT sequence, item FROM thread_items \
     WHERE agent_id = ?1 AND activity = 1 AND sequence > ?2 AND sequence < ?3 \
     ORDER BY sequence ASC LIMIT 1";

/// The last call a run made — the row a folded run prints beside its count.
///
/// Read on its own for the same reason the oldest row is: a run that ended in
/// a hundred thoughts keeps its last call under everything the page ships, and
/// a digest is answered off the span it was handed. Memory sees the whole run
/// and names the call, so a stored page that did not go looking for it would
/// print a different row from the same conversation.
pub(super) const THREAD_RUN_LAST_CALL_SQL: &str = "SELECT sequence, item FROM thread_items \
     WHERE agent_id = ?1 AND tool_call = 1 AND sequence > ?2 AND sequence < ?3 \
     ORDER BY sequence DESC LIMIT 1";

/// How long a conversation is, asked of the primary key rather than of the
/// items: the load needs the total to say how much of a conversation it left
/// behind, and reading the items to count them would spend what paging saves.
pub(super) const THREAD_ITEM_COUNT_SQL: &str =
    "SELECT COUNT(*) FROM thread_items WHERE agent_id = ?1";

/// The forward cursor, which `thread_items_cursor` covers: the seek is the
/// filter, so a poll that finds nothing new reads nothing.
pub(super) const THREAD_CURSOR_SQL: &str = "SELECT item FROM thread_items \
     WHERE agent_id = ?1 AND updated_sequence > ?2 \
     ORDER BY updated_sequence";

/// The newest counter value anything in a conversation has reached, read off
/// the far end of `thread_items_cursor` rather than by looking at the items.
///
/// A load reads the tail, so it cannot see that an item under the tail was
/// mutated in place — a message marked seen, a comment resolved — before the
/// process before it stopped. This is how far a cursor has to have travelled
/// for the tail to be the whole answer to it.
pub(super) const THREAD_LAST_SEQUENCE_SQL: &str =
    "SELECT COALESCE(MAX(updated_sequence), 0) FROM thread_items WHERE agent_id = ?1";

/// Durable inbox summary inputs. Both are single indexed seeks; boot never
/// deserializes the activity that may separate the last message from the tail.
pub(super) const THREAD_LAST_MESSAGE_SQL: &str = "SELECT sequence, item FROM thread_items \
     WHERE agent_id = ?1 AND message = 1 ORDER BY sequence DESC LIMIT 1";

pub(super) const THREAD_FIRST_ATTENTION_AFTER_SQL: &str = "SELECT item FROM thread_items \
     WHERE agent_id = ?1 AND attention = 1 AND sequence > ?2 ORDER BY sequence LIMIT 1";

pub(super) const THREAD_LAST_ATTENTION_SQL: &str =
    "SELECT COALESCE(MAX(sequence), 0) FROM thread_items \
     WHERE agent_id = ?1 AND attention = 1";

/// The classification columns hoisted out of an item's JSON, each with the
/// schema version it arrived in: a stored database older than that version is
/// missing the column, and [`classify_stored_items`](Store::classify_stored_items)
/// fills it in.
///
/// One list rather than one migration step per column, because every one of
/// them is the same upgrade — an integer flag, defaulted to zero, backfilled
/// from the items already stored.
pub(super) const HOISTED_ITEM_COLUMNS: [(i64, &str); 4] = [
    (2, "attention"),
    (3, "message"),
    (4, "tool_call"),
    (5, "activity"),
];

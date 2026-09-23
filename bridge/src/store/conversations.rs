use super::schema::{
    THREAD_CONVERSATION_STRUCTURE_SQL, THREAD_RUN_LAST_CALL_SQL, THREAD_RUN_OLDEST_SQL,
};
use super::{
    Store, StoreError, THREAD_ACTIVITY_RANGE_SQL, THREAD_CONVERSATION_FLOOR_SQL, THREAD_CURSOR_SQL,
    THREAD_FORWARD_PAGE_SQL, THREAD_ITEM_COUNT_SQL, THREAD_MESSAGE_PAGE_SQL, THREAD_PAGE_SQL,
};
use crate::thread::cut_activity_runs;
use crate::thread::page_activity_budget;
use crate::thread::run_items_shipped;
use crate::thread::MessageRole;
use crate::thread::PageCut;
use crate::thread::ThreadItem;
use rusqlite::Connection;
use rusqlite::OptionalExtension;
use std::path::PathBuf;

/// How much of a conversation a load reads and the daemon then holds.
///
/// The tail, never the whole: a conversation costs this process a constant
/// rather than its length, and everything older is one page read away. Far
/// above any page a client asks for or any catch-up packet an agent is handed,
/// so the bound is only ever felt by history nobody has scrolled back to.
///
/// The tail is the conversation's working set, and what is loaded is what the
/// daemon reasons over: the mailbox an agent is sent to, the unread count a
/// row carries. Both are about what has just been said, and a message with
/// this many items of conversation after it has been gone past rather than
/// left waiting. The two readers that are asked ABOUT history instead —
/// searching a conversation, and replaying an entity's anchor — go to the
/// store for the whole of it (`Store::thread_items`) rather than answering
/// off the tail.
pub const RESIDENT_CONVERSATION_TAIL: usize = 200;

impl Store {
    /// One page of a conversation: the newest `limit` items strictly older
    /// than `before_sequence`, or the newest `limit` items when it is `None`.
    ///
    /// Handed back oldest-first even though SQL reads it newest-first, so the
    /// caller renders a page in the order it happened without reversing it
    /// again. This is what a first load asks for: a conversation of hundreds
    /// of items ships its tail, and the client walks backward from there.
    pub fn thread_page(
        &self,
        agent_id: &str,
        before_sequence: Option<u64>,
        limit: usize,
    ) -> Result<Vec<ThreadItem>, StoreError> {
        // No bound means "from the newest", which the same seek expresses as a
        // point past every sequence there could be.
        let before = before_sequence
            .and_then(|sequence| i64::try_from(sequence).ok())
            .unwrap_or(i64::MAX);
        let connection = self.connection();
        let mut statement = connection.prepare(THREAD_PAGE_SQL)?;
        read_thread_page(&mut statement, agent_id, before, limit)
    }
    /// One FORWARD page of a conversation: the oldest `limit` items strictly
    /// after `after_sequence`, in the order they happened.
    ///
    /// What a cache-first client's sync reads when its cursor predates the
    /// tail this process loaded. Unbounded by kind on purpose — a cache
    /// filling a gap has to hold every row in it, activity included, and the
    /// caller walks on from the last item it was handed.
    pub fn thread_page_after(
        &self,
        agent_id: &str,
        after_sequence: u64,
        limit: usize,
    ) -> Result<Vec<ThreadItem>, StoreError> {
        let after = i64::try_from(after_sequence).unwrap_or(i64::MAX);
        let connection = self.connection();
        let mut statement = connection.prepare(THREAD_FORWARD_PAGE_SQL)?;
        // Read in the conversation's own order, so nothing is reversed after.
        let page = decode_thread_items(
            agent_id,
            statement.query_map(rusqlite::params![agent_id, after, limit as i64], |row| {
                row.get::<_, String>(0)
            })?,
        );
        drop(statement);
        drop(connection);
        page
    }
    /// One page of a conversation, measured in messages: the items down to and
    /// including the `limit`-th message below `before_sequence`, cut the way
    /// every page is cut, with whether anything at all remains below what it
    /// shipped.
    ///
    /// The cut is [`cut_activity_runs`](crate::thread::cut_activity_runs), the
    /// same one the resident path uses — the only difference is the census,
    /// which here counts in SQL off the hoisted `tool_call` column instead of
    /// walking a tail. So a run of a thousand calls ships its newest hundred
    /// from either path, and says a thousand from either path.
    pub fn thread_conversation_page(
        &self,
        agent_id: &str,
        before_sequence: Option<u64>,
        limit: usize,
    ) -> Result<(PageCut<ThreadItem>, bool), StoreError> {
        let span = self.conversation_span(agent_id, before_sequence, limit)?;
        let cut = cut_activity_runs(span, limit, |from, through| {
            self.run_census(agent_id, from, through)
        })?;
        // Off the page's own oldest item, never off the floor: the cap may have
        // omitted the oldest of a run, and a page that claimed to reach the
        // floor it asked for would tell a client to skip what it never sent.
        let Some(shipped_floor) = cut.items.first().map(ThreadItem::sequence) else {
            return Ok((cut, false));
        };
        let has_more = self
            .connection()
            .query_row(
                "SELECT 1 FROM thread_items WHERE agent_id = ?1 AND sequence < ?2 LIMIT 1",
                rusqlite::params![agent_id, shipped_floor as i64],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        Ok((cut, has_more))
    }
    /// The span a page reaches over, newest-first: the conversation between the
    /// floor and the seek, with the newest of each run of activity between two
    /// of its items woven back in.
    ///
    /// The floor is a message, so the span's oldest item ends whatever run sits
    /// above it — which is what lets the cut count a run it can see the edges
    /// of.
    ///
    /// Nothing here reads a whole run. What comes back is what the page ships
    /// plus each run's edge rows, so the cut has the same items and the same
    /// digests to work from as a page cut out of memory, and the rows past the
    /// cap are never fetched at all.
    ///
    /// The connection is held for the reads and let go before the JSON is
    /// parsed. Deserializing a page of items is the expensive half of this
    /// call, and every other store call — an append, a poll, another
    /// conversation's page — waits behind the same mutex while it runs.
    pub(super) fn conversation_span(
        &self,
        agent_id: &str,
        before_sequence: Option<u64>,
        limit: usize,
    ) -> Result<Vec<ThreadItem>, StoreError> {
        let before = before_sequence
            .and_then(|sequence| i64::try_from(sequence).ok())
            .unwrap_or(i64::MAX);
        // A page of nothing is not a page; the callers clamp to at least one.
        let limit = limit.max(1);
        let connection = self.connection();
        let floor: i64 = connection
            .query_row(
                THREAD_CONVERSATION_FLOOR_SQL,
                rusqlite::params![agent_id, before, (limit - 1) as i64],
                |row| row.get(0),
            )
            .optional()?
            .unwrap_or(0);
        let raw_items = RunReader::new(&connection, agent_id, page_activity_budget(limit))?
            .span(before, floor)?;
        drop(connection);
        decode_thread_item_text(agent_id, raw_items)
    }
    /// The newest activity strictly between two sequences, oldest-first and
    /// bounded by `limit` — what an opened run renders, read out of the
    /// history no load holds.
    ///
    /// The exact sibling of `Thread::activity_between`, and the same statement
    /// a page's own run read steps: exclusive at both ends, so the bound
    /// arithmetic a verb with an inclusive span needs lives at that verb's call
    /// site rather than in two statements that would have to be kept equal.
    pub fn thread_activity_range(
        &self,
        agent_id: &str,
        after_sequence: u64,
        before_sequence: u64,
        limit: usize,
    ) -> Result<Vec<ThreadItem>, StoreError> {
        let connection = self.connection();
        let rows = read_sequenced_rows(
            &mut connection.prepare(THREAD_ACTIVITY_RANGE_SQL)?,
            rusqlite::params![
                agent_id,
                i64::try_from(after_sequence).unwrap_or(i64::MAX),
                i64::try_from(before_sequence).unwrap_or(i64::MAX),
                limit as i64
            ],
        )?;
        drop(connection);
        let mut page =
            decode_thread_item_text(agent_id, rows.into_iter().map(|(_, item)| item).collect())?;
        // Read newest-first off the seek, handed back in the order the run
        // happened in — the way every other page is.
        page.reverse();
        Ok(page)
    }
    /// The newest `limit` **messages** of a conversation, oldest-first — what
    /// a resumed agent's catch-up packet carries.
    ///
    /// Messages of either role, outcomes among them because an outcome is a
    /// message. The limit counts messages, so a session that emitted hundreds
    /// of tool calls before it stopped still hands its replacement what the
    /// human said: the seek runs down the conversation index and never reads
    /// the activity between the words at all.
    pub fn thread_message_page(
        &self,
        agent_id: &str,
        limit: usize,
    ) -> Result<Vec<ThreadItem>, StoreError> {
        let connection = self.connection();
        let mut statement = connection.prepare(THREAD_MESSAGE_PAGE_SQL)?;
        let mut page = decode_thread_items(
            agent_id,
            statement.query_map(rusqlite::params![agent_id, limit as i64], |row| {
                row.get::<_, String>(0)
            })?,
        )?;
        // Read newest-first off the seek, handed back in the order the
        // conversation happened — the way `read_thread_page` reverses.
        page.reverse();
        Ok(page)
    }
    pub fn thread_item_count(&self, agent_id: &str) -> Result<u64, StoreError> {
        let count: i64 = self
            .connection()
            .query_row(THREAD_ITEM_COUNT_SQL, [agent_id], |row| row.get(0))?;
        Ok(count as u64)
    }
    /// The forward cursor: everything that has happened on a conversation
    /// since `after_sequence`, oldest-first.
    ///
    /// It compares `updated_sequence`, not `sequence`, because an item mutated
    /// in place — a message marked seen, a comment resolved — is news to a
    /// client whose cursor is already past that item's creation. The
    /// `thread_items_cursor` index is on exactly that column, so the seek is
    /// the filter.
    pub fn thread_items_after(
        &self,
        agent_id: &str,
        after_sequence: u64,
    ) -> Result<Vec<ThreadItem>, StoreError> {
        let after = i64::try_from(after_sequence).unwrap_or(i64::MAX);
        let connection = self.connection();
        let mut statement = connection.prepare(THREAD_CURSOR_SQL)?;
        let mut delta = decode_thread_items(
            agent_id,
            statement.query_map(rusqlite::params![agent_id, after], |row| {
                row.get::<_, String>(0)
            })?,
        )?;
        // Index order is mutation order; the conversation's own order is
        // creation order, which is what a client merges its cache against.
        delta.sort_by_key(ThreadItem::sequence);
        Ok(delta)
    }
    /// The whole of one conversation, history included — the deliberate
    /// exception to paging.
    ///
    /// For the two readers that are answering ABOUT history rather than
    /// rendering it: a search of the conversation, and the anchor the boot
    /// migration replays out of everything the user ever said. Both are wrong
    /// if they only see the tail, and neither runs on a poll.
    pub fn thread_items(&self, agent_id: &str) -> Result<Vec<ThreadItem>, StoreError> {
        let connection = self.connection();
        let mut statement = connection
            .prepare("SELECT item FROM thread_items WHERE agent_id = ?1 ORDER BY sequence")?;
        let rows = statement.query_map([agent_id], |row| row.get::<_, String>(0))?;
        decode_thread_items(agent_id, rows)
    }
}

pub(super) fn read_thread_page(
    statement: &mut rusqlite::Statement<'_>,
    agent_id: &str,
    before: i64,
    limit: usize,
) -> Result<Vec<ThreadItem>, StoreError> {
    let mut page = decode_thread_items(
        agent_id,
        statement.query_map(rusqlite::params![agent_id, before, limit as i64], |row| {
            row.get::<_, String>(0)
        })?,
    )?;
    page.reverse();
    Ok(page)
}

/// One page's span: the conversation between its bounds, and the runs of
/// activity between those items read one at a time and never whole.
///
/// Holds what the page may still ship, because that is what says how much of
/// the next run to read: a page that has spent its budget reads a run's edges
/// and nothing else. Statements are prepared once and stepped per run, so a
/// page over a conversation of many turns still prepares four.
pub(super) struct RunReader<'a> {
    agent_id: &'a str,
    structure: rusqlite::Statement<'a>,
    newest: rusqlite::Statement<'a>,
    oldest: rusqlite::Statement<'a>,
    last_call: rusqlite::Statement<'a>,
    activity_left: usize,
}

impl<'a> RunReader<'a> {
    fn new(
        connection: &'a Connection,
        agent_id: &'a str,
        activity_left: usize,
    ) -> Result<Self, StoreError> {
        Ok(RunReader {
            agent_id,
            structure: connection.prepare(THREAD_CONVERSATION_STRUCTURE_SQL)?,
            newest: connection.prepare(THREAD_ACTIVITY_RANGE_SQL)?,
            oldest: connection.prepare(THREAD_RUN_OLDEST_SQL)?,
            last_call: connection.prepare(THREAD_RUN_LAST_CALL_SQL)?,
            activity_left,
        })
    }

    /// The span, newest-first: the page's own items between `floor` and
    /// `before`, each with the run that sits above it read in first.
    ///
    /// `before` bounds the newest run and `floor` the oldest. Both are
    /// exclusive of the item that ends the run, which is why the floor is
    /// stepped down: the floor row itself is a message, and a page that reaches
    /// the start of a conversation has no message under its oldest run at all.
    fn span(mut self, before: i64, floor: i64) -> Result<Vec<String>, StoreError> {
        let agent_id = self.agent_id;
        let structure = read_sequenced_rows(
            &mut self.structure,
            rusqlite::params![agent_id, before, floor],
        )?;
        let mut span = Vec::with_capacity(structure.len());
        let mut above = before;
        for (sequence, item) in structure {
            span.extend(self.run_between(sequence, above)?);
            span.push(item);
            above = sequence;
        }
        span.extend(self.run_between(floor - 1, above)?);
        Ok(span)
    }

    /// One run, newest-first: as much of its newest as the page can still
    /// ship, plus the two rows its digest is written off — where the run
    /// starts, and the last call it made.
    ///
    /// Those two are what make the digest the run's own rather than the read's,
    /// and the cut drops them again under the cap — so a run reads at most two
    /// rows more than it ships, whatever else happened inside it.
    fn run_between(&mut self, after: i64, below: i64) -> Result<Vec<String>, StoreError> {
        if after + 1 >= below {
            return Ok(Vec::new());
        }
        let agent_id = self.agent_id;
        let wanted = self.newest_rows_wanted() as i64;
        let newest = read_sequenced_rows(
            &mut self.newest,
            rusqlite::params![agent_id, after, below, wanted],
        )?;
        self.activity_left -= run_items_shipped(newest.len(), self.activity_left);
        let mut run: std::collections::BTreeMap<i64, String> = newest.into_iter().collect();
        for edge in [&mut self.oldest, &mut self.last_call] {
            for (sequence, item) in
                read_sequenced_rows(edge, rusqlite::params![agent_id, after, below])?
            {
                run.entry(sequence).or_insert(item);
            }
        }
        Ok(run.into_values().rev().collect())
    }

    /// How many of a run's newest rows to read: what the page could still ship
    /// of it, and never fewer than one.
    ///
    /// The newest row is the digest's `through_sequence`, which a run prints
    /// whether or not the page has anything left to spend on it.
    fn newest_rows_wanted(&self) -> usize {
        run_items_shipped(usize::MAX, self.activity_left).max(1)
    }
}

/// The `(sequence, item)` rows of a prepared conversation read, in the order
/// the statement returns them — the shape every read of the span speaks in,
/// so a run's rows can be merged with its edges by sequence.
pub(super) fn read_sequenced_rows(
    statement: &mut rusqlite::Statement<'_>,
    params: impl rusqlite::Params,
) -> Result<Vec<(i64, String)>, StoreError> {
    let rows = statement.query_map(params, |row| Ok((row.get(0)?, row.get(1)?)))?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

/// Turn stored item rows into conversation items. Reads the rows off the
/// statement — so the caller is still holding the connection — and decodes
/// them in the same breath.
pub(super) fn decode_thread_items(
    agent_id: &str,
    rows: rusqlite::MappedRows<'_, impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<String>>,
) -> Result<Vec<ThreadItem>, StoreError> {
    decode_thread_item_text(agent_id, rows.collect::<Result<Vec<String>, _>>()?)
}

#[derive(Default)]
pub(super) struct StoredConversationSummary {
    pub(super) last_message_sequence: u64,
    pub(super) activity_at: Option<String>,
    pub(super) working: bool,
}

#[cfg(test)]
thread_local! {
    /// Stored rows this OS thread has put through serde on their way out of
    /// the database, since the process started.
    ///
    /// A page built by reading a whole run and dropping most of it is
    /// byte-identical to one built by reading what it ships, so nothing about
    /// the answer can tell the two apart — only the count can. Per-OS-thread
    /// rather than global so tests running side by side do not read each
    /// other's reads.
    static ITEMS_DECODED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

pub(super) fn stored_conversation_summary(
    last_message: &mut rusqlite::Statement<'_>,
    first_attention_after: &mut rusqlite::Statement<'_>,
    agent_id: &str,
) -> Result<StoredConversationSummary, StoreError> {
    let row: Option<(i64, String)> = last_message
        .query_row([agent_id], |row| Ok((row.get(0)?, row.get(1)?)))
        .optional()?;
    let Some((sequence, raw)) = row else {
        return Ok(StoredConversationSummary::default());
    };
    let item: ThreadItem = serde_json::from_str(&raw).map_err(|source| StoreError::Corrupt {
        path: PathBuf::from(format!("thread_items/{agent_id}/{sequence}")),
        source,
    })?;
    let ThreadItem::Message(message) = item else {
        unreachable!("message classification must identify a message")
    };
    let mut summary = StoredConversationSummary {
        last_message_sequence: sequence as u64,
        activity_at: Some(message.created_at),
        working: match message.role {
            MessageRole::User => message.seen_at.is_some(),
            MessageRole::Agent => message.still_working,
        },
    };
    if summary.working {
        let stopping: Option<String> = first_attention_after
            .query_row(rusqlite::params![agent_id, sequence], |row| row.get(0))
            .optional()?;
        if let Some(raw) = stopping {
            let item: ThreadItem =
                serde_json::from_str(&raw).map_err(|source| StoreError::Corrupt {
                    path: PathBuf::from(format!("thread_items/{agent_id}/after-{sequence}")),
                    source,
                })?;
            summary.activity_at = Some(item.created_at().to_string());
            summary.working = false;
        }
    }
    Ok(summary)
}

type SessionMessageTime = (Option<String>, Option<String>, String, u64, i64);

impl Store {
    /// Every user/agent message timestamp and its stored owner. The partial
    /// message index excludes tool/event rows before JSON is touched. Sorting
    /// parsed milliseconds in Rust handles valid RFC 3339 offsets correctly.
    pub fn session_message_times(&self) -> Result<Vec<SessionMessageTime>, StoreError> {
        let connection = self.connection();
        let mut statement = connection.prepare(
            "SELECT a.owner_id, t.agent_id, t.sequence, json_extract(t.item, '$.data.created_at') \
             FROM thread_items t JOIN agents a ON a.id = t.agent_id \
             WHERE t.message = 1 AND COALESCE(json_extract(t.item, '$.data.from_build'), 0) = 0",
        )?;
        let mut rows = statement.query([])?;
        let mut messages = Vec::new();
        while let Some(row) = rows.next()? {
            let at: Option<String> = row.get(3)?;
            if let Some(ts) = at
                .as_deref()
                .and_then(crate::session_summary::message_millis)
            {
                let sequence: i64 = row.get(2)?;
                messages.push((Some(row.get(0)?), None, row.get(1)?, sequence as u64, ts));
            }
        }
        drop(rows);
        drop(statement);
        let mut retained_runs =
            connection.prepare("SELECT run_id, ts_ms FROM inbox_retained_run_messages")?;
        let run_rows = retained_runs.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?;
        for row in run_rows {
            let (run_id, ts) = row?;
            messages.push((Some(run_id), None, String::new(), 0, ts));
        }
        drop(retained_runs);
        let mut retained =
            connection.prepare("SELECT project_id, ts_ms FROM inbox_retained_messages")?;
        let retained_rows = retained.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?;
        for row in retained_rows {
            let (project_id, ts) = row?;
            messages.push((None, Some(project_id), String::new(), 0, ts));
        }
        messages.sort_by_key(|row| row.4);
        Ok(messages)
    }
}

/// Turn stored item TEXT into conversation items, naming the conversation in
/// the error so a corrupt row says which agent's history stopped parsing.
///
/// Takes the text rather than the rows so a caller that has finished with the
/// database can let the connection go before it parses.
pub(super) fn decode_thread_item_text(
    agent_id: &str,
    raw_items: Vec<String>,
) -> Result<Vec<ThreadItem>, StoreError> {
    count_decoded_items(raw_items.len());
    raw_items
        .into_iter()
        .map(|raw| serde_json::from_str(&raw))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|source| StoreError::Corrupt {
            path: PathBuf::from(format!("thread_items/{agent_id}")),
            source,
        })
}

/// How many stored rows this OS thread has deserialized — read before and
/// after a call to measure what it cost.
#[cfg(test)]
pub fn items_decoded() -> usize {
    ITEMS_DECODED.with(std::cell::Cell::get)
}

#[cfg(test)]
pub(super) fn count_decoded_items(count: usize) {
    ITEMS_DECODED.with(|counter| counter.set(counter.get() + count));
}

/// Nothing is counted outside the tests: only they ask what a read cost.
#[cfg(not(test))]
pub(super) fn count_decoded_items(_count: usize) {}

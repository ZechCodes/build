use super::AppState;
use crate::app::{require_str, WORKING_INDICATOR_NOTICE};
use crate::operation::THREAD_POST_METHOD;
use crate::store::{now_rfc3339, Store};
use crate::thread::ThreadDetail;
use serde_json::{json, Value};

/// What a panel claims to have read of one conversation.
///
/// `window_floor` is where the window it holds starts: a client paging a long
/// conversation reaches the end of a WINDOW rather than the end of the thread,
/// so a report from one carries no claim about the items below the floor.
/// `through` is the newest message its viewport actually reached — reading is
/// per message, so a reader half way down what arrived clears half of it and
/// the rest goes on waiting. No `through` means the panel read to the end of
/// what it holds.
#[derive(Clone, Copy, Default, Debug)]
pub(in crate::app) struct ReadReport {
    pub(in crate::app) window_floor: Option<u64>,
    pub(in crate::app) through: Option<u64>,
}

impl ReadReport {
    /// How far this report reads a conversation whose newest item is
    /// `last_sequence`. Never past the end: a stale `through` from a panel
    /// holding a longer thread than this one claims nothing extra.
    pub(in crate::app) fn reached(self, last_sequence: u64) -> u64 {
        self.through.unwrap_or(last_sequence).min(last_sequence)
    }
}

/// The detail polls' optional `thread_after_sequence` cursor. A missing or
/// garbage (non-integer, negative) value reads as absent — the poll then gets
/// the conversation's newest page instead of an error.
pub(in crate::app) fn thread_cursor(params: &Value) -> Option<u64> {
    params.get("thread_after_sequence").and_then(Value::as_u64)
}

/// How much conversation a call can hold — a detail poll's answer or a
/// mutation's: a page of the size its `thread_limit` names, or the
/// conversation whole when it names none.
///
/// Silence has to keep meaning "whole". A client written before paging holds
/// every item and reconciles each later delta against `thread_total`; hand it
/// a window unasked and that count never matches again, so it drops its cache
/// every tick, never sends a cursor, and never sees a word above the window.
/// A client that can page says so, and says how much.
///
/// Which is why only silence may mean whole. A limit nobody can read is still
/// a client saying it can page, and answering it with the conversation entire
/// hands it the one thing paging exists to prevent — with no `has_more` to
/// tell it the answer was not the page it asked for. So an unreadable limit
/// falls back to the default page, not to the unbounded answer. (The garbage
/// cursor above can afford to read as absent: what it falls back to is
/// bounded.)
pub(in crate::app) fn thread_detail(params: &Value) -> ThreadDetail {
    match params.get("thread_limit") {
        // `null` is how a client spells a field it is not sending.
        None | Some(Value::Null) => ThreadDetail::Full,
        Some(named) => ThreadDetail::Page(
            thread_limit_size(named)
                .unwrap_or(crate::thread::DEFAULT_THREAD_PAGE)
                .clamp(1, crate::thread::MAX_THREAD_PAGE),
        ),
    }
}

/// The page size a `thread_limit` names, however its client spelled the
/// number: a JSON integer, the whole float a language without an integer type
/// encodes one as, or the string a URL or a number-stringifying encoder leaves
/// behind. `None` for anything that names no size at all — a negative count, a
/// word, a structure.
pub(in crate::app) fn thread_limit_size(named: &Value) -> Option<usize> {
    let size = match named {
        Value::Number(number) => number.as_u64().or_else(|| {
            let whole = number.as_f64().filter(|float| float.fract() == 0.0)?;
            (whole >= 0.0).then_some(whole as u64)
        })?,
        Value::String(text) => text.trim().parse().ok()?,
        _ => return None,
    };
    usize::try_from(size).ok()
}

/// How much conversation the VIEW under a detail poll builds, given what the
/// poll already has to put in its place.
///
/// A poll that named an agent or carried a cursor gets its `thread` from
/// [`AppState::detail_thread_value`], and that answer overwrites the view's
/// own — so building the view's whole meant every item of the conversation
/// through serde on every steady-state poll of every open browser, thrown away
/// unread. The wire was bounded; the daemon was not. The digest holds the key
/// until the replacement lands and costs nothing per item.
///
/// Taking the replacement itself, rather than re-reading the params that imply
/// one, is what keeps the two from drifting: the cheap thread is built exactly
/// when there is something to overwrite it with.
pub(in crate::app) fn view_thread_detail(
    replacement: &Option<Value>,
    params: &Value,
) -> ThreadDetail {
    match replacement {
        Some(_) => ThreadDetail::Digest,
        None => thread_detail(params),
    }
}

/// How much a paged verb asked for. Absent means the default; a limit larger
/// than one page could sanely carry is clamped rather than refused, since the
/// caller still wants an answer back — and shipping the whole of a long
/// conversation is the thing paging exists to prevent.
pub(in crate::app) fn page_limit_param(params: &Value, default: usize, most: usize) -> usize {
    params
        .get("limit")
        .and_then(thread_limit_size)
        .map(|limit| limit.clamp(1, most))
        .unwrap_or(default)
}

/// How much of a conversation a `thread.page` call asked for.
pub(in crate::app) fn thread_page_limit(params: &Value) -> usize {
    page_limit_param(
        params,
        crate::thread::DEFAULT_THREAD_PAGE,
        crate::thread::MAX_THREAD_PAGE,
    )
}

/// How many ITEMS one forward page carries.
///
/// The backward page's limit buys messages, because a reviewer scrolling up
/// is after what was said. The forward page's buys items, because the caller
/// is a cache filling a gap and every row in that gap is one it has to hold.
/// So it is both the default and the ceiling: a client may ask for less, and
/// asks again from the last item it was handed.
pub const LATEST_THREAD_ITEMS: usize = 100;

/// How much of a conversation a forward `thread.page` call asked for.
pub(in crate::app) fn thread_page_forward_limit(params: &Value) -> usize {
    page_limit_param(params, LATEST_THREAD_ITEMS, LATEST_THREAD_ITEMS)
}

/// How much of one run's activity a `thread.activity` call asked for.
pub(in crate::app) fn activity_page_limit(params: &Value) -> usize {
    page_limit_param(
        params,
        crate::thread::DEFAULT_ACTIVITY_PAGE,
        crate::thread::MAX_ACTIVITY_PAGE,
    )
}

/// The span a `thread.activity` call names, read against the conversation it
/// names it in.
///
/// A span that runs backwards, or that reaches past anything this conversation
/// has ever said, is a client asking about work that never happened — an error
/// rather than an empty page, because an empty page is an answer a client
/// caches.
pub(in crate::app) fn activity_span_params(
    params: &Value,
    thread: &crate::thread::Thread,
) -> Result<(u64, u64), String> {
    let from = required_sequence_param(params, "from_sequence")?;
    let through = required_sequence_param(params, "through_sequence")?;
    if from > through {
        return Err(format!(
            "from_sequence {from} is above through_sequence {through}"
        ));
    }
    let last = thread.last_sequence();
    if through > last {
        return Err(format!(
            "this conversation reaches sequence {last}, not {through}"
        ));
    }
    Ok((from, through))
}

/// A sequence bound a verb cannot do without, named in the refusal so a client
/// that sent the wrong shape is told which one.
pub(in crate::app) fn required_sequence_param(params: &Value, key: &str) -> Result<u64, String> {
    params
        .get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| format!("missing required param: {key}"))
}

/// The conversation owner a thread verb names. `entity_id` is the canonical
/// name, and a detail surface's own id is accepted as well, so a client paging
/// the view it is looking at does not have to rename the id it already holds.
pub(in crate::app) fn conversation_owner_param(params: &Value) -> Result<String, String> {
    ["entity_id", "run_id", "plan_id", "issue_id"]
        .iter()
        .find_map(|key| {
            params
                .get(key)
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
        })
        .map(str::to_string)
        .ok_or_else(|| "missing required param: entity_id".to_string())
}

impl AppState {
    pub(in crate::app) fn read_operation_messages_for_agent(
        &mut self,
        entity_id: &str,
        agent_id: &str,
        operation_id: &str,
    ) -> Result<Value, String> {
        let receipt = self
            .operation_receipt(operation_id)?
            .ok_or_else(|| format!("unknown operation_id: {operation_id}"))?;
        let delivery = receipt
            .delivery
            .as_ref()
            .ok_or_else(|| "read_unread_messages: operation has no delivery intent".to_string())?;
        let address = self.resolve_conversation_address(entity_id, Some(agent_id))?;
        if receipt.method != THREAD_POST_METHOD
            || delivery.owner_id != entity_id
            || delivery.agent_id != agent_id
            || receipt.conversation_id != address.conversation_id
        {
            return Err("read_unread_messages: operation does not belong to this agent".into());
        }
        let payload = delivery
            .payload
            .as_ref()
            .ok_or_else(|| "read_unread_messages: operation has no bounded payload".to_string())?;
        let messages = payload.messages.clone();
        let start = payload.start_sequence;
        let end = payload.end_sequence;
        let now = now_rfc3339();
        let acknowledged_sequence = if let Some(store) = self.store.as_ref() {
            Some(
                store
                    .acknowledge_operation_messages(
                        &receipt.conversation_id,
                        operation_id,
                        start,
                        end,
                        &now,
                    )
                    .map_err(|error| format!("operation message store: {error}"))?,
            )
        } else {
            None
        };
        let result = self.edit_agent_conversation(entity_id, agent_id, |thread, _| {
            thread.read_operation_messages(operation_id, start, end, &now);
            if let Some(sequence) = acknowledged_sequence {
                thread.advance_sequence_to(sequence);
            }
            thread.note_operation_read(&now);
            Ok(json!({
                "thread_id": thread.id,
                "agent_id": thread.agent.id,
                "messages": messages,
                "working": WORKING_INDICATOR_NOTICE,
            }))
        });
        if let Ok(value) = &result {
            if value["messages"]
                .as_array()
                .is_some_and(|messages| !messages.is_empty())
            {
                self.start_agent_working(entity_id, agent_id, &now);
            }
            self.observe_conversation_working(entity_id, &now);
        }
        result
    }

    /// Answer a history query out of the conversations this agent may read.
    ///
    /// That is its own, and — when it is implementing an Issue — the Issue's,
    /// which is where its first agent's words are actually recorded. Never
    /// another entity's: an agent asking about work it was never given must
    /// come back empty, not informed.
    /// The whole of a conversation, for a reader answering ABOUT its history
    /// rather than rendering it.
    ///
    /// Borrows what is already held whenever that is the whole conversation —
    /// which it is for every conversation this process wrote itself — and goes
    /// The conversation store, or the one refusal every history read gives
    /// when this bridge keeps no store: a bounded load left part of the
    /// conversation on disk, and without a store that part cannot be read.
    pub(in crate::app) fn history_store(&self) -> Result<&Store, String> {
        self.store
            .as_ref()
            .ok_or_else(|| "this conversation's history is not stored".to_string())
    }

    /// to the store only for the history a bounded load left there.
    pub(in crate::app) fn whole_conversation<'a>(
        &self,
        thread: &'a crate::thread::Thread,
    ) -> Result<std::borrow::Cow<'a, [crate::thread::ThreadItem]>, String> {
        if thread.total_item_count() == thread.items.len() as u64 {
            return Ok(std::borrow::Cow::Borrowed(&thread.items));
        }
        let store = self.history_store()?;
        store
            .thread_items(&thread.agent.id)
            .map(std::borrow::Cow::Owned)
            .map_err(|error| format!("conversation store: {error}"))
    }

    pub(in crate::app) fn search_agent_conversations(
        &self,
        entity_id: &str,
        agent_id: &str,
        query: &crate::thread::ConversationQuery,
    ) -> Result<Value, String> {
        let thread = self.agent_conversation(entity_id, Some(agent_id))?;

        let mut hits: Vec<crate::thread::ConversationHit> = Vec::new();
        // A search is asked about the whole conversation — what was decided
        // about a thing, however long ago — so it looks past the resident tail.
        let items = self.whole_conversation(thread)?;
        hits.extend(thread.search_items(&items, query));
        hits.truncate(query.effective_limit());
        Ok(json!({
            "hits": hits,
            "threads_searched": [thread.id.clone()],
        }))
    }

    pub(crate) fn thread_revision(&self, params: &Value) -> Result<Value, String> {
        let entity_id = require_str(params, "entity_id")?;
        let revision_id = require_str(params, "revision_id")?;
        let address = self.resolve_conversation_params(&entity_id, params)?;
        let thread = self.conversation_at(&address)?;
        let revision = thread
            .revisions
            .iter()
            .find(|revision| revision.id == revision_id)
            .ok_or("unknown revision_id")?;
        let contents = revision
            .snapshot
            .as_ref()
            .ok_or("revision snapshot is no longer retained")?;
        Ok(json!({
            "revision_id": revision.id,
            "artifact": revision.artifact,
            "created_at": revision.created_at,
            "contents": contents,
        }))
    }

    /// One page up a conversation: the items immediately older than
    /// `before_sequence`, newest page when it says nothing. This is the other
    /// half of the bounded first load — the reviewer scrolling back through
    /// work the detail poll deliberately left off the wire.
    ///
    /// Ships exactly what a first page ships, so a client merges a page up the
    /// way it merges the page it opened on.
    pub(crate) fn thread_page(&self, params: &Value) -> Result<Value, String> {
        let entity_id = conversation_owner_param(params)?;
        let address = self.resolve_conversation_params(&entity_id, params)?;
        let thread = self.conversation_at(&address)?;
        let before = params.get("before_sequence").and_then(Value::as_u64);
        let after = params.get("after_sequence").and_then(Value::as_u64);
        let newest = params
            .get("newest")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        match (before, after, newest) {
            // Two cursors name two walks, and a verb that picked one would
            // answer a page nobody asked for.
            (Some(_), Some(_), _) => {
                Err("provide exactly one of before_sequence and after_sequence".to_string())
            }
            (_, Some(after), true) => {
                Ok(thread.wire_value_newest_page_after(after, thread_page_forward_limit(params)))
            }
            (_, Some(after), false) => {
                self.thread_page_after(thread, after, thread_page_forward_limit(params))
            }
            (_, None, true) => Err("newest must be used with after_sequence".to_string()),
            (_, None, false) => self.thread_page_at(thread, before, thread_page_limit(params)),
        }
    }

    /// The activity of one folded run, wherever it lives — what a client asks
    /// for when a reviewer opens a run a page shipped only the digest of.
    ///
    /// A run older than the newest message never changes: the agent that made
    /// it has moved on, and nothing appends inside a closed run. So a client
    /// may cache what this answers under the run's `from_sequence` until the
    /// entity is evicted. Only the tail run is live, and that one is not
    /// fetched at all — it arrives as the forward deltas a poll already
    /// carries.
    pub(crate) fn thread_activity(&self, params: &Value) -> Result<Value, String> {
        let entity_id = conversation_owner_param(params)?;
        let address = self.resolve_conversation_params(&entity_id, params)?;
        let thread = self.conversation_at(&address)?;
        let (from, through) = activity_span_params(params, thread)?;
        self.activity_span(
            thread,
            from,
            through,
            params.get("before_sequence").and_then(Value::as_u64),
            activity_page_limit(params),
        )
    }

    /// One page of a span of activity, out of memory or out of the store — the
    /// one place that choice is made, and the one place the bounds a span is
    /// asked for become the bounds a read takes.
    ///
    /// The store's range is exclusive at both ends because a page's own run
    /// read is; a span named by a digest is inclusive, and `before_sequence` is
    /// a third bound on the same end as `through`. So both arrive here as
    /// named numbers and leave as the two the statement takes.
    pub(in crate::app) fn activity_span(
        &self,
        thread: &crate::thread::Thread,
        from_sequence: u64,
        through_sequence: u64,
        before_sequence: Option<u64>,
        limit: usize,
    ) -> Result<Value, String> {
        let rows = crate::thread::activity_rows_read(limit);
        if thread.resident_from_sequence() <= from_sequence {
            let span =
                thread.activity_between(from_sequence, through_sequence, before_sequence, rows);
            return Ok(crate::thread::wire_value_activity_page(&span, limit));
        }
        let store = self.history_store()?;
        let below = before_sequence
            .unwrap_or(u64::MAX)
            .min(through_sequence.saturating_add(1));
        let span = store
            .thread_activity_range(
                &thread.agent.id,
                from_sequence.saturating_sub(1),
                below,
                rows,
            )
            .map_err(|error| format!("conversation store: {error}"))?;
        Ok(crate::thread::wire_value_activity_page(&span, limit))
    }

    /// One page of a conversation, wherever the page lives.
    ///
    /// A conversation is loaded as its tail, so a page far enough up one — or
    /// a first page of a session that emitted hundreds of tool calls between
    /// two words — leaves memory. Where it does, the page comes back out of
    /// the store: the same page, in the same shape, off the same seek, cut by
    /// the same rule. The one gate every page passes, so the page a reviewer
    /// opens on and the pages a scroll asks for above it always abut.
    pub(in crate::app) fn thread_page_at(
        &self,
        thread: &crate::thread::Thread,
        before_sequence: Option<u64>,
        limit: usize,
    ) -> Result<Value, String> {
        if thread.page_reaches_stored_history(before_sequence, limit) {
            return self.stored_thread_page(thread, before_sequence, limit);
        }
        Ok(thread.wire_value_page(before_sequence, limit))
    }

    /// The page a detail poll opens a conversation on.
    ///
    /// A detail view is the whole entity, and it renders whether or not the
    /// store answers: a conversation short of its oldest turn is worth more to
    /// a reviewer than an error where the view was, so a store that cannot be
    /// read is reported and the tail memory holds is shipped instead.
    ///
    /// The fallback page carries NO activity digests. The gate only sends a
    /// page to the store when the tail is too short to hold it, so the tail's
    /// oldest item can sit in the middle of a run whose older half is stored —
    /// and a census counted off memory would then name an exact number that is
    /// short. A digest's count is read as fact, so a degraded page ships none
    /// and the client falls back to counting the rows it was handed.
    pub(in crate::app) fn first_thread_page(
        &self,
        thread: &crate::thread::Thread,
        limit: usize,
    ) -> Value {
        self.thread_page_at(thread, None, limit)
            .unwrap_or_else(|error| {
                eprintln!(
                    "first_thread_page {}: {error}; shipping the resident tail",
                    thread.agent.id
                );
                let mut page = thread.wire_value_page(None, limit);
                page["activity_digests"] = json!([]);
                page
            })
    }

    /// One page of the history no load read, straight off the store.
    ///
    /// Cut exactly as a resident page is — the limit buys messages, the
    /// activity between them rides along folded under its per-run cap, and the
    /// digests beside it count what the cap left off. The store answers
    /// `has_more` for items of any kind below what it shipped, so a client's
    /// backward walk still abuts.
    pub(in crate::app) fn stored_thread_page(
        &self,
        thread: &crate::thread::Thread,
        before_sequence: Option<u64>,
        limit: usize,
    ) -> Result<Value, String> {
        let store = self.history_store()?;
        let (cut, has_more) = store
            .thread_conversation_page(&thread.agent.id, before_sequence, limit)
            .map_err(|error| format!("conversation store: {error}"))?;
        Ok(thread.wire_value_of_page(&cut, has_more))
    }

    /// One forward page of a conversation, wherever the items live.
    ///
    /// The sibling of [`thread_page_at`](Self::thread_page_at), and the same
    /// gate: a cursor the resident tail reaches is answered out of memory,
    /// and one below it is completed out of the store. The client walks on
    /// from the last item it was handed, so the two never have to abut across
    /// a page boundary the way the backward walk's do — the items themselves
    /// say where the next page starts.
    pub(in crate::app) fn thread_page_after(
        &self,
        thread: &crate::thread::Thread,
        after_sequence: u64,
        limit: usize,
    ) -> Result<Value, String> {
        if !thread.forward_page_reaches_stored_history(after_sequence) {
            return Ok(thread.wire_value_page_after(after_sequence, limit));
        }
        let store = self.history_store()?;
        // One row past the page, so `has_more` is a row rather than a second
        // query — the same trick the activity span reads by.
        let history = store
            .thread_page_after(&thread.agent.id, after_sequence, limit + 1)
            .map_err(|error| format!("conversation store: {error}"))?;
        Ok(thread.wire_value_page_after_including_history(after_sequence, limit, &history))
    }

    /// A cursor's delta completed out of the store: the forward seek answers
    /// for the history the load left behind, and the conversation merges it
    /// with the tail it holds.
    ///
    /// Only a cursor older than the mark the load read gets here, and the
    /// answer carries a `thread_last_sequence` past that mark — so a client
    /// pays this read once after a restart and its steady-state polls go on
    /// being answered out of memory.
    pub(in crate::app) fn stored_thread_delta(
        &self,
        thread: &crate::thread::Thread,
        after_sequence: u64,
    ) -> Result<Value, String> {
        let store = self.history_store()?;
        let history = store
            .thread_items_after(&thread.agent.id, after_sequence)
            .map_err(|error| format!("conversation store: {error}"))?;
        Ok(thread.wire_value_after_including_history(after_sequence, &history))
    }
}

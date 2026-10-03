use super::{
    count_serialized_items, page_span, EventClass, MessageRole, Thread, ThreadItem, ToolCallOutcome,
};
use serde::Serialize;
use serde_json::json;
use serde_json::Value;
use std::borrow::Borrow;

/// How many MESSAGES a page carries when the caller asks for one without
/// saying how large.
///
/// The limit buys what was said, so twenty is twenty turns of conversation —
/// the sitting a reviewer opens onto — however much work happened between
/// them. Everything older is a page up, which is the point: a conversation of
/// hundreds no longer ships whole to show its last hour.
pub const DEFAULT_THREAD_PAGE: usize = 20;

/// The most conversation one page ships, however large a limit it asks for.
///
/// A scroll-back that asks for the whole conversation at once is the thing
/// paging exists to prevent, so the cap holds even when the caller means well.
pub const MAX_THREAD_PAGE: usize = 200;

/// How many items of any ONE run of activity a page ships.
///
/// A run is folded to a single row, so what the wire has to carry is the newest
/// of it — the last thing the agent did, and enough above it to read as work.
/// The digest beside the page carries the truth about the rest: how many calls
/// the run made, and which one was last. Items past the cap are omitted, which
/// is why a page is not a contiguous run of sequences.
pub const PAGE_ACTIVITY_RUN_CAP: usize = 100;

/// How many activity items a page may ship per message of its limit: a page of
/// 1 ships at most 10 activity items, so the smallest polls — the branch
/// surface and console poll `thread_limit` 1 every tick and read none of it —
/// stay small; the default 20 ships at most 200.
pub const PAGE_ACTIVITY_PER_MESSAGE: usize = 10;

/// The whole-page activity budget a limit buys.
///
/// The per-run cap bounds one run; this bounds the page. Twenty runs each
/// capped at a hundred is still two thousand rows for twenty messages, which
/// is the shape the cap alone leaves behind — so the budget is spent
/// newest-first and the runs a reviewer has not scrolled to fold down to their
/// digests.
pub fn page_activity_budget(limit: usize) -> usize {
    limit * PAGE_ACTIVITY_PER_MESSAGE
}

/// How many items of one run a page ships: the shortest of the run itself, the
/// per-run [`PAGE_ACTIVITY_RUN_CAP`] and what is left of the page's
/// [`page_activity_budget`].
///
/// One rule in one place, because two readers spend the budget. The cut spends
/// it over a span it already holds; the store spends it deciding how much of a
/// run to read at all, and a run it reads more of than it ships is exactly the
/// cost the bounded read exists to remove.
pub fn run_items_shipped(run_len: usize, activity_left: usize) -> usize {
    run_len.min(PAGE_ACTIVITY_RUN_CAP).min(activity_left)
}

/// How much of one run's activity a `thread.activity` call ships when it does
/// not say: a whole fold's worth, so opening a run of ordinary size is one
/// call.
pub const DEFAULT_ACTIVITY_PAGE: usize = 200;

/// The most one `thread.activity` call ships, however large a limit it asks
/// for. A client that wants the whole of a five-thousand-call run walks it.
pub const MAX_ACTIVITY_PAGE: usize = 500;

/// How many rows a page of activity READS: one more than it ships.
///
/// That row is the answer to `has_more` — it exists or it does not — which is
/// a row rather than a second query, and it is the same row whether the span
/// came out of memory or out of SQLite.
pub fn activity_rows_read(limit: usize) -> usize {
    limit + 1
}

/// The newest tool call of an activity run, as a folded row prints it.
///
/// A fixed shape: `summary` and `outcome` serialize as `null` when the call
/// carries none, so the client falls back rather than reading around an absent
/// field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LastToolCall {
    pub sequence: u64,
    pub created_at: String,
    pub summary: Option<String>,
    pub outcome: Option<ToolCallOutcome>,
}

/// What one folded run of activity amounts to, whatever a page shipped of it.
///
/// The counts are the fact only the bridge can see: a client counts the rows
/// it was handed, and the cap means those are not all the rows there were. So
/// the span is named — `[from_sequence, through_sequence]`, the run's own first
/// and last item — and the census is exact over it, cap-omitted items included:
/// `rows` is every item of work in the span, which is the number the fold
/// prints, and `tool_calls` is the calls among them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ActivityDigest {
    pub from_sequence: u64,
    pub through_sequence: u64,
    pub tool_calls: u64,
    pub rows: u64,
    pub last_tool_call: Option<LastToolCall>,
}

/// The census of one span of a conversation: how many rows of work it holds,
/// and how many of those are tool calls. Inclusive at both ends, because a
/// digest's span is.
///
/// Memory and the store each answer it over the same rule — `is_activity()`
/// and `is_tool_call()`, hoisted into the store's `activity` and `tool_call`
/// columns — so a page reads the same numbers whichever path answered it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RunCensus {
    pub tool_calls: u64,
    pub rows: u64,
}

/// A page: what ships, and what the runs inside it mean. Held together because
/// they are one answer — items alone would say a run was a hundred calls long.
pub struct PageCut<T> {
    pub items: Vec<T>,
    pub digests: Vec<ActivityDigest>,
}

/// Cut a page out of the span it reaches over: keep everything that is not
/// activity, keep the newest of every run that is, and answer for each run
/// with a digest.
///
/// Two bounds hold the activity down, and a run keeps the smaller of them: the
/// per-run [`PAGE_ACTIVITY_RUN_CAP`], and what is left of the whole page's
/// [`page_activity_budget`]. The walk is newest-first, so the runs a reviewer
/// opens on are whole and the ones above them fold to their digests. Messages
/// and every other non-activity item ship whatever the budget has left — the
/// limit bought them, and they are what the page is FOR.
///
/// The digests are exact either way. Their counts come from the census over
/// each run's whole span, never from what shipped.
///
/// Generic over how the caller holds an item, so neither page path has to take
/// its span apart and put it back together: memory passes borrows off its
/// resident tail, the store passes the items it just decoded, and both get the
/// same cut back. Takes the span newest-first — the order both paths read in —
/// and hands the items back oldest-first, the order a conversation is rendered
/// in.
///
/// `census` is the count over a span, and the only thing that differs between
/// reading memory and reading SQLite. It is asked for the run's whole span,
/// which both callers can answer exactly: a run on a page never reaches below
/// the span, because the span's oldest item is a message and a message ends a
/// run.
pub fn cut_activity_runs<T, E>(
    span_newest_first: Vec<T>,
    limit: usize,
    census: impl Fn(u64, u64) -> Result<RunCensus, E>,
) -> Result<PageCut<T>, E>
where
    T: Borrow<ThreadItem>,
{
    let mut page = PageBeingCut {
        cut: PageCut {
            items: Vec::with_capacity(span_newest_first.len()),
            digests: Vec::new(),
        },
        activity_left: page_activity_budget(limit),
    };
    let mut run: Vec<T> = Vec::new();
    for item in span_newest_first {
        if item.borrow().is_activity() {
            run.push(item);
            continue;
        }
        page = fold_activity_run(page, std::mem::take(&mut run), &census)?;
        page.cut.items.push(item);
    }
    let mut cut = fold_activity_run(page, run, &census)?.cut;
    cut.items.reverse();
    cut.digests.reverse();
    Ok(cut)
}

/// A page part-way through being cut: the answer so far, and how much activity
/// it may still ship. Held together so the budget cannot be spent by anything
/// that is not also adding to the page.
pub(super) struct PageBeingCut<T> {
    cut: PageCut<T>,
    activity_left: usize,
}

/// Close one run onto the page: its digest, then as much of its newest as the
/// per-run cap and the page's remaining budget allow. Both the run and the
/// page being cut are newest-first, so the caller reverses once at the end
/// rather than per run.
pub(super) fn fold_activity_run<T, E>(
    mut page: PageBeingCut<T>,
    run_newest_first: Vec<T>,
    census: &impl Fn(u64, u64) -> Result<RunCensus, E>,
) -> Result<PageBeingCut<T>, E>
where
    T: Borrow<ThreadItem>,
{
    let (Some(newest), Some(oldest)) = (run_newest_first.first(), run_newest_first.last()) else {
        return Ok(page);
    };
    let from_sequence = oldest.borrow().sequence();
    let through_sequence = newest.borrow().sequence();
    let counted = census(from_sequence, through_sequence)?;
    page.cut.digests.push(ActivityDigest {
        from_sequence,
        through_sequence,
        tool_calls: counted.tool_calls,
        rows: counted.rows,
        last_tool_call: run_newest_first
            .iter()
            .find_map(|item| item.borrow().tool_call().map(LastToolCall::of)),
    });
    let ships = run_items_shipped(run_newest_first.len(), page.activity_left);
    page.activity_left -= ships;
    page.cut
        .items
        .extend(run_newest_first.into_iter().take(ships));
    Ok(page)
}

/// One page of one run's activity: the items, where they start, and whether
/// the span holds older ones.
///
/// Takes the span oldest-first with the one row of overflow
/// [`activity_rows_read`] asked for, and ships everything but that row.
/// Generic over how the caller holds an item so the two readers — the resident
/// tail, which borrows, and the store, which owns what it decoded — build one
/// answer rather than two that have to be kept equal.
///
/// No digests: a client asks for this span because it already holds the digest
/// that named it.
pub fn wire_value_activity_page<T: Borrow<ThreadItem>>(
    span_oldest_first: &[T],
    limit: usize,
) -> Value {
    let has_more = span_oldest_first.len() > limit;
    let page: Vec<&ThreadItem> = span_oldest_first
        .iter()
        .skip(usize::from(has_more))
        .map(Borrow::borrow)
        .collect();
    count_serialized_items(page.len());
    json!({
        "items": page,
        "oldest_sequence": page.first().map(|item| item.sequence()),
        "has_more": has_more,
    })
}

/// How many hits a query returns when it does not say.
pub const DEFAULT_QUERY_LIMIT: usize = 20;

/// The most any one query returns, however large a limit it asks for.
pub const MAX_QUERY_LIMIT: usize = 100;

pub(super) fn default_query_limit() -> usize {
    DEFAULT_QUERY_LIMIT
}

/// What an entry says about itself in the inbox: whether anything has needed
/// the human since they last read the conversation, how much, and why.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UnreadSummary {
    pub count: u64,
    /// The newest attention item's kind. `None` exactly when `count` is 0.
    pub reason: Option<&'static str>,
}

impl Thread {
    pub(super) fn refresh_conversation_summary_from_resident(&mut self) {
        for item in &self.items {
            match item {
                ThreadItem::Message(message)
                    if message.sequence >= self.last_message_sequence_summary =>
                {
                    self.last_message_sequence_summary = message.sequence;
                    if message.from_agent.is_none() && message.sent_to.is_none() {
                        self.last_own_message_sequence_summary = message.sequence;
                    }
                    self.conversation_activity_at_summary = Some(message.created_at.clone());
                    self.conversation_working = match message.role {
                        MessageRole::User => message.seen_at.is_some(),
                        MessageRole::Agent => message.still_working,
                    };
                }
                ThreadItem::Event(event)
                    if self.conversation_working
                        && event.sequence > self.last_message_sequence_summary
                        && event.event.class() == EventClass::Attention =>
                {
                    self.conversation_activity_at_summary = Some(event.created_at.clone());
                    self.conversation_working = false;
                }
                _ => {}
            }
            if item.attention_reason().is_some() {
                self.last_attention_sequence_summary =
                    self.last_attention_sequence_summary.max(item.sequence());
            }
        }
    }
    /// The oldest sequence this process read, or 0 when it read the whole
    /// conversation — the floor under which the stored history is not this
    /// process's to rewrite.
    pub fn resident_from_sequence(&self) -> u64 {
        self.resident_from_sequence
    }
    /// How long the whole conversation is, resident or not. What the client is
    /// told, because it is what the client's gap check means: "is my cache a
    /// window, or did it lose something?"
    pub fn total_item_count(&self) -> u64 {
        self.earlier_item_count + self.items.len() as u64
    }
    /// Whether the page asked for reaches under the tail this process holds,
    /// and so has to be read from the store instead of out of memory.
    ///
    /// Measured the way the page is: the tail answers when it holds the page's
    /// worth of MESSAGES below the seek. A tail of pure activity holds no page
    /// at all, however many items it holds — which is also what makes a page
    /// answered from memory able to count its own runs, since its oldest walked
    /// item is then a resident message.
    pub fn page_reaches_stored_history(&self, before_sequence: Option<u64>, limit: usize) -> bool {
        if self.earlier_item_count == 0 {
            return false;
        }
        let before = before_sequence.unwrap_or(u64::MAX);
        self.items
            .iter()
            .filter(|item| item.sequence() < before && item.counts_toward_page())
            .count()
            < limit
    }
    /// How many rows of work, and how many tool calls among them, the
    /// resident tail holds between two sequences, inclusive — the memory
    /// census, and the exact sibling of the store's `run_census`.
    ///
    /// Exact wherever a page is allowed to ask it: a page answered out of
    /// memory has `page_reaches_stored_history() == false`, so every run it
    /// touches is resident whole and nothing in `[from, through]` sits under
    /// the tail.
    pub fn run_census(&self, from_sequence: u64, through_sequence: u64) -> RunCensus {
        self.items
            .iter()
            .filter(|item| (from_sequence..=through_sequence).contains(&item.sequence()))
            .fold(RunCensus::default(), |counted, item| RunCensus {
                tool_calls: counted.tool_calls + u64::from(item.is_tool_call()),
                rows: counted.rows + u64::from(item.is_activity()),
            })
    }
    /// The newest activity of a span the tail holds, oldest-first — what an
    /// opened run renders, read out of memory.
    ///
    /// The span is inclusive at both ends, because it is a digest's own
    /// `[from_sequence, through_sequence]`; `before_sequence` is the backward
    /// walk's seek and is exclusive, the way every other page's is. The exact
    /// sibling of the store's `thread_activity_range`, and the caller picks
    /// between them by whether the tail reaches the span at all.
    pub fn activity_between(
        &self,
        from_sequence: u64,
        through_sequence: u64,
        before_sequence: Option<u64>,
        limit: usize,
    ) -> Vec<&ThreadItem> {
        let below = before_sequence.unwrap_or(u64::MAX);
        let span: Vec<&ThreadItem> = self
            .items
            .iter()
            .filter(|item| {
                item.is_activity()
                    && item.sequence() < below
                    && (from_sequence..=through_sequence).contains(&item.sequence())
            })
            .collect();
        span[span.len().saturating_sub(limit)..].to_vec()
    }
    /// Whether a cursor this far back reaches under the tail this process
    /// holds, and so has to be completed out of the store.
    ///
    /// An item under the tail is one this process cannot mutate — it does not
    /// hold it — so the only news down there is a mutation the process before
    /// this one made, and every one of those is at or below the counter value
    /// the load read. A cursor past that mark has already been told everything
    /// the history has to say, which is what stops a caught-up client from
    /// asking the store anything on its steady-state polls.
    pub fn cursor_reaches_stored_history(&self, after_sequence: u64) -> bool {
        self.earlier_item_count > 0 && after_sequence < self.stored_last_sequence
    }
    /// When the newest resident item was created. This is conversation-detail
    /// metadata, not the inbox activity clock: tool and lifecycle events count
    /// here, while [`conversation_activity_at`](Self::conversation_activity_at)
    /// deliberately excludes them.
    pub fn last_item_at(&self) -> Option<&str> {
        self.items.last().map(ThreadItem::created_at)
    }
    /// When the USER said each of the things they have said here, in order.
    ///
    /// The boot migration's input: an entity that predates anchors is anchored
    /// by replaying exactly these through the anchor rule, so its place in the
    /// inbox is the place it would always have had.
    pub fn user_message_times(&self) -> impl Iterator<Item = &str> {
        Thread::user_message_times_in(&self.items)
    }
    /// The same reading of items the caller read for itself. The migration is
    /// about everything the user ever said, so it looks at the conversation
    /// whole rather than at whatever tail a load left resident.
    pub fn user_message_times_in(items: &[ThreadItem]) -> impl Iterator<Item = &str> {
        items.iter().filter_map(|item| match item {
            ThreadItem::Message(message) if message.role == MessageRole::User => {
                Some(message.created_at.as_str())
            }
            _ => None,
        })
    }
    /// The items a `changes` push carries after a cursor, oldest first.
    ///
    /// Reads `latest_sequence`, not `sequence`: a push is a DELTA — "what has
    /// happened here that you have not been told about" — and a message whose
    /// delivery status moved from queued to sent has happened, however old the
    /// message is. Cutting by creation order instead would leave that message
    /// out of every push it is ever the subject of, and the reader watching
    /// their own message sit on "Queued" would go on watching it until
    /// something else read the conversation whole. The forward PAGE is the one
    /// that walks creation order ([`resident_after_sequence`]).
    ///
    /// `None` — send the tip alone and let the client page forward — when
    /// more than `limit` of them are waiting, or when the cursor sits under
    /// the tail this process holds. Both are a client better served by one
    /// read of its own than by a push bigger than that read.
    pub fn push_items_after(&self, after_sequence: u64, limit: usize) -> Option<Vec<Value>> {
        if self.forward_page_reaches_stored_history(after_sequence) {
            return None;
        }
        let window = self.resident_after(after_sequence);
        (window.len() <= limit).then(|| window.into_iter().map(|item| json!(item)).collect())
    }

    /// The first line the human said here, for a conversation with no topic
    /// of its own to be called by in a list.
    ///
    /// Cut to `max_chars` characters, because the first thing said to an
    /// agent is as often a pasted page as a sentence, and a title is a line.
    /// `None` when nobody has said anything yet — and for a conversation so
    /// long its opening message sits under the tail this process holds, which
    /// is a conversation that has had a topic set on it or has been going
    /// long enough not to need one.
    pub fn first_user_line(&self, max_chars: usize) -> Option<String> {
        let opening = self.items.iter().find_map(|item| match item {
            ThreadItem::Message(message) if message.role == MessageRole::User => {
                Some(&message.body)
            }
            _ => None,
        })?;
        let line = opening
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())?;
        Some(line.chars().take(max_chars).collect())
    }

    /// The items this process holds that a cursor has not been told about.
    pub(super) fn resident_after(&self, after_sequence: u64) -> Vec<&ThreadItem> {
        self.items
            .iter()
            .filter(|item| item.latest_sequence() > after_sequence)
            .collect()
    }
    /// Backward view for a first load and for scrolling up: the newest `limit`
    /// items strictly older than `before_sequence`, ascending, in the shape
    /// `wire_value_after` produces plus the two fields a backward walk needs —
    /// `oldest_sequence`, the seek for the next page up, and `has_more`,
    /// whether asking for one is worth it.
    ///
    /// Reads the tail this process holds, so `has_more` counts the history no
    /// load read as pages still to come: the caller answers those from the
    /// store through [`wire_value_of_page`](Self::wire_value_of_page).
    pub fn wire_value_page(&self, before_sequence: Option<u64>, limit: usize) -> Value {
        // No bound means "from the newest", which the same filter expresses as
        // a point past every sequence there could be.
        let before = before_sequence.unwrap_or(u64::MAX);
        let older: Vec<&ThreadItem> = self
            .items
            .iter()
            .filter(|item| item.sequence() < before)
            .collect();
        let reached = older.len() - page_span(older.iter().rev().copied(), limit);
        let span: Vec<&ThreadItem> = older[reached..].iter().rev().copied().collect();
        // The census is exact over the span every digest NAMES, because the
        // walk never leaves the resident tail: nothing in `[from, through]`
        // sits under it. A digest covers a WHOLE run when the walk reached a
        // message, which is what the page gate buys — a page memory answers
        // has `page_reaches_stored_history() == false`, so its limit-th
        // message is resident and a message ends the oldest run. A conversation
        // shorter than the limit is reached whole and has nothing below it.
        let census = |from: u64, through: u64| {
            Ok::<RunCensus, std::convert::Infallible>(self.run_census(from, through))
        };
        let cut = match cut_activity_runs(span, limit, census) {
            Ok(cut) => cut,
            Err(impossible) => match impossible {},
        };
        // What is left below what shipped, plus the history no load read: both
        // are pages the client can still ask for.
        let outstanding = match cut.items.first().map(|item| item.sequence()) {
            Some(oldest) => older.iter().filter(|item| item.sequence() < oldest).count(),
            None => older.len(),
        } + self.earlier_item_count as usize;
        self.wire_value_of_page(&cut, outstanding > 0)
    }
    /// Whether a forward page from this cursor reaches under the tail this
    /// process loaded, and so has to be completed out of the store.
    ///
    /// The sibling of [`page_reaches_stored_history`](Self::page_reaches_stored_history),
    /// measured the way a forward page is: the very next item after the
    /// cursor. A cursor sitting at or above the tail's floor has everything
    /// above it resident, whatever the store holds below.
    pub fn forward_page_reaches_stored_history(&self, after_sequence: u64) -> bool {
        self.earlier_item_count > 0
            && after_sequence.saturating_add(1) < self.resident_from_sequence
    }
    /// Forward view for a cache filling its gap: the oldest `limit` items
    /// strictly after `after_sequence`, in the order they happened, in the
    /// page shape a backward walk answers in.
    ///
    /// No digests and no folding. The caller is caching the conversation, not
    /// rendering a sitting of it, and a run folded away would be a hole its
    /// cache could not tell from history. `has_more` says another page waits,
    /// and the client asks again from the last item it was handed.
    pub fn wire_value_page_after(&self, after_sequence: u64, limit: usize) -> Value {
        let window = self.resident_after_sequence(after_sequence).collect();
        self.wire_value_of_forward_window(window, limit)
    }
    /// The newest `limit` items strictly after a cache's cursor, oldest first.
    ///
    /// The resident tail is larger than the largest forward page, so the tip
    /// always lives in memory even when the cursor predates what was loaded.
    /// `has_more` then means exactly what this mode needs: items newer than the
    /// cursor were omitted between that cursor and the returned window.
    pub fn wire_value_newest_page_after(&self, after_sequence: u64, limit: usize) -> Value {
        let window: Vec<&ThreadItem> = self.resident_after_sequence(after_sequence).collect();
        let omitted_resident = window.len() > limit;
        let start = window.len().saturating_sub(limit);
        let cut = PageCut {
            items: window.into_iter().skip(start).collect(),
            digests: Vec::new(),
        };
        let has_more = omitted_resident || self.forward_page_reaches_stored_history(after_sequence);
        self.wire_value_of_page(&cut, has_more)
    }
    /// The same forward view, completed with items read back out of the
    /// store: the history under the tail this process loaded. Items the tail
    /// already holds are taken from the tail — memory is the fresher copy of
    /// those, exactly as it is for the mutation cursor.
    pub fn wire_value_page_after_including_history(
        &self,
        after_sequence: u64,
        limit: usize,
        history: &[ThreadItem],
    ) -> Value {
        let window: Vec<&ThreadItem> = history
            .iter()
            .filter(|item| {
                item.sequence() > after_sequence && item.sequence() < self.resident_from_sequence
            })
            .chain(self.resident_after_sequence(after_sequence))
            .collect();
        self.wire_value_of_forward_window(window, limit)
    }
    /// The resident items that happened after a cursor, oldest first.
    ///
    /// Reads `sequence`, not `latest_sequence`: a forward PAGE is a walk
    /// through the conversation's own order, where a mutated item keeps its
    /// place. The mutation cursor
    /// ([`resident_after`](Self::resident_after)) is the one that reads the
    /// other column.
    fn resident_after_sequence(&self, after_sequence: u64) -> impl Iterator<Item = &ThreadItem> {
        self.items
            .iter()
            .filter(move |item| item.sequence() > after_sequence)
    }
    /// One forward page, cut out of the window that reaches past it.
    fn wire_value_of_forward_window(&self, window: Vec<&ThreadItem>, limit: usize) -> Value {
        let has_more = window.len() > limit;
        let cut = PageCut {
            items: window.into_iter().take(limit).collect(),
            digests: Vec::new(),
        };
        self.wire_value_of_page(&cut, has_more)
    }
    /// The page shape, around the cut the caller already made — off the tail
    /// this process holds, or off a page read back out of the store. Takes the
    /// cut whole, borrowed or owned, so items and digests cannot be assembled
    /// out of sync.
    ///
    /// A page is NOT guaranteed contiguous: the per-run cap omits items inside
    /// a run of activity, and `activity_digests` is what accounts for them —
    /// one per run the page touches, exact over the whole run. `has_more` is
    /// unchanged and means what it always did: something sits below the page's
    /// oldest SHIPPED item.
    ///
    /// `thread_total` still counts the whole conversation, not the page, so the
    /// client can tell "my cache is a bounded window" from "my cache lost
    /// something" — the gap check the forward cursor already relies on.
    pub fn wire_value_of_page<T: Borrow<ThreadItem>>(
        &self,
        cut: &PageCut<T>,
        has_more: bool,
    ) -> Value {
        let page: Vec<&ThreadItem> = cut.items.iter().map(Borrow::borrow).collect();
        count_serialized_items(page.len());
        json!({
            "id": self.id,
            "thread_id": self.id,
            "thread_generation_revision": self.generation_revision,
            "agent": self.agent,
            "sessions": self.sessions,
            "items": page,
            "activity_digests": cut.digests,
            "revisions": self.revision_summaries(),
            "last_completion": self.last_completion,
            "thread_total": self.total_item_count(),
            "thread_last_sequence": self.last_sequence(),
            "oldest_sequence": page.first().map(|item| item.sequence()),
            "has_more": has_more,
        })
    }
    /// Whether the packet has to be read from the store rather than off the
    /// tail — the sibling of [`page_reaches_stored_history`](Self::page_reaches_stored_history).
    ///
    /// True only when there is history under the tail AND the tail itself does
    /// not hold the packet's worth of messages. Both halves are answered off
    /// integers this process already has, so a conversation held whole — the
    /// common small case, and every storeless test daemon — hands a packet at
    /// today's speed and touches no SQL. The starved tail is the one that pays
    /// the read, and it is the one the packet exists for.
    pub fn catch_up_reaches_stored_history(&self, limit: usize) -> bool {
        self.earlier_item_count > 0
            && self
                .items
                .iter()
                .filter(|item| matches!(item, ThreadItem::Message(_)))
                .count()
                < limit
    }
}

#[cfg(test)]
mod tests;

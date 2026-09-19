use super::support::*;
use super::*;

/// The conversation is the one thing that cannot be re-derived, and saving
/// splits it off the record into its own rows. It has to come back — from a
/// store opened again from scratch, not from the process that wrote it.
#[test]
fn a_runs_conversation_survives_a_store_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("tasks");
    let mut record = run_record("run-1", Some("plan-1"), NOW);
    let agent_id = record.agents[0].id.clone();
    record.agents[0]
        .thread
        .post_user("build the thing", None, NOW);
    record.agents[0].thread.post_agent("on it", None, NOW);

    Store::new(&root)
        .expect("store opens")
        .save_run(&record)
        .expect("the run saves");

    let reloaded = reload_run(&Store::new(&root).expect("store reopens"), "run-1");
    assert_eq!(reloaded.agents.len(), 1);
    assert_eq!(reloaded.agents[0].id, agent_id);
    assert_eq!(
        reloaded.agents[0].thread.items, record.agents[0].thread.items,
        "every conversation item comes back unchanged"
    );
}

/// Appending one message writes ONE row. This is the whole reason the store
/// changed: the JSON records it replaced rewrote every conversation on the
/// Issue for every append, and a store that upserted all N items per save
/// would have moved that cost rather than removed it.
#[test]
fn appending_one_message_writes_one_row_however_long_the_conversation_is() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().join("tasks")).expect("store opens");
    let mut record = run_record("run-1", None, NOW);
    for n in 0..60 {
        record.agents[0]
            .thread
            .post_user(format!("message {n}"), None, NOW);
    }
    store.save_run(&record).expect("the first save writes");

    let before = store.total_changes();
    record.agents[0].thread.post_user("one more", None, NOW);
    store.save_run(&record).expect("the append saves");
    let written = store.total_changes() - before;

    // One thread item, plus the agent row and the run row that always
    // carry the entity's own state. Never the 61 items already stored.
    assert!(
        written <= 3,
        "an append wrote {written} rows — the conversation is being rewritten"
    );
    assert_eq!(reload_run(&store, "run-1").agents[0].thread.items.len(), 61);
}

/// A conversation comes back in the order it happened, and an item taken
/// off it is gone rather than resurrected by the next load.
#[test]
fn conversation_items_keep_their_order_and_a_removed_item_stays_removed() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().join("tasks")).expect("store opens");
    let mut record = run_record("run-1", None, NOW);
    for n in 0..5 {
        record.agents[0]
            .thread
            .post_user(format!("message {n}"), None, NOW);
    }
    store.save_run(&record).expect("the run saves");

    let ordered: Vec<u64> = reload_run(&store, "run-1").agents[0]
        .thread
        .items
        .iter()
        .map(|item| item.sequence())
        .collect();
    assert!(
        ordered.windows(2).all(|pair| pair[0] < pair[1]),
        "items came back out of order: {ordered:?}"
    );

    record.agents[0].thread.items.remove(2);
    store.save_run(&record).expect("the shortened run saves");
    let after = reload_run(&store, "run-1");
    assert_eq!(after.agents[0].thread.items.len(), 4);
    assert_eq!(
        after.agents[0].thread.items, record.agents[0].thread.items,
        "the removed item did not come back"
    );
}

/// Inbox summaries survive when the message they describe is older than
/// the resident tail. Tool work is not conversation activity, while the
/// attention event that ends a read user's in-flight turn records its stop.
#[test]
fn conversation_summary_survives_activity_burial_and_restart() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("tasks");
    let mut record = run_record("run-1", None, NOW);
    let thread = &mut record.agents[0].thread;
    let message_sequence = {
        thread.post_user("please investigate", None, "2026-08-13T10:00:00Z");
        thread.last_message_sequence()
    };
    thread.read_unread("2026-08-13T10:01:00Z");
    for n in 0..=RESIDENT_CONVERSATION_TAIL {
        thread.push_event(
            crate::thread::ThreadEventKind::ToolUse,
            Some(format!("tool {n}")),
            None,
            None,
            "2026-08-13T10:02:00Z",
        );
    }
    assert_eq!(
        thread.conversation_activity_at(),
        Some("2026-08-13T10:00:00Z"),
        "tool activity is not conversation activity"
    );
    let attention_sequence = thread.push_event(
        crate::thread::ThreadEventKind::Blocked,
        Some("need input".into()),
        None,
        None,
        "2026-08-13T10:03:00Z",
    );
    for n in 0..=RESIDENT_CONVERSATION_TAIL {
        thread.push_event(
            crate::thread::ThreadEventKind::ToolUse,
            Some(format!("later tool {n}")),
            None,
            None,
            "2026-08-13T10:04:00Z",
        );
    }

    Store::new(&root).unwrap().save_run(&record).unwrap();
    let reopened = Store::new(&root).unwrap();
    let loaded = reload_run(&reopened, "run-1");
    let thread = &loaded.agents[0].thread;
    assert!(thread.items.len() <= RESIDENT_CONVERSATION_TAIL);
    assert!(thread
        .items
        .iter()
        .all(|item| !matches!(item, ThreadItem::Message(_)) && item.attention_reason().is_none()));
    assert_eq!(thread.last_message_sequence(), message_sequence);
    assert_eq!(thread.last_attention_sequence(), attention_sequence);
    assert_eq!(
        thread.conversation_activity_at(),
        Some("2026-08-13T10:03:00Z")
    );
    assert!(
        thread.last_sequence() > thread.last_message_sequence(),
        "the seen mutation and tools advance only the general cursor"
    );
}

/// The dismissal line is one of those summaries, and it counts only what
/// this conversation's own two parties said. A run of hand-offs on top of a
/// buried message is the case the hoisted column exists for: without it the
/// newest message the store can find is a machine's, and a row the human
/// cleared comes back at every boot.
#[test]
fn the_own_message_line_survives_a_burial_under_hand_offs() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("tasks");
    let mut record = run_record("run-1", None, NOW);
    let thread = &mut record.agents[0].thread;
    thread.post_agent("which name did you want?", None, "2026-08-13T10:00:00Z");
    let own_sequence = thread.last_own_message_sequence();
    for n in 0..=RESIDENT_CONVERSATION_TAIL {
        thread.post_user_from_agent(
            format!("step {n}"),
            crate::thread::AgentIdentity::new("project-1".to_string()),
            "2026-08-13T10:02:00Z",
        );
    }
    assert_eq!(thread.last_own_message_sequence(), own_sequence);
    assert!(thread.last_message_sequence() > own_sequence, "in memory");

    Store::new(&root).unwrap().save_run(&record).unwrap();
    let loaded = reload_run(&Store::new(&root).unwrap(), "run-1");
    let thread = &loaded.agents[0].thread;
    assert!(
        thread.items.iter().all(ThreadItem::is_handoff),
        "the tail holds nothing but hand-offs"
    );
    assert_eq!(thread.last_own_message_sequence(), own_sequence);
    assert!(
        thread.last_message_sequence() > own_sequence,
        "and reloaded"
    );
}

/// What paging is for, said at the load: a conversation costs the daemon
/// its tail, not its length. A boot that reads every item of every
/// conversation back into memory pays the whole cost the paged reads
/// exist to avoid, however carefully those reads seek.
#[test]
fn a_load_reads_the_tail_of_a_conversation_not_all_of_it() {
    let dir = tempfile::tempdir().unwrap();
    let held = RESIDENT_CONVERSATION_TAIL + 60;
    let (store, _agent_id) = store_with_conversation(&dir.path().join("tasks"), held);

    let reloaded = reload_run(&store, "run-1");
    let thread = &reloaded.agents[0].thread;
    assert_eq!(
        thread.items.len(),
        RESIDENT_CONVERSATION_TAIL,
        "the load is bounded by the tail, not by the conversation"
    );
    assert_eq!(
        thread.items.first().map(ThreadItem::sequence),
        Some(61),
        "the tail is the newest items, so the load starts past the first 60"
    );
    assert_eq!(
        thread.total_item_count(),
        held as u64,
        "a conversation still knows how long it is, whatever was read of it"
    );
    assert_eq!(
        thread.last_sequence(),
        held as u64,
        "appends carry on from the end of the conversation, not the end of the tail"
    );
}

/// A short conversation is not a special case — a page wider than the
/// conversation is simply the whole of it.
#[test]
fn a_page_wider_than_the_conversation_returns_all_of_it() {
    let dir = tempfile::tempdir().unwrap();
    let (store, agent_id) = store_with_conversation(&dir.path().join("tasks"), 4);

    let page = store
        .thread_page(&agent_id, None, 500)
        .expect("a page reads");
    assert_eq!(sequences(&page), vec![1, 2, 3, 4]);
}

/// The forward cursor is not a `sequence > ?` filter. Marking a message
/// seen moves its `updated_sequence` and leaves its creation sequence
/// where it was, and the client polling from a cursor past that creation
/// sequence still has to be told.
#[test]
fn the_forward_cursor_returns_a_message_mutated_in_place() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("tasks");
    let store = Store::new(&root).expect("store opens");
    let mut record = run_record("run-1", None, NOW);
    record.agents[0].thread.post_user("read me", None, NOW);
    record.agents[0].thread.post_agent("on it", None, NOW);
    store.save_run(&record).expect("the conversation saves");
    let agent_id = record.agents[0].id.clone();
    let cursor = record.agents[0].thread.last_sequence();

    assert!(
        store
            .thread_items_after(&agent_id, cursor)
            .expect("the cursor reads")
            .is_empty(),
        "nothing has happened since the cursor yet"
    );

    let seen = record.agents[0].thread.read_unread(NOW);
    assert_eq!(seen.len(), 1, "there is one unread message to mark seen");
    store.save_run(&record).expect("the mutation saves");

    let delta = store
        .thread_items_after(&agent_id, cursor)
        .expect("the cursor reads");
    assert_eq!(
        sequences(&delta),
        vec![1],
        "the message whose updated_sequence moved did not come back"
    );
}

/// How long a conversation is, without reading it. The client needs the
/// total to know whether it holds the whole thing; loading the items to
/// count them would undo the paging it pays for.
#[test]
fn counting_a_conversation_does_not_load_it() {
    let dir = tempfile::tempdir().unwrap();
    let (store, agent_id) = store_with_conversation(&dir.path().join("tasks"), 60);

    assert_eq!(
        store.thread_item_count(&agent_id).expect("the count reads"),
        60
    );
    assert_eq!(
        store
            .thread_page(&agent_id, None, 5)
            .expect("a page reads")
            .len(),
        5,
        "the count is not the size of what a read returns"
    );
    assert_eq!(
        store
            .thread_item_count("no-such-agent")
            .expect("the count reads"),
        0
    );
}

/// The contract underneath every paging test above: both reads seek into
/// the conversation rather than walking it. A predicate SQLite cannot
/// answer from an index — or an `ORDER BY` it has to satisfy with a sort —
/// reads every row of a 600-item conversation to hand back ten of them,
/// and the returned page looks identical either way.
#[test]
fn the_conversation_reads_seek_instead_of_walking() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().join("tasks")).expect("store opens");
    let connection = store.connection();
    for statement in [
        THREAD_PAGE_SQL,
        THREAD_FORWARD_PAGE_SQL,
        THREAD_CURSOR_SQL,
        THREAD_MESSAGE_PAGE_SQL,
        THREAD_CONVERSATION_FLOOR_SQL,
        THREAD_CONVERSATION_STRUCTURE_SQL,
        THREAD_ACTIVITY_RANGE_SQL,
        THREAD_RUN_OLDEST_SQL,
        THREAD_RUN_LAST_CALL_SQL,
        THREAD_TOOL_CALL_COUNT_SQL,
        THREAD_ACTIVITY_COUNT_SQL,
    ] {
        let plan = query_plan(&connection, statement);
        assert!(
            plan.iter().any(|step| step.contains("SEARCH")),
            "{statement} does not seek: {plan:?}"
        );
        assert!(
            !plan
                .iter()
                .any(|step| step.contains("SCAN") || step.contains("TEMP B-TREE")),
            "{statement} walks or sorts the conversation: {plan:?}"
        );
    }
}

/// The forward cursor's index by name. `thread_items_cursor` exists only
/// for this query — the primary key already covers `agent_id` — so a plan
/// that no longer names it means the index is dead weight on every write.
#[test]
fn the_forward_cursor_reads_through_its_own_index() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().join("tasks")).expect("store opens");
    let connection = store.connection();
    let plan = query_plan(&connection, THREAD_CURSOR_SQL);
    assert!(
        plan.iter().any(|step| step.contains("thread_items_cursor")),
        "the forward cursor does not use thread_items_cursor: {plan:?}"
    );
}

/// The run reads' index by name. A page reads the newest of each run
/// between two things somebody said, and the span above a page's floor
/// holds far more work than conversation — so a run read answered off the
/// primary key would step over every row of the span to find the activity
/// in it, which is the walk the bounded read exists to stop.
#[test]
fn the_run_reads_seek_through_the_activity_index() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().join("tasks")).expect("store opens");
    let connection = store.connection();
    for statement in [THREAD_ACTIVITY_RANGE_SQL, THREAD_RUN_OLDEST_SQL] {
        let plan = query_plan(&connection, statement);
        assert!(
            plan.iter()
                .any(|step| step.contains("thread_items_activity")),
            "{statement} does not use thread_items_activity: {plan:?}"
        );
    }
    let plan = query_plan(&connection, THREAD_RUN_LAST_CALL_SQL);
    assert!(
        plan.iter()
            .any(|step| step.contains("thread_items_tool_calls")),
        "the run's last call does not use thread_items_tool_calls: {plan:?}"
    );
}

/// The message index by name. The primary key would answer this statement
/// too — by walking every tool call between the words, which is the whole
/// cost the partial index exists to skip — and the page it returned would
/// look identical either way. So the plan is pinned to the index, not
/// merely to a seek.
#[test]
fn the_message_page_reads_through_the_message_index() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().join("tasks")).expect("store opens");
    let connection = store.connection();
    let plan = query_plan(&connection, THREAD_MESSAGE_PAGE_SQL);
    assert!(
        plan.iter()
            .any(|step| step.contains("thread_items_messages")),
        "the message page does not use thread_items_messages: {plan:?}"
    );

    // The same for the seek that finds where a page reaches back to: on
    // the primary key it would count every tool call on the way down.
    let plan = query_plan(&connection, THREAD_CONVERSATION_FLOOR_SQL);
    assert!(
        plan.iter()
            .any(|step| step.contains("thread_items_messages")),
        "the page floor does not use thread_items_messages: {plan:?}"
    );
}

/// Removing an agent from the roster takes its conversation with it — a
/// left-behind row would reappear on the next load as an agent the entity
/// no longer has.
#[test]
fn removing_an_agent_removes_its_conversation() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().join("tasks")).expect("store opens");
    let mut record = run_record("run-1", None, NOW);
    let mut roster = AgentRoster::restore(
        "run-1",
        record.agents.clone(),
        Default::default(),
        ModelChoice::default(),
        NOW,
    );
    let second = roster.add("run-1", ModelChoice::default(), NOW).id.clone();
    roster
        .by_id_mut(&second)
        .expect("the second agent is on the roster")
        .thread
        .post_user("only agent two hears this", None, NOW);
    record.agents = roster.agents().to_vec();
    store.save_run(&record).expect("both agents save");
    assert_eq!(reload_run(&store, "run-1").agents.len(), 2);

    roster
        .remove(&second)
        .expect("the second agent is removable");
    record.agents = roster.agents().to_vec();
    store.save_run(&record).expect("the shortened roster saves");

    let after = reload_run(&store, "run-1");
    assert_eq!(after.agents.len(), 1);
    assert!(after.agents.iter().all(|agent| agent.id != second));
}

/// A conversation is loaded as its newest items, so the badge has to count
/// what is under them. This is the query that does it — and it counts only
/// what calls the human, only between the cursor and the tail.
#[test]
fn unread_under_the_tail_is_counted_in_the_database() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().join("tasks")).expect("store opens");
    let mut record = run_record("run-1", None, NOW);
    let agent_id = record.agents[0].id.clone();
    // Agent messages call the human; status events do not.
    for n in 0..10 {
        record.agents[0]
            .thread
            .post_agent(format!("said {n}"), None, NOW);
        record.agents[0].thread.push_event(
            crate::thread::ThreadEventKind::Triaged,
            None,
            None,
            None,
            NOW,
        );
    }
    store.save_run(&record).expect("the run saves");
    let floor = record.agents[0].thread.last_sequence() + 1;

    assert_eq!(
        store
            .unread_attention_between(&agent_id, 0, floor)
            .expect("the count runs"),
        10,
        "only the items that call the human are counted"
    );
    // A cursor inside the conversation counts only what is above it.
    let midpoint = record.agents[0].thread.items[9].sequence();
    let above = store
        .unread_attention_between(&agent_id, midpoint, floor)
        .expect("the count runs");
    assert!(above < 10 && above > 0, "counted {above} above the cursor");
    // A conversation held whole has no history under it to ask about.
    assert_eq!(
        store
            .unread_attention_between(&agent_id, 0, 0)
            .expect("the count runs"),
        0
    );
}

/// A conversation buried in activity still hands its messages back, and
/// hands back nothing else: the query the catch-up packet reads through,
/// against exactly the thread that starved it.
#[test]
fn the_message_page_reads_past_the_activity_between_the_words() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().join("tasks")).expect("store opens");
    let mut record = run_record("run-1", None, NOW);
    record.agents[0]
        .thread
        .post_user("please rename the helper", None, NOW);
    for index in 0..300 {
        record.agents[0].thread.push_event(
            crate::thread::ThreadEventKind::ToolUse,
            Some(format!("Read file-{index}.rs")),
            None,
            None,
            NOW,
        );
    }
    record.agents[0].thread.post_outcome(
        crate::thread::MessageOutcome::Blocked,
        "needs production credentials",
        None,
        NOW,
    );
    record.agents[0].thread.push_event(
        crate::thread::ThreadEventKind::Interrupted,
        Some("the daemon restarted".to_string()),
        None,
        None,
        NOW,
    );
    store.save_run(&record).expect("the conversation saves");
    let agent_id = record.agents[0].id.clone();

    let messages = store
        .thread_message_page(&agent_id, 40)
        .expect("the message page reads");
    let bodies: Vec<String> = messages
        .iter()
        .map(|item| match item {
            ThreadItem::Message(message) => message.body.clone(),
            ThreadItem::Event(event) => panic!("an event came back: {event:?}"),
        })
        .collect();
    assert_eq!(
        bodies,
        vec![
            "please rename the helper".to_string(),
            "needs production credentials".to_string(),
        ],
        "the words are handed back oldest-first, activity and observations left behind"
    );
}

/// The limit counts messages and keeps the newest of them, whatever sits
/// between.
#[test]
fn the_message_page_keeps_the_newest_messages_up_to_its_limit() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().join("tasks")).expect("store opens");
    let mut record = run_record("run-1", None, NOW);
    for index in 0..10 {
        record.agents[0]
            .thread
            .post_user(format!("ask {index}"), None, NOW);
        record.agents[0].thread.push_event(
            crate::thread::ThreadEventKind::Reasoning,
            Some("thinking".to_string()),
            None,
            None,
            NOW,
        );
    }
    store.save_run(&record).expect("the conversation saves");

    let messages = store
        .thread_message_page(&record.agents[0].id, 3)
        .expect("the message page reads");
    assert_eq!(sequences(&messages), vec![15, 17, 19], "{messages:?}");
}

/// The two readings of the activity rule — `ThreadItem::is_activity()` and
/// the store's `activity = 1` — held equal over every kind there is. The
/// bounded page read seeks down this column, so a column that drifted from
/// the enum would cut runs where no run ends.
#[test]
fn the_hoisted_activity_column_agrees_with_the_activity_rule() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().join("tasks")).expect("store opens");
    let mut record = run_record("run-1", None, NOW);
    for kind in crate::thread::ThreadEventKind::ALL {
        record.agents[0]
            .thread
            .push_event(kind, Some(kind.as_str().to_string()), None, None, NOW);
    }
    record.agents[0].thread.post_user("a question", None, NOW);
    record.agents[0].thread.post_agent("an answer", None, NOW);
    store.save_run(&record).expect("the conversation saves");

    let activity_in_rust: Vec<u64> = record.agents[0]
        .thread
        .items
        .iter()
        .filter(|item| item.is_activity())
        .map(ThreadItem::sequence)
        .collect();

    assert!(!activity_in_rust.is_empty(), "the fixture folds nothing");
    assert_eq!(
        stored_activity_sequences(&store, &record.agents[0].id),
        activity_in_rust
    );
}

/// A v4 database gains the activity column and is classified in place, the
/// way v3 gained tool_call. Nobody's stored conversation has to be
/// rewritten for a page to read past the work in it.
#[test]
fn a_v4_database_is_migrated_and_its_activity_classified() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("tasks");
    let agent_id;
    {
        let store = Store::new(&root).expect("store opens");
        let mut record = run_record("run-1", None, NOW);
        agent_id = record.agents[0].id.clone();
        record.agents[0].thread.post_user("said before", None, NOW);
        record.agents[0].thread.push_event(
            crate::thread::ThreadEventKind::ToolUse,
            Some("Read a file".to_string()),
            None,
            None,
            NOW,
        );
        record.agents[0].thread.push_event(
            crate::thread::ThreadEventKind::Done,
            Some("finished".to_string()),
            None,
            None,
            NOW,
        );
        store.save_run(&record).expect("the run saves");
        store.pretend_to_be_v4();
    }
    let migrated = Store::new(&root).expect("a v4 store opens");

    assert_eq!(
        stored_activity_sequences(&migrated, &agent_id),
        vec![2],
        "the backfill classified the items already stored"
    );
}

/// A stored page reaches back to the `limit`-th MESSAGE, the way a
/// resident one does: an outcome or a commit between two messages rides
/// beside them instead of spending the budget the page is measured in.
#[test]
fn a_stored_page_floor_is_measured_in_messages() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().join("tasks")).expect("store opens");
    let mut record = run_record("run-1", None, NOW);
    for turn in 0..6 {
        record.agents[0]
            .thread
            .post_user(format!("ask {turn}"), None, NOW);
        record.agents[0].thread.push_event(
            crate::thread::ThreadEventKind::Done,
            Some(format!("finished {turn}")),
            None,
            None,
            NOW,
        );
    }
    store.save_run(&record).expect("the conversation saves");

    let (page, has_more) = store
        .thread_conversation_page(&record.agents[0].id, None, 3)
        .expect("a page reads");
    let shipped = sequences(&page.items);
    assert_eq!(
        page.items
            .iter()
            .filter(|item| item.counts_toward_page())
            .count(),
        3,
        "the limit counts messages: {shipped:?}"
    );
    assert_eq!(
        shipped.len(),
        6,
        "each message's attention event rides with it: {shipped:?}"
    );
    assert!(has_more, "there is history below this page");
}

/// A stored page is measured the same way a resident one is: its limit
/// buys MESSAGES, the activity between two messages travels with them, and
/// `has_more` answers for items of any kind below what was shipped.
#[test]
fn a_stored_page_is_measured_in_messages() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().join("tasks")).expect("store opens");
    let mut record = run_record("run-1", None, NOW);
    for turn in 0..10 {
        record.agents[0]
            .thread
            .post_user(format!("ask {turn}"), None, NOW);
        for index in 0..5 {
            record.agents[0].thread.push_event(
                crate::thread::ThreadEventKind::ToolUse,
                Some(format!("Read file-{turn}-{index}.rs")),
                None,
                None,
                NOW,
            );
        }
    }
    store.save_run(&record).expect("the conversation saves");
    let agent_id = record.agents[0].id.clone();

    let (page, has_more) = store
        .thread_conversation_page(&agent_id, None, 3)
        .expect("a page reads");
    let shipped = sequences(&page.items);
    assert_eq!(
        page.items
            .iter()
            .filter(|item| item.counts_toward_page())
            .count(),
        3,
        "the limit counts messages: {shipped:?}"
    );
    assert!(
        shipped.len() > 3,
        "the activity between the messages rides with them: {shipped:?}"
    );
    assert_eq!(
        shipped,
        (*shipped.first().unwrap()..=60).collect::<Vec<u64>>(),
        "no run here reaches the cap, so this page is contiguous, oldest-first"
    );
    assert!(has_more, "there is history below this page");

    // Pages abut at their seeks: the walk sees every item exactly once.
    let mut walked = shipped;
    let mut before = walked.first().copied();
    loop {
        let (page, has_more) = store
            .thread_conversation_page(&agent_id, before, 3)
            .expect("a page reads");
        let shipped = sequences(&page.items);
        assert!(
            !shipped.is_empty(),
            "a page below {before:?} came back empty"
        );
        walked.splice(0..0, shipped);
        if !has_more {
            break;
        }
        before = walked.first().copied();
    }
    assert_eq!(walked, (1..=60).collect::<Vec<u64>>());
}

/// The cap holds in SQL too: an all-activity stretch ships its newest
/// hundred and the digest beside it counts the rest. The limit is the one
/// that buys a budget of exactly the cap, so the cap is what bounds it.
#[test]
fn a_stored_page_of_pure_activity_ships_its_newest_and_counts_them_all() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().join("tasks")).expect("store opens");
    let mut record = run_record("run-1", None, NOW);
    record.agents[0].thread.post_user("rename it", None, NOW);
    for index in 0..500 {
        record.agents[0].thread.push_event(
            crate::thread::ThreadEventKind::ToolUse,
            Some(format!("Read file-{index}.rs")),
            None,
            None,
            NOW,
        );
    }
    store.save_run(&record).expect("the conversation saves");

    let (cut, has_more) = store
        .thread_conversation_page(
            &record.agents[0].id,
            None,
            crate::thread::PAGE_ACTIVITY_RUN_CAP / crate::thread::PAGE_ACTIVITY_PER_MESSAGE,
        )
        .expect("a page reads");
    assert_eq!(cut.items.len(), 1 + crate::thread::PAGE_ACTIVITY_RUN_CAP);
    assert!(!has_more, "nothing sits below the page's oldest item");
    let digest = cut.digests.first().expect("the run has a digest");
    assert_eq!(digest.tool_calls, 500, "the omitted calls counted");
    assert_eq!(digest.rows, 500, "and counted as rows");
    assert_eq!(
        digest.last_tool_call.as_ref().map(|last| last.sequence),
        Some(501)
    );
}

/// The two page paths are one rule. The same conversation, read out of
/// memory and read back out of SQLite, ships the same items and the same
/// digests — the only difference between them is where the census counted.
///
/// The fixture's runs are all past the per-run cap and together past the
/// page budget, so both bounds are exercised on both sides of the seam.
#[test]
fn the_stored_page_and_the_resident_page_are_the_same_page() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().join("tasks")).expect("store opens");
    let mut record = run_record("run-1", None, NOW);
    for turn in 0..8 {
        record.agents[0]
            .thread
            .post_user(format!("ask {turn}"), None, NOW);
        for index in 0..crate::thread::PAGE_ACTIVITY_RUN_CAP + 20 {
            record.agents[0].thread.push_event(
                crate::thread::ThreadEventKind::ToolUse,
                Some(format!("Read file-{turn}-{index}.rs")),
                None,
                None,
                NOW,
            );
        }
        record.agents[0].thread.push_event(
            crate::thread::ThreadEventKind::Committed,
            Some(format!("committed {turn}")),
            None,
            None,
            NOW,
        );
    }
    store.save_run(&record).expect("the conversation saves");
    let thread = &record.agents[0].thread;

    let limit = 3;
    let (cut, has_more) = store
        .thread_conversation_page(&record.agents[0].id, None, limit)
        .expect("a page reads");
    let from_store = thread.wire_value_of_page(&cut, has_more);
    let from_memory = thread.wire_value_page(None, limit);

    assert_eq!(
        cut.items.iter().filter(|item| item.is_activity()).count(),
        crate::thread::page_activity_budget(limit),
        "the page spends its budget and stops"
    );

    assert_eq!(from_store["items"], from_memory["items"]);
    assert_eq!(
        from_store["activity_digests"],
        from_memory["activity_digests"]
    );
    assert_eq!(from_store["has_more"], from_memory["has_more"]);
    assert_eq!(
        from_store["activity_digests"]
            .as_array()
            .unwrap()
            .iter()
            .map(|digest| digest["tool_calls"].as_u64().unwrap())
            .collect::<Vec<u64>>(),
        vec![crate::thread::PAGE_ACTIVITY_RUN_CAP as u64 + 20; 3],
        "{from_store:?}"
    );
}

/// The cost of a page over a long run: the newest of it, and never the run.
///
/// Five thousand calls between two messages is the shape that made the old
/// read expensive — every row of the span was fetched and deserialized to
/// build a page that ships a hundred of them. The bounded read fetches what
/// it ships plus the run's oldest row, which is what the digest's
/// `from_sequence` names, so the decode count is the cap rather than the
/// run.
#[test]
fn a_page_over_a_long_run_decodes_its_cap_and_not_the_run() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().join("tasks")).expect("store opens");
    let mut record = run_record("run-1", None, NOW);
    record.agents[0]
        .thread
        .post_user("rewrite it all", None, NOW);
    for index in 0..5_000 {
        record.agents[0].thread.push_event(
            crate::thread::ThreadEventKind::ToolUse,
            Some(format!("Read file-{index}.rs")),
            None,
            None,
            NOW,
        );
    }
    store.save_run(&record).expect("the conversation saves");

    let before = items_decoded();
    let (cut, _) = store
        .thread_conversation_page(&record.agents[0].id, None, 20)
        .expect("a page reads");
    let decoded = items_decoded() - before;

    assert_eq!(
        cut.items.iter().filter(|item| item.is_activity()).count(),
        crate::thread::PAGE_ACTIVITY_RUN_CAP
    );
    let digest = cut.digests.first().expect("the run has a digest");
    assert_eq!(
        digest.tool_calls, 5_000,
        "the census still counts every call"
    );
    assert_eq!(digest.rows, 5_000, "and every row");
    assert!(
        decoded <= crate::thread::PAGE_ACTIVITY_RUN_CAP + 2,
        "the page decoded {decoded} rows: the cap, the run's oldest row and \
             the message it hangs under is all it may read"
    );
}

/// A folded run names the last call it made, however much thinking came
/// after it — the row the client prints beside the count.
///
/// The bounded read fetches the newest of a run, so a run that ends in a
/// hundred thoughts has its last call under what was fetched. Memory sees
/// the whole run and names it, so a stored page that did not go looking
/// for it would print a different row from the same conversation.
#[test]
fn a_stored_page_names_the_last_call_of_a_run_that_ends_in_thinking() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().join("tasks")).expect("store opens");
    let mut record = run_record("run-1", None, NOW);
    record.agents[0]
        .thread
        .post_user("think it through", None, NOW);
    record.agents[0].thread.push_event(
        crate::thread::ThreadEventKind::Reasoning,
        Some("where to start".to_string()),
        None,
        None,
        NOW,
    );
    record.agents[0].thread.push_event(
        crate::thread::ThreadEventKind::ToolUse,
        Some("Read lib.rs".to_string()),
        None,
        None,
        NOW,
    );
    for index in 0..150 {
        record.agents[0].thread.push_event(
            crate::thread::ThreadEventKind::Reasoning,
            Some(format!("thought {index}")),
            None,
            None,
            NOW,
        );
    }
    store.save_run(&record).expect("the conversation saves");
    let thread = &record.agents[0].thread;

    let limit = 1;
    let (cut, has_more) = store
        .thread_conversation_page(&record.agents[0].id, None, limit)
        .expect("a page reads");
    let from_store = thread.wire_value_of_page(&cut, has_more);
    let from_memory = thread.wire_value_page(None, limit);

    assert_eq!(
        cut.digests
            .first()
            .expect("the run has a digest")
            .last_tool_call
            .as_ref()
            .map(|last| last.sequence),
        Some(3),
        "the call under the thinking is the one the row prints"
    );
    assert_eq!(from_store["items"], from_memory["items"]);
    assert_eq!(
        from_store["activity_digests"],
        from_memory["activity_digests"]
    );
}

/// A v2 database gains the message column and is classified in place, the
/// way v1 gained attention. Nobody's stored conversation has to be
/// rewritten for the packet to read it.
#[test]
fn a_v2_database_is_migrated_and_its_messages_classified() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("tasks");
    let agent_id;
    {
        let store = Store::new(&root).expect("store opens");
        let mut record = run_record("run-1", None, NOW);
        agent_id = record.agents[0].id.clone();
        record.agents[0].thread.post_user("said before", None, NOW);
        record.agents[0].thread.push_event(
            crate::thread::ThreadEventKind::ToolUse,
            Some("Read a file".to_string()),
            None,
            None,
            NOW,
        );
        store.save_run(&record).expect("the run saves");
        store.pretend_to_be_v2();
    }
    let migrated = Store::new(&root).expect("a v2 store opens");

    assert_eq!(
        sequences(
            &migrated
                .thread_message_page(&agent_id, 40)
                .expect("the message page reads")
        ),
        vec![1],
        "the backfill classified the items already stored"
    );
}

/// A v6 database gains the hand-off column the same way, and the dismissal
/// line comes back right on the first boot that added it: a store written
/// before the column knew nothing about who signed a message.
#[test]
fn a_v6_database_is_migrated_and_its_hand_offs_classified() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("tasks");
    let own_sequence;
    {
        let store = Store::new(&root).expect("store opens");
        let mut record = run_record("run-1", None, NOW);
        let thread = &mut record.agents[0].thread;
        thread.post_agent("which name did you want?", None, NOW);
        own_sequence = thread.last_own_message_sequence();
        // Under the tail, so only the column can find it.
        for n in 0..=RESIDENT_CONVERSATION_TAIL {
            thread.post_user_from_agent(
                format!("step {n}"),
                crate::thread::AgentIdentity::new("project-1".to_string()),
                NOW,
            );
        }
        store.save_run(&record).expect("the run saves");
        store.pretend_to_be_v6();
    }

    let migrated = Store::new(&root).expect("a v6 store opens");

    let loaded = reload_run(&migrated, "run-1");
    assert_eq!(
        loaded.agents[0].thread.last_own_message_sequence(),
        own_sequence,
        "the backfill classified the hand-offs already stored"
    );
}

/// A capture record that will not parse fails the boot that read it. The
/// text is the one thing the user cannot re-derive, so dropping it quietly
/// is the one thing the store must never do.
#[test]
fn an_unreadable_capture_record_fails_fast() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().join("tasks")).expect("store opens");
    store.corrupt_capture_row("capture-1");
    assert!(matches!(
        store.load_all_captures(),
        Err(StoreError::Corrupt { .. })
    ));
}

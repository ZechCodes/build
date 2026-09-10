use super::*;

const NOW: &str = "2026-09-06T18:03:11.412Z";

/// The cut as a default page takes it: the whole conversation newest-first,
/// under the budget [`DEFAULT_THREAD_PAGE`] buys, with a census that counts
/// the rows and tool calls of a span exactly.
fn cut_over(thread: &Thread) -> PageCut<&ThreadItem> {
    let span: Vec<&ThreadItem> = thread.items.iter().rev().collect();
    let census = |from: u64, through: u64| {
        Ok::<RunCensus, std::convert::Infallible>(thread.run_census(from, through))
    };
    match cut_activity_runs(span, DEFAULT_THREAD_PAGE, census) {
        Ok(cut) => cut,
        Err(impossible) => match impossible {},
    }
}

/// The memory census itself: how many rows and how many tool calls a span
/// of the resident tail holds, inclusive at both ends. What a digest's
/// counts are when a page is answered out of memory, and the sibling of
/// the SQL counts the store answers with.
#[test]
fn the_memory_census_counts_the_rows_and_calls_of_a_span_inclusively() {
    let mut thread = Thread::new("run-census");
    thread.post_user("rename the helper", None, NOW);
    let first = call(&mut thread, "Read one.rs");
    thread.push_event(ThreadEventKind::Reasoning, None, None, None, NOW);
    let last = call(&mut thread, "Read two.rs");
    thread.post_agent("renamed it", None, NOW);

    assert_eq!(
        thread.run_census(first, last),
        RunCensus {
            tool_calls: 2,
            rows: 3
        },
        "both ends count, and the thought between them is a row"
    );
    assert_eq!(
        thread.run_census(first, first),
        RunCensus {
            tool_calls: 1,
            rows: 1
        },
        "one item, one call, one row"
    );
    assert_eq!(
        thread.run_census(1, thread.last_sequence()),
        RunCensus {
            tool_calls: 2,
            rows: 3
        },
        "the words are neither a call nor a row of work"
    );
    assert_eq!(thread.run_census(last + 1, u64::MAX), RunCensus::default());
}

fn call(thread: &mut Thread, summary: &str) -> u64 {
    thread.push_event(
        ThreadEventKind::ToolUse,
        Some(summary.to_string()),
        None,
        None,
        NOW,
    )
}

/// The shape the fold exists for. A thousand calls ship as a hundred, and
/// the digest carries the truth about the rest: how many there were, and
/// which one was last.
#[test]
fn a_run_of_a_thousand_calls_ships_its_newest_hundred_and_counts_them_all() {
    let mut thread = Thread::new("run-busy");
    thread.post_user("rename the helper", None, NOW);
    for index in 0..1000 {
        call(&mut thread, &format!("Read file-{index}.rs"));
    }

    let cut = cut_over(&thread);
    let shipped: Vec<u64> = cut.items.iter().map(|item| item.sequence()).collect();
    assert_eq!(
        shipped.len(),
        1 + PAGE_ACTIVITY_RUN_CAP,
        "{}",
        shipped.len()
    );
    assert_eq!(
        shipped[0], 1,
        "the message is not activity and always ships"
    );
    assert_eq!(
        &shipped[1..],
        (902..=1001).collect::<Vec<u64>>(),
        "the newest of the run, oldest-first"
    );

    let digest = cut.digests.first().expect("the run has a digest");
    assert_eq!(digest.from_sequence, 2);
    assert_eq!(digest.through_sequence, 1001);
    assert_eq!(digest.tool_calls, 1000, "exact over the whole run");
    assert_eq!(digest.rows, 1000);
    let last = digest
        .last_tool_call
        .as_ref()
        .expect("the run called tools");
    assert_eq!(last.sequence, 1001);
    assert_eq!(last.summary.as_deref(), Some("Read file-999.rs"));
    assert_eq!(last.created_at, NOW);
    assert!(last.outcome.is_none(), "{last:?}");
}

/// A run that only thought made no call and names none, but every thought
/// is a row: the fold it collapses to still says how much is inside it.
#[test]
fn a_run_of_pure_reasoning_counts_its_rows_and_names_no_call() {
    let mut thread = Thread::new("run-quiet");
    thread.post_user("what do you make of it", None, NOW);
    for _ in 0..5 {
        thread.push_event(
            ThreadEventKind::Reasoning,
            Some("thinking".to_string()),
            None,
            None,
            NOW,
        );
    }

    let cut = cut_over(&thread);
    let digest = cut.digests.first().expect("the run has a digest");
    assert_eq!(digest.tool_calls, 0);
    assert_eq!(digest.rows, 5, "five thoughts are five rows");
    assert!(digest.last_tool_call.is_none(), "{digest:?}");
    assert_eq!(cut.items.len(), 6, "nothing was capped");
}

/// The newest call, wherever in the run it sits: the cap keeps the newest
/// ITEMS, and a run that thought for a hundred steps after its last call
/// still says which call that was.
#[test]
fn the_last_call_is_named_even_when_the_cap_left_it_off_the_wire() {
    let mut thread = Thread::new("run-thinky");
    thread.post_user("rename the helper", None, NOW);
    let called = call(&mut thread, "Bash(cargo test)");
    for _ in 0..PAGE_ACTIVITY_RUN_CAP + 20 {
        thread.push_event(ThreadEventKind::Reasoning, None, None, None, NOW);
    }

    let cut = cut_over(&thread);
    let shipped: Vec<u64> = cut.items.iter().map(|item| item.sequence()).collect();
    assert!(!shipped.contains(&called), "the cap left the call off");
    let digest = cut.digests.first().expect("the run has a digest");
    assert_eq!(digest.tool_calls, 1);
    assert_eq!(
        digest.rows,
        1 + PAGE_ACTIVITY_RUN_CAP as u64 + 20,
        "the rows the cap left off the wire are counted all the same"
    );
    assert_eq!(
        digest.last_tool_call.as_ref().map(|last| last.sequence),
        Some(called)
    );
}

/// A run is maximal, and anything that is not activity ends one: a message
/// or a lifecycle marker both do.
#[test]
fn every_non_activity_item_ends_a_run() {
    let mut thread = Thread::new("run-mixed");
    thread.post_user("rename the helper", None, NOW);
    call(&mut thread, "Read one.rs");
    call(&mut thread, "Read two.rs");
    thread.push_event(ThreadEventKind::Committed, None, None, None, NOW);
    call(&mut thread, "Read three.rs");
    thread.post_agent("done", None, NOW);
    thread.push_event(ThreadEventKind::Narration, None, None, None, NOW);

    let cut = cut_over(&thread);
    let runs: Vec<(u64, u64, u64, u64)> = cut
        .digests
        .iter()
        .map(|digest| {
            (
                digest.from_sequence,
                digest.through_sequence,
                digest.tool_calls,
                digest.rows,
            )
        })
        .collect();
    assert_eq!(
        runs,
        vec![(2, 3, 2, 2), (5, 5, 1, 1), (7, 7, 0, 1)],
        "{runs:?}"
    );
    assert_eq!(
        cut.items
            .iter()
            .map(|item| item.sequence())
            .collect::<Vec<u64>>(),
        (1..=7).collect::<Vec<u64>>(),
        "nothing was capped, so the page is whole"
    );
}

/// The open run at the end of a conversation reaches the newest thing
/// there is, so the client knows the digest speaks for everything it holds
/// below it.
#[test]
fn the_open_tail_runs_through_sequence_is_the_last_sequence() {
    let mut thread = Thread::new("run-live");
    thread.post_user("rename the helper", None, NOW);
    for index in 0..300 {
        call(&mut thread, &format!("Read file-{index}.rs"));
    }

    let cut = cut_over(&thread);
    assert_eq!(
        cut.digests.last().expect("a run").through_sequence,
        thread.last_sequence()
    );
}

/// A conversation with no activity in it cuts to itself: every item ships,
/// oldest-first, and there is nothing to fold.
#[test]
fn a_conversation_of_words_alone_cuts_to_itself() {
    let mut thread = Thread::new("run-talky");
    for turn in 0..4 {
        thread.post_user(format!("ask {turn}"), None, NOW);
        thread.post_agent(format!("answer {turn}"), None, NOW);
    }

    let cut = cut_over(&thread);
    assert!(cut.digests.is_empty(), "{:?}", cut.digests);
    assert_eq!(
        cut.items
            .iter()
            .map(|item| item.sequence())
            .collect::<Vec<u64>>(),
        (1..=8).collect::<Vec<u64>>()
    );
}

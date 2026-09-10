use super::*;

const NOW: &str = "2026-08-29T09:00:00Z";

/// A working session as the thread records it: each message followed by
/// the activity the agent emitted after it.
fn conversation_with_activity(turns: usize, activity_per_turn: usize) -> Thread {
    let mut thread = Thread::new("run-busy");
    for turn in 0..turns {
        thread.post_user(format!("ask {turn}"), None, NOW);
        for index in 0..activity_per_turn {
            thread.push_event(
                ThreadEventKind::ToolUse,
                Some(format!("Read file-{turn}-{index}.rs")),
                None,
                None,
                NOW,
            );
        }
    }
    thread
}

fn page_items(page: &Value) -> Vec<u64> {
    page["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["data"]["sequence"].as_u64().unwrap())
        .collect()
}

fn counted_in_page(page: &Value) -> usize {
    page["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|item| item["type"] == "message")
        .count()
}

/// The limit buys conversation. A page opened on a session that emitted
/// twenty tool calls per turn carries its five messages and the activity
/// between them, rather than five tool calls and nothing said.
#[test]
fn a_pages_limit_buys_conversation_and_activity_rides_beside_it() {
    let thread = conversation_with_activity(10, 5);
    let page = thread.wire_value_page(None, 5);

    assert_eq!(counted_in_page(&page), 5, "the limit counts messages");
    let shipped = page_items(&page);
    assert!(
        shipped.len() > 5,
        "the activity between the messages travels with them: {shipped:?}"
    );
    // One contiguous run of sequences, ending at the newest item.
    assert_eq!(
        shipped,
        (shipped[0]..=thread.last_sequence()).collect::<Vec<u64>>()
    );
    assert_eq!(page["oldest_sequence"], shipped[0], "{page:?}");
    assert_eq!(page["has_more"], true);
    assert_eq!(page["thread_total"], 60, "the total counts every item");
}

/// A page's limit buys MESSAGES, and nothing else on a conversation spends
/// it. An outcome, a block, a commit — everything Build records about the
/// work — rides beside the words it belongs to, so the page a reviewer
/// opens on is always the last twenty things anybody said.
#[test]
fn a_pages_limit_buys_messages_and_the_lifecycle_rides_beside_them() {
    let mut thread = Thread::new("run-outcomes");
    for turn in 0..6 {
        thread.post_user(format!("ask {turn}"), None, NOW);
        thread.push_event(
            ThreadEventKind::Done,
            Some(format!("finished {turn}")),
            None,
            None,
            NOW,
        );
    }

    let page = thread.wire_value_page(None, 3);
    let shipped = page_items(&page);
    assert_eq!(counted_in_page(&page), 3, "the limit counts messages");
    assert_eq!(
        shipped.len(),
        6,
        "each message's attention event rides with it: {shipped:?}"
    );
}

/// The page stops AT the limit-th counted item: activity older than it is
/// the next page's, so a page is never padded with work nobody asked for.
#[test]
fn a_page_ends_on_its_oldest_message_not_on_the_activity_under_it() {
    let thread = conversation_with_activity(4, 3);
    let page = thread.wire_value_page(None, 2);

    let shipped = page_items(&page);
    let oldest = shipped[0];
    assert!(
        matches!(
            thread.items.iter().find(|item| item.sequence() == oldest),
            Some(ThreadItem::Message(_))
        ),
        "the page opens on a message: {shipped:?}"
    );
}

/// The cap. An all-activity stretch cannot make a page unbounded: the run
/// ships its newest hundred and the digest beside it says how many there
/// really were, so the reviewer is told a thousand without being sent one.
///
/// The limit is the one that buys a budget of exactly the cap, so what
/// bounds this page is the cap and nothing else.
#[test]
fn an_all_activity_stretch_is_bounded_by_the_run_cap() {
    let mut thread = Thread::new("run-busy");
    thread.post_user("please rename the helper", None, NOW);
    for index in 0..1000 {
        thread.push_event(
            ThreadEventKind::ToolUse,
            Some(format!("Read file-{index}.rs")),
            None,
            None,
            NOW,
        );
    }

    let page = thread.wire_value_page(None, PAGE_ACTIVITY_RUN_CAP / PAGE_ACTIVITY_PER_MESSAGE);
    let shipped = page_items(&page);
    assert_eq!(
        shipped.len(),
        1 + PAGE_ACTIVITY_RUN_CAP,
        "{}",
        shipped.len()
    );
    assert_eq!(counted_in_page(&page), 1, "the one thing said is on it");
    assert_eq!(
        page["has_more"], false,
        "nothing sits below the page's oldest item"
    );
    assert_eq!(page["oldest_sequence"], shipped[0]);

    let digests = page["activity_digests"].as_array().unwrap();
    assert_eq!(digests.len(), 1, "{digests:?}");
    assert_eq!(digests[0]["from_sequence"], 2);
    assert_eq!(digests[0]["through_sequence"], thread.last_sequence());
    assert_eq!(digests[0]["tool_calls"], 1000, "the omitted calls counted");
    assert_eq!(digests[0]["rows"], 1000, "a run of calls is as many rows");
    assert_eq!(digests[0]["last_tool_call"]["sequence"], 1001);
    assert_eq!(
        digests[0]["last_tool_call"]["summary"], "Read file-999.rs",
        "{digests:?}"
    );
}

/// The smallest poll there is. The branch surface and the console ask for
/// one message a tick and read none of the work between them, so a page of
/// 1 over an open run of a thousand calls ships the message, ten rows of
/// activity, and a digest that says a thousand.
#[test]
fn a_page_of_one_message_ships_ten_activity_items_and_counts_the_rest() {
    let mut thread = Thread::new("run-busy");
    thread.post_user("please rename the helper", None, NOW);
    for index in 0..1000 {
        thread.push_event(
            ThreadEventKind::ToolUse,
            Some(format!("Read file-{index}.rs")),
            None,
            None,
            NOW,
        );
    }

    let page = thread.wire_value_page(None, 1);
    let shipped = page_items(&page);
    assert_eq!(counted_in_page(&page), 1, "the one thing said is on it");
    assert_eq!(
        shipped.len(),
        1 + page_activity_budget(1),
        "a page of one buys ten items of work: {}",
        shipped.len()
    );

    let digests = page["activity_digests"].as_array().unwrap();
    assert_eq!(digests.len(), 1, "{digests:?}");
    assert_eq!(
        digests[0]["tool_calls"], 1000,
        "the digest is exact whatever the page shipped"
    );
    assert_eq!(digests[0]["last_tool_call"]["sequence"], 1001);
}

/// The whole-page budget. The per-run cap bounds ONE run, and twenty
/// capped runs on one page is still two thousand rows — so a page spends a
/// budget across all of its runs, newest first. The newest runs are whole,
/// the oldest shrink to their digest, and every digest still counts its
/// whole run.
#[test]
fn a_page_of_many_runs_spends_its_budget_newest_first() {
    let mut thread = Thread::new("run-long");
    for turn in 0..DEFAULT_THREAD_PAGE {
        thread.post_user(format!("ask {turn}"), None, NOW);
        for index in 0..PAGE_ACTIVITY_RUN_CAP {
            thread.push_event(
                ThreadEventKind::ToolUse,
                Some(format!("Read file-{turn}-{index}.rs")),
                None,
                None,
                NOW,
            );
        }
    }

    let budget = page_activity_budget(DEFAULT_THREAD_PAGE);
    let page = thread.wire_value_page(None, DEFAULT_THREAD_PAGE);
    let shipped = page_items(&page);
    assert_eq!(
        counted_in_page(&page),
        DEFAULT_THREAD_PAGE,
        "the limit still buys every message it asked for"
    );
    assert_eq!(
        shipped.len(),
        DEFAULT_THREAD_PAGE + budget,
        "and the work beside them is the budget, no more: {}",
        shipped.len()
    );

    let digests = page["activity_digests"].as_array().unwrap();
    assert_eq!(
        digests.len(),
        DEFAULT_THREAD_PAGE,
        "one per run, whatever the run shipped: {}",
        digests.len()
    );
    assert!(
        digests
            .iter()
            .all(|digest| digest["tool_calls"] == PAGE_ACTIVITY_RUN_CAP),
        "every digest is exact over its whole run: {digests:?}"
    );
    let shipped_per_run: Vec<usize> = digests
        .iter()
        .map(|digest| {
            let span = digest["from_sequence"].as_u64().unwrap()
                ..=digest["through_sequence"].as_u64().unwrap();
            shipped.iter().filter(|item| span.contains(item)).count()
        })
        .collect();
    assert_eq!(
        shipped_per_run
            .iter()
            .rev()
            .take(2)
            .copied()
            .collect::<Vec<usize>>(),
        vec![PAGE_ACTIVITY_RUN_CAP; 2],
        "the newest runs are whole: {shipped_per_run:?}"
    );
    assert!(
        shipped_per_run[..DEFAULT_THREAD_PAGE - 2]
            .iter()
            .all(|count| *count == 0),
        "the older ones are their digest and nothing else: {shipped_per_run:?}"
    );
}

/// Every page carries its runs' digests; a forward delta carries none —
/// a delta says what arrived, and what arrived is what the client holds.
#[test]
fn a_page_carries_activity_digests_and_a_delta_carries_none() {
    let thread = conversation_with_activity(4, 3);

    let page = thread.wire_value_page(None, 2);
    let digests = page["activity_digests"].as_array().unwrap();
    assert_eq!(digests.len(), 2, "one per run on the page: {digests:?}");
    assert!(
        digests
            .iter()
            .all(|digest| digest["tool_calls"] == 3 && digest["last_tool_call"].is_object()),
        "{digests:?}"
    );

    let delta = thread.wire_value_after(0);
    assert!(delta.get("activity_digests").is_none(), "{delta:?}");
}

/// A run of nothing but thinking folds to a row with nothing to claim, and
/// the fixed shape says so rather than leaving the field out.
#[test]
fn a_run_without_tool_calls_ships_a_null_last_call() {
    let mut thread = Thread::new("run-quiet");
    thread.post_user("what do you make of it", None, NOW);
    for _ in 0..4 {
        thread.push_event(ThreadEventKind::Reasoning, None, None, None, NOW);
    }

    let page = thread.wire_value_page(None, 5);
    let digests = page["activity_digests"].as_array().unwrap();
    assert_eq!(digests[0]["tool_calls"], 0, "{digests:?}");
    assert!(digests[0]["last_tool_call"].is_null(), "{digests:?}");
}

/// The shape on the wire, whole. A client reads these five names and no
/// others, and a digest always carries `last_tool_call` — the object when
/// the run made a call, `null` when it made none — so nothing has to read
/// around a field that is sometimes absent.
#[test]
fn an_activity_digest_ships_a_fixed_shape() {
    let mut thread = Thread::new("run-shape");
    thread.post_user("rename the helper", None, NOW);
    thread.push_event(ThreadEventKind::Reasoning, None, None, None, NOW);
    let call = thread.push_event(
        ThreadEventKind::ToolUse,
        Some("Bash(cargo test)".to_string()),
        None,
        None,
        NOW,
    );
    assert!(thread.resolve_tool_call(call, ToolCallOutcome::Ok, "1735 passed"));

    let page = thread.wire_value_page(None, 5);
    assert_eq!(
        page["activity_digests"][0],
        json!({
            "from_sequence": 2,
            "through_sequence": 3,
            "tool_calls": 1,
            "rows": 2,
            "last_tool_call": {
                "sequence": 3,
                "created_at": NOW,
                "summary": "Bash(cargo test)\n\u{2192} 1735 passed",
                "outcome": "ok"
            }
        }),
        "{page:?}"
    );
}

/// The run still open at the end of a conversation is digested through the
/// thread's last sequence, not through some item inside it: a client
/// holding only the page can tell exactly which arrivals the digest has
/// already counted and which it must add itself.
#[test]
fn the_open_tail_runs_digest_reaches_the_threads_last_sequence() {
    let thread = conversation_with_activity(6, 4);

    let page = thread.wire_value_page(None, 2);
    let digests = page["activity_digests"].as_array().unwrap();
    let tail = digests.last().expect("the open run has a digest");
    assert_eq!(tail["through_sequence"], thread.last_sequence(), "{tail:?}");
    assert_eq!(tail["tool_calls"], 4, "{tail:?}");
    assert_eq!(
        tail["last_tool_call"]["sequence"],
        thread.last_sequence(),
        "{tail:?}"
    );
}

/// What a sequence-paging client relies on: pages abut at their seeks, so
/// walking `before_sequence = oldest_sequence` sees every item exactly
/// once and skips none — activity included.
#[test]
fn paging_backward_over_an_activity_heavy_thread_sees_every_item_once() {
    let thread = conversation_with_activity(9, 7);

    let mut walked: Vec<u64> = Vec::new();
    let mut before = None;
    loop {
        let page = thread.wire_value_page(before, 2);
        let shipped = page_items(&page);
        assert!(!shipped.is_empty(), "{page:?}");
        walked.splice(0..0, shipped.clone());
        if !page["has_more"].as_bool().unwrap() {
            break;
        }
        before = Some(page["oldest_sequence"].as_u64().unwrap());
    }

    assert_eq!(walked, (1..=thread.last_sequence()).collect::<Vec<u64>>());
}

/// The gate that sends a page to the store is measured in messages too: a
/// tail holding only activity cannot answer a page, however many items it
/// holds, and a tail holding the page's worth of words answers it whatever
/// else is under it.
#[test]
fn the_stored_page_gate_is_measured_in_messages() {
    let mut whole = Thread::new("run-busy");
    whole.post_user("please rename the helper", None, NOW);
    for index in 0..300 {
        whole.push_event(
            ThreadEventKind::ToolUse,
            Some(format!("Read file-{index}.rs")),
            None,
            None,
            NOW,
        );
    }
    let stored = whole.items.clone();

    let mut starved = Thread::new("run-busy");
    starved.adopt_stored_tail(
        stored[stored.len() - 20..].to_vec(),
        (stored.len() - 20) as u64,
        whole.last_sequence(),
    );
    assert!(
        starved.page_reaches_stored_history(None, 5),
        "a tail of pure activity holds no page of conversation"
    );
    assert!(
        starved.page_reaches_stored_history(None, 1),
        "not even one message: the words are under the tail"
    );

    whole.post_agent("renamed it", None, NOW);
    let spoken = whole.items.clone();
    let mut fed = Thread::new("run-busy");
    fed.adopt_stored_tail(
        spoken[spoken.len() - 20..].to_vec(),
        (spoken.len() - 20) as u64,
        whole.last_sequence(),
    );
    assert!(
        !fed.page_reaches_stored_history(None, 1),
        "a tail holding the page's words answers it from memory"
    );
}

use super::*;

// ---- paged conversation reads -----------------------------------------

/// A run whose conversation is several pages long, so what a poll ships can
/// be told apart from what the conversation holds.
pub(in crate::app::tests) fn run_with_long_conversation(
    state: &mut AppState,
    run_id: &str,
    turns: usize,
) -> usize {
    let mut active =
        crate::orchestrator::ActiveRun::reattach(&fake_run_record(run_id), ".build/plan.md".into());
    for turn in 0..turns {
        primary_thread_mut(&mut active.agents).post_user(
            format!("turn {turn}"),
            None,
            now_rfc3339(),
        );
    }
    let held = primary_thread(&active.agents).items.len();
    let project_id = state.project_at(0).id.clone();
    state.projects.bind_entity(run_id.into(), project_id);
    state.runs.insert(run_id.into(), active);
    held
}

/// The sequences a page shipped, in the order it shipped them.
fn page_sequences(thread: &Value) -> Vec<u64> {
    thread["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["data"]["sequence"].as_u64().unwrap())
        .collect()
}

#[test]
fn a_detail_poll_that_names_no_page_ships_the_conversation_whole() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let held = run_with_long_conversation(&mut state, "run-unbounded", 250);

    let opened = state.handle(req("run.get", json!({ "run_id": "run-unbounded" })));
    let thread = &opened["result"]["thread"];
    assert_eq!(
        thread["items"].as_array().unwrap().len(),
        held,
        "{thread:?}"
    );

    // The silence is the whole point: a client written before paging holds
    // the conversation entire and reconciles every later delta against
    // `thread_total`. An answer that names one has told it a count it can
    // never match, so it drops its cache every tick, never sends a cursor
    // again, and never sees a word above the window it was handed.
    assert!(thread.get("thread_total").is_none(), "{thread:?}");
    assert!(thread.get("has_more").is_none(), "{thread:?}");

    // And the delta after it reconciles: the count the daemon names is the
    // count the client now holds, so the cache survives and the next poll
    // is a cursored one — the bandwidth win the cursor exists for, still
    // reachable by a client that cannot page.
    let last_sequence = thread["items"].as_array().unwrap().last().unwrap()["data"]["sequence"]
        .as_u64()
        .unwrap();
    let delta = state.handle(req(
        "run.get",
        json!({ "run_id": "run-unbounded", "thread_after_sequence": last_sequence }),
    ));
    let delta_thread = &delta["result"]["thread"];
    assert!(
        delta_thread["items"].as_array().unwrap().is_empty(),
        "{delta_thread:?}"
    );
    assert_eq!(
        delta_thread["thread_total"], held as u64,
        "{delta_thread:?}"
    );
}

#[test]
fn a_detail_poll_cannot_ask_for_more_conversation_than_a_page_carries() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    run_with_long_conversation(&mut state, "run-greedy", 250);

    let greedy = state.handle(req(
        "run.get",
        json!({ "run_id": "run-greedy", "thread_limit": 10_000 }),
    ));
    let thread = &greedy["result"]["thread"];
    assert_eq!(
        thread["items"].as_array().unwrap().len(),
        crate::thread::MAX_THREAD_PAGE,
        "{thread:?}"
    );
}

/// Silence means whole, so a limit that cannot be read must not read as
/// silence: a client that asked for a page and got the conversation entire
/// has no `has_more` to tell it the answer was not the one it asked for,
/// and pays the whole cost of it on every first load.
#[test]
fn a_detail_poll_whose_limit_is_not_a_plain_integer_still_gets_a_page() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let held = run_with_long_conversation(&mut state, "run-odd-limit", 250);

    let page_of = |state: &mut AppState, limit: Value| {
        let answer = state.handle(req(
            "run.get",
            json!({ "run_id": "run-odd-limit", "thread_limit": limit }),
        ));
        answer["result"]["thread"].clone()
    };

    // A number that went through a URL or an encoder without an integer
    // type still names the page its client typed.
    for spelling in [json!("20"), json!(20.0)] {
        let thread = page_of(&mut state, spelling.clone());
        assert_eq!(
            thread["items"].as_array().unwrap().len(),
            crate::thread::DEFAULT_THREAD_PAGE,
            "{spelling} -> {thread:?}"
        );
        assert_eq!(thread["has_more"], true, "{spelling} -> {thread:?}");
    }

    // A limit nobody can read is still a client saying it can page, so it
    // gets one — the default's worth — rather than the unbounded answer.
    for nonsense in [json!(-1), json!("twenty"), json!(true), json!([20])] {
        let thread = page_of(&mut state, nonsense.clone());
        assert_eq!(
            thread["items"].as_array().unwrap().len(),
            crate::thread::DEFAULT_THREAD_PAGE,
            "{nonsense} -> {thread:?}"
        );
        assert_eq!(thread["has_more"], true, "{nonsense} -> {thread:?}");
    }

    // `null` is how a client spells a field it is not sending, so it keeps
    // meaning what leaving the field out means.
    let unspoken = page_of(&mut state, Value::Null);
    assert_eq!(
        unspoken["items"].as_array().unwrap().len(),
        held,
        "{unspoken:?}"
    );
    assert!(unspoken.get("has_more").is_none(), "{unspoken:?}");
}

#[test]
fn a_detail_poll_that_asks_for_a_page_of_a_long_conversation_gets_one_not_all_of_it() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let held = run_with_long_conversation(&mut state, "run-long", 250);

    let opened = state.handle(req(
        "run.get",
        json!({
            "run_id": "run-long",
            "thread_limit": crate::thread::DEFAULT_THREAD_PAGE,
        }),
    ));
    let thread = &opened["result"]["thread"];
    let items = thread["items"].as_array().unwrap();
    assert_eq!(
        items.len(),
        crate::thread::DEFAULT_THREAD_PAGE,
        "{thread:?}"
    );

    // Bounded, but still honest about the conversation behind it: the
    // fields a client already reads mean exactly what they did.
    assert_eq!(thread["thread_total"], held as u64, "{thread:?}");
    assert_eq!(
        thread["thread_last_sequence"].as_u64().unwrap(),
        *page_sequences(thread).last().unwrap()
    );
    assert_eq!(thread["has_more"], true, "{thread:?}");
    assert_eq!(
        thread["oldest_sequence"].as_u64().unwrap(),
        page_sequences(thread)[0]
    );

    // The page is the tail — the work you were doing, not the first hour.
    assert_eq!(items.last().unwrap()["data"]["body"], "turn 249");
    assert!(
        !thread["items"].to_string().contains("turn 0\""),
        "{thread:?}"
    );
}

/// A restart drops the history under the tail; the tab watching it does
/// not drop its cursor. An item mutated in place before the restart — a
/// message the agent marked seen — is news no creation sequence expresses,
/// so a delta answered only out of the tail leaves that tab rendering a
/// read message unread for as long as it stays open.
#[test]
fn a_cursored_poll_after_a_restart_reships_a_mutation_under_the_tail() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    run_with_long_conversation(&mut state, "run-restart-delta", 250);
    // Where the open tab's cursor sits: it holds every item said so far.
    let cursor = primary_thread(&state.runs["run-restart-delta"].agents).last_sequence();

    // The agent reads the mailbox, which bumps the `updated_sequence` of
    // every message in it — including the ones at the very start of the
    // conversation, which the restart is about to leave in the store.
    state
        .on_mcp_action("run-restart-delta", BridgeAction::ReadUnreadMessages)
        .unwrap();
    let active = state
        .runs
        .remove("run-restart-delta")
        .expect("the run is there");
    let total = primary_thread(&active.agents).items.len();
    state
        .persist_run_record("run-restart-delta", &active)
        .expect("the run saves");

    let mut restarted = qa_state(&repo, dir.path());
    assert!(
        primary_thread(&restarted.runs["run-restart-delta"].agents)
            .items
            .len()
            < total,
        "the restart loaded the conversation whole, so the delta proves nothing"
    );

    let delta = restarted.handle(req(
        "run.get",
        json!({
            "run_id": "run-restart-delta",
            "thread_after_sequence": cursor,
        }),
    ));
    let thread = &delta["result"]["thread"];
    let items = thread["items"].as_array().unwrap();
    assert!(
        items
            .iter()
            .any(|item| item["data"]["body"] == "turn 0" && item["data"]["seen_at"].is_string()),
        "the mutation under the tail never reached the cursor: {thread:?}"
    );
    assert_eq!(thread["thread_total"], total as u64, "{thread:?}");

    // And it drains: the high-water mark it names is past the mutations it
    // just shipped, so the tab asks once and stops asking.
    let advanced = thread["thread_last_sequence"].as_u64().unwrap();
    let drained = restarted.handle(req(
        "run.get",
        json!({
            "run_id": "run-restart-delta",
            "thread_after_sequence": advanced,
        }),
    ));
    assert!(
        drained["result"]["thread"]["items"]
            .as_array()
            .unwrap()
            .is_empty(),
        "{drained:?}"
    );
}

/// The write path is the hot one: a reviewer types one word into a
/// conversation of hundreds, and the answer to that post is a whole view of
/// the run. It has to obey the same bound the detail polls do — a client
/// that named a `thread_limit` gets that page back, not every item it
/// already holds, serialized and encrypted for it to drop on the floor.
#[test]
fn posting_a_message_answers_with_the_page_the_client_asked_for() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let held = run_with_long_conversation(&mut state, "run-post-page", 250);

    let posted = state.handle(req(
        "thread.post",
        json!({
            "entity_id": "run-post-page",
            "body": "one more word",
            "thread_limit": crate::thread::DEFAULT_THREAD_PAGE,
        }),
    ));
    assert_eq!(posted["ok"], true, "{posted:?}");
    let thread = &posted["result"]["thread"];
    assert_eq!(
        thread["items"].as_array().unwrap().len(),
        crate::thread::DEFAULT_THREAD_PAGE,
        "{thread:?}"
    );

    // Bounded, and honest about what sits behind the window: the word just
    // said is the newest item, and the count names the whole conversation.
    assert_eq!(thread["thread_total"], held as u64 + 1, "{thread:?}");
    assert_eq!(
        thread["items"].as_array().unwrap().last().unwrap()["data"]["body"],
        "one more word"
    );
    assert!(
        !thread["items"].to_string().contains("turn 0\""),
        "{thread:?}"
    );
}

#[test]
fn thread_post_names_the_sequence_it_appended() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    run_with_long_conversation(&mut state, "run-post-named", 3);

    let posted = state.handle(req(
        "thread.post",
        json!({ "entity_id": "run-post-named", "body": "one more word" }),
    ));
    assert_eq!(posted["ok"], true, "{posted:?}");
    let items = posted["result"]["thread"]["items"].as_array().unwrap();
    let appended = items.last().unwrap();
    assert_eq!(appended["data"]["body"], "one more word");
    assert_eq!(
        posted["result"]["posted_sequence"], appended["data"]["sequence"],
        "{posted:?}"
    );

    let again = state.handle(req(
        "thread.post",
        json!({ "entity_id": "run-post-named", "body": "and another" }),
    ));
    assert_eq!(again["ok"], true, "{again:?}");
    assert!(
        again["result"]["posted_sequence"].as_u64().unwrap()
            > posted["result"]["posted_sequence"].as_u64().unwrap(),
        "{again:?}"
    );
}

#[test]
fn posting_to_an_implementation_names_the_sequence_in_its_issues_conversation() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (_issue_id, run_id) = planned_run_in_review(&mut state, "name the routed post");

    let posted = state.handle(req(
        "thread.post",
        json!({ "entity_id": run_id, "body": "one more word" }),
    ));
    assert_eq!(posted["ok"], true, "{posted:?}");
    let items = posted["result"]["thread"]["items"].as_array().unwrap();
    let appended = items
        .iter()
        .rev()
        .find(|item| item["data"]["body"] == "one more word")
        .unwrap();
    assert_eq!(
        posted["result"]["posted_sequence"], appended["data"]["sequence"],
        "{posted:?}"
    );
}

/// And the same silence rule as the reads: a client that named no page is
/// one that cannot page, so posting still answers it with the conversation
/// entire.
#[test]
fn posting_a_message_without_naming_a_page_still_answers_with_the_whole_conversation() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let held = run_with_long_conversation(&mut state, "run-post-whole", 250);

    let posted = state.handle(req(
        "thread.post",
        json!({ "entity_id": "run-post-whole", "body": "one more word" }),
    ));
    assert_eq!(posted["ok"], true, "{posted:?}");
    let thread = &posted["result"]["thread"];
    assert_eq!(
        thread["items"].as_array().unwrap().len(),
        held + 1,
        "{thread:?}"
    );
    assert!(thread.get("thread_total").is_none(), "{thread:?}");
}

/// The routed post: an implementation's first agent speaks in its Issue's
/// conversation, so the branch view that answers the post carries the
/// Issue's items. That answer is bounded by the same limit.
#[test]
fn posting_to_an_implementation_answers_with_a_page_of_its_conversation() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (_, run_id) = planned_run_in_review(&mut state, "page the run post");
    let held = {
        let run = state.runs.get_mut(&run_id).unwrap();
        for turn in 0..250 {
            primary_thread_mut(&mut run.agents).post_user(
                format!("turn {turn}"),
                None,
                now_rfc3339(),
            );
        }
        primary_thread(&run.agents).items.len()
    };

    let posted = state.handle(req(
        "thread.post",
        json!({
            "entity_id": run_id,
            "body": "one more word",
            "thread_limit": crate::thread::DEFAULT_THREAD_PAGE,
        }),
    ));
    assert_eq!(posted["ok"], true, "{posted:?}");
    let thread = &posted["result"]["thread"];
    assert_eq!(
        thread["items"].as_array().unwrap().len(),
        crate::thread::DEFAULT_THREAD_PAGE,
        "{thread:?}"
    );
    assert_eq!(thread["thread_total"], held as u64 + 1, "{thread:?}");
    assert!(
        !thread["items"].to_string().contains("turn 0\""),
        "{thread:?}"
    );
}

#[test]
fn thread_page_walks_backward_to_the_start_of_the_conversation() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let held = run_with_long_conversation(&mut state, "run-walk", 250);

    let mut walked: Vec<u64> = Vec::new();
    let mut before: Option<u64> = None;
    loop {
        let page = state.handle(req(
            "thread.page",
            json!({
                "entity_id": "run-walk",
                "before_sequence": before,
                "limit": 25,
            }),
        ));
        assert_eq!(page["ok"], true, "{page:?}");
        let thread = &page["result"];
        let sequences = page_sequences(thread);
        assert_eq!(sequences.len(), 25, "{thread:?}");
        assert_eq!(thread["thread_total"], held as u64, "{thread:?}");
        let mut older = sequences;
        older.extend(walked);
        walked = older;
        if thread["has_more"] == json!(false) {
            assert!(thread["oldest_sequence"].is_number(), "{thread:?}");
            break;
        }
        before = thread["oldest_sequence"].as_u64();
    }

    // Every item, once, in the order it happened.
    assert_eq!(walked.len(), held);
    let mut ascending = walked.clone();
    ascending.sort_unstable();
    ascending.dedup();
    assert_eq!(walked, ascending);
}

/// A restarted daemon holds the tail of a long conversation, not all of
/// it, so a walk back through one leaves memory and reaches the store.
/// The reviewer scrolling up must not be able to tell: every item, once,
/// in the order it happened, right back to the first thing ever said.
#[test]
fn paging_reaches_the_history_a_restart_never_loaded() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let held = run_with_long_conversation(&mut state, "run-restart", 250);
    let active = state.runs.remove("run-restart").expect("the run is there");
    state
        .persist_run_record("run-restart", &active)
        .expect("the run saves");

    let mut restarted = qa_state(&repo, dir.path());
    let resident = primary_thread(&restarted.runs["run-restart"].agents)
        .items
        .len();
    assert!(
        resident < held,
        "the restart loaded the conversation whole: {resident} of {held}"
    );

    let mut walked: Vec<u64> = Vec::new();
    let mut before: Option<u64> = None;
    loop {
        let page = restarted.handle(req(
            "thread.page",
            json!({
                "entity_id": "run-restart",
                "before_sequence": before,
                "limit": 25,
            }),
        ));
        assert_eq!(page["ok"], true, "{page:?}");
        let thread = &page["result"];
        let sequences = page_sequences(thread);
        assert_eq!(sequences.len(), 25, "{thread:?}");
        assert_eq!(thread["thread_total"], held as u64, "{thread:?}");
        let mut older = sequences;
        older.extend(walked);
        walked = older;
        if thread["has_more"] == json!(false) {
            break;
        }
        before = thread["oldest_sequence"].as_u64();
    }

    let every_sequence: Vec<u64> = (1..=held as u64).collect();
    assert_eq!(
        walked, every_sequence,
        "the walk missed, repeated or reordered the history it read out of the store"
    );
}

#[test]
fn thread_page_clamps_the_limit_it_was_asked_for() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    run_with_long_conversation(&mut state, "run-clamp", 250);

    let greedy = state.handle(req(
        "thread.page",
        json!({ "entity_id": "run-clamp", "limit": 10_000 }),
    ));
    assert_eq!(
        greedy["result"]["items"].as_array().unwrap().len(),
        crate::thread::MAX_THREAD_PAGE,
        "{greedy:?}"
    );

    // A limit of nothing is not an empty page — it is the smallest one.
    let empty = state.handle(req(
        "thread.page",
        json!({ "entity_id": "run-clamp", "limit": 0 }),
    ));
    assert_eq!(empty["result"]["items"].as_array().unwrap().len(), 1);

    // No limit at all is the same page a first load gets.
    let unsaid = state.handle(req("thread.page", json!({ "entity_id": "run-clamp" })));
    assert_eq!(
        unsaid["result"]["items"].as_array().unwrap().len(),
        crate::thread::DEFAULT_THREAD_PAGE
    );

    // Scroll-back reads a page size the way a first load does, so a
    // client whose numbers arrive as strings walks the same history.
    let stringified = state.handle(req(
        "thread.page",
        json!({ "entity_id": "run-clamp", "limit": "120" }),
    ));
    assert_eq!(
        stringified["result"]["items"].as_array().unwrap().len(),
        120,
        "{stringified:?}"
    );
}

#[test]
fn thread_page_reads_the_agent_it_was_addressed_to() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    run_with_long_conversation(&mut state, "run-addressed", 80);
    let second_agent_id = {
        let active = state.runs.get_mut("run-addressed").unwrap();
        let id = active
            .agents
            .add("run-addressed", ModelChoice::default(), &now_rfc3339())
            .id
            .clone();
        active.agents.by_id_mut(&id).unwrap().thread.post_user(
            "only the second agent heard this",
            None,
            now_rfc3339(),
        );
        id
    };

    let addressed = state.handle(req(
        "thread.page",
        json!({ "entity_id": "run-addressed", "agent_id": second_agent_id }),
    ));
    assert_eq!(addressed["result"]["thread_total"], 1, "{addressed:?}");
    assert_eq!(
        addressed["result"]["items"][0]["data"]["body"],
        "only the second agent heard this"
    );
}

#[test]
fn thread_page_of_an_unknown_entity_is_an_error() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let missing = state.handle(req(
        "thread.page",
        json!({ "entity_id": "run-that-never-was" }),
    ));
    assert_eq!(missing["ok"], false, "{missing:?}");
    assert_eq!(missing["error"], "unknown id", "{missing:?}");

    let nameless = state.handle(req("thread.page", json!({})));
    assert_eq!(nameless["ok"], false, "{nameless:?}");
}

#[test]
fn conversation_reads_reject_invalid_or_stale_explicit_identity() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let run_id = adopted_run(&mut state, &repo, dir.path(), "strict-thread-address");
    let agent_id = primary_agent_id(&state, &run_id);
    let conversation_id = state.runs[&run_id]
        .agents
        .by_id(&agent_id)
        .unwrap()
        .conversation_id()
        .to_string();

    for invalid in [json!(""), json!(42)] {
        let refused = state.handle(req(
            "thread.page",
            json!({ "entity_id": run_id, "agent_id": invalid }),
        ));
        assert_eq!(refused["ok"], false, "{refused:?}");
    }
    let stale = state.handle(req(
        "thread.page",
        json!({
            "entity_id": run_id,
            "agent_id": agent_id,
            "conversation_id": "agent-stale",
        }),
    ));
    assert_eq!(stale["ok"], false, "{stale:?}");
    assert!(stale["error"]
        .as_str()
        .unwrap()
        .contains("stale conversation_id"));

    let accepted = state.handle(req(
        "thread.page",
        json!({
            "entity_id": run_id,
            "agent_id": agent_id,
            "conversation_id": conversation_id,
        }),
    ));
    assert_eq!(accepted["ok"], true, "{accepted:?}");
}

/// A conversation with one long run of work in it — the fixture both
/// `thread.activity` paths read — with the run's own span, which is what a
/// client asks for and what the entity's own lifecycle events shift.
fn conversation_with_a_long_run(state: &mut AppState, calls: usize) -> (String, u64, u64) {
    let issue = state
        .plan_create(&json!({ "goal": "trim the retry loop", "dispatch": false }))
        .expect("create a stored legacy plan below the retired RPC boundary");
    let issue_id = issue["plan_id"].as_str().unwrap().to_string();
    let agent_id = primary_agent_id(state, &issue_id);
    state
        .edit_agent_conversation(&issue_id, &agent_id, |thread, _| {
            thread.post_user("go on then", None, "2026-08-29T09:00:00Z");
            for index in 0..calls {
                thread.push_event(
                    crate::thread::ThreadEventKind::ToolUse,
                    Some(format!("Read file-{index}.rs")),
                    None,
                    None,
                    "2026-08-29T09:01:00Z",
                );
            }
            Ok(())
        })
        .expect("the conversation is written");
    let run: Vec<u64> = state
        .agent_conversation(&issue_id, None)
        .expect("the conversation")
        .items
        .iter()
        .filter(|item| item.is_activity())
        .map(crate::thread::ThreadItem::sequence)
        .collect();
    (issue_id, run[0], run[run.len() - 1])
}

/// The span a folded run opens onto, answered the same way wherever the
/// run lives. A client caches a historical run by its `from_sequence`, so
/// the answer cannot depend on how much of the conversation this process
/// happens to hold.
#[test]
fn thread_activity_answers_one_span_the_same_from_memory_and_from_the_store() {
    let (dir, repo) = init_repo();
    let mut resident = qa_state(&repo, dir.path());
    let (issue_id, first_call, _) = conversation_with_a_long_run(&mut resident, 400);
    let through = first_call + 98;
    assert_eq!(
        resident
            .agent_conversation(&issue_id, None)
            .expect("the conversation")
            .resident_from_sequence(),
        0,
        "the state that wrote the conversation holds the whole of it"
    );

    let span = json!({
        "entity_id": issue_id,
        "from_sequence": first_call,
        "through_sequence": through,
    });
    let from_memory = resident.handle(req("thread.activity", span.clone()));

    let mut reloaded = qa_state(&repo, dir.path());
    assert!(
        reloaded
            .agent_conversation(&issue_id, None)
            .expect("the conversation")
            .resident_from_sequence()
            > first_call,
        "the fixture has to put the span under the reloaded tail"
    );
    let from_store = reloaded.handle(req("thread.activity", span));

    assert_eq!(from_memory["ok"], true, "{from_memory:?}");
    assert_eq!(from_store["result"], from_memory["result"]);
    let items = from_store["result"]["items"].as_array().unwrap();
    assert_eq!(items.len(), 99, "the whole span, oldest-first");
    assert_eq!(items[0]["data"]["sequence"], json!(first_call));
    assert_eq!(items[98]["data"]["sequence"], json!(through));
    assert_eq!(from_store["result"]["oldest_sequence"], json!(first_call));
    assert_eq!(from_store["result"]["has_more"], json!(false));
}

/// Paging inside one open run, and what it costs: a span far longer than a
/// page walks backward through `before_sequence`, hands every item back
/// exactly once, and never reads the span it is walking.
#[test]
fn thread_activity_pages_backward_through_a_span_without_reading_it_whole() {
    let (dir, repo) = init_repo();
    let (issue_id, first_call, last_call) = {
        let mut writing = qa_state(&repo, dir.path());
        conversation_with_a_long_run(&mut writing, 400)
    };
    let mut state = qa_state(&repo, dir.path());

    let limit = 20;
    let mut walked: Vec<u64> = Vec::new();
    let mut before: Option<u64> = None;
    loop {
        let mut params = json!({
            "entity_id": issue_id,
            "from_sequence": first_call,
            "through_sequence": last_call,
            "limit": limit,
        });
        if let Some(seek) = before {
            params["before_sequence"] = json!(seek);
        }
        let decoded_before = crate::store::items_decoded();
        let answer = state.handle(req("thread.activity", params));
        let page = &answer["result"];
        let shipped: Vec<u64> = page["items"]
            .as_array()
            .unwrap_or_else(|| panic!("a page came back: {answer:?}"))
            .iter()
            .map(|item| item["data"]["sequence"].as_u64().unwrap())
            .collect();
        assert!(
            crate::store::items_decoded() - decoded_before <= limit + 1,
            "a page of {limit} read {} rows: the span is never read whole",
            crate::store::items_decoded() - decoded_before
        );
        assert!(
            !shipped.is_empty(),
            "an empty page below {before:?}: {page}"
        );
        assert!(shipped.len() <= limit, "a page over its limit: {page}");
        assert_eq!(page["oldest_sequence"].as_u64(), shipped.first().copied());
        walked.splice(0..0, shipped);
        if !page["has_more"].as_bool().unwrap() {
            break;
        }
        before = walked.first().copied();
    }
    assert_eq!(walked, (first_call..=last_call).collect::<Vec<u64>>());
}

/// What a page costs is the bridge's to decide, not the caller's: a call
/// that names no limit gets the default page, and one that names a huge
/// limit gets the cap. Either way the rest of the span is still there.
#[test]
fn thread_activity_ships_the_default_page_and_clamps_a_greedy_one() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (issue_id, first_call, last_call) = conversation_with_a_long_run(&mut state, 700);

    let span = json!({
        "entity_id": issue_id,
        "from_sequence": first_call,
        "through_sequence": last_call,
    });

    let unnamed = state.handle(req("thread.activity", span.clone()));
    let default_page = &unnamed["result"];
    assert_eq!(
        default_page["items"].as_array().map(Vec::len),
        Some(crate::thread::DEFAULT_ACTIVITY_PAGE),
        "{unnamed:?}"
    );
    assert_eq!(default_page["has_more"], json!(true), "{unnamed:?}");

    let mut greedy = span;
    greedy["limit"] = json!(5000);
    let clamped = state.handle(req("thread.activity", greedy));
    let capped_page = &clamped["result"];
    assert_eq!(
        capped_page["items"].as_array().map(Vec::len),
        Some(crate::thread::MAX_ACTIVITY_PAGE),
        "{clamped:?}"
    );
    assert_eq!(capped_page["has_more"], json!(true), "{clamped:?}");
}

/// The two ways a client can ask for work that never happened: on an
/// entity that is not there, and over a span this conversation never
/// reached. Both are errors rather than an empty page, because an empty
/// page is a cacheable answer.
#[test]
fn thread_activity_refuses_an_unknown_entity_and_a_span_off_the_conversation() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (issue_id, first_call, last_call) = conversation_with_a_long_run(&mut state, 4);

    let missing = state.handle(req(
        "thread.activity",
        json!({
            "entity_id": "run-that-never-was",
            "from_sequence": first_call,
            "through_sequence": last_call,
        }),
    ));
    assert_eq!(missing["ok"], false, "{missing:?}");
    assert_eq!(missing["error"], "unknown id", "{missing:?}");

    let past_the_end = state.handle(req(
        "thread.activity",
        json!({
            "entity_id": issue_id,
            "from_sequence": first_call,
            "through_sequence": last_call + 500,
        }),
    ));
    assert_eq!(past_the_end["ok"], false, "{past_the_end:?}");
    assert!(
        past_the_end["error"]
            .as_str()
            .unwrap()
            .contains(&format!("reaches sequence {last_call}")),
        "{past_the_end:?}"
    );

    let backwards = state.handle(req(
        "thread.activity",
        json!({
            "entity_id": issue_id,
            "from_sequence": last_call,
            "through_sequence": first_call,
        }),
    ));
    assert_eq!(backwards["ok"], false, "{backwards:?}");

    let nameless = state.handle(req(
        "thread.activity",
        json!({ "entity_id": issue_id, "through_sequence": last_call }),
    ));
    assert_eq!(nameless["ok"], false, "{nameless:?}");
    assert_eq!(
        nameless["error"], "missing required param: from_sequence",
        "{nameless:?}"
    );
}

/// The failure a client has to be able to tell from an empty run: the span
/// is under this process's tail and the history that holds it cannot be
/// read. An empty page would be cached as "this run had no work in it".
#[test]
fn thread_activity_says_so_when_the_history_it_needs_is_not_stored() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (issue_id, first_call, last_call) = conversation_with_a_long_run(&mut state, 12);
    let agent_id = primary_agent_id(&state, &issue_id);
    state
        .edit_agent_conversation(&issue_id, &agent_id, |thread, _| {
            let tail = thread.items[4..].to_vec();
            let last = thread.last_sequence();
            thread.adopt_stored_tail(tail, 400, last);
            Ok(())
        })
        .expect("the conversation is written");
    state.store = None;

    let answer = state.handle(req(
        "thread.activity",
        json!({
            "entity_id": issue_id,
            "from_sequence": first_call,
            "through_sequence": last_call,
        }),
    ));
    assert_eq!(answer["ok"], false, "{answer:?}");
    assert_eq!(
        answer["error"], "this conversation's history is not stored",
        "{answer:?}"
    );
}

// ---- the forward page a cache-first client syncs on ----------------------

/// A client that holds a conversation up to a sequence asks for what was
/// said after it, oldest first, and is told whether more is waiting.
#[test]
fn thread_page_after_a_cached_sequence_walks_forward_to_the_end() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let held = run_with_long_conversation(&mut state, "run-forward", 250);

    let page = state.handle(req(
        "thread.page",
        json!({ "entity_id": "run-forward", "after_sequence": 100 }),
    ));
    assert_eq!(page["ok"], true, "{page:?}");
    let thread = &page["result"];
    assert_eq!(
        page_sequences(thread),
        (101..=200).collect::<Vec<u64>>(),
        "{thread:?}"
    );
    assert_eq!(thread["has_more"], json!(true), "{thread:?}");
    assert_eq!(thread["oldest_sequence"], json!(101), "{thread:?}");
    assert_eq!(thread["thread_total"], held as u64, "{thread:?}");

    // The tail, asked for from the last item the client was handed.
    let rest = state.handle(req(
        "thread.page",
        json!({ "entity_id": "run-forward", "after_sequence": 240 }),
    ));
    let tail = &rest["result"];
    assert_eq!(
        page_sequences(tail),
        (241..=250).collect::<Vec<u64>>(),
        "{tail:?}"
    );
    assert_eq!(tail["has_more"], json!(false), "{tail:?}");

    // Caught up: an empty page, and nothing more to ask for.
    let caught_up = state.handle(req(
        "thread.page",
        json!({ "entity_id": "run-forward", "after_sequence": held }),
    ));
    assert!(
        caught_up["result"]["items"].as_array().unwrap().is_empty(),
        "{caught_up:?}"
    );
    assert_eq!(caught_up["result"]["has_more"], json!(false));
    assert!(caught_up["result"]["oldest_sequence"].is_null());
}

/// `newest` bounds a returning client's catch-up at the conversation tip,
/// while `has_more` tells it that the old cursor and this window do not abut.
#[test]
fn thread_page_newest_after_a_cached_sequence_returns_the_tip() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let held = run_with_long_conversation(&mut state, "run-newest-forward", 250);

    let page = state.handle(req(
        "thread.page",
        json!({
            "entity_id": "run-newest-forward",
            "after_sequence": 100,
            "newest": true,
        }),
    ));
    assert_eq!(page["ok"], true, "{page:?}");
    let thread = &page["result"];
    assert_eq!(
        page_sequences(thread),
        (151..=250).collect::<Vec<u64>>(),
        "{thread:?}"
    );
    assert_eq!(thread["has_more"], json!(true), "{thread:?}");
    assert_eq!(thread["oldest_sequence"], json!(151), "{thread:?}");
    assert_eq!(thread["thread_total"], held as u64, "{thread:?}");

    let short_delta = state.handle(req(
        "thread.page",
        json!({
            "entity_id": "run-newest-forward",
            "after_sequence": 240,
            "newest": true,
        }),
    ));
    assert_eq!(
        page_sequences(&short_delta["result"]),
        (241..=250).collect::<Vec<u64>>(),
        "{short_delta:?}"
    );
    assert_eq!(short_delta["result"]["has_more"], json!(false));

    let limited = state.handle(req(
        "thread.page",
        json!({
            "entity_id": "run-newest-forward",
            "after_sequence": 100,
            "newest": true,
            "limit": 5,
        }),
    ));
    assert_eq!(
        page_sequences(&limited["result"]),
        (246..=250).collect::<Vec<u64>>(),
        "{limited:?}"
    );
    assert_eq!(limited["result"]["has_more"], json!(true));
}

#[test]
fn thread_page_newest_requires_a_forward_cursor() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    run_with_long_conversation(&mut state, "run-newest-no-cursor", 3);

    let refused = state.handle(req(
        "thread.page",
        json!({ "entity_id": "run-newest-no-cursor", "newest": true }),
    ));
    assert_eq!(refused["ok"], false, "{refused:?}");
    assert_eq!(refused["error_code"], "invalid_params", "{refused:?}");
    assert!(
        refused["error"]
            .as_str()
            .unwrap()
            .contains("newest must be used with after_sequence"),
        "{refused:?}"
    );
}

/// The forward cap is the sync constant, and a client may ask for less of
/// it but never more.
#[test]
fn the_forward_page_is_capped_at_the_latest_thread_items() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    run_with_long_conversation(&mut state, "run-forward-cap", 250);

    let greedy = state.handle(req(
        "thread.page",
        json!({ "entity_id": "run-forward-cap", "after_sequence": 0, "limit": 10_000 }),
    ));
    assert_eq!(
        greedy["result"]["items"].as_array().unwrap().len(),
        crate::app::LATEST_THREAD_ITEMS,
        "{greedy:?}"
    );

    let smaller = state.handle(req(
        "thread.page",
        json!({ "entity_id": "run-forward-cap", "after_sequence": 0, "limit": 5 }),
    ));
    assert_eq!(
        page_sequences(&smaller["result"]),
        (1..=5).collect::<Vec<u64>>(),
        "{smaller:?}"
    );
    assert_eq!(smaller["result"]["has_more"], json!(true));
}

/// Two cursors name two different walks. A verb that guessed which one the
/// caller meant would answer a page nobody asked for.
#[test]
fn thread_page_refuses_both_cursors_at_once() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    run_with_long_conversation(&mut state, "run-two-cursors", 40);

    let refused = state.handle(req(
        "thread.page",
        json!({
            "entity_id": "run-two-cursors",
            "before_sequence": 30,
            "after_sequence": 10,
        }),
    ));
    assert_eq!(refused["ok"], false, "{refused:?}");
    assert_eq!(refused["error_code"], "invalid_params", "{refused:?}");
    assert!(
        refused["error"]
            .as_str()
            .unwrap()
            .contains("exactly one of"),
        "{refused:?}"
    );
}

/// A cursor of the wrong type is refused, never ignored — the typed params
/// are read before the handler sees them, and this pins that they are.
///
/// What it costs if they ever stop being: a quoted sequence dropped on the
/// floor hands the caller the NEWEST page for a forward walk it asked to
/// start at 100, and a cache would write that over its own history without
/// ever knowing it had drifted. The refusal above goes with it — a mistyped
/// `before_sequence` beside a good `after_sequence` would no longer read as
/// two cursors at once.
#[test]
fn thread_page_refuses_a_cursor_that_is_not_a_number() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    run_with_long_conversation(&mut state, "run-mistyped-cursor", 40);

    for params in [
        json!({ "entity_id": "run-mistyped-cursor", "after_sequence": "10" }),
        json!({ "entity_id": "run-mistyped-cursor", "before_sequence": "30" }),
        json!({ "entity_id": "run-mistyped-cursor", "after_sequence": -1 }),
        json!({
            "entity_id": "run-mistyped-cursor",
            "before_sequence": "30",
            "after_sequence": 10,
        }),
    ] {
        let refused = state.handle(req("thread.page", params.clone()));
        assert_eq!(refused["ok"], false, "{params}: {refused:?}");
        assert_eq!(refused["error_code"], "invalid_params", "{refused:?}");
    }

    // Null is how a client with no cursor names one, so it reads as the
    // newest page rather than as a refusal.
    let newest = state.handle(req(
        "thread.page",
        json!({ "entity_id": "run-mistyped-cursor", "before_sequence": Value::Null }),
    ));
    assert_eq!(newest["ok"], true, "{newest:?}");
    assert!(!page_sequences(&newest["result"]).is_empty(), "{newest:?}");
}

/// A restarted daemon holds the tail, not the conversation. A client whose
/// cursor predates the tail must still be walked forward from where it is,
/// item by item, out of the history no load read.
#[test]
fn the_forward_page_reaches_the_history_a_restart_never_loaded() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let held = run_with_long_conversation(&mut state, "run-forward-restart", 250);
    let active = state
        .runs
        .remove("run-forward-restart")
        .expect("the run is there");
    state
        .persist_run_record("run-forward-restart", &active)
        .expect("the run saves");

    let mut restarted = qa_state(&repo, dir.path());
    let resident = primary_thread(&restarted.runs["run-forward-restart"].agents)
        .items
        .len();
    assert!(
        resident < held,
        "the restart loaded the conversation whole: {resident} of {held}"
    );

    let newest = restarted.handle(req(
        "thread.page",
        json!({
            "entity_id": "run-forward-restart",
            "after_sequence": 100,
            "newest": true,
        }),
    ));
    assert_eq!(newest["ok"], true, "{newest:?}");
    assert_eq!(
        page_sequences(&newest["result"]),
        (151..=250).collect::<Vec<u64>>(),
        "{newest:?}"
    );
    assert_eq!(newest["result"]["has_more"], json!(true));

    let mut walked: Vec<u64> = Vec::new();
    let mut after = 0u64;
    loop {
        let page = restarted.handle(req(
            "thread.page",
            json!({ "entity_id": "run-forward-restart", "after_sequence": after }),
        ));
        assert_eq!(page["ok"], true, "{page:?}");
        let thread = &page["result"];
        let sequences = page_sequences(thread);
        assert!(!sequences.is_empty(), "{thread:?}");
        walked.extend(&sequences);
        after = *sequences.last().unwrap();
        if thread["has_more"] == json!(false) {
            break;
        }
    }

    assert_eq!(
        walked,
        (1..=held as u64).collect::<Vec<u64>>(),
        "the forward walk missed, repeated or reordered the stored history"
    );
}

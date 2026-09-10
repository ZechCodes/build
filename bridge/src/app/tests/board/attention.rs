use super::*;

// ---- attention: what the rail orders and colours itself by ---------------

pub(in crate::app::tests) fn board_entry(state: &mut AppState, id: &str) -> Value {
    let board = state.handle(req("board.list", json!({})));
    for key in ["runs", "plans", "external_worktrees"] {
        if let Some(list) = board["result"][key].as_array() {
            for entry in list {
                let entry_id = entry["run_id"]
                    .as_str()
                    .or_else(|| entry["plan_id"].as_str())
                    .or_else(|| entry["worktree_id"].as_str());
                if entry_id == Some(id) {
                    return entry.clone();
                }
            }
        }
    }
    panic!("{id} not on the board: {board:?}");
}

pub(in crate::app::tests) fn attention_of(state: &mut AppState, id: &str) -> Value {
    board_entry(state, id)["attention"].clone()
}

/// Append one item to the conversation an Issue and its implementation
/// share, the way an agent or a lifecycle step would.
pub(in crate::app::tests) fn push_to_issue_conversation(
    state: &mut AppState,
    issue_id: &str,
    write: impl FnOnce(&mut crate::thread::Thread),
) {
    let issue = state.plans.get_mut(issue_id).expect("the issue exists");
    write(issue.agents.sole_thread_mut());
}

/// The whole unread rule in one pass: an agent handing back makes the entry
/// unread and says why, `entity.seen` reads the conversation through, and
/// the work happening afterwards updates the entry silently.
#[test]
fn unread_follows_attention_events_and_entity_seen_clears_it() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (issue_id, run_id) = planned_run_in_review(&mut state, "unread");

    // An agent that reported done has handed back, and nobody has looked.
    let entry = board_entry(&mut state, &run_id);
    assert_eq!(entry["unread"], true, "{entry:?}");
    assert_eq!(entry["needs_attention"], true, "{entry:?}");
    assert!(entry["unread_count"].as_u64().unwrap() >= 1, "{entry:?}");

    let seen = state.handle(req("entity.seen", json!({ "entity_id": run_id })));
    assert_eq!(seen["ok"], true, "{seen:?}");
    let entry = board_entry(&mut state, &run_id);
    assert_eq!(entry["unread"], false, "{entry:?}");
    assert_eq!(entry["unread_count"], 0, "{entry:?}");
    assert!(entry["unread_reason"].is_null(), "{entry:?}");
    assert_eq!(entry["needs_attention"], false, "{entry:?}");

    // The work carrying on is not news.
    push_to_issue_conversation(&mut state, &issue_id, |thread| {
        thread.push_event(
            crate::thread::ThreadEventKind::Committed,
            Some("Committed 3 files".to_string()),
            None,
            None,
            now_rfc3339(),
        );
        thread.push_event(
            crate::thread::ThreadEventKind::RevisionCreated,
            None,
            None,
            None,
            now_rfc3339(),
        );
    });
    let entry = board_entry(&mut state, &run_id);
    assert_eq!(
        entry["unread"], false,
        "status events stay quiet: {entry:?}"
    );

    // The agent handing back is, and the entry says which handoff it was.
    push_to_issue_conversation(&mut state, &issue_id, |thread| {
        thread.push_event(
            crate::thread::ThreadEventKind::Done,
            Some("Implemented the change".to_string()),
            None,
            None,
            now_rfc3339(),
        );
    });
    let entry = board_entry(&mut state, &run_id);
    assert_eq!(entry["unread"], true, "{entry:?}");
    assert_eq!(entry["unread_count"], 1, "{entry:?}");
    assert_eq!(entry["unread_reason"], "done", "{entry:?}");

    // The newest one is what it says, and they accumulate.
    push_to_issue_conversation(&mut state, &issue_id, |thread| {
        thread.post_agent("which name did you want?", None, now_rfc3339());
    });
    let entry = board_entry(&mut state, &run_id);
    assert_eq!(entry["unread_count"], 2, "{entry:?}");
    assert_eq!(entry["unread_reason"], "agent_message", "{entry:?}");
}

/// How far the shared conversation has counted, for a test that wants to
/// name one message by the sequence it landed on.
fn issue_thread_last_sequence(state: &AppState, issue_id: &str) -> u64 {
    state
        .plans
        .get(issue_id)
        .expect("the issue exists")
        .agents
        .sole_thread()
        .last_sequence()
}

/// Reading is per message. A panel whose viewport reached the middle of
/// what arrived says the sequence it got to, and the badge goes on counting
/// everything below it — rather than the reader having to reach the very
/// end before anything clears.
#[test]
fn entity_seen_reads_through_the_sequence_the_reader_names() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (issue_id, run_id) = planned_run_in_review(&mut state, "read through");
    state.handle(req("entity.seen", json!({ "entity_id": run_id })));

    push_to_issue_conversation(&mut state, &issue_id, |thread| {
        thread.post_agent("which name did you want?", None, now_rfc3339());
    });
    let first_question = issue_thread_last_sequence(&state, &issue_id);
    push_to_issue_conversation(&mut state, &issue_id, |thread| {
        thread.post_agent("and which module does it go in?", None, now_rfc3339());
    });
    let entry = board_entry(&mut state, &run_id);
    assert_eq!(entry["unread_count"], 2, "{entry:?}");

    let seen = state.handle(req(
        "entity.seen",
        json!({ "entity_id": run_id, "read_through_sequence": first_question }),
    ));
    assert_eq!(seen["ok"], true, "{seen:?}");
    let entry = board_entry(&mut state, &run_id);
    assert_eq!(
        entry["unread_count"], 1,
        "the message below the viewport is still waiting: {entry:?}"
    );
}

/// A report from behind the cursor moves nothing: a second panel holding an
/// older sequence cannot resurrect a badge the reader already cleared.
#[test]
fn a_read_through_report_behind_the_cursor_moves_nothing() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (issue_id, run_id) = planned_run_in_review(&mut state, "no rewind");
    state.handle(req("entity.seen", json!({ "entity_id": run_id })));

    push_to_issue_conversation(&mut state, &issue_id, |thread| {
        thread.post_agent("which name did you want?", None, now_rfc3339());
    });
    let first_question = issue_thread_last_sequence(&state, &issue_id);
    push_to_issue_conversation(&mut state, &issue_id, |thread| {
        thread.post_agent("and which module does it go in?", None, now_rfc3339());
    });
    state.handle(req("entity.seen", json!({ "entity_id": run_id })));
    assert_eq!(board_entry(&mut state, &run_id)["unread_count"], 0);

    state.handle(req(
        "entity.seen",
        json!({ "entity_id": run_id, "read_through_sequence": first_question }),
    ));
    let entry = board_entry(&mut state, &run_id);
    assert_eq!(entry["unread_count"], 0, "{entry:?}");
}

/// The bubble carries the cursor, so the panel can rule its unread divider
/// and open on the first message the reader has not seen.
#[test]
fn an_agent_bubble_carries_the_read_cursor() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (issue_id, run_id) = planned_run_in_review(&mut state, "cursor on the wire");
    push_to_issue_conversation(&mut state, &issue_id, |thread| {
        thread.post_agent("which name did you want?", None, now_rfc3339());
    });
    let first_question = issue_thread_last_sequence(&state, &issue_id);
    push_to_issue_conversation(&mut state, &issue_id, |thread| {
        thread.post_agent("and which module does it go in?", None, now_rfc3339());
    });

    let unread = board_entry(&mut state, &run_id);
    let bubble = &unread["agents"][0];
    assert_eq!(
        bubble["read_through_sequence"], 0,
        "nothing read yet: {bubble:?}"
    );

    state.handle(req(
        "entity.seen",
        json!({ "entity_id": run_id, "read_through_sequence": first_question }),
    ));
    let read = board_entry(&mut state, &run_id);
    let bubble = &read["agents"][0];
    assert_eq!(
        bubble["read_through_sequence"], first_question,
        "{bubble:?}"
    );
}

/// The reviewer's own messages are not news to the reviewer, and a progress
/// note is the agent saying it is still going — neither pulls anyone in.
#[test]
fn a_reviewers_own_message_and_an_agents_progress_note_leave_the_entry_read() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (issue_id, run_id) = planned_run_in_review(&mut state, "quiet posts");
    state.handle(req("entity.seen", json!({ "entity_id": run_id })));

    let posted = state.handle(req(
        "thread.post",
        json!({ "entity_id": run_id, "body": "please rename the helper" }),
    ));
    assert_eq!(posted["ok"], true, "{posted:?}");
    push_to_issue_conversation(&mut state, &issue_id, |thread| {
        thread.post_agent_progress("still digging", None, now_rfc3339());
    });

    let entry = board_entry(&mut state, &run_id);
    assert_eq!(entry["unread"], false, "{entry:?}");
    assert!(entry["unread_reason"].is_null(), "{entry:?}");
}

/// What reaches the Issue from its implementation: the outcomes, never the
/// progress. The rule is the event class, over every kind there is — so a
/// kind added later cannot quietly start (or stop) travelling.
#[test]
fn only_a_runs_attention_outcomes_reach_the_issue_that_owns_it() {
    for event in crate::thread::ThreadEventKind::ALL {
        assert_eq!(
            run_outcome_mirrors_to_issue(event),
            event.class() == crate::thread::EventClass::Attention,
            "{event:?}"
        );
    }
    for outcome in [
        crate::thread::ThreadEventKind::Done,
        crate::thread::ThreadEventKind::Blocked,
        crate::thread::ThreadEventKind::RunFailed,
        crate::thread::ThreadEventKind::Merged,
        crate::thread::ThreadEventKind::Abandoned,
    ] {
        assert!(run_outcome_mirrors_to_issue(outcome), "{outcome:?}");
    }
    for progress in [
        crate::thread::ThreadEventKind::RunStarted,
        crate::thread::ThreadEventKind::Committed,
        crate::thread::ThreadEventKind::Pushed,
        crate::thread::ThreadEventKind::RevisionCreated,
        crate::thread::ThreadEventKind::StageStarted,
    ] {
        assert!(!run_outcome_mirrors_to_issue(progress), "{progress:?}");
    }
}

/// The Issue is where a planned implementation's outcomes have always
/// landed, and they still land there — as the agent's own message now,
/// needing the human exactly once and saying which outcome it was.
#[test]
fn a_reported_outcome_is_news_on_the_issue_that_owns_the_implementation() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (issue_id, run_id) = planned_run_in_review(&mut state, "outcome on the issue");
    let seen = state.handle(req("entity.seen", json!({ "entity_id": run_id })));
    assert_eq!(seen["ok"], true, "{seen:?}");

    state.on_agent_done(
        &run_id,
        DoneReport {
            phase: DonePhase::Build,
            status: DoneStatus::Blocked,
            summary: "Needs production credentials".into(),
            outputs: DoneOutputs::default(),
        },
    );

    let issue_thread = &state.plans[&issue_id].agents.sole_thread();
    let outcomes: Vec<&crate::thread::ThreadMessage> = issue_thread
        .items
        .iter()
        .filter_map(|item| match item {
            crate::thread::ThreadItem::Message(message) if message.outcome.is_some() => {
                Some(message)
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        outcomes.len(),
        1,
        "one report, one record: {:?}",
        issue_thread.items
    );
    assert_eq!(
        outcomes[0].outcome,
        Some(crate::thread::MessageOutcome::Blocked)
    );
    assert_eq!(outcomes[0].body, "Needs production credentials");
    let packet = issue_thread.catch_up_markdown(40);
    assert!(
        packet.contains("- agent [blocked]: Needs production credentials"),
        "the packet says why the predecessor stopped: {packet}"
    );

    let entry = board_entry(&mut state, &run_id);
    assert_eq!(entry["unread"], true, "{entry:?}");
    assert_eq!(entry["unread_reason"], "blocked", "{entry:?}");
}

/// An issue whose conversation is buried under a session's worth of
/// activity, booted again: the tail the daemon reads holds nothing but
/// tool calls, so the packet has to come from the store or the replacement
/// agent is handed nothing at all.
fn issue_buried_in_activity(state: &mut AppState, goal: &str, said: &str) -> String {
    let issue_id = plan_id_of(&state.handle(req(
        "issue.create",
        json!({ "goal": goal, "dispatch": false }),
    )));
    let agent_id = primary_agent_id(state, &issue_id);
    state
        .edit_agent_conversation(&issue_id, &agent_id, |thread, _| {
            thread.post_user(said, None, "2026-08-29T09:00:00Z");
            for index in 0..crate::store::RESIDENT_CONVERSATION_TAIL + 40 {
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
    issue_id
}

/// The page a reviewer OPENS on is cut by the same gate a scroll is. An
/// issue whose tail holds nothing but tool calls has its words under the
/// tail, so a detail poll that reads only memory hands the reviewer a
/// conversation with nothing said in it — and digests a run it can only
/// see the newest of.
#[test]
fn a_detail_polls_page_reaches_the_words_under_a_starved_tail() {
    let (dir, repo) = init_repo();
    let issue_id = {
        let mut state = qa_state(&repo, dir.path());
        issue_buried_in_activity(
            &mut state,
            "fix the login redirect",
            "the redirect drops the query string",
        )
    };

    let mut state = qa_state(&repo, dir.path());
    let answer = state.handle(req(
        "issue.get",
        json!({ "issue_id": issue_id, "thread_limit": 20 }),
    ));
    let thread = &answer["result"]["thread"];
    let said: Vec<&str> = thread["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|item| item["type"] == "message")
        .map(|item| item["data"]["body"].as_str().unwrap())
        .collect();
    assert_eq!(
        said,
        vec![
            "fix the login redirect",
            "the redirect drops the query string"
        ],
        "the page a reviewer opens on says nothing"
    );
    assert_eq!(
        thread["items"].as_array().unwrap().len(),
        said.len() + crate::thread::PAGE_ACTIVITY_RUN_CAP,
        "the run ships its newest hundred and no more"
    );

    // And the digest over the run covers the WHOLE run, not the slice of
    // it the tail happened to hold.
    let digests = thread["activity_digests"].as_array().unwrap();
    assert_eq!(digests.len(), 1, "{digests:?}");
    assert_eq!(
        digests[0]["tool_calls"],
        crate::store::RESIDENT_CONVERSATION_TAIL as u64 + 40,
        "{digests:?}"
    );
}

/// §6.3's first failure. A conversation is loaded as its newest 200 items,
/// and one session emits hundreds of tool calls — so the agent that boots
/// onto that tail is exactly the one whose messages-only packet finds no
/// messages in it. The words come out of the store instead.
#[test]
fn a_starved_tail_hands_a_resumed_agent_the_words_from_the_store() {
    let (dir, repo) = init_repo();
    let issue_id = {
        let mut state = qa_state(&repo, dir.path());
        issue_buried_in_activity(
            &mut state,
            "fix the login redirect",
            "the redirect drops the query string",
        )
    };

    let state = qa_state(&repo, dir.path());
    let thread = state
        .agent_conversation(&issue_id, None)
        .expect("the issue's conversation");
    assert!(
        thread
            .items
            .iter()
            .all(|item| matches!(item, crate::thread::ThreadItem::Event(_))),
        "the fixture did not starve the tail: {:?}",
        thread.items.first()
    );
    assert_eq!(
        thread.catch_up_markdown(crate::orchestrator::CATCH_UP_MESSAGES),
        "",
        "the tail alone is the empty packet this fixes"
    );

    let packet = state.catch_up_packet(thread, crate::orchestrator::CATCH_UP_MESSAGES);
    assert_eq!(
        packet, "- user: fix the login redirect\n- user: the redirect drops the query string",
        "the packet reads the store when the tail holds no conversation"
    );
}

/// Where the packet is composed: at the door every cold prompt passes, not
/// where the turn was built. A queued turn carries the prompt and the
/// protocol; the conversation is added when it is handed over — which is
/// also why a message posted while the turn waited for the lock is in it.
#[test]
fn the_catch_up_packet_is_composed_when_the_turn_is_delivered() {
    let (dir, repo) = init_repo();
    let issue_id = {
        let mut state = qa_state(&repo, dir.path());
        issue_buried_in_activity(&mut state, "fix the redirect", "keep the query string")
    };
    let mut state = qa_state(&repo, dir.path());

    let posted = state.handle(req(
        "thread.post",
        json!({ "entity_id": issue_id, "body": "start with the router" }),
    ));
    assert_eq!(posted["ok"], true, "{posted:?}");
    let queued = state
        .pending_agent_turns
        .last()
        .expect("the message queued a turn");
    assert!(
        queued.wants_catch_up,
        "a cold prompt wants the conversation"
    );
    assert!(
        !queued.said().cold.contains("Catch-up packet"),
        "the packet is not baked in at queue time: {}",
        queued.said().cold
    );

    let delivered =
        state.cold_prompt_with_catch_up(&queued.owner, &queued.agent_id, &queued.said().cold);
    assert!(
        delivered.contains("Catch-up packet from the durable conversation"),
        "{delivered}"
    );
    assert!(
        delivered.contains("- user: keep the query string"),
        "the words under the tail travel with the turn: {delivered}"
    );
    assert!(
        delivered.contains("- user: start with the router"),
        "so do the words posted while it waited: {delivered}"
    );
    assert!(
        delivered.contains("Build conversation protocol"),
        "the protocol block is still the prompt's own: {delivered}"
    );
}

/// §6.3's second failure, over the wire. A reviewer opening a conversation
/// mid-session used to be handed sixty tool calls with the last thing
/// anyone said somewhere below them. The page's limit buys conversation
/// now — and `has_more` / `oldest_sequence` still mean exactly what a
/// client walking back by sequence needs them to mean, across the seam
/// between the resident tail and the history under it.
#[test]
fn a_page_over_an_activity_heavy_conversation_still_shows_what_was_said() {
    let (dir, repo) = init_repo();
    let issue_id = {
        let mut state = qa_state(&repo, dir.path());
        let issue_id = plan_id_of(&state.handle(req(
            "issue.create",
            json!({ "goal": "trim the retry loop", "dispatch": false }),
        )));
        let agent_id = primary_agent_id(&state, &issue_id);
        state
            .edit_agent_conversation(&issue_id, &agent_id, |thread, _| {
                for turn in 0..40 {
                    thread.post_user(format!("ask {turn}"), None, "2026-08-29T09:00:00Z");
                    for index in 0..6 {
                        thread.push_event(
                            crate::thread::ThreadEventKind::ToolUse,
                            Some(format!("Read file-{turn}-{index}.rs")),
                            None,
                            None,
                            "2026-08-29T09:01:00Z",
                        );
                    }
                }
                Ok(())
            })
            .expect("the conversation is written");
        issue_id
    };
    let mut state = qa_state(&repo, dir.path());
    let held = state
        .agent_conversation(&issue_id, None)
        .expect("the conversation")
        .total_item_count();
    assert!(
        held > crate::store::RESIDENT_CONVERSATION_TAIL as u64,
        "the fixture has to reach under the tail: {held}"
    );

    let first = state.handle(req(
        "thread.page",
        json!({ "entity_id": issue_id, "limit": 5 }),
    ));
    let page = &first["result"];
    let said: Vec<&str> = page["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|item| item["type"] == "message")
        .map(|item| item["data"]["body"].as_str().unwrap())
        .collect();
    assert_eq!(
        said.len(),
        5,
        "the page a reviewer opens on is five turns of conversation: {said:?}"
    );
    assert_eq!(said.last(), Some(&"ask 39"), "{said:?}");

    // And the work between the words is folded rather than shipped row by
    // row: one digest per run, each counting the calls it made.
    let digests = page["activity_digests"].as_array().unwrap();
    assert_eq!(digests.len(), 5, "one per run on the page: {digests:?}");
    assert!(
        digests.iter().all(|digest| digest["tool_calls"] == 6),
        "{digests:?}"
    );

    // And the walk back is whole: every item exactly once, in order, over
    // the seam between the tail and the stored history under it.
    let mut walked: Vec<u64> = Vec::new();
    let mut before: Option<u64> = None;
    loop {
        let mut params = json!({ "entity_id": issue_id, "limit": 5 });
        if let Some(seek) = before {
            params["before_sequence"] = json!(seek);
        }
        let answer = state.handle(req("thread.page", params));
        let page = &answer["result"];
        let shipped: Vec<u64> = page["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["data"]["sequence"].as_u64().unwrap())
            .collect();
        assert!(
            !shipped.is_empty(),
            "an empty page below {before:?}: {page}"
        );
        assert_eq!(
            page["oldest_sequence"].as_u64(),
            shipped.first().copied(),
            "oldest_sequence is the oldest item shipped, rider or not"
        );
        assert_eq!(page["thread_total"], json!(held));
        walked.splice(0..0, shipped);
        if !page["has_more"].as_bool().unwrap() {
            break;
        }
        before = walked.first().copied();
    }
    assert_eq!(walked, (1..=held).collect::<Vec<u64>>());
}

/// The degraded first page. A detail view renders whether or not the store
/// answers, so a first page whose gate said "this reaches the stored
/// history" and then could not read it ships the resident tail instead —
/// and ships it WITHOUT digests. The tail's oldest item can sit mid-run, so
/// a census counted off memory would name an exact number that is short,
/// and a client prints a digest's count as fact. No digest sends the client
/// back to counting the rows it was handed: honest, and visibly a floor.
#[test]
fn a_first_page_that_cannot_reach_the_store_ships_no_activity_digests() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let issue_id = plan_id_of(&state.handle(req(
        "issue.create",
        json!({ "goal": "trim the retry loop", "dispatch": false }),
    )));
    let agent_id = primary_agent_id(&state, &issue_id);
    state
        .edit_agent_conversation(&issue_id, &agent_id, |thread, _| {
            for index in 0..12 {
                thread.push_event(
                    crate::thread::ThreadEventKind::ToolUse,
                    Some(format!("Read file-{index}.rs")),
                    None,
                    None,
                    "2026-08-29T09:01:00Z",
                );
            }
            thread.post_agent("renamed it", None, "2026-08-29T09:02:00Z");
            // The tail as a load leaves it: history underneath it, and an
            // oldest item that sits in the middle of a run — the run runs
            // on below the tail, where only the store can count it.
            let tail = thread.items[1..].to_vec();
            let last = thread.last_sequence();
            thread.adopt_stored_tail(tail, 400, last);
            Ok(())
        })
        .expect("the conversation is written");

    // The store goes away under the page the way a read failure leaves it:
    // the gate still says the history is down there, and nothing answers.
    state.store = None;
    let thread = state
        .agent_conversation(&issue_id, None)
        .expect("the conversation");
    assert!(
        thread.page_reaches_stored_history(None, 5),
        "the fixture has to trip the page gate"
    );
    let page = state.first_thread_page(thread, 5);
    assert_eq!(
        page["activity_digests"],
        json!([]),
        "a page the store could not answer counts nothing: {page}"
    );
    assert!(
        !page["items"].as_array().unwrap().is_empty(),
        "the resident tail still ships: {page}"
    );
}

/// The Issue's conversation is where the human follows the work they asked
/// for, so its implementation being abandoned is news there — and the
/// mirrored event says which implementation it came from.
#[test]
fn abandoning_an_implementation_is_news_on_its_issue() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (issue_id, run_id) = planned_run_in_review(&mut state, "mirror the outcome");
    state.handle(req("entity.seen", json!({ "entity_id": issue_id })));

    // The dedup rule: while the implementation is live the Issue has no row
    // of its own, so nothing mirrored onto it can ask a second time.
    let live = work_item_rows(&mut state);
    assert!(
        !live
            .iter()
            .any(|row| row["kind"] == "issue" && row["issue_id"] == json!(issue_id)),
        "{live:?}"
    );

    let abandoned = state.handle(req("run.abandon", json!({ "run_id": run_id })));
    assert_eq!(abandoned["ok"], true, "{abandoned:?}");

    let issue_thread = &state.plans[&issue_id].agents.sole_thread();
    let mirrored = issue_thread
        .items
        .iter()
        .filter_map(|item| match item {
            crate::thread::ThreadItem::Event(event)
                if event.event == crate::thread::ThreadEventKind::Abandoned =>
            {
                Some(event)
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        mirrored.len(),
        1,
        "one abandon, one mirrored event: {:?}",
        issue_thread.items
    );
    assert!(
        mirrored[0]
            .links
            .contains(&crate::thread::ThreadLink::Implementation {
                issue_id: issue_id.clone(),
                implementation_id: run_id.clone(),
            }),
        "the mirror names the implementation it came from: {:?}",
        mirrored[0]
    );

    // The finished implementation stops speaking for the issue, whose own
    // row now says why it needs reading.
    let rows = work_item_rows(&mut state);
    let issue = rows
        .iter()
        .find(|row| row["kind"] == "issue" && row["issue_id"] == json!(issue_id))
        .unwrap_or_else(|| panic!("the issue has its row back: {rows:?}"));
    assert_eq!(issue["unread"], true, "{issue:?}");
    assert_eq!(issue["unread_reason"], "abandoned", "{issue:?}");
}

/// A branch nobody planned has no Issue to tell. Its own conversation still
/// records the outcome.
#[test]
fn abandoning_a_run_with_no_issue_mirrors_nowhere() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let run_id = adopted_run(&mut state, &repo, dir.path(), "feature-planless");

    let abandoned = state.handle(req("run.abandon", json!({ "run_id": run_id })));
    assert_eq!(abandoned["ok"], true, "{abandoned:?}");
    assert!(state.plans.is_empty(), "adoption mints no issue");
    let own = primary_thread(&state.runs[&run_id].agents);
    assert!(
        own.items.iter().any(|item| matches!(
            item,
            crate::thread::ThreadItem::Event(event)
                if event.event == crate::thread::ThreadEventKind::Abandoned
        )),
        "{:?}",
        own.items
    );
}

/// Reading a stage doc IS engaging with an issue — they are a queue you
/// triage by reading — so it stamps. A run needs an action.
#[test]
fn opening_a_stage_counts_as_touching_an_issue_but_reading_a_run_does_not() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (plan_id, run_id) = planned_run_in_review(&mut state, "attention");

    // A fresh run made by implementing: the implement stamped it.
    assert_eq!(attention_of(&mut state, &run_id)["interacted"], true);

    // Reading the run changes nothing about interaction.
    let before = attention_of(&mut state, &run_id);
    state.handle(req("run.get", json!({ "run_id": run_id })));
    state.handle(req("run.diff", json!({ "run_id": run_id })));
    assert_eq!(
        attention_of(&mut state, &run_id),
        before,
        "reading is not acting"
    );

    // Opening a stage doc stamps the issue.
    let mut fresh = qa_state(&repo, dir.path());
    let plan = fresh.handle(req("plan.create", json!({ "goal": "queue item" })));
    let queued = plan_id_of(&plan);
    assert_eq!(attention_of(&mut fresh, &queued)["interacted"], false);
    fresh.handle(req(
        "plan.stage_doc",
        json!({ "plan_id": queued, "stage_id": "first-half" }),
    ));
    assert_eq!(attention_of(&mut fresh, &queued)["interacted"], true);
    let _ = plan_id;
}

/// A rejected verb never happened, so it cannot count as touching anything.
#[test]
fn a_refused_action_does_not_stamp() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let plan = state.handle(req("plan.create", json!({ "goal": "never approved" })));
    let plan_id = plan_id_of(&plan);
    let refused = state.handle(req(
        "plan.stage_approve",
        json!({ "plan_id": plan_id, "stage_id": "no-such-stage" }),
    ));
    assert_eq!(refused["ok"], false, "{refused:?}");
    assert_eq!(attention_of(&mut state, &plan_id)["interacted"], false);
}

/// A worktree Build cut is something you asked for, so it arrives already
/// touched and surfaces in the rail. One made outside Build waits in the
/// Worktrees row until you act on it here.
#[test]
fn a_build_made_worktree_arrives_touched_and_a_hand_made_one_does_not() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let project_id = state.projects[0].id.clone();

    let created = state.handle(req(
        "worktree.create",
        json!({ "project_id": project_id, "name": "spike" }),
    ));
    let build_made = created["result"]["worktree_id"]
        .as_str()
        .unwrap()
        .to_string();

    add_external_worktree(&repo, dir.path(), "by-hand", "by-hand");
    state.scan_external_worktrees_now(&project_id).unwrap();
    let hand_made = state
        .scan_external_worktrees_now(&project_id)
        .unwrap()
        .into_iter()
        .find(|w| w.branch.as_deref() == Some("by-hand"))
        .expect("the hand-made worktree is discoverable")
        .id;

    assert_eq!(attention_of(&mut state, &build_made)["interacted"], true);
    assert_eq!(attention_of(&mut state, &hand_made)["interacted"], false);
}

/// Seen is versioned: looking at something does not make it seen forever.
#[test]
fn seeing_an_entity_lasts_only_until_it_moves() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (_, run_id) = planned_run_in_review(&mut state, "seen versioning");

    assert_eq!(attention_of(&mut state, &run_id)["seen"], false);
    let seen = state.handle(req("entity.seen", json!({ "entity_id": run_id })));
    assert_eq!(seen["ok"], true, "{seen:?}");
    assert_eq!(attention_of(&mut state, &run_id)["seen"], true);

    // It moves on: unseen again, without anyone clearing a flag.
    std::thread::sleep(Duration::from_millis(1100));
    state.handle(req("run.abandon", json!({ "run_id": run_id })));
    assert_eq!(attention_of(&mut state, &run_id)["seen"], false);
}

/// The pulse means "an agent is working here", which is a different claim
/// from "a tab is open". A shell is never an agent; an agent that has
/// stopped painting is waiting for you, not working; and a dead agent's
/// retained screen is not a heartbeat.
///
/// The signal is read off the worktree's agent TAB now, not off a
/// terminal's kind — a tab is the only place an agent can be, so there is
/// nowhere else for the pulse to come from.
#[tokio::test]
async fn only_a_recently_painting_agent_counts_as_working() {
    let (dir, repo) = init_repo();
    let (state, _handler, root) = agent_tab_fixture(&repo, dir.path(), "run-pulse");
    ensure_agent_tab(
        &state,
        &root,
        "run-pulse",
        &crate::agent::derived_agent_id("run-pulse"),
        &ModelChoice::default(),
        "start",
    )
    .unwrap();

    let key = derived_agent_key(&AppState::canonical_root(&root), "run-pulse");
    let mut s = state.lock().unwrap();
    let agent = s.tabs.get(&key).expect("the agent tab");
    assert!(
        agent_is_working(agent),
        "a freshly spawned agent has just painted"
    );

    // A dead agent's retained screen is not a heartbeat: the tab still holds
    // the last thing it painted, and that is a corpse, not progress.
    let dead = {
        let agent = s.tabs.get_mut(&key).unwrap();
        agent.live = false;
        let dead = agent_is_working(agent);
        agent.live = true; // restore: the next case is about a LIVE agent
        dead
    };
    assert!(!dead, "a dead agent's retained screen is not a heartbeat");

    // Left at its prompt overnight: the tab is live, the process is running,
    // and it has painted nothing since the window closed. That agent is
    // waiting for YOU — a pulse here teaches the human to ignore the pulse.
    let parked = {
        let agent = s.tabs.get_mut(&key).unwrap();
        assert!(
            agent_is_working(agent),
            "still working right up until it falls silent"
        );
        agent
            .session
            .backdate_last_output(AGENT_WORKING_WINDOW + Duration::from_secs(1));
        assert!(agent.live, "the tab is live");
        assert!(
            !matches!(agent.session.status(), AgentStatus::Ended { .. }),
            "and its process still running"
        );
        agent_is_working(agent)
    };
    assert!(
        !parked,
        "an agent parked at its prompt is waiting for you, not working"
    );

    // The human's own shell is never an agent, however busy it looks.
    let shell_root = AppState::canonical_root(&repo);
    let (shell, _rx) = Tab::spawn_shell(
        &shell_harness_spec("/bin/bash"),
        "term-77".to_string(),
        shell_root.clone(),
        terminal_size(80, 24),
    )
    .expect("a shell tab spawns");
    assert!(!agent_is_working(&shell));
    shell.session.end();
}

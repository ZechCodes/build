use super::*;

pub(in crate::app::tests) fn activity_rows(
    thread: &crate::thread::Thread,
) -> Vec<crate::thread::ThreadEvent> {
    thread
        .items
        .iter()
        .filter_map(|item| match item {
            crate::thread::ThreadItem::Event(event)
                if event.event.class() == crate::thread::EventClass::Status
                    && matches!(
                        event.event,
                        crate::thread::ThreadEventKind::Reasoning
                            | crate::thread::ThreadEventKind::ToolUse
                            | crate::thread::ThreadEventKind::ToolResult
                            | crate::thread::ThreadEventKind::Narration
                            | crate::thread::ThreadEventKind::TaskUpdate
                    ) =>
            {
                Some(event.clone())
            }
            _ => None,
        })
        .collect()
}

pub(in crate::app::tests) fn activity_of(
    thread: &crate::thread::Thread,
) -> Vec<(crate::thread::ThreadEventKind, String)> {
    activity_rows(thread)
        .into_iter()
        .map(|event| (event.event, event.summary.unwrap_or_default()))
        .collect()
}

/// What a session with no terminal has instead of a screen: its reasoning,
/// tool calls, narration and background work, landing in the conversation
/// the human already reads.
///
/// The five kinds are `Status`, so an agent thinking out loud moves no
/// unread count — that is the property that makes putting activity in the
/// conversation safe, and it is asserted here rather than assumed.
#[tokio::test]
async fn a_reporting_session_pumps_its_work_into_the_conversation() {
    let (dir, repo) = init_repo();
    let mut app = qa_state(&repo, dir.path());
    let root = insert_run(
        &mut app,
        &repo,
        dir.path(),
        "run-activity",
        RunState::Building,
    );
    let key = insert_dictated_agent_tab(
        &mut app,
        &root,
        "run-activity",
        DictatedSession::reporting(AgentStatus::Working),
    );
    // Everything on the conversation so far has been read, so anything the
    // badge shows after this is the activity's doing.
    app.handle(req("entity.seen", json!({ "entity_id": "run-activity" })));
    let state = app.shared();

    let (activity, subscribed) = broadcast::channel(16);
    spawn_activity_pump(&state, key.clone(), Some(subscribed));
    for reported in [
        crate::harness::AgentActivity::Reasoning {
            summary: "the index is unused".into(),
        },
        crate::harness::AgentActivity::ToolUse {
            call_id: "toolu_1".into(),
            summary: "Read bridge/src/app.rs".into(),
        },
        crate::harness::AgentActivity::ToolResult {
            call_id: "toolu_1".into(),
            outcome: crate::harness::ToolOutcome::Ok,
            summary: "fn main() {}".into(),
        },
        crate::harness::AgentActivity::Narration {
            summary: "dropped the index".into(),
        },
        crate::harness::AgentActivity::TaskUpdate {
            summary: "reindex the archive — started".into(),
        },
    ] {
        activity
            .send(crate::harness::ActivityReport::own_work(reported))
            .expect("the pump is listening");
    }

    // Four rows for five reports: the answer completes the call's row
    // rather than minting one of its own.
    let reported = wait_for(Duration::from_secs(5), || {
        let s = state.lock().unwrap();
        let reported = activity_of(primary_thread(&s.runs["run-activity"].agents));
        (reported.len() == 4).then_some(reported)
    })
    .await
    .expect("the reported work reaches the conversation");
    assert_eq!(
        reported,
        vec![
            (
                crate::thread::ThreadEventKind::Reasoning,
                "the index is unused".to_string()
            ),
            (
                crate::thread::ThreadEventKind::ToolUse,
                "Read bridge/src/app.rs\n→ fn main() {}".to_string()
            ),
            (
                crate::thread::ThreadEventKind::Narration,
                "dropped the index".to_string()
            ),
            (
                crate::thread::ThreadEventKind::TaskUpdate,
                "reindex the archive — started".to_string()
            ),
        ],
        "in the order the agent did them"
    );

    let mut s = state.lock().unwrap();
    let view = run_detail(&mut s, json!({ "run_id": "run-activity" }));
    assert_eq!(
        view["result"]["unread_count"], 0,
        "an agent working is not an agent addressing anyone: {view:?}"
    );
}

/// The death rites a session with no terminal would otherwise fall through.
///
/// The byte pump performs them when the PTY closes — the tab stops being
/// live, the conversation's session lineage ends. A session that paints
/// nothing has no PTY to close, so its activity stream ending is the same
/// moment, and the same two things have to happen: without them a dead
/// agent's tab reads as live and its run stays in session until the idle
/// sweep explains the exit as silence.
#[tokio::test]
async fn the_activity_stream_closing_ends_the_session_the_way_a_pty_eof_does() {
    let (dir, repo) = init_repo();
    let mut app = qa_state(&repo, dir.path());
    let root = insert_run(&mut app, &repo, dir.path(), "run-rites", RunState::Building);
    let key = insert_dictated_agent_tab(
        &mut app,
        &root,
        "run-rites",
        DictatedSession::reporting(AgentStatus::Working),
    );
    let state = app.shared();
    assert_eq!(open_session_count(&state, "run-rites"), 1);

    let (activity, subscribed) = broadcast::channel(4);
    spawn_activity_pump(&state, key.clone(), Some(subscribed));
    drop(activity);

    wait_for(Duration::from_secs(5), || {
        (open_session_count(&state, "run-rites") == 0).then_some(())
    })
    .await
    .expect("the session lineage closes when the stream does");
    let s = state.lock().unwrap();
    assert!(
        !s.session_registry.test_tab(&key).unwrap().live,
        "and the tab stops reading as live, so nothing else has to guess"
    );
    assert!(
        s.session_registry.contains(&key),
        "the tab is RETAINED, exactly as an agent tab whose PTY ended is"
    );
}

/// Put `run_id` on the headless provider, running `spec`.
///
/// Both the launch config a delivery reads (the entity's model choice) and
/// the one the rail reads before there is a session (the agent's own) are
/// set, because a real provider change sets both and a test that moved only
/// one would prove the daemon agrees with itself when it does not.
pub(in crate::app::tests) fn run_on_a_headless_provider(
    state: &Arc<Mutex<AppState>>,
    repo: &std::path::Path,
    run_id: &str,
    spec: HarnessSpec,
) -> ModelChoice {
    let choice = a_headless_provider_running(state, repo, spec);
    let mut s = state.lock().unwrap();
    let run = s.runs.get_mut(run_id).expect("the run");
    run.model_choice = choice.clone();
    run.agents.resolve_mut(None).expect("its agent").choice = choice.clone();
    choice
}

/// The whole path, end to end: a human says something to a run whose
/// provider has no terminal, and what comes back is a conversation.
///
/// Nothing here is hand-built — the daemon picks the session protocol off the
/// provider, opens a real child, hands it the turn as a value, and the
/// activity pump posts what the child reported into the thread the human
/// reads. The child is a fake stream-json harness replaying a recording of
/// what claude says; no model turn is ever run.
///
/// Then it leaves, the way a real one does when its work is over, and the
/// death rites the byte pump owes a PTY are owed here too.
#[tokio::test]
async fn a_headless_agent_turns_a_message_into_activity_and_leaves() {
    let (dir, repo) = init_repo();
    let (state, handler) = shared_state_and_handler(&repo, dir.path());
    let root = insert_run_without_agent(&state, &repo, dir.path().join("side"), "run-headless");
    let key = derived_agent_key(&root, "run-headless");
    use crate::harness::adk::fake;
    run_on_a_headless_provider(
        &state,
        &repo,
        "run-headless",
        fake::stream_json_harness_that_leaves(&[
            fake::THINKING,
            fake::TOOL_USE,
            fake::TOOL_RESULT,
            fake::NARRATION,
            fake::RESULT,
        ]),
    );
    // Everything said so far has been read, so an unread entry after this
    // is the activity's doing.
    let posted = call(
        &handler,
        "thread.post",
        json!({ "entity_id": "run-headless", "body": "drop the index" }),
    );
    assert_eq!(posted["ok"], true, "{posted:?}");
    call(
        &handler,
        "entity.seen",
        json!({ "entity_id": "run-headless" }),
    );

    // Three rows for four protocol lines: the call and its answer are ONE
    // row, updated in place when the answer arrived.
    let reported = wait_for(Duration::from_secs(10), || {
        let s = state.lock().unwrap();
        let reported = activity_of(primary_thread(&s.runs["run-headless"].agents));
        (reported.len() == 3).then_some(reported)
    })
    .await
    .expect("the turn's work reaches the conversation");
    assert_eq!(
        reported,
        vec![
            (
                crate::thread::ThreadEventKind::Reasoning,
                "the index is unused".to_string()
            ),
            (
                crate::thread::ThreadEventKind::ToolUse,
                "Read bridge/src/app.rs\n→ fn main() {}".to_string()
            ),
            (
                crate::thread::ThreadEventKind::Narration,
                "dropped the index".to_string()
            ),
        ],
        "in the order the child reported them, and saying what it said"
    );
    {
        let s = state.lock().unwrap();
        let calls = tool_call_rows(primary_thread(&s.runs["run-headless"].agents));
        assert_eq!(calls.len(), 1, "{calls:?}");
        assert_eq!(calls[0].outcome, Some(crate::thread::ToolCallOutcome::Ok));
        assert!(
            calls[0].updated_sequence > calls[0].sequence,
            "the row was updated in place, so every cursor re-ships it: {:?}",
            calls[0]
        );
    }

    // The child answered its one turn and left. Both rites the byte pump
    // performs on PTY EOF are owed here, and nothing else in the daemon
    // learns a harness died on its own.
    wait_for(Duration::from_secs(10), || {
        let s = state.lock().unwrap();
        let lineage = &primary_thread(&s.runs["run-headless"].agents).sessions;
        // Opened by the cold delivery and closed by the pump — asserted as
        // one thing, because a lineage that was never opened would satisfy
        // "nothing is open" without a rite having been performed.
        (lineage.len() == 1 && lineage[0].ended_at.is_some()).then_some(())
    })
    .await
    .expect("the session Build opened is closed when the child's stream ends");
    assert_eq!(open_session_count(&state, "run-headless"), 0);
    let listed = agent_roster(
        &mut state.lock().unwrap(),
        json!({ "entity_id": "run-headless" }),
    );
    let bubble = &listed["result"]["agents"][0];
    assert_eq!(
        bubble["has_terminal"], false,
        "the rail never offers a basement this provider has none of: {bubble:?}"
    );
    assert_eq!(
        bubble["working"], false,
        "and an agent that has left is not working: {bubble:?}"
    );
    let view = run_detail(
        &mut state.lock().unwrap(),
        json!({ "run_id": "run-headless" }),
    );
    assert_eq!(
        view["result"]["unread_count"], 0,
        "an agent working is not an agent addressing anyone: {view:?}"
    );

    let s = state.lock().unwrap();
    assert!(
        !s.session_registry.test_tab(&key).unwrap().live,
        "the tab stops reading as live"
    );
    assert!(
        s.session_registry.test_tab(&key).unwrap().screen.is_none(),
        "and never had a grid to be retained"
    );
}

/// Put a headless agent on `run_id` running `spec`, deliver `body`, and
/// wait until its tool-call rows read as `want`.
///
/// The wait is for the rows themselves rather than for a clock: the pump
/// runs on its own task, and an update lands on a row the conversation
/// already holds — so a test that slept a guess could not tell an answer
/// that had not arrived yet from one that never would.
async fn tool_calls_after_a_turn(
    state: &Arc<Mutex<AppState>>,
    handler: &FrameHandler,
    repo: &std::path::Path,
    run_id: &str,
    spec: HarnessSpec,
    body: &str,
    want: &[(String, Option<crate::thread::ToolCallOutcome>)],
) {
    run_on_a_headless_provider(state, repo, run_id, spec);
    let posted = call(
        handler,
        "thread.post",
        json!({ "entity_id": run_id, "body": body }),
    );
    assert_eq!(posted["ok"], true, "{posted:?}");
    let rows = wait_for(Duration::from_secs(10), || {
        let s = state.lock().unwrap();
        let rows = tool_calls_of(primary_thread(&s.runs[run_id].agents));
        (rows == want).then_some(rows)
    })
    .await;
    assert!(
        rows.is_some(),
        "the conversation reads {:?}, wanted {want:?}",
        tool_calls_of(primary_thread(&state.lock().unwrap().runs[run_id].agents))
    );
}

/// A failed answer lands on the call it answers like any other, and the
/// failure travels as the outcome — the row itself stays toneless, because
/// a tool that failed is the agent's problem and not a call for the human.
#[tokio::test]
async fn a_failed_answer_closes_the_call_it_answers() {
    let (dir, repo) = init_repo();
    let (state, handler) = shared_state_and_handler(&repo, dir.path());
    insert_run_without_agent(&state, &repo, dir.path().join("side"), "run-failed-tool");
    use crate::harness::adk::fake;
    tool_calls_after_a_turn(
        &state,
        &handler,
        &repo,
        "run-failed-tool",
        fake::stream_json_harness(&[fake::TOOL_USE, fake::ERROR_TOOL_RESULT, fake::RESULT]),
        "read the file",
        &[(
            "Read bridge/src/app.rs\n→ File does not exist. Note: your current working \
             directory is /work."
                .to_string(),
            Some(crate::thread::ToolCallOutcome::Error),
        )],
    )
    .await;

    let view = run_detail(
        &mut state.lock().unwrap(),
        json!({ "run_id": "run-failed-tool" }),
    );
    assert_eq!(
        view["result"]["unread_count"], 0,
        "a failed tool call still asks the human for nothing: {view:?}"
    );
}

/// The pairing is by id, and only by id: two calls answered in the reverse
/// order each land on their own row. Adjacency would have crossed them.
#[tokio::test]
async fn two_calls_answered_out_of_order_land_on_their_own_rows() {
    let (dir, repo) = init_repo();
    let (state, handler) = shared_state_and_handler(&repo, dir.path());
    insert_run_without_agent(&state, &repo, dir.path().join("side"), "run-two-calls");
    use crate::harness::adk::fake;
    tool_calls_after_a_turn(
        &state,
        &handler,
        &repo,
        "run-two-calls",
        fake::stream_json_harness(&[
            fake::TOOL_USE,
            fake::SECOND_TOOL_USE,
            fake::SECOND_TOOL_RESULT,
            fake::TOOL_RESULT,
            fake::RESULT,
        ]),
        "read both",
        &[
            (
                "Read bridge/src/app.rs\n→ fn main() {}".to_string(),
                Some(crate::thread::ToolCallOutcome::Ok),
            ),
            (
                "Read bridge/src/thread.rs\n→ pub struct Thread".to_string(),
                Some(crate::thread::ToolCallOutcome::Ok),
            ),
        ],
    )
    .await;
}

/// The degraded path, kept alive: an answer to a call this session never
/// announced has no row to land on, so it mints the standalone
/// `tool_result` row every answer used to mint. Stored rows must render
/// forever, and this is what still produces one.
#[tokio::test]
async fn an_answer_to_a_call_nobody_announced_mints_a_row_of_its_own() {
    let (dir, repo) = init_repo();
    let (state, handler) = shared_state_and_handler(&repo, dir.path());
    insert_run_without_agent(&state, &repo, dir.path().join("side"), "run-orphan");
    use crate::harness::adk::fake;
    run_on_a_headless_provider(
        &state,
        &repo,
        "run-orphan",
        fake::stream_json_harness(&[fake::ORPHAN_TOOL_RESULT, fake::NARRATION, fake::RESULT]),
    );
    let posted = call(
        &handler,
        "thread.post",
        json!({ "entity_id": "run-orphan", "body": "read the file" }),
    );
    assert_eq!(posted["ok"], true, "{posted:?}");

    let reported = wait_for(Duration::from_secs(10), || {
        let s = state.lock().unwrap();
        let reported = activity_of(primary_thread(&s.runs["run-orphan"].agents));
        (reported.len() == 2).then_some(reported)
    })
    .await
    .expect("the orphan answer reaches the conversation");
    assert_eq!(
        reported[0],
        (
            crate::thread::ThreadEventKind::ToolResult,
            "an answer to nothing".to_string()
        ),
        "exactly the row this kind always minted"
    );
    assert!(
        tool_call_rows(primary_thread(
            &state.lock().unwrap().runs["run-orphan"].agents
        ))
        .is_empty(),
        "and no call row was invented to hang it on"
    );
}

/// A call the interrupted turn left open closes honestly: no answer ever
/// arrived, so the row says which boundary ended it rather than going on
/// claiming to run.
///
/// The second turn is the fence. Its call pairs into a fresh row, which
/// proves the drain emptied both maps — the reader's and the pump's —
/// rather than leaving the first turn's id behind to swallow it.
#[tokio::test]
async fn a_call_an_interrupted_turn_left_open_closes_as_unanswered() {
    let (dir, repo) = init_repo();
    let (state, handler) = shared_state_and_handler(&repo, dir.path());
    insert_run_without_agent(&state, &repo, dir.path().join("side"), "run-unanswered");
    use crate::harness::adk::fake;
    tool_calls_after_a_turn(
        &state,
        &handler,
        &repo,
        "run-unanswered",
        fake::stream_json_harness_turn_by_turn(&[
            &[fake::TOOL_USE, fake::FAILED_RESULT],
            &[
                fake::SECOND_TOOL_USE,
                fake::SECOND_TOOL_RESULT,
                fake::RESULT,
            ],
        ]),
        "read the file",
        &[(
            "Read bridge/src/app.rs\n→ no answer — turn ended".to_string(),
            Some(crate::thread::ToolCallOutcome::Unanswered),
        )],
    )
    .await;

    let posted = call(
        &handler,
        "thread.post",
        json!({ "entity_id": "run-unanswered", "body": "try the other one" }),
    );
    assert_eq!(posted["ok"], true, "{posted:?}");
    let rows = wait_for(Duration::from_secs(10), || {
        let s = state.lock().unwrap();
        let rows = tool_calls_of(primary_thread(&s.runs["run-unanswered"].agents));
        // The call row appears before its result. Wait for the answer that
        // this assertion is about, not just for the second row to exist.
        (rows.len() == 2 && rows[1].1.is_some()).then_some(rows)
    })
    .await
    .expect("the next turn's call pairs into a row of its own");
    assert_eq!(
        rows[1],
        (
            "Read bridge/src/thread.rs\n→ pub struct Thread".to_string(),
            Some(crate::thread::ToolCallOutcome::Ok)
        ),
    );
}

#[tokio::test]
async fn a_subagents_rows_fold_under_the_call_that_spawned_them() {
    let (_dir, state, key) = a_run_with_a_reporting_tab("run-folded");

    let reported =
        crate::harness::adk::reports_minted_by(crate::harness::stream_fixtures::SUBAGENT_FIXTURE);
    let (activity, subscribed) = broadcast::channel(reported.len());
    spawn_activity_pump(&state, key.clone(), Some(subscribed));
    for report in reported {
        activity.send(report).expect("the pump is listening");
    }

    let folded = wait_for(Duration::from_secs(5), || {
        let s = state.lock().unwrap();
        let rows = activity_rows(primary_thread(&s.runs["run-folded"].agents));
        let folded: Vec<crate::thread::ThreadEvent> = rows
            .iter()
            .filter(|row| row.parent_sequence.is_some())
            .cloned()
            .collect();
        (folded.len() == 4).then_some(folded)
    })
    .await
    .expect("the subagent's four rows reach the conversation");

    let spawning_call_row = {
        let s = state.lock().unwrap();
        let thread = primary_thread(&s.runs["run-folded"].agents);
        tool_call_rows(thread)
            .into_iter()
            .find(|row| {
                row.summary
                    .as_deref()
                    .unwrap_or_default()
                    .starts_with("Agent")
            })
            .expect("the Agent call minted a row")
    };
    assert_eq!(
        folded
            .iter()
            .map(|row| (row.event, row.parent_sequence))
            .collect::<Vec<_>>(),
        vec![
            (
                crate::thread::ThreadEventKind::Reasoning,
                Some(spawning_call_row.sequence)
            ),
            (
                crate::thread::ThreadEventKind::ToolUse,
                Some(spawning_call_row.sequence)
            ),
            (
                crate::thread::ThreadEventKind::Reasoning,
                Some(spawning_call_row.sequence)
            ),
            (
                crate::thread::ThreadEventKind::Narration,
                Some(spawning_call_row.sequence)
            ),
        ],
        "every row the subagent minted names the call that spawned it"
    );
    assert_eq!(
        folded[1].outcome,
        Some(crate::thread::ToolCallOutcome::Ok),
        "and the subagent's own call is answered on its own folded row"
    );
    assert_eq!(
        spawning_call_row.outcome,
        Some(crate::thread::ToolCallOutcome::Ok),
        "the Agent call was answered on line 12: {spawning_call_row:?}"
    );

    drop(activity);
    wait_for(Duration::from_secs(5), || {
        (!state
            .lock()
            .unwrap()
            .session_registry
            .test_tab(&key)
            .unwrap()
            .live)
            .then_some(())
    })
    .await
    .expect("the stream closing ends the session");
    let s = state.lock().unwrap();
    let after_close = tool_call_rows(primary_thread(&s.runs["run-folded"].agents))
        .into_iter()
        .find(|row| row.sequence == spawning_call_row.sequence)
        .expect("the Agent call's row is still there");
    assert_eq!(
        after_close.outcome,
        Some(crate::thread::ToolCallOutcome::Ok),
        "a call answered while the session ran is not re-resolved when it ends: {after_close:?}"
    );
    assert!(
        s.session_registry
            .test_tab(&key)
            .unwrap()
            .call_sequences
            .is_empty(),
        "and the session's pairing dies with it"
    );
}

#[tokio::test]
async fn a_report_naming_a_call_with_no_row_lands_flat() {
    let (_dir, state, key) = a_run_with_a_reporting_tab("run-orphan");

    let (activity, subscribed) = broadcast::channel(4);
    spawn_activity_pump(&state, key.clone(), Some(subscribed));
    activity
        .send(crate::harness::ActivityReport {
            activity: crate::harness::AgentActivity::Reasoning {
                summary: "counting the characters".into(),
            },
            parent_call_id: Some("toolu_done".into()),
        })
        .expect("the pump is listening");

    let rows = wait_for(Duration::from_secs(5), || {
        let s = state.lock().unwrap();
        let rows = activity_rows(primary_thread(&s.runs["run-orphan"].agents));
        (!rows.is_empty()).then_some(rows)
    })
    .await
    .expect("the row is minted rather than dropped");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].summary.as_deref(), Some("counting the characters"));
    assert_eq!(
        rows[0].parent_sequence, None,
        "a call with no row of its own can never be a parent: {:?}",
        rows[0]
    );
}

/// The death rites close what the turn boundary never saw. A session that
/// dies over an open call leaves a row claiming to run, and no later event
/// would ever contradict it — so the pump closes it, and closes it BEFORE
/// the session-ended row, so the timeline reads calls-closed-then-session-
/// ended rather than a session ending over work that still claims to run.
#[tokio::test]
async fn the_death_rites_close_the_calls_the_session_died_over() {
    let (_dir, state, key) = a_run_with_a_reporting_tab("run-died");

    let (activity, subscribed) = broadcast::channel(4);
    spawn_activity_pump(&state, key.clone(), Some(subscribed));
    activity
        .send(crate::harness::ActivityReport::own_work(
            crate::harness::AgentActivity::ToolUse {
                call_id: "toolu_1".into(),
                summary: "Bash npm test".into(),
            },
        ))
        .expect("the pump is listening");
    wait_for(Duration::from_secs(5), || {
        let s = state.lock().unwrap();
        (!tool_call_rows(primary_thread(&s.runs["run-died"].agents)).is_empty()).then_some(())
    })
    .await
    .expect("the call reaches the conversation");

    drop(activity);
    let closed = wait_for(Duration::from_secs(5), || {
        let s = state.lock().unwrap();
        let rows = tool_call_rows(primary_thread(&s.runs["run-died"].agents));
        rows.first().filter(|row| row.outcome.is_some()).cloned()
    })
    .await
    .expect("the call the session died over is closed");
    assert_eq!(
        closed.summary.as_deref(),
        Some("Bash npm test\n→ no answer — session ended")
    );
    assert_eq!(
        closed.outcome,
        Some(crate::thread::ToolCallOutcome::Unanswered)
    );

    wait_for(Duration::from_secs(5), || {
        (open_session_count(&state, "run-died") == 0).then_some(())
    })
    .await
    .expect("and the rites otherwise ran exactly as they do today");
    let s = state.lock().unwrap();
    let ended = primary_thread(&s.runs["run-died"].agents)
        .items
        .iter()
        .rev()
        .find_map(|item| match item {
            crate::thread::ThreadItem::Event(event)
                if event.event == crate::thread::ThreadEventKind::SessionEnded =>
            {
                Some(event.sequence)
            }
            _ => None,
        })
        .expect("the session lineage closed");
    assert!(
        closed.updated_sequence < ended,
        "the calls close before the session does: {closed:?} then {ended}"
    );
    assert!(
        s.session_registry
            .test_tab(&key)
            .unwrap()
            .call_sequences
            .is_empty(),
        "and the pairing is cleared beside the tab going not live"
    );
}
